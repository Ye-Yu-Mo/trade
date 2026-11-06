# We built an LLM trading framework you can verify. Then our SMA rule lost to a coin flip.

> Rust + DeepSeek multi-agent trading framework. The point is not "it makes money". The point is "you can check".
>
> https://github.com/Ye-Yu-Mo/trade

## The least valuable thing in a trading repo is a profit screenshot

Most LLM trading projects show a nice equity curve and a video. Then you discover the repo needs a mainnet API key to run, the backtest ignores fees, and the parameters were picked on the full history.

We wanted the opposite: something anyone can run in 60 seconds, and whose results can be questioned and reproduced.

## 60 seconds, no API key

```bash
git clone https://github.com/Ye-Yu-Mo/trade
cd trade
cargo run --release -- demo
```

It runs a deterministic SMA backtest on a bundled 90-day BTCUSDT 1h sample, seeds a dashboard, and serves it at `http://127.0.0.1:8080`. You get the equity curve, trades, risk state, recent decisions, a backtest browser, and an **agent trace** recorded from a real DeepSeek run.

No network. No keys. No pretending it is profitable.

## How we try not to fool ourselves

**Walk-forward, not "best parameters on history".** The data is split into rolling `train` / `test` windows. We search fast/slow pairs on the training window and evaluate the chosen pair only on the next, unseen window. Test equity is chained; multiple symbols are equal-weighted.

**Costs are on by default.** 5 bps per side, 2 bps slippage.

**Risk is code, not a prompt.** The risk manager is a deterministic function: timing score below 5 is rejected, ATR above 8% is rejected / above 5% halves the size, counter-trend direction halves it again, position and exchange limits are enforced. The LLM executor's output is then clamped: `rejected` becomes HOLD, `adjusted` gets its size truncated. The model can propose, but it cannot override risk.

## First benchmark

BTCUSDT + ETHUSDT, 1h, 2025-01-01 → 2025-06-01, train 720 / test 240, 5 bps + 2 bps, random seed 42:

| Strategy | Portfolio | Buy & hold | Max DD | Sharpe | Trades | Win rate | Fees |
|---|---:|---:|---:|---:|---:|---:|---:|
| SMA walk-forward | -7.42% | +2.86% | 7.54% | -2.28 | 422 | 33.4% | 404.31 |
| Random (p=0.05) | +0.17% | +2.86% | 4.18% | 0.11 | 213 | 41.8% | 213.36 |
| Buy & hold | +2.76% | +2.76% | 37.38% | 0.48 | 0 | — | 10.00 |

Three things stood out:

1. **The SMA rule lost to a coin flip.** 422 trades and 404 USDT of fees on a 10,000 USDT account — about 4% of capital paid in costs. Churning an hourly signal is hard to make back.
2. **Buy & hold made more, with a 37% drawdown.** Return without risk is not a result.
3. **Walk-forward deflates "optimal" parameters.** A single-range backtest can look great; the next unseen window usually disagrees.

An unflattering result is part of the product. A framework that can only produce good news is probably lying.

## Making the agents observable

`trade analyze` runs one full decision chain without placing orders:

```bash
cargo run --release -- analyze --symbol BTCUSDT --interval 1h
```

A real run:

```
[1/4] Market analyst (1477ms, 4419 tokens): Bearish / Strong / Markdown
      Volume breakdown, bearish MA stack, new low, weak bounce.
[2/4] Strategy (1355ms, 1236 tokens): OpenShort | timing 8/10
[3/4] Risk (code): Low | Approved | size 0.026
[4/4] Executor (1012ms, 1327 tokens): Sell | High
```

Every stage is written to `logs/agent_trace.jsonl` with the model, cache hit, latency, token usage and full JSON output. The dashboard renders it as a timeline.

That is the real reason for multiple agents: not to look human, but to split a decision into steps you can inspect and replace independently.

## Engineering choices

- 2 model calls per symbol per cycle (strategy + executor); risk is code.
- JSON mode, 60s timeout, retries, and a cache key that includes model / base URL / temperature.
- Exchange-side `STOP_MARKET` / `TAKE_PROFIT_MARKET` orders with `closePosition=true`.
- Explicit `CLOSE` action so exiting a position does not force a reversal.
- 69 unit tests covering the paper broker, risk rules, indicators, cache keys and path safety.

## Limitations

- No funding rates, liquidation or order-book impact; intrabar stops are inferred from high/low.
- LLM decisions are non-deterministic.
- Only Binance USDT-M futures.
- The LLM benchmark is opt-in (one call per test bar), so the table above does not include it.
- Six months and two symbols is a small sample. Do not extrapolate.

## Next

- Run the LLM benchmark across models
- More exchange adapters
- A one-command Docker demo
- A prompt evaluation set so people can submit their own results

If you get different benchmark numbers, please open an issue. Negative results are useful.

**Research and education only. Not investment advice.**

https://github.com/Ye-Yu-Mo/trade
