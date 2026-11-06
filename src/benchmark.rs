// 策略基准（排行榜）。
//
// 在同一个区间、同一套成本、同一套 walk-forward 窗口下比较：
// SMA（训练窗选参）/ 随机基线 / 单智能体 LLM / 买入持有。
//
// 输出一份可以直接贴进 README 的 Markdown 表格。

use crate::backtest::EquityPoint;
use crate::data;
use crate::walkforward::{self, StrategyFamily, WalkForwardConfig, WalkForwardMetrics};
use anyhow::{bail, Context, Result};
use serde::Serialize;
use std::collections::BTreeMap;
use std::fmt::Write as _;
use std::fs;
use std::path::PathBuf;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BenchmarkStrategy {
    Sma,
    Random,
    Llm,
    BuyHold,
}

impl BenchmarkStrategy {
    pub fn parse(value: &str) -> Result<Self> {
        match value.trim().to_ascii_lowercase().as_str() {
            "sma" => Ok(Self::Sma),
            "random" => Ok(Self::Random),
            "llm" => Ok(Self::Llm),
            "buy_hold" | "buyhold" | "hold" => Ok(Self::BuyHold),
            other => bail!("未知策略: {}（可选 sma / random / llm / buy_hold）", other),
        }
    }

    pub fn label(&self) -> String {
        match self {
            Self::Sma => "SMA walk-forward".to_string(),
            Self::Random => "Random".to_string(),
            Self::Llm => "LLM single-agent".to_string(),
            Self::BuyHold => "Buy & hold".to_string(),
        }
    }
}

