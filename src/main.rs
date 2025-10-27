// 多智能体加密货币自动交易系统

mod executor;
mod llm;
mod market;
mod multi_agent;
mod state;
mod types;

use anyhow::{Context, Result};
use dotenvy::dotenv;
use std::env;
use tokio::time::{interval, Duration};

// 交易周期结果
struct CycleResult {
    traded: bool,  // 是否执行了交易
}

// 配置结构
struct Config {
    binance_api_key: String,
    binance_secret: String,
    deepseek_api_key: String,
    trade_symbols: Vec<String>,  // 多标的交易
    min_trade_amount: f64,  // 最小交易数量
    max_trade_amount: f64,  // 最大交易数量
    trade_interval_secs: u64,
    leverage: u32,
    max_position: f64,  // 每个标的最大持仓量
    portfolio_mode: String,  // balanced/aggressive/conservative
}

impl Config {
    fn from_env() -> Result<Self> {
        let symbols_str = env::var("TRADE_SYMBOLS").unwrap_or_else(|_| "BTCUSDT".to_string());
        let trade_symbols: Vec<String> = symbols_str
            .split(',')
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty())
            .collect();

        Ok(Config {
            binance_api_key: env::var("BINANCE_API_KEY").context("缺少 BINANCE_API_KEY")?,
            binance_secret: env::var("BINANCE_SECRET").context("缺少 BINANCE_SECRET")?,
            deepseek_api_key: env::var("DEEPSEEK_API_KEY").context("缺少 DEEPSEEK_API_KEY")?,
            trade_symbols,
            min_trade_amount: env::var("MIN_TRADE_AMOUNT")
                .unwrap_or_else(|_| "0.001".to_string())
                .parse()
                .context("MIN_TRADE_AMOUNT 格式错误")?,
            max_trade_amount: env::var("MAX_TRADE_AMOUNT")
                .unwrap_or_else(|_| "0.003".to_string())
                .parse()
                .context("MAX_TRADE_AMOUNT 格式错误")?,
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
            max_position: env::var("MAX_POSITION")
                .unwrap_or_else(|_| "0.005".to_string())
                .parse()
                .context("MAX_POSITION 格式错误")?,
            portfolio_mode: env::var("PORTFOLIO_MODE")
                .unwrap_or_else(|_| "balanced".to_string()),
        })
    }
}

