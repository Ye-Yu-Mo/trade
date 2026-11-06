# 快速开始

## 0. 一条命令看效果（不需要任何 API key）

```bash
cargo run --release -- demo
```

会跑一次内置样本的 SMA 回测并启动 Dashboard。想调试真实的多智能体决策链：

```bash
cargo run --release -- analyze --symbol BTCUSDT --interval 1h
```

## 1. 先跑一次回测（不需要任何 API key）

```bash
cargo run --release -- backtest \
  --symbol BTCUSDT \
  --interval 1h \
  --start 2025-01-01 \
  --end 2025-06-01 \
  --strategy sma
```

结果会打印在终端，同时写入 `backtest/`：

- `*.json` — 配置、指标、成交明细、权益曲线
- `*_equity.csv` — 权益曲线
- `*_trades.csv` — 每笔成交

K线缓存在 `data/klines/`，同一区间再次回测不会重复下载。

## 2. 配置测试网并跑实盘循环

```bash
cp .env.example .env
# 编辑 .env，填入 Binance 测试网与 DeepSeek 的密钥
```

```bash
./run.sh
# 或者
cargo run --release -- run
```

无子命令时默认就是 `run`，旧用法 `cargo run --release` 仍然有效。

## 3. 查看运行日志

```bash
tail -f logs/app_r*.log
tail -10 logs/trades.jsonl | jq
tail -10 logs/decisions.jsonl | jq
cat logs/performance.json | jq
```

## 4. 常用命令

```bash
# 用 LLM 策略回测，先限制规模控制成本
cargo run --release -- backtest --strategy llm --max-bars 200

# 查看全部回测参数
cargo run --release -- backtest --help

# 策略基准排行榜（SMA / 随机 / 买入持有）
cargo run --release -- benchmark --symbols BTCUSDT,ETHUSDT --interval 1h \
  --start 2025-01-01 --end 2025-06-01

# Walk-forward 样本外回测
cargo run --release -- walkforward --symbols BTCUSDT,ETHUSDT --interval 1h \
  --start 2025-01-01 --end 2025-06-01

# 启动只读 Dashboard（另开一个终端）
cargo run --release -- dashboard --port 8080
# 或随交易循环一起启动
cargo run --release -- run --dashboard 8080

# 跳过手续费/滑点做敏感性对比（不建议用于结论）
cargo run --release -- backtest --fee-bps 0 --slippage-bps 0

# 测试配置连通性
./test_config.sh
```

## 5. 安全提示

1. 默认使用测试网（`BINANCE_TESTNET=true`），不会动真实资金。
2. 本项目是研究/教学框架，不构成投资建议。
3. 回测不模拟资金费率与强平，结果偏乐观。
4. 上主网前至少跑数周测试网，并使用极小仓位。
