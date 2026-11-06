// DeepSeek 客户端：统一的模型调用、JSON 模式、超时重试、响应缓存。
//
// 所有智能体都通过这里请求模型，方便集中控制模型名、温度、超时和错误处理。

use anyhow::{anyhow, Context, Result};
use async_openai::{
    config::OpenAIConfig,
    types::{
        ChatCompletionRequestMessage, ChatCompletionRequestSystemMessageArgs,
        ChatCompletionRequestUserMessageArgs, CreateChatCompletionRequestArgs, ResponseFormat,
    },
    Client,
};
use log::{debug, warn};
use serde::de::DeserializeOwned;
use sha2::{Digest, Sha256};
use std::fs;
use std::path::PathBuf;
use std::time::Duration;
use tokio::time::{sleep, timeout};

const DEFAULT_BASE_URL: &str = "https://api.deepseek.com";
const DEFAULT_MODEL: &str = "deepseek-chat";
const DEFAULT_TIMEOUT_SECS: u64 = 60;
const JSON_PARSE_ATTEMPTS: u32 = 2;

#[derive(Debug, Clone)]
struct LlmSettings {
    base_url: String,
    model: String,
    temperature: f32,
    timeout: Duration,
    retries: u32,
    json_mode: bool,
}

impl LlmSettings {
    fn from_env() -> Self {
        let flag = |key: &str, fallback: bool| -> bool {
            match std::env::var(key).as_deref() {
                Ok("false") | Ok("0") | Ok("off") => false,
                Ok("true") | Ok("1") | Ok("on") => true,
                _ => fallback,
            }
        };

        Self {
            base_url: std::env::var("DEEPSEEK_BASE_URL")
                .unwrap_or_else(|_| DEFAULT_BASE_URL.to_string()),
            model: std::env::var("DEEPSEEK_MODEL").unwrap_or_else(|_| DEFAULT_MODEL.to_string()),
            temperature: std::env::var("LLM_TEMPERATURE")
                .ok()
                .and_then(|v| v.parse::<f32>().ok())
                .filter(|v| (0.0..=2.0).contains(v))
                .unwrap_or(0.2),
            timeout: Duration::from_secs(
                std::env::var("LLM_TIMEOUT_SECS")
                    .ok()
                    .and_then(|v| v.parse::<u64>().ok())
                    .filter(|v| *v > 0)
                    .unwrap_or(DEFAULT_TIMEOUT_SECS),
            ),
            retries: std::env::var("LLM_MAX_RETRIES")
                .ok()
                .and_then(|v| v.parse::<u32>().ok())
                .unwrap_or(1),
            json_mode: flag("LLM_JSON_MODE", true),
        }
    }

    fn retry_key(&self) -> String {
        format!(
            "{}|{}|{}|{}",
            self.model,
            self.base_url,
            self.temperature,
            if self.json_mode { "json" } else { "text" }
        )
    }
}

/// 调用模型，返回原始文本。相同 (模型, 参数, 提示词) 会命中磁盘缓存。
pub async fn chat(system_prompt: &str, user_prompt: &str, api_key: &str) -> Result<String> {
    let settings = LlmSettings::from_env();
    let key = cache_key(&settings, system_prompt, user_prompt);
    get_or_fetch(&settings, &key, system_prompt, user_prompt, api_key).await
}

/// 调用模型并解析 JSON。解析失败时会作废缓存并重试一次。
pub async fn chat_json<T: DeserializeOwned>(
    system_prompt: &str,
    user_prompt: &str,
    api_key: &str,
) -> Result<T> {
    let settings = LlmSettings::from_env();
    let key = cache_key(&settings, system_prompt, user_prompt);
    let mut last_error = None;

    for attempt in 1..=JSON_PARSE_ATTEMPTS {
        let content = if attempt == 1 {
            get_or_fetch(&settings, &key, system_prompt, user_prompt, api_key).await?
        } else {
            // 缓存里可能是无法解析的旧响应，删掉后重新请求
            invalidate_cache(&key);
            fetch_and_cache(&settings, &key, system_prompt, user_prompt, api_key).await?
        };

        match parse_json(&content) {
            Ok(value) => return Ok(value),
            Err(err) => {
                warn!(
                    "LLM 返回无法解析的 JSON（第 {}/{} 次）: {:#}",
                    attempt, JSON_PARSE_ATTEMPTS, err
                );
                last_error = Some(err);
            }
        }
    }

    Err(last_error
        .unwrap_or_else(|| anyhow!("LLM 返回为空"))
        .context("LLM 连续返回无法解析的 JSON"))
}

/// 从模型回复中提取第一个 JSON 对象并反序列化。
/// 兼容模型把 JSON 包在 markdown 代码块里的情况。
pub fn parse_json<T: DeserializeOwned>(response: &str) -> Result<T> {
    let start = response.find('{').context("未找到JSON起始")?;
    let end = response.rfind('}').context("未找到JSON结束")? + 1;
    serde_json::from_str(&response[start..end]).context("解析JSON失败")
}

async fn get_or_fetch(
    settings: &LlmSettings,
    key: &str,
    system_prompt: &str,
    user_prompt: &str,
    api_key: &str,
) -> Result<String> {
    if let Some(cached) = read_cache(key) {
        debug!("LLM 缓存命中: {}", key);
        return Ok(cached);
    }
    fetch_and_cache(settings, key, system_prompt, user_prompt, api_key).await
}

