// 命令行入口。
//
// `trade`（无子命令）保持原有行为：启动实盘/测试网交易循环。
// `trade backtest` 进入历史回测，不需要 Binance API key。

use crate::backtest::{self, BacktestConfig};
use crate::data;
use crate::strategy::Strategy;
use anyhow::{anyhow, bail, Result};
use chrono::Utc;
use clap::{Args, Parser, Subcommand, ValueEnum};
use std::env;
use std::path::PathBuf;

const DEFAULT_LOOKBACK_DAYS: i64 = 180;

#[derive(Debug, Parser)]
#[command(
    name = "trade",
    version,
    about = "多智能体加密货币交易系统 / 可回测的 LLM 交易框架"
)]
pub struct Cli {
    #[command(subcommand)]
    pub command: Option<Command>,
}

#[derive(Debug, Subcommand)]
pub enum Command {
    /// 启动实盘 / 测试网交易循环（默认行为）
    Run,
    /// 用历史数据回测策略
    Backtest(Box<BacktestArgs>),
}

#[derive(Debug, Clone, Copy, ValueEnum)]
pub enum StrategyArg {
    /// SMA5/SMA20 交叉，确定性基线，不需要 API key
    Sma,
    /// DeepSeek 单智能体策略，需要 DEEPSEEK_API_KEY
    Llm,
}

#[derive(Debug, Args)]
pub struct BacktestArgs {
    /// 交易对，例如 BTCUSDT
    #[arg(long, default_value = "BTCUSDT")]
    pub symbol: String,

    /// K线周期：1m, 15m, 30m, 1h, 4h, 1d
    #[arg(long, default_value = "1h")]
    pub interval: String,

    /// 开始时间，例如 2024-01-01 或 2024-01-01 08:00；默认结束时间前 180 天
    #[arg(long)]
    pub start: Option<String>,

    /// 结束时间；默认当前时间
    #[arg(long)]
    pub end: Option<String>,

    /// 策略
    #[arg(long, value_enum, default_value_t = StrategyArg::Sma)]
    pub strategy: StrategyArg,

    /// 初始资金（USDT）
    #[arg(long, default_value_t = 10_000.0)]
    pub initial_balance: f64,

    /// 每次开仓占当前权益的比例 (0, 1]
    #[arg(long, default_value_t = 0.2)]
    pub position_pct: f64,

    /// 杠杆倍数。回测默认 1 倍，避免制造虚假收益
    #[arg(long, default_value_t = 1.0)]
    pub leverage: f64,

    /// 单边手续费（bps）。Binance U 本位 taker 约 5 bps
    #[arg(long, default_value_t = 5.0)]
    pub fee_bps: f64,

    /// 滑点（bps）
    #[arg(long, default_value_t = 2.0)]
    pub slippage_bps: f64,

    /// 止损：仓位浮亏达到开仓名义价值的该比例时平仓
    #[arg(long, default_value_t = 0.02)]
    pub stop_loss_pct: f64,

    /// 止盈：仓位浮盈达到开仓名义价值的该比例时平仓
    #[arg(long, default_value_t = 0.05)]
    pub take_profit_pct: f64,

    /// 指标预热所需K线数量
    #[arg(long, default_value_t = 120)]
    pub warmup: usize,

    /// 最多回测多少根K线（LLM 策略建议配合缓存和此参数控制成本）
    #[arg(long)]
    pub max_bars: Option<usize>,

    /// K线缓存目录
    #[arg(long, default_value = "data/klines")]
    pub data_dir: PathBuf,

    /// 回测结果输出目录
    #[arg(long, default_value = "backtest")]
    pub out_dir: PathBuf,
}

pub async fn run_backtest(args: BacktestArgs) -> Result<()> {
    let mut end_ms = match &args.end {
        Some(value) => data::parse_time(value)?,
        None => Utc::now().timestamp_millis(),
    };

    // 默认结束时间对齐到K线边界，避免每次运行都生成新的缓存键
    let step = data::interval_millis(&args.interval);
    if args.end.is_none() {
        if let Some(step) = step {
            end_ms = end_ms / step * step;
        }
    }

    let mut start_ms = match &args.start {
        Some(value) => data::parse_time(value)?,
        None => end_ms - DEFAULT_LOOKBACK_DAYS * 24 * 3600 * 1000,
    };

    if args.start.is_none() {
        if let Some(step) = step {
            start_ms = start_ms / step * step;
        }
    }

    if start_ms >= end_ms {
        bail!("开始时间必须早于结束时间");
    }

    let strategy = match args.strategy {
        StrategyArg::Sma => Strategy::sma_cross(),
        StrategyArg::Llm => {
            let api_key = env::var("DEEPSEEK_API_KEY")
                .map_err(|_| anyhow!("--strategy llm 需要设置 DEEPSEEK_API_KEY"))?;
            println!(
                "提示：LLM 策略会为每根K线调用一次模型，响应会缓存在 .cache/llm/。\n\
                 建议先用 --max-bars 控制规模，重复运行同一区间不会再产生费用。"
            );
            Strategy::llm(api_key)
        }
    };

    let strategy_name = strategy.name().to_string();

    let config = BacktestConfig {
        symbol: args.symbol,
        interval: args.interval,
        start_ms,
        end_ms,
        strategy: strategy_name,
        initial_balance: args.initial_balance,
        position_pct: args.position_pct,
        leverage: args.leverage,
        fee_rate: args.fee_bps / 10_000.0,
        slippage_bps: args.slippage_bps,
        stop_loss_pct: args.stop_loss_pct,
        take_profit_pct: args.take_profit_pct,
        warmup: args.warmup,
        data_dir: args.data_dir,
        out_dir: args.out_dir,
        max_bars: args.max_bars,
    };

    backtest::run(config, strategy).await?;
    Ok(())
}
