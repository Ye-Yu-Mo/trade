// 多智能体交易决策系统

use crate::executor::AccountInfo;
use crate::types::*;
use anyhow::{Context, Result};
use async_openai::{
    config::OpenAIConfig,
    types::{
        ChatCompletionRequestMessage, ChatCompletionRequestSystemMessageArgs,
        ChatCompletionRequestUserMessageArgs, CreateChatCompletionRequestArgs,
    },
    Client,
};

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

fn build_market_analyst_prompt(klines: &[Kline], indicators: &TechnicalIndicators) -> String {
    let last_kline = klines.last().unwrap();
    let is_bullish = last_kline.close > last_kline.open;

    // 计算K线实体和影线
    let body_size = (last_kline.close - last_kline.open).abs();
    let upper_shadow = last_kline.high - last_kline.close.max(last_kline.open);
    let lower_shadow = last_kline.close.min(last_kline.open) - last_kline.low;
    let total_range = last_kline.high - last_kline.low;
    let body_ratio = if total_range > 0.0 { (body_size / total_range) * 100.0 } else { 0.0 };

    // 最近5根K线详情
    let recent_5: Vec<&Kline> = klines.iter().rev().take(5).rev().collect();
    let mut kline_details = String::new();
    for (i, k) in recent_5.iter().enumerate() {
        let k_type = if k.close > k.open { "阳" } else { "阴" };
        let change = ((k.close - k.open) / k.open) * 100.0;
        kline_details.push_str(&format!(
            "\n  K{}: {} 开{:.2} 高{:.2} 低{:.2} 收{:.2} ({:+.2}%) 量{:.0}",
            i + 1, k_type, k.open, k.high, k.low, k.close, change, k.volume
        ));
    }

    // 近期高低点（最近10根）
    let recent_10: Vec<&Kline> = klines.iter().rev().take(10).rev().collect();
    let recent_high = recent_10.iter().map(|k| k.high).fold(f64::MIN, f64::max);
    let recent_low = recent_10.iter().map(|k| k.low).fold(f64::MAX, f64::min);
    let price_position = ((last_kline.close - recent_low) / (recent_high - recent_low)) * 100.0;

    // 均线状态
    let ma_trend = if indicators.sma_5 > indicators.sma_20 {
        "多头排列"
    } else {
        "空头排列"
    };
    let price_vs_ma5 = ((last_kline.close - indicators.sma_5) / indicators.sma_5) * 100.0;
    let price_vs_ma20 = ((last_kline.close - indicators.sma_20) / indicators.sma_20) * 100.0;
    let ma_divergence = ((indicators.sma_5 - indicators.sma_20) / indicators.sma_20) * 100.0;

    // 动量状态
    let momentum = if indicators.price_change_1 > 1.0 {
        "强势上涨"
    } else if indicators.price_change_1 > 0.3 {
        "温和上涨"
    } else if indicators.price_change_1 < -1.0 {
        "强势下跌"
    } else if indicators.price_change_1 < -0.3 {
        "温和下跌"
    } else {
        "窄幅震荡"
    };

    // 成交量状态
    let volume_status = if indicators.volume_ratio > 2.0 {
        "异常放量"
    } else if indicators.volume_ratio > 1.5 {
        "明显放量"
    } else if indicators.volume_ratio < 0.5 {
        "明显缩量"
    } else {
        "正常量能"
    };

    // 连续涨跌统计
    let mut consecutive_up = 0;
    let mut consecutive_down = 0;
    for k in recent_5.iter().rev() {
        if k.close > k.open {
            consecutive_up += 1;
            break;
        }
    }
    for k in recent_5.iter().rev() {
        if k.close < k.open {
            consecutive_down += 1;
            break;
        }
    }

    format!(
        r#"## 市场数据全景

### 1. 当前K线详情
**最新价格**: {:.2}
**K线形态**: {} (实体占比 {:.1}%)
  - 开盘: {:.2}
  - 最高: {:.2}
  - 最低: {:.2}
  - 收盘: {:.2}
  - 上影线: {:.2} / 下影线: {:.2}
**涨跌幅**: {:+.2}%
**成交量**: {:.0}

### 2. 近期K线走势（最近5根）
{:}

### 3. 价格位置分析
**近期高点**: {:.2} (10周期内)
**近期低点**: {:.2} (10周期内)
**当前位置**: {:.1}% (0%=低点, 100%=高点)
**价格区间**: {:.2}

### 4. 均线系统
**MA5**: {:.2} | 价格偏离 {:+.2}%
**MA20**: {:.2} | 价格偏离 {:+.2}%
**均线状态**: {} (MA5-MA20偏离 {:+.2}%)
**价格相对MA5**: {}
**价格相对MA20**: {}

### 5. 动量与趋势
**1周期动量**: {:+.2}% ({})
**3周期动量**: {:+.2}%
**短期趋势**: {}

### 6. 成交量分析
**当前成交量**: {:.0}
**20周期均量**: {:.0}
**量比**: {:.2}
**量能状态**: {}

### 7. 形态识别
**连续阳线**: {} 根
**连续阴线**: {} 根
**K线实体**: {} (实体占比 {:.1}%)

## 任务
基于以上完整的市场数据，进行深度分析：
1. 识别当前市场处于哪个阶段（积累/上升/分配/下跌）
2. 判断趋势方向和强度
3. 确定关键支撑位和压力位
4. 给出核心市场判断

严格返回JSON:
{{"trend": "bullish"|"bearish"|"neutral", "strength": "strong"|"medium"|"weak", "market_phase": "accumulation"|"markup"|"distribution"|"markdown", "support": {:.2}, "resistance": {:.2}, "analysis": "核心判断"}}"#,
        last_kline.close,
        if is_bullish { "阳线" } else { "阴线" },
        body_ratio,
        last_kline.open,
        last_kline.high,
        last_kline.low,
        last_kline.close,
        upper_shadow,
        lower_shadow,
        ((last_kline.close - last_kline.open) / last_kline.open) * 100.0,
        last_kline.volume,
        kline_details,
        recent_high,
        recent_low,
        price_position,
        recent_high - recent_low,
        indicators.sma_5,
        price_vs_ma5,
        indicators.sma_20,
        price_vs_ma20,
        ma_trend,
        ma_divergence,
        if price_vs_ma5 > 0.0 { "上方" } else { "下方" },
        if price_vs_ma20 > 0.0 { "上方" } else { "下方" },
        indicators.price_change_1,
        momentum,
        indicators.price_change_3,
        if indicators.price_change_3 > 0.0 { "上升趋势" } else { "下降趋势" },
        last_kline.volume,
        last_kline.volume / indicators.volume_ratio,
        indicators.volume_ratio,
        volume_status,
        if consecutive_up > 0 { consecutive_up } else { 0 },
        if consecutive_down > 0 { consecutive_down } else { 0 },
        if is_bullish { "阳线主导" } else { "阴线主导" },
        body_ratio,
        recent_low,
        recent_high
    )
}

