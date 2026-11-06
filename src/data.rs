// 历史K线获取与本地缓存。
//
// 回测不应该每次运行都打 Binance 接口，也不想被限流。
// 这里按 (symbol, interval, start, end) 缓存成 JSON，重复回测直接读盘。

use crate::market;
use crate::types::Kline;
use anyhow::{bail, Context, Result};
use chrono::{DateTime, NaiveDate, NaiveDateTime, TimeZone, Utc};
use log::info;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};

const PAGE_LIMIT: u32 = 1500; // Binance U 本位合约单次上限
const MAX_PAGES: usize = 500;

#[derive(Debug, Serialize, Deserialize)]
struct CachedKlines {
    symbol: String,
    interval: String,
    start_ms: i64,
    end_ms: i64,
    klines: Vec<Kline>,
}

/// 加载指定区间的K线，优先读缓存。
pub async fn load_klines(
    symbol: &str,
    interval: &str,
    start_ms: i64,
    end_ms: i64,
    data_dir: &Path,
) -> Result<Vec<Kline>> {
    if start_ms >= end_ms {
        bail!("开始时间必须早于结束时间");
    }

    let path = cache_path(data_dir, symbol, interval, start_ms, end_ms);

    if let Ok(text) = fs::read_to_string(&path) {
        if let Ok(cache) = serde_json::from_str::<CachedKlines>(&text) {
            if !cache.klines.is_empty() {
                info!(
                    "命中K线缓存: {} ({} 根)",
                    path.display(),
                    cache.klines.len()
                );
                return Ok(cache.klines);
            }
        }
    }

    info!(
        "下载K线: {} {} {} → {}",
        symbol,
        interval,
        format_ms(start_ms),
        format_ms(end_ms)
    );

    let klines = download(symbol, interval, start_ms, end_ms).await?;

    if klines.is_empty() {
        bail!("未获取到任何K线，请检查交易对、周期和时间范围");
    }

    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).context("创建缓存目录失败")?;
    }
    let cache = CachedKlines {
        symbol: symbol.to_string(),
        interval: interval.to_string(),
        start_ms,
        end_ms,
        klines: klines.clone(),
    };
    let json = serde_json::to_string(&cache).context("序列化K线缓存失败")?;
    fs::write(&path, json).context("写入K线缓存失败")?;

    info!("已缓存 {} 根K线到 {}", klines.len(), path.display());
    Ok(klines)
}

async fn download(symbol: &str, interval: &str, start_ms: i64, end_ms: i64) -> Result<Vec<Kline>> {
    let mut cursor = start_ms;
    let mut by_time: BTreeMap<i64, Kline> = BTreeMap::new();

    for _ in 0..MAX_PAGES {
        let page = market::fetch_klines_range(symbol, interval, cursor, end_ms, PAGE_LIMIT).await?;

        let Some(last) = page.last() else {
            break;
        };
        let last_ts = last.timestamp;

        for kline in page {
            by_time.insert(kline.timestamp, kline);
        }

        // 已经到区间末尾，或接口没有继续前进
        if last_ts + 1 >= end_ms || last_ts < cursor {
            break;
        }
        cursor = last_ts + 1;
    }

    Ok(by_time.into_values().collect())
}

fn cache_path(
    data_dir: &Path,
    symbol: &str,
    interval: &str,
    start_ms: i64,
    end_ms: i64,
) -> PathBuf {
    let name = format!("{}_{}_{}_{}.json", symbol, interval, start_ms, end_ms);
    data_dir.join(name)
}

fn format_ms(ms: i64) -> String {
    Utc.timestamp_millis_opt(ms)
        .single()
        .map(|dt| dt.format("%Y-%m-%d %H:%M").to_string())
        .unwrap_or_else(|| ms.to_string())
}

