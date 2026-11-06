use crate::types::{Position, PositionSide, Signal, TradeAction, TradeResult, TradingDecision};
use anyhow::{anyhow, bail, Context, Result};
use futures::{SinkExt, StreamExt};
use hmac::{Hmac, Mac};
use log::{debug, warn};
use serde::Deserialize;
use serde_json::Value;
use sha2::Sha256;
use std::env;
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};
use tokio::sync::{watch, OnceCell};
use tokio::time::{sleep, timeout, Duration, MissedTickBehavior};
use tokio_tungstenite::connect_async;
use tokio_tungstenite::tungstenite::protocol::Message;

type HmacSha256 = Hmac<Sha256>;

#[derive(Debug, Clone, Copy)]
pub struct SymbolConstraints {
    pub step_size: f64,
    pub min_qty: f64,
    pub max_qty: Option<f64>,
    pub min_notional: f64,
    pub tick_size: f64,
}

#[derive(Debug, Clone, Deserialize)]
#[allow(non_snake_case)]
pub struct AccountInfo {
    pub totalWalletBalance: String,
    pub availableBalance: String,
}

static ACCOUNT_STREAM: OnceCell<AccountStreamHandle> = OnceCell::const_new();

struct AccountStreamHandle {
    sender: Arc<watch::Sender<Option<AccountInfo>>>,
}

impl AccountStreamHandle {
    async fn new(api_key: &str, secret: &str) -> Result<Self> {
        let (sender, _receiver) = watch::channel(None);
        let sender = Arc::new(sender);

        match fetch_account_info_rest(api_key, secret).await {
            Ok(initial) => {
                sender.send_replace(Some(initial));
            }
            Err(err) => {
                warn!("初始账户快照获取失败，将等待 WebSocket 推送: {:#}", err);
            }
        }

        spawn_account_stream(sender.clone(), api_key.to_string(), secret.to_string());

        Ok(Self { sender })
    }

    fn subscribe(&self) -> watch::Receiver<Option<AccountInfo>> {
        self.sender.subscribe()
    }
}

#[derive(Debug, Deserialize)]
struct BinanceOrderResponse {
    #[serde(rename = "orderId")]
    order_id: Option<i64>,
    status: Option<String>,
    #[serde(rename = "avgPrice")]
    avg_price: Option<String>,
    #[serde(rename = "executedQty")]
    executed_qty: Option<String>,
}

/// 市价单的实际成交信息。avg_price 为 None 表示交易所还没返回成交均价。
#[derive(Debug, Clone)]
pub struct OrderFill {
    pub order_id: i64,
    pub status: String,
    pub avg_price: Option<f64>,
    pub executed_qty: Option<f64>,
}

impl std::fmt::Display for OrderFill {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "订单ID:{} 状态:{}", self.order_id, self.status)?;
        if let Some(price) = self.avg_price {
            write!(f, " 均价:{:.6}", price)?;
        }
        if let Some(qty) = self.executed_qty {
            write!(f, " 成交:{:.6}", qty)?;
        }
        Ok(())
    }
}

#[derive(Debug, Deserialize)]
struct BinanceError {
    code: Option<i32>,
    msg: Option<String>,
}

// 根据环境变量选择 Binance URL
fn get_binance_base_url() -> String {
    match env::var("BINANCE_TESTNET").as_deref() {
        Ok("true") => "https://testnet.binancefuture.com".to_string(),
        _ => "https://fapi.binance.com".to_string(),
    }
}

// 生成签名
fn generate_signature(query_string: &str, secret: &str) -> String {
    let mut mac = HmacSha256::new_from_slice(secret.as_bytes()).expect("HMAC 初始化失败");
    mac.update(query_string.as_bytes());
    let result = mac.finalize();
    hex::encode(result.into_bytes())
}

// 获取时间戳
fn get_timestamp() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_millis() as u64
}

#[derive(Debug, Deserialize)]
struct ExchangeInfoResponse {
    symbols: Vec<ExchangeInfoSymbol>,
}

#[derive(Debug, Deserialize)]
struct ExchangeInfoSymbol {
    symbol: String,
    filters: Vec<ExchangeFilter>,
}

#[derive(Debug, Deserialize)]
#[serde(tag = "filterType")]
enum ExchangeFilter {
    #[serde(rename = "LOT_SIZE")]
    LotSize {
        #[serde(rename = "minQty")]
        min_qty: String,
        #[serde(rename = "maxQty")]
        max_qty: String,
        #[serde(rename = "stepSize")]
        step_size: String,
    },
    #[serde(rename = "PRICE_FILTER")]
    PriceFilter {
        #[serde(rename = "tickSize")]
        tick_size: String,
    },
    #[serde(rename = "MIN_NOTIONAL")]
    MinNotional {
        #[serde(rename = "notional")]
        notional: String,
    },
    #[serde(other)]
    Other,
}

fn parse_float(value: &str) -> f64 {
    value.parse::<f64>().unwrap_or(0.0)
}

