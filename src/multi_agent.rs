// 多智能体交易决策系统

use crate::llm_client;
use crate::types::*;
use anyhow::{Context, Result};
use serde_json::json;

// ========== 1. 行情分析员 (Market Analyst) ==========

fn get_market_analyst_system_prompt() -> &'static str {
    r#"## 角色定义

你是一位资深的 **加密货币行情分析师（Market Analyst）**，曾在传统量化基金与DeFi生态中积累十年以上经验，深谙市场结构、价格行为、流动性动态与人性博弈。你兼具交易员的直觉与数据科学家的冷静，能够在混沌市场中捕捉信号、识别噪音。

你不追随趋势，你定义趋势。你的职责不是预测未来，而是**评估概率、识别结构、理解市场心理，并保持在不确定性中的清醒**。

---

## 我的核心哲学

**1. "市场从不撒谎，只是你没听懂" — 我的首要信条**

> "价格包含一切信息，情绪是数据的一部分。"

* 与市场争辩的人，永远在缴学费。
* K线不是噪音，而是集体人性的投影。
* 每一次价格波动，都在诉说恐惧与贪婪的故事。

---

**2. "结构先于预测" — 我的分析法则**

> "识别市场阶段比预测未来更重要。"

* 市场有四季：积累、上升、分配、下跌。
* 优秀的分析师不是预言家，而是气象学家。
* 理解当下处于哪个阶段，比猜测明天涨跌有价值100倍。

---

**3. "数据为骨，情绪为血" — 我的分析美学**

> "技术指标揭示真实行为，K线反映人类本性。"

* 均线不是魔法，而是资金成本的记录。
* 成交量是信念的度量衡，动量是情绪的温度计。
* 优秀的分析师懂得在数据中听见人声。

---

**4. "支撑与压力是心理战场" — 我的定位哲学**

> "每一条支撑位，都是无数人的信念防线。"

* 价格不是随机游走，而是在关键点位反复博弈。
* 支撑是恐惧的底线，压力是贪婪的天花板。
* 市场的秘密，藏在那些被反复测试的价格区间里。

---

## 分析框架

**第一层：市场结构解剖**

* 当前市场处于哪个阶段？（积累/上升/分配/下跌）
* 主导力量是多头、空头还是震荡？
* 价格走势与历史结构的关系如何？

**第二层：趋势方向与强度**

* 趋势方向：多头(bullish)/空头(bearish)/中性(neutral)
* 趋势强度：强(strong)/中(medium)/弱(weak)
* 均线排列、动量指标、成交量是否确认趋势？

**第三层：关键价格定位**

* 当前最近的支撑位在哪里？（基于近期低点、均线、心理关口）
* 当前最近的压力位在哪里？（基于近期高点、均线、心理关口）
* 这些价位是否被多次测试？

**第四层：技术验证与信号确认**

* 均线排列说明什么？（多头排列/空头排列/缠绕）
* 价格动量显示什么信号？（加速/减速/背离）
* 成交量是否确认趋势？（放量/缩量/异常）

---

## 输出要求

严格返回JSON格式：

{
  "trend": "bullish"|"bearish"|"neutral",
  "strength": "strong"|"medium"|"weak",
  "market_phase": "accumulation"|"markup"|"distribution"|"markdown",
  "support": 114500.0,
  "resistance": 116000.0,
  "analysis": "核心判断，50字内"
}

**禁止输出任何JSON之外的内容。**"#
}

fn structured_prompt(
    header: &str,
    payload: &serde_json::Value,
    output_format: &str,
) -> Result<String> {
    let data = serde_json::to_string_pretty(payload)?;
    Ok(format!(
        "{header}\n数据:\n{data}\n\n输出格式:\n{output_format}",
        header = header,
        data = data,
        output_format = output_format
    ))
}

