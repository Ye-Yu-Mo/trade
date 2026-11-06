// Walk-forward 回测。
//
// 目标不是找一组“历史最优参数”，而是回答：
// 在每个滚动窗口里用训练集选参数，拿到下一段没见过的数据上，结果还稳不稳。
//
// 流程：
// 1. 把每个标的切成 [warmup][train][test][train][test]...
// 2. 训练窗口内遍历 fast/slow 参数组合，按 收益 - 0.5 * 回撤 选一组
// 3. 只把选出的参数应用到紧随其后的测试窗口
// 4. 测试窗口的权益链接起来，得到样本外曲线
// 5. 多标的按等权分配初始资金，报告单标的和组合结果

use crate::backtest::{
    self, BacktestConfig, BacktestResult, EquityPoint, Fill, Metrics, SegmentRequest,
};
use crate::data;
use crate::strategy::Strategy;
use crate::types::Kline;
use anyhow::{bail, Context, Result};
use chrono::{TimeZone, Utc};
use log::info;
use serde::Serialize;
use std::collections::BTreeMap;
use std::fmt::Write as _;
use std::fs;
use std::path::PathBuf;

#[derive(Debug, Clone, Serialize)]
pub struct WalkForwardConfig {
    pub symbols: Vec<String>,
    pub interval: String,
    pub start_ms: i64,
    pub end_ms: i64,
    pub train_bars: usize,
    pub test_bars: usize,
    pub warmup: usize,
    pub fasts: Vec<usize>,
    pub slows: Vec<usize>,
    pub initial_balance: f64,
    pub position_pct: f64,
    pub leverage: f64,
    pub fee_rate: f64,
    pub slippage_bps: f64,
    pub stop_loss_pct: f64,
    pub take_profit_pct: f64,
    pub data_dir: PathBuf,
    pub out_dir: PathBuf,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct WindowPlan {
    pub train_start: usize,
    pub train_end: usize,
    pub test_start: usize,
    pub test_end: usize,
}

#[derive(Debug, Clone, Serialize)]
pub struct WindowReport {
    pub train_from: i64,
    pub train_to: i64,
    pub test_from: i64,
    pub test_to: i64,
    pub fast: usize,
    pub slow: usize,
    pub train_return_pct: f64,
    pub train_drawdown_pct: f64,
    pub train_trades: usize,
    pub test_return_pct: f64,
    pub test_drawdown_pct: f64,
    pub test_trades: usize,
    pub test_win_rate_pct: f64,
}

#[derive(Debug, Clone, Serialize, Default)]
pub struct WalkForwardMetrics {
    pub total_return_pct: f64,
    pub max_drawdown_pct: f64,
    pub sharpe: f64,
    pub trade_count: usize,
    pub win_rate_pct: f64,
    pub profit_factor: Option<f64>,
    pub total_fees: f64,
    pub exposure_pct: f64,
    pub bars: usize,
    pub windows: usize,
}

#[derive(Debug, Clone, Serialize)]
pub struct SymbolWalkForward {
    pub symbol: String,
    pub initial_equity: f64,
    pub final_equity: f64,
    pub buy_and_hold_pct: f64,
    pub metrics: WalkForwardMetrics,
    pub windows: Vec<WindowReport>,
    pub equity_curve: Vec<EquityPoint>,
}

#[derive(Debug, Clone, Serialize)]
pub struct PortfolioSummary {
    pub initial_equity: f64,
    pub final_equity: f64,
    pub buy_and_hold_pct: f64,
    pub metrics: WalkForwardMetrics,
    pub equity_curve: Vec<EquityPoint>,
}

#[derive(Debug, Clone, Serialize)]
pub struct WalkForwardReport {
    pub config: WalkForwardConfig,
    pub symbols: Vec<SymbolWalkForward>,
    pub portfolio: PortfolioSummary,
}

pub async fn run(cfg: WalkForwardConfig) -> Result<WalkForwardReport> {
    if cfg.symbols.is_empty() {
        bail!("至少需要一个标的");
    }
    if cfg.train_bars < 2 || cfg.test_bars < 1 {
        bail!("train_bars 和 test_bars 必须为正");
    }
    if cfg.initial_balance <= 0.0 {
        bail!("initial_balance 必须为正");
    }

    let grid = parameter_grid(&cfg.fasts, &cfg.slows);
    if grid.is_empty() {
        bail!("参数网格为空：需要至少一组 fast < slow");
    }

    let max_slow = cfg.slows.iter().copied().max().unwrap_or(20);
    let warmup = cfg.warmup.max(max_slow + 5).max(5);
    let allocation = cfg.initial_balance / cfg.symbols.len() as f64;

    let mut symbol_reports = Vec::new();
    let mut all_trades: Vec<Fill> = Vec::new();

    for symbol in &cfg.symbols {
        info!("walk-forward: {} 开始", symbol);
        let klines = data::load_klines(
            symbol,
            &cfg.interval,
            cfg.start_ms,
            cfg.end_ms,
            &cfg.data_dir,
        )
        .await?;

        let plans = plan_windows(klines.len(), warmup, cfg.train_bars, cfg.test_bars);
        if plans.is_empty() {
            bail!(
                "{} 的K线不足以切分窗口：{} 根，需要 warmup({}) + train({}) + test({}) + 1",
                symbol,
                klines.len(),
                warmup,
                cfg.train_bars,
                cfg.test_bars
            );
        }

        let (report, trades) =
            run_symbol(&cfg, symbol, &klines, &plans, allocation, warmup, &grid).await?;
        symbol_reports.push(report);
        all_trades.extend(trades);
    }

    let portfolio = build_portfolio(&cfg, &symbol_reports, &all_trades);
    let report = WalkForwardReport {
        config: cfg,
        symbols: symbol_reports,
        portfolio,
    };

    persist(&report)?;
    print_report(&report);

    Ok(report)
}

async fn run_symbol(
    cfg: &WalkForwardConfig,
    symbol: &str,
    klines: &[Kline],
    plans: &[WindowPlan],
    initial_equity: f64,
    warmup: usize,
    grid: &[(usize, usize)],
) -> Result<(SymbolWalkForward, Vec<Fill>)> {
    let backtest_cfg = build_backtest_config(cfg, symbol, warmup);
    let mut equity = initial_equity;
    let mut curve: Vec<EquityPoint> = Vec::new();
    let mut trades: Vec<Fill> = Vec::new();
    let mut windows = Vec::new();

    for plan in plans {
        let (fast, slow, train) = choose_params(&backtest_cfg, klines, plan, equity, grid).await?;

        let mut strategy = Strategy::sma_cross_with(fast, slow);
        let test = backtest::run_segment(
            &backtest_cfg,
            &mut strategy,
            &SegmentRequest {
                klines,
                start_index: plan.test_start,
                end_index: plan.test_end,
                initial_equity: equity,
            },
        )
        .await?;

        equity = test.metrics.final_equity;
        if curve.is_empty() {
            curve.extend(test.equity_curve.iter().cloned());
        } else {
            curve.extend(test.equity_curve.iter().skip(1).cloned());
        }
        trades.extend(test.trades);

        windows.push(WindowReport {
            train_from: klines[plan.train_start].timestamp,
            train_to: klines[plan.train_end - 1].timestamp,
            test_from: klines[plan.test_start].timestamp,
            test_to: klines[plan.test_end].timestamp,
            fast,
            slow,
            train_return_pct: train.metrics.total_return_pct,
            train_drawdown_pct: train.metrics.max_drawdown_pct,
            train_trades: train.metrics.trade_count,
            test_return_pct: test.metrics.total_return_pct,
            test_drawdown_pct: test.metrics.max_drawdown_pct,
            test_trades: test.metrics.trade_count,
            test_win_rate_pct: test.metrics.win_rate_pct,
        });
    }

    let (exposure, _) = exposure_and_bars(&curve);
    let metrics = aggregate_metrics(
        initial_equity,
        &curve,
        &trades,
        &cfg.interval,
        windows.len(),
        exposure,
    );

    let first_close = klines[plans[0].test_start].close;
    let last_close = klines[plans[plans.len() - 1].test_end].close;
    let buy_and_hold_pct = if first_close > 0.0 {
        (last_close - first_close) / first_close * 100.0
    } else {
        0.0
    };

    Ok((
        SymbolWalkForward {
            symbol: symbol.to_string(),
            initial_equity,
            final_equity: equity,
            buy_and_hold_pct,
            metrics,
            windows,
            equity_curve: curve,
        },
        trades,
    ))
}

async fn choose_params(
    cfg: &BacktestConfig,
    klines: &[Kline],
    plan: &WindowPlan,
    initial_equity: f64,
    grid: &[(usize, usize)],
) -> Result<(usize, usize, BacktestResult)> {
    let mut best: Option<(f64, (usize, usize), BacktestResult)> = None;

    for &(fast, slow) in grid {
        let mut strategy = Strategy::sma_cross_with(fast, slow);
        let result = backtest::run_segment(
            cfg,
            &mut strategy,
            &SegmentRequest {
                klines,
                start_index: plan.train_start,
                // 最后一笔训练成交落在训练窗口内，避免用到测试窗口第一根K线
                end_index: plan.train_end - 1,
                initial_equity,
            },
        )
        .await?;

        let score = train_score(&result.metrics);
        let better = match &best {
            None => true,
            Some((best_score, _, _)) => score > *best_score,
        };
        if better {
            best = Some((score, (fast, slow), result));
        }
    }

    let (_, (fast, slow), result) = best.context("没有可用的参数组合")?;
    Ok((fast, slow, result))
}

fn train_score(metrics: &Metrics) -> f64 {
    if metrics.liquidated || metrics.trade_count == 0 {
        return f64::NEG_INFINITY;
    }
    metrics.total_return_pct - 0.5 * metrics.max_drawdown_pct
}

fn aggregate_metrics(
    initial_equity: f64,
    curve: &[EquityPoint],
    trades: &[Fill],
    interval: &str,
    windows: usize,
    exposure_pct: f64,
) -> WalkForwardMetrics {
    let final_equity = curve.last().map(|p| p.equity).unwrap_or(initial_equity);
    let total_return_pct = if initial_equity > 0.0 {
        (final_equity - initial_equity) / initial_equity * 100.0
    } else {
        0.0
    };
    let (max_drawdown_pct, sharpe) = backtest::drawdown_and_sharpe(curve, interval);

    let closed: Vec<f64> = trades.iter().filter_map(|t| t.pnl).collect();
    let wins = closed.iter().filter(|p| **p > 0.0).count();
    let gross_profit: f64 = closed.iter().filter(|p| **p > 0.0).sum();
    let gross_loss: f64 = closed.iter().filter(|p| **p < 0.0).map(|p| p.abs()).sum();

    WalkForwardMetrics {
        total_return_pct,
        max_drawdown_pct,
        sharpe,
        trade_count: closed.len(),
        win_rate_pct: if closed.is_empty() {
            0.0
        } else {
            wins as f64 / closed.len() as f64 * 100.0
        },
        profit_factor: if gross_loss > 0.0 {
            Some(gross_profit / gross_loss)
        } else {
            None
        },
        total_fees: trades.iter().map(|t| t.fee).sum(),
        exposure_pct,
        bars: curve.len(),
        windows,
    }
}

fn exposure_and_bars(curve: &[EquityPoint]) -> (f64, usize) {
    if curve.is_empty() {
        return (0.0, 0);
    }
    let exposed = curve.iter().filter(|p| p.position.is_some()).count();
    (exposed as f64 / curve.len() as f64 * 100.0, curve.len())
}

fn build_portfolio(
    cfg: &WalkForwardConfig,
    symbols: &[SymbolWalkForward],
    trades: &[Fill],
) -> PortfolioSummary {
    let mut totals: BTreeMap<i64, f64> = BTreeMap::new();
    for symbol in symbols {
        for point in &symbol.equity_curve {
            *totals.entry(point.time).or_insert(0.0) += point.equity;
        }
    }

    let curve: Vec<EquityPoint> = totals
        .into_iter()
        .map(|(time, equity)| EquityPoint {
            time,
            datetime: format_time(time),
            equity,
            price: 0.0,
            position: None,
        })
        .collect();

    let exposure = if symbols.is_empty() {
        0.0
    } else {
        symbols.iter().map(|s| s.metrics.exposure_pct).sum::<f64>() / symbols.len() as f64
    };

    let final_equity = curve
        .last()
        .map(|p| p.equity)
        .unwrap_or(cfg.initial_balance);
    let metrics = aggregate_metrics(
        cfg.initial_balance,
        &curve,
        trades,
        &cfg.interval,
        symbols.iter().map(|s| s.windows.len()).max().unwrap_or(0),
        exposure,
    );

    let buy_and_hold_pct = if symbols.is_empty() {
        0.0
    } else {
        symbols.iter().map(|s| s.buy_and_hold_pct).sum::<f64>() / symbols.len() as f64
    };

    PortfolioSummary {
        initial_equity: cfg.initial_balance,
        final_equity,
        buy_and_hold_pct,
        metrics,
        equity_curve: curve,
    }
}

fn build_backtest_config(cfg: &WalkForwardConfig, symbol: &str, warmup: usize) -> BacktestConfig {
    BacktestConfig {
        symbol: symbol.to_string(),
        interval: cfg.interval.clone(),
        start_ms: cfg.start_ms,
        end_ms: cfg.end_ms,
        strategy: "sma_cross".to_string(),
        initial_balance: cfg.initial_balance,
        position_pct: cfg.position_pct,
        leverage: cfg.leverage,
        fee_rate: cfg.fee_rate,
        slippage_bps: cfg.slippage_bps,
        stop_loss_pct: cfg.stop_loss_pct,
        take_profit_pct: cfg.take_profit_pct,
        warmup,
        data_dir: cfg.data_dir.clone(),
        out_dir: cfg.out_dir.clone(),
        max_bars: None,
    }
}

pub fn plan_windows(
    n: usize,
    warmup: usize,
    train_bars: usize,
    test_bars: usize,
) -> Vec<WindowPlan> {
    let mut plans = Vec::new();
    if train_bars < 2 || test_bars < 1 {
        return plans;
    }

    let mut start = warmup;
    loop {
        let train_end = start + train_bars;
        let test_start = train_end;
        let test_end = test_start + test_bars;
        if test_end >= n {
            break;
        }
        plans.push(WindowPlan {
            train_start: start,
            train_end,
            test_start,
            test_end,
        });
        start += test_bars;
    }
    plans
}

pub fn parameter_grid(fasts: &[usize], slows: &[usize]) -> Vec<(usize, usize)> {
    let mut grid: Vec<(usize, usize)> = fasts
        .iter()
        .copied()
        .flat_map(|fast| {
            slows
                .iter()
                .copied()
                .filter(move |slow| fast > 0 && fast < *slow)
                .map(move |slow| (fast, slow))
        })
        .collect();
    grid.sort_unstable();
    grid.dedup();
    grid
}

fn persist(report: &WalkForwardReport) -> Result<()> {
    fs::create_dir_all(&report.config.out_dir).context("创建回测输出目录失败")?;
    let stamp = Utc::now().format("%Y%m%d_%H%M%S").to_string();
    let base = format!("walkforward_{}", stamp);

    let json_path = report.config.out_dir.join(format!("{}.json", base));
    fs::write(
        &json_path,
        serde_json::to_string_pretty(report).context("序列化 walk-forward 报告失败")?,
    )
    .context("写入 walk-forward 报告失败")?;

    let csv_path = report
        .config
        .out_dir
        .join(format!("{}_portfolio.csv", base));
    fs::write(&csv_path, portfolio_csv(&report.portfolio.equity_curve))
        .context("写入组合权益曲线失败")?;

    info!(
        "walk-forward 报告: {} / {}",
        json_path.display(),
        csv_path.display()
    );
    Ok(())
}

fn portfolio_csv(points: &[EquityPoint]) -> String {
    let mut out = String::from("time,datetime,equity\n");
    for p in points {
        let _ = writeln!(out, "{},{},{:.2}", p.time, p.datetime, p.equity);
    }
    out
}

fn print_report(report: &WalkForwardReport) {
    println!();
    println!("================ Walk-forward 结果 ================");
    println!(
        "训练 {} 根 / 测试 {} 根 | fast={:?} slow={:?} | 成本 {:.3}% + {:.1}bps",
        report.config.train_bars,
        report.config.test_bars,
        report.config.fasts,
        report.config.slows,
        report.config.fee_rate * 100.0,
        report.config.slippage_bps
    );
    println!("---------------------------------------------------");
    println!(
        "{:<10} {:>10} {:>9} {:>9} {:>8} {:>7} {:>7}",
        "标的", "OOS收益", "买入持有", "最大回撤", "Sharpe", "交易", "胜率"
    );
    for symbol in &report.symbols {
        let m = &symbol.metrics;
        println!(
            "{:<10} {:>+9.2}% {:>+8.2}% {:>8.2}% {:>8.2} {:>7} {:>6.1}%",
            symbol.symbol,
            m.total_return_pct,
            symbol.buy_and_hold_pct,
            m.max_drawdown_pct,
            m.sharpe,
            m.trade_count,
            m.win_rate_pct
        );
    }
    let m = &report.portfolio.metrics;
    println!("---------------------------------------------------");
    println!(
        "{:<10} {:>+9.2}% {:>9} {:>8.2}% {:>8.2} {:>7} {:>6.1}%",
        "组合(等权)",
        m.total_return_pct,
        format!("{:+.2}%", report.portfolio.buy_and_hold_pct),
        m.max_drawdown_pct,
        m.sharpe,
        m.trade_count,
        m.win_rate_pct
    );
    println!(
        "窗口数: {} | 组合期末权益: {:.2} USDT",
        report
            .symbols
            .iter()
            .map(|s| s.windows.len())
            .max()
            .unwrap_or(0),
        report.portfolio.final_equity
    );
    println!("===================================================");
    println!();
}

fn format_time(ms: i64) -> String {
    Utc.timestamp_millis_opt(ms)
        .single()
        .map(|dt| dt.format("%Y-%m-%d %H:%M").to_string())
        .unwrap_or_else(|| ms.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn plan_is_rolling_and_non_overlapping() {
        let plans = plan_windows(1000, 120, 300, 100);
        assert!(!plans.is_empty());
        for pair in plans.windows(2) {
            assert_eq!(pair[0].test_start, pair[0].train_end);
            assert_eq!(pair[1].train_start, pair[0].train_start + 100);
        }
        let last = plans.last().unwrap();
        assert!(last.test_end < 1000);
    }

    #[test]
    fn plan_returns_empty_when_data_is_short() {
        assert!(plan_windows(200, 120, 300, 100).is_empty());
        assert!(plan_windows(1000, 120, 0, 100).is_empty());
        assert!(plan_windows(1000, 120, 300, 0).is_empty());
    }

    #[test]
    fn grid_filters_invalid_pairs_and_dedups() {
        let grid = parameter_grid(&[5, 10, 5], &[20, 50, 10]);
        assert_eq!(grid, vec![(5, 10), (5, 20), (5, 50), (10, 20), (10, 50)]);
    }

    #[test]
    fn aggregate_metrics_uses_curve_and_trades() {
        let curve = vec![
            EquityPoint {
                time: 1,
                datetime: String::new(),
                equity: 10_000.0,
                price: 100.0,
                position: Some("Long".into()),
            },
            EquityPoint {
                time: 2,
                datetime: String::new(),
                equity: 11_000.0,
                price: 100.0,
                position: Some("Long".into()),
            },
            EquityPoint {
                time: 3,
                datetime: String::new(),
                equity: 9_500.0,
                price: 100.0,
                position: None,
            },
        ];
        let trade = Fill {
            time: 3,
            datetime: String::new(),
            action: crate::types::TradeAction::CloseLong,
            price: 100.0,
            qty: 1.0,
            fee: 5.0,
            pnl: Some(-500.0),
            reason: String::new(),
            equity_after: 9_500.0,
        };
        let metrics = aggregate_metrics(10_000.0, &curve, &[trade], "1h", 2, 66.6);
        assert!((metrics.total_return_pct + 5.0).abs() < 1e-9);
        assert!((metrics.max_drawdown_pct - 13.6363).abs() < 0.01);
        assert_eq!(metrics.trade_count, 1);
        assert_eq!(metrics.win_rate_pct, 0.0);
        assert!((metrics.total_fees - 5.0).abs() < 1e-9);
    }

    #[test]
    fn train_score_rejects_no_trade_results() {
        let metrics = Metrics::default();
        assert_eq!(train_score(&metrics), f64::NEG_INFINITY);
    }
}
