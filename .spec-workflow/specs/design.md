# 单智能体加密货币自动交易系统 - Design Document

## Overview

单体架构，数据流单向：**Market Data → Indicators → LLM Decision → Execution → State Logging**

核心原则：
- **数据结构优先**：清晰的结构体定义，避免 HashMap/Any 等弱类型
- **消除特殊情况**：统一的错误处理，统一的决策流程
- **保持简洁**：main.rs + 4个模块，总代码量控制在500行以内
- **单一职责**：每个模块只做一件事，函数不超过30行

## Architecture

```
┌─────────────────────────────────────────────────────────────┐
│                         main.rs                              │
│  - 定时循环调度 (tokio::time::interval)                       │
│  - 错误处理和日志记录                                         │
│  - 环境变量加载 (.env)                                        │
└──────────────────────┬──────────────────────────────────────┘
                       │
         ┌─────────────┼─────────────┬──────────────┐
         │             │             │              │
         ▼             ▼             ▼              ▼
    ┌────────┐   ┌─────────┐   ┌──────────┐   ┌────────┐
    │ market │   │   llm   │   │ executor │   │  state │
    │  .rs   │   │   .rs   │   │   .rs    │   │  .rs   │
    └────────┘   └─────────┘   └──────────┘   └────────┘
        │             │             │              │
        │             │             │              │
   获取K线        调用DeepSeek    执行交易       状态持久化
   计算指标       解析决策        查询持仓       交易日志
```

**数据流：**
1. `market::fetch_klines()` → `KlineData` (20根K线)
2. `market::calculate_indicators()` → `TechnicalIndicators`
3. `llm::analyze()` → `TradingDecision`
4. `executor::execute()` → `TradeResult`
5. `state::log_trade()` → 写入文件

## Components and Interfaces

### 1. market.rs - 市场数据模块

**职责：** 从 Binance API 获取数据并计算技术指标

```rust
// 获取K线数据
pub async fn fetch_klines(
    symbol: &str,
    interval: &str,
    limit: u32
) -> Result<Vec<Kline>>;

// 计算技术指标
pub fn calculate_indicators(klines: &[Kline]) -> TechnicalIndicators;

// 获取当前价格
pub async fn fetch_current_price(symbol: &str) -> Result<f64>;
```

### 2. llm.rs - LLM决策模块

**职责：** 调用 DeepSeek API 生成交易决策

```rust
// 构建提示词
fn build_prompt(
    klines: &[Kline],
    indicators: &TechnicalIndicators,
    position: &Option<Position>,
) -> String;

// 调用LLM并解析决策
pub async fn analyze(
    klines: &[Kline],
    indicators: &TechnicalIndicators,
    position: &Option<Position>,
) -> Result<TradingDecision>;
```

### 3. executor.rs - 交易执行模块

**职责：** 通过 Binance 测试网 API 执行交易

```rust
// 查询当前持仓
pub async fn get_position(symbol: &str) -> Result<Option<Position>>;

// 执行交易决策
pub async fn execute_decision(
    symbol: &str,
    decision: &TradingDecision,
    current_position: &Option<Position>,
    current_price: f64,
) -> Result<TradeResult>;

// 内部：开多/平多/开空/平空的具体实现
async fn open_long(symbol: &str, amount: f64) -> Result<Order>;
async fn close_long(symbol: &str, amount: f64) -> Result<Order>;
async fn open_short(symbol: &str, amount: f64) -> Result<Order>;
async fn close_short(symbol: &str, amount: f64) -> Result<Order>;
```

### 4. state.rs - 状态管理模块

**职责：** 交易日志记录和状态持久化

```rust
// 记录交易
pub fn log_trade(trade_result: &TradeResult) -> Result<()>;

// 记录决策（包含未执行的HOLD信号）
pub fn log_decision(
    symbol: &str,
    decision: &TradingDecision,
    position: &Option<Position>,
) -> Result<()>;
```

## Data Models

### 核心数据结构

```rust
// K线数据
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Kline {
    pub timestamp: i64,
    pub open: f64,
    pub high: f64,
    pub low: f64,
    pub close: f64,
    pub volume: f64,
}

// 技术指标
#[derive(Debug, Clone, Serialize)]
pub struct TechnicalIndicators {
    pub sma_5: f64,
    pub sma_20: f64,
    pub price_change_1: f64,  // 1周期涨跌幅 (%)
    pub price_change_3: f64,  // 3周期涨跌幅 (%)
    pub volume_ratio: f64,    // 当前成交量/20均量
}

// 交易决策
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TradingDecision {
    pub signal: Signal,       // BUY | SELL | HOLD
    pub reason: String,
    pub confidence: Confidence, // HIGH | MEDIUM | LOW
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum Signal {
    Buy,
    Sell,
    Hold,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum Confidence {
    High,
    Medium,
    Low,
}

// 持仓信息
#[derive(Debug, Clone, Serialize)]
pub struct Position {
    pub side: PositionSide,   // Long | Short
    pub amount: f64,
    pub entry_price: f64,
    pub unrealized_pnl: f64,
}

#[derive(Debug, Clone, Serialize)]
pub enum PositionSide {
    Long,
    Short,
}

// 交易结果
#[derive(Debug, Clone, Serialize)]
pub struct TradeResult {
    pub symbol: String,
    pub action: TradeAction,
    pub price: f64,
    pub amount: f64,
    pub timestamp: i64,
    pub reason: String,
    pub pnl: Option<f64>,  // 平仓时才有盈亏
}

#[derive(Debug, Clone, Serialize)]
pub enum TradeAction {
    OpenLong,
    CloseLong,
    OpenShort,
    CloseShort,
    Hold,
}
```

## Error Handling

### 统一错误处理策略

1. **使用 `anyhow::Result<T>`**：所有可能失败的函数返回 Result
2. **不让单次错误中断主循环**：
   ```rust
   loop {
       if let Err(e) = run_trading_cycle().await {
           eprintln!("Cycle failed: {:#}", e);
           // 记录错误后继续
       }
       interval.tick().await;
   }
   ```

3. **网络错误重试**：
   - Binance API 调用失败：重试3次，间隔2秒
   - DeepSeek API 超时：60秒后失败，记录并跳过本次周期

4. **数据验证**：
   - K线数据不足20根：返回错误，不计算指标
   - LLM返回非JSON：返回错误，不执行交易
   - 持仓查询失败：假设空仓，记录警告

5. **日志分级**：
   - **ERROR**：API调用失败、订单执行失败
   - **WARN**：数据不足、LLM解析失败
   - **INFO**：交易决策、订单成功

## Testing Strategy

### MVP阶段测试方法

1. **手动测试优先**（第一周）：
   - 运行程序，观察日志输出
   - 检查 Binance 测试网账户持仓变化
   - 验证交易日志文件内容正确性

2. **关键路径测试**：
   - 测试 K线数据获取和指标计算（打印验证）
   - 测试 DeepSeek API 调用和JSON解析（单独测试脚本）
   - 测试交易执行逻辑（空仓→多仓→空仓→空仓，验证状态转换）

3. **边界情况测试**：
   - 网络断开时程序不崩溃
   - K线数据不足时跳过本次周期
   - LLM返回格式错误时记录并跳过

4. **第二周添加单元测试**：
   - `calculate_indicators()` 的准确性
   - `build_prompt()` 的格式正确性
   - 持仓状态转换逻辑（不依赖真实API）
