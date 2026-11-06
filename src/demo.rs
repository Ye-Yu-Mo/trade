// 一条命令的离线 Demo。
//
// 不下载数据、不需要 API key：
// 用仓库自带的K线跑一次 SMA 回测，把结果写成 Dashboard 能读的日志，
// 再把一段真实的 Agent 决策记录作为示例放进 trace。

use crate::backtest::{self, BacktestConfig, BacktestResult, SegmentRequest};
use crate::dashboard::{self, DashboardConfig};
use crate::data;
use crate::strategy::Strategy;
use crate::types::TradeAction;
use anyhow::{bail, Context, Result};
use chrono::{TimeZone, Utc};
use serde_json::json;
use std::fmt::Write as _;
use std::fs;
use std::path::{Path, PathBuf};

const DEMO_LOG_DIR: &str = "demo/logs";
const DEMO_BACKTEST_DIR: &str = "demo/backtest";
const SAMPLE_KLINES: &str = "examples/BTCUSDT_1h_sample.json";
const SAMPLE_TRACE: &str = "examples/agent_trace.sample.jsonl";
const INITIAL_BALANCE: f64 = 10_000.0;
const WARMUP: usize = 120;

pub struct DemoConfig {
    pub port: u16,
    pub no_server: bool,
}

pub async fn run(cfg: DemoConfig) -> Result<()> {
    let klines = data::load_kline_file(Path::new(SAMPLE_KLINES))
        .context("读取示例K线失败，请在仓库根目录运行")?;
    if klines.len() < WARMUP + 2 {
        bail!("示例K线不足：{} 根", klines.len());
    }

    let first = klines.first().map(|k| k.timestamp).unwrap_or(0);
    let last = klines.last().map(|k| k.timestamp).unwrap_or(0);

    let backtest_cfg = BacktestConfig {
        symbol: "BTCUSDT".to_string(),
        interval: "1h".to_string(),
        start_ms: first,
        end_ms: last,
        strategy: "sma_cross".to_string(),
        initial_balance: INITIAL_BALANCE,
        position_pct: 0.2,
        leverage: 1.0,
        fee_rate: 0.0005,
        slippage_bps: 2.0,
        stop_loss_pct: 0.02,
        take_profit_pct: 0.05,
        warmup: WARMUP,
        data_dir: PathBuf::from("data/klines"),
        out_dir: PathBuf::from("demo/backtest"),
        max_bars: None,
    };

    let mut strategy = Strategy::sma_cross();
    let result = backtest::run_segment(
        &backtest_cfg,
        &mut strategy,
        &SegmentRequest {
            klines: &klines,
            start_index: WARMUP - 1,
            end_index: klines.len() - 1,
            initial_equity: INITIAL_BALANCE,
        },
    )
    .await?;

    let log_dir = Path::new(DEMO_LOG_DIR);
    seed_logs(log_dir, &result, first, last)?;
    copy_sample_trace(log_dir)?;
    seed_backtest(Path::new(DEMO_BACKTEST_DIR), &result)?;

    println!();
    println!("================ 离线 Demo 就绪 ================");
    println!(
        "标的: BTCUSDT 1h | 策略: SMA5/SMA20 | 区间: {:.0}h",
        (last - first) as f64 / 3_600_000.0
    );
    println!(
        "结果: {:.2} → {:.2} USDT（{:+.2}%），{} 笔交易，最大回撤 {:.2}%",
        result.metrics.initial_equity,
        result.metrics.final_equity,
        result.metrics.total_return_pct,
        result.metrics.trade_count,
        result.metrics.max_drawdown_pct
    );
    println!("数据目录: {}/", DEMO_LOG_DIR);
    println!("（Agent 决策流展示的是仓库内记录的示例 trace，不是本次回测）");
    println!("================================================");

    if cfg.no_server {
        println!();
        println!(
            "跳过 Dashboard。查看: cargo run --release -- dashboard --logs-dir {}",
            DEMO_LOG_DIR
        );
        return Ok(());
    }

    println!("Dashboard: http://127.0.0.1:{}", cfg.port);
    println!("按 Ctrl+C 退出");
    dashboard::serve(
        "127.0.0.1",
        cfg.port,
        DashboardConfig {
            logs_dir: PathBuf::from(DEMO_LOG_DIR),
            backtest_dir: PathBuf::from(DEMO_BACKTEST_DIR),
        },
    )
    .await
}

