// 单智能体加密货币自动交易系统

mod executor;
mod llm;
mod market;
mod state;
mod types;

use anyhow::{Context, Result};
use dotenvy::dotenv;
use std::env;
use tokio::time::{interval, Duration};

// 配置结构
struct Config {
    binance_api_key: String,
    binance_secret: String,
    deepseek_api_key: String,
    trade_symbol: String,
    trade_amount: f64,
    trade_interval_secs: u64,
    leverage: u32,
}

impl Config {
    fn from_env() -> Result<Self> {
        Ok(Config {
            binance_api_key: env::var("BINANCE_API_KEY").context("缺少 BINANCE_API_KEY")?,
            binance_secret: env::var("BINANCE_SECRET").context("缺少 BINANCE_SECRET")?,
            deepseek_api_key: env::var("DEEPSEEK_API_KEY").context("缺少 DEEPSEEK_API_KEY")?,
            trade_symbol: env::var("TRADE_SYMBOL").unwrap_or_else(|_| "BTCUSDT".to_string()),
            trade_amount: env::var("TRADE_AMOUNT")
                .unwrap_or_else(|_| "0.001".to_string())
                .parse()
                .context("TRADE_AMOUNT 格式错误")?,
            trade_interval_secs: match env::var("TRADE_INTERVAL")
                .unwrap_or_else(|_| "1m".to_string())
                .as_str()
            {
                "1m" => 60,
                "15m" => 15 * 60,
                "30m" => 30 * 60,
                "1h" => 60 * 60,
                _ => 60,
            },
            leverage: env::var("LEVERAGE")
                .unwrap_or_else(|_| "10".to_string())
                .parse()
                .context("LEVERAGE 格式错误")?,
        })
    }
}

// Task 7.1: 单次交易周期
async fn run_trading_cycle(config: &Config, interval_str: &str) -> Result<()> {
    println!("\n============================================================");
    println!("执行时间: {}", chrono::Local::now().format("%Y-%m-%d %H:%M:%S"));
    println!("============================================================");

    // 1. 获取K线数据
    let klines = market::fetch_klines(&config.trade_symbol, interval_str, 20).await?;
    println!("✓ 获取到 {} 根K线", klines.len());

    // 2. 计算技术指标
    let indicators = market::calculate_indicators(&klines)?;
    println!(
        "✓ 技术指标: SMA(5)={:.2}, SMA(20)={:.2}, 价格变化1={:.2}%",
        indicators.sma_5, indicators.sma_20, indicators.price_change_1
    );

    // 3. 查询账户信息
    let account = executor::get_account_info(&config.binance_api_key, &config.binance_secret).await?;
    println!(
        "✓ 账户: 可用余额 {} USDT",
        account.availableBalance
    );

    // 4. 查询当前持仓
    let position = executor::get_position(
        &config.trade_symbol,
        &config.binance_api_key,
        &config.binance_secret,
    )
    .await?;

    match &position {
        None => println!("✓ 当前持仓: 空仓"),
        Some(pos) => println!(
            "✓ 当前持仓: {:?}仓 {:.4}, 盈亏: {:.2} USDT",
            pos.side, pos.amount, pos.unrealized_pnl
        ),
    }

    // 5. LLM决策
    let decision = llm::analyze(&klines, &indicators, &position, &account, &config.deepseek_api_key).await?;
    println!(
        "✓ 决策: {:?}, 信心: {:?}, 理由: {}",
        decision.signal, decision.confidence, decision.reason
    );
    state::log_decision(&config.trade_symbol, &decision, &position)?;

    // 6. 执行交易
    if decision.signal != types::Signal::Hold {
        let price = market::fetch_current_price(&config.trade_symbol).await?;
        match executor::execute_decision(
            &config.trade_symbol,
            &decision,
            &position,
            price,
            config.trade_amount,
            &config.binance_api_key,
            &config.binance_secret,
        )
        .await
        {
            Ok(result) => {
                println!(
                    "✓ 交易执行: {:?}, 价格: {:.2}, 数量: {:.4}",
                    result.action, result.price, result.amount
                );
                if let Some(details) = &result.order_details {
                    println!("  订单详情: {}", details);
                }
                if let Some(pnl) = result.pnl {
                    println!("  平仓盈亏: {:.2} USDT", pnl);
                }
                state::log_trade(&result)?;
            }
            Err(e) => {
                eprintln!("✗ 交易执行失败: {:#}", e);
                // 记录失败的交易尝试
                let failed_result = types::TradeResult {
                    symbol: config.trade_symbol.clone(),
                    action: types::TradeAction::Hold,
                    price,
                    amount: 0.0,
                    timestamp: chrono::Utc::now().timestamp(),
                    reason: format!("交易失败: {:#}", e),
                    pnl: None,
                    order_details: Some(format!("ERROR: {:#}", e)),
                };
                state::log_trade(&failed_result)?;
            }
        }
    } else {
        println!("✓ 保持观望");
    }

    Ok(())
}

