use crate::types::{Kline, TechnicalIndicators};
use anyhow::{bail, Context, Result};
use log::warn;
use serde::Deserialize;
use std::env;
use tokio::time::{sleep, Duration};

const MAX_RETRIES: u32 = 3;
const RETRY_DELAY_SECS: u64 = 2;

// 根据环境变量选择 Binance URL
pub(crate) fn get_binance_base_url() -> String {
    match env::var("BINANCE_TESTNET").as_deref() {
        Ok("true") => "https://testnet.binancefuture.com".to_string(),
        _ => "https://fapi.binance.com".to_string(),
    }
}

// Binance API K线响应格式（12 个字段，只使用前 6 个）
#[derive(Debug, Deserialize)]
#[allow(dead_code)]
struct BinanceKline(
    i64,    // 开盘时间
    String, // 开盘价
    String, // 最高价
    String, // 最低价
    String, // 收盘价
    String, // 成交量
    i64,    // 收盘时间
    String, // 成交额
    u64,    // 成交笔数
    String, // 主动买入成交量
    String, // 主动买入成交额
    String, // 忽略
);

impl From<BinanceKline> for Kline {
    fn from(k: BinanceKline) -> Self {
        Kline {
            timestamp: k.0,
            open: k.1.parse().unwrap_or(0.0),
            high: k.2.parse().unwrap_or(0.0),
            low: k.3.parse().unwrap_or(0.0),
            close: k.4.parse().unwrap_or(0.0),
            volume: k.5.parse().unwrap_or(0.0),
        }
    }
}

// 获取最近 limit 根K线
pub async fn fetch_klines(symbol: &str, interval: &str, limit: u32) -> Result<Vec<Kline>> {
    let url = format!(
        "{}/fapi/v1/klines?symbol={}&interval={}&limit={}",
        get_binance_base_url(),
        symbol,
        interval,
        limit
    );
    request_with_retry(&url).await
}

// 按时间区间分页抓取K线，供回测使用
pub(crate) async fn fetch_klines_range(
    symbol: &str,
    interval: &str,
    start_ms: i64,
    end_ms: i64,
    limit: u32,
) -> Result<Vec<Kline>> {
    let url = format!(
        "{}/fapi/v1/klines?symbol={}&interval={}&startTime={}&endTime={}&limit={}",
        get_binance_base_url(),
        symbol,
        interval,
        start_ms,
        end_ms,
        limit
    );
    request_with_retry(&url).await
}

async fn request_once(url: &str) -> Result<Vec<Kline>> {
    let response = reqwest::get(url).await.context("请求K线失败")?;
    let status = response.status();
    if !status.is_success() {
        let body = response.text().await.unwrap_or_default();
        bail!("K线接口返回错误 [{}]: {}", status, body);
    }
    let rows: Vec<BinanceKline> = response.json().await.context("解析K线数据失败")?;
    Ok(rows.into_iter().map(Kline::from).collect())
}

// 网络错误和 HTTP 错误都会重试，最多 MAX_RETRIES 次
async fn request_with_retry(url: &str) -> Result<Vec<Kline>> {
    let mut last_error = None;

    for attempt in 1..=MAX_RETRIES {
        match request_once(url).await {
            Ok(klines) => return Ok(klines),
            Err(err) => {
                warn!(
                    "K线请求失败（第 {}/{} 次）: {:#}",
                    attempt, MAX_RETRIES, err
                );
                last_error = Some(err);
                if attempt < MAX_RETRIES {
                    sleep(Duration::from_secs(RETRY_DELAY_SECS)).await;
                }
            }
        }
    }

    Err(last_error.unwrap_or_else(|| anyhow::anyhow!("获取K线数据失败")))
}

