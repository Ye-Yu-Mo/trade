use crate::types::{Position, PositionSide, Signal, TradeAction, TradeResult, TradingDecision};
use anyhow::{anyhow, Context, Result};
use futures::{SinkExt, StreamExt};
use hmac::{Hmac, Mac};
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
use log::{debug, warn};

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
    symbol: Option<String>,
    status: Option<String>,
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
        return Err(anyhow!(
            "申请 listenKey 失败 [{}]",
            response.status()
        ));
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
        return Err(anyhow!(
            "listenKey 保活失败 [{}]",
            response.status()
        ));
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
async fn place_order(
    symbol: &str,
    side: &str,          // BUY or SELL
    position_side: &str, // LONG or SHORT
    quantity: f64,
    api_key: &str,
    secret: &str,
) -> Result<String> {
    let base_url = get_binance_base_url();
    let timestamp = get_timestamp();
    let query_string = format!(
        "symbol={}&side={}&positionSide={}&type=MARKET&quantity={}&timestamp={}",
        symbol, side, position_side, quantity, timestamp
    );
    let signature = generate_signature(&query_string, secret);

    let url = format!(
        "{}/fapi/v1/order?{}&signature={}",
        base_url, query_string, signature
    );

    let client = reqwest::Client::new();
    let response = client
        .post(&url)
        .header("X-MBX-APIKEY", api_key)
        .send()
        .await
        .context("订单请求失败")?;

    let status = response.status();
    let response_text = response.text().await.context("读取响应失败")?;

    if !status.is_success() {
        // 尝试解析错误信息
        if let Ok(error) = serde_json::from_str::<BinanceError>(&response_text) {
            return Err(anyhow::anyhow!(
                "订单失败 [code:{}]: {}",
                error.code.unwrap_or(-1),
                error.msg.unwrap_or_else(|| "未知错误".to_string())
            ));
        } else {
            return Err(anyhow::anyhow!("订单失败 [{}]: {}", status, response_text));
        }
    }

    // 解析成功响应
    if let Ok(order) = serde_json::from_str::<BinanceOrderResponse>(&response_text) {
        Ok(format!(
            "订单ID:{}, 状态:{}",
            order.order_id.unwrap_or(0),
            order.status.unwrap_or_else(|| "UNKNOWN".to_string())
        ))
    } else {
        Ok("订单已提交".to_string())
    }
}

async fn open_long(symbol: &str, amount: f64, api_key: &str, secret: &str) -> Result<String> {
    place_order(symbol, "BUY", "LONG", amount, api_key, secret).await
}

async fn close_long(symbol: &str, amount: f64, api_key: &str, secret: &str) -> Result<String> {
    place_order(symbol, "SELL", "LONG", amount, api_key, secret).await
}

async fn open_short(symbol: &str, amount: f64, api_key: &str, secret: &str) -> Result<String> {
    place_order(symbol, "SELL", "SHORT", amount, api_key, secret).await
}

async fn close_short(symbol: &str, amount: f64, api_key: &str, secret: &str) -> Result<String> {
    place_order(symbol, "BUY", "SHORT", amount, api_key, secret).await
}

