# 单智能体加密货币自动交易系统

基于 DeepSeek 的单一决策流合约交易机器人，使用 Rust 实现，在 Binance 测试网上对 BTCUSDT 和 ETHUSDT 进行自动交易。

## 核心特性

- **单体架构**：数据获取 → 技术指标计算 → LLM 决策 → 交易执行 → 日志记录
- **专业提示词**：基于"交易分析师"角色，理性、数据驱动、风险优先
- **合约交易**：支持做多/做空，自动处理多空切换
- **技术指标**：SMA(5/20)、价格变化率、成交量比
- **风险控制**：持仓监控、盈亏跟踪、错误重试

## 快速开始

### 1. 配置环境

复制环境变量模板：
```bash
cp .env.example .env
```

编辑 `.env` 文件，填入以下信息：

```bash
# Binance 测试网 API (从 https://testnet.binancefuture.com 获取)
BINANCE_API_KEY=your_testnet_api_key
BINANCE_SECRET=your_testnet_secret
BINANCE_TESTNET=true

# DeepSeek API (从 https://platform.deepseek.com 获取)
DEEPSEEK_API_KEY=your_deepseek_key

# 交易配置
TRADE_SYMBOL=BTCUSDT
TRADE_AMOUNT=0.001
TRADE_INTERVAL=15m  # 可选: 15m, 30m, 1h
```

### 2. 编译运行

```bash
cargo build --release
cargo run --release
```

### 3. 查看日志

程序会自动创建 `logs/` 目录：

- `logs/trades.jsonl` - 交易记录（开仓/平仓/价格/盈亏）
- `logs/decisions.jsonl` - 决策记录（包含 HOLD 信号）

每行一个 JSON 对象，可用 `jq` 查看：

```bash
# 查看最近10条交易
tail -10 logs/trades.jsonl | jq

# 查看所有决策
cat logs/decisions.jsonl | jq -c '{signal: .decision.signal, reason: .decision.reason}'
```

## 系统架构

```
┌─────────────────────────────────────────────┐
│              main.rs (166行)                 │
│  - 配置加载 (.env)                            │
│  - 定时循环 (tokio)                           │
│  - 错误处理 (不中断)                          │
└──────────────┬──────────────────────────────┘
               │
    ┌──────────┼──────────┬──────────┬─────────┐
    │          │          │          │         │
    ▼          ▼          ▼          ▼         ▼
┌────────┐ ┌─────┐ ┌──────────┐ ┌──────┐ ┌────────┐
│ market │ │ llm │ │ executor │ │state │ │ types  │
│ (118行)│ │(179)│ │  (267行) │ │(55行)│ │ (81行) │
└────────┘ └─────┘ └──────────┘ └──────┘ └────────┘
```

### 数据流

1. **market::fetch_klines()** → 获取20根K线数据
2. **market::calculate_indicators()** → 计算 SMA、价格变化率、成交量比
3. **executor::get_position()** → 查询当前持仓（多/空/空仓）
4. **llm::analyze()** → 调用 DeepSeek API，返回 `{signal, reason, confidence}`
5. **executor::execute_decision()** → 执行交易决策
   - BUY: 开多仓（或平空后开多）
   - SELL: 开空仓（或平多后开空）
   - HOLD: 观望
6. **state::log_trade()** / **state::log_decision()** → 记录到日志

## LLM 决策逻辑

### System Prompt（交易分析师角色）

```
核心哲学:
1. 市场从不撒谎 - 价格包含一切信息
2. 先活下来，再谈盈利 - 风险控制第一
3. 系统胜于直觉 - 策略可复现
4. 数据为骨，情绪为血 - 技术+心理

分析框架:
- 市场结构: 识别趋势阶段
- 技术验证: SMA交叉、动量、成交量
- 风险评估: 持仓风险、回撤、止损
- 操作纪律: 入场/出场逻辑
```

### User Prompt（数据驱动）

系统会自动构建包含以下信息的提示词：

- **K线形态**：阳线/阴线、实体占比、收盘价
- **均线状态**：多头/空头排列、价格相对 MA5/MA20 的偏离
- **动量判断**：上涨加速/下跌加速/震荡整理
- **成交量**：放量/缩量/正常
- **持仓风险**：空仓/盈利中/亏损较大

### 输出格式

```json
{
  "signal": "BUY",
  "reason": "多头排列+放量突破+短期加速",
  "confidence": "HIGH"
}
```

## 风险提示

⚠️ **重要警告**：

1. **仅供学习研究**：本项目用于技术验证，不构成投资建议
2. **测试网优先**：强烈建议先在 Binance 测试网运行数周
3. **资金管理**：即使测试网，也应设置合理的 `TRADE_AMOUNT`
4. **监控运行**：定期检查日志，观察决策质量
5. **LLM 局限性**：大语言模型不是水晶球，无法预测未来

## 常见问题

### 如何获取 Binance 测试网 API？

**重要更新**：Binance 测试网已迁移到 Demo 平台

1. 访问 https://demo.binance.com
2. 使用 GitHub 或 Google 账号登录（或注册新账号）
3. 进入 Futures 交易页面
4. 点击右上角头像 → API Management
5. 创建新的 API Key（自动获得测试资金）
6. 复制 API Key 和 Secret 到 `.env` 文件

**注意**：
- 测试网 REST API 地址：`https://testnet.binancefuture.com`
- Demo 平台会自动提供虚拟 USDT 用于测试
- 测试网数据与主网实时同步，但交易不影响真实资金

### 如何获取 DeepSeek API？

1. 访问 https://platform.deepseek.com
2. 注册账号并充值（首次赠送免费额度）
3. 在 API Keys 页面创建密钥

### 程序报错：`缺少 BINANCE_API_KEY`

检查 `.env` 文件是否存在且正确配置。

### 程序报错：`获取K线数据失败`

1. 检查网络连接
2. 确认 Binance 测试网是否可访问
3. 查看是否被限流（稍后重试）

### 如何修改交易周期？

编辑 `.env` 文件中的 `TRADE_INTERVAL`：
- `15m` - 15分钟
- `30m` - 30分钟
- `1h` - 1小时

### 如何添加更多交易对？

当前版本仅支持单一交易对。如需同时交易多个币种，需要：
1. 修改 `Config` 结构支持多 symbol
2. 在主循环中为每个 symbol 分别执行 `run_trading_cycle()`

## 代码统计

```
总计: 866 行
├── executor.rs  267行 (交易执行+HMAC签名)
├── llm.rs      179行 (DeepSeek API+提示词)
├── main.rs     166行 (主程序+配置+循环)
├── market.rs   118行 (Binance API+指标计算)
├── types.rs     81行 (数据结构定义)
└── state.rs     55行 (日志记录)
```

## 技术栈

- **Rust** 1.83+ (edition 2021)
- **async-openai** - DeepSeek API 客户端
- **reqwest** - HTTP 请求
- **tokio** - 异步运行时
- **serde/serde_json** - JSON 序列化
- **hmac/sha2** - Binance API 签名
- **anyhow** - 错误处理
- **chrono** - 时间处理

## License

MIT
