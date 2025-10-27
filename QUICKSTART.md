# 快速启动指南

## ✅ 环境已配置完成

你的 API 配置已验证通过：
- ✓ Binance 测试网 API 连接正常
- ✓ DeepSeek API 密钥有效
- ✓ BTC 当前价格: $114,788.00

## 🚀 立即运行

### 方式 1：使用启动脚本（推荐）

```bash
./run.sh
```

### 方式 2：手动编译运行

```bash
cargo build --release
cargo run --release
```

## 📊 运行效果

程序启动后会显示：

```
============================================================
单智能体加密货币自动交易系统
============================================================

交易对: BTCUSDT
交易数量: 0.001
交易周期: 900秒 (15分钟)
API密钥: tFOu2wjY***

启动中...

✓ Binance API 连接成功, BTCUSDT 当前价格: $114788.00

============================================================
执行时间: 2025-10-27 15:30:00
============================================================
✓ 获取到 20 根K线
✓ 技术指标: SMA(5)=114500.23, SMA(20)=113200.45, 价格变化1=+0.12%
✓ 当前持仓: 空仓
✓ 决策: BUY, 信心: MEDIUM, 理由: 多头排列+短期上涨+正常量能
✓ 交易执行: OpenLong, 价格: 114788.00, 数量: 0.0010
```

## 📁 日志文件

程序运行后会自动创建 `logs/` 目录：

```
logs/
├── trades.jsonl      # 交易记录（每笔开仓/平仓）
└── decisions.jsonl   # 决策记录（包含 HOLD 信号）
```

### 查看日志

使用日志查看工具：

```bash
./check_logs.sh
```

或手动查看：

```bash
# 实时监控交易日志
tail -f logs/trades.jsonl

# 查看最近 10 条决策
tail -10 logs/decisions.jsonl | jq

# 统计盈亏
cat logs/trades.jsonl | jq -s '[.[] | select(.pnl != null) | .pnl] | add'
```

## ⚙️ 调整配置

编辑 `.env` 文件修改参数：

```bash
# 修改交易数量
TRADE_AMOUNT=0.002

# 修改交易周期
TRADE_INTERVAL=30m  # 可选: 15m, 30m, 1h

# 切换交易对
TRADE_SYMBOL=ETHUSDT
```

修改后重启程序即可生效。

## 🛡️ 安全提示

1. **当前使用测试网**：你的配置使用 Binance 测试网，不会操作真实资金
2. **观察运行**：建议先运行 24 小时，观察决策质量
3. **检查日志**：定期查看 `logs/` 目录，分析盈亏情况
4. **调整参数**：根据市场情况调整 `TRADE_AMOUNT` 和 `TRADE_INTERVAL`

## 🔧 常见操作

### 停止程序

按 `Ctrl+C` 停止程序。

### 清除日志

```bash
rm -rf logs/
```

### 重新测试配置

```bash
./test_config.sh
```

### 查看完整文档

```bash
cat README.md
```

## 📈 监控建议

建议在另一个终端窗口实时监控日志：

**终端 1：运行程序**
```bash
./run.sh
```

**终端 2：监控决策**
```bash
tail -f logs/decisions.jsonl | jq -c '{时间: (.timestamp | todate), 信号: .decision.signal, 理由: .decision.reason}'
```

**终端 3：监控交易**
```bash
tail -f logs/trades.jsonl | jq -c '{时间: (.timestamp | todate), 动作: .action, 价格: .price, 盈亏: .pnl}'
```

## ⏱️ 预期行为

- **每 15 分钟**执行一次决策周期
- **每次决策**会调用 DeepSeek API（约 1-3 秒）
- **HOLD 信号**只记录日志，不执行交易
- **BUY/SELL 信号**会执行开仓/平仓操作

## 🎯 下一步

1. **观察第一个周期**：等待 15 分钟，查看第一次决策
2. **分析决策逻辑**：查看 `logs/decisions.jsonl`，理解 LLM 的分析
3. **调整策略**：根据实际效果调整 `TRADE_AMOUNT` 或 `TRADE_INTERVAL`
4. **扩展功能**：如需添加更多技术指标，修改 `src/market.rs`

---

**准备好了吗？运行 `./run.sh` 启动系统！**