pub async fn market_analyst_analyze(
    klines: &[Kline],
    indicators: &TechnicalIndicators,
    api_key: &str,
) -> Result<MarketReport> {
    let prompt = build_market_analyst_prompt(klines, indicators);
    let response = call_deepseek(get_market_analyst_system_prompt(), &prompt, api_key).await?;
    parse_json_response(&response)
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

---

## 输出要求

严格返回JSON格式：

{
  "action": "open_long"|"open_short"|"add_position"|"close_position"|"hold",
  "reasoning": "策略逻辑，50字内",
  "timing_score": 8,
  "target_side": "Long"|"Short"|null
}

**禁止输出任何JSON之外的内容。**"#
}

fn build_strategy_researcher_prompt(
    market_report: &MarketReport,
    position: &Option<Position>,
) -> String {
    // 持仓详情
    let (position_status, position_detail, position_risk) = match position {
        None => (
            "空仓".to_string(),
            "当前无持仓，可以自由选择方向".to_string(),
            "无持仓风险".to_string(),
        ),
        Some(pos) => {
            let side_str = format!("{:?}", pos.side);
            let pnl_pct = (pos.unrealized_pnl / (pos.entry_price * pos.amount)) * 100.0;
            let risk_level = if pos.unrealized_pnl > 10.0 {
                "盈利可观，可考虑止盈或加仓"
            } else if pos.unrealized_pnl > 0.0 {
                "小幅盈利，趋势确认可加仓"
            } else if pos.unrealized_pnl > -10.0 {
                "小幅亏损，需确认趋势是否反转"
            } else {
                "亏损较大，建议考虑止损"
            };

            (
                format!("{}仓持仓中", side_str),
                format!(
                    "持仓方向: {}\n  持仓数量: {:.4}\n  开仓价格: {:.2}\n  浮动盈亏: {:.2} USDT ({:+.2}%)",
                    side_str, pos.amount, pos.entry_price, pos.unrealized_pnl, pnl_pct
                ),
                risk_level.to_string(),
            )
        }
    };

    // 趋势与持仓一致性判断
    let trend_position_alignment = match position {
        None => "空仓状态，可根据趋势开仓".to_string(),
        Some(pos) => {
            let is_long = matches!(pos.side, crate::types::PositionSide::Long);
            let is_bullish = matches!(market_report.trend, crate::types::TrendDirection::Bullish);

            if (is_long && is_bullish) || (!is_long && !is_bullish) {
                "持仓方向与趋势一致，可考虑持有或加仓".to_string()
            } else if is_bullish && !is_long {
                "持有空仓但趋势看多，建议平仓或反向开多".to_string()
            } else if !is_bullish && is_long {
                "持有多仓但趋势看空，建议平仓或反向开空".to_string()
            } else {
                "趋势中性，建议根据市场阶段决策".to_string()
            }
        }
    };

    // 市场阶段建议
    let phase_suggestion = match market_report.market_phase {
        crate::types::MarketPhase::Accumulation => "积累阶段，适合低位布局，等待突破",
        crate::types::MarketPhase::Markup => "上升阶段，趋势强劲，适合顺势做多或加仓",
        crate::types::MarketPhase::Distribution => "分配阶段，高位震荡，建议减仓或观望",
        crate::types::MarketPhase::Markdown => "下跌阶段，趋势向下，适合做空或空仓观望",
    };

    // 趋势强度建议
    let strength_suggestion = match market_report.strength {
        crate::types::TrendStrength::Strong => "趋势强劲，时机成熟，建议果断执行",
        crate::types::TrendStrength::Medium => "趋势中等，可交易但需谨慎，控制仓位",
        crate::types::TrendStrength::Weak => "趋势较弱，信号不明确，建议观望或轻仓试探",
    };

    format!(
        r#"## 行情分析员完整报告

### 市场趋势研判
**趋势方向**: {:?} ({}方向)
**趋势强度**: {:?}
**强度评估**: {}

### 市场阶段识别
**当前阶段**: {:?}
**阶段特征**: {}

### 技术分析核心
**核心判断**: {}
**支撑位**: {:.2}
**压力位**: {:.2}
**关键区间**: {:.2} (压力-支撑差距)

---

## 当前持仓状态

### 持仓概况
**状态**: {}
**详情**:
{}

### 风险评估
**持仓风险**: {}

### 趋势一致性
**趋势与持仓**: {}

---

## 策略决策依据

### 市场环境
1. **趋势环境**: {:?} + {:?} = {}
2. **市场阶段**: {:?} → {}
3. **价格位置**: 支撑 {:.2} | 压力 {:.2}

### 操作逻辑参考
- **开多条件**: 趋势bullish + 阶段accumulation/markup + 价格接近支撑
- **加多条件**: 已持Long + 趋势bullish + 强度strong/medium
- **平多条件**: 已持Long + 趋势bearish/neutral + 价格接近压力
- **开空条件**: 趋势bearish + 阶段distribution/markdown + 价格接近压力
- **加空条件**: 已持Short + 趋势bearish + 强度strong/medium
- **平空条件**: 已持Short + 趋势bullish + 价格接近支撑
- **观望条件**: 趋势neutral + 强度weak + 信号矛盾

---

## 任务
基于以上完整的市场分析和持仓状态，制定交易策略：
1. 判断当前应该采取什么操作（开仓/加仓/平仓/持有）
2. 给出清晰的策略逻辑
3. 评估时机成熟度（1-10分）
4. 明确目标持仓方向

严格返回JSON:
{{"action": "open_long"|"open_short"|"add_position"|"close_position"|"hold", "reasoning": "策略逻辑", "timing_score": 7, "target_side": "Long"|"Short"|null}}"#,
        market_report.trend,
        match market_report.trend {
            crate::types::TrendDirection::Bullish => "看多",
            crate::types::TrendDirection::Bearish => "看空",
            crate::types::TrendDirection::Neutral => "中性",
        },
        market_report.strength,
        strength_suggestion,
        market_report.market_phase,
        phase_suggestion,
        market_report.analysis,
        market_report.support,
        market_report.resistance,
        market_report.resistance - market_report.support,
        position_status,
        position_detail,
        position_risk,
        trend_position_alignment,
        market_report.trend,
        market_report.strength,
        strength_suggestion,
        market_report.market_phase,
        phase_suggestion,
        market_report.support,
        market_report.resistance,
    )
}

