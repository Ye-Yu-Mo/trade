// 只读 Dashboard。
//
// 刻意不共享内存状态：直接读 logs/ 和 backtest/ 下的文件。
// 这样 `trade run` 和 `trade dashboard` 可以是两个独立进程，
// Dashboard 挂掉也不会影响交易。

use anyhow::{Context, Result};
use axum::{
    extract::{Query, State},
    http::StatusCode,
    response::{Html, IntoResponse},
    routing::get,
    Json, Router,
};
use log::{info, warn};
use serde::Deserialize;
use serde_json::{json, Value};
use std::fs;
use std::io::{Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};
use std::sync::Arc;

const MAX_JSONL_BYTES: u64 = 512 * 1024;
const DEFAULT_LIMIT: usize = 50;
const MAX_LIMIT: usize = 1000;

#[derive(Debug, Clone)]
pub struct DashboardConfig {
    pub logs_dir: PathBuf,
    pub backtest_dir: PathBuf,
}

pub async fn serve(host: &str, port: u16, config: DashboardConfig) -> Result<()> {
    let addr = format!("{}:{}", host, port);
    let listener = tokio::net::TcpListener::bind(&addr)
        .await
        .with_context(|| format!("无法监听 {}", addr))?;

    info!("Dashboard 已启动: http://{}", addr);
    if host != "127.0.0.1" && host != "localhost" {
        warn!("Dashboard 绑定在 {}，请确认网络环境可信", host);
    }

    axum::serve(listener, router(config))
        .await
        .context("Dashboard 服务异常")?;
    Ok(())
}

fn router(config: DashboardConfig) -> Router {
    Router::new()
        .route("/", get(index))
        .route("/api/status", get(api_status))
        .route("/api/performance", get(api_performance))
        .route("/api/risk", get(api_risk))
        .route("/api/trades", get(api_trades))
        .route("/api/decisions", get(api_decisions))
        .route("/api/equity", get(api_equity))
        .route("/api/backtests", get(api_backtests))
        .route("/api/backtest", get(api_backtest))
        .with_state(Arc::new(config))
}

async fn index() -> Html<&'static str> {
    Html(include_str!("dashboard.html"))
}

async fn api_status(State(cfg): State<Arc<DashboardConfig>>) -> impl IntoResponse {
    ok(read_json(&cfg.logs_dir.join("status.json")))
}

async fn api_performance(State(cfg): State<Arc<DashboardConfig>>) -> impl IntoResponse {
    ok(read_json(&cfg.logs_dir.join("performance.json")))
}

async fn api_risk(State(cfg): State<Arc<DashboardConfig>>) -> impl IntoResponse {
    ok(read_json(&cfg.logs_dir.join("risk_state.json")))
}

#[derive(Debug, Deserialize)]
struct LimitQuery {
    limit: Option<usize>,
}

async fn api_trades(
    State(cfg): State<Arc<DashboardConfig>>,
    Query(query): Query<LimitQuery>,
) -> impl IntoResponse {
    let limit = clamp_limit(query.limit);
    let path = cfg.logs_dir.join("trades.jsonl");
    if !path.is_file() {
        return ok(Value::Array(Vec::new()));
    }
    match tail_jsonl(&path, limit) {
        Ok(items) => ok(Value::Array(items)),
        Err(err) => internal_error(err),
    }
}

async fn api_decisions(
    State(cfg): State<Arc<DashboardConfig>>,
    Query(query): Query<LimitQuery>,
) -> impl IntoResponse {
    let limit = clamp_limit(query.limit);
    let path = cfg.logs_dir.join("decisions.jsonl");
    if !path.is_file() {
        return ok(Value::Array(Vec::new()));
    }
    match tail_jsonl(&path, limit) {
        Ok(items) => ok(Value::Array(items)),
        Err(err) => internal_error(err),
    }
}

async fn api_equity(
    State(cfg): State<Arc<DashboardConfig>>,
    Query(query): Query<LimitQuery>,
) -> impl IntoResponse {
    let limit = clamp_limit(query.limit);
    let path = cfg.logs_dir.join("equity.jsonl");
    if !path.is_file() {
        return ok(Value::Array(Vec::new()));
    }
    match tail_jsonl(&path, limit) {
        Ok(mut items) => {
            items.reverse(); // 曲线按时间正序
            ok(Value::Array(items))
        }
        Err(err) => internal_error(err),
    }
}

async fn api_backtests(State(cfg): State<Arc<DashboardConfig>>) -> impl IntoResponse {
    ok(Value::Array(list_backtests(&cfg.backtest_dir)))
}

#[derive(Debug, Deserialize)]
struct FileQuery {
    file: String,
}

async fn api_backtest(
    State(cfg): State<Arc<DashboardConfig>>,
    Query(query): Query<FileQuery>,
) -> impl IntoResponse {
    match safe_backtest_path(&cfg.backtest_dir, &query.file) {
        Some(path) => ok(read_json(&path)),
        None => (
            StatusCode::BAD_REQUEST,
            Json(json!({"error": "invalid file"})),
        ),
    }
}

fn ok(value: Value) -> (StatusCode, Json<Value>) {
    (StatusCode::OK, Json(value))
}

fn internal_error(err: anyhow::Error) -> (StatusCode, Json<Value>) {
    warn!("Dashboard 读取失败: {:#}", err);
    (
        StatusCode::INTERNAL_SERVER_ERROR,
        Json(json!({"error": format!("{:#}", err)})),
    )
}

