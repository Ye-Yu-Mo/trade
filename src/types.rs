use serde::{Deserialize, Serialize};
use std::fmt;

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
    pub sma_50: f64,
    pub sma_100: f64,
    pub price_change_1: f64,  // 1周期涨跌幅 (%)
    pub price_change_3: f64,  // 3周期涨跌幅 (%)
    pub price_change_6: f64,  // 6周期涨跌幅 (%)
    pub price_change_12: f64, // 12周期涨跌幅 (%)
    pub atr_14: f64,          // 14周期 ATR
    pub atr_percent: f64,     // ATR 占收盘价的比例 (%)
    pub volume_ratio: f64,    // 当前成交量/20均量
}

// ===== 交易决策相关类型 (Task 2.3) =====
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct TradingDecision {
    pub signal: Signal,
    pub reason: String,
    pub confidence: Confidence,
    pub amount: f64, // AI建议的交易数量
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

// ===== 多智能体系统数据结构 =====

// 1. 行情分析员输出
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MarketReport {
    pub trend: TrendDirection,     // 趋势方向
    pub strength: TrendStrength,   // 趋势强度
    pub market_phase: MarketPhase, // 市场阶段
    pub support: f64,              // 支撑位
    pub resistance: f64,           // 压力位
    pub analysis: String,          // 详细分析
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "lowercase")]
pub enum TrendDirection {
    Bullish, // 多头
    Bearish, // 空头
    Neutral, // 中性
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "lowercase")]
pub enum TrendStrength {
    Strong, // 强
    Medium, // 中
    Weak,   // 弱
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "snake_case")]
pub enum MarketPhase {
    Accumulation, // 积累
    Markup,       // 上升
    Distribution, // 分配
    Markdown,     // 下跌
}

// 2. 策略研究员输出
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StrategyAdvice {
    pub action: StrategyAction,            // 建议操作
    pub reasoning: String,                 // 策略逻辑
    pub timing_score: u8,                  // 时机评分 1-10
    pub target_side: Option<PositionSide>, // 建议持仓方向
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "snake_case")]
pub enum StrategyAction {
    OpenLong,      // 开多
    OpenShort,     // 开空
    AddPosition,   // 加仓
    ClosePosition, // 平仓
    Hold,          // 持有
}

// 3. 风险管理员输出
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RiskAssessment {
    pub risk_level: RiskLevel,    // 风险等级
    pub suggested_amount: f64,    // 建议数量
    pub approval: ApprovalStatus, // 审批状态
    pub warnings: Vec<String>,    // 风险警告
    pub reason: String,           // 风险评估理由
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "lowercase")]
pub enum RiskLevel {
    Low,
    Medium,
    High,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "lowercase")]
pub enum ApprovalStatus {
    Approved, // 批准
    Adjusted, // 调整后批准
    Rejected, // 拒绝
}

// ===== 投资组合管理类型 =====

// 投资组合协调员输出
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PortfolioAllocation {
    pub allocations: Vec<SymbolAllocation>, // 每个标的的资金分配
    pub total_available: f64,               // 总可用资金
    pub strategy: PortfolioStrategy,        // 组合策略
    pub reasoning: String,                  // 分配理由
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SymbolAllocation {
    pub symbol: String,                   // 标的名称
    pub allocated_balance: f64,           // 分配的资金
    pub weight: f64,                      // 权重 (0.0-1.0)
    pub priority: AllocationPriority,     // 优先级
    pub max_amount_override: Option<f64>, // 覆盖的最大交易量
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "lowercase")]
pub enum AllocationPriority {
    High,   // 高优先级（强信号）
    Medium, // 中优先级
    Low,    // 低优先级（弱信号）
    Skip,   // 本轮跳过
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "lowercase")]
pub enum PortfolioStrategy {
    Balanced,     // 均衡分配
    Aggressive,   // 激进（集中强信号）
    Conservative, // 保守（分散风险）
}

// ===== 持仓和交易结果类型 (Task 2.4) =====
#[derive(Debug, Clone, Serialize)]
pub struct Position {
    pub side: PositionSide,
    pub amount: f64,
    pub entry_price: f64,
    pub unrealized_pnl: f64,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
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

impl fmt::Display for TradeAction {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let label = match self {
            TradeAction::OpenLong => "OpenLong",
            TradeAction::CloseLong => "CloseLong",
            TradeAction::OpenShort => "OpenShort",
            TradeAction::CloseShort => "CloseShort",
            TradeAction::Hold => "Hold",
        };
        write!(f, "{}", label)
    }
}
