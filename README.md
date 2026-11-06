# Multi-Agent Cryptocurrency Auto Trading System

[![CI](https://github.com/Ye-Yu-Mo/trade/actions/workflows/ci.yml/badge.svg)](https://github.com/Ye-Yu-Mo/trade/actions/workflows/ci.yml)
[![License: MIT](https://img.shields.io/badge/License-MIT-yellow.svg)](LICENSE)
[![Rust](https://img.shields.io/badge/rust-stable-orange.svg)](rust-toolchain.toml)

A multi-agent collaborative portfolio trading bot built with Rust, using DeepSeek LLM for decision making, and running on Binance testnet for automated cryptocurrency trading.

> This is a research and education framework, **not** a profitable trading system. See [Known Limitations](#known-limitations).

[English Version](README.md) | [中文版本](README_ZH.md)

## Core Features

- **Multi-Agent Architecture**: Market Analyst → Strategy Researcher → Risk Manager → Trade Executor → Portfolio Coordinator
- **Portfolio Management**: Support for multiple symbols with intelligent fund allocation
- **Professional Role Division**: Each agent specializes in specific domains, collaborative decision making
- **Contract Trading**: Support for long/short positions, automatic position switching
- **Technical Indicators**: SMA(5/20/50/100), Price Change Rate, ATR, Volume Ratio
- **Risk Control**: Position monitoring, P&L tracking, fund allocation, error retry
- **Backtesting**: deterministic SMA baseline and an LLM baseline, with fees and slippage. No API key required for the baseline.

## System Architecture

### Multi-Agent Collaboration Process

```mermaid
flowchart TD
    A[main.rs<br/>Main Program] --> B[Phase 1: Parallel Market Analysis]
    A --> C[Phase 2: Portfolio Fund Allocation]
    A --> D[Phase 3: Sequential Trade Execution]
    
    B --> B1[Market Analyst<br/>market.rs]
    B --> B2[Market Analyst<br/>market.rs]
    B --> B3[Market Analyst<br/>market.rs]
    
    B1 --> E[MarketReport]
    B2 --> E
    B3 --> E
    
    E --> F[Portfolio Coordinator]
    F --> G[PortfolioAllocation]
    
    G --> H[Strategy Researcher]
    H --> I[StrategySuggestion]
    
    I --> J[Risk Manager]
    J --> K[RiskAssessment]
    
    K --> L[Trade Executor<br/>executor.rs]
    L --> M[TradingDecision]
    L --> N[TradeResult]
    
    M --> O[state.rs<br/>Logging]
    N --> O
    
    subgraph "Parallel Execution"
        B1
        B2
        B3
    end
    
    subgraph "Sequential Execution"
        H
        J
        L
    end
    
    style A fill:#e1f5fe
    style B1 fill:#f3e5f5
    style B2 fill:#f3e5f5
    style B3 fill:#f3e5f5
    style F fill:#e8f5e8
    style H fill:#fff3e0
    style J fill:#ffebee
    style L fill:#e8eaf6
    style O fill:#f5f5f5
```

### Agent Role Division

1. **Market Analyst** (`market.rs`)
   - Fetch K-line data, calculate technical indicators
   - Analyze market trends, strength, phases
   - Output: `MarketReport`

2. **Strategy Researcher**
   - Develop trading strategies based on market analysis
   - Suggest target positions, stop-loss, take-profit
   - Output: `StrategySuggestion`

3. **Risk Manager**
   - Assess trading risks, approve strategies
   - Calculate suggested trade quantities
   - Output: `RiskAssessment`

4. **Trade Executor** (`executor.rs`)
   - Make final decisions based on all inputs
   - Execute specific trading operations
   - Output: `TradingDecision`

5. **Portfolio Coordinator**
   - Manage fund allocation across multiple symbols
   - Develop portfolio strategies
   - Output: `PortfolioAllocation`

## Quick Start

### 1. Configure Environment

Copy environment template:
```bash
cp .env.example .env
```

Edit `.env` file with your information:

```bash
# Binance Testnet API (from https://testnet.binancefuture.com)
BINANCE_API_KEY=your_testnet_api_key
BINANCE_SECRET=your_testnet_secret
BINANCE_TESTNET=true

# DeepSeek API (from https://platform.deepseek.com)
DEEPSEEK_API_KEY=your_deepseek_key

# Trading Configuration
TRADE_SYMBOLS=BTCUSDT,ETHUSDT  # Multiple symbols, comma separated
MIN_TRADE_AMOUNT=0.001  # AI decision minimum trade quantity
MAX_TRADE_AMOUNT=0.003  # AI decision maximum trade quantity
TRADE_INTERVAL=1m  # Options: 1m, 15m, 30m, 1h
LEVERAGE=10  # Leverage: 1-125
MAX_POSITION=0.005  # Maximum position per symbol
PORTFOLIO_MODE=balanced  # Portfolio mode: balanced | aggressive | conservative
```

### 2. Build and Run

```bash
cargo build --release
cargo run --release
```

### 3. Check Logs

The program automatically creates `logs/` directory:

- `logs/trades.jsonl` - Trade records (open/close positions, prices, P&L)
- `logs/decisions.jsonl` - Decision records (including multi-agent analysis process)
- `logs/performance.json` - Performance tracking data

Each line is a JSON object, view with `jq`:

```bash
# View recent 10 trades
tail -10 logs/trades.jsonl | jq

# View all decisions
cat logs/decisions.jsonl | jq -c '{symbol: .symbol, signal: .decision.signal, reason: .decision.reason}'

# View performance data
cat logs/performance.json | jq
```

## Backtesting

Backtesting does **not** need a Binance API key. The deterministic SMA baseline is built in.

```bash
cargo run --release -- backtest \
  --symbol BTCUSDT \
  --interval 1h \
  --start 2025-01-01 \
  --end 2025-06-01 \
  --strategy sma
```

```text
================ 回测结果 ================
标的: BTCUSDT | 周期: 1h | 策略: sma_cross
区间: 2025-01-01 00:00 → 2025-03-01 00:00（1298 根K线）
成本: 手续费 0.050% / 单边 | 滑点 2.0 bps
-----------------------------------------
初始资金:      10000.00 USDT
最终权益:      9679.37 USDT
策略收益:      -3.21%  |  买入持有: -14.77%
最大回撤:      7.32%
Sharpe:        -2.03
交易次数:      63  |  胜率: 28.6%  |  暴露时间: 99.8%
盈亏比:        0.70
累计手续费:    124.10 USDT
=========================================
```

How it works:

- A signal is generated at bar close and filled at the **next bar's open**, so there is no look-ahead bias.
- Fees (default 5 bps per side, Binance USDT-M taker) and slippage (default 2 bps) are always applied.
- Low-confidence signals are ignored to avoid churning on flat moving averages.
- Every run reports buy-and-hold return next to the strategy return.
- K-lines are cached in `data/klines/`; results go to `backtest/` as JSON plus equity/trade CSVs.

Key options:

| Option | Default | Meaning |
|---|---|---|
| `--strategy` | `sma` | `sma` (deterministic) or `llm` (DeepSeek) |
| `--position-pct` | `0.2` | Fraction of equity per position |
| `--leverage` | `1.0` | Kept at 1x by default to avoid fake returns |
| `--fee-bps` | `5` | Per-side fee in basis points |
| `--slippage-bps` | `2` | Per-side slippage in basis points |
| `--max-bars` | all | Limit the number of decisions (useful for `--strategy llm`) |

The LLM baseline makes one model call per bar and caches responses in `.cache/llm/`, so repeated
runs over the same range are free. Start with `--max-bars` to control cost.

Run `cargo run -- backtest --help` for the full list.

## Multi-Agent Decision Process

### Phase 1: Parallel Market Analysis

Each symbol executes in parallel:
1. **Fetch K-line Data**: 120 K-lines, calculate technical indicators
2. **Market Analyst Decision**: Analyze trends, strength, market phases
3. **Position Query**: Get current position status

### Phase 2: Portfolio Fund Allocation

1. **Portfolio Coordinator Allocation**: Allocate funds based on symbol analysis results
2. **Strategy Development**: Choose portfolio strategy based on configuration
   - `balanced`: Balanced allocation, moderate risk
   - `aggressive`: Concentrate on best symbols, pursue high returns
   - `conservative`: Diversified investment, control risk

### Phase 3: Sequential Trade Execution

Execute sequentially for each symbol:
1. **Strategy Researcher Suggestion**: Develop specific strategies based on market conditions
2. **Risk Manager Assessment**: Approve strategies, calculate suggested quantities
3. **Trade Executor Execution**: Make comprehensive decisions, execute trades

## Technical Indicators

System calculates the following technical indicators:

- **Moving Averages**: SMA5, SMA20, SMA50, SMA100
- **Price Change Rate**: 1-period, 3-period, 6-period, 12-period
- **ATR Indicator**: 14-period Average True Range
- **Volume Ratio**: Current volume / Average volume

## Risk Control

### Multi-layer Risk Protection

1. **Agent Collaboration**: Each decision reviewed by multiple professional roles
2. **Fund Allocation**: Portfolio coordinator allocates funds reasonably
3. **Trading Constraints**: Adhere to exchange minimum order quantity, minimum notional value rules
4. **Position Limits**: Maximum position control per symbol
5. **Error Handling**: Single failure doesn't affect overall operation

### Portfolio Strategies

- **Balanced Mode**: Equally allocate based on analysis quality
- **Aggressive Mode**: Concentrate funds on most promising symbols
- **Conservative Mode**: Strictly control single symbol risk exposure

## Known Limitations

These are real, and they matter more than the feature list:

1. **The live executor cannot go flat.** `TradingDecision` only carries BUY / SELL / HOLD. While
   holding a long, SELL closes the long *and immediately opens a short*. The strategy layer's
   `close_position` intent is dropped. The backtester models the same behavior for parity.
2. **Stop-loss / take-profit are advisory only.** The strategy agent emits `stop_loss_pct` and
   `take_profit_pct`, but nothing enforces them and no exchange conditional orders are placed.
3. **The backtest is simplified.** It ignores funding rates, margin liquidation, order-book impact
   and partial fills. Fills happen at the next bar open with a fixed slippage.
4. **LLM decisions are non-deterministic.** The same bar can produce different signals. The cache
   avoids repeated charges, not variance. Measure a strategy over many runs before believing it.
5. **No edge is claimed.** A naive SMA baseline and an LLM baseline are both included precisely so
   results can be compared honestly.

## Risk Warning

⚠️ **Important Warnings**:

1. **For Learning and Research Only**: This project is for technical validation, not investment advice
2. **Testnet First**: Strongly recommend running on Binance testnet for several weeks first
3. **Fund Management**: Even on testnet, set reasonable trading parameters
4. **Monitor Operation**: Regularly check logs, observe decision quality
5. **LLM Limitations**: Large language models are not crystal balls, cannot predict the future

## FAQ

### How to Get Binance Testnet API?

**Important Update**: Binance testnet has migrated to Demo platform

1. Visit https://demo.binance.com
2. Login with GitHub or Google account (or register new account)
3. Go to Futures trading page
4. Click top-right avatar → API Management
5. Create new API Key (automatically get test funds)
6. Copy API Key and Secret to `.env` file

**Note**:
- Testnet REST API address: `https://testnet.binancefuture.com`
- Demo platform automatically provides virtual USDT for testing
- Testnet data syncs with mainnet in real-time, but trades don't affect real funds

### How to Get DeepSeek API?

1. Visit https://platform.deepseek.com
2. Register account and top up (first time gets free credits)
3. Create API key in API Keys page

### Program Error: `Missing BINANCE_API_KEY`

Check if `.env` file exists and is correctly configured.

### Program Error: `Failed to fetch K-line data`

1. Check network connection
2. Confirm Binance testnet is accessible
3. Check if rate limited (retry later)

### How to Modify Trading Interval?

Edit `TRADE_INTERVAL` in `.env` file:
- `1m` - 1 minute
- `15m` - 15 minutes
- `30m` - 30 minutes
- `1h` - 1 hour

### How to Add More Trading Pairs?

Edit `TRADE_SYMBOLS` in `.env` file, separate multiple pairs with commas:
```bash
TRADE_SYMBOLS=BTCUSDT,ETHUSDT,SOLUSDT,ADAUSDT
```

## Project Layout

```
src/
├── main.rs          Live / testnet trading loop and portfolio coordination
├── cli.rs           Command line interface (clap)
├── backtest.rs      Paper broker, backtest engine, metrics
├── strategy.rs      Pluggable strategies: SMA cross, LLM baseline
├── data.rs          Historical K-line download and local cache
├── llm_client.rs    DeepSeek client, response cache, JSON parsing
├── multi_agent.rs   Analyst / strategist / risk manager / executor / coordinator
├── executor.rs      Binance USDT-M REST + account WebSocket
├── market.rs        K-line fetching and technical indicators
├── types.rs         Core data structures
├── state.rs         Trade and decision logs
├── performance.rs   Realized P&L and drawdown tracking
└── logging.rs       Logging setup
```

## Tech Stack

- **Rust** 1.83+ (edition 2021)
- **async-openai** - DeepSeek API client
- **reqwest** - HTTP requests
- **tokio** - Async runtime
- **serde/serde_json** - JSON serialization
- **hmac/sha2** - Binance API signing
- **anyhow** - Error handling
- **chrono** - Time handling
- **futures** - Async programming
- **log/flexi_logger** - Logging system

## License

MIT — see [LICENSE](LICENSE).