pub async fn fetch_symbol_constraints(
    symbols: &[String],
) -> Result<std::collections::HashMap<String, SymbolConstraints>> {
    let base_url = get_binance_base_url();
    let mut map = std::collections::HashMap::new();

    for symbol in symbols {
        let url = format!("{}/fapi/v1/exchangeInfo?symbol={}", base_url, symbol);
        let resp: ExchangeInfoResponse = reqwest::get(&url)
            .await
            .with_context(|| format!("获取交易规则失败: {}", symbol))?
            .json()
            .await
            .with_context(|| format!("解析交易规则失败: {}", symbol))?;

        let info = resp
            .symbols
            .into_iter()
            .find(|s| s.symbol == *symbol)
            .with_context(|| format!("交易规则中缺少标的: {}", symbol))?;

        let mut step_size = None;
        let mut min_qty = None;
        let mut max_qty = None;
        let mut min_notional = None;
        let mut tick_size = None;

        for filter in info.filters {
            match filter {
                ExchangeFilter::LotSize {
                    min_qty: min,
                    max_qty: max,
                    step_size: step,
                } => {
                    min_qty = Some(parse_float(&min));
                    let mq = parse_float(&max);
                    max_qty = if mq > 0.0 { Some(mq) } else { None };
                    step_size = Some(parse_float(&step));
                }
                ExchangeFilter::PriceFilter { tick_size: tick } => {
                    tick_size = Some(parse_float(&tick));
                }
                ExchangeFilter::MinNotional { notional } => {
                    min_notional = Some(parse_float(&notional));
                }
                ExchangeFilter::Other => {}
            }
        }

        let constraints = SymbolConstraints {
            step_size: step_size.unwrap_or(0.0),
            min_qty: min_qty.unwrap_or(0.0),
            max_qty,
            min_notional: min_notional.unwrap_or(0.0),
            tick_size: tick_size.unwrap_or(0.0),
        };

        map.insert(symbol.clone(), constraints);
    }

    Ok(map)
}

pub fn quantize_down(value: f64, step: f64) -> f64 {
    if step <= 0.0 {
        return value;
    }
    let steps = (value / step).floor();
    (steps * step).max(0.0)
}

pub fn quantize_up(value: f64, step: f64) -> f64 {
    if step <= 0.0 {
        return value;
    }
    let steps = (value / step).ceil();
    (steps * step).max(0.0)
}

pub fn quantize_price(price: f64, tick_size: f64) -> f64 {
    if tick_size <= 0.0 {
        return price;
    }
    let steps = (price / tick_size).round();
    (steps * tick_size).max(0.0)
}

// Task 5.1: 查询当前持仓
#[derive(Debug, Deserialize)]
#[allow(non_snake_case)]
struct PositionRisk {
    symbol: String,
    positionSide: String,
    positionAmt: String,
    entryPrice: String,
    unRealizedProfit: String,
}

pub async fn get_position(symbol: &str, api_key: &str, secret: &str) -> Result<Option<Position>> {
    let base_url = get_binance_base_url();
    let timestamp = get_timestamp();
    let query_string = format!("timestamp={}", timestamp);
    let signature = generate_signature(&query_string, secret);

    let url = format!(
        "{}/fapi/v2/positionRisk?{}&signature={}",
        base_url, query_string, signature
    );

    let client = reqwest::Client::new();
    let positions: Vec<PositionRisk> = client
        .get(&url)
        .header("X-MBX-APIKEY", api_key)
        .send()
        .await
        .context("查询持仓失败")?
        .json()
        .await
        .context("解析持仓数据失败")?;

    let mut best_position: Option<Position> = None;

    for pos in positions {
        if pos.symbol != symbol {
            continue;
        }

        let amount: f64 = pos.positionAmt.parse().unwrap_or(0.0);
        if amount.abs() < 0.0001 {
            continue;
        }

        let side = match pos.positionSide.as_str() {
            "LONG" => PositionSide::Long,
            "SHORT" => PositionSide::Short,
            _ => {
                if amount > 0.0 {
                    PositionSide::Long
                } else {
                    PositionSide::Short
                }
            }
        };

        let candidate = Position {
            side,
            amount: amount.abs(),
            entry_price: pos.entryPrice.parse().unwrap_or(0.0),
            unrealized_pnl: pos.unRealizedProfit.parse().unwrap_or(0.0),
        };

        let replace = match &best_position {
            None => true,
            Some(existing) => candidate.amount > existing.amount,
        };

        if replace {
            best_position = Some(candidate);
        }
    }

    Ok(best_position)
}

// 通过 REST 接口获取账户信息（备用路径）
async fn fetch_account_info_rest(api_key: &str, secret: &str) -> Result<AccountInfo> {
    let base_url = get_binance_base_url();
    let timestamp = get_timestamp();
    let query_string = format!("timestamp={}", timestamp);
    let signature = generate_signature(&query_string, secret);

    let url = format!(
        "{}/fapi/v2/account?{}&signature={}",
        base_url, query_string, signature
    );

    let client = reqwest::Client::new();
    let response = client
        .get(&url)
        .header("X-MBX-APIKEY", api_key)
        .send()
        .await
        .context("查询账户信息失败")?
        .json::<AccountInfo>()
        .await
        .context("解析账户数据失败")?;

    Ok(response)
}

// 对外暴露：优先返回 WebSocket 推送的账户快照，必要时退回 REST
pub async fn get_account_info(api_key: &str, secret: &str) -> Result<AccountInfo> {
    let handle = ACCOUNT_STREAM
        .get_or_try_init(|| {
            let api_key = api_key.to_string();
            let secret = secret.to_string();
            async move { AccountStreamHandle::new(&api_key, &secret).await }
        })
        .await?;

    let mut receiver = handle.subscribe();

    if let Some(snapshot) = receiver.borrow().clone() {
        return Ok(snapshot);
    }

    let wait_for_update = async {
        loop {
            receiver
                .changed()
                .await
                .map_err(|_| anyhow!("账户状态推送通道已关闭"))?;

            if let Some(snapshot) = receiver.borrow().clone() {
                return Ok(snapshot);
            }
        }
    };

    match timeout(Duration::from_secs(3), wait_for_update).await {
        Ok(result) => result,
        Err(_) => fetch_account_info_rest(api_key, secret).await,
    }
}

