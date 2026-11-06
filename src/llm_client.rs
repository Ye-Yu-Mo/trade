// DeepSeek 客户端：统一的模型调用、响应缓存与 JSON 解析。
//
// 所有智能体都通过这里请求模型，方便集中控制模型名、缓存和错误处理。

use anyhow::{Context, Result};
use async_openai::{
    config::OpenAIConfig,
    types::{
        ChatCompletionRequestMessage, ChatCompletionRequestSystemMessageArgs,
        ChatCompletionRequestUserMessageArgs, CreateChatCompletionRequestArgs,
    },
    Client,
};
use log::debug;
use sha2::{Digest, Sha256};
use std::fs;
use std::path::PathBuf;

const DEFAULT_BASE_URL: &str = "https://api.deepseek.com";
const DEFAULT_MODEL: &str = "deepseek-chat";

/// 调用模型。相同 (system, user) 提示词会命中磁盘缓存，
/// 回测和重复运行时不会重复付费。
pub async fn chat(system_prompt: &str, user_prompt: &str, api_key: &str) -> Result<String> {
    let key = cache_key(system_prompt, user_prompt);

    if let Some(cached) = read_cache(&key) {
        debug!("LLM 缓存命中: {}", key);
        return Ok(cached);
    }

    let content = request(system_prompt, user_prompt, api_key).await?;

    if cache_enabled() {
        write_cache(&key, &content);
    }

    Ok(content)
}

/// 从模型回复中提取第一个 JSON 对象并反序列化。
/// 兼容模型把 JSON 包在 markdown 代码块里的情况。
pub fn parse_json<T: serde::de::DeserializeOwned>(response: &str) -> Result<T> {
    let start = response.find('{').context("未找到JSON起始")?;
    let end = response.rfind('}').context("未找到JSON结束")? + 1;
    serde_json::from_str(&response[start..end]).context("解析JSON失败")
}

async fn request(system_prompt: &str, user_prompt: &str, api_key: &str) -> Result<String> {
    let base_url =
        std::env::var("DEEPSEEK_BASE_URL").unwrap_or_else(|_| DEFAULT_BASE_URL.to_string());
    let model = std::env::var("DEEPSEEK_MODEL").unwrap_or_else(|_| DEFAULT_MODEL.to_string());

    let config = OpenAIConfig::new()
        .with_api_key(api_key)
        .with_api_base(base_url);

    let client = Client::with_config(config);

    let request = CreateChatCompletionRequestArgs::default()
        .model(model)
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

fn cache_key(system_prompt: &str, user_prompt: &str) -> String {
    let mut hasher = Sha256::new();
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

#[cfg(test)]
mod tests {
    use super::*;

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
        let a = cache_key("sys", "user");
        assert_eq!(a, cache_key("sys", "user"));
        assert_ne!(a, cache_key("sys", "user2"));
        assert_eq!(a.len(), 64);
    }
}
