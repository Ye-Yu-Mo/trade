use crate::types::{Position, TradeResult, TradingDecision};
use anyhow::{Context, Result};
use std::fs::{create_dir_all, OpenOptions};
use std::io::Write;

// Task 6.1: 记录交易
pub fn log_trade(trade_result: &TradeResult) -> Result<()> {
    create_dir_all("logs").context("创建logs目录失败")?;

    let mut file = OpenOptions::new()
        .create(true)
        .append(true)
        .open("logs/trades.jsonl")
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
    create_dir_all("logs").context("创建logs目录失败")?;

    let mut file = OpenOptions::new()
        .create(true)
        .append(true)
        .open("logs/decisions.jsonl")
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