pub async fn strategy_researcher_suggest(
    market_report: &MarketReport,
    position: &Option<Position>,
    api_key: &str,
) -> Result<StrategyAdvice> {
    let prompt = build_strategy_researcher_prompt(market_report, position);
    let response = call_deepseek(get_strategy_researcher_system_prompt(), &prompt, api_key).await?;
    parse_json_response(&response)
}

// ========== 3. 风险管理员 (Risk Manager) ==========

fn get_risk_manager_system_prompt() -> &'static str {
    r#"## 角色定义

你是一位 **加密货币风险管理员（Risk Manager）**，曾在投资银行风控部门工作多年，见证过无数因忽视风险而爆仓的案例。你的职责不是帮助交易员赚钱，而是**确保他们能活着看到明天的太阳**。

你是团队中最不受欢迎的人，因为你总是说"不"。但你也是最重要的人，因为你是最后一道防线。

你的职责是**评估每一笔交易的风险敞口，在贪婪与理性之间划出红线，并保持对市场的敬畏**。

---

## 我的核心哲学

**1. "先活下来，再谈盈利" — 我的生存法则**

> "控制风险的能力，比预测方向的能力重要100倍。"

* 没有止损的信念，叫做幻想。
* 盈利是奖励，但生存是前提。
* 市场会原谅你的无知，但不会原谅你的贪婪。

