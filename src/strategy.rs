// 策略抽象：回测和实盘共用同一套“给方向”的接口。
//
// 策略只输出方向，不输出数量。仓位大小由执行层按资金比例决定，
// 这样策略逻辑与资金管理解耦，回测也可复现。

use crate::llm_client;
use crate::types::{Confidence, Kline, Position, Signal, TechnicalIndicators};
use anyhow::Result;
use serde::Deserialize;
use serde_json::json;

/// 每根K线传给策略的上下文。
pub struct BarContext<'a> {
    pub symbol: &'a str,
    pub interval: &'a str,
    pub window: &'a [Kline],
    pub indicators: &'a TechnicalIndicators,
    pub position: Option<&'a Position>,
    pub equity: f64,
    pub available_balance: f64,
}

#[derive(Debug, Clone)]
pub struct Direction {
    pub signal: Signal,
    pub confidence: Confidence,
    pub reason: String,
}

impl Direction {
    fn hold(reason: impl Into<String>) -> Self {
        Self {
            signal: Signal::Hold,
            confidence: Confidence::Low,
            reason: reason.into(),
        }
    }
}

/// 可插拔策略。
pub enum Strategy {
    SmaCross(SmaCrossStrategy),
    Llm(LlmStrategy),
}

impl Strategy {
    pub fn sma_cross() -> Self {
        Strategy::SmaCross(SmaCrossStrategy)
    }

    pub fn llm(api_key: String) -> Self {
        Strategy::Llm(LlmStrategy { api_key })
    }

    pub fn name(&self) -> &'static str {
        match self {
            Strategy::SmaCross(_) => "sma_cross",
            Strategy::Llm(_) => "llm",
        }
    }

    pub async fn decide(&mut self, ctx: &BarContext<'_>) -> Result<Direction> {
        match self {
            Strategy::SmaCross(s) => Ok(s.decide(ctx)),
            Strategy::Llm(s) => s.decide(ctx).await,
        }
    }
}

// ========== 确定性基线：SMA5 / SMA20 交叉 ==========
//
// 用途：不依赖 API key，任何人 clone 下来就能跑回测，
// 也是衡量 LLM 策略是否有超额收益的对照组。

pub struct SmaCrossStrategy;

impl SmaCrossStrategy {
    fn decide(&self, ctx: &BarContext<'_>) -> Direction {
        let fast = ctx.indicators.sma_5;
        let slow = ctx.indicators.sma_20;

        if slow.abs() < f64::EPSILON {
            return Direction::hold("均线数据不可用");
        }

        let gap_pct = (fast - slow) / slow * 100.0;
        let confidence = if gap_pct.abs() >= 0.5 {
            Confidence::High
        } else if gap_pct.abs() >= 0.1 {
            Confidence::Medium
        } else {
            Confidence::Low
        };

        if fast > slow {
            Direction {
                signal: Signal::Buy,
                confidence,
                reason: format!(
                    "SMA5 {:.2} 高于 SMA20 {:.2}（+{:.2}%）",
                    fast, slow, gap_pct
                ),
            }
        } else {
            Direction {
                signal: Signal::Sell,
                confidence,
                reason: format!("SMA5 {:.2} 低于 SMA20 {:.2}（{:.2}%）", fast, slow, gap_pct),
            }
        }
    }
}

// ========== LLM 单智能体基线 ==========
//
// 每根K线一次调用，结果走 llm_client 的磁盘缓存。
// 多智能体版本在回测里成本过高，留到后续版本。

pub struct LlmStrategy {
    api_key: String,
}

#[derive(Debug, Deserialize)]
struct LlmDirection {
    signal: Signal,
    confidence: Confidence,
    #[serde(default)]
    reason: String,
}

impl LlmStrategy {
    async fn decide(&mut self, ctx: &BarContext<'_>) -> Result<Direction> {
        let prompt = build_prompt(ctx)?;
        let response = llm_client::chat(SYSTEM_PROMPT, &prompt, &self.api_key).await?;
        let parsed: LlmDirection = llm_client::parse_json(&response)?;

        Ok(Direction {
            signal: parsed.signal,
            confidence: parsed.confidence,
            reason: parsed.reason,
        })
    }
}

