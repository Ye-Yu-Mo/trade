// 回测引擎。
//
// 设计原则：
// 1. 信号在K线收盘时产生，成交在下一根K线开盘时执行 —— 避免未来函数。
// 2. 手续费和滑点默认都计入，不给出“零成本”的幻觉。
// 3. 输出可直接比较的指标，并且始终给出买入持有基准。
//
// 已知限制（见 README）：
// - 不模拟资金费率、爆仓和保证金强平。
// - 策略只能给方向，仓位按固定比例开，不支持分批建仓。

use crate::data;
use crate::market;
use crate::risk;
use crate::strategy::{BarContext, Strategy};
use crate::types::{Confidence, Kline, Position, PositionSide, Signal, TradeAction};
use anyhow::{bail, Context, Result};
use chrono::{TimeZone, Utc};
use log::info;
use serde::Serialize;
use std::fmt::Write as _;
use std::fs;
use std::path::PathBuf;

#[derive(Debug, Clone, Serialize)]
pub struct BacktestConfig {
    pub symbol: String,
    pub interval: String,
    pub start_ms: i64,
    pub end_ms: i64,
    pub strategy: String,
    pub initial_balance: f64,
    pub position_pct: f64,
    pub leverage: f64,
    pub fee_rate: f64,
    pub slippage_bps: f64,
    pub stop_loss_pct: f64,
    pub take_profit_pct: f64,
    pub warmup: usize,
    pub data_dir: PathBuf,
    pub out_dir: PathBuf,
    pub max_bars: Option<usize>,
}

#[derive(Debug, Clone)]
pub struct PaperPosition {
    pub side: PositionSide,
    pub qty: f64,
    pub entry_price: f64,
    pub entry_fee: f64,
}

#[derive(Debug, Clone, Serialize)]
pub struct Fill {
    pub time: i64,
    pub datetime: String,
    pub action: TradeAction,
    pub price: f64,
    pub qty: f64,
    pub fee: f64,
    /// 仅平仓单有值：已扣除开仓和平仓手续费的净盈亏
    pub pnl: Option<f64>,
    pub reason: String,
    pub equity_after: f64,
}

#[derive(Debug, Clone, Serialize)]
pub struct EquityPoint {
    pub time: i64,
    pub datetime: String,
    pub equity: f64,
    pub price: f64,
    pub position: Option<String>,
}

#[derive(Debug, Clone, Serialize, Default)]
pub struct Metrics {
    pub initial_equity: f64,
    pub final_equity: f64,
    pub total_return_pct: f64,
    pub buy_and_hold_return_pct: f64,
    pub max_drawdown_pct: f64,
    pub sharpe: f64,
    pub trade_count: usize,
    pub win_rate_pct: f64,
    /// 盈利总额 / 亏损总额；没有亏损单时为 None（即无穷大）
    pub profit_factor: Option<f64>,
    pub avg_win: Option<f64>,
    pub avg_loss: Option<f64>,
    pub total_fees: f64,
    pub exposure_pct: f64,
    pub bars: usize,
    pub liquidated: bool,
}

#[derive(Debug, Clone, Serialize)]
pub struct BacktestResult {
    pub config: BacktestConfig,
    pub metrics: Metrics,
    pub trades: Vec<Fill>,
    pub equity_curve: Vec<EquityPoint>,
}

/// 极简纸面账户。只处理市价单，按比例开仓。
pub struct PaperBroker {
    pub cash: f64,
    pub position: Option<PaperPosition>,
    pub fee_rate: f64,
    pub slippage: f64,
    pub fills: Vec<Fill>,
    pub fees_paid: f64,
}

impl PaperBroker {
    pub fn new(initial_balance: f64, fee_rate: f64, slippage_bps: f64) -> Self {
        Self {
            cash: initial_balance,
            position: None,
            fee_rate,
            slippage: slippage_bps / 10_000.0,
            fills: Vec::new(),
            fees_paid: 0.0,
        }
    }

