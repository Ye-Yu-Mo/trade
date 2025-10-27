# 单智能体加密货币自动交易系统 - Task List

## Implementation Tasks

### Phase 1: 项目基础设置

- [x] 1. **配置项目依赖和环境**
    - [x] 1.1. 更新 Cargo.toml
        - *Goal*: 添加所有必需依赖
        - *Details*:
          - tokio (async runtime)
          - reqwest (HTTP client)
          - serde, serde_json (序列化)
          - anyhow (错误处理)
          - chrono (时间处理)
          - async-openai (DeepSeek API)
          - 移除现有的 binance 和 deepseek_rs 依赖（使用原生 HTTP 调用 Binance API）
        - *Requirements*: 非功能需求 - 可维护性

    - [x] 1.2. 创建 .env.example 文件
        - *Goal*: 提供环境变量模板
        - *Details*:
          ```
          BINANCE_API_KEY=your_testnet_api_key
          BINANCE_SECRET=your_testnet_secret
          BINANCE_TESTNET=true
          DEEPSEEK_API_KEY=your_deepseek_key
          TRADE_SYMBOL=BTCUSDT
          TRADE_AMOUNT=0.001
          TRADE_INTERVAL=15m
          ```
        - *Requirements*: 安全要求 - API Key 管理

    - [x] 1.3. 更新 .gitignore
        - *Goal*: 防止敏感信息泄露
        - *Details*: 添加 `.env`, `logs/`, `*.log`
        - *Requirements*: 安全要求

### Phase 2: 核心数据结构定义

- [x] 2. **定义共享数据类型 (src/types.rs)**
    - [x] 2.1. 定义 Kline 结构体
        - *Goal*: K线数据模型
        - *Details*: 包含 timestamp, open, high, low, close, volume 字段，实现 Serialize/Deserialize
        - *Requirements*: 数据模型定义

    - [x] 2.2. 定义 TechnicalIndicators 结构体
        - *Goal*: 技术指标数据模型
        - *Details*: sma_5, sma_20, price_change_1, price_change_3, volume_ratio
        - *Requirements*: 核心功能 1 - 市场数据采集

    - [x] 2.3. 定义交易决策相关类型
        - *Goal*: LLM 决策输出模型
        - *Details*: TradingDecision, Signal (Buy/Sell/Hold), Confidence (High/Medium/Low)
        - *Requirements*: 核心功能 2 - LLM决策引擎

    - [x] 2.4. 定义持仓和交易结果类型
        - *Goal*: 交易执行和状态管理模型
        - *Details*: Position, PositionSide, TradeResult, TradeAction
        - *Requirements*: 核心功能 3 - 合约模拟盘交易执行

### Phase 3: 市场数据模块实现

- [x] 3. **实现 market.rs 模块**
    - [x] 3.1. 实现 fetch_klines() 函数
        - *Goal*: 从 Binance 测试网获取 K线数据
        - *Details*:
          - 使用 reqwest 调用 Binance API: `/fapi/v1/klines`
          - 参数: symbol, interval, limit=20
          - 解析响应并转换为 Vec<Kline>
          - 网络错误重试3次，间隔2秒
        - *Requirements*: MVP - 成功连接 Binance 测试网并获取 K线数据

    - [x] 3.2. 实现 calculate_indicators() 函数
        - *Goal*: 计算技术指标
        - *Details*:
          - SMA(5): 最近5根K线收盘价平均
          - SMA(20): 最近20根K线收盘价平均
          - price_change_1: (close[last] - close[last-1]) / close[last-1] * 100
          - price_change_3: (close[last] - close[last-3]) / close[last-3] * 100
          - volume_ratio: volume[last] / avg(volume[last-20])
        - *Requirements*: MVP - 正确计算技术指标

    - [x] 3.3. 实现 fetch_current_price() 函数
        - *Goal*: 获取当前价格
        - *Details*: 调用 `/fapi/v1/ticker/price`
        - *Requirements*: 交易执行需要实时价格