// 计算技术指标
pub fn calculate_indicators(klines: &[Kline]) -> Result<TechnicalIndicators> {
    if klines.len() < 5 {
        bail!("K线数据不足5根，无法计算指标");
    }

    let len = klines.len();
    let closes: Vec<f64> = klines.iter().map(|k| k.close).collect();
    let volumes: Vec<f64> = klines.iter().map(|k| k.volume).collect();

    let avg_last = |window: usize| -> f64 {
        let win = window.min(len);
        closes[len - win..].iter().sum::<f64>() / win as f64
    };

    let pct_change = |periods: usize| -> f64 {
        if len <= periods {
            0.0
        } else {
            let last = closes[len - 1];
            let prev = closes[len - 1 - periods];
            if prev.abs() < f64::EPSILON {
                0.0
            } else {
                (last - prev) / prev * 100.0
            }
        }
    };

    let sma_5 = avg_last(5);
    let sma_20 = avg_last(20);
    let sma_50 = avg_last(50);
    let sma_100 = avg_last(100);

    let price_change_1 = pct_change(1);
    let price_change_3 = pct_change(3);
    let price_change_6 = pct_change(6);
    let price_change_12 = pct_change(12);

    let volume_window = 20.min(len);
    let avg_volume = volumes[len - volume_window..].iter().sum::<f64>() / volume_window as f64;
    let volume_ratio = if avg_volume.abs() < f64::EPSILON {
        0.0
    } else {
        volumes[len - 1] / avg_volume
    };

    let atr_period = 14.min(len.saturating_sub(1));
    let mut atr_sum = 0.0;
    let mut atr_count = 0;
    for i in len - atr_period..len {
        let current = &klines[i];
        let prev_close = if i == 0 {
            current.close
        } else {
            klines[i - 1].close
        };

        let tr = (current.high - current.low)
            .max((current.high - prev_close).abs())
            .max((current.low - prev_close).abs());

        atr_sum += tr;
        atr_count += 1;
    }

    let atr_14 = if atr_count > 0 {
        atr_sum / atr_count as f64
    } else {
        0.0
    };

    let latest_close = closes[len - 1];
    let atr_percent = if latest_close.abs() < f64::EPSILON {
        0.0
    } else {
        atr_14 / latest_close * 100.0
    };

    Ok(TechnicalIndicators {
        sma_5,
        sma_20,
        sma_50,
        sma_100,
        price_change_1,
        price_change_3,
        price_change_6,
        price_change_12,
        atr_14,
        atr_percent,
        volume_ratio,
    })
}

// 获取当前价格
#[derive(Debug, Deserialize)]
struct PriceResponse {
    price: String,
}

pub async fn fetch_current_price(symbol: &str) -> Result<f64> {
    let url = format!(
        "{}/fapi/v1/ticker/price?symbol={}",
        get_binance_base_url(),
        symbol
    );

    let response: PriceResponse = reqwest::get(&url)
        .await
        .context("获取价格失败")?
        .json()
        .await
        .context("解析价格数据失败")?;

    response.price.parse().context("价格字符串转换失败")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn bars(closes: &[f64]) -> Vec<Kline> {
        closes
            .iter()
            .enumerate()
            .map(|(i, &close)| Kline {
                timestamp: i as i64,
                open: close,
                high: close * 1.01,
                low: close * 0.99,
                close,
                volume: 100.0,
            })
            .collect()
    }

    #[test]
    fn rejects_too_few_bars() {
        assert!(calculate_indicators(&bars(&[1.0, 2.0, 3.0, 4.0])).is_err());
    }

    #[test]
    fn computes_smas_and_changes() {
        let data = bars(&[1.0, 2.0, 3.0, 4.0, 5.0]);
        let ind = calculate_indicators(&data).unwrap();
        assert!((ind.sma_5 - 3.0).abs() < 1e-9);
        assert!((ind.sma_20 - 3.0).abs() < 1e-9);
        assert!((ind.price_change_1 - 25.0).abs() < 1e-9);
        assert!((ind.price_change_3 - 150.0).abs() < 1e-9);
    }

    #[test]
    fn atr_uses_true_range() {
        let data = bars(&[10.0; 20]);
        let ind = calculate_indicators(&data).unwrap();
        // high - low = 0.2, 无跳空，TR 均为 0.2
        assert!((ind.atr_14 - 0.2).abs() < 1e-9);
        assert!((ind.atr_percent - 2.0).abs() < 1e-9);
    }

    #[test]
    fn volume_ratio_compares_last_to_average() {
        let mut data = bars(&[10.0; 20]);
        data[19].volume = 200.0;
        let ind = calculate_indicators(&data).unwrap();
        assert!((ind.volume_ratio - 200.0 / 105.0).abs() < 1e-9);
    }
}