---

**2. "聪明人死于杠杆，天才死于自信" — 我的警示箴言**

> "永远给市场留下犯错的空间。"

* LTCM的天才们用一个公式证明：智商与生存能力无关。
* 杠杆是放大器，它放大收益，更放大人性的弱点。
* 市场不关心你有多聪明，只关心你能承受多少痛苦。

---

**3. "规则是铁律，不是建议" — 我的工作准则**

> "风控不是协商，而是底线。"

* 风险限额不是用来突破的，而是用来服从的。
* 每一次"这次不一样"，都是下一次爆仓的序幕。
* 我的职责不是让你开心，而是让你安全。

---

**4. "恐惧是理性的另一个名字" — 我的情绪管理**

> "当所有人都勇敢时，我选择恐惧。"

* 市场最危险的时刻，是所有人都觉得安全的时刻。
* 恐惧让我谨慎，谨慎让我生存，生存让我盈利。
* 优秀的风控不是消除风险，而是确保风险可控。

---

## 风险评估框架

**第一层：账户风险检查**

* 可用余额是否充足？（最少保留30%缓冲）
* 单次交易占总资金比例是否过大？（建议<5%）
* 是否会超过最大持仓限制？
* 当前杠杆倍数下，能承受多大回撤？

**第二层：市场风险评估**

* 趋势强度是否足够支撑这个操作？
* 时机评分是否达到可操作标准？（建议≥6分）
* 当前波动率是否异常？
* 是否存在突发事件风险（政策、黑天鹅）？

**第三层：策略风险验证**

* 策略逻辑是否清晰？
* 操作与当前仓位是否冲突？
* 是否存在过度交易倾向？
* 止损机制是否明确？

**第四层：审批决策**

* approved: 完全批准，风险可控
* adjusted: 调整交易数量后批准，降低敞口
* rejected: 拒绝交易，风险过高或逻辑不清

---

## 输出要求

严格返回JSON格式：

{
  "risk_level": "low"|"medium"|"high",
  "suggested_amount": 0.001,
  "approval": "approved"|"adjusted"|"rejected",
  "warnings": ["风险点1", "风险点2"],
  "reason": "风险评估理由，50字内"
}

**禁止输出任何JSON之外的内容。**"#
}