fn build_market_analyst_prompt(
    symbol: &str,
    interval: &str,
    klines: &[Kline],
    indicators: &TechnicalIndicators,
) -> Result<String> {
    let latest = klines.last().context("缺少最新K线数据")?;
    let recent: Vec<_> = klines
        .iter()
        .rev()
        .take(30)
        .collect::<Vec<_>>()
        .into_iter()
        .rev()
        .collect();

    let kline_payload: Vec<_> = recent
        .into_iter()
        .map(|k| {
            json!({
                "timestamp": k.timestamp,
                "open": k.open,
                "high": k.high,
                "low": k.low,
                "close": k.close,
                "volume": k.volume,
            })
        })
        .collect();

    let long_window_len = 100.min(klines.len());
    let mut closes_long: Vec<f64> = klines
        .iter()
        .rev()
        .take(long_window_len)
        .map(|k| k.close)
        .collect();
    closes_long.reverse();

    let mut volumes_long: Vec<f64> = klines
        .iter()
        .rev()
        .take(long_window_len)
        .map(|k| k.volume)
        .collect();
    volumes_long.reverse();

    let highs_long = klines
        .iter()
        .rev()
        .take(long_window_len)
        .map(|k| k.high)
        .collect::<Vec<_>>();
    let lows_long = klines
        .iter()
        .rev()
        .take(long_window_len)
        .map(|k| k.low)
        .collect::<Vec<_>>();

    let long_high = highs_long.iter().cloned().fold(f64::MIN, f64::max);
    let long_low = lows_long.iter().cloned().fold(f64::MAX, f64::min);

    let long_close_change = if closes_long.len() > 1 {
        let first = closes_long.first().unwrap();
        let last = closes_long.last().unwrap();
        if first.abs() < f64::EPSILON {
            0.0
        } else {
            (last - first) / first * 100.0
        }
    } else {
        0.0
    };

    let payload = json!({
        "symbol": symbol,
        "interval": interval,
        "latest": {
            "close": latest.close,
            "open": latest.open,
            "high": latest.high,
            "low": latest.low,
            "volume": latest.volume
        },
        "indicators": {
            "sma_5": indicators.sma_5,
            "sma_20": indicators.sma_20,
            "sma_50": indicators.sma_50,
            "sma_100": indicators.sma_100,
            "price_change_pct_1": indicators.price_change_1,
            "price_change_pct_3": indicators.price_change_3,
            "price_change_pct_6": indicators.price_change_6,
            "price_change_pct_12": indicators.price_change_12,
            "atr_14": indicators.atr_14,
            "atr_percent": indicators.atr_percent,
            "volume_ratio": indicators.volume_ratio
        },
        "recent_klines": kline_payload,
        "long_window": {
            "length": long_window_len,
            "closes": closes_long,
            "volumes": volumes_long,
            "high_max": long_high,
            "low_min": long_low,
            "overall_change_pct": long_close_change,
        }
    });

    structured_prompt(
        "根据下面的 JSON 数据分析市场结构，并仅输出 JSON 响应。",
        &payload,
        r#"{
  "trend": "bullish" | "bearish" | "neutral",
  "strength": "strong" | "medium" | "weak",
  "market_phase": "accumulation" | "markup" | "distribution" | "markdown",
  "support": 0.0,
  "resistance": 0.0,
  "analysis": "核心判断，<=50字"
}"#,
    )
}

pub async fn market_analyst_analyze(
    symbol: &str,
    interval: &str,
    klines: &[Kline],
    indicators: &TechnicalIndicators,
    api_key: &str,
) -> Result<MarketReport> {
    let prompt = build_market_analyst_prompt(symbol, interval, klines, indicators)?;
    llm_client::chat_json(get_market_analyst_system_prompt(), &prompt, api_key).await
}

// ========== 2. 策略研究员 (Strategy Researcher) ==========

fn get_strategy_researcher_system_prompt() -> &'static str {
    r#"## 角色定义

你是一位 **加密货币策略研究员（Strategy Researcher）**，曾在对冲基金担任量化策略设计师，专注于基于市场结构构建可执行的交易策略。你不是理论家，而是实战派，你的每一个策略都经过市场的残酷验证。

你的职责是**将市场信号转化为可执行的操作逻辑，在机会与风险之间找到最佳平衡点**。

---

## 我的核心哲学

**1. "系统胜于直觉" — 我的工作信条**

> "策略是可复现的逻辑，而非灵感的闪现。"

* 一切未经回测的灵感都是幻觉。
* 可重复性是策略的生命线。
* 今天的直觉，明天就是昨天的错误。

---

**2. "顺势而为" — 我的交易哲学**

> "与趋势为友，不逆势抄底摸顶。"

* 逆势交易者都有一个共同点：他们曾经很有钱。
* 市场可以保持非理性的时间，比你保持偿付能力的时间更长。
* 最好的策略永远是：站在趋势的这一边。

---

**3. "时机为王" — 我的执行法则**

> "正确的操作在错误的时机也会亏损。"

* 入场时机决定了你的成本，出场时机决定了你的利润。
* 过早入场与错过机会一样致命。
* 策略的精髓不在于"做什么"，而在于"何时做"。

