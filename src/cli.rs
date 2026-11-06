// 命令行入口。
//
// `trade`（无子命令）保持原有行为：启动实盘/测试网交易循环。
// `trade backtest` 进入历史回测，不需要 Binance API key。

use crate::analyze;
use crate::backtest::{self, BacktestConfig};
use crate::dashboard;
use crate::data;
use crate::demo;
use crate::strategy::Strategy;
use crate::walkforward::{self, WalkForwardConfig};
use anyhow::{anyhow, bail, Context, Result};
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
    Run(RunArgs),
    /// 用历史数据回测策略
    Backtest(Box<BacktestArgs>),
    /// 启动只读 Dashboard（读取 logs/ 与 backtest/）
    Dashboard(DashboardArgs),
    /// Walk-forward 样本外回测，支持多标的
    Walkforward(WalkForwardArgs),
    /// 对最新K线跑一次多智能体决策链（dry-run，不下单）
    Analyze(AnalyzeArgs),
    /// 离线 Demo：无需 API key，一条命令启动带示例数据的 Dashboard
    Demo(DemoArgs),
}

#[derive(Debug, Args, Default)]
pub struct RunArgs {
    /// 同时启动只读 Dashboard 的端口（默认不启动）
    #[arg(long)]
    pub dashboard: Option<u16>,

    /// Dashboard 绑定地址
    #[arg(long, default_value = "127.0.0.1")]
    pub dashboard_host: String,
}

#[derive(Debug, Args)]
pub struct DashboardArgs {
    /// 绑定地址。只读服务，默认只监听本机
    #[arg(long, default_value = "127.0.0.1")]
    pub host: String,

    /// 端口
    #[arg(long, default_value_t = 8080)]
    pub port: u16,

    /// 日志目录
    #[arg(long, default_value = "logs")]
    pub logs_dir: PathBuf,

    /// 回测结果目录
    #[arg(long, default_value = "backtest")]
    pub backtest_dir: PathBuf,
}

#[derive(Debug, Args)]
pub struct WalkForwardArgs {
    /// 标的列表，逗号分隔
    #[arg(long, default_value = "BTCUSDT,ETHUSDT")]
    pub symbols: String,

    /// K线周期
    #[arg(long, default_value = "1h")]
    pub interval: String,

    /// 开始时间；默认结束时间前 365 天
    #[arg(long)]
    pub start: Option<String>,

    /// 结束时间；默认当前时间
    #[arg(long)]
    pub end: Option<String>,

    /// 每个训练窗口的K线数量
    #[arg(long, default_value_t = 720)]
    pub train_bars: usize,

    /// 每个测试窗口的K线数量，同时也是滚动步长
    #[arg(long, default_value_t = 240)]
    pub test_bars: usize,

    /// fast 均线候选，逗号分隔
    #[arg(long, default_value = "5,10,20")]
    pub fast: String,

    /// slow 均线候选，逗号分隔
    #[arg(long, default_value = "20,50,100")]
    pub slow: String,

    /// 初始资金（多标的时等权分配）
    #[arg(long, default_value_t = 10_000.0)]
    pub initial_balance: f64,

    /// 每次开仓占当前权益的比例
    #[arg(long, default_value_t = 0.2)]
    pub position_pct: f64,

    /// 杠杆倍数
    #[arg(long, default_value_t = 1.0)]
    pub leverage: f64,

    /// 单边手续费（bps）
    #[arg(long, default_value_t = 5.0)]
    pub fee_bps: f64,

    /// 滑点（bps）
    #[arg(long, default_value_t = 2.0)]
    pub slippage_bps: f64,

    /// 止损
    #[arg(long, default_value_t = 0.02)]
    pub stop_loss_pct: f64,

    /// 止盈
    #[arg(long, default_value_t = 0.05)]
    pub take_profit_pct: f64,

    /// 指标预热K线数（会自动不小于最大 slow + 5）
    #[arg(long, default_value_t = 120)]
    pub warmup: usize,

    /// K线缓存目录
    #[arg(long, default_value = "data/klines")]
    pub data_dir: PathBuf,

    /// 报告输出目录
    #[arg(long, default_value = "backtest")]
    pub out_dir: PathBuf,
}

#[derive(Debug, Args)]
pub struct AnalyzeArgs {
    /// 交易对
    #[arg(long, default_value = "BTCUSDT")]
    pub symbol: String,

    /// K线周期
    #[arg(long, default_value = "1h")]
    pub interval: String,

