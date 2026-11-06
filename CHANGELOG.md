# Changelog

所有值得注意的变更都记录在这里。版本号遵循 [SemVer](https://semver.org/lang/zh-CN/)。

## [Unreleased] — v0.6.0

### Added

- **LLM 请求可靠性**
  - 默认开启 JSON 模式（`response_format=json_object`）
  - 单次请求超时（默认 60 秒）与重试（默认 1 次）
  - JSON 解析失败时自动作废缓存并重新请求一次
  - 模型、温度、超时、重试均可配置：`DEEPSEEK_MODEL`、`LLM_TEMPERATURE`、`LLM_TIMEOUT_SECS`、`LLM_MAX_RETRIES`、`LLM_JSON_MODE`
  - 缓存键包含模型、base URL、温度和 JSON 模式，换模型不会误用旧响应

### Changed

- **风险管理员改为确定性代码**（`risk::assess`）：按时机评分、ATR、趋势一致性、仓位上限和交易所约束计算 `RiskAssessment`，不再调用 LLM
  - 交易员 LLM 的结论由 `risk::enforce` 强制约束：`rejected` 直接 HOLD，`adjusted` 数量被截断
- 每个标的的模型调用从 3 次降到 2 次（策略 + 交易员），整轮从 `4N+1` 降到 `3N+1`
- 所有智能体改用 `chat_json`，统一 JSON 解析与失败重试

## [Unreleased] — v0.5.0

### Added

- **只读 Dashboard**（`src/dashboard.rs` + `src/dashboard.html`）
  - `trade dashboard --port 8080` 独立启动，或 `trade run --dashboard 8080` 随交易循环启动
  - 账户权益曲线、当前持仓、风控状态、最近决策 / 成交、回测结果浏览
  - 每周期写入 `logs/status.json`（原子替换）与 `logs/equity.jsonl`
  - 只读、默认绑定 `127.0.0.1`；回测文件查询做了目录穿越校验

## [Unreleased] — v0.4.0

### Added

- **交易所条件单**：开仓后在 Binance 挂 `STOP_MARKET` / `TAKE_PROFIT_MARKET`（`closePosition=true`），由交易所在盘中触发，不再只依赖每个周期轮询
  - 已有且触发价一致时复用，避免每轮撤单重挂
  - 平仓到空仓后自动清理残留条件单
  - 条件单失败时本地风控仍然兜底
  - `RISK_EXCHANGE_STOPS` 控制开关，`RISK_TRIGGER_TYPE` 选择 `MARK_PRICE` / `CONTRACT_PRICE`
- 纯函数 `protective_prices` 与撤单/挂单接口，含单元测试

### Fixed

- 移除持仓缓存：每轮都从交易所读取真实持仓。原缓存的过期判断实际总是命中，
  条件单在周期之间触发后，程序仍会拿旧仓位做决策。

## [0.3.0] — 未发布（本地提交）

### Added

- **显式平仓语义**：新增 `Signal::Close`，可以退出到空仓，而不必反手。
  - LLM 交易员与回测策略都支持 `CLOSE`
  - 熔断期间的反向信号会被降级为 `CLOSE`，而不是开新仓
- **确定性风控层**（`src/risk.rs`）
  - 止损 / 止盈：按开仓名义价值计算，触发时跳过 LLM 直接平仓
  - 最大回撤熔断（默认 20%）与日内亏损熔断（默认 5%）
  - 熔断状态持久化到 `logs/risk_state.json`；回撤熔断需人工删除状态文件后重启才解除
  - 熔断期间禁止开仓 / 加仓，只允许平仓
- 回测支持**盘中**止损止盈：用 K 线高低点判断触发，跳空按开盘价成交
- 回测新增 `--stop-loss-pct` / `--take-profit-pct`
- 下单使用 `newClientOrderId`：网络重试幂等，不会重复下单
- 解析订单 `avgPrice` / `executedQty`，盈亏按真实成交价计算；未返回时回查订单

### Changed

- `SmaCross` 基线在均线粘合时：有仓位则 `CLOSE` 观望，空仓则 `HOLD`
- 回测跳过低信心的**入场**信号，但平仓信号始终执行
- 实盘下单失败对 5xx / 网络错误重试 3 次，4xx 业务错误直接返回

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