fn spawn_account_stream(
    sender: Arc<watch::Sender<Option<AccountInfo>>>,
    api_key: String,
    secret: String,
) {
    tokio::spawn(async move {
        account_stream_loop(sender, api_key, secret).await;
    });
}

async fn account_stream_loop(
    sender: Arc<watch::Sender<Option<AccountInfo>>>,
    api_key: String,
    secret: String,
) {
    let client = reqwest::Client::new();
    let mut backoff = Duration::from_secs(5);

    loop {
        match establish_account_stream(sender.clone(), &client, &api_key, &secret).await {
            Ok(()) => {
                debug!("账户 WebSocket 流结束，准备重连");
                backoff = Duration::from_secs(5);
                sleep(Duration::from_secs(1)).await;
            }
            Err(err) => {
                warn!(
                    "账户 WebSocket 流异常，{} 秒后重连: {:#}",
                    backoff.as_secs(),
                    err
                );
                sleep(backoff).await;
                backoff = (backoff * 2).min(Duration::from_secs(60));
            }
        }
    }
}

async fn establish_account_stream(
    sender: Arc<watch::Sender<Option<AccountInfo>>>,
    client: &reqwest::Client,
    api_key: &str,
    secret: &str,
) -> Result<()> {
    let listen_key = create_listen_key(client, api_key).await?;

    if let Ok(snapshot) = fetch_account_info_rest(api_key, secret).await {
        sender.send_replace(Some(snapshot));
    }

    let ws_url = format!("{}/{}", get_binance_ws_base_url(), listen_key);
    let (mut ws_stream, _) = connect_async(&ws_url)
        .await
        .with_context(|| format!("连接账户 WebSocket 失败: {}", ws_url))?;

    let mut keepalive = tokio::time::interval(Duration::from_secs(30 * 60));
    keepalive.set_missed_tick_behavior(MissedTickBehavior::Delay);

    loop {
        tokio::select! {
            message = ws_stream.next() => {
                match message {
                    Some(Ok(Message::Text(text))) => {
                        match parse_account_stream_event(&text) {
                            Some(AccountStreamEvent::Snapshot(info)) => {
                                sender.send_replace(Some(info));
                            }
                            Some(AccountStreamEvent::ListenKeyExpired) => {
                                return Err(anyhow!("listenKey 已过期"));
                            }
                            None => {}
                        }
                    }
                    Some(Ok(Message::Ping(payload))) => {
                        ws_stream.send(Message::Pong(payload)).await?;
                    }
                    Some(Ok(Message::Pong(_))) => {
                        // ignore
                    }
                    Some(Ok(Message::Binary(_))) => {}
                    Some(Ok(Message::Frame(_))) => {}
                    Some(Ok(Message::Close(frame))) => {
                        return Err(anyhow!("账户 WebSocket 主动关闭: {:?}", frame));
                    }
                    Some(Err(err)) => {
                        return Err(err.into());
                    }
                    None => {
                        return Err(anyhow!("账户 WebSocket 提前结束"));
                    }
                }
            }
            _ = keepalive.tick() => {
                if let Err(err) = keepalive_listen_key(client, &listen_key, api_key).await {
                    warn!("账户 listenKey 保活失败: {:#}", err);
                }
            }
        }
    }
}

async fn create_listen_key(client: &reqwest::Client, api_key: &str) -> Result<String> {
    #[derive(Deserialize)]
    #[allow(non_snake_case)]
    struct ListenKeyResponse {
        listenKey: String,
    }

    let url = format!("{}/fapi/v1/listenKey", get_binance_base_url());
    let response = client
        .post(&url)
        .header("X-MBX-APIKEY", api_key)
        .send()
        .await
        .context("申请 listenKey 失败")?;

    if !response.status().is_success() {
        return Err(anyhow!("申请 listenKey 失败 [{}]", response.status()));
    }

    let body: ListenKeyResponse = response.json().await.context("解析 listenKey 响应失败")?;
    Ok(body.listenKey)
}

async fn keepalive_listen_key(
    client: &reqwest::Client,
    listen_key: &str,
    api_key: &str,
) -> Result<()> {
    let url = format!("{}/fapi/v1/listenKey", get_binance_base_url());
    let response = client
        .put(&url)
        .header("X-MBX-APIKEY", api_key)
        .query(&[("listenKey", listen_key)])
        .send()
        .await
        .context("listenKey 保活请求失败")?;

    if !response.status().is_success() {
        return Err(anyhow!("listenKey 保活失败 [{}]", response.status()));
    }

    Ok(())
}

enum AccountStreamEvent {
    Snapshot(AccountInfo),
    ListenKeyExpired,
}

fn parse_account_stream_event(text: &str) -> Option<AccountStreamEvent> {
    let value: Value = serde_json::from_str(text).ok()?;
    let event_type = value.get("e")?.as_str()?;

    match event_type {
        "ACCOUNT_UPDATE" => extract_account_update(&value),
        "listenKeyExpired" => Some(AccountStreamEvent::ListenKeyExpired),
        _ => None,
    }
}