pub struct BenchmarkConfig {
    pub walkforward: WalkForwardConfig,
    pub strategies: Vec<BenchmarkStrategy>,
    pub seed: u64,
    pub flip_prob: f64,
    pub api_key: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct SymbolBenchmark {
    pub symbol: String,
    pub total_return_pct: f64,
    pub max_drawdown_pct: f64,
    pub sharpe: f64,
    pub trade_count: usize,
    pub win_rate_pct: f64,
    pub total_fees: f64,
}

#[derive(Debug, Clone, Serialize)]
pub struct BenchmarkEntry {
    pub strategy: String,
    pub metrics: WalkForwardMetrics,
    pub buy_and_hold_pct: f64,
    pub final_equity: f64,
    pub per_symbol: Vec<SymbolBenchmark>,
}

#[derive(Debug, Clone, Serialize)]
pub struct BenchmarkReport {
    pub config: WalkForwardConfig,
    pub rows: Vec<BenchmarkEntry>,
    pub markdown: String,
}

pub async fn run(cfg: BenchmarkConfig) -> Result<BenchmarkReport> {
    if cfg.strategies.is_empty() {
        bail!("至少选择一个策略");
    }

    let mut rows = Vec::new();

    for strategy in &cfg.strategies {
        let entry = match strategy {
            BenchmarkStrategy::BuyHold => buy_and_hold_entry(&cfg.walkforward).await?,
            BenchmarkStrategy::Sma => {
                let report =
                    walkforward::compute(&cfg.walkforward, StrategyFamily::Sma, None).await?;
                entry_from_report(&BenchmarkStrategy::Sma.label(), report)
            }
            BenchmarkStrategy::Random => {
                let report = walkforward::compute(
                    &cfg.walkforward,
                    StrategyFamily::Random {
                        seed: cfg.seed,
                        flip_prob: cfg.flip_prob,
                    },
                    None,
                )
                .await?;
                entry_from_report(&format!("Random (p={:.2})", cfg.flip_prob), report)
            }
            BenchmarkStrategy::Llm => {
                let key = cfg
                    .api_key
                    .as_deref()
                    .context("benchmark 包含 llm 时需要 DEEPSEEK_API_KEY")?;
                let report =
                    walkforward::compute(&cfg.walkforward, StrategyFamily::Llm, Some(key)).await?;
                entry_from_report(&BenchmarkStrategy::Llm.label(), report)
            }
        };
        rows.push(entry);
    }

    let markdown = render_markdown(&rows, &cfg.walkforward);
    let report = BenchmarkReport {
        config: cfg.walkforward,
        rows,
        markdown,
    };

    persist(&report)?;
    print_report(&report);

    Ok(report)
}

fn entry_from_report(label: &str, report: walkforward::WalkForwardReport) -> BenchmarkEntry {
    let per_symbol = report
        .symbols
        .iter()
        .map(|symbol| SymbolBenchmark {
            symbol: symbol.symbol.clone(),
            total_return_pct: symbol.metrics.total_return_pct,
            max_drawdown_pct: symbol.metrics.max_drawdown_pct,
            sharpe: symbol.metrics.sharpe,
            trade_count: symbol.metrics.trade_count,
            win_rate_pct: symbol.metrics.win_rate_pct,
            total_fees: symbol.metrics.total_fees,
        })
        .collect();

    BenchmarkEntry {
        strategy: label.to_string(),
        metrics: report.portfolio.metrics,
        buy_and_hold_pct: report.portfolio.buy_and_hold_pct,
        final_equity: report.portfolio.final_equity,
        per_symbol,
    }
}

async fn buy_and_hold_entry(cfg: &WalkForwardConfig) -> Result<BenchmarkEntry> {
    let allocation = cfg.initial_balance / cfg.symbols.len() as f64;
    let warmup = walkforward::effective_warmup(cfg);

    let mut per_symbol = Vec::new();
    let mut curves: Vec<Vec<EquityPoint>> = Vec::new();
    let mut total_fees = 0.0;
    let mut windows = 0;

    for symbol in &cfg.symbols {
        let klines = data::load_klines(
            symbol,
            &cfg.interval,
            cfg.start_ms,
            cfg.end_ms,
            &cfg.data_dir,
        )
        .await?;

        let plans = walkforward::plan_windows(klines.len(), warmup, cfg.train_bars, cfg.test_bars);
        if plans.is_empty() {
            bail!("{} 的K线不足以切分窗口", symbol);
        }
        windows = plans.len();

        let entry_index = plans[0].test_start + 1;
        let exit_index = plans[plans.len() - 1].test_end;
        let entry_price = klines[entry_index].open;
        if entry_price <= 0.0 {
            bail!("{} 入场价格无效", symbol);
        }

        let qty = allocation * (1.0 - cfg.fee_rate) / entry_price;
        let mut curve = Vec::new();
        for (index, kline) in klines
            .iter()
            .enumerate()
            .take(exit_index + 1)
            .skip(entry_index)
        {
            let mut equity = qty * kline.close;
            if index == exit_index {
                equity *= 1.0 - cfg.fee_rate;
            }
            curve.push(EquityPoint {
                time: kline.timestamp,
                datetime: walkforward::format_time(kline.timestamp),
                equity,
                price: kline.close,
                position: Some("Long".to_string()),
            });
        }

        let fees = allocation * cfg.fee_rate * 2.0;
        total_fees += fees;

        let mut metrics =
            walkforward::aggregate_metrics(allocation, &curve, &[], &cfg.interval, windows, 100.0);
        metrics.total_fees = fees;

        per_symbol.push(SymbolBenchmark {
            symbol: symbol.clone(),
            total_return_pct: metrics.total_return_pct,
            max_drawdown_pct: metrics.max_drawdown_pct,
            sharpe: metrics.sharpe,
            trade_count: 0,
            win_rate_pct: 0.0,
            total_fees: fees,
        });
        curves.push(curve);
    }

    let mut totals: BTreeMap<i64, f64> = BTreeMap::new();
    for curve in &curves {
        for point in curve {
            *totals.entry(point.time).or_insert(0.0) += point.equity;
        }
    }
    let portfolio_curve: Vec<EquityPoint> = totals
        .into_iter()
        .map(|(time, equity)| EquityPoint {
            time,
            datetime: walkforward::format_time(time),
            equity,
            price: 0.0,
            position: None,
        })
        .collect();

    let mut metrics = walkforward::aggregate_metrics(
        cfg.initial_balance,
        &portfolio_curve,
        &[],
        &cfg.interval,
        windows,
        100.0,
    );
    metrics.total_fees = total_fees;

    Ok(BenchmarkEntry {
        strategy: BenchmarkStrategy::BuyHold.label(),
        buy_and_hold_pct: metrics.total_return_pct,
        metrics,
        final_equity: portfolio_curve
            .last()
            .map(|p| p.equity)
            .unwrap_or(cfg.initial_balance),
        per_symbol,
    })
}

fn render_markdown(rows: &[BenchmarkEntry], cfg: &WalkForwardConfig) -> String {
    let mut out = String::new();
    let _ = writeln!(out, "# Strategy benchmark");
    let _ = writeln!(out);
    let _ = writeln!(
        out,
        "{} · {} · {} → {} · train {}/test {} · fee {:.3}%/side + {:.1}bps",
        cfg.symbols.join(", "),
        cfg.interval,
        walkforward::format_time(cfg.start_ms),
        walkforward::format_time(cfg.end_ms),
        cfg.train_bars,
        cfg.test_bars,
        cfg.fee_rate * 100.0,
        cfg.slippage_bps
    );
    let _ = writeln!(out);
    let _ = writeln!(
        out,
        "| Strategy | Portfolio | Buy & hold | Max DD | Sharpe | Trades | Win rate | Fees |"
    );
    let _ = writeln!(out, "|---|---:|---:|---:|---:|---:|---:|---:|");
    for row in rows {
        let m = &row.metrics;
        let win_rate = if m.trade_count == 0 {
            "—".to_string()
        } else {
            format!("{:.1}%", m.win_rate_pct)
        };
        let _ = writeln!(
            out,
            "| {} | {:+.2}% | {:+.2}% | {:.2}% | {:.2} | {} | {} | {:.2} |",
            row.strategy,
            m.total_return_pct,
            row.buy_and_hold_pct,
            m.max_drawdown_pct,
            m.sharpe,
            m.trade_count,
            win_rate,
            m.total_fees
        );
    }
    out
}

fn persist(report: &BenchmarkReport) -> Result<()> {
    fs::create_dir_all(&report.config.out_dir).context("创建基准输出目录失败")?;
    let stamp = chrono::Utc::now().format("%Y%m%d_%H%M%S").to_string();
    let base = format!("benchmark_{}", stamp);

    let json_path = report.config.out_dir.join(format!("{}.json", base));
    fs::write(
        &json_path,
        serde_json::to_string_pretty(report).context("序列化基准结果失败")?,
    )
    .context("写入基准 JSON 失败")?;

    let md_path = report.config.out_dir.join(format!("{}.md", base));
    fs::write(&md_path, &report.markdown).context("写入基准 Markdown 失败")?;

    log::info!(
        "benchmark 报告: {} / {}",
        json_path.display(),
        md_path.display()
    );
    Ok(())
}

fn print_report(report: &BenchmarkReport) {
    println!();
    println!("================ 策略基准 ================");
    print!("{}", report.markdown);
    println!("==========================================");
    let out_dir = PathBuf::from(&report.config.out_dir);
    println!("报告目录: {}", out_dir.display());
    println!();
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    fn test_config() -> WalkForwardConfig {
        WalkForwardConfig {
            symbols: vec!["BTCUSDT".to_string()],
            interval: "1h".to_string(),
            start_ms: 0,
            end_ms: 1,
            train_bars: 720,
            test_bars: 240,
            warmup: 120,
            fasts: vec![5],
            slows: vec![20],
            initial_balance: 10_000.0,
            position_pct: 0.2,
            leverage: 1.0,
            fee_rate: 0.0005,
            slippage_bps: 2.0,
            stop_loss_pct: 0.02,
            take_profit_pct: 0.05,
            data_dir: PathBuf::from("data/klines"),
            out_dir: PathBuf::from("backtest"),
        }
    }

    #[test]
    fn parses_strategy_names() {
        assert_eq!(
            BenchmarkStrategy::parse("sma").unwrap(),
            BenchmarkStrategy::Sma
        );
        assert_eq!(
            BenchmarkStrategy::parse("buy_hold").unwrap(),
            BenchmarkStrategy::BuyHold
        );
        assert!(BenchmarkStrategy::parse("momentum").is_err());
    }

    #[test]
    fn markdown_renders_signed_metrics() {
        let row = BenchmarkEntry {
            strategy: "SMA".to_string(),
            metrics: WalkForwardMetrics {
                total_return_pct: -1.0,
                max_drawdown_pct: 2.0,
                sharpe: -0.5,
                trade_count: 3,
                win_rate_pct: 33.3,
                total_fees: 1.0,
                ..Default::default()
            },
            buy_and_hold_pct: 4.0,
            final_equity: 9_900.0,
            per_symbol: Vec::new(),
        };

        let markdown = render_markdown(&[row], &test_config());
        assert!(markdown.contains("| SMA | -1.00% | +4.00% | 2.00% |"));
        assert!(markdown.contains("BTCUSDT"));
        assert!(markdown.contains("train 720/test 240"));
    }
}
