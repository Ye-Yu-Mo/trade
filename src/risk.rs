// 确定性风控层。
//
// 原则：LLM 只能提出意图，代码决定能不能执行。
// 这里不调用任何模型，所有规则都可以被单元测试覆盖。

use crate::logging;
use crate::types::{Position, PositionSide, Signal, TradingDecision};
use anyhow::{Context, Result};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::fs;
use std::path::Path;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RiskLimits {
    pub enabled: bool,
    /// 仓位浮亏达到开仓名义价值的该比例时强制平仓
    pub stop_loss_pct: f64,
    /// 仓位浮盈达到开仓名义价值的该比例时强制平仓
    pub take_profit_pct: f64,
    /// 从权益峰值回撤达到该比例时熔断
    pub max_drawdown_pct: f64,
    /// 当日亏损达到该比例时熔断
    pub daily_loss_pct: f64,
    /// 熔断时是否强制平掉已有仓位
    pub close_on_halt: bool,
    /// 是否在交易所挂 closePosition 条件单（止损/止盈）
    pub exchange_stops: bool,
    /// 条件单触发价类型：MARK_PRICE 或 CONTRACT_PRICE
    pub trigger_type: String,
}

impl Default for RiskLimits {
    fn default() -> Self {
        Self {
            enabled: true,
            stop_loss_pct: 0.02,
            take_profit_pct: 0.05,
            max_drawdown_pct: 0.20,
            daily_loss_pct: 0.05,
            close_on_halt: true,
            exchange_stops: true,
            trigger_type: "MARK_PRICE".to_string(),
        }
    }
}