// Task 7.2 & 7.3: 主函数
#[tokio::main]
async fn main() -> Result<()> {
    // 加载环境变量
    dotenv().ok();

    // 加载配置
    let config = Config::from_env().context("配置加载失败")?;

    // 启动日志
    println!("\n============================================================");
    println!("单智能体加密货币自动交易系统");
    println!("============================================================\n");
    println!("交易对: {}", config.trade_symbol);
    println!("交易数量: {}", config.trade_amount);
    println!("杠杆倍数: {}x", config.leverage);
    println!(
        "交易周期: {}秒 ({}分钟)",
        config.trade_interval_secs,
        config.trade_interval_secs / 60
    );
    println!("API密钥: {}***", &config.binance_api_key[..8]);
    println!("\n启动中...\n");

    // 环境检查 - 测试Binance连接
    match market::fetch_current_price(&config.trade_symbol).await {
        Ok(price) => println!("✓ Binance API 连接成功, {} 当前价格: ${:.2}", config.trade_symbol, price),
        Err(e) => {
            eprintln!("✗ Binance API 连接失败: {:#}", e);
            return Err(e);
        }
    }

    // 设置持仓模式为双向 (必须在交易前设置)
    match executor::set_dual_position_mode(&config.binance_api_key, &config.binance_secret).await {
        Ok(_) => println!("✓ 持仓模式设置成功: 双向持仓"),
        Err(e) => {
            eprintln!("✗ 持仓模式设置失败: {:#}", e);
            return Err(e);
        }
    }

    // 设置杠杆倍数
    match executor::set_leverage(
        &config.trade_symbol,
        config.leverage,
        &config.binance_api_key,
        &config.binance_secret,
    )
    .await
    {
        Ok(_) => println!("✓ 杠杆设置成功: {}x", config.leverage),
        Err(e) => {
            eprintln!("✗ 杠杆设置失败: {:#}", e);
            return Err(e);
        }
    }

    // 获取并显示账户信息
    println!("\n账户状态:");
    match executor::get_account_info(&config.binance_api_key, &config.binance_secret).await {
        Ok(account) => {
            println!("  总余额: {} USDT", account.totalWalletBalance);
            println!("  可用余额: {} USDT", account.availableBalance);
        }
        Err(e) => {
            eprintln!("  获取账户信息失败: {:#}", e);
        }
    }

    // 获取并显示当前持仓
    match executor::get_position(
        &config.trade_symbol,
        &config.binance_api_key,
        &config.binance_secret,
    )
    .await
    {
        Ok(Some(pos)) => {
            println!("  当前持仓: {:?}仓 {:.4}", pos.side, pos.amount);
            println!("  入场价格: ${:.2}", pos.entry_price);
            println!("  浮动盈亏: {:.2} USDT", pos.unrealized_pnl);
        }
        Ok(None) => {
            println!("  当前持仓: 空仓");
        }
        Err(e) => {
            eprintln!("  获取持仓信息失败: {:#}", e);
        }
    }

    // 确定interval字符串
    let interval_str = match config.trade_interval_secs {
        60 => "1m",
        900 => "15m",
        1800 => "30m",
        3600 => "1h",
        _ => "1m",
    };

    // 主循环
    let mut ticker = interval(Duration::from_secs(config.trade_interval_secs));

    loop {
        ticker.tick().await;

        if let Err(e) = run_trading_cycle(&config, interval_str).await {
            eprintln!("\n✗ 交易周期执行失败: {:#}\n", e);
            // 不中断，继续下一次
        }
    }
}