    pub fn unrealized(&self, price: f64) -> f64 {
        match &self.position {
            None => 0.0,
            Some(pos) => match pos.side {
                PositionSide::Long => (price - pos.entry_price) * pos.qty,
                PositionSide::Short => (pos.entry_price - price) * pos.qty,
            },
        }
    }

    pub fn equity(&self, price: f64) -> f64 {
        self.cash + self.unrealized(price)
    }

    pub fn position_view(&self, price: f64) -> Option<Position> {
        self.position.as_ref().map(|pos| Position {
            side: pos.side.clone(),
            amount: pos.qty,
            entry_price: pos.entry_price,
            unrealized_pnl: self.unrealized(price),
        })
    }

    /// 根据方向调整仓位：同向不动，反向平仓后反手，空仓开仓。
    pub fn execute(
        &mut self,
        signal: Signal,
        price: f64,
        time: i64,
        position_pct: f64,
        leverage: f64,
        reason: &str,
    ) -> Result<()> {
        let target = match signal {
            Signal::Close => {
                self.close(price, time, reason)?;
                return Ok(());
            }
            Signal::Buy => PositionSide::Long,
            Signal::Sell => PositionSide::Short,
            Signal::Hold => return Ok(()),
        };

        let current_side = self.position.as_ref().map(|p| p.side.clone());

        if current_side.as_ref() == Some(&target) {
            return Ok(()); // 已持有同向仓位，回测不做加仓
        }

        if current_side.is_some() {
            self.close(price, time, reason)?;
        }

        self.open(target, price, time, position_pct, leverage, reason)
    }

    fn open(
        &mut self,
        side: PositionSide,
        price: f64,
        time: i64,
        position_pct: f64,
        leverage: f64,
        reason: &str,
    ) -> Result<()> {
        if price <= 0.0 {
            bail!("成交价格无效: {}", price);
        }

        let equity = self.equity(price);
        if equity <= 0.0 {
            bail!("账户权益已耗尽");
        }

        let notional = equity * position_pct.clamp(0.0, 1.0) * leverage.max(0.0);
        if notional <= 0.0 {
            return Ok(());
        }

        let fill_price = match side {
            PositionSide::Long => price * (1.0 + self.slippage),
            PositionSide::Short => price * (1.0 - self.slippage),
        };
        let qty = notional / fill_price;
        let fee = fill_price * qty * self.fee_rate;
        self.cash -= fee;
        self.fees_paid += fee;

        let action = match side {
            PositionSide::Long => TradeAction::OpenLong,
            PositionSide::Short => TradeAction::OpenShort,
        };

        self.fills.push(Fill {
            time,
            datetime: format_time(time),
            action,
            price: fill_price,
            qty,
            fee,
            pnl: None,
            reason: reason.to_string(),
            equity_after: self.cash,
        });

        self.position = Some(PaperPosition {
            side,
            qty,
            entry_price: fill_price,
            entry_fee: fee,
        });

        Ok(())
    }

    pub fn close(&mut self, price: f64, time: i64, reason: &str) -> Result<()> {
        let Some(pos) = self.position.take() else {
            return Ok(());
        };

        let fill_price = match pos.side {
            PositionSide::Long => price * (1.0 - self.slippage),
            PositionSide::Short => price * (1.0 + self.slippage),
        };

        let gross = match pos.side {
            PositionSide::Long => (fill_price - pos.entry_price) * pos.qty,
            PositionSide::Short => (pos.entry_price - fill_price) * pos.qty,
        };
        let exit_fee = fill_price * pos.qty * self.fee_rate;
        let net = gross - exit_fee - pos.entry_fee;

        self.cash += gross - exit_fee;
        self.fees_paid += exit_fee;

        let action = match pos.side {
            PositionSide::Long => TradeAction::CloseLong,
            PositionSide::Short => TradeAction::CloseShort,
        };

        self.fills.push(Fill {
            time,
            datetime: format_time(time),
            action,
            price: fill_price,
            qty: pos.qty,
            fee: exit_fee,
            pnl: Some(net),
            reason: reason.to_string(),
            equity_after: self.cash,
        });

        Ok(())
    }
}