// Task 5.3: 执行交易决策
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
        Signal::Hold => {
            return Ok(TradeResult {
                symbol: symbol.to_string(),
                action: TradeAction::Hold,
                price: execution_price,
                amount: 0.0,
                timestamp,
                reason: decision.reason.clone(),
                pnl: None,
                order_details: None,
            });
        }
        Signal::Buy => {
            match current_position {
                None => {
                    // 空仓 → 开多
                    let order_info = open_long(symbol, trade_amount, api_key, secret).await?;
                    Ok(TradeResult {
                        symbol: symbol.to_string(),
                        action: TradeAction::OpenLong,
                        price: execution_price,
                        amount: trade_amount,
                        timestamp,
                        reason: decision.reason.clone(),
                        pnl: None,
                        order_details: Some(order_info),
                    })
                }
                Some(pos) if pos.side == PositionSide::Short => {
                    // 持有空仓 → 平空 → 开多
                    let close_info = close_short(symbol, pos.amount, api_key, secret).await?;
                    let pnl = (pos.entry_price - execution_price) * pos.amount;
                    let open_info = open_long(symbol, trade_amount, api_key, secret).await?;
                    Ok(TradeResult {
                        symbol: symbol.to_string(),
                        action: TradeAction::OpenLong,
                        price: execution_price,
                        amount: trade_amount,
                        timestamp,
                        reason: format!("{} (平空仓盈亏: {:.2})", decision.reason, pnl),
                        pnl: Some(pnl),
                        order_details: Some(format!("平空:{}, 开多:{}", close_info, open_info)),
                    })
                }
                Some(pos) => {
                    // 已持有多仓 → 检查是否可以加仓
                    let new_total = pos.amount + trade_amount;
                    if new_total > max_position {
                        Ok(TradeResult {
                            symbol: symbol.to_string(),
                            action: TradeAction::Hold,
                            price: execution_price,
                            amount: 0.0,
                            timestamp,
                            reason: format!(
                                "已达最大持仓 {:.4}/{:.4}，无法加仓",
                                pos.amount, max_position
                            ),
                            pnl: None,
                            order_details: None,
                        })
                    } else {
                        // 加仓
                        let order_info = open_long(symbol, trade_amount, api_key, secret).await?;
                        Ok(TradeResult {
                            symbol: symbol.to_string(),
                            action: TradeAction::OpenLong,
                            price: execution_price,
                            amount: trade_amount,
                            timestamp,
                            reason: format!(
                                "{} (加仓: {:.4} → {:.4})",
                                decision.reason, pos.amount, new_total
                            ),
                            pnl: None,
                            order_details: Some(order_info),
                        })
                    }
                }
            }
        }
        Signal::Sell => {
            match current_position {
                None => {
                    // 空仓 → 开空
                    let order_info = open_short(symbol, trade_amount, api_key, secret).await?;
                    Ok(TradeResult {
                        symbol: symbol.to_string(),
                        action: TradeAction::OpenShort,
                        price: execution_price,
                        amount: trade_amount,
                        timestamp,
                        reason: decision.reason.clone(),
                        pnl: None,
                        order_details: Some(order_info),
                    })
                }
                Some(pos) if pos.side == PositionSide::Long => {
                    // 持有多仓 → 平多 → 开空
                    let close_info = close_long(symbol, pos.amount, api_key, secret).await?;
                    let pnl = (execution_price - pos.entry_price) * pos.amount;
                    let open_info = open_short(symbol, trade_amount, api_key, secret).await?;
                    Ok(TradeResult {
                        symbol: symbol.to_string(),
                        action: TradeAction::OpenShort,
                        price: execution_price,
                        amount: trade_amount,
                        timestamp,
                        reason: format!("{} (平多仓盈亏: {:.2})", decision.reason, pnl),
                        pnl: Some(pnl),
                        order_details: Some(format!("平多:{}, 开空:{}", close_info, open_info)),
                    })
                }
                Some(pos) => {
                    // 已持有空仓 → 检查是否可以加仓
                    let new_total = pos.amount + trade_amount;
                    if new_total > max_position {
                        Ok(TradeResult {
                            symbol: symbol.to_string(),
                            action: TradeAction::Hold,
                            price: execution_price,
                            amount: 0.0,
                            timestamp,
                            reason: format!(
                                "已达最大持仓 {:.4}/{:.4}，无法加仓",
                                pos.amount, max_position
                            ),
                            pnl: None,
                            order_details: None,
                        })
                    } else {
                        // 加仓
                        let order_info = open_short(symbol, trade_amount, api_key, secret).await?;
                        Ok(TradeResult {
                            symbol: symbol.to_string(),
                            action: TradeAction::OpenShort,
                            price: execution_price,
                            amount: trade_amount,
                            timestamp,
                            reason: format!(
                                "{} (加仓: {:.4} → {:.4})",
                                decision.reason, pos.amount, new_total
                            ),
                            pnl: None,
                            order_details: Some(order_info),
                        })
                    }
                }
            }
        }
    }
}
