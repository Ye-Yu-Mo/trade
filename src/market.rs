use crate::types::{Kline, TechnicalIndicators};
use anyhow::{Context, Result};
use serde::Deserialize;
use std::env;

// 根据环境变量选择 Binance URL
fn get_binance_base_url() -> String {
    match env::var("BINANCE_TESTNET").as_deref() {
        Ok("true") => "https://testnet.binancefuture.com".to_string(),
        _ => "https://fapi.binance.com".to_string(),
    }
}

// Binance API K线响应格式
#[derive(Debug, Deserialize)]
struct BinanceKline(
    i64,   // 开盘时间
    String, // 开盘价
    String, // 最高价
    String, // 最低价
    String, // 收盘价
    String, // 成交量
    i64,   // 收盘时间
    String, // 成交额
    u64,   // 成交笔数
    String, // 主动买入成交量
    String, // 主动买入成交额
    String, // 忽略
);

// Task 3.1: 获取K线数据
pub async fn fetch_klines(symbol: &str, interval: &str, limit: u32) -> Result<Vec<Kline>> {
    let base_url = get_binance_base_url();
    let url = format!(
        "{}/fapi/v1/klines?symbol={}&interval={}&limit={}",
        base_url, symbol, interval, limit
    );

    let mut retries = 0;
    let max_retries = 3;

    loop {
        match reqwest::get(&url).await {
            Ok(response) => {
                let klines: Vec<BinanceKline> = response
                    .json()
                    .await
                    .context("解析K线数据失败")?;

                return Ok(klines
                    .into_iter()
                    .map(|k| Kline {
                        timestamp: k.0,
                        open: k.1.parse().unwrap_or(0.0),
                        high: k.2.parse().unwrap_or(0.0),
                        low: k.3.parse().unwrap_or(0.0),
                        close: k.4.parse().unwrap_or(0.0),
                        volume: k.5.parse().unwrap_or(0.0),
                    })
                    .collect());
            }
            Err(e) => {
                retries += 1;
                if retries >= max_retries {
                    return Err(e).context("获取K线数据失败，已重试3次");
                }
                tokio::time::sleep(tokio::time::Duration::from_secs(2)).await;
            }
        }
    }
}

// Task 3.2: 计算技术指标
pub fn calculate_indicators(klines: &[Kline]) -> Result<TechnicalIndicators> {
    if klines.len() < 20 {
        anyhow::bail!("K线数据不足20根，无法计算指标");
    }

    let closes: Vec<f64> = klines.iter().map(|k| k.close).collect();
    let volumes: Vec<f64> = klines.iter().map(|k| k.volume).collect();

    // SMA(5)
    let sma_5 = closes[closes.len() - 5..].iter().sum::<f64>() / 5.0;

    // SMA(20)
    let sma_20 = closes.iter().sum::<f64>() / closes.len() as f64;

    // 价格变化率
    let last_idx = closes.len() - 1;
    let price_change_1 = (closes[last_idx] - closes[last_idx - 1]) / closes[last_idx - 1] * 100.0;
    let price_change_3 = (closes[last_idx] - closes[last_idx - 3]) / closes[last_idx - 3] * 100.0;

    // 成交量比
    let avg_volume = volumes.iter().sum::<f64>() / volumes.len() as f64;
    let volume_ratio = volumes[last_idx] / avg_volume;

    Ok(TechnicalIndicators {
        sma_5,
        sma_20,
        price_change_1,
        price_change_3,
        volume_ratio,
    })
}

// Task 3.3: 获取当前价格
#[derive(Debug, Deserialize)]
struct PriceResponse {
    price: String,
}

pub async fn fetch_current_price(symbol: &str) -> Result<f64> {
    let base_url = get_binance_base_url();
    let url = format!("{}/fapi/v1/ticker/price?symbol={}", base_url, symbol);

    let response: PriceResponse = reqwest::get(&url)
        .await
        .context("获取价格失败")?
        .json()
        .await
        .context("解析价格数据失败")?;

    response
        .price
        .parse()
        .context("价格字符串转换失败")
}