fn extract_account_update(value: &Value) -> Option<AccountStreamEvent> {
    let data = value.get("a")?;
    let balances = data.get("B")?.as_array()?;

    for balance in balances {
        if balance.get("a")?.as_str()? == "USDT" {
            let total = balance.get("wb")?.as_str()?.to_string();
            let available = balance.get("cw")?.as_str()?.to_string();
            return Some(AccountStreamEvent::Snapshot(AccountInfo {
                totalWalletBalance: total,
                availableBalance: available,
            }));
        }
    }

    None
}

fn get_binance_ws_base_url() -> String {
    match env::var("BINANCE_TESTNET").as_deref() {
        Ok("true") => "wss://stream.binancefuture.com/ws".to_string(),
        _ => "wss://fstream.binance.com/ws".to_string(),
    }
}

// 设置持仓模式为双向 (Hedge Mode)
pub async fn set_dual_position_mode(api_key: &str, secret: &str) -> Result<()> {
    let base_url = get_binance_base_url();
    let timestamp = get_timestamp();
    let query_string = format!("dualSidePosition=true&timestamp={}", timestamp);
    let signature = generate_signature(&query_string, secret);

    let url = format!(
        "{}/fapi/v1/positionSide/dual?{}&signature={}",
        base_url, query_string, signature
    );

    let client = reqwest::Client::new();
    let response = client
        .post(&url)
        .header("X-MBX-APIKEY", api_key)
        .send()
        .await
        .context("设置持仓模式失败")?;

    let status = response.status();
    if !status.is_success() {
        let error_text = response
            .text()
            .await
            .unwrap_or_else(|_| "未知错误".to_string());
        // 如果已经是双向模式,会返回错误但可以忽略
        if error_text.contains("-4059") {
            return Ok(()); // 已经是双向模式
        }
        return Err(anyhow::anyhow!(
            "设置持仓模式失败 [{}]: {}",
            status,
            error_text
        ));
    }

    Ok(())
}

// 设置杠杆倍数
pub async fn set_leverage(symbol: &str, leverage: u32, api_key: &str, secret: &str) -> Result<()> {
    let base_url = get_binance_base_url();
    let timestamp = get_timestamp();
    let query_string = format!(
        "symbol={}&leverage={}&timestamp={}",
        symbol, leverage, timestamp
    );
    let signature = generate_signature(&query_string, secret);

    let url = format!(
        "{}/fapi/v1/leverage?{}&signature={}",
        base_url, query_string, signature
    );

    let client = reqwest::Client::new();
    let response = client
        .post(&url)
        .header("X-MBX-APIKEY", api_key)
        .send()
        .await
        .context("设置杠杆失败")?;

    let status = response.status();
    if !status.is_success() {
        let error_text = response
            .text()
            .await
            .unwrap_or_else(|_| "未知错误".to_string());
        return Err(anyhow::anyhow!("设置杠杆失败 [{}]: {}", status, error_text));
    }

    Ok(())
}

// Task 5.2: 订单执行函数
const MAX_ORDER_RETRIES: u32 = 3;
const ORDER_RETRY_DELAY_MS: u64 = 500;

/// clientOrderId 让重试变成幂等操作：网络抖动时不会重复下单。
fn client_order_id(symbol: &str, tag: &str) -> String {
    format!("t{}-{}-{}", get_timestamp(), symbol, tag)
}

fn parse_order_fill(text: &str) -> Option<OrderFill> {
    let order: BinanceOrderResponse = serde_json::from_str(text).ok()?;
    Some(OrderFill {
        order_id: order.order_id.unwrap_or(0),
        status: order.status.unwrap_or_else(|| "UNKNOWN".to_string()),
        avg_price: order
            .avg_price
            .and_then(|v| v.parse::<f64>().ok())
            .filter(|p| *p > 0.0),
        executed_qty: order
            .executed_qty
            .and_then(|v| v.parse::<f64>().ok())
            .filter(|q| *q > 0.0),
    })
}

async fn place_order(
    symbol: &str,
    side: &str,          // BUY or SELL
    position_side: &str, // LONG or SHORT
    quantity: f64,
    client_order_id: &str,
    api_key: &str,
    secret: &str,
) -> Result<OrderFill> {
    let base_url = get_binance_base_url();
    let client = reqwest::Client::new();
    let mut last_error: Option<anyhow::Error> = None;

    for attempt in 1..=MAX_ORDER_RETRIES {
        let timestamp = get_timestamp();
        let query_string = format!(
            "symbol={}&side={}&positionSide={}&type=MARKET&quantity={}&newClientOrderId={}&timestamp={}",
            symbol, side, position_side, quantity, client_order_id, timestamp
        );
        let signature = generate_signature(&query_string, secret);
        let url = format!(
            "{}/fapi/v1/order?{}&signature={}",
            base_url, query_string, signature
        );

        match client
            .post(&url)
            .header("X-MBX-APIKEY", api_key)
            .send()
            .await
        {
            Ok(response) => {
                let status = response.status();
                let response_text = response.text().await.context("读取响应失败")?;

                if !status.is_success() {
                    let error =
                        if let Ok(parsed) = serde_json::from_str::<BinanceError>(&response_text) {
                            anyhow::anyhow!(
                                "订单失败 [code:{}]: {}",
                                parsed.code.unwrap_or(-1),
                                parsed.msg.unwrap_or_else(|| "未知错误".to_string())
                            )
                        } else {
                            anyhow::anyhow!("订单失败 [{}]: {}", status, response_text)
                        };

                    // 4xx 属于业务错误（余额不足、参数错误等），重试没有意义
                    if status.is_client_error() {
                        return Err(error);
                    }
                    last_error = Some(error);
                } else {
                    let mut fill = parse_order_fill(&response_text).context("解析订单响应失败")?;
                    if fill.avg_price.is_none() && fill.order_id > 0 {
                        match query_order(symbol, fill.order_id, api_key, secret).await {
                            Ok(Some(queried)) => fill = queried,
                            Ok(None) => {}
                            Err(err) => warn!("查询成交价失败，使用下单前价格: {:#}", err),
                        }
                    }
                    return Ok(fill);
                }
            }
            Err(err) => {
                last_error = Some(err.into());
            }
        }

        if attempt < MAX_ORDER_RETRIES {
            warn!(
                "下单失败（第 {}/{} 次），{}ms 后使用同一 clientOrderId 重试",
                attempt, MAX_ORDER_RETRIES, ORDER_RETRY_DELAY_MS
            );
            sleep(Duration::from_millis(ORDER_RETRY_DELAY_MS)).await;
        }
    }

    Err(last_error.unwrap_or_else(|| anyhow::anyhow!("订单失败")))
}

