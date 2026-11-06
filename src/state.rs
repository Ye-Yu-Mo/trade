use crate::llm_client::LlmMeta;
use crate::logging;
use crate::types::{Position, TradeResult, TradingDecision};
use anyhow::{Context, Result};
use log::warn;
use serde::Serialize;
use std::fs::{create_dir_all, OpenOptions};
use std::io::Write;
use std::path::Path;

// Task 6.1: 记录交易
pub fn log_trade(trade_result: &TradeResult) -> Result<()> {
    let base_dir = Path::new(logging::logs_directory());
    create_dir_all(base_dir).context("创建logs目录失败")?;

    let mut file = OpenOptions::new()
        .create(true)
        .append(true)
        .open(base_dir.join("trades.jsonl"))
        .context("打开trades.jsonl失败")?;

    let json = serde_json::to_string(trade_result).context("序列化交易结果失败")?;
    writeln!(file, "{}", json).context("写入交易日志失败")?;

    Ok(())
}

// Task 6.2: 记录决策
#[derive(serde::Serialize)]
struct DecisionLog {
    timestamp: i64,
    symbol: String,
    decision: TradingDecision,
    position: Option<Position>,
}

pub fn log_decision(
    symbol: &str,
    decision: &TradingDecision,
    position: &Option<Position>,
) -> Result<()> {
    let base_dir = Path::new(logging::logs_directory());
    create_dir_all(base_dir).context("创建logs目录失败")?;

    let mut file = OpenOptions::new()
        .create(true)
        .append(true)
        .open(base_dir.join("decisions.jsonl"))
        .context("打开decisions.jsonl失败")?;

    let log = DecisionLog {
        timestamp: chrono::Utc::now().timestamp(),
        symbol: symbol.to_string(),
        decision: decision.clone(),
        position: position.clone(),
    };

    let json = serde_json::to_string(&log).context("序列化决策日志失败")?;
    writeln!(file, "{}", json).context("写入决策日志失败")?;

    Ok(())
}

// ===== 运行状态快照（供 Dashboard 读取） =====

#[derive(Debug, Clone, Serialize)]
pub struct AccountStatus {
    pub total_wallet_balance: f64,
    pub available_balance: f64,
}

#[derive(Debug, Clone, Serialize)]
pub struct PositionStatus {
    pub symbol: String,
    pub side: String,
    pub amount: f64,
    pub entry_price: f64,
    pub unrealized_pnl: f64,
}

#[derive(Debug, Clone, Serialize)]
pub struct RiskStatus {
    pub halted: bool,
    pub halt_reason: Option<String>,
    pub peak_equity: f64,
    pub last_equity: f64,
    pub stop_loss_pct: f64,
    pub take_profit_pct: f64,
}

#[derive(Debug, Clone, Serialize)]
pub struct StatusSnapshot {
    pub updated_at: i64,
    pub updated_at_text: String,
    pub account: AccountStatus,
    pub risk: RiskStatus,
    pub positions: Vec<PositionStatus>,
}

/// 原子写入 logs/status.json，避免 Dashboard 读到半个文件。
pub fn write_status(snapshot: &StatusSnapshot) -> Result<()> {
    let dir = Path::new(logging::logs_directory());
    create_dir_all(dir).context("创建logs目录失败")?;

    let path = dir.join("status.json");
    let tmp = dir.join("status.json.tmp");
    let json = serde_json::to_string_pretty(snapshot).context("序列化运行状态失败")?;
    std::fs::write(&tmp, json).context("写入运行状态失败")?;
    std::fs::rename(&tmp, &path).context("替换运行状态失败")?;
    Ok(())
}

/// 每个周期追加一个权益点，供 Dashboard 画曲线。
pub fn append_equity(timestamp: i64, equity: f64) -> Result<()> {
    let dir = Path::new(logging::logs_directory());
    create_dir_all(dir).context("创建logs目录失败")?;

    let mut file = OpenOptions::new()
        .create(true)
        .append(true)
        .open(dir.join("equity.jsonl"))
        .context("打开equity.jsonl失败")?;

    writeln!(
        file,
        "{}",
        serde_json::json!({"timestamp": timestamp, "equity": equity})
    )
    .context("写入权益曲线失败")?;

    Ok(())
}

// ===== Agent 决策链追踪 =====

#[derive(Debug, Clone, Serialize)]
pub struct TraceEvent {
    pub timestamp: i64,
    pub symbol: String,
    pub stage: String,
    /// live / sample / demo
    pub source: String,
    pub model: String,
    pub cached: bool,
    pub latency_ms: u64,
    pub prompt_tokens: u32,
    pub completion_tokens: u32,
    pub output: serde_json::Value,
}

/// 追加一条 Agent 决策链记录，供 Dashboard 展示。
pub fn append_trace(event: &TraceEvent) -> Result<()> {
    let dir = Path::new(logging::logs_directory());
    create_dir_all(dir).context("创建logs目录失败")?;

    let mut file = OpenOptions::new()
        .create(true)
        .append(true)
        .open(dir.join("agent_trace.jsonl"))
        .context("打开agent_trace.jsonl失败")?;

    writeln!(file, "{}", serde_json::to_string(event)?).context("写入Agent trace失败")?;
    Ok(())
}

pub fn log_llm_trace(
    symbol: &str,
    stage: &str,
    source: &str,
    meta: &LlmMeta,
    output: &impl Serialize,
) {
    let event = TraceEvent {
        timestamp: chrono::Utc::now().timestamp(),
        symbol: symbol.to_string(),
        stage: stage.to_string(),
        source: source.to_string(),
        model: meta.model.clone(),
        cached: meta.cached,
        latency_ms: meta.latency_ms,
        prompt_tokens: meta.prompt_tokens,
        completion_tokens: meta.completion_tokens,
        output: serde_json::to_value(output).unwrap_or(serde_json::Value::Null),
    };
    if let Err(err) = append_trace(&event) {
        warn!("写入 Agent trace 失败: {:#}", err);
    }
}

pub fn log_code_trace(symbol: &str, stage: &str, source: &str, output: &impl Serialize) {
    let meta = LlmMeta {
        model: "deterministic".to_string(),
        ..Default::default()
    };
    log_llm_trace(symbol, stage, source, &meta, output);
}

#[cfg(test)]
mod tests {
    #[test]
    fn bundled_trace_sample_is_valid_jsonl() {
        let text = include_str!("../examples/agent_trace.sample.jsonl");
        let events: Vec<serde_json::Value> = text
            .lines()
            .map(|line| serde_json::from_str(line).expect("invalid trace line"))
            .collect();

        assert!(events.len() >= 4);
        assert!(events.iter().all(|event| event["stage"].is_string()));
        assert!(events
            .iter()
            .any(|event| event["stage"] == "trade_executor"));
    }
}