fn build_risk_manager_prompt(
    market_report: &MarketReport,
    strategy: &StrategyAdvice,
    account: &AccountInfo,
    position: &Option<Position>,
    min_amount: f64,
    max_amount: f64,
    max_position: f64,
) -> String {
    // 解析账户余额
    let available_balance: f64 = account.availableBalance.parse().unwrap_or(0.0);
    let total_balance: f64 = account.totalWalletBalance.parse().unwrap_or(0.0);
    let used_margin = total_balance - available_balance;

    // 持仓分析
    let (current_position_amount, position_value, position_pnl, position_direction) = match position {
        None => (0.0, 0.0, 0.0, "无持仓".to_string()),
        Some(pos) => {
            let value = pos.amount * pos.entry_price;
            (pos.amount, value, pos.unrealized_pnl, format!("{:?}仓", pos.side))
        }
    };

    // 计算各种风险指标
    let max_trade_value = max_amount * market_report.support.max(market_report.resistance);
    let position_utilization = if max_position > 0.0 {
        (current_position_amount / max_position) * 100.0
    } else {
        0.0
    };

    let trade_to_balance_ratio = if available_balance > 0.0 {
        (max_trade_value / available_balance) * 100.0
    } else {
        999.9
    };

    // 策略操作风险分析
    let action_risk = match strategy.action {
        crate::types::StrategyAction::OpenLong | crate::types::StrategyAction::OpenShort => {
            "新开仓位，风险可控，但需确认信号强度"
        },
        crate::types::StrategyAction::AddPosition => {
            "加仓操作，会增加风险敞口，需谨慎评估"
        },
        crate::types::StrategyAction::ClosePosition => {
            "平仓操作，降低风险，通常应批准"
        },
        crate::types::StrategyAction::Hold => {
            "观望操作，无新增风险"
        },
    };

    // 趋势强度风险
    let trend_risk = match market_report.strength {
        crate::types::TrendStrength::Strong => "趋势强劲，操作风险较低",
        crate::types::TrendStrength::Medium => "趋势中等，需适当控制仓位",
        crate::types::TrendStrength::Weak => "趋势较弱，建议减小仓位或观望",
    };

    // 时机评分风险
    let timing_risk = if strategy.timing_score >= 8 {
        "时机成熟，可以执行"
    } else if strategy.timing_score >= 6 {
        "时机尚可，建议谨慎控制仓位"
    } else {
        "时机不佳，建议观望或拒绝"
    };

    // 潜在风险点识别
    let mut auto_warnings = Vec::new();

    if available_balance < 100.0 {
        auto_warnings.push("可用余额不足100 USDT，建议谨慎交易");
    }

    if position_utilization > 80.0 {
        auto_warnings.push("持仓已接近上限，建议拒绝加仓");
    }

    if trade_to_balance_ratio > 10.0 {
        auto_warnings.push("单次交易占比过大，建议调整数量");
    }

    if strategy.timing_score < 5 {
        auto_warnings.push("时机评分过低，建议拒绝交易");
    }

    if matches!(market_report.strength, crate::types::TrendStrength::Weak) {
        auto_warnings.push("趋势较弱，信号不明确");
    }

    let warnings_hint = if auto_warnings.is_empty() {
        "暂无明显风险点".to_string()
    } else {
        auto_warnings.join("; ")
    };

    // 建议仓位计算
    let suggested_base = if strategy.timing_score >= 8 && matches!(market_report.strength, crate::types::TrendStrength::Strong) {
        max_amount * 0.8  // 强信号用80%最大仓位
    } else if strategy.timing_score >= 6 {
        (min_amount + max_amount) / 2.0  // 中等信号用中等仓位
    } else {
        min_amount  // 弱信号用最小仓位
    };

    format!(
        r#"## 行情分析员报告

### 市场环境
**趋势**: {:?} ({:?})
**市场阶段**: {:?}
**核心分析**: {}
**支撑位**: {:.2} USDT
**压力位**: {:.2} USDT

### 趋势风险评估
{}

---

## 策略研究员建议

### 操作建议
**建议操作**: {:?}
**策略逻辑**: {}
**时机评分**: {}/10
**目标方向**: {}

### 策略风险分析
**操作类型风险**: {}
**时机风险**: {}

---

## 账户状态详情

### 资金状况
**总余额**: {} USDT
**可用余额**: {} USDT
**已用保证金**: {:.2} USDT
**资金使用率**: {:.1}%

### 持仓状况
**当前持仓方向**: {}
**持仓数量**: {:.4}
**持仓价值**: {:.2} USDT
**浮动盈亏**: {:.2} USDT
**持仓利用率**: {:.1}% (当前/最大 {:.4}/{:.4})

### 风险限制
**单次最小交易**: {:.4}
**单次最大交易**: {:.4}
**最大持仓限制**: {:.4}
**理论最大交易价值**: {:.2} USDT
**交易占余额比**: {:.1}%

---

## 风险评估矩阵

### 自动识别的风险点
{}

### 关键风险指标
1. **账户安全**: 可用余额 {} USDT ({}建议≥100)
2. **仓位风险**: 当前 {:.4} / 最大 {:.4} ({}建议<80%)
3. **单笔风险**: 最大交易占比 {:.1}% ({}建议<10%)
4. **时机风险**: 评分 {}/10 ({}建议≥6)
5. **趋势风险**: {:?} ({}确定性要求)

### 建议仓位参考
**基础建议**: {:.4} (根据信号强度计算)
**最小允许**: {:.4}
**最大允许**: {:.4}

---

## 任务
作为风险管理员，你需要：
1. 综合评估所有风险因素（账户、市场、策略）
2. 决定是否批准这笔交易（approved/adjusted/rejected）
3. 如果批准，给出建议的交易数量（必须在 [{:.4}, {:.4}] 范围内）
4. 列出所有需要注意的风险警告
5. 给出简明的风险评估理由

**决策标准**:
- rejected: 余额不足、时机差(<5分)、持仓超限、信号矛盾
- adjusted: 信号可行但需减小仓位、趋势中等需谨慎
- approved: 信号强、风险低、账户安全、时机成熟

严格返回JSON:
{{"risk_level": "low"|"medium"|"high", "suggested_amount": {:.4}, "approval": "approved"|"adjusted"|"rejected", "warnings": ["风险点1", "风险点2"], "reason": "评估理由"}}"#,
        market_report.trend,
        market_report.strength,
        market_report.market_phase,
        market_report.analysis,
        market_report.support,
        market_report.resistance,
        trend_risk,
        strategy.action,
        strategy.reasoning,
        strategy.timing_score,
        match &strategy.target_side {
            Some(side) => format!("{:?}", side),
            None => "无".to_string(),
        },
        action_risk,
        timing_risk,
        account.totalWalletBalance,
        account.availableBalance,
        used_margin,
        (used_margin / total_balance) * 100.0,
        position_direction,
        current_position_amount,
        position_value,
        position_pnl,
        position_utilization,
        current_position_amount,
        max_position,
        min_amount,
        max_amount,
        max_position,
        max_trade_value,
        trade_to_balance_ratio,
        warnings_hint,
        account.availableBalance,
        if available_balance >= 100.0 { "✓" } else { "✗" },
        current_position_amount,
        max_position,
        if position_utilization < 80.0 { "✓" } else { "✗" },
        trade_to_balance_ratio,
        if trade_to_balance_ratio < 10.0 { "✓" } else { "✗" },
        strategy.timing_score,
        if strategy.timing_score >= 6 { "✓" } else { "✗" },
        market_report.strength,
        if matches!(market_report.strength, crate::types::TrendStrength::Strong | crate::types::TrendStrength::Medium) { "✓" } else { "✗" },
        suggested_base,
        min_amount,
        max_amount,
        min_amount,
        max_amount,
        suggested_base,
    )
}

