// 多智能体加密货币自动交易系统

mod backtest;
mod cli;
mod dashboard;
mod data;
mod executor;
mod llm_client;
mod logging;
mod market;
mod multi_agent;
mod performance;
mod risk;
mod state;
mod strategy;
mod types;
mod walkforward;

use anyhow::{Context, Result};
use clap::Parser;
use dotenvy::dotenv;
use futures::future::try_join_all;
use log::{error, info, warn};
use performance::PerformanceTracker;
use std::env;
use tokio::time::{interval, Duration};

// 并行分析后的执行结果
struct SymbolCycleResult {
    traded: bool, // 是否执行了交易
    account_snapshot: Option<executor::AccountInfo>,
    trade_result: Option<types::TradeResult>,
}

// 并行分析产物
struct SymbolAnalysis {
    symbol: String,
    position: Option<types::Position>,
    market_report: types::MarketReport,
    last_price: f64,
    atr_percent: f64,
}

fn adjust_trade_quantity(
    desired: f64,
    allocated_max: f64,
    allocated_balance: f64,
    price: f64,
    constraints: &executor::SymbolConstraints,
) -> Option<f64> {
    if allocated_max <= 0.0 {
        return None;
    }

    let mut effective_max = allocated_max;
    if allocated_balance > 0.0 && price > 0.0 {
        let balance_limit = allocated_balance / price;
        if balance_limit > 0.0 {
            effective_max = effective_max.min(balance_limit);
        }
    }

    let mut qty = desired.max(0.0);
    if qty == 0.0 {
        qty = constraints.min_qty.max(constraints.step_size);
    }

    if qty > effective_max {
        qty = effective_max;
    }

    qty = executor::quantize_down(qty, constraints.step_size);

    if qty < constraints.min_qty {
        qty = executor::quantize_up(constraints.min_qty, constraints.step_size);
    }

    if let Some(max_qty) = constraints.max_qty {
        if qty > max_qty {
            qty = executor::quantize_down(max_qty, constraints.step_size);
        }
    }

    if constraints.min_notional > 0.0 && price > 0.0 {
        let required =
            executor::quantize_up(constraints.min_notional / price, constraints.step_size);
        if required > qty {
            qty = required;
        }
    }

    if qty > effective_max {
        qty = executor::quantize_down(effective_max, constraints.step_size);
    }

    if let Some(max_qty) = constraints.max_qty {
        if qty > max_qty {
            qty = executor::quantize_down(max_qty, constraints.step_size);
        }
    }

    if qty < constraints.min_qty {
        return None;
    }

    if constraints.min_notional > 0.0
        && price > 0.0
        && qty * price + f64::EPSILON < constraints.min_notional
    {
        return None;
    }

    if qty <= 0.0 {
        None
    } else {
        Some(qty)
    }
}

// 配置结构
struct Config {
    binance_api_key: String,
    binance_secret: String,
    deepseek_api_key: String,
    trade_symbols: Vec<String>, // 多标的交易
    trade_interval_secs: u64,
    leverage: u32,
    max_position: f64,      // 每个标的最大持仓量
    portfolio_mode: String, // balanced/aggressive/conservative
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
            portfolio_mode: env::var("PORTFOLIO_MODE").unwrap_or_else(|_| "balanced".to_string()),
        })
    }

    fn desired_portfolio_strategy(&self) -> types::PortfolioStrategy {
        match self.portfolio_mode.to_lowercase().as_str() {
            "aggressive" => types::PortfolioStrategy::Aggressive,
            "conservative" => types::PortfolioStrategy::Conservative,
            _ => types::PortfolioStrategy::Balanced,
        }
    }
}

