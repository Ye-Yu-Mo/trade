# Changelog

所有值得注意的变更都记录在这里。版本号遵循 [SemVer](https://semver.org/lang/zh-CN/)。

## [Unreleased]

## [0.2.0] - 2026-11-06

第一个可公开复现的版本：把只能跑实盘的原型，变成一个可以回测、可以被 CI 验证的框架。

### Added

- **历史回测引擎**（`src/backtest.rs`）
  - 信号在K线收盘产生，成交在下一根K线开盘执行，避免未来函数
  - 内置手续费与滑点模型
  - 输出收益率、最大回撤、Sharpe、胜率、盈亏比、暴露时间，并与买入持有基准对比
  - 结果写入 JSON、权益曲线 CSV、成交明细 CSV
- **策略抽象**（`src/strategy.rs`）
  - `sma_cross`：确定性基线，不需要任何 API key
  - `llm`：DeepSeek 单智能体基线，支持响应缓存
- **命令行接口**（`src/cli.rs`，基于 clap）
  - `trade run`：原有实盘/测试网交易循环（默认行为）
  - `trade backtest ...`：历史回测
- **历史数据缓存**（`src/data.rs`）
  - 按标 / 周期 / 时间区间缓存到 `data/klines/`
  - 自动分页抓取，支持 1500 根/页
- **统一 LLM 客户端**（`src/llm_client.rs`）
  - 集中管理模型名、base URL、JSON 解析
  - 磁盘响应缓存（`LLM_CACHE_DIR`，默认 `.cache/llm`），可用 `LLM_CACHE=off` 关闭
- 21 个单元测试
- GitHub Actions CI：fmt + clippy（`-D warnings`）+ test + release build
- `LICENSE`（MIT）、`CHANGELOG.md`、`rust-toolchain.toml`

### Changed

- `market::fetch_klines` 的重试逻辑：现在对 HTTP 4xx/5xx 同样重试，之前只对传输层错误重试
- `main.rs` 拆分为 `run_live()` 与回测入口，默认无参数行为保持不变
- `PerformanceTracker` 启动时从 `logs/performance.json` 恢复历史绩效，不再每次重启归零
- 风险管理员与执行层的过长参数列表暂时标注 `allow(clippy::too_many_arguments)`，待 v0.4 重构

### Fixed

- 修复 API key 前缀日志在密钥短于 8 字节时 panic 的问题
- 修复平仓交易无法构造 `TradeAction::CloseLong` / `CloseShort` 的警告（回测引擎已使用）

### Removed

- 删除未被引用的单智能体模块 `src/llm.rs`，逻辑由 `src/strategy.rs` 取代

### Known Issues

- **实盘执行层无法平仓到空仓**：`TradingDecision` 只有 BUY/SELL/HOLD，持多仓时 SELL 会直接反手开空。回测引擎按同样的语义建模，计划在 v0.4 引入显式的 `Close` 意图。
- 策略给出的 `stop_loss_pct` / `take_profit_pct` 目前只写日志，没有真正下单执行。
- 回测未模拟资金费率与保证金强平。
