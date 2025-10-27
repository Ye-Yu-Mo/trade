use crate::executor::AccountInfo;
use crate::types::{Kline, Position, TechnicalIndicators, TradingDecision};
use anyhow::{Context, Result};
use async_openai::{
    config::OpenAIConfig,
    types::{ChatCompletionRequestMessage, ChatCompletionRequestSystemMessageArgs, ChatCompletionRequestUserMessageArgs, CreateChatCompletionRequestArgs},
    Client,
};

// System Prompt - 激进交易策略
fn get_system_prompt() -> &'static str {
    r#"你是激进的加密货币交易员，专注于捕捉市场机会：

## 核心哲学
1. **机会稍纵即逝** - 趋势启动时必须果断进场，犹豫就会错过
2. **动量为王** - 价格动量是最强信号，顺势而为才能获利
3. **主动交易** - 市场是用来交易的，不是用来观望的
4. **敢于试错** - 小亏损可以接受，错过大行情才是真正的损失

## 分析框架
**趋势识别**: 快速判断多空方向，优先跟随主趋势
**动量捕捉**: 价格加速、均线发散是最强入场信号
**量能确认**: 放量突破果断跟进，缩量整理准备入场
**快进快出**: 持仓不追求完美，有利润就是好交易

## 输出要求
严格返回JSON格式，包含：
- signal: "BUY"(看多开多/平空) | "SELL"(看空开空/平多) | "HOLD"(仅在极度矛盾时使用)
- reason: 核心分析逻辑（50字内，强调动量和趋势）
- confidence: "HIGH"(明确信号) | "MEDIUM"(可交易信号) | "LOW"(信号较弱但可尝试)

交易原则：宁愿多做错，不要错过。HOLD仅在信号完全矛盾时使用。

禁止输出任何JSON之外的内容。"#
}

// Task 4.1: 构建提示词
fn build_prompt(
    klines: &[Kline],
    indicators: &TechnicalIndicators,
    position: &Option<Position>,
    account: &AccountInfo,
    min_amount: f64,
    max_amount: f64,
) -> String {
    // 最近5根K线（从旧到新）
    let recent_klines: Vec<&Kline> = klines.iter().rev().take(5).rev().collect();

    // 判断K线形态
    let last_kline = recent_klines.last().unwrap();
    let is_bullish = last_kline.close > last_kline.open;
    let body_size = (last_kline.close - last_kline.open).abs();
    let range = last_kline.high - last_kline.low;
    let body_ratio = if range > 0.0 { body_size / range } else { 0.0 };

    let kline_summary = format!(
        "K线形态: {} (实体占比{:.1}%), 收盘价{:.2}",
        if is_bullish { "阳线" } else { "阴线" },
        body_ratio * 100.0,
        last_kline.close
    );

    // 均线状态
    let ma_trend = if indicators.sma_5 > indicators.sma_20 {
        "多头排列(短期强势)"
    } else {
        "空头排列(短期弱势)"
    };

    let price_vs_ma5 = ((last_kline.close - indicators.sma_5) / indicators.sma_5) * 100.0;
    let price_vs_ma20 = ((last_kline.close - indicators.sma_20) / indicators.sma_20) * 100.0;

    // 动量与成交量
    let momentum_status = if indicators.price_change_1 > 0.5 {
        "短期上涨加速"
    } else if indicators.price_change_1 < -0.5 {
        "短期下跌加速"
    } else {
        "震荡整理"
    };

    let volume_status = if indicators.volume_ratio > 1.5 {
        "放量(信号可信度高)"
    } else if indicators.volume_ratio < 0.7 {
        "缩量(观望情绪)"
    } else {
        "正常量能"
    };

    // 持仓状态
    let position_risk = match position {
        None => "空仓，可开仓".to_string(),
        Some(pos) => {
            let risk_desc = if pos.unrealized_pnl > 0.0 {
                "盈利中"
            } else if pos.unrealized_pnl < -10.0 {
                "亏损较大，考虑止损"
            } else {
                "微亏"
            };
            format!(
                "{:?}仓持仓{:.4}，开仓价{:.2}，盈亏{:.2}U ({})",
                pos.side, pos.amount, pos.entry_price, pos.unrealized_pnl, risk_desc
            )
        }
    };

    format!(
        r#"## 市场数据分析

**K线**: {}
**均线**: {} | 价格相对MA5: {:+.2}%, 相对MA20: {:+.2}%
**动量**: {} | 1周期{:+.2}%, 3周期{:+.2}%
**成交量**: {}
**持仓**: {}
**账户**: 可用余额 {} USDT
**交易限制**: 单次最小 {:.4}, 最大 {:.4}

## 任务
基于以上数据，以激进交易员的视角给出决策：
- 有动量信号（价格变化>±0.3%或均线发散）→ 立即BUY/SELL
- 多头排列且非大幅回调 → 偏向BUY
- 空头排列且非大幅反弹 → 偏向SELL
- **支持加仓**: 已持有多仓时可继续BUY加多，已持有空仓时可继续SELL加空
- 趋势强劲时果断加仓，弱势时观望或反向操作
- 账户余额充足时更应主动交易，余额不足时谨慎观望
- 仅在信号完全矛盾或余额不足时 → HOLD

**交易数量决策**:
- 你需要根据信号强度和账户余额决定交易数量(amount)
- 信号强(HIGH) → 用接近最大值
- 信号中(MEDIUM) → 用中等仓位
- 信号弱(LOW) → 用最小值或接近最小值
- 必须在 [{:.4}, {:.4}] 范围内

记住：宁愿多交易，不要错过机会。小波动也可以是入场信号。支持加仓放大收益。

严格返回JSON:
{{"signal": "BUY"|"SELL"|"HOLD", "amount": 0.001, "reason": "核心逻辑", "confidence": "HIGH"|"MEDIUM"|"LOW"}}"#,
        kline_summary,
        ma_trend,
        price_vs_ma5,
        price_vs_ma20,
        momentum_status,
        indicators.price_change_1,
        indicators.price_change_3,
        volume_status,
        position_risk,
        account.availableBalance,
        min_amount,
        max_amount,
        min_amount,
        max_amount
    )
}

// Task 4.2: 调用DeepSeek并解析决策
pub async fn analyze(
    klines: &[Kline],
    indicators: &TechnicalIndicators,
    position: &Option<Position>,
    account: &AccountInfo,
    min_amount: f64,
    max_amount: f64,
    api_key: &str,
) -> Result<TradingDecision> {
    let prompt = build_prompt(klines, indicators, position, account, min_amount, max_amount);

    let config = OpenAIConfig::new()
        .with_api_key(api_key)
        .with_api_base("https://api.deepseek.com");

    let client = Client::with_config(config);

    let request = CreateChatCompletionRequestArgs::default()
        .model("deepseek-chat")
        .messages(vec![
            ChatCompletionRequestMessage::System(
                ChatCompletionRequestSystemMessageArgs::default()
                    .content(get_system_prompt())
                    .build()?
            ),
            ChatCompletionRequestMessage::User(
                ChatCompletionRequestUserMessageArgs::default()
                    .content(prompt)
                    .build()?
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

    // 提取JSON（处理markdown代码块）
    let json_start = content.find('{').context("未找到JSON起始")?;
    let json_end = content.rfind('}').context("未找到JSON结束")? + 1;
    let json_str = &content[json_start..json_end];

    serde_json::from_str(json_str).context("解析决策JSON失败")
}
