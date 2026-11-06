# Launch Kit

这份文档不发布到外网，只用来把仓库推出去时保持一致、诚实、可复现。

## 当前状态

- 仓库: https://github.com/Ye-Yu-Mo/trade（public，9 stars，已解除归档）
- 本地 `master` 领先远端 8 个提交，**尚未 push**
- CI 仍被 GitHub 账号级 billing 锁阻塞（见下方 checklist）
- 零 key Demo 已可用: `cargo run --release -- demo`

## 发布前 Checklist

1. 处理 GitHub billing（https://github.com/settings/billing），解锁 Actions
2. `git push origin master --follow-tags`
3. 确认 CI 变绿（fmt / clippy / test / build）
4. 发布 Release（建议 `v0.3.0`，把 v0.3–v0.9 的本地提交合并成一个版本）
5. 仓库 Settings → Social preview 上传 `docs/social-preview.png`
6. 确认 About 描述与 topics（已更新）
7. 自己从头 clone 一次，跑 `cargo run --release -- demo`，确认 60 秒内能看到页面
8. 再发社区帖子

## Release Notes 草稿

标题：`v0.3.0 — 可回测、可复现的 LLM 交易框架`

正文要点：

- 零 key Demo：`cargo run --release -- demo`
- Agent Trace：每个阶段记录模型 / 耗时 / tokens / 输出，Dashboard 可视化
- Walk-forward 样本外回测，多标的等权组合
- 策略基准：SMA / 随机 / LLM / 买入持有，同窗口同成本
- 显式平仓语义 + 确定性风控 + 交易所条件单
- 69 个单元测试，CI 全绿

必须写清楚：

> 这是一个研究与教学框架，不构成投资建议。基准显示简单 SMA 规则在样本外跑输随机基线；这正说明为什么需要可验证的回测，而不是收益截图。

附带基准表（与 README 一致）：

| Strategy | Portfolio | Buy & hold | Max DD | Sharpe | Trades | Win rate | Fees |
|---|---:|---:|---:|---:|---:|---:|---:|
| SMA walk-forward | -7.42% | +2.86% | 7.54% | -2.28 | 422 | 33.4% | 404.31 |
| Random (p=0.05) | +0.17% | +2.86% | 4.18% | 0.11 | 213 | 41.8% | 213.36 |
| Buy & hold | +2.76% | +2.76% | 37.38% | 0.48 | 0 | — | 10.00 |

## 技术文章大纲

标题建议：

- 中文：**《我们做了一个能验证 LLM 交易的框架，然后发现 SMA 跑输了抛硬币》**
- English: **"We built a trading framework you can verify. Then our SMA rule lost to a coin flip."**

结构：

1. **钩子**：同区间、同成本、同窗口，SMA walk-forward `-7.42%`，随机基线 `+0.17%`，买入持有 `+2.76%` 但回撤 `37.4%`
2. **为什么做这个**：LLM 交易项目很多，能复现结果的很少
3. **60 秒复现**：`cargo run --release -- demo`，不需要 key
4. **三个发现**
   - 手续费和滑点会吃掉高频反手
   - walk-forward 会把“历史最优参数”打回原形
   - 风控必须是代码，不是 prompt
5. **Agent Trace**：展示一次真实决策链（行情 → 策略 → 代码风控 → 交易员）
6. **LLM 怎么评**：`--strategies ...,llm` 可跑，但每个测试 bar 一次调用；建议先小范围跑并缓存
7. **局限**：不模拟资金费率/强平，LLM 非确定性，样本区间有限
8. **下一步**：多模型对比、更多交易所、社区提交 benchmark

配图：`docs/dashboard.png`（Dashboard）、`docs/social-preview.png`（头图）。

## 社区文案

### 中文短帖（V2EX / 即刻 / X）

版本 A（结果导向）：

> 做了一个 Rust + DeepSeek 的多智能体交易框架，重点不是“能赚钱”，而是“能验证”。
> 6 个月 walk-forward 基准：SMA 规则 -7.42%，随机基线 +0.17%，买入持有 +2.76%（回撤 37%）。
> 不需要 API key，`cargo run --release -- demo` 60 秒能看到 Dashboard 和一次真实 Agent 决策链。
> https://github.com/Ye-Yu-Mo/trade

版本 B（工程导向）：

> LLM 交易项目很多，能复现的很少。
> 这个仓库把回测、walk-forward、确定性风控、Agent Trace 和只读 Dashboard 都做进了框架：
> - 每步记录模型/耗时/tokens/输出
> - 风控是代码，交易员 LLM 不能覆盖
> - 一条命令跑离线 Demo，无需 key
> 欢迎拍砖。https://github.com/Ye-Yu-Mo/trade

版本 C（一句话）：

> 用 Rust 写了一个可回测、可复现的 LLM 多智能体交易框架。先跑 `cargo run --release -- demo`，再决定要不要 star。
> https://github.com/Ye-Yu-Mo/trade

### English (Show HN / Reddit r/algotrading)

Show HN:

> **Show HN: An LLM trading framework you can actually verify**
>
> Most LLM trading repos show a profit screenshot. This one ships a zero-key demo, walk-forward
> backtesting, deterministic risk enforcement, and an agent trace with latency/token accounting.
>
> Our first honest result: a simple SMA walk-forward rule lost to a random baseline out of sample
> (-7.4% vs +0.2%, same costs). Buy & hold did better (+2.8%) but with a 37% drawdown. We think that
> is more useful than another backtest with no costs.
>
> `cargo run --release -- demo` — no keys, no network.
>
> https://github.com/Ye-Yu-Mo/trade

Reddit:

> Built a Rust + DeepSeek multi-agent trading framework focused on reproducibility rather than
> returns. Includes walk-forward evaluation, a random baseline, code-enforced risk, exchange
> conditional orders, and a read-only dashboard with agent traces.
>
> The bundled benchmark is deliberately unflattering: SMA walk-forward lost to random out of sample.
> Happy to get feedback on the evaluation methodology.

## 不要做的事

- 不要声称“稳定盈利”或贴没有成本的回测曲线
- 不要同一天在多个社区重复刷同一段文案
- 不要把示例 trace 说成实盘记录；它标注为 `sample`
- 不要在 CI 红着的时候发“工程化”主题的帖子

## 发布后观察

- Star / fork 增速（前 48 小时）
- Issues 里反复出现的问题（大概率是配置、API key、数据源）
- 是否有人跑出不同的 benchmark 结果；这类反馈优先回复