impl RiskLimits {
    pub fn from_env() -> Self {
        let defaults = Self::default();
        let parse_pct = |key: &str, fallback: f64| -> f64 {
            std::env::var(key)
                .ok()
                .and_then(|v| v.parse::<f64>().ok())
                .filter(|v| *v > 0.0 && *v < 1.0)
                .unwrap_or(fallback)
        };
        let flag = |key: &str, fallback: bool| -> bool {
            match std::env::var(key).as_deref() {
                Ok("false") | Ok("0") | Ok("off") => false,
                Ok("true") | Ok("1") | Ok("on") => true,
                _ => fallback,
            }
        };

        Self {
            enabled: flag("RISK_ENABLED", defaults.enabled),
            stop_loss_pct: parse_pct("RISK_STOP_LOSS_PCT", defaults.stop_loss_pct),
            take_profit_pct: parse_pct("RISK_TAKE_PROFIT_PCT", defaults.take_profit_pct),
            max_drawdown_pct: parse_pct("RISK_MAX_DRAWDOWN_PCT", defaults.max_drawdown_pct),
            daily_loss_pct: parse_pct("RISK_DAILY_LOSS_PCT", defaults.daily_loss_pct),
            close_on_halt: flag("RISK_CLOSE_ON_HALT", defaults.close_on_halt),
            exchange_stops: flag("RISK_EXCHANGE_STOPS", defaults.exchange_stops),
            trigger_type: match std::env::var("RISK_TRIGGER_TYPE").as_deref() {
                Ok("CONTRACT_PRICE") => "CONTRACT_PRICE".to_string(),
                _ => defaults.trigger_type,
            },
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum HaltKind {
    DailyLoss,
    MaxDrawdown,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct RiskState {
    pub peak_equity: f64,
    /// UTC 日期，格式 YYYY-MM-DD
    pub day: String,
    pub day_start_equity: f64,
    pub last_equity: f64,
    pub halt: Option<HaltKind>,
    pub halt_reason: Option<String>,
}

pub struct RiskGuard {
    pub limits: RiskLimits,
    state: RiskState,
}

impl RiskGuard {
    pub fn load(limits: RiskLimits) -> Self {
        let state = read_state().unwrap_or_default();
        Self { limits, state }
    }

    pub fn halted(&self) -> bool {
        self.limits.enabled && self.state.halt.is_some()
    }

    pub fn state(&self) -> &RiskState {
        &self.state
    }

    pub fn halt_reason(&self) -> Option<&str> {
        self.state.halt_reason.as_deref()
    }

    /// 用当前账户权益更新峰值、日内基准和熔断状态。
    /// 返回 Some(reason) 表示本次调用新触发了熔断。
    pub fn observe_equity(&mut self, equity: f64) -> Option<String> {
        self.observe_equity_at(equity, Utc::now())
    }

    fn observe_equity_at(&mut self, equity: f64, now: DateTime<Utc>) -> Option<String> {
        if !self.limits.enabled || equity <= 0.0 {
            return None;
        }

        let today = now.format("%Y-%m-%d").to_string();

        if self.state.day.is_empty() {
            self.state.day = today.clone();
            self.state.day_start_equity = equity;
        } else if self.state.day != today {
            self.state.day = today;
            self.state.day_start_equity = equity;
            // 日内亏损熔断按自然日重置；回撤熔断必须人工介入（重启）才解除
            if self.state.halt == Some(HaltKind::DailyLoss) {
                self.state.halt = None;
                self.state.halt_reason = None;
            }
        }

        if equity > self.state.peak_equity {
            self.state.peak_equity = equity;
        }
        self.state.last_equity = equity;

        if self.halted() {
            return None;
        }

        if let Some(reason) = self.drawdown_breach(equity) {
            self.state.halt = Some(HaltKind::MaxDrawdown);
            self.state.halt_reason = Some(reason.clone());
            return Some(reason);
        }

        if let Some(reason) = self.daily_breach(equity) {
            self.state.halt = Some(HaltKind::DailyLoss);
            self.state.halt_reason = Some(reason.clone());
            return Some(reason);
        }

        None
    }

    fn drawdown_breach(&self, equity: f64) -> Option<String> {
        if self.state.peak_equity <= 0.0 || self.limits.max_drawdown_pct <= 0.0 {
            return None;
        }
        let drawdown = (self.state.peak_equity - equity) / self.state.peak_equity;
        if drawdown >= self.limits.max_drawdown_pct {
            Some(format!(
                "权益回撤 {:.2}% 达到上限 {:.2}%（峰值 {:.2} → 当前 {:.2}）",
                drawdown * 100.0,
                self.limits.max_drawdown_pct * 100.0,
                self.state.peak_equity,
                equity
            ))
        } else {
            None
        }
    }

    fn daily_breach(&self, equity: f64) -> Option<String> {
        if self.state.day_start_equity <= 0.0 || self.limits.daily_loss_pct <= 0.0 {
            return None;
        }
        let loss = (self.state.day_start_equity - equity) / self.state.day_start_equity;
        if loss >= self.limits.daily_loss_pct {
            Some(format!(
                "日内亏损 {:.2}% 达到上限 {:.2}%（日初 {:.2} → 当前 {:.2}）",
                loss * 100.0,
                self.limits.daily_loss_pct * 100.0,
                self.state.day_start_equity,
                equity
            ))
        } else {
            None
        }
    }

    /// 止损 / 止盈检查。返回 Some(reason) 表示应当强制平仓。
    pub fn exit_reason(&self, position: &Position) -> Option<String> {
        if !self.limits.enabled {
            return None;
        }
        exit_reason_for(
            position,
            self.limits.stop_loss_pct,
            self.limits.take_profit_pct,
        )
    }

    /// 熔断期间是否应当强制平仓出场。
    pub fn halt_exit_reason(&self) -> Option<String> {
        if self.halted() && self.limits.close_on_halt {
            Some(format!(
                "风控熔断，强制平仓: {}",
                self.halt_reason().unwrap_or("trading halted")
            ))
        } else {
            None
        }
    }

    /// 熔断期间禁止开仓 / 加仓。
    /// Buy/Sell 若会开启新敞口，则降级为 Close 或 Hold。
    pub fn gate(&self, decision: &mut TradingDecision, position: &Option<Position>) {
        if !self.halted() {
            return;
        }

        let downgraded = match (&decision.signal, position) {
            (Signal::Buy, Some(p)) | (Signal::Sell, Some(p))
                if self.limits.close_on_halt
                    && ((matches!(decision.signal, Signal::Buy)
                        && p.side == PositionSide::Short)
                        || (matches!(decision.signal, Signal::Sell)
                            && p.side == PositionSide::Long)) =>
            {
                Some(Signal::Close)
            }
            (Signal::Buy, _) | (Signal::Sell, _) => Some(Signal::Hold),
            _ => None,
        };

        if let Some(signal) = downgraded {
            decision.signal = signal;
            decision.amount = 0.0;
            decision.reason = format!(
                "[风控熔断] {} | {}",
                self.halt_reason().unwrap_or("trading halted"),
                decision.reason
            );
        }
    }

    pub fn persist(&self) -> Result<()> {
        let dir = Path::new(logging::logs_directory());
        fs::create_dir_all(dir).context("创建logs目录失败")?;
        let path = dir.join("risk_state.json");
        fs::write(
            &path,
            serde_json::to_string_pretty(&self.state).context("序列化风控状态失败")?,
        )
        .context("写入风控状态失败")?;
        Ok(())
    }
}

/// 与 RiskGuard 解耦的纯函数，回测和实盘共用。
pub fn exit_reason_for(
    position: &Position,
    stop_loss_pct: f64,
    take_profit_pct: f64,
) -> Option<String> {
    let notional = position.entry_price * position.amount;
    if notional <= 0.0 {
        return None;
    }

    let pnl_pct = position.unrealized_pnl / notional;

    if stop_loss_pct > 0.0 && pnl_pct <= -stop_loss_pct {
        return Some(format!(
            "止损触发: 浮亏 {:.2}% ≤ -{:.2}%",
            pnl_pct * 100.0,
            stop_loss_pct * 100.0
        ));
    }

    if take_profit_pct > 0.0 && pnl_pct >= take_profit_pct {
        return Some(format!(
            "止盈触发: 浮盈 {:.2}% ≥ {:.2}%",
            pnl_pct * 100.0,
            take_profit_pct * 100.0
        ));
    }

    None
}

/// 用一根K线的高低点判断止损/止盈是否在盘中触发。
/// 返回 (成交价, 触发类型)。
///
/// 同时触发止损和止盈时，按止损处理（保守假设）。
/// 跳空开盘时按开盘价成交，而不是理想的触发价。
pub fn intrabar_exit(
    side: &PositionSide,
    entry: f64,
    open: f64,
    high: f64,
    low: f64,
    stop_loss_pct: f64,
    take_profit_pct: f64,
) -> Option<(f64, &'static str)> {
    if entry <= 0.0 {
        return None;
    }

    let stop_price = match side {
        PositionSide::Long => entry * (1.0 - stop_loss_pct),
        PositionSide::Short => entry * (1.0 + stop_loss_pct),
    };
    let take_price = match side {
        PositionSide::Long => entry * (1.0 + take_profit_pct),
        PositionSide::Short => entry * (1.0 - take_profit_pct),
    };

    let stop_hit = stop_loss_pct > 0.0
        && match side {
            PositionSide::Long => low <= stop_price,
            PositionSide::Short => high >= stop_price,
        };
    let take_hit = take_profit_pct > 0.0
        && match side {
            PositionSide::Long => high >= take_price,
            PositionSide::Short => low <= take_price,
        };

    if stop_hit {
        // 跳空时按更差的开盘价成交
        let fill = match side {
            PositionSide::Long => stop_price.min(open),
            PositionSide::Short => stop_price.max(open),
        };
        return Some((fill, "止损"));
    }

    if take_hit {
        let fill = match side {
            PositionSide::Long => take_price.max(open),
            PositionSide::Short => take_price.min(open),
        };
        return Some((fill, "止盈"));
    }

    None
}

fn read_state() -> Option<RiskState> {
    let path = Path::new(logging::logs_directory()).join("risk_state.json");
    fs::read_to_string(path)
        .ok()
        .and_then(|text| serde_json::from_str(&text).ok())
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeZone;

    fn position(side: PositionSide, entry: f64, amount: f64, pnl: f64) -> Position {
        Position {
            side,
            amount,
            entry_price: entry,
            unrealized_pnl: pnl,
        }
    }

    fn guard() -> RiskGuard {
        RiskGuard {
            limits: RiskLimits {
                enabled: true,
                stop_loss_pct: 0.02,
                take_profit_pct: 0.05,
                max_drawdown_pct: 0.20,
                daily_loss_pct: 0.05,
                close_on_halt: true,
                exchange_stops: false,
                trigger_type: "MARK_PRICE".to_string(),
            },
            state: RiskState::default(),
        }
    }

    fn at(day: u32) -> DateTime<Utc> {
        Utc.with_ymd_and_hms(2026, 1, day, 12, 0, 0).unwrap()
    }

    #[test]
    fn stop_loss_triggers_at_threshold() {
        let pos = position(PositionSide::Long, 100.0, 1.0, -2.0);
        let reason = exit_reason_for(&pos, 0.02, 0.05).unwrap();
        assert!(reason.contains("止损"));
    }

    #[test]
    fn stop_loss_not_triggered_below_threshold() {
        let pos = position(PositionSide::Long, 100.0, 1.0, -1.99);
        assert!(exit_reason_for(&pos, 0.02, 0.05).is_none());
    }

    #[test]
    fn take_profit_triggers_for_short_too() {
        let pos = position(PositionSide::Short, 100.0, 1.0, 5.0);
        assert!(exit_reason_for(&pos, 0.02, 0.05).unwrap().contains("止盈"));
    }

    #[test]
    fn drawdown_halts_and_persists_same_day() {
        let mut g = guard();
        // 只看回撤，避免 5% 的日内亏损规则先触发
        g.limits.daily_loss_pct = 0.99;
        assert!(g.observe_equity_at(10_000.0, at(1)).is_none());
        assert!(g.observe_equity_at(8_500.0, at(1)).is_none()); // -15%
        let reason = g.observe_equity_at(7_900.0, at(1)).unwrap(); // -21%
        assert!(reason.contains("回撤"));
        assert!(g.halted());
        assert_eq!(g.state.halt, Some(HaltKind::MaxDrawdown));

        // 回撤熔断不会在次日自动解除
        assert!(g.observe_equity_at(7_900.0, at(2)).is_none());
        assert!(g.halted());
    }

    #[test]
    fn daily_loss_halts_and_resets_next_day() {
        let mut g = guard();
        assert!(g.observe_equity_at(10_000.0, at(1)).is_none());
        let reason = g.observe_equity_at(9_400.0, at(1)).unwrap(); // -6%
        assert!(reason.contains("日内亏损"));
        assert!(g.halted());

        assert!(g.observe_equity_at(9_400.0, at(2)).is_none());
        assert!(!g.halted());
    }

    #[test]
    fn gate_blocks_new_positions_when_halted() {
        let mut g = guard();
        g.observe_equity_at(10_000.0, at(1));
        g.observe_equity_at(7_000.0, at(1));
        assert!(g.halted());

        let mut open = TradingDecision {
            signal: Signal::Buy,
            reason: "test".into(),
            confidence: crate::types::Confidence::High,
            amount: 1.0,
        };
        g.gate(&mut open, &None);
        assert_eq!(open.signal, Signal::Hold);
        assert_eq!(open.amount, 0.0);
    }

    #[test]
    fn intrabar_long_stop_uses_stop_price() {
        let hit = intrabar_exit(&PositionSide::Long, 100.0, 100.0, 101.0, 97.5, 0.02, 0.05);
        let (fill, label) = hit.unwrap();
        assert_eq!(label, "止损");
        assert!((fill - 98.0).abs() < 1e-9);
    }

    #[test]
    fn intrabar_long_stop_respects_gap() {
        let hit = intrabar_exit(&PositionSide::Long, 100.0, 96.0, 96.5, 95.0, 0.02, 0.05);
        let (fill, label) = hit.unwrap();
        assert_eq!(label, "止损");
        assert!((fill - 96.0).abs() < 1e-9); // 以开盘价成交
    }

    #[test]
    fn intrabar_long_take_profit() {
        let hit = intrabar_exit(&PositionSide::Long, 100.0, 100.0, 106.0, 99.0, 0.02, 0.05);
        let (fill, label) = hit.unwrap();
        assert_eq!(label, "止盈");
        assert!((fill - 105.0).abs() < 1e-9);
    }

    #[test]
    fn intrabar_stop_wins_when_both_touched() {
        let hit = intrabar_exit(&PositionSide::Long, 100.0, 100.0, 106.0, 97.0, 0.02, 0.05);
        assert_eq!(hit.unwrap().1, "止损");
    }

    #[test]
    fn intrabar_short_stop() {
        let hit = intrabar_exit(&PositionSide::Short, 100.0, 100.0, 103.0, 99.0, 0.02, 0.05);
        let (fill, label) = hit.unwrap();
        assert_eq!(label, "止损");
        assert!((fill - 102.0).abs() < 1e-9);
    }

    #[test]
    fn gate_downgrades_reversal_to_close() {
        let mut g = guard();
        g.observe_equity_at(10_000.0, at(1));
        g.observe_equity_at(7_000.0, at(1));

        let short = position(PositionSide::Short, 100.0, 1.0, 0.0);
        let mut decision = TradingDecision {
            signal: Signal::Buy,
            reason: "reversal".into(),
            confidence: crate::types::Confidence::High,
            amount: 1.0,
        };
        g.gate(&mut decision, &Some(short));
        assert_eq!(decision.signal, Signal::Close);
    }
}
