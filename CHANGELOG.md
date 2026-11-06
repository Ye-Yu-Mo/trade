# Changelog

所有值得注意的变更都记录在这里。版本号遵循 [SemVer](https://semver.org/lang/zh-CN/)。

## [0.3.0] - 2026-11-06

把项目从“能回测”推进到“能被验证、能被看见”。

### Added

- **显式平仓语义**：新增 `Signal::Close`，可以退出到空仓而不必反手；LLM 交易员、SMA 基线和回测都支持 `CLOSE`
- **确定性风控**（`src/risk.rs`）
  - 止损 / 止盈按开仓名义价值计算，触发时跳过 LLM 直接平仓
  - 最大回撤熔断（默认 20%）与日内亏损熔断（默认 5%），状态持久化到 `logs/risk_state.json`
  - 风险管理员改为代码：`risk::assess` + `risk::enforce`，交易员 LLM 不能覆盖风控结论
- **交易所条件单**：开仓后挂 `STOP_MARKET` / `TAKE_PROFIT_MARKET`（`closePosition=true`），盘中由交易所触发；已有且价格一致时复用
- **只读 Dashboard**（axum + 单页 HTML）：权益曲线、持仓、风控、决策 / 成交、回测浏览、Agent 决策流；默认只绑定 `127.0.0.1`
- **Walk-forward 样本外回测**：滚动训练 / 测试窗口，训练段搜索 fast/slow，测试段只评估未见过数据；多标的等权组合
- **Agent 决策链**：每个阶段记录模型、耗时、tokens、完整 JSON 输出；`trade analyze` 可 dry-run 跑完整链路
- **策略基准排行榜**：SMA walk-forward / 随机基线 / LLM / 买入持有，同窗口同成本，输出 Markdown
- **离线 Demo**：`trade demo`，无需 API key 或网络，跑内置样本并启动 Dashboard
- **LLM 可靠性**：JSON 模式、超时、重试、解析失败重试、模型感知缓存；模型 / 温度 / 超时均可配置
- **随机基线策略**：固定种子，可复现
- 回测盘中止损止盈、下单 `newClientOrderId` 幂等、真实成交价解析
- 69 个单元测试、CI（fmt / clippy / test / build）、MIT LICENSE、示例数据与 trace

### Changed

- 每个标的每轮模型调用从 3 次降到 2 次（策略 + 交易员），整轮从 `4N+1` 降到 `3N+1`
- `SmaCross` 支持任意 fast/slow；均线粘合时有仓位则平仓观望
- 回测引擎拆出 `run_segment`，可在任意片段复用
- `trade` 无子命令时仍保持原有实盘行为

### Fixed

- 移除总是命中的持仓缓存：每轮从交易所读取真实持仓，避免条件单触发后仍基于旧仓位决策
- 修复 K 线请求对 HTTP 5xx 不重试、API key 前缀日志 panic、绩效重启归零
- 训练窗口最后一笔成交不再越界到测试窗口

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