### Phase 4: LLM 决策模块实现

- [ ] 4. **实现 llm.rs 模块**
    - [x] 4.1. 实现 build_prompt() 函数
        - *Goal*: 构造 DeepSeek 提示词
        - *Details*:
          - 包含最近5根K线的 OHLCV 数据（格式化为表格）
          - 包含技术指标
          - 包含当前持仓状态
          - 明确要求返回 JSON 格式：{signal, reason, confidence}
        - *Requirements*: 核心功能 2 - LLM决策引擎

    - [x] 4.2. 实现 analyze() 函数
        - *Goal*: 调用 DeepSeek API 并解析决策
        - *Details*:
          - 使用 async-openai 库
          - 配置 base_url 为 "https://api.deepseek.com"
          - 超时设置60秒
          - 从响应中提取 JSON（处理 markdown 代码块）
          - 解析为 TradingDecision 结构体
        - *Requirements*: MVP - 成功调用 DeepSeek API 获得决策

### Phase 5: 交易执行模块实现

- [ ] 5. **实现 executor.rs 模块**
    - [x] 5.1. 实现 get_position() 函数
        - *Goal*: 查询 Binance 测试网当前持仓
        - *Details*:
          - 调用 `/fapi/v2/positionRisk` (需要签名)
          - 过滤出指定 symbol 的持仓
          - 解析 positionAmt（正数=多仓，负数=空仓，0=空仓）
          - 返回 Option<Position>
        - *Requirements*: MVP - 正确查询测试网持仓状态和盈亏

    - [x] 5.2. 实现订单执行函数
        - *Goal*: 执行开多/平多/开空/平空操作
        - *Details*:
          - open_long(): POST `/fapi/v1/order` with side=BUY, positionSide=LONG
          - close_long(): POST `/fapi/v1/order` with side=SELL, positionSide=LONG
          - open_short(): POST `/fapi/v1/order` with side=SELL, positionSide=SHORT
          - close_short(): POST `/fapi/v1/order` with side=BUY, positionSide=SHORT
          - 使用市价单 (type=MARKET)
          - 所有请求需要 HMAC SHA256 签名
        - *Requirements*: MVP - 开多/平多/开空/平空逻辑正确

    - [x] 5.3. 实现 execute_decision() 函数
        - *Goal*: 根据 LLM 决策执行交易
        - *Details*:
          - HOLD 信号：返回 TradeAction::Hold
          - BUY 信号 + 空仓：开多
          - BUY 信号 + 空仓：平空 → 开多
          - SELL 信号 + 空仓：开空
          - SELL 信号 + 多仓：平多 → 开空
        - *Requirements*: MVP - 多空切换逻辑正确

### Phase 6: 状态管理模块实现

- [ ] 6. **实现 state.rs 模块**
    - [x] 6.1. 实现 log_trade() 函数
        - *Goal*: 记录交易到日志文件
        - *Details*:
          - 追加模式写入 `logs/trades.jsonl`
          - 每行一个 JSON 对象（TradeResult）
          - 包含时间戳、币种、动作、价格、数量、理由、盈亏
        - *Requirements*: MVP - 交易日志可读

    - [x] 6.2. 实现 log_decision() 函数
        - *Goal*: 记录所有决策（包括 HOLD）
        - *Details*:
          - 追加模式写入 `logs/decisions.jsonl`
          - 包含时间戳、决策、持仓状态
        - *Requirements*: User Story - 记录所有决策便于回溯

### Phase 7: 主程序集成