#[derive(Debug, Clone)]
pub struct SegmentRequest<'a> {
    pub klines: &'a [Kline],
    /// 第一根参与决策的K线索引，必须满足 start_index + 1 >= warmup
    pub start_index: usize,
    /// 最后一根可作为“下一根成交”的K线索引（含）
    pub end_index: usize,
    pub initial_equity: f64,
}

pub async fn run(cfg: BacktestConfig, strategy: Strategy) -> Result<BacktestResult> {
    if cfg.warmup < 5 {
        bail!("warmup 至少为 5 根K线");
    }

    let klines = data::load_klines(
        &cfg.symbol,
        &cfg.interval,
        cfg.start_ms,
        cfg.end_ms,
        &cfg.data_dir,
    )
    .await?;

    if klines.len() < cfg.warmup + 2 {
        bail!(
            "K线数量不足：需要至少 {} 根（warmup {} + 2），实际 {} 根",
            cfg.warmup + 2,
            cfg.warmup,
            klines.len()
        );
    }

    let last_index = match cfg.max_bars {
        Some(n) => (cfg.warmup - 1 + n.max(1)).min(klines.len() - 1),
        None => klines.len() - 1,
    };

    let mut strategy = strategy;
    let result = run_segment(
        &cfg,
        &mut strategy,
        &SegmentRequest {
            klines: &klines,
            start_index: cfg.warmup - 1,
            end_index: last_index,
            initial_equity: cfg.initial_balance,
        },
    )
    .await?;

    persist(&result)?;
    print_report(&result);

    Ok(result)
}

