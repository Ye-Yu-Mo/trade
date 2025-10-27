use serde::{Deserialize, Serialize};

// ===== K线数据 (Task 2.1) =====
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Kline {
    pub timestamp: i64,
    pub open: f64,
    pub high: f64,
    pub low: f64,
    pub close: f64,
    pub volume: f64,
}

// ===== 技术指标 (Task 2.2) =====
#[derive(Debug, Clone, Serialize)]
pub struct TechnicalIndicators {
    pub sma_5: f64,
    pub sma_20: f64,
    pub price_change_1: f64,  // 1周期涨跌幅 (%)
    pub price_change_3: f64,  // 3周期涨跌幅 (%)
    pub volume_ratio: f64,    // 当前成交量/20均量
}

// ===== 交易决策相关类型 (Task 2.3) =====
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct TradingDecision {
    pub signal: Signal,
    pub reason: String,
    pub confidence: Confidence,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "UPPERCASE")]
pub enum Signal {
    Buy,
    Sell,
    Hold,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "UPPERCASE")]
pub enum Confidence {
    High,
    Medium,
    Low,
}

// ===== 持仓和交易结果类型 (Task 2.4) =====
#[derive(Debug, Clone, Serialize)]
pub struct Position {
    pub side: PositionSide,
    pub amount: f64,
    pub entry_price: f64,
    pub unrealized_pnl: f64,
}

#[derive(Debug, Clone, Serialize, PartialEq)]
pub enum PositionSide {
    Long,
    Short,
}

#[derive(Debug, Clone, Serialize)]
pub struct TradeResult {
    pub symbol: String,
    pub action: TradeAction,
    pub price: f64,
    pub amount: f64,
    pub timestamp: i64,
    pub reason: String,
    pub pnl: Option<f64>,
    pub order_details: Option<String>, // 订单执行详情或错误信息
}

#[derive(Debug, Clone, Serialize)]
pub enum TradeAction {
    OpenLong,
    CloseLong,
    OpenShort,
    CloseShort,
    Hold,
}