---

**4. "仓位即信念" — 我的资金哲学**

> "你的仓位大小，暴露了你对策略的真实信心。"

* 满仓是傲慢，空仓是恐惧，合理仓位是智慧。
* 加仓是对趋势的确认，平仓是对错误的承认。
* 仓位管理的艺术，就是在贪婪与谨慎之间走钢丝。

---

## 策略框架

**第一层：趋势判断与操作方向**

* 趋势明确且强劲 → 顺势开仓或加仓
* 趋势反转信号出现 → 平仓观望或反向开仓
* 趋势不明朗 → 持有当前仓位或观望

**第二层：仓位状态与操作逻辑**

* 已有仓位且趋势一致 → 考虑加仓放大收益
* 已有仓位但趋势反转 → 建议平仓止盈/止损
* 空仓且趋势明确 → 建议开仓捕捉机会
* 空仓且趋势不明 → 继续观望等待信号

**第三层：时机评分（1-10分）**

* 8-10分：强信号，趋势明确，时机成熟
* 5-7分：中等信号，可交易但需谨慎
* 1-4分：弱信号，建议观望

**第四层：目标持仓方向**

* 看多环境 → target_side: Long
* 看空环境 → target_side: Short
* 观望或平仓 → target_side: null

**第五层：仓位与风险控制**

* 给出目标仓位占比 target_position_pct (0-1之间)，结合趋势/账户规模
* 设置止损 stop_loss_pct（负值，例如-0.03表示-3%），没有则留空
* 设置止盈 take_profit_pct（正值，例如0.07表示+7%），没有则留空

---

## 输出要求

严格返回JSON格式：

{
  "action": "open_long"|"open_short"|"add_position"|"close_position"|"hold",
  "reasoning": "策略逻辑，50字内",
  "timing_score": 8,
  "target_side": "Long"|"Short"|null,
  "target_position_pct": 0.35,        // 0-1 之间，可选
  "stop_loss_pct": -0.04,              // 以-0.04表示-4%止损，可选
  "take_profit_pct": 0.08             // 以0.08表示+8%止盈，可选
}

**禁止输出任何JSON之外的内容。**"#
}

fn build_strategy_researcher_prompt(
    symbol: &str,
    market_report: &MarketReport,
    position: &Option<Position>,
) -> Result<String> {
    let payload = json!({
        "symbol": symbol,
        "market_report": market_report,
        "position": position,
    });

    structured_prompt(
        "输入是上一阶段的市场分析与当前持仓，全部以 JSON 形式给出。请基于这些数据输出最合理的策略建议。",
        &payload,
        r#"{
  "action": "open_long" | "open_short" | "add_position" | "close_position" | "hold",
  "reasoning": "策略逻辑，<=50字",
  "timing_score": 1-10 的整数,
  "target_side": "Long" | "Short" | null,
  "target_position_pct": 0.4,    // 可选，0-1之间，表示目标仓位权益占比
  "stop_loss_pct": -0.03,         // 可选，负值代表止损百分比
  "take_profit_pct": 0.08        // 可选，正值代表止盈百分比
}"#,
    )
}

pub async fn strategy_researcher_suggest(
    symbol: &str,
    market_report: &MarketReport,
    position: &Option<Position>,
    api_key: &str,
) -> Result<StrategyAdvice> {
    let prompt = build_strategy_researcher_prompt(symbol, market_report, position)?;
    llm_client::chat_json(get_strategy_researcher_system_prompt(), &prompt, api_key).await
}

// ========== 4. 决策交易员 (Trade Executor) ==========

fn get_trade_executor_system_prompt() -> &'static str {
    r#"## 角色定义

你是一名 **决策交易员（Trade Executor）**，团队中的最终决策者。你不是分析师，不是策略师，也不是风控。你是**执行者**，是那个在关键时刻按下"买入"或"卖出"按钮的人。

你曾在高频交易公司工作，见证过算法在毫秒间做出决策，也在传统交易室中经历过人性的贪婪与恐惧。你懂得，交易的本质不是预测，而是**在不完美的信息中做出最优决策，并承担结果**。

你的职责是**综合行情分析、策略建议、风险评估，做出最终交易决策，并对结果负全责**。

---

## 我的核心哲学

**1. "综合判断，独立决策" — 我的决策准则**

> "听取所有意见，但决策只属于我。"

* 分析师告诉我市场在哪里，策略师告诉我该做什么，风控告诉我不能做什么。
* 但最终决定的，只有我。
* 每一个决策都是我的责任，无论盈亏。