/// 在给定的一段K线上运行回测，不读盘、不落盘、不打印。
/// walk-forward 复用它来评估训练窗口和测试窗口。
pub async fn run_segment(
    cfg: &BacktestConfig,
    strategy: &mut Strategy,
    req: &SegmentRequest<'_>,
) -> Result<BacktestResult> {
    if cfg.warmup < 5 {
        bail!("warmup 至少为 5 根K线");
    }
    if cfg.position_pct <= 0.0 || cfg.position_pct > 1.0 {
        bail!("position_pct 必须在 (0, 1] 之间");
    }
    if req.initial_equity <= 0.0 {
        bail!("初始权益必须为正");
    }
    if req.start_index + 1 < cfg.warmup {
        bail!(
            "start_index {} 之前没有足够的预热K线（warmup {}）",
            req.start_index,
            cfg.warmup
        );
    }
    if req.end_index <= req.start_index || req.end_index >= req.klines.len() {
        bail!(
            "回测区间无效: start_index={}, end_index={}, bars={}",
            req.start_index,
            req.end_index,
            req.klines.len()
        );
    }

    let klines = req.klines;
    let warmup = cfg.warmup;
    let mut broker = PaperBroker::new(req.initial_equity, cfg.fee_rate, cfg.slippage_bps);
    let mut equity_curve = vec![EquityPoint {
        time: klines[req.start_index].timestamp,
        datetime: format_time(klines[req.start_index].timestamp),
        equity: req.initial_equity,
        price: klines[req.start_index].close,
        position: None,
    }];
    let mut liquidated = false;

    for i in req.start_index..req.end_index {
        let window = &klines[i + 1 - warmup..=i];
        let indicators = market::calculate_indicators(window)?;
        let current_price = klines[i].close;
        let next = &klines[i + 1];

        // 用本根K线的高低点判断盘中止损/止盈，避免只用收盘价低估风险
        let mut risk_exit = false;
        let open_position = broker
            .position
            .as_ref()
            .map(|p| (p.side.clone(), p.entry_price));
        if let Some((side, entry)) = open_position {
            if let Some((fill_price, label)) = risk::intrabar_exit(
                &side,
                entry,
                klines[i].open,
                klines[i].high,
                klines[i].low,
                cfg.stop_loss_pct,
                cfg.take_profit_pct,
            ) {
                let reason = format!("{}触发: 入场 {:.4}, 触发价 {:.4}", label, entry, fill_price);
                broker.close(fill_price, klines[i].timestamp, &reason)?;
                risk_exit = true;
            }
        }

        // 风控出场当根不再重新入场，避免同一根K线里反复进出
        if !risk_exit {
            let position = broker.position_view(current_price);
            let ctx = BarContext {
                symbol: &cfg.symbol,
                interval: &cfg.interval,
                window,
                indicators: &indicators,
                position: position.as_ref(),
                equity: broker.equity(current_price),
                available_balance: broker.cash,
            };

            let direction = strategy.decide(&ctx).await?;

            // 低信心入场信号跳过；平仓信号永远执行
            let executable = matches!(direction.signal, Signal::Close)
                || direction.confidence != Confidence::Low;

            if direction.signal != Signal::Hold && executable {
                broker.execute(
                    direction.signal,
                    next.open,
                    next.timestamp,
                    cfg.position_pct,
                    cfg.leverage,
                    &direction.reason,
                )?;
            }
        }

        let equity = broker.equity(next.close);
        equity_curve.push(EquityPoint {
            time: next.timestamp,
            datetime: format_time(next.timestamp),
            equity,
            price: next.close,
            position: broker.position.as_ref().map(|p| format!("{:?}", p.side)),
        });

        if equity <= 0.0 {
            liquidated = true;
            break;
        }
    }

    let last_bar = &klines[req.end_index];
    if !liquidated && broker.position.is_some() {
        broker.close(last_bar.close, last_bar.timestamp, "回测结束，强制平仓")?;
        if let Some(point) = equity_curve.last_mut() {
            point.equity = broker.equity(last_bar.close);
            point.position = None;
        }
    }

    let first_close = klines[req.start_index].close;
    let last_close = klines[req.end_index].close;
    let buy_and_hold_pct = if first_close > 0.0 {
        (last_close - first_close) / first_close * 100.0
    } else {
        0.0
    };

    let metrics = compute_metrics(
        req.initial_equity,
        &cfg.interval,
        buy_and_hold_pct,
        &broker,
        &equity_curve,
        liquidated,
    );

    Ok(BacktestResult {
        config: cfg.clone(),
        metrics,
        trades: broker.fills,
        equity_curve,
    })
}

fn compute_metrics(
    initial_equity: f64,
    interval: &str,
    buy_and_hold_pct: f64,
    broker: &PaperBroker,
    equity_curve: &[EquityPoint],
    liquidated: bool,
) -> Metrics {
    let final_equity = equity_curve
        .last()
        .map(|p| p.equity)
        .unwrap_or(initial_equity);

    let total_return_pct = if initial_equity > 0.0 {
        (final_equity - initial_equity) / initial_equity * 100.0
    } else {
        0.0
    };

    let (max_drawdown_pct, sharpe) = drawdown_and_sharpe(equity_curve, interval);

    let closed: Vec<f64> = broker.fills.iter().filter_map(|f| f.pnl).collect();
    let wins: Vec<f64> = closed.iter().copied().filter(|p| *p > 0.0).collect();
    let losses: Vec<f64> = closed.iter().copied().filter(|p| *p < 0.0).collect();

    let trade_count = closed.len();
    let win_rate_pct = if trade_count > 0 {
        wins.len() as f64 / trade_count as f64 * 100.0
    } else {
        0.0
    };

    let gross_profit: f64 = wins.iter().sum();
    let gross_loss: f64 = losses.iter().map(|p| p.abs()).sum();
    let profit_factor = if gross_loss > 0.0 {
        Some(gross_profit / gross_loss)
    } else {
        None
    };

    let avg_win = average(&wins);
    let avg_loss = average(&losses);

    let exposure_bars = equity_curve.iter().filter(|p| p.position.is_some()).count();
    let exposure_pct = if equity_curve.is_empty() {
        0.0
    } else {
        exposure_bars as f64 / equity_curve.len() as f64 * 100.0
    };

    Metrics {
        initial_equity,
        final_equity,
        total_return_pct,
        buy_and_hold_return_pct: buy_and_hold_pct,
        max_drawdown_pct,
        sharpe,
        trade_count,
        win_rate_pct,
        profit_factor,
        avg_win,
        avg_loss,
        total_fees: broker.fees_paid,
        exposure_pct,
        bars: equity_curve.len(),
        liquidated,
    }
}