// 并行阶段：每个标的的行情与持仓分析
async fn analyze_symbol(
    symbol: String,
    config: &Config,
    interval_str: &str,
) -> Result<SymbolAnalysis> {
    info!("--- 分析标的: {} ---", symbol);

    // 1. 获取K线数据
    const ANALYSIS_KLINE_LIMIT: u32 = 120;
    let klines = market::fetch_klines(&symbol, interval_str, ANALYSIS_KLINE_LIMIT).await?;
    info!("获取到 {} 根K线", klines.len());

    // 2. 计算技术指标
    let indicators = market::calculate_indicators(&klines)?;
    info!(
        "技术指标: SMA5={:.2}, SMA20={:.2}, SMA50={:.2}, SMA100={:.2}, Δ1={:.2}%, Δ3={:.2}%, Δ6={:.2}%, Δ12={:.2}%, ATR14={:.4} ({:.2}%), 量比={:.2}",
        indicators.sma_5,
        indicators.sma_20,
        indicators.sma_50,
        indicators.sma_100,
        indicators.price_change_1,
        indicators.price_change_3,
        indicators.price_change_6,
        indicators.price_change_12,
        indicators.atr_14,
        indicators.atr_percent,
        indicators.volume_ratio
    );

    // 3. 获取持仓。每轮都从交易所读取：条件单可能在周期之间改变持仓，
    //    缓存一次错误会让后续决策全部建立在假仓位之上。
    let position =
        executor::get_position(&symbol, &config.binance_api_key, &config.binance_secret).await?;
    match &position {
        None => info!("当前持仓: 空仓"),
        Some(pos) => info!(
            "当前持仓: {:?}仓 {:.4}, 盈亏: {:.2} USDT",
            pos.side, pos.amount, pos.unrealized_pnl
        ),
    }

    // 4. 行情分析
    info!("--- 行情分析员决策 ---");
    let market_report = multi_agent::market_analyst_analyze(
        &symbol,
        interval_str,
        &klines,
        &indicators,
        &config.deepseek_api_key,
    )
    .await?;
    info!(
        "行情分析员: {:?} ({:?}) | 阶段: {:?} | {}",
        market_report.trend,
        market_report.strength,
        market_report.market_phase,
        market_report.analysis
    );

    Ok(SymbolAnalysis {
        symbol,
        position,
        market_report,
        last_price: klines.last().map(|k| k.close).unwrap_or(0.0),
        atr_percent: indicators.atr_percent,
    })
}