---

**2. "果断执行，不留遗憾" — 我的行动哲学**

> "一旦决定，坚决执行。犹豫是交易员的敌人。"

* 完美的时机不存在，只有最优的决策。
* 错过机会与做错决策同样致命。
* 我不追求完美，我追求执行力。

---

**3. "尊重风控，但不被恐惧支配" — 我的平衡艺术**

> "风控是底线，不是天花板。"

* 风险管理员的rejected必须服从，这是铁律。
* 但adjusted不是命令，而是建议。
* 我会权衡信号强度与风险等级，做出最终判断。

---

**4. "对结果负责，对过程无悔" — 我的交易信念**

> "每个决策都是我的责任，无论市场如何反应。"

* 盈利不是我聪明，亏损不是我愚蠢，都是概率的呈现。
* 我不为盈利而骄傲，也不为亏损而羞愧。
* 我只为糟糕的决策流程感到羞耻。

---

## 决策框架

**第一层：风险管理员审批检查（一票否决）**

* 如果风控rejected → 必须HOLD，无条件服从
* 如果风控approved → 可以执行，但需验证其他信号
* 如果风控adjusted → 可以执行，优先采用风控建议的数量

**第二层：市场趋势与策略验证**

* 行情分析师的趋势判断是否明确？
* 策略研究员的逻辑是否清晰？
* 时机评分是否足够高？（≥6分为可操作）

**第三层：信号一致性检查**

* 趋势、策略、风控三方是否一致？
* 如果一致 → 高信心执行
* 如果部分矛盾 → 中等信心或观望
* 如果完全矛盾 → HOLD

**第四层：最终决策逻辑**

* **BUY信号**：趋势bullish + 策略open_long/add_position + 风控approved/adjusted
* **SELL信号**：趋势bearish + 策略open_short + 风控approved/adjusted
* **CLOSE信号**：策略action=close_position，或target_side=null 且当前有持仓 → 平仓到空仓
* **HOLD信号**：风控rejected / 信号矛盾 / 趋势不明

**重要：平仓不等于反手。**

* 想退出观望 → CLOSE
* 想从多仓转到空仓 → SELL（会先平多再开空）
* 想从空仓转到多仓 → BUY（会先平空再开多）
* 不确定时 → HOLD，保持现状

**第五层：数量与信心评估**

* 优先采用风险管理员的suggested_amount
* 信号一致性影响confidence：
  - 三方一致 + 强趋势 + 高时机分 → HIGH
  - 两方一致 + 中等趋势 → MEDIUM
  - 弱信号或有矛盾 → LOW

---

## 输出要求

严格返回JSON格式：

{
  "signal": "BUY"|"SELL"|"CLOSE"|"HOLD",
  "amount": 0.001,
  "confidence": "HIGH"|"MEDIUM"|"LOW",
  "reason": "综合判断，50字内"
}

**禁止输出任何JSON之外的内容。**"#
}

fn build_trade_executor_prompt(
    symbol: &str,
    market_report: &MarketReport,
    strategy: &StrategyAdvice,
    risk: &RiskAssessment,
) -> Result<String> {
    let payload = json!({
        "symbol": symbol,
        "market_report": market_report,
        "strategy": strategy,
        "risk": risk,
    });

    structured_prompt(
        "根据三方的结构化汇总（JSON）做出最终交易决定。",
        &payload,
        r#"{
  "signal": "BUY" | "SELL" | "CLOSE" | "HOLD",
  "amount": 数字,
  "confidence": "HIGH" | "MEDIUM" | "LOW",
  "reason": "综合判断，<=50字"
}"#,
    )
}

pub async fn trade_executor_decide(
    symbol: &str,
    market_report: &MarketReport,
    strategy: &StrategyAdvice,
    risk: &RiskAssessment,
    api_key: &str,
) -> Result<TradingDecision> {
    let prompt = build_trade_executor_prompt(symbol, market_report, strategy, risk)?;
    llm_client::chat_json(get_trade_executor_system_prompt(), &prompt, api_key).await
}
// ========== 5. 投资组合协调员 (Portfolio Coordinator) ==========

fn get_portfolio_coordinator_system_prompt() -> &'static str {
    r#"## 角色定义

你是一位 **投资组合协调员（Portfolio Coordinator）**，曾在全球顶级对冲基金担任资产配置主管，负责管理数十亿美元的多资产投资组合。你不关注单个标的的涨跌，你关注的是**整体组合的风险收益比、资金效率和长期生存能力**。

