// 单次多智能体决策链（dry-run）。
//
// 不连接交易账户、不下单，只根据最新K线走一遍：
// 行情分析 → 策略 → 风控（代码） → 交易员。
// 用途：调试 prompt、观察模型输出、生成 Dashboard 可展示的 trace。

use crate::executor;
use crate::market;
use crate::multi_agent;
use crate::risk::{self, RiskRequest};
use crate::state;
use crate::types::Position;
use anyhow::{bail, Context, Result};

pub struct AnalyzeConfig {
    pub symbol: String,
    pub interval: String,
    pub balance: f64,
    pub position_pct: f64,
    pub api_key: String,
}

pub async fn run(cfg: AnalyzeConfig) -> Result<()> {
    let klines = market::fetch_klines(&cfg.symbol, &cfg.interval, 120).await?;
    if klines.len() < 20 {
        bail!("K线数据不足：{} 根", klines.len());
    }

    let indicators = market::calculate_indicators(&klines)?;
    let last_price = klines.last().map(|k| k.close).context("缺少最新K线")?;
    if last_price <= 0.0 {
        bail!("最新价格无效");
    }

    let constraints = executor::fetch_symbol_constraints(std::slice::from_ref(&cfg.symbol))
        .await?
        .remove(&cfg.symbol)
        .context("缺少交易约束")?;

    let position: Option<Position> = None;
    let allocated = cfg.balance * cfg.position_pct;

    println!();
    println!("=== 多智能体决策链: {} {} ===", cfg.symbol, cfg.interval);
    println!(
        "最新价 {:.2} | 权益 {:.2} USDT | ATR {:.2}% | 单次仓位 {:.0}%",
        last_price,
        cfg.balance,
        indicators.atr_percent,
        cfg.position_pct * 100.0
    );
    println!();

    let (market_report, meta) = multi_agent::market_analyst_analyze(
        &cfg.symbol,
        &cfg.interval,
        &klines,
        &indicators,
        &cfg.api_key,
    )
    .await?;
    state::log_llm_trace(&cfg.symbol, "market_analyst", "live", &meta, &market_report);
    println!(
        "[1/4] 行情分析员 ({}ms, {} tokens): {:?} / {:?} / {:?}",
        meta.latency_ms,
        meta.prompt_tokens + meta.completion_tokens,
        market_report.trend,
        market_report.strength,
        market_report.market_phase
    );
    println!("      {}", market_report.analysis);

    let (strategy, meta) = multi_agent::strategy_researcher_suggest(
        &cfg.symbol,
        &market_report,
        &position,
        &cfg.api_key,
    )
    .await?;
    state::log_llm_trace(&cfg.symbol, "strategy_researcher", "live", &meta, &strategy);
    println!(
        "[2/4] 策略研究员 ({}ms): {:?} | 时机 {}/10 | {}",
        meta.latency_ms, strategy.action, strategy.timing_score, strategy.reasoning
    );

    let risk = risk::assess(&RiskRequest {
        market_report: &market_report,
        strategy: &strategy,
        position: &position,
        equity: cfg.balance,
        allocated_balance: allocated,
        allocated_max_amount: allocated / last_price,
        max_position: allocated / last_price,
        price: last_price,
        atr_percent: indicators.atr_percent,
        constraints: &constraints,
    });
    state::log_code_trace(&cfg.symbol, "risk_manager", "live", &risk);
    println!(
        "[3/4] 风险管理员（代码）: {:?} | 审批 {:?} | 建议 {:.6} | {}",
        risk.risk_level, risk.approval, risk.suggested_amount, risk.reason
    );
    if !risk.warnings.is_empty() {
        println!("      警告: {}", risk.warnings.join("；"));
    }

    let (mut decision, meta) = multi_agent::trade_executor_decide(
        &cfg.symbol,
        &market_report,
        &strategy,
        &risk,
        &cfg.api_key,
    )
    .await?;
    state::log_llm_trace(&cfg.symbol, "trade_executor", "live", &meta, &decision);

    risk::enforce(&mut decision, &risk);
    state::log_code_trace(&cfg.symbol, "final_decision", "live", &decision);

    println!(
        "[4/4] 决策交易员 ({}ms): {:?} | 信心 {:?} | {}",
        meta.latency_ms, decision.signal, decision.confidence, decision.reason
    );
    println!();
    println!(
        "最终决策: {:?}，数量 {:.6}，信心 {:?}",
        decision.signal, decision.amount, decision.confidence
    );
    println!("已写入 logs/agent_trace.jsonl 与 logs/decisions.jsonl");

    state::log_decision(&cfg.symbol, &decision, &position)?;
    Ok(())
}