fn clamp_limit(value: Option<usize>) -> usize {
    value.unwrap_or(DEFAULT_LIMIT).clamp(1, MAX_LIMIT)
}

fn read_json(path: &Path) -> Value {
    fs::read_to_string(path)
        .ok()
        .and_then(|text| serde_json::from_str(&text).ok())
        .unwrap_or_else(|| json!({}))
}

/// 读取 JSONL 的尾部。返回最新在前。
fn tail_jsonl(path: &Path, limit: usize) -> Result<Vec<Value>> {
    let mut file = fs::File::open(path).with_context(|| format!("打开 {} 失败", path.display()))?;

    let len = file.metadata().context("读取文件信息失败")?.len();
    let start = len.saturating_sub(MAX_JSONL_BYTES);
    file.seek(SeekFrom::Start(start)).context("定位文件失败")?;

    let mut bytes = Vec::new();
    file.read_to_end(&mut bytes).context("读取文件失败")?;
    let text = String::from_utf8_lossy(&bytes);
    let mut lines: Vec<&str> = text.lines().collect();

    // 从中间开始读时，第一行可能是残缺的
    if start > 0 && !lines.is_empty() {
        lines.remove(0);
    }

    Ok(lines
        .iter()
        .rev()
        .filter_map(|line| serde_json::from_str::<Value>(line).ok())
        .take(limit)
        .collect())
}

fn list_backtests(dir: &Path) -> Vec<Value> {
    let Ok(entries) = fs::read_dir(dir) else {
        return Vec::new();
    };

    let mut items: Vec<Value> = entries
        .flatten()
        .map(|entry| entry.path())
        .filter(|path| path.extension().and_then(|ext| ext.to_str()) == Some("json"))
        .filter_map(|path| {
            let text = fs::read_to_string(&path).ok()?;
            let value: Value = serde_json::from_str(&text).ok()?;
            let config = value.get("config")?.as_object()?;
            let metrics = value.get("metrics").cloned().unwrap_or_else(|| json!({}));
            Some(json!({
                "file": path.file_name()?.to_string_lossy(),
                "symbol": config.get("symbol").cloned().unwrap_or(Value::Null),
                "interval": config.get("interval").cloned().unwrap_or(Value::Null),
                "strategy": config.get("strategy").cloned().unwrap_or(Value::Null),
                "start_ms": config.get("start_ms").cloned().unwrap_or(Value::Null),
                "end_ms": config.get("end_ms").cloned().unwrap_or(Value::Null),
                "metrics": metrics,
            }))
        })
        .collect();

    items.sort_by_key(|item| {
        std::cmp::Reverse(item.get("end_ms").and_then(Value::as_i64).unwrap_or(0))
    });
    items
}

fn safe_backtest_path(dir: &Path, file: &str) -> Option<PathBuf> {
    let name = Path::new(file).file_name()?.to_str()?;
    if name != file || !name.ends_with(".json") {
        return None;
    }
    if !name
        .chars()
        .all(|c| c.is_ascii_alphanumeric() || matches!(c, '_' | '-' | '.'))
    {
        return None;
    }
    let path = dir.join(name);
    path.is_file().then_some(path)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    fn temp_file(name: &str, content: &str) -> PathBuf {
        let path =
            std::env::temp_dir().join(format!("trade-dashboard-{}-{}", std::process::id(), name));
        let mut file = fs::File::create(&path).unwrap();
        file.write_all(content.as_bytes()).unwrap();
        path
    }

    #[test]
    fn tail_returns_newest_first_and_respects_limit() {
        let path = temp_file("tail.jsonl", "{\"n\":1}\n{\"n\":2}\nnot-json\n{\"n\":3}\n");
        let items = tail_jsonl(&path, 2).unwrap();
        assert_eq!(items.len(), 2);
        assert_eq!(items[0]["n"], 3);
        assert_eq!(items[1]["n"], 2);
        let _ = fs::remove_file(path);
    }

    #[test]
    fn missing_file_is_an_error() {
        assert!(tail_jsonl(Path::new("/definitely/not/here.jsonl"), 1).is_err());
    }

    #[test]
    fn rejects_path_traversal() {
        let dir = std::env::temp_dir();
        assert!(safe_backtest_path(&dir, "../etc/passwd").is_none());
        assert!(safe_backtest_path(&dir, "foo/bar.json").is_none());
        assert!(safe_backtest_path(&dir, "foo.txt").is_none());
    }

    #[test]
    fn limit_is_clamped() {
        assert_eq!(clamp_limit(None), DEFAULT_LIMIT);
        assert_eq!(clamp_limit(Some(0)), 1);
        assert_eq!(clamp_limit(Some(10_000)), MAX_LIMIT);
    }

    #[test]
    fn lists_only_json_backtests() {
        let dir = std::env::temp_dir().join(format!("trade-bt-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        fs::write(
            dir.join("a.json"),
            r#"{"config":{"symbol":"BTCUSDT","interval":"1h","strategy":"sma_cross","start_ms":0,"end_ms":10},"metrics":{"total_return_pct":1.0}}"#,
        )
        .unwrap();
        fs::write(dir.join("b.csv"), "x").unwrap();

        let items = list_backtests(&dir);
        assert_eq!(items.len(), 1);
        assert_eq!(items[0]["symbol"], "BTCUSDT");
        let _ = fs::remove_dir_all(dir);
    }
}