async fn query_order(
    symbol: &str,
    order_id: i64,
    api_key: &str,
    secret: &str,
) -> Result<Option<OrderFill>> {
    let timestamp = get_timestamp();
    let query_string = format!(
        "symbol={}&orderId={}&timestamp={}",
        symbol, order_id, timestamp
    );
    let signature = generate_signature(&query_string, secret);
    let url = format!(
        "{}/fapi/v1/order?{}&signature={}",
        get_binance_base_url(),
        query_string,
        signature
    );

    let response = reqwest::Client::new()
        .get(&url)
        .header("X-MBX-APIKEY", api_key)
        .send()
        .await
        .context("查询订单失败")?;

    if !response.status().is_success() {
        return Ok(None);
    }

    let text = response.text().await.context("读取订单响应失败")?;
    Ok(parse_order_fill(&text))
}

async fn open_long(symbol: &str, amount: f64, api_key: &str, secret: &str) -> Result<OrderFill> {
    let id = client_order_id(symbol, "ol");
    place_order(symbol, "BUY", "LONG", amount, &id, api_key, secret).await
}

async fn close_long(symbol: &str, amount: f64, api_key: &str, secret: &str) -> Result<OrderFill> {
    let id = client_order_id(symbol, "cl");
    place_order(symbol, "SELL", "LONG", amount, &id, api_key, secret).await
}

async fn open_short(symbol: &str, amount: f64, api_key: &str, secret: &str) -> Result<OrderFill> {
    let id = client_order_id(symbol, "os");
    place_order(symbol, "SELL", "SHORT", amount, &id, api_key, secret).await
}

async fn close_short(symbol: &str, amount: f64, api_key: &str, secret: &str) -> Result<OrderFill> {
    let id = client_order_id(symbol, "cs");
    place_order(symbol, "BUY", "SHORT", amount, &id, api_key, secret).await
}

// ===== 交易所条件单：止损 / 止盈 =====
//
// 本地风控每个周期检查一次，极端行情下会滑价。挂 closePosition 条件单作为兜底，
// 由交易所在盘中触发，不受轮询间隔影响。

const PROTECTIVE_PREFIX: &str = "risk-";
const PROTECTIVE_SL_PREFIX: &str = "risk-sl-";
const PROTECTIVE_TP_PREFIX: &str = "risk-tp-";

#[derive(Debug, Deserialize)]
#[allow(non_snake_case)]
struct OpenOrder {
    orderId: i64,
    clientOrderId: String,
    positionSide: String,
    stopPrice: String,
}

#[derive(Debug, Clone, Copy)]
pub struct ProtectiveConfig<'a> {
    pub stop_loss_pct: f64,
    pub take_profit_pct: f64,
    pub trigger_type: &'a str,
}

#[derive(Debug, Default, Clone)]
pub struct ProtectiveOrders {
    pub stop_loss: Option<OrderFill>,
    pub take_profit: Option<OrderFill>,
    pub reused: bool,
}

/// 计算（并量化）止损 / 止盈触发价。
pub fn protective_prices(
    side: &PositionSide,
    entry: f64,
    stop_loss_pct: f64,
    take_profit_pct: f64,
    tick_size: f64,
) -> (Option<f64>, Option<f64>) {
    if entry <= 0.0 {
        return (None, None);
    }

    let stop = if stop_loss_pct > 0.0 {
        let raw = match side {
            PositionSide::Long => entry * (1.0 - stop_loss_pct),
            PositionSide::Short => entry * (1.0 + stop_loss_pct),
        };
        Some(quantize_price(raw, tick_size))
    } else {
        None
    };

    let take = if take_profit_pct > 0.0 {
        let raw = match side {
            PositionSide::Long => entry * (1.0 + take_profit_pct),
            PositionSide::Short => entry * (1.0 - take_profit_pct),
        };
        Some(quantize_price(raw, tick_size))
    } else {
        None
    };

    (stop, take)
}

fn side_label(side: &PositionSide) -> &'static str {
    match side {
        PositionSide::Long => "LONG",
        PositionSide::Short => "SHORT",
    }
}