pub async fn risk_manager_assess(
    market_report: &MarketReport,
    strategy: &StrategyAdvice,
    account: &AccountInfo,
    position: &Option<Position>,
    min_amount: f64,
    max_amount: f64,
    max_position: f64,
    api_key: &str,
) -> Result<RiskAssessment> {
    let prompt = build_risk_manager_prompt(
        market_report,
        strategy,
        account,
        position,
        min_amount,
        max_amount,
        max_position,
    );
    let response = call_deepseek(get_risk_manager_system_prompt(), &prompt, api_key).await?;
    parse_json_response(&response)
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
* **HOLD信号**：风控rejected / 信号矛盾 / 趋势不明

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
  "signal": "BUY"|"SELL"|"HOLD",
  "amount": 0.001,
  "confidence": "HIGH"|"MEDIUM"|"LOW",
  "reason": "综合判断，50字内"
}

**禁止输出任何JSON之外的内容。**"#
}

fn build_trade_executor_prompt(
    market_report: &MarketReport,
    strategy: &StrategyAdvice,
    risk: &RiskAssessment,
) -> String {
    // 处理警告信息
    let warnings_str = if risk.warnings.is_empty() {
        "无风险警告".to_string()
    } else {
        format!("\n{}", risk.warnings.iter()
            .enumerate()
            .map(|(i, w)| format!("  {}. {}", i + 1, w))
            .collect::<Vec<_>>()
            .join("\n"))
    };

    // 信号一致性分析
    let signal_consistency = {
        let trend_direction = match market_report.trend {
            crate::types::TrendDirection::Bullish => "看多",
            crate::types::TrendDirection::Bearish => "看空",
            crate::types::TrendDirection::Neutral => "中性",
        };

        let strategy_direction = match strategy.action {
            crate::types::StrategyAction::OpenLong | crate::types::StrategyAction::AddPosition => "建议做多",
            crate::types::StrategyAction::OpenShort => "建议做空",
            crate::types::StrategyAction::ClosePosition => "建议平仓",
            crate::types::StrategyAction::Hold => "建议观望",
        };

        let risk_approval_str = match risk.approval {
            crate::types::ApprovalStatus::Approved => "完全批准",
            crate::types::ApprovalStatus::Adjusted => "调整后批准",
            crate::types::ApprovalStatus::Rejected => "拒绝执行",
        };

        format!(
            "行情: {} | 策略: {} | 风控: {}",
            trend_direction, strategy_direction, risk_approval_str
        )
    };

    // 一致性评分
    let consistency_score = {
        let trend_matches = match (&market_report.trend, &strategy.action) {
            (crate::types::TrendDirection::Bullish, crate::types::StrategyAction::OpenLong) => true,
            (crate::types::TrendDirection::Bullish, crate::types::StrategyAction::AddPosition) => true,
            (crate::types::TrendDirection::Bearish, crate::types::StrategyAction::OpenShort) => true,
            _ => false,
        };

        let risk_approved = !matches!(risk.approval, crate::types::ApprovalStatus::Rejected);

        let strong_signal = matches!(market_report.strength, crate::types::TrendStrength::Strong);

        let good_timing = strategy.timing_score >= 7;

        match (trend_matches, risk_approved, strong_signal, good_timing) {
            (true, true, true, true) => "非常一致 (4/4)",
            (true, true, true, false) | (true, true, false, true) => "高度一致 (3/4)",
            (true, true, false, false) | (false, true, true, true) => "部分一致 (2/4)",
            _ => "存在矛盾 (≤1/4)",
        }
    };

    // 建议信心等级
    let suggested_confidence = {
        if matches!(risk.approval, crate::types::ApprovalStatus::Rejected) {
            "建议: LOW (风控拒绝)"
        } else if matches!(market_report.strength, crate::types::TrendStrength::Strong)
            && strategy.timing_score >= 8
            && matches!(risk.approval, crate::types::ApprovalStatus::Approved) {
            "建议: HIGH (信号强劲)"
        } else if strategy.timing_score >= 6 {
            "建议: MEDIUM (信号尚可)"
        } else {
            "建议: LOW (信号较弱)"
        }
    };

    // 决策路径提示
    let decision_path = match risk.approval {
        crate::types::ApprovalStatus::Rejected => {
            "【强制HOLD】风控已拒绝，必须观望".to_string()
        },
        _ => {
            match strategy.action {
                crate::types::StrategyAction::OpenLong | crate::types::StrategyAction::AddPosition => {
                    if matches!(market_report.trend, crate::types::TrendDirection::Bullish) {
                        "【倾向BUY】趋势+策略一致看多，风控已批准".to_string()
                    } else {
                        "【谨慎BUY或HOLD】策略看多但趋势不明确".to_string()
                    }
                },
                crate::types::StrategyAction::OpenShort => {
                    if matches!(market_report.trend, crate::types::TrendDirection::Bearish) {
                        "【倾向SELL】趋势+策略一致看空，风控已批准".to_string()
                    } else {
                        "【谨慎SELL或HOLD】策略看空但趋势不明确".to_string()
                    }
                },
                crate::types::StrategyAction::ClosePosition | crate::types::StrategyAction::Hold => {
                    "【倾向HOLD】策略建议观望或平仓".to_string()
                },
            }
        }
    };

    format!(
        r#"## 三方决策汇总

### 🔍 行情分析员报告
**市场趋势**: {:?} (强度: {:?})
**市场阶段**: {:?}
**技术分析**: {}
**支撑/压力**: {:.2} / {:.2}

**核心结论**: {}方向，{}强度，处于{}阶段

---

### 📊 策略研究员建议
**建议操作**: {:?}
**策略逻辑**: {}
**时机评分**: {}/10
**目标方向**: {}

**核心结论**: {}，时机评分{}分（{}分为合格线）

---

### ⚠️  风险管理员评估
**风险等级**: {:?}
**审批状态**: {:?}
**建议数量**: {:.4}
**风险警告**: {}
**风控理由**: {}

**核心结论**: {}，建议数量 {:.4}

---

## 综合决策分析

### 信号一致性检查
**三方立场**: {}
**一致性评分**: {}
**信心建议**: {}

### 决策路径提示
{}

---

## 最终任务
作为决策交易员，你需要：

1. **强制规则**（一票否决）:
   - 如果风控status=rejected → 必须返回signal="HOLD"

2. **决策逻辑**:
   - BUY: 趋势bullish + 策略open_long/add_position + 风控approved/adjusted
   - SELL: 趋势bearish + 策略open_short + 风控approved/adjusted
   - HOLD: 其他所有情况（信号矛盾/趋势不明/风控拒绝）

3. **数量决策**:
   - 优先使用风控建议的 {:.4}
   - 如果HOLD，amount可以是0.0

4. **信心评估**:
   - HIGH: 三方一致 + 强趋势 + 高时机分(≥8) + 风控approved
   - MEDIUM: 两方一致 + 中等趋势 + 时机分≥6
   - LOW: 信号弱或有矛盾或风控rejected

5. **理由总结**:
   - 50字内说明你的决策依据（综合三方意见）

---

严格返回JSON:
{{"signal": "BUY"|"SELL"|"HOLD", "amount": {:.4}, "confidence": "HIGH"|"MEDIUM"|"LOW", "reason": "综合判断50字内"}}"#,
        market_report.trend,
        market_report.strength,
        market_report.market_phase,
        market_report.analysis,
        market_report.support,
        market_report.resistance,
        match market_report.trend {
            crate::types::TrendDirection::Bullish => "看多",
            crate::types::TrendDirection::Bearish => "看空",
            crate::types::TrendDirection::Neutral => "中性",
        },
        match market_report.strength {
            crate::types::TrendStrength::Strong => "强",
            crate::types::TrendStrength::Medium => "中等",
            crate::types::TrendStrength::Weak => "弱",
        },
        match market_report.market_phase {
            crate::types::MarketPhase::Accumulation => "积累",
            crate::types::MarketPhase::Markup => "上升",
            crate::types::MarketPhase::Distribution => "分配",
            crate::types::MarketPhase::Markdown => "下跌",
        },
        strategy.action,
        strategy.reasoning,
        strategy.timing_score,
        match &strategy.target_side {
            Some(side) => format!("{:?}", side),
            None => "无".to_string(),
        },
        match strategy.action {
            crate::types::StrategyAction::OpenLong => "建议开多",
            crate::types::StrategyAction::OpenShort => "建议开空",
            crate::types::StrategyAction::AddPosition => "建议加仓",
            crate::types::StrategyAction::ClosePosition => "建议平仓",
            crate::types::StrategyAction::Hold => "建议观望",
        },
        strategy.timing_score,
        6,
        risk.risk_level,
        risk.approval,
        risk.suggested_amount,
        warnings_str,
        risk.reason,
        match risk.approval {
            crate::types::ApprovalStatus::Approved => "完全批准",
            crate::types::ApprovalStatus::Adjusted => "调整后批准",
            crate::types::ApprovalStatus::Rejected => "拒绝执行",
        },
        risk.suggested_amount,
        signal_consistency,
        consistency_score,
        suggested_confidence,
        decision_path,
        risk.suggested_amount,
        risk.suggested_amount,
    )
}