pub(crate) fn drawdown_and_sharpe(equity_curve: &[EquityPoint], interval: &str) -> (f64, f64) {
    let mut peak = f64::MIN;
    let mut max_dd = 0.0;

    for point in equity_curve {
        if point.equity > peak {
            peak = point.equity;
        }
        if peak > 0.0 {
            let dd = (peak - point.equity) / peak * 100.0;
            if dd > max_dd {
                max_dd = dd;
            }
        }
    }

    let mut returns = Vec::new();
    for pair in equity_curve.windows(2) {
        let prev = pair[0].equity;
        if prev > 0.0 {
            returns.push(pair[1].equity / prev - 1.0);
        }
    }

    let sharpe = if returns.len() > 1 {
        let mean = returns.iter().sum::<f64>() / returns.len() as f64;
        let variance =
            returns.iter().map(|r| (r - mean).powi(2)).sum::<f64>() / (returns.len() - 1) as f64;
        let std = variance.sqrt();
        if std > 0.0 {
            mean / std * bars_per_year(interval).sqrt()
        } else {
            0.0
        }
    } else {
        0.0
    };

    (max_dd, sharpe)
}

fn average(values: &[f64]) -> Option<f64> {
    if values.is_empty() {
        None
    } else {
        Some(values.iter().sum::<f64>() / values.len() as f64)
    }
}

pub fn bars_per_year(interval: &str) -> f64 {
    let (num, unit) = interval.split_at(interval.len().saturating_sub(1));
    let n: f64 = num.parse().unwrap_or(1.0);
    let per_year = match unit {
        "m" => 365.0 * 24.0 * 60.0,
        "h" => 365.0 * 24.0,
        "d" => 365.0,
        "w" => 52.0,
        _ => 365.0 * 24.0,
    };
    per_year / n
}

fn persist(result: &BacktestResult) -> Result<()> {
    let cfg = &result.config;
    fs::create_dir_all(&cfg.out_dir).context("创建回测输出目录失败")?;

    let base = format!(
        "{}_{}_{}_{}_{}",
        cfg.symbol,
        cfg.interval,
        cfg.strategy,
        format_day(cfg.start_ms),
        format_day(cfg.end_ms)
    );

    let json_path = cfg.out_dir.join(format!("{}.json", base));
    fs::write(
        &json_path,
        serde_json::to_string_pretty(result).context("序列化回测结果失败")?,
    )
    .context("写入回测结果失败")?;

    let equity_path = cfg.out_dir.join(format!("{}_equity.csv", base));
    fs::write(&equity_path, equity_csv(&result.equity_curve)).context("写入权益曲线失败")?;

    let trades_path = cfg.out_dir.join(format!("{}_trades.csv", base));
    fs::write(&trades_path, trades_csv(&result.trades)).context("写入交易明细失败")?;

    info!(
        "回测结果: {} / {} / {}",
        json_path.display(),
        equity_path.display(),
        trades_path.display()
    );

    Ok(())
}

fn equity_csv(points: &[EquityPoint]) -> String {
    let mut out = String::from("time,datetime,equity,price,position\n");
    for p in points {
        let _ = writeln!(
            out,
            "{},{},{:.2},{:.6},{}",
            p.time,
            p.datetime,
            p.equity,
            p.price,
            p.position.as_deref().unwrap_or("")
        );
    }
    out
}