fn close_side(side: &PositionSide) -> &'static str {
    match side {
        PositionSide::Long => "SELL",
        PositionSide::Short => "BUY",
    }
}

fn protective_order_matches(
    order: &OpenOrder,
    client_prefix: &str,
    position_side: &str,
    price: f64,
    tick_size: f64,
) -> bool {
    if !order.clientOrderId.starts_with(client_prefix) || order.positionSide != position_side {
        return false;
    }
    let Ok(existing) = order.stopPrice.parse::<f64>() else {
        return false;
    };
    let tolerance = if tick_size > 0.0 {
        tick_size
    } else {
        (price.abs() * 1e-9).max(1e-9)
    };
    (existing - price).abs() <= tolerance
}

async fn list_open_orders(symbol: &str, api_key: &str, secret: &str) -> Result<Vec<OpenOrder>> {
    let timestamp = get_timestamp();
    let query_string = format!("symbol={}&timestamp={}", symbol, timestamp);
    let signature = generate_signature(&query_string, secret);
    let url = format!(
        "{}/fapi/v1/openOrders?{}&signature={}",
        get_binance_base_url(),
        query_string,
        signature
    );

    let response = reqwest::Client::new()
        .get(&url)
        .header("X-MBX-APIKEY", api_key)
        .send()
        .await
        .context("查询挂单失败")?;

    let status = response.status();
    if !status.is_success() {
        let body = response.text().await.unwrap_or_default();
        bail!("查询挂单失败 [{}]: {}", status, body);
    }

    response.json().await.context("解析挂单失败")
}

async fn cancel_order(symbol: &str, order_id: i64, api_key: &str, secret: &str) -> Result<()> {
    let timestamp = get_timestamp();
    let query_string = format!(
        "symbol={}&orderId={}&timestamp={}",
        symbol, order_id, timestamp
    );
    let signature = generate_signature(&query_string, secret);
    let url = format!(
        "{}/fapi/v1/order?{}&signature={}",
        get_binance_base_url(),
        query_string,
        signature
    );

    let response = reqwest::Client::new()
        .delete(&url)
        .header("X-MBX-APIKEY", api_key)
        .send()
        .await
        .context("撤销订单失败")?;

    let status = response.status();
    if !status.is_success() {
        let body = response.text().await.unwrap_or_default();
        bail!("撤销订单失败 [{}]: {}", status, body);
    }

    Ok(())
}

/// 撤销该标的所有本机器人挂出的保护单。返回撤销数量。
pub async fn cancel_protective_orders(symbol: &str, api_key: &str, secret: &str) -> Result<usize> {
    let orders = list_open_orders(symbol, api_key, secret).await?;
    let mut cancelled = 0;
    for order in orders {
        if order.clientOrderId.starts_with(PROTECTIVE_PREFIX) {
            cancel_order(symbol, order.orderId, api_key, secret).await?;
            cancelled += 1;
        }
    }
    Ok(cancelled)
}

#[allow(clippy::too_many_arguments)]
async fn place_protective_order(
    symbol: &str,
    side: &str,
    position_side: &str,
    order_type: &str,
    stop_price: f64,
    client_order_id: &str,
    trigger_type: &str,
    api_key: &str,
    secret: &str,
) -> Result<OrderFill> {
    let timestamp = get_timestamp();
    let query_string = format!(
        "symbol={}&side={}&positionSide={}&type={}&stopPrice={}&closePosition=true&workingType={}&newClientOrderId={}&timestamp={}",
        symbol, side, position_side, order_type, stop_price, trigger_type, client_order_id, timestamp
    );
    let signature = generate_signature(&query_string, secret);
    let url = format!(
        "{}/fapi/v1/order?{}&signature={}",
        get_binance_base_url(),
        query_string,
        signature
    );

    let response = reqwest::Client::new()
        .post(&url)
        .header("X-MBX-APIKEY", api_key)
        .send()
        .await
        .context("提交条件单失败")?;

    let status = response.status();
    let text = response.text().await.context("读取条件单响应失败")?;
    if !status.is_success() {
        if let Ok(error) = serde_json::from_str::<BinanceError>(&text) {
            bail!(
                "条件单失败 [code:{}]: {}",
                error.code.unwrap_or(-1),
                error.msg.unwrap_or_else(|| "未知错误".to_string())
            );
        }
        bail!("条件单失败 [{}]: {}", status, text);
    }

    parse_order_fill(&text).context("解析条件单响应失败")
}