async fn fetch_and_cache(
    settings: &LlmSettings,
    key: &str,
    system_prompt: &str,
    user_prompt: &str,
    api_key: &str,
) -> Result<String> {
    let content = request_with_retry(settings, system_prompt, user_prompt, api_key).await?;
    if cache_enabled() {
        write_cache(key, &content);
    }
    Ok(content)
}

async fn request_with_retry(
    settings: &LlmSettings,
    system_prompt: &str,
    user_prompt: &str,
    api_key: &str,
) -> Result<String> {
    let attempts = settings.retries.saturating_add(1);
    let mut last_error: Option<anyhow::Error> = None;

    for attempt in 1..=attempts {
        match timeout(
            settings.timeout,
            request(settings, system_prompt, user_prompt, api_key),
        )
        .await
        {
            Ok(Ok(content)) => return Ok(content),
            Ok(Err(err)) => {
                warn!("LLM 请求失败（第 {}/{} 次）: {:#}", attempt, attempts, err);
                last_error = Some(err);
            }
            Err(_) => {
                warn!(
                    "LLM 请求超时（第 {}/{} 次，{} 秒）",
                    attempt,
                    attempts,
                    settings.timeout.as_secs()
                );
                last_error = Some(anyhow!("LLM 请求超时"));
            }
        }

        if attempt < attempts {
            sleep(Duration::from_millis(400 * u64::from(attempt))).await;
        }
    }

    Err(last_error.unwrap_or_else(|| anyhow!("LLM 请求失败")))
}

async fn request(
    settings: &LlmSettings,
    system_prompt: &str,
    user_prompt: &str,
    api_key: &str,
) -> Result<String> {
    let config = OpenAIConfig::new()
        .with_api_key(api_key)
        .with_api_base(&settings.base_url);

    let client = Client::with_config(config);

    let mut builder = CreateChatCompletionRequestArgs::default();
    builder
        .model(&settings.model)
        .temperature(settings.temperature)
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
        ]);

    if settings.json_mode {
        builder.response_format(ResponseFormat::JsonObject);
    }

    let request = builder.build()?;

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

fn cache_enabled() -> bool {
    !matches!(
        std::env::var("LLM_CACHE").as_deref(),
        Ok("off") | Ok("0") | Ok("false")
    )
}

fn cache_dir() -> PathBuf {
    std::env::var("LLM_CACHE_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|_| PathBuf::from(".cache/llm"))
}

fn cache_key(settings: &LlmSettings, system_prompt: &str, user_prompt: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(settings.retry_key().as_bytes());
    hasher.update(b"\n---\n");
    hasher.update(system_prompt.as_bytes());
    hasher.update(b"\n---\n");
    hasher.update(user_prompt.as_bytes());
    hex::encode(hasher.finalize())
}

fn read_cache(key: &str) -> Option<String> {
    if !cache_enabled() {
        return None;
    }
    let text = fs::read_to_string(cache_dir().join(key)).ok()?;
    if text.trim().is_empty() {
        None
    } else {
        Some(text)
    }
}

fn write_cache(key: &str, content: &str) {
    let dir = cache_dir();
    if fs::create_dir_all(&dir).is_err() {
        return;
    }
    let _ = fs::write(dir.join(key), content);
}

fn invalidate_cache(key: &str) {
    if cache_enabled() {
        let _ = fs::remove_file(cache_dir().join(key));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn settings(model: &str, temperature: f32) -> LlmSettings {
        LlmSettings {
            base_url: DEFAULT_BASE_URL.to_string(),
            model: model.to_string(),
            temperature,
            timeout: Duration::from_secs(60),
            retries: 1,
            json_mode: true,
        }
    }

    #[test]
    fn parses_json_wrapped_in_markdown() {
        let text = "```json\n{\"signal\":\"BUY\",\"confidence\":\"HIGH\",\"reason\":\"test\"}\n```";
        #[derive(serde::Deserialize)]
        struct D {
            signal: String,
            confidence: String,
            reason: String,
        }
        let parsed: D = parse_json(text).unwrap();
        assert_eq!(parsed.signal, "BUY");
        assert_eq!(parsed.confidence, "HIGH");
        assert_eq!(parsed.reason, "test");
    }

    #[test]
    fn cache_key_is_stable_and_sensitive_to_input() {
        let s = settings(DEFAULT_MODEL, 0.2);
        let a = cache_key(&s, "sys", "user");
        assert_eq!(a, cache_key(&s, "sys", "user"));
        assert_ne!(a, cache_key(&s, "sys", "user2"));
        assert_ne!(a, cache_key(&settings("other-model", 0.2), "sys", "user"));
        assert_ne!(a, cache_key(&settings(DEFAULT_MODEL, 0.7), "sys", "user"));
        assert_eq!(a.len(), 64);
    }

    #[test]
    fn retry_key_separates_json_and_text_mode() {
        let mut s = settings(DEFAULT_MODEL, 0.2);
        let json = s.retry_key();
        s.json_mode = false;
        assert_ne!(json, s.retry_key());
    }
}