- [ ] 7. **实现 main.rs 主循环**
    - [x] 7.1. 实现 run_trading_cycle() 函数
        - *Goal*: 单次交易周期的完整流程
        - *Details*:
          ```rust
          async fn run_trading_cycle(symbol: &str) -> Result<()> {
              // 1. 获取K线数据
              let klines = market::fetch_klines(symbol, interval, 20).await?;
              // 2. 计算技术指标
              let indicators = market::calculate_indicators(&klines);
              // 3. 查询当前持仓
              let position = executor::get_position(symbol).await?;
              // 4. LLM决策
              let decision = llm::analyze(&klines, &indicators, &position).await?;
              state::log_decision(symbol, &decision, &position)?;
              // 5. 执行交易
              if decision.signal != Signal::Hold {
                  let price = market::fetch_current_price(symbol).await?;
                  let result = executor::execute_decision(symbol, &decision, &position, price).await?;
                  state::log_trade(&result)?;
              }
              Ok(())
          }
          ```
        - *Requirements*: 核心功能 4 - 定时循环调度

    - [x] 7.2. 实现主循环和错误处理
        - *Goal*: 定时执行交易周期，处理错误不中断
        - *Details*:
          - 从 .env 读取配置
          - 根据 TRADE_INTERVAL 设置 tokio::time::interval
          - loop 中捕获错误，打印日志后继续
        - *Requirements*: MVP - 连续3小时无故障运行

    - [x] 7.3. 添加启动日志和环境检查
        - *Goal*: 程序启动时验证配置和连接
        - *Details*:
          - 检查 .env 必需字段
          - 测试 Binance API 连接
          - 测试 DeepSeek API 连接
          - 打印配置信息（隐藏敏感字段）
        - *Requirements*: 可靠性要求

### Phase 8: 测试和优化

- [ ] 8. **验证和测试**
    - [x] 8.1. 手动测试完整流程
        - *Goal*: 验证 MVP 所有验收标准
        - *Details*:
          - 运行程序至少3小时
          - 观察日志输出
          - 检查 Binance 测试网持仓变化
          - 验证交易日志内容
        - *Requirements*: MVP 所有验收标准

    - [x] 8.2. 边界情况测试
        - *Goal*: 验证错误处理
        - *Details*:
          - 断网测试（拔网线或关闭WiFi）
          - DeepSeek API 返回非 JSON
          - Binance API 限流
        - *Requirements*: 可靠性要求

    - [x] 8.3. 代码审查和优化
        - *Goal*: 确保代码简洁性
        - *Details*:
          - 检查是否有超过3层缩进
          - 检查函数是否超过30行
          - 检查总代码量是否在500行内
          - 消除重复代码
        - *Requirements*: 可维护性要求

## Task Dependencies

**串行依赖：**
- Phase 1 (项目基础) 必须最先完成
- Phase 2 (数据结构) 依赖 Phase 1
- Phase 3-6 (各模块) 依赖 Phase 2
- Phase 7 (主程序) 依赖 Phase 3-6 全部完成
- Phase 8 (测试) 依赖 Phase 7

**并行执行：**
- Phase 3, 4, 5, 6 可以并行开发（数据结构定义后）

**关键路径：**
Phase 1 → Phase 2 → Phase 3 → Phase 7 → Phase 8

## Estimated Timeline

**Phase 1: 项目基础设置** - 1 小时
- 配置依赖、环境变量、gitignore

**Phase 2: 核心数据结构定义** - 1.5 小时
- 定义所有 struct 和 enum

**Phase 3: 市场数据模块** - 3 小时
- Binance API 集成 + 技术指标计算

**Phase 4: LLM 决策模块** - 2 小时
- DeepSeek API 集成 + JSON 解析

**Phase 5: 交易执行模块** - 4 小时
- Binance 签名认证 + 订单执行逻辑（最复杂）

**Phase 6: 状态管理模块** - 1 小时
- 日志文件写入

**Phase 7: 主程序集成** - 2 小时
- 主循环 + 配置加载 + 错误处理

**Phase 8: 测试和优化** - 3.5 小时
- 手动测试 + 边界测试 + 代码审查

**总计：18 小时**（约2-3个工作日）