/// 确保持仓有对应的止损/止盈条件单。
///
/// - 已存在且触发价一致时复用，避免每轮撤单重挂
/// - 缺失或价格不符时，先撤掉旧的保护单再重挂
pub async fn ensure_protective_orders(
    symbol: &str,
    position: &Position,
    config: &ProtectiveConfig<'_>,
    constraints: &SymbolConstraints,
    api_key: &str,
    secret: &str,
) -> Result<ProtectiveOrders> {
    let (stop_price, take_price) = protective_prices(
        &position.side,
        position.entry_price,
        config.stop_loss_pct,
        config.take_profit_pct,
        constraints.tick_size,
    );

    if stop_price.is_none() && take_price.is_none() {
        return Ok(ProtectiveOrders::default());
    }

    let position_side = side_label(&position.side);
    let existing = list_open_orders(symbol, api_key, secret).await?;

    let stop_valid = stop_price.is_none_or(|price| {
        existing.iter().any(|order| {
            protective_order_matches(
                order,
                PROTECTIVE_SL_PREFIX,
                position_side,
                price,
                constraints.tick_size,
            )
        })
    });
    let take_valid = take_price.is_none_or(|price| {
        existing.iter().any(|order| {
            protective_order_matches(
                order,
                PROTECTIVE_TP_PREFIX,
                position_side,
                price,
                constraints.tick_size,
            )
        })
    });

    if stop_valid && take_valid {
        return Ok(ProtectiveOrders {
            stop_loss: None,
            take_profit: None,
            reused: true,
        });
    }

    for order in &existing {
        if order.clientOrderId.starts_with(PROTECTIVE_PREFIX) {
            if let Err(err) = cancel_order(symbol, order.orderId, api_key, secret).await {
                warn!("撤销旧保护单失败 ({}): {:#}", order.orderId, err);
            }
        }
    }

    let timestamp = get_timestamp();
    let side = close_side(&position.side);
    let mut result = ProtectiveOrders::default();

    if let Some(price) = stop_price {
        let id = format!("{}{}", PROTECTIVE_SL_PREFIX, timestamp);
        result.stop_loss = Some(
            place_protective_order(
                symbol,
                side,
                position_side,
                "STOP_MARKET",
                price,
                &id,
                config.trigger_type,
                api_key,
                secret,
            )
            .await?,
        );
    }

    if let Some(price) = take_price {
        let id = format!("{}{}", PROTECTIVE_TP_PREFIX, timestamp);
        result.take_profit = Some(
            place_protective_order(
                symbol,
                side,
                position_side,
                "TAKE_PROFIT_MARKET",
                price,
                &id,
                config.trigger_type,
                api_key,
                secret,
            )
            .await?,
        );
    }

    Ok(result)
}

fn filled_price(fill: &OrderFill, fallback: f64) -> f64 {
    fill.avg_price.unwrap_or(fallback)
}

fn filled_qty(fill: &OrderFill, fallback: f64) -> f64 {
    fill.executed_qty.unwrap_or(fallback)
}

fn close_pnl(side: &PositionSide, entry: f64, exit: f64, qty: f64) -> f64 {
    match side {
        PositionSide::Long => (exit - entry) * qty,
        PositionSide::Short => (entry - exit) * qty,
    }
}

fn hold_result(symbol: &str, price: f64, timestamp: i64, reason: String) -> TradeResult {
    TradeResult {
        symbol: symbol.to_string(),
        action: TradeAction::Hold,
        price,
        amount: 0.0,
        timestamp,
        reason,
        pnl: None,
        order_details: None,
    }
}