/// 周期字符串（如 `1h`）对应的毫秒数，用于把默认时间对齐到K线边界。
pub fn interval_millis(interval: &str) -> Option<i64> {
    let (num, unit) = interval.split_at(interval.len().checked_sub(1)?);
    let n: i64 = num.parse().ok()?;
    if n <= 0 {
        return None;
    }
    let unit_ms = match unit {
        "m" => 60_000,
        "h" => 3_600_000,
        "d" => 86_400_000,
        "w" => 604_800_000,
        _ => return None,
    };
    Some(n * unit_ms)
}

/// 从本地 JSON 文件读取K线数组（示例数据 / 离线回测）。
pub fn load_kline_file(path: &Path) -> Result<Vec<Kline>> {
    let text =
        fs::read_to_string(path).with_context(|| format!("读取K线文件失败: {}", path.display()))?;
    let klines: Vec<Kline> = serde_json::from_str(&text)
        .with_context(|| format!("解析K线文件失败: {}", path.display()))?;
    if klines.is_empty() {
        bail!("K线文件为空: {}", path.display());
    }
    Ok(klines)
}

/// 解析用户输入的时间。支持：
/// - `2025-01-01`
/// - `2025-01-01 12:30`
/// - `2025-01-01T12:30:00Z`
/// - 毫秒时间戳
pub fn parse_time(input: &str) -> Result<i64> {
    let trimmed = input.trim();

    if let Ok(dt) = DateTime::parse_from_rfc3339(trimmed) {
        return Ok(dt.timestamp_millis());
    }

    if let Ok(native) = NaiveDateTime::parse_from_str(trimmed, "%Y-%m-%d %H:%M:%S") {
        return Ok(native.and_utc().timestamp_millis());
    }

    if let Ok(native) = NaiveDateTime::parse_from_str(trimmed, "%Y-%m-%d %H:%M") {
        return Ok(native.and_utc().timestamp_millis());
    }

    if let Ok(date) = NaiveDate::parse_from_str(trimmed, "%Y-%m-%d") {
        return Ok(date
            .and_hms_opt(0, 0, 0)
            .context("日期转换失败")?
            .and_utc()
            .timestamp_millis());
    }

    if let Ok(ms) = trimmed.parse::<i64>() {
        return Ok(ms);
    }

    bail!(
        "无法解析时间: {}（示例: 2025-01-01 或 2025-01-01 12:30）",
        input
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_common_time_formats() {
        assert_eq!(
            parse_time("2025-01-01").unwrap(),
            parse_time("2025-01-01 00:00").unwrap()
        );
        assert_eq!(
            parse_time("2025-01-01T00:00:00Z").unwrap(),
            parse_time("2025-01-01 00:00").unwrap()
        );
        assert_eq!(parse_time("1735689600000").unwrap(), 1735689600000);
        assert!(parse_time("not-a-date").is_err());
    }

    #[test]
    fn cache_path_is_deterministic() {
        let a = cache_path(Path::new("data"), "BTCUSDT", "1h", 1, 2);
        let b = cache_path(Path::new("data"), "BTCUSDT", "1h", 1, 2);
        assert_eq!(a, b);
        assert!(a.to_string_lossy().contains("BTCUSDT_1h_1_2.json"));
    }

    #[test]
    fn bundled_kline_sample_is_valid() {
        let text = include_str!("../examples/BTCUSDT_1h_sample.json");
        let klines: Vec<Kline> = serde_json::from_str(text).unwrap();
        assert!(klines.len() > 2_000);
        assert!(klines.iter().all(|k| k.close > 0.0));
    }

    #[test]
    fn parses_interval_durations() {
        assert_eq!(interval_millis("1m"), Some(60_000));
        assert_eq!(interval_millis("15m"), Some(900_000));
        assert_eq!(interval_millis("1h"), Some(3_600_000));
        assert_eq!(interval_millis("4h"), Some(14_400_000));
        assert_eq!(interval_millis("1d"), Some(86_400_000));
        assert_eq!(interval_millis("1x"), None);
    }
}