    /// 虚拟账户权益（USDT）
    #[arg(long, default_value_t = 10_000.0)]
    pub balance: f64,

    /// 单次仓位占权益比例
    #[arg(long, default_value_t = 0.2)]
    pub position_pct: f64,
}

#[derive(Debug, Args)]
pub struct DemoArgs {
    /// Dashboard 端口
    #[arg(long, default_value_t = 8080)]
    pub port: u16,

    /// 只生成数据，不启动 Dashboard
    #[arg(long)]
    pub no_server: bool,
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

fn resolve_range(
    start: &Option<String>,
    end: &Option<String>,
    interval: &str,
    lookback_days: i64,
) -> Result<(i64, i64)> {
    let mut end_ms = match end {
        Some(value) => data::parse_time(value)?,
        None => Utc::now().timestamp_millis(),
    };

    // 默认时间对齐到K线边界，避免每次运行都生成新的缓存键
    let step = data::interval_millis(interval);
    if end.is_none() {
        if let Some(step) = step {
            end_ms = end_ms / step * step;
        }
    }

    let mut start_ms = match start {
        Some(value) => data::parse_time(value)?,
        None => end_ms - lookback_days * 24 * 3600 * 1000,
    };

    if start.is_none() {
        if let Some(step) = step {
            start_ms = start_ms / step * step;
        }
    }

    if start_ms >= end_ms {
        bail!("开始时间必须早于结束时间");
    }

    Ok((start_ms, end_ms))
}

pub async fn run_backtest(args: BacktestArgs) -> Result<()> {
    let (start_ms, end_ms) = resolve_range(
        &args.start,
        &args.end,
        &args.interval,
        DEFAULT_LOOKBACK_DAYS,
    )?;

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

pub async fn run_dashboard(args: DashboardArgs) -> Result<()> {
    dashboard::serve(
        &args.host,
        args.port,
        dashboard::DashboardConfig {
            logs_dir: args.logs_dir,
            backtest_dir: args.backtest_dir,
        },
    )
    .await
}

fn parse_usize_list(input: &str, label: &str) -> Result<Vec<usize>> {
    let values: Vec<usize> = input
        .split(',')
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(|value| {
            value
                .parse::<usize>()
                .with_context(|| format!("{} 参数无效: {}", label, value))
        })
        .collect::<Result<_>>()?;

    if values.is_empty() {
        bail!("{} 不能为空", label);
    }
    Ok(values)
}

pub async fn run_walkforward(args: WalkForwardArgs) -> Result<()> {
    let symbols: Vec<String> = args
        .symbols
        .split(',')
        .map(str::trim)
        .filter(|symbol| !symbol.is_empty())
        .map(|symbol| symbol.to_string())
        .collect();

    if symbols.is_empty() {
        bail!("--symbols 不能为空");
    }

    let fasts = parse_usize_list(&args.fast, "--fast")?;
    let slows = parse_usize_list(&args.slow, "--slow")?;
    let (start_ms, end_ms) = resolve_range(&args.start, &args.end, &args.interval, 365)?;

    let config = WalkForwardConfig {
        symbols,
        interval: args.interval,
        start_ms,
        end_ms,
        train_bars: args.train_bars,
        test_bars: args.test_bars,
        warmup: args.warmup,
        fasts,
        slows,
        initial_balance: args.initial_balance,
        position_pct: args.position_pct,
        leverage: args.leverage,
        fee_rate: args.fee_bps / 10_000.0,
        slippage_bps: args.slippage_bps,
        stop_loss_pct: args.stop_loss_pct,
        take_profit_pct: args.take_profit_pct,
        data_dir: args.data_dir,
        out_dir: args.out_dir,
    };

    walkforward::run(config).await?;
    Ok(())
}

pub async fn run_analyze(args: AnalyzeArgs) -> Result<()> {
    let api_key =
        env::var("DEEPSEEK_API_KEY").map_err(|_| anyhow!("analyze 需要设置 DEEPSEEK_API_KEY"))?;

    analyze::run(analyze::AnalyzeConfig {
        symbol: args.symbol,
        interval: args.interval,
        balance: args.balance,
        position_pct: args.position_pct,
        api_key,
    })
    .await
}

pub async fn run_demo(args: DemoArgs) -> Result<()> {
    demo::run(demo::DemoConfig {
        port: args.port,
        no_server: args.no_server,
    })
    .await
}
