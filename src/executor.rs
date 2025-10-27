use crate::types::{Position, PositionSide, Signal, TradeAction, TradeResult, TradingDecision};
use anyhow::{Context, Result};
use hmac::{Hmac, Mac};
use serde::Deserialize;
use sha2::Sha256;
use std::env;
use std::time::{SystemTime, UNIX_EPOCH};

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

// Task 5.1: 查询当前持仓
#[derive(Debug, Deserialize)]
#[allow(non_snake_case)]
struct PositionRisk {
    symbol: String,
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

    for pos in positions {
        if pos.symbol == symbol {
            let amount: f64 = pos.positionAmt.parse().unwrap_or(0.0);
            if amount.abs() < 0.0001 {
                return Ok(None); // 空仓
            }
            let side = if amount > 0.0 {
                PositionSide::Long
            } else {
                PositionSide::Short
            };
            return Ok(Some(Position {
                side,
                amount: amount.abs(),
                entry_price: pos.entryPrice.parse().unwrap_or(0.0),
                unrealized_pnl: pos.unRealizedProfit.parse().unwrap_or(0.0),
            }));
        }
    }

    Ok(None)
}

// 获取账户信息
pub async fn get_account_info(api_key: &str, secret: &str) -> Result<AccountInfo> {
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
    current_price: f64,
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
                price: current_price,
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
                        price: current_price,
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
                    let pnl = (pos.entry_price - current_price) * pos.amount;
                    let open_info = open_long(symbol, trade_amount, api_key, secret).await?;
                    Ok(TradeResult {
                        symbol: symbol.to_string(),
                        action: TradeAction::OpenLong,
                        price: current_price,
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
                            price: current_price,
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
                            price: current_price,
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
                        price: current_price,
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
                    let pnl = (current_price - pos.entry_price) * pos.amount;
                    let open_info = open_short(symbol, trade_amount, api_key, secret).await?;
                    Ok(TradeResult {
                        symbol: symbol.to_string(),
                        action: TradeAction::OpenShort,
                        price: current_price,
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
                            price: current_price,
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
                            price: current_price,
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