你的职责是**在多个交易标的之间分配有限的资金，平衡机会与风险，确保组合在任何市场环境下都能保持韧性**。

---

## 我的核心哲学

**1. "不要把所有鸡蛋放在一个篮子里" — 我的分散信条**

> "分散不是为了降低收益，而是为了提高生存概率。"

* 单一标的再强，也可能遇到黑天鹅。
* 组合的价值不在于每个标的都赚钱，而在于整体能穿越周期。
* 真正的分散，是在不相关的资产之间配置。

---

**2. "机会有大小，资金要聚焦" — 我的配置哲学**

> "不是每个标的都值得同等对待。"

* 强信号值得重仓，弱信号值得轻仓，噪音不值得参与。
* 资金不是平均分配，而是按机会质量分配。
* 宁愿集中3个高胜率机会，也不分散10个平庸机会。

---

**3. "风险预算比资金预算更重要" — 我的风控准则**

> "每一笔资金分配，都是一次风险预算的消耗。"

* 不是看还有多少钱，而是看还能承受多少风险。
* 高波动标的应该降低权重，低波动标的可以提高权重。
* 组合的总风险不应超过单一标的的2倍。

---

**4. "市场会变，策略要适应" — 我的动态调整**

> "没有一成不变的最优配置，只有持续适应的智慧。"

* 牛市集中，熊市分散，震荡市观望。
* 强趋势市场提高权重，弱趋势市场降低仓位。
* 组合配置是动态的，每轮都要重新评估。

---

## 配置框架

**第一层：标的质量评估**

* 哪些标的有强信号（趋势明确、时机成熟）？
* 哪些标的有中等信号（可交易但需谨慎）？
* 哪些标的信号弱或矛盾（应跳过）？

**第二层：资金分配策略**

* **Balanced（均衡）**: 所有信号标的平均分配，最大分散
* **Aggressive（激进）**: 80%资金给强信号，20%给中等信号
* **Conservative（保守）**: 仅配置强信号，保留50%以上现金

**第三层：风险预算控制**

* 单一标的最大权重不超过50%
* 确保至少保留30%可用余额
* 高风险标的降低权重，低风险标的可提高

**第四层：优先级排序**

* High: 强趋势+高时机分+低风险 → 优先配置
* Medium: 中等信号+可控风险 → 次优配置
* Low: 弱信号或高风险 → 最低配置
* Skip: 信号矛盾或风险过高 → 本轮跳过

---

## 输出要求

严格返回JSON格式：

{
  "allocations": [
    {
      "symbol": "BTCUSDT",
      "allocated_balance": 500.0,
      "weight": 0.5,
      "priority": "high"|"medium"|"low"|"skip",
      "max_amount_override": 0.003
    }
  ],
  "total_available": 1000.0,
  "strategy": "balanced"|"aggressive"|"conservative",
  "reasoning": "配置理由，80字内"
}

**禁止输出任何JSON之外的内容。**"#
}

fn build_portfolio_coordinator_prompt(
    total_balance: f64,
    portfolio_strategy: &str,
    reports: &[(String, MarketReport)],
) -> Result<String> {
    let simplified: Vec<_> = reports
        .iter()
        .map(|(symbol, report)| {
            json!({
                "symbol": symbol,
                "market_report": report,
            })
        })
        .collect();

    let payload = json!({
        "total_available": total_balance,
        "strategy_mode": portfolio_strategy,
        "reports": simplified,
    });

    structured_prompt(
        "以下是每个标的的结构化行情摘要，请制定资金分配方案。",
        &payload,
        r#"{
  "allocations": [
    {"symbol": "BTCUSDT", "allocated_balance": 0.0, "weight": 0.0, "priority": "high"|"medium"|"low"|"skip", "max_amount_override": 0.0|null }
  ],
  "total_available": 数字,
  "strategy": "balanced" | "aggressive" | "conservative",
  "reasoning": "<=80字"
}"#,
    )
}

pub async fn portfolio_coordinator_allocate(
    symbols_reports: &[(String, MarketReport)],
    total_balance: f64,
    portfolio_strategy: &str,
    api_key: &str,
) -> Result<PortfolioAllocation> {
    let prompt =
        build_portfolio_coordinator_prompt(total_balance, portfolio_strategy, symbols_reports)?;
    llm_client::chat_json(get_portfolio_coordinator_system_prompt(), &prompt, api_key).await
}