fn build_prompt(ctx: &BarContext<'_>) -> Result<String> {
    let recent: Vec<_> = ctx
        .window
        .iter()
        .rev()
        .take(20)
        .collect::<Vec<_>>()
        .into_iter()
        .rev()
        .map(|k| {
            json!({
                "t": k.timestamp,
                "o": k.open,
                "h": k.high,
                "l": k.low,
                "c": k.close,
                "v": k.volume,
            })
        })
        .collect();

    let position = ctx.position.map(|p| {
        json!({
            "side": format!("{:?}", p.side),
            "amount": p.amount,
            "entry_price": p.entry_price,
            "unrealized_pnl": p.unrealized_pnl,
        })
    });

    let payload = json!({
        "symbol": ctx.symbol,
        "interval": ctx.interval,
        "equity": ctx.equity,
        "available_balance": ctx.available_balance,
        "position": position,
        "indicators": ctx.indicators,
        "recent_klines": recent,
    });

    Ok(format!(
        "根据以下 JSON 数据判断下一根K线的交易方向，只输出 JSON。\n{}",
        serde_json::to_string_pretty(&payload)?
    ))
}

const SYSTEM_PROMPT: &str = r#"你是一名加密货币趋势交易员。

规则：
1. 只依据给定的价格、均线、动量和成交量数据判断方向，不要编造数据。
2. BUY 表示持有多头，SELL 表示持有空头，HOLD 表示维持现状。
3. 趋势不明确时选择 HOLD，不要为了交易而交易。
4. reason 控制在 50 字以内。

严格输出 JSON，不要输出任何其他内容：
{
  "signal": "BUY" | "SELL" | "HOLD",
  "confidence": "HIGH" | "MEDIUM" | "LOW",
  "reason": "判断依据"
}"#;

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::TechnicalIndicators;

    fn indicators(sma_5: f64, sma_20: f64) -> TechnicalIndicators {
        TechnicalIndicators {
            sma_5,
            sma_20,
            sma_50: sma_20,
            sma_100: sma_20,
            price_change_1: 0.0,
            price_change_3: 0.0,
            price_change_6: 0.0,
            price_change_12: 0.0,
            atr_14: 0.0,
            atr_percent: 0.0,
            volume_ratio: 1.0,
        }
    }

    fn context<'a>(window: &'a [Kline], ind: &'a TechnicalIndicators) -> BarContext<'a> {
        BarContext {
            symbol: "BTCUSDT",
            interval: "1h",
            window,
            indicators: ind,
            position: None,
            equity: 10_000.0,
            available_balance: 10_000.0,
        }
    }

    #[test]
    fn golden_cross_is_buy() {
        let ind = indicators(101.0, 100.0);
        let bars = vec![Kline {
            timestamp: 0,
            open: 100.0,
            high: 101.0,
            low: 99.0,
            close: 100.0,
            volume: 1.0,
        }];
        let ctx = context(&bars, &ind);
        let dir = SmaCrossStrategy.decide(&ctx);
        assert_eq!(dir.signal, Signal::Buy);
        assert_eq!(dir.confidence, Confidence::High);
    }

    #[test]
    fn death_cross_is_sell() {
        let ind = indicators(99.0, 100.0);
        let bars = vec![Kline {
            timestamp: 0,
            open: 100.0,
            high: 101.0,
            low: 99.0,
            close: 100.0,
            volume: 1.0,
        }];
        let ctx = context(&bars, &ind);
        let dir = SmaCrossStrategy.decide(&ctx);
        assert_eq!(dir.signal, Signal::Sell);
    }

    #[test]
    fn tiny_gap_is_low_confidence() {
        let ind = indicators(100.02, 100.0);
        let bars = vec![Kline {
            timestamp: 0,
            open: 100.0,
            high: 101.0,
            low: 99.0,
            close: 100.0,
            volume: 1.0,
        }];
        let ctx = context(&bars, &ind);
        let dir = SmaCrossStrategy.decide(&ctx);
        assert_eq!(dir.signal, Signal::Buy);
        assert_eq!(dir.confidence, Confidence::Low);
    }

    #[test]
    fn missing_sma_means_hold() {
        let ind = indicators(100.0, 0.0);
        let bars = vec![Kline {
            timestamp: 0,
            open: 100.0,
            high: 101.0,
            low: 99.0,
            close: 100.0,
            volume: 1.0,
        }];
        let ctx = context(&bars, &ind);
        assert_eq!(SmaCrossStrategy.decide(&ctx).signal, Signal::Hold);
    }
}
