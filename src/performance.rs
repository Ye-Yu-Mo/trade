use crate::logging;
use crate::types::{TradeAction, TradeResult};
use anyhow::{Context, Result};
use chrono::Utc;
use serde::{Deserialize, Serialize};
use std::fs::{create_dir_all, write};
use std::path::Path;

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct PerformanceSnapshot {
    pub total_realized_pnl: f64,
    pub total_trades: u64,
    pub winning_trades: u64,
    pub losing_trades: u64,
    pub best_trade: Option<f64>,
    pub worst_trade: Option<f64>,
    pub equity_peak: f64,
    pub max_drawdown: f64,
    pub last_update: Option<i64>,
}

#[derive(Clone, Debug, Default)]
pub struct PerformanceTracker {
    snapshot: PerformanceSnapshot,
}

impl PerformanceTracker {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn snapshot(&self) -> &PerformanceSnapshot {
        &self.snapshot
    }

    pub fn update(&mut self, trade: &TradeResult) -> bool {
        if matches!(trade.action, TradeAction::Hold) {
            return false;
        }

        self.snapshot.total_trades += 1;

        if let Some(pnl) = trade.pnl {
            self.snapshot.total_realized_pnl += pnl;

            self.snapshot.best_trade = match self.snapshot.best_trade {
                Some(best) => Some(best.max(pnl)),
                None => Some(pnl),
            };

            self.snapshot.worst_trade = match self.snapshot.worst_trade {
                Some(worst) => Some(worst.min(pnl)),
                None => Some(pnl),
            };

            if pnl > 0.0 {
                self.snapshot.winning_trades += 1;
            } else if pnl < 0.0 {
                self.snapshot.losing_trades += 1;
            }

            let equity = self.snapshot.total_realized_pnl;
            if equity > self.snapshot.equity_peak {
                self.snapshot.equity_peak = equity;
            } else {
                let drawdown = self.snapshot.equity_peak - equity;
                if drawdown > self.snapshot.max_drawdown {
                    self.snapshot.max_drawdown = drawdown;
                }
            }
        }

        self.snapshot.last_update = Some(Utc::now().timestamp());
        true
    }

    pub fn persist(&self) -> Result<()> {
        let base_dir = Path::new(logging::logs_directory());
        create_dir_all(base_dir).context("创建logs目录失败")?;
        let path = base_dir.join("performance.json");
        let json = serde_json::to_string_pretty(&self.snapshot).context("序列化绩效数据失败")?;
        write(path, json).context("写入绩效数据失败")?;
        Ok(())
    }
}