// 决策与执行阶段：在所有分析完成后顺序执行
async fn execute_symbol_cycle(
    analysis: &SymbolAnalysis,
    allocated_max_amount: f64,
    allocated_balance: f64,
    constraints: &executor::SymbolConstraints,
    account: &executor::AccountInfo,
    config: &Config,
    risk_guard: &risk::RiskGuard,
) -> Result<SymbolCycleResult> {
    info!("--- 决策执行: {} ---", analysis.symbol);

    info!("账户: 可用余额 {} USDT", account.availableBalance);

    match &analysis.position {
        None => info!("当前持仓: 空仓"),
        Some(pos) => info!(
            "当前持仓: {:?}仓 {:.4}, 盈亏: {:.2} USDT",
            pos.side, pos.amount, pos.unrealized_pnl
        ),
    }

    // 确定性风控优先于 LLM：止损/止盈/熔断触发时直接平仓，不再调用模型
    let forced_exit = analysis.position.as_ref().and_then(|pos| {
        risk_guard
            .exit_reason(pos)
            .or_else(|| risk_guard.halt_exit_reason())
    });

    let decision = if let Some(reason) = forced_exit {
        warn!("确定性风控触发，跳过 LLM 决策: {}", reason);
        types::TradingDecision {
            signal: types::Signal::Close,
            reason,
            confidence: types::Confidence::High,
            amount: analysis.position.as_ref().map(|p| p.amount).unwrap_or(0.0),
        }
    } else {
        info!("--- 多智能体决策开始 ---");

        // 策略研究员
        let strategy = multi_agent::strategy_researcher_suggest(
            &analysis.symbol,
            &analysis.market_report,
            &analysis.position,
            &config.deepseek_api_key,
        )
        .await?;
        info!(
            "策略研究员: {:?} | 时机评分: {}/10 | {}",
            strategy.action, strategy.timing_score, strategy.reasoning
        );
        if let Some(pct) = strategy.target_position_pct {
            info!("目标仓位占比: {:.2}%", pct * 100.0);
        }
        if let Some(sl) = strategy.stop_loss_pct {
            info!("建议止损: {:.2}%", sl * 100.0);
        }
        if let Some(tp) = strategy.take_profit_pct {
            info!("建议止盈: {:.2}%", tp * 100.0);
        }

        // 风险管理员：确定性代码，不调用模型
        let risk = risk::assess(&risk::RiskRequest {
            market_report: &analysis.market_report,
            strategy: &strategy,
            position: &analysis.position,
            equity: account.totalWalletBalance.parse().unwrap_or(0.0),
            allocated_balance,
            allocated_max_amount,
            max_position: config.max_position,
            price: analysis.last_price,
            atr_percent: analysis.atr_percent,
            constraints,
        });
        info!(
            "风险管理员: {:?} | 审批: {:?} | 建议数量: {:.4} | {}",
            risk.risk_level, risk.approval, risk.suggested_amount, risk.reason
        );
        if !risk.warnings.is_empty() {
            warn!("风险警告: {}", risk.warnings.join(", "));
        }

        // 决策交易员
        let mut decision = multi_agent::trade_executor_decide(
            &analysis.symbol,
            &analysis.market_report,
            &strategy,
            &risk,
            &config.deepseek_api_key,
        )
        .await?;
        info!(
            "决策交易员: {:?}, 数量: {:.4}, 信心: {:?} | {}",
            decision.signal, decision.amount, decision.confidence, decision.reason
        );

        // LLM 不能覆盖风控结论
        risk::enforce(&mut decision, &risk);
        // 代码层的最后一道闸门：熔断期间不允许开启新敞口
        risk_guard.gate(&mut decision, &analysis.position);
        info!("--- 多智能体决策完成 ---");
        decision
    };

    info!(
        "最终决策: {:?}, 数量: {:.4}, 信心: {:?} | {}",
        decision.signal, decision.amount, decision.confidence, decision.reason
    );

    state::log_decision(&analysis.symbol, &decision, &analysis.position)?;

    let mut traded = false;
    let mut account_snapshot = Some(account.clone());
    let mut position_snapshot = analysis.position.clone();
    let mut position_verified = true;
    let mut latest_trade_result = None;

    if decision.signal != types::Signal::Hold {
        let raw_price = market::fetch_current_price(&analysis.symbol).await?;
        let quoted_price = executor::quantize_price(raw_price, constraints.tick_size);
        info!("价格对齐: 原始 {:.6} → {:.6}", raw_price, quoted_price);
        // 平仓使用交易所返回的实际持仓量，不受开仓约束影响
        let trade_amount = if decision.signal == types::Signal::Close {
            analysis.position.as_ref().map(|p| p.amount).unwrap_or(0.0)
        } else {
            match adjust_trade_quantity(
                decision.amount,
                allocated_max_amount,
                allocated_balance,
                quoted_price,
                constraints,
            ) {
                Some(qty) => qty,
                None => {
                    warn!(
                        "无法满足交易约束，保持观望: 建议 {:.6}, 分配上限 {:.6}, 分配资金 {:.2} USDT, 价格 {:.6}",
                        decision.amount, allocated_max_amount, allocated_balance, quoted_price
                    );
                    return Ok(SymbolCycleResult {
                        traded: false,
                        account_snapshot: Some(account.clone()),
                        trade_result: None,
                    });
                }
            }
        };

        if decision.signal != types::Signal::Close
            && (trade_amount - decision.amount).abs() > f64::EPSILON
        {
            warn!(
                "交易数量根据约束调整: 建议 {:.6} → {:.6}",
                decision.amount, trade_amount
            );
        }

        // 平仓前先撤条件单，避免本地平仓与交易所触发同时发生
        if decision.signal == types::Signal::Close {
            if let Err(err) = executor::cancel_protective_orders(
                &analysis.symbol,
                &config.binance_api_key,
                &config.binance_secret,
            )
            .await
            {
                warn!("平仓前撤销保护单失败: {:#}", err);
            }
        }

        match executor::execute_decision(
            &analysis.symbol,
            &decision,
            &analysis.position,
            quoted_price,
            trade_amount,
            config.max_position,
            &config.binance_api_key,
            &config.binance_secret,
        )
        .await
        {
            Ok(result) => {
                traded = !matches!(result.action, types::TradeAction::Hold);

                let trade_record = result.clone();
                info!(
                    "交易执行: {:?}, 价格: {:.2}, 数量: {:.4}",
                    result.action, result.price, result.amount
                );
                if let Some(details) = &result.order_details {
                    info!("订单详情: {}", details);
                }
                if let Some(pnl) = result.pnl {
                    info!("平仓盈亏: {:.2} USDT", pnl);
                }
                state::log_trade(&trade_record)?;
                latest_trade_result = Some(trade_record);

                if traded {
                    account_snapshot =
                        executor::get_account_info(&config.binance_api_key, &config.binance_secret)
                            .await
                            .ok();
                    match executor::get_position(
                        &analysis.symbol,
                        &config.binance_api_key,
                        &config.binance_secret,
                    )
                    .await
                    {
                        Ok(position) => {
                            position_snapshot = position;
                            position_verified = true;
                        }
                        Err(err) => {
                            // 查询失败不等于空仓：保留旧快照，避免误撤保护单
                            warn!("交易后刷新持仓失败，保留旧快照: {:#}", err);
                            position_verified = false;
                        }
                    }
                }
            }
            Err(e) => {
                error!("交易执行失败: {:#}", e);
                let failed_result = types::TradeResult {
                    symbol: analysis.symbol.clone(),
                    action: types::TradeAction::Hold,
                    price: quoted_price,
                    amount: 0.0,
                    timestamp: chrono::Utc::now().timestamp(),
                    reason: format!("交易失败: {:#}", e),
                    pnl: None,
                    order_details: Some(format!("ERROR: {:#}", e)),
                };
                state::log_trade(&failed_result)?;
                latest_trade_result = Some(failed_result);
            }
        }
    } else {
        info!("保持观望");
    }

    // 交易所条件单：有仓位就确保持仓有止损/止盈兜底；回到空仓则清理残留
    if risk_guard.limits.exchange_stops && position_verified {
        match &position_snapshot {
            Some(position) => {
                let protective = executor::ProtectiveConfig {
                    stop_loss_pct: risk_guard.limits.stop_loss_pct,
                    take_profit_pct: risk_guard.limits.take_profit_pct,
                    trigger_type: &risk_guard.limits.trigger_type,
                };
                match executor::ensure_protective_orders(
                    &analysis.symbol,
                    position,
                    &protective,
                    constraints,
                    &config.binance_api_key,
                    &config.binance_secret,
                )
                .await
                {
                    Ok(orders) if orders.reused => {
                        info!("保护单已存在: {}", analysis.symbol);
                    }
                    Ok(orders) => {
                        if let Some(stop_loss) = &orders.stop_loss {
                            info!("已挂止损: {}", stop_loss);
                        }
                        if let Some(take_profit) = &orders.take_profit {
                            info!("已挂止盈: {}", take_profit);
                        }
                    }
                    Err(err) => {
                        warn!("挂保护单失败，仍由本地风控兜底: {:#}", err);
                    }
                }
            }
            None => {
                if traded {
                    match executor::cancel_protective_orders(
                        &analysis.symbol,
                        &config.binance_api_key,
                        &config.binance_secret,
                    )
                    .await
                    {
                        Ok(count) if count > 0 => info!("已撤销 {} 个残留保护单", count),
                        Ok(_) => {}
                        Err(err) => warn!("撤销保护单失败: {:#}", err),
                    }
                }
            }
        }
    }

    Ok(SymbolCycleResult {
        traded,
        account_snapshot,
        trade_result: latest_trade_result,
    })
}