fn trades_csv(fills: &[Fill]) -> String {
    let mut out = String::from("time,datetime,action,price,qty,fee,pnl,equity_after,reason\n");
    for f in fills {
        let pnl = f.pnl.map(|v| format!("{:.4}", v)).unwrap_or_default();
        let _ = writeln!(
            out,
            "{},{},{},{:.6},{:.8},{:.6},{},{:.2},\"{}\"",
            f.time,
            f.datetime,
            f.action,
            f.price,
            f.qty,
            f.fee,
            pnl,
            f.equity_after,
            f.reason.replace('"', "'")
        );
    }
    out
}

fn print_report(result: &BacktestResult) {
    let m = &result.metrics;
    let cfg = &result.config;

    println!();
    println!("================ 回测结果 ================");
    println!(
        "标的: {} | 周期: {} | 策略: {}",
        cfg.symbol, cfg.interval, cfg.strategy
    );
    println!(
        "区间: {} → {}（{} 根K线）",
        format_time(cfg.start_ms),
        format_time(cfg.end_ms),
        m.bars
    );
    println!(
        "成本: 手续费 {:.3}% / 单边 | 滑点 {:.1} bps",
        cfg.fee_rate * 100.0,
        cfg.slippage_bps
    );
    println!("-----------------------------------------");
    println!("初始资金:      {:.2} USDT", m.initial_equity);
    println!("最终权益:      {:.2} USDT", m.final_equity);
    println!(
        "策略收益:      {:+.2}%  |  买入持有: {:+.2}%",
        m.total_return_pct, m.buy_and_hold_return_pct
    );
    println!("最大回撤:      {:.2}%", m.max_drawdown_pct);
    println!("Sharpe:        {:.2}", m.sharpe);
    println!(
        "交易次数:      {}  |  胜率: {:.1}%  |  暴露时间: {:.1}%",
        m.trade_count, m.win_rate_pct, m.exposure_pct
    );
    println!(
        "盈亏比:        {}",
        m.profit_factor
            .map(|v| format!("{:.2}", v))
            .unwrap_or_else(|| "∞（无亏损单）".to_string())
    );
    println!(
        "平均盈利/亏损: {} / {}",
        m.avg_win
            .map(|v| format!("{:+.2}", v))
            .unwrap_or_else(|| "-".to_string()),
        m.avg_loss
            .map(|v| format!("{:.2}", v))
            .unwrap_or_else(|| "-".to_string())
    );
    println!("累计手续费:    {:.2} USDT", m.total_fees);
    if m.liquidated {
        println!("⚠️  账户在回测期间爆仓，结果已截断");
    }
    println!("=========================================");
    println!();
}

fn format_time(ms: i64) -> String {
    Utc.timestamp_millis_opt(ms)
        .single()
        .map(|dt| dt.format("%Y-%m-%d %H:%M").to_string())
        .unwrap_or_else(|| ms.to_string())
}