// Task 5.3: 执行交易决策
#[allow(clippy::too_many_arguments)]
pub async fn execute_decision(
    symbol: &str,
    decision: &TradingDecision,
    current_position: &Option<Position>,
    execution_price: f64,
    trade_amount: f64,
    max_position: f64,
    api_key: &str,
    secret: &str,
) -> Result<TradeResult> {
    let timestamp = get_timestamp() as i64;

    match decision.signal {
        Signal::Hold => Ok(hold_result(
            symbol,
            execution_price,
            timestamp,
            decision.reason.clone(),
        )),
        Signal::Close => match current_position {
            None => Ok(hold_result(
                symbol,
                execution_price,
                timestamp,
                format!("{} (无持仓可平)", decision.reason),
            )),
            Some(pos) => {
                let fill = match pos.side {
                    PositionSide::Long => close_long(symbol, pos.amount, api_key, secret).await?,
                    PositionSide::Short => close_short(symbol, pos.amount, api_key, secret).await?,
                };
                let price = filled_price(&fill, execution_price);
                let pnl = close_pnl(&pos.side, pos.entry_price, price, pos.amount);
                Ok(TradeResult {
                    symbol: symbol.to_string(),
                    action: match pos.side {
                        PositionSide::Long => TradeAction::CloseLong,
                        PositionSide::Short => TradeAction::CloseShort,
                    },
                    price,
                    amount: pos.amount,
                    timestamp,
                    reason: format!("{} (平仓盈亏: {:.2})", decision.reason, pnl),
                    pnl: Some(pnl),
                    order_details: Some(fill.to_string()),
                })
            }
        },
        Signal::Buy => match current_position {
            None => {
                let fill = open_long(symbol, trade_amount, api_key, secret).await?;
                Ok(TradeResult {
                    symbol: symbol.to_string(),
                    action: TradeAction::OpenLong,
                    price: filled_price(&fill, execution_price),
                    amount: filled_qty(&fill, trade_amount),
                    timestamp,
                    reason: decision.reason.clone(),
                    pnl: None,
                    order_details: Some(fill.to_string()),
                })
            }
            Some(pos) if pos.side == PositionSide::Short => {
                // 平空 → 开多
                let close = close_short(symbol, pos.amount, api_key, secret).await?;
                let close_price = filled_price(&close, execution_price);
                let pnl = close_pnl(
                    &PositionSide::Short,
                    pos.entry_price,
                    close_price,
                    pos.amount,
                );
                let open = open_long(symbol, trade_amount, api_key, secret).await?;
                Ok(TradeResult {
                    symbol: symbol.to_string(),
                    action: TradeAction::OpenLong,
                    price: filled_price(&open, execution_price),
                    amount: filled_qty(&open, trade_amount),
                    timestamp,
                    reason: format!("{} (平空仓盈亏: {:.2})", decision.reason, pnl),
                    pnl: Some(pnl),
                    order_details: Some(format!("平空:{}, 开多:{}", close, open)),
                })
            }
            Some(pos) => {
                let new_total = pos.amount + trade_amount;
                if new_total > max_position {
                    Ok(hold_result(
                        symbol,
                        execution_price,
                        timestamp,
                        format!(
                            "已达最大持仓 {:.4}/{:.4}，无法加仓",
                            pos.amount, max_position
                        ),
                    ))
                } else {
                    let fill = open_long(symbol, trade_amount, api_key, secret).await?;
                    Ok(TradeResult {
                        symbol: symbol.to_string(),
                        action: TradeAction::OpenLong,
                        price: filled_price(&fill, execution_price),
                        amount: filled_qty(&fill, trade_amount),
                        timestamp,
                        reason: format!(
                            "{} (加仓: {:.4} → {:.4})",
                            decision.reason, pos.amount, new_total
                        ),
                        pnl: None,
                        order_details: Some(fill.to_string()),
                    })
                }
            }
        },
        Signal::Sell => match current_position {
            None => {
                let fill = open_short(symbol, trade_amount, api_key, secret).await?;
                Ok(TradeResult {
                    symbol: symbol.to_string(),
                    action: TradeAction::OpenShort,
                    price: filled_price(&fill, execution_price),
                    amount: filled_qty(&fill, trade_amount),
                    timestamp,
                    reason: decision.reason.clone(),
                    pnl: None,
                    order_details: Some(fill.to_string()),
                })
            }
            Some(pos) if pos.side == PositionSide::Long => {
                // 平多 → 开空
                let close = close_long(symbol, pos.amount, api_key, secret).await?;
                let close_price = filled_price(&close, execution_price);
                let pnl = close_pnl(
                    &PositionSide::Long,
                    pos.entry_price,
                    close_price,
                    pos.amount,
                );
                let open = open_short(symbol, trade_amount, api_key, secret).await?;
                Ok(TradeResult {
                    symbol: symbol.to_string(),
                    action: TradeAction::OpenShort,
                    price: filled_price(&open, execution_price),
                    amount: filled_qty(&open, trade_amount),
                    timestamp,
                    reason: format!("{} (平多仓盈亏: {:.2})", decision.reason, pnl),
                    pnl: Some(pnl),
                    order_details: Some(format!("平多:{}, 开空:{}", close, open)),
                })
            }
            Some(pos) => {
                let new_total = pos.amount + trade_amount;
                if new_total > max_position {
                    Ok(hold_result(
                        symbol,
                        execution_price,
                        timestamp,
                        format!(
                            "已达最大持仓 {:.4}/{:.4}，无法加仓",
                            pos.amount, max_position
                        ),
                    ))
                } else {
                    let fill = open_short(symbol, trade_amount, api_key, secret).await?;
                    Ok(TradeResult {
                        symbol: symbol.to_string(),
                        action: TradeAction::OpenShort,
                        price: filled_price(&fill, execution_price),
                        amount: filled_qty(&fill, trade_amount),
                        timestamp,
                        reason: format!(
                            "{} (加仓: {:.4} → {:.4})",
                            decision.reason, pos.amount, new_total
                        ),
                        pnl: None,
                        order_details: Some(fill.to_string()),
                    })
                }
            }
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn protective_prices_long_and_short() {
        let (sl, tp) = protective_prices(&PositionSide::Long, 100.0, 0.02, 0.05, 0.0);
        assert!((sl.unwrap() - 98.0).abs() < 1e-9);
        assert!((tp.unwrap() - 105.0).abs() < 1e-9);

        let (sl, tp) = protective_prices(&PositionSide::Short, 100.0, 0.02, 0.05, 0.0);
        assert!((sl.unwrap() - 102.0).abs() < 1e-9);
        assert!((tp.unwrap() - 95.0).abs() < 1e-9);
    }

    #[test]
    fn protective_prices_respect_disabled_limits() {
        let (sl, tp) = protective_prices(&PositionSide::Long, 100.0, 0.0, 0.0, 0.0);
        assert!(sl.is_none());
        assert!(tp.is_none());
    }

    #[test]
    fn protective_prices_quantize_to_tick() {
        let (sl, _) = protective_prices(&PositionSide::Long, 100.0, 0.02, 0.0, 0.5);
        assert_eq!(sl, Some(98.0));
    }

    #[test]
    fn matches_only_matching_prefix_side_and_price() {
        let order = OpenOrder {
            orderId: 1,
            clientOrderId: "risk-sl-123".into(),
            positionSide: "LONG".into(),
            stopPrice: "98.0".into(),
        };
        assert!(protective_order_matches(
            &order,
            PROTECTIVE_SL_PREFIX,
            "LONG",
            98.0,
            0.1
        ));
        assert!(!protective_order_matches(
            &order,
            PROTECTIVE_TP_PREFIX,
            "LONG",
            98.0,
            0.1
        ));
        assert!(!protective_order_matches(
            &order,
            PROTECTIVE_SL_PREFIX,
            "SHORT",
            98.0,
            0.1
        ));
        assert!(!protective_order_matches(
            &order,
            PROTECTIVE_SL_PREFIX,
            "LONG",
            97.0,
            0.1
        ));
    }

    #[test]
    fn client_order_id_fits_binance_limit() {
        let id = client_order_id("BTCUSDT", "ol");
        assert!(id.len() <= 36, "id too long: {}", id);
    }
}