// 多标的投资组合交易周期
async fn run_portfolio_cycle(
    config: &Config,
    interval_str: &str,
    constraints_map: &std::collections::HashMap<String, executor::SymbolConstraints>,
    performance_tracker: &mut PerformanceTracker,
    risk_guard: &mut risk::RiskGuard,
) -> Result<bool> {
    info!("============================================================");
    info!(
        "执行时间: {}",
        chrono::Local::now().format("%Y-%m-%d %H:%M:%S")
    );
    info!("============================================================");

    // 1. 获取所有标的的行情分析
    info!("=== 第一阶段：行情分析 (并行) ===");
    let mut analysis_futures = Vec::new();
    for symbol in &config.trade_symbols {
        analysis_futures.push(analyze_symbol(symbol.clone(), config, interval_str));
    }

    let analyses: Vec<SymbolAnalysis> = try_join_all(analysis_futures).await?;

    // 2. 投资组合协调员分配资金
    info!("=== 第二阶段：投资组合资金分配 ===");
    let symbols_reports: Vec<(String, types::MarketReport)> = analyses
        .iter()
        .map(|a| (a.symbol.clone(), a.market_report.clone()))
        .collect();

    let mut current_account =
        executor::get_account_info(&config.binance_api_key, &config.binance_secret).await?;
    let total_balance: f64 = current_account.availableBalance.parse().unwrap_or(0.0);
    info!("总可用资金: {} USDT", total_balance);

    // 用钱包总余额衡量权益：可用余额会随保证金占用波动，不适合做回撤基准
    let wallet_equity: f64 = current_account
        .totalWalletBalance
        .parse()
        .unwrap_or(total_balance);
    if let Some(reason) = risk_guard.observe_equity(wallet_equity) {
        error!("风控熔断: {}", reason);
    }
    if risk_guard.halted() {
        warn!(
            "当前处于熔断状态，仅允许平仓/止损: {}",
            risk_guard.halt_reason().unwrap_or("trading halted")
        );
    }

    // 写入 Dashboard 读取的运行状态与权益曲线
    let positions_status: Vec<state::PositionStatus> = analyses
        .iter()
        .filter_map(|analysis| {
            analysis
                .position
                .as_ref()
                .map(|position| state::PositionStatus {
                    symbol: analysis.symbol.clone(),
                    side: format!("{:?}", position.side),
                    amount: position.amount,
                    entry_price: position.entry_price,
                    unrealized_pnl: position.unrealized_pnl,
                })
        })
        .collect();

    let risk_state = risk_guard.state();
    let now = chrono::Utc::now();
    let status = state::StatusSnapshot {
        updated_at: now.timestamp(),
        updated_at_text: chrono::Local::now().format("%Y-%m-%d %H:%M:%S").to_string(),
        account: state::AccountStatus {
            total_wallet_balance: wallet_equity,
            available_balance: total_balance,
        },
        risk: state::RiskStatus {
            halted: risk_guard.halted(),
            halt_reason: risk_guard.halt_reason().map(str::to_string),
            peak_equity: risk_state.peak_equity,
            last_equity: risk_state.last_equity,
            stop_loss_pct: risk_guard.limits.stop_loss_pct,
            take_profit_pct: risk_guard.limits.take_profit_pct,
        },
        positions: positions_status,
    };
    if let Err(err) = state::write_status(&status) {
        warn!("写入运行状态失败: {:#}", err);
    }
    if let Err(err) = state::append_equity(status.updated_at, wallet_equity) {
        warn!("写入权益曲线失败: {:#}", err);
    }

    let mut portfolio_allocation = multi_agent::portfolio_coordinator_allocate(
        &symbols_reports,
        total_balance,
        &config.portfolio_mode,
        &config.deepseek_api_key,
    )
    .await?;

    let desired_strategy = config.desired_portfolio_strategy();
    if portfolio_allocation.strategy != desired_strategy {
        warn!(
            "组合策略由配置 {} 指定为 {:?}，LLM 返回 {:?}，强制使用配置策略",
            config.portfolio_mode, desired_strategy, portfolio_allocation.strategy
        );
        portfolio_allocation.strategy = desired_strategy;
    }

    info!("组合策略: {:?}", portfolio_allocation.strategy);
    info!("分配理由: {}", portfolio_allocation.reasoning);
    info!("各标的分配:");
    for alloc in &portfolio_allocation.allocations {
        info!(
            "{} - 权重:{:.1}% | 优先级:{:?} | 分配:{:.2} USDT",
            alloc.symbol,
            alloc.weight * 100.0,
            alloc.priority,
            alloc.allocated_balance
        );
    }

    // 3. 对每个标的执行交易决策
    info!("=== 第三阶段：执行交易 ===");
    let mut analysis_map: std::collections::HashMap<String, SymbolAnalysis> = analyses
        .into_iter()
        .map(|analysis| (analysis.symbol.clone(), analysis))
        .collect();

    let mut any_traded = false;

    for alloc in &portfolio_allocation.allocations {
        let analysis = match analysis_map.remove(&alloc.symbol) {
            Some(a) => a,
            None => {
                error!("未找到标的 {} 的分析结果，跳过执行", alloc.symbol);
                continue;
            }
        };

        if alloc.priority == types::AllocationPriority::Skip {
            info!("跳过标的: {} (优先级: Skip)", alloc.symbol);
            continue;
        }

        let allocated_balance = alloc.allocated_balance;
        let symbol = alloc.symbol.clone();
        let constraint = match constraints_map.get(&symbol) {
            Some(c) => *c,
            None => {
                error!("缺少交易约束，跳过标的 {}", symbol);
                continue;
            }
        };

        if allocated_balance <= 0.0 {
            warn!("分配资金为零，跳过标的 {}", symbol);
            continue;
        }

        let mut max_amount = alloc.max_amount_override.unwrap_or(config.max_position);
        if let Some(max_qty) = constraint.max_qty {
            max_amount = max_amount.min(max_qty);
        }
        if max_amount <= 0.0 {
            warn!("最大持仓上限为零，跳过标的 {}", symbol);
            continue;
        }

        if constraint.min_qty > max_amount {
            warn!(
                "最小下单量 {:.6} 超过最大允许 {:.6}，跳过标的 {}",
                constraint.min_qty, max_amount, symbol
            );
            continue;
        }

        match execute_symbol_cycle(
            &analysis,
            max_amount,
            allocated_balance,
            &constraint,
            &current_account,
            config,
            risk_guard,
        )
        .await
        {
            Ok(result) => {
                let SymbolCycleResult {
                    traded: cycle_traded,
                    account_snapshot,
                    trade_result,
                } = result;

                if cycle_traded {
                    any_traded = true;
                }

                if let Some(snapshot) = account_snapshot {
                    current_account = snapshot;
                }

                if let Some(trade) = trade_result.as_ref() {
                    if performance_tracker.update(trade) {
                        performance_tracker.persist()?;
                    }
                }
            }
            Err(e) => {
                error!("标的 {} 交易周期失败: {:#}", symbol, e);
            }
        }
    }

    if let Err(err) = risk_guard.persist() {
        warn!("保存风控状态失败: {:#}", err);
    }

    Ok(any_traded)
}