pub async fn trade_executor_decide(
    market_report: &MarketReport,
    strategy: &StrategyAdvice,
    risk: &RiskAssessment,
    api_key: &str,
) -> Result<TradingDecision> {
    let prompt = build_trade_executor_prompt(market_report, strategy, risk);
    let response = call_deepseek(get_trade_executor_system_prompt(), &prompt, api_key).await?;
    parse_json_response(&response)
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
    symbols_reports: &[(String, MarketReport)],
    total_balance: f64,
    portfolio_strategy: &str,
) -> String {
    let mut reports_summary = String::new();

    for (i, (symbol, report)) in symbols_reports.iter().enumerate() {
        let quality_score = match (report.strength, report.market_phase) {
            (crate::types::TrendStrength::Strong, crate::types::MarketPhase::Markup) => "优质",
            (crate::types::TrendStrength::Strong, _) => "良好",
            (crate::types::TrendStrength::Medium, _) => "中等",
            _ => "较弱",
        };

        reports_summary.push_str(&format!(
            r#"
### 标的{}: {}
**趋势**: {:?} (强度: {:?})
**阶段**: {:?}
**分析**: {}
**支撑/压力**: {:.2} / {:.2}
**信号质量**: {}
"#,
            i + 1,
            symbol,
            report.trend,
            report.strength,
            report.market_phase,
            report.analysis,
            report.support,
            report.resistance,
            quality_score
        ));
    }

    format!(
        r#"## 投资组合概况

**总可用资金**: {:.2} USDT
**标的数量**: {}
**配置策略**: {}

---

## 各标的行情报告

{}

---

## 任务

作为投资组合协调员，你需要：

1. **评估每个标的的机会质量**:
   - 识别强信号标的（趋势strong + 阶段markup/accumulation）
   - 识别中等信号标的（趋势medium或阶段合理）
   - 识别弱信号标的（趋势weak或阶段distribution/markdown）

2. **制定资金分配方案**:
   - Balanced: 平均分配给所有可交易标的
   - Aggressive: 集中80%给强信号，20%给中等信号
   - Conservative: 仅配置强信号，保留50%+现金

3. **设定优先级**:
   - High: 强信号，优先执行
   - Medium: 中等信号，次优执行
   - Low: 弱信号，谨慎执行
   - Skip: 无信号或风险高，跳过

4. **风险控制**:
   - 单一标的权重不超过0.6 (60%)
   - 保留至少30%可用余额
   - 总权重必须≤1.0

5. **可选的max_amount_override**:
   - 如果某标的机会特别好，可以提高其最大交易量
   - 如果某标的风险较高，可以降低其最大交易量

---

严格返回JSON（allocations数组必须包含所有标的）:
{{"allocations": [{{"symbol": "BTCUSDT", "allocated_balance": 300.0, "weight": 0.3, "priority": "high", "max_amount_override": null}}], "total_available": {:.2}, "strategy": "{}", "reasoning": "配置理由"}}"#,
        total_balance,
        symbols_reports.len(),
        portfolio_strategy,
        reports_summary,
        total_balance,
        portfolio_strategy
    )
}