// Task 7.1: 单个标的交易周期
async fn run_single_symbol_cycle(
    symbol: &str,
    config: &Config,
    interval_str: &str,
    allocated_max_amount: f64,  // 投资组合分配的最大数量
    use_cache: bool,
    cached_account: &Option<executor::AccountInfo>,
    cached_position: &Option<types::Position>,
) -> Result<CycleResult> {
    println!("\n--- 处理标的: {} ---", symbol);

    // 1. 获取K线数据
    let klines = market::fetch_klines(symbol, interval_str, 20).await?;
    println!("✓ 获取到 {} 根K线", klines.len());

    // 2. 计算技术指标
    let indicators = market::calculate_indicators(&klines)?;
    println!(
        "✓ 技术指标: SMA(5)={:.2}, SMA(20)={:.2}, 价格变化1={:.2}%",
        indicators.sma_5, indicators.sma_20, indicators.price_change_1
    );

    // 3. 使用缓存的账户信息或重新查询
    let account = if use_cache && cached_account.is_some() {
        println!("✓ 账户: 可用余额 {} USDT (缓存)", cached_account.as_ref().unwrap().availableBalance);
        cached_account.as_ref().unwrap().clone()
    } else {
        let acc = executor::get_account_info(&config.binance_api_key, &config.binance_secret).await?;
        println!("✓ 账户: 可用余额 {} USDT", acc.availableBalance);
        acc
    };

    // 4. 使用缓存的持仓信息或重新查询
    let position = if use_cache {
        match cached_position {
            None => {
                println!("✓ 当前持仓: 空仓 (缓存)");
                None
            }
            Some(pos) => {
                println!(
                    "✓ 当前持仓: {:?}仓 {:.4}, 盈亏: {:.2} USDT (缓存)",
                    pos.side, pos.amount, pos.unrealized_pnl
                );
                Some(pos.clone())
            }
        }
    } else {
        let pos = executor::get_position(
            symbol,
            &config.binance_api_key,
            &config.binance_secret,
        )
        .await?;
        match &pos {
            None => println!("✓ 当前持仓: 空仓"),
            Some(p) => println!(
                "✓ 当前持仓: {:?}仓 {:.4}, 盈亏: {:.2} USDT",
                p.side, p.amount, p.unrealized_pnl
            ),
        }
        pos
    };

    // 5. 多智能体决策流程
    println!("\n--- 多智能体决策开始 ---");

    // 5.1 行情分析员
    let market_report = multi_agent::market_analyst_analyze(
        &klines,
        &indicators,
        &config.deepseek_api_key,
    )
    .await?;
    println!(
        "✓ 行情分析员: {:?} ({:?}) | 阶段: {:?} | {}",
        market_report.trend, market_report.strength, market_report.market_phase, market_report.analysis
    );

    // 5.2 策略研究员
    let strategy = multi_agent::strategy_researcher_suggest(
        &market_report,
        &position,
        &config.deepseek_api_key,
    )
    .await?;
    println!(
        "✓ 策略研究员: {:?} | 时机评分: {}/10 | {}",
        strategy.action, strategy.timing_score, strategy.reasoning
    );

    // 5.3 风险管理员
    let risk = multi_agent::risk_manager_assess(
        &market_report,
        &strategy,
        &account,
        &position,
        config.min_trade_amount,
        allocated_max_amount,  // 使用投资组合分配的最大数量
        config.max_position,
        &config.deepseek_api_key,
    )
    .await?;
    println!(
        "✓ 风险管理员: {:?} | 审批: {:?} | 建议数量: {:.4} | {}",
        risk.risk_level, risk.approval, risk.suggested_amount, risk.reason
    );
    if !risk.warnings.is_empty() {
        println!("  ⚠️  风险警告: {}", risk.warnings.join(", "));
    }

    // 5.4 决策交易员
    let decision = multi_agent::trade_executor_decide(
        &market_report,
        &strategy,
        &risk,
        &config.deepseek_api_key,
    )
    .await?;
    println!(
        "✓ 决策交易员: {:?}, 数量: {:.4}, 信心: {:?} | {}",
        decision.signal, decision.amount, decision.confidence, decision.reason
    );
    println!("--- 多智能体决策完成 ---\n");

    state::log_decision(symbol, &decision, &position)?;

    // 6. 执行交易
    let mut traded = false;
    if decision.signal != types::Signal::Hold {
        let price = market::fetch_current_price(symbol).await?;
        // 限制AI建议的数量在配置范围内
        let trade_amount = decision.amount.clamp(config.min_trade_amount, allocated_max_amount);
        if trade_amount != decision.amount {
            println!("  ⚠️  AI建议数量 {:.4} 超出范围，调整为 {:.4}", decision.amount, trade_amount);
        }

        match executor::execute_decision(
            symbol,
            &decision,
            &position,
            price,
            trade_amount,
            config.max_position,
            &config.binance_api_key,
            &config.binance_secret,
        )
        .await
        {
            Ok(result) => {
                // 判断是否真正执行了交易（不是Hold）
                traded = !matches!(result.action, types::TradeAction::Hold);

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
                    symbol: symbol.to_string(),
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

    Ok(CycleResult { traded })
}

// 多标的投资组合交易周期
async fn run_portfolio_cycle(
    config: &Config,
    interval_str: &str,
    symbols_cache: &mut std::collections::HashMap<String, (Option<executor::AccountInfo>, Option<types::Position>, bool)>,
) -> Result<bool> {
    println!("\n============================================================");
    println!("执行时间: {}", chrono::Local::now().format("%Y-%m-%d %H:%M:%S"));
    println!("============================================================");

    // 1. 获取所有标的的行情分析
    println!("\n=== 第一阶段：行情分析 ===");
    let mut symbols_reports: Vec<(String, types::MarketReport)> = Vec::new();

    for symbol in &config.trade_symbols {
        println!("\n分析标的: {}", symbol);
        let klines = market::fetch_klines(symbol, interval_str, 20).await?;
        let indicators = market::calculate_indicators(&klines)?;

        let market_report = multi_agent::market_analyst_analyze(
            &klines,
            &indicators,
            &config.deepseek_api_key,
        )
        .await?;

        println!(
            "  趋势: {:?} ({:?}) | 阶段: {:?}",
            market_report.trend, market_report.strength, market_report.market_phase
        );

        symbols_reports.push((symbol.clone(), market_report));
    }

    // 2. 投资组合协调员分配资金
    println!("\n=== 第二阶段：投资组合资金分配 ===");

    // 获取总可用余额（使用第一个标的的账户信息，因为是共享账户）
    let account = executor::get_account_info(&config.binance_api_key, &config.binance_secret).await?;
    let total_balance: f64 = account.availableBalance.parse().unwrap_or(0.0);

    println!("总可用资金: {} USDT", total_balance);

    let portfolio_allocation = multi_agent::portfolio_coordinator_allocate(
        &symbols_reports,
        total_balance,
        &config.portfolio_mode,
        &config.deepseek_api_key,
    )
    .await?;

    println!("组合策略: {:?}", portfolio_allocation.strategy);
    println!("分配理由: {}", portfolio_allocation.reasoning);
    println!("\n各标的分配:");
    for alloc in &portfolio_allocation.allocations {
        println!(
            "  {} - 权重:{:.1}% | 优先级:{:?} | 分配:{:.2} USDT",
            alloc.symbol,
            alloc.weight * 100.0,
            alloc.priority,
            alloc.allocated_balance
        );
    }

    // 3. 对每个标的执行交易决策
    println!("\n=== 第三阶段：执行交易 ===");

    let mut any_traded = false;

    for alloc in &portfolio_allocation.allocations {
        // 跳过优先级为Skip的标的
        if alloc.priority == types::AllocationPriority::Skip {
            println!("\n跳过标的: {} (优先级: Skip)", alloc.symbol);
            continue;
        }

        // 确定该标的的最大交易量
        let max_amount = alloc.max_amount_override.unwrap_or(config.max_trade_amount);

        // 从缓存获取状态
        let (cached_account, cached_position, use_cache) = symbols_cache
            .get(&alloc.symbol)
            .cloned()
            .unwrap_or((None, None, false));

        // 执行单个标的的交易周期
        match run_single_symbol_cycle(
            &alloc.symbol,
            config,
            interval_str,
            max_amount,
            use_cache,
            &cached_account,
            &cached_position,
        )
        .await
        {
            Ok(result) => {
                if result.traded {
                    any_traded = true;

                    // 更新该标的的缓存
                    let new_account = executor::get_account_info(&config.binance_api_key, &config.binance_secret).await.ok();
                    let new_position = executor::get_position(&alloc.symbol, &config.binance_api_key, &config.binance_secret).await.ok().flatten();

                    symbols_cache.insert(alloc.symbol.clone(), (new_account, new_position, true));
                } else {
                    // 未交易，继续使用缓存
                }
            }
            Err(e) => {
                eprintln!("\n✗ 标的 {} 交易周期失败: {:#}\n", alloc.symbol, e);
                // 失败时清除该标的的缓存
                symbols_cache.insert(alloc.symbol.clone(), (None, None, false));
            }
        }
    }

    Ok(any_traded)
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
    println!("多智能体加密货币自动交易系统 - 投资组合版");
    println!("============================================================\n");
    println!("交易标的: {:?}", config.trade_symbols);
    println!("组合策略: {}", config.portfolio_mode);
    println!("交易数量: {:.4} - {:.4} (AI决策)", config.min_trade_amount, config.max_trade_amount);
    println!("杠杆倍数: {}x", config.leverage);
    println!(
        "交易周期: {}秒 ({}分钟)",
        config.trade_interval_secs,
        config.trade_interval_secs / 60
    );
    println!("API密钥: {}***", &config.binance_api_key[..8]);
    println!("\n启动中...\n");

    // 环境检查 - 测试Binance连接（使用第一个标的）
    if let Some(first_symbol) = config.trade_symbols.first() {
        match market::fetch_current_price(first_symbol).await {
            Ok(price) => println!("✓ Binance API 连接成功, {} 当前价格: ${:.2}", first_symbol, price),
            Err(e) => {
                eprintln!("✗ Binance API 连接失败: {:#}", e);
                return Err(e);
            }
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

    // 为所有标的设置杠杆倍数
    for symbol in &config.trade_symbols {
        match executor::set_leverage(
            symbol,
            config.leverage,
            &config.binance_api_key,
            &config.binance_secret,
        )
        .await
        {
            Ok(_) => println!("✓ {} 杠杆设置成功: {}x", symbol, config.leverage),
            Err(e) => {
                eprintln!("✗ {} 杠杆设置失败: {:#}", symbol, e);
                // 继续处理其他标的，不中断
            }
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

    // 获取并显示所有标的的持仓
    println!("\n各标的持仓:");
    for symbol in &config.trade_symbols {
        match executor::get_position(
            symbol,
            &config.binance_api_key,
            &config.binance_secret,
        )
        .await
        {
            Ok(Some(pos)) => {
                println!("  {} - {:?}仓 {:.4}, 入场 ${:.2}, 盈亏 {:.2} USDT",
                    symbol, pos.side, pos.amount, pos.entry_price, pos.unrealized_pnl);
            }
            Ok(None) => {
                println!("  {} - 空仓", symbol);
            }
            Err(e) => {
                eprintln!("  {} - 获取失败: {:#}", symbol, e);
            }
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

    // 初始化多标的缓存状态
    // HashMap<Symbol, (AccountInfo, Position, use_cache)>
    let mut symbols_cache = std::collections::HashMap::new();

    // 主循环
    let mut ticker = interval(Duration::from_secs(config.trade_interval_secs));

    loop {
        ticker.tick().await;

        match run_portfolio_cycle(
            &config,
            interval_str,
            &mut symbols_cache,
        )
        .await
        {
            Ok(_any_traded) => {
                // run_portfolio_cycle内部已处理缓存更新
            }
            Err(e) => {
                eprintln!("\n✗ 投资组合交易周期失败: {:#}\n", e);
                // 错误时清空所有缓存
                symbols_cache.clear();
            }
        }
    }
}