fn format_day(ms: i64) -> String {
    Utc.timestamp_millis_opt(ms)
        .single()
        .map(|dt| dt.format("%Y%m%d").to_string())
        .unwrap_or_else(|| ms.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::strategy::Direction;

    fn broker() -> PaperBroker {
        PaperBroker::new(10_000.0, 0.0005, 0.0)
    }

    #[test]
    fn open_long_then_close_profitable() {
        let mut b = broker();
        b.execute(Signal::Buy, 100.0, 1, 0.5, 1.0, "test").unwrap();
        assert!(matches!(
            b.position.as_ref().unwrap().side,
            PositionSide::Long
        ));
        assert!((b.position.as_ref().unwrap().qty - 50.0).abs() < 1e-9);

        let entry_fee = 100.0 * 50.0 * 0.0005;
        b.close(110.0, 2, "exit").unwrap();

        let net = (110.0 - 100.0) * 50.0 - entry_fee - 110.0 * 50.0 * 0.0005;
        assert!((b.cash - (10_000.0 + net)).abs() < 1e-6);
        assert!((b.fills.last().unwrap().pnl.unwrap() - net).abs() < 1e-6);
    }

    #[test]
    fn sell_flips_long_position() {
        let mut b = broker();
        b.execute(Signal::Buy, 100.0, 1, 0.5, 1.0, "long").unwrap();
        b.execute(Signal::Sell, 105.0, 2, 0.5, 1.0, "flip").unwrap();

        assert!(matches!(
            b.position.as_ref().unwrap().side,
            PositionSide::Short
        ));
        assert_eq!(b.fills.len(), 3); // open long, close long, open short
        assert!(b.fills[1].pnl.is_some_and(|p| p > 0.0));
    }

    #[test]
    fn close_signal_flattens_without_reopening() {
        let mut b = broker();
        b.execute(Signal::Buy, 100.0, 1, 0.5, 1.0, "long").unwrap();
        b.execute(Signal::Close, 105.0, 2, 0.5, 1.0, "exit")
            .unwrap();

        assert!(b.position.is_none());
        assert_eq!(b.fills.len(), 2);
        assert!(matches!(b.fills[1].action, TradeAction::CloseLong));
        assert!(b.fills[1].pnl.is_some_and(|p| p > 0.0));
    }

    #[test]
    fn same_direction_does_not_add() {
        let mut b = broker();
        b.execute(Signal::Buy, 100.0, 1, 0.5, 1.0, "first").unwrap();
        b.execute(Signal::Buy, 101.0, 2, 0.5, 1.0, "again").unwrap();
        assert_eq!(b.fills.len(), 1);
    }

    #[test]
    fn hold_does_nothing() {
        let mut b = broker();
        b.execute(Signal::Hold, 100.0, 1, 0.5, 1.0, "hold").unwrap();
        assert!(b.position.is_none());
        assert!(b.fills.is_empty());
    }

    #[test]
    fn slippage_moves_fill_price_against_you() {
        let mut b = PaperBroker::new(10_000.0, 0.0, 10.0); // 10 bps
        b.execute(Signal::Buy, 100.0, 1, 0.5, 1.0, "slip").unwrap();
        assert!((b.fills[0].price - 100.1).abs() < 1e-9);
    }

    #[test]
    fn drawdown_and_sharpe_from_curve() {
        let curve = vec![
            point(1, 100.0),
            point(2, 120.0),
            point(3, 90.0),
            point(4, 110.0),
        ];
        let (dd, _) = drawdown_and_sharpe(&curve, "1h");
        assert!((dd - 25.0).abs() < 1e-9); // 120 → 90
    }

    #[test]
    fn bars_per_year_is_sane() {
        assert!((bars_per_year("1h") - 8760.0).abs() < 1e-9);
        assert!((bars_per_year("15m") - 35040.0).abs() < 1e-9);
        assert!((bars_per_year("1d") - 365.0).abs() < 1e-9);
    }

    #[test]
    fn metrics_count_wins_and_losses() {
        let mut b = broker();
        b.execute(Signal::Buy, 100.0, 1, 0.5, 1.0, "w").unwrap();
        b.close(110.0, 2, "win").unwrap();
        b.execute(Signal::Sell, 110.0, 3, 0.5, 1.0, "l").unwrap();
        b.close(120.0, 4, "loss").unwrap();

        let curve = vec![point(1, 10_000.0), point(2, b.cash)];
        let m = compute_metrics(10_000.0, "1h", 0.0, &b, &curve, false);

        assert_eq!(m.trade_count, 2);
        assert!((m.win_rate_pct - 50.0).abs() < 1e-9);
        assert!(m.profit_factor.is_some());
    }

    fn point(time: i64, equity: f64) -> EquityPoint {
        EquityPoint {
            time,
            datetime: String::new(),
            equity,
            price: 100.0,
            position: None,
        }
    }

    // 保证 Direction 在测试里可用（避免未使用导入）
    #[test]
    fn direction_hold_compiles() {
        let d = Direction {
            signal: Signal::Hold,
            confidence: crate::types::Confidence::Low,
            reason: String::new(),
        };
        assert_eq!(d.signal, Signal::Hold);
    }
}