pub async fn portfolio_coordinator_allocate(
    symbols_reports: &[(String, MarketReport)],
    total_balance: f64,
    portfolio_strategy: &str,
    api_key: &str,
) -> Result<crate::types::PortfolioAllocation> {
    let prompt = build_portfolio_coordinator_prompt(symbols_reports, total_balance, portfolio_strategy);
    let response = call_deepseek(get_portfolio_coordinator_system_prompt(), &prompt, api_key).await?;
    parse_json_response(&response)
}

// ========== 通用工具函数 ==========

async fn call_deepseek(system_prompt: &str, user_prompt: &str, api_key: &str) -> Result<String> {
    let config = OpenAIConfig::new()
        .with_api_key(api_key)
        .with_api_base("https://api.deepseek.com");

    let client = Client::with_config(config);

    let request = CreateChatCompletionRequestArgs::default()
        .model("deepseek-chat")
        .messages(vec![
            ChatCompletionRequestMessage::System(
                ChatCompletionRequestSystemMessageArgs::default()
                    .content(system_prompt)
                    .build()?,
            ),
            ChatCompletionRequestMessage::User(
                ChatCompletionRequestUserMessageArgs::default()
                    .content(user_prompt)
                    .build()?,
            ),
        ])
        .build()?;

    let response = client
        .chat()
        .create(request)
        .await
        .context("DeepSeek API 调用失败")?;

    let content = response
        .choices
        .first()
        .and_then(|c| c.message.content.as_ref())
        .context("DeepSeek 返回为空")?;

    Ok(content.clone())
}

fn parse_json_response<T: serde::de::DeserializeOwned>(response: &str) -> Result<T> {
    // 提取JSON（处理markdown代码块）
    let json_start = response.find('{').context("未找到JSON起始")?;
    let json_end = response.rfind('}').context("未找到JSON结束")? + 1;
    let json_str = &response[json_start..json_end];

    serde_json::from_str(json_str).context("解析JSON失败")
}