// Task 7.2 & 7.3: 主函数
#[tokio::main]
async fn main() -> Result<()> {
    dotenv().ok();

    logging::init_logging().context("初始化日志系统失败")?;

    let cli = cli::Cli::parse();

    match cli
        .command
        .unwrap_or(cli::Command::Run(cli::RunArgs::default()))
    {
        cli::Command::Run(args) => run_live(args).await,
        cli::Command::Backtest(args) => cli::run_backtest(*args).await,
        cli::Command::Dashboard(args) => cli::run_dashboard(args).await,
        cli::Command::Walkforward(args) => cli::run_walkforward(args).await,
    }
}

async fn run_live(args: cli::RunArgs) -> Result<()> {
    if let Some(port) = args.dashboard {
        let dashboard_config = dashboard::DashboardConfig {
            logs_dir: std::path::PathBuf::from(logging::logs_directory()),
            backtest_dir: std::path::PathBuf::from("backtest"),
        };
        let host = args.dashboard_host;
        tokio::spawn(async move {
            if let Err(err) = dashboard::serve(&host, port, dashboard_config).await {
                error!("Dashboard 退出: {:#}", err);
            }
        });
    }

    // 加载配置
    let config = Config::from_env().context("配置加载失败")?;

    let symbol_constraints = executor::fetch_symbol_constraints(&config.trade_symbols)
        .await
        .context("拉取交易规则失败")?;

    // 启动日志
    info!("============================================================");
    info!("多智能体加密货币自动交易系统 - 投资组合版");
    info!("============================================================");
    info!("交易标的: {:?}", config.trade_symbols);
    info!("组合策略: {}", config.portfolio_mode);
    info!("杠杆倍数: {}x", config.leverage);
    for symbol in &config.trade_symbols {
        if let Some(cons) = symbol_constraints.get(symbol) {
            info!(
                "约束 {}: step={}, minQty={}, minNotional={}, maxQty={:?}",
                symbol, cons.step_size, cons.min_qty, cons.min_notional, cons.max_qty
            );
        }
    }
    info!(
        "交易周期: {}秒 ({}分钟)",
        config.trade_interval_secs,
        config.trade_interval_secs / 60
    );
    let key_prefix: String = config.binance_api_key.chars().take(8).collect();
    info!("API密钥前缀: {}***", key_prefix);
    info!("启动中...");

    // 环境检查 - 测试Binance连接（使用第一个标的）
    if let Some(first_symbol) = config.trade_symbols.first() {
        match market::fetch_current_price(first_symbol).await {
            Ok(price) => info!(
                "Binance API 连接成功, {} 当前价格: ${:.2}",
                first_symbol, price
            ),
            Err(e) => {
                error!("Binance API 连接失败: {:#}", e);
                return Err(e);
            }
        }
    }

    // 设置持仓模式为双向 (必须在交易前设置)
    match executor::set_dual_position_mode(&config.binance_api_key, &config.binance_secret).await {
        Ok(_) => info!("持仓模式设置成功: 双向持仓"),
        Err(e) => {
            error!("持仓模式设置失败: {:#}", e);
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
            Ok(_) => info!("{} 杠杆设置成功: {}x", symbol, config.leverage),
            Err(e) => {
                error!("{} 杠杆设置失败: {:#}", symbol, e);
                // 继续处理其他标的，不中断
            }
        }
    }

    // 获取并显示账户信息
    info!("账户状态:");
    match executor::get_account_info(&config.binance_api_key, &config.binance_secret).await {
        Ok(account) => {
            info!("总余额: {} USDT", account.totalWalletBalance);
            info!("可用余额: {} USDT", account.availableBalance);
        }
        Err(e) => {
            error!("获取账户信息失败: {:#}", e);
        }
    }

    // 获取并显示所有标的的持仓
    info!("各标的持仓:");
    for symbol in &config.trade_symbols {
        match executor::get_position(symbol, &config.binance_api_key, &config.binance_secret).await
        {
            Ok(Some(pos)) => {
                info!(
                    "{} - {:?}仓 {:.4}, 入场 ${:.2}, 盈亏 {:.2} USDT",
                    symbol, pos.side, pos.amount, pos.entry_price, pos.unrealized_pnl
                );
            }
            Ok(None) => {
                info!("{} - 空仓", symbol);
            }
            Err(e) => {
                error!("{} - 获取失败: {:#}", symbol, e);
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

    let mut performance_tracker = PerformanceTracker::load();
    let mut risk_guard = risk::RiskGuard::load(risk::RiskLimits::from_env());
    let limits = &risk_guard.limits;
    info!(
        "风控: enabled={} | 止损 {:.2}% | 止盈 {:.2}% | 最大回撤 {:.2}% | 日内亏损 {:.2}% | 熔断平仓={} | 交易所条件单={} ({})",
        limits.enabled,
        limits.stop_loss_pct * 100.0,
        limits.take_profit_pct * 100.0,
        limits.max_drawdown_pct * 100.0,
        limits.daily_loss_pct * 100.0,
        limits.close_on_halt,
        limits.exchange_stops,
        limits.trigger_type
    );
    if risk_guard.halted() {
        warn!(
            "历史风控状态为熔断: {}。请确认后删除 logs/risk_state.json 再重启以恢复交易。",
            risk_guard.halt_reason().unwrap_or("trading halted")
        );
    }

    let snapshot = performance_tracker.snapshot();
    if snapshot.total_trades > 0 {
        info!(
            "历史绩效: {} 笔交易, 已实现盈亏 {:.2} USDT, 最大回撤 {:.2}",
            snapshot.total_trades, snapshot.total_realized_pnl, snapshot.max_drawdown
        );
    }

    // 主循环
    let mut ticker = interval(Duration::from_secs(config.trade_interval_secs));

    loop {
        ticker.tick().await;

        match run_portfolio_cycle(
            &config,
            interval_str,
            &symbol_constraints,
            &mut performance_tracker,
            &mut risk_guard,
        )
        .await
        {
            Ok(_any_traded) => {
                // run_portfolio_cycle内部已处理缓存更新
            }
            Err(e) => {
                error!("投资组合交易周期失败: {:#}", e);
            }
        }
    }
}