fn seed_logs(dir: &Path, result: &BacktestResult, first: i64, last: i64) -> Result<()> {
    fs::create_dir_all(dir).context("创建 demo 日志目录失败")?;

    let mut equity = String::new();
    for point in &result.equity_curve {
        writeln!(
            equity,
            "{}",
            json!({"timestamp": point.time, "equity": point.equity})
        )?;
    }
    fs::write(dir.join("equity.jsonl"), equity).context("写入 demo 权益曲线失败")?;

    let mut trades = String::new();
    for fill in &result.trades {
        let entry = json!({
            "symbol": result.config.symbol,
            "action": format!("{}", fill.action),
            "price": fill.price,
            "amount": fill.qty,
            "timestamp": fill.time,
            "reason": fill.reason,
            "pnl": fill.pnl,
            "order_details": format!("手续费 {:.4} USDT", fill.fee),
        });
        writeln!(trades, "{}", entry)?;
    }
    fs::write(dir.join("trades.jsonl"), trades).context("写入 demo 成交失败")?;

    let mut decisions = String::new();
    for fill in &result.trades {
        let signal = match fill.action {
            TradeAction::OpenLong => "BUY",
            TradeAction::OpenShort => "SELL",
            TradeAction::CloseLong | TradeAction::CloseShort => "CLOSE",
            TradeAction::Hold => "HOLD",
        };
        let entry = json!({
            "timestamp": fill.time,
            "symbol": result.config.symbol,
            "decision": {
                "signal": signal,
                "reason": fill.reason,
                "confidence": "MEDIUM",
                "amount": fill.qty,
            },
            "position": null,
        });
        writeln!(decisions, "{}", entry)?;
    }
    fs::write(dir.join("decisions.jsonl"), decisions).context("写入 demo 决策失败")?;

    let closed: Vec<f64> = result.trades.iter().filter_map(|t| t.pnl).collect();
    let wins = closed.iter().filter(|p| **p > 0.0).count();
    let losses = closed.iter().filter(|p| **p < 0.0).count();
    let best = closed.iter().cloned().reduce(f64::max);
    let worst = closed.iter().cloned().reduce(f64::min);

    let mut peak: f64 = f64::MIN;
    let mut max_drawdown: f64 = 0.0;
    for point in &result.equity_curve {
        peak = peak.max(point.equity);
        max_drawdown = max_drawdown.max(peak - point.equity);
    }
    if peak == f64::MIN {
        peak = result.metrics.final_equity;
    }

    let performance = json!({
        "total_realized_pnl": closed.iter().sum::<f64>(),
        "total_trades": result.metrics.trade_count,
        "winning_trades": wins,
        "losing_trades": losses,
        "best_trade": best,
        "worst_trade": worst,
        "equity_peak": peak,
        "max_drawdown": max_drawdown,
        "last_update": last,
    });
    fs::write(
        dir.join("performance.json"),
        serde_json::to_string_pretty(&performance)?,
    )
    .context("写入 demo 绩效失败")?;

    let status = json!({
        "updated_at": last,
        "updated_at_text": format_time(last),
        "account": {
            "total_wallet_balance": result.metrics.final_equity,
            "available_balance": result.metrics.final_equity,
        },
        "risk": {
            "halted": false,
            "halt_reason": null,
            "peak_equity": peak,
            "last_equity": result.metrics.final_equity,
            "stop_loss_pct": 0.02,
            "take_profit_pct": 0.05,
        },
        "positions": [],
    });
    fs::write(
        dir.join("status.json"),
        serde_json::to_string_pretty(&status)?,
    )
    .context("写入 demo 状态失败")?;

    let _ = first;
    Ok(())
}

fn seed_backtest(dir: &Path, result: &BacktestResult) -> Result<()> {
    fs::create_dir_all(dir).context("创建 demo 回测目录失败")?;
    fs::write(
        dir.join("demo_BTCUSDT_1h_sma.json"),
        serde_json::to_string_pretty(result).context("序列化 demo 回测结果失败")?,
    )
    .context("写入 demo 回测结果失败")?;
    Ok(())
}

fn copy_sample_trace(dir: &Path) -> Result<()> {
    let source = Path::new(SAMPLE_TRACE);
    if source.is_file() {
        fs::copy(source, dir.join("agent_trace.jsonl")).context("复制示例 trace 失败")?;
    }
    Ok(())
}

fn format_time(ms: i64) -> String {
    Utc.timestamp_millis_opt(ms)
        .single()
        .map(|dt| dt.format("%Y-%m-%d %H:%M:%S").to_string())
        .unwrap_or_else(|| ms.to_string())
}
