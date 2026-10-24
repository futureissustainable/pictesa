//! Small helpers shared by provider adapters.

use std::time::Duration;

use pictesa_core::{json_response, MediaInput, ProviderError, ProviderResult};
use serde_json::Value;

/// Timeout for one API call. Synchronous image APIs (OpenRouter, OpenAI, Gemini) can take minutes at 4K.
pub const API_TIMEOUT: Duration = Duration::from_secs(300);

pub async fn get_json(req: reqwest::RequestBuilder) -> ProviderResult<Value> {
    let resp = req.timeout(API_TIMEOUT).send().await.map_err(net_error)?;
    json_response(resp).await
}

pub async fn send_json(req: reqwest::RequestBuilder, body: &Value) -> ProviderResult<Value> {
    let resp = req.timeout(API_TIMEOUT).json(body).send().await.map_err(net_error)?;
    json_response(resp).await
}

pub fn net_error(e: reqwest::Error) -> ProviderError {
    if e.is_timeout() {
        ProviderError::Transient(format!("request timed out: {e}"))
    } else {
        ProviderError::from(e)
    }
}

/// Extracts a human message from common error shapes (`detail`, `error.message`, `message`).
pub fn error_message(v: &Value) -> Option<String> {
    let pick = |v: &Value| -> Option<String> {
        match v {
            Value::String(s) => Some(s.clone()),
            Value::Array(a) => {
                let parts: Vec<String> = a
                    .iter()
                    .filter_map(|e| e.get("msg").or_else(|| e.get("message")).and_then(Value::as_str).map(String::from))
                    .collect();
                (!parts.is_empty()).then(|| parts.join("; "))
            }
            Value::Object(_) => v.get("message").and_then(Value::as_str).map(String::from),
            _ => None,
        }
    };
    v.get("detail").and_then(pick).or_else(|| v.get("error").and_then(pick)).or_else(|| v.get("message").and_then(pick))
}

/// Classifies a provider-reported failure message.
pub fn failure(msg: impl Into<String>) -> ProviderError {
    let msg = msg.into();
    let lower = msg.to_ascii_lowercase();
    if ["safety", "content policy", "moderation", "nsfw", "flagged", "sensitive"].iter().any(|w| lower.contains(w)) {
        ProviderError::ContentPolicy(msg)
    } else if lower.contains("insufficient") && (lower.contains("credit") || lower.contains("balance") || lower.contains("quota")) {
        ProviderError::InsufficientCredits(msg)
    } else {
        ProviderError::Failed(msg)
    }
}

/// Inline files below this size as data URIs; upload larger ones where the provider supports it.
pub const INLINE_LIMIT: u64 = 4 * 1024 * 1024;

pub fn file_size(input: &MediaInput) -> u64 {
    std::fs::metadata(&input.path).map(|m| m.len()).unwrap_or(0)
}

/// Normalises "16:9" style ratios to width and height for providers that need pixels.
pub fn ratio_to_size(ratio: &str, long_edge: u32) -> Option<(u32, u32)> {
    let (w, h) = ratio.split_once(':')?;
    let (w, h): (f64, f64) = (w.trim().parse().ok()?, h.trim().parse().ok()?);
    if w <= 0.0 || h <= 0.0 {
        return None;
    }
    let scale = long_edge as f64 / w.max(h);
    let round = |x: f64| ((x * scale / 16.0).round() as u32 * 16).max(16);
    Some((round(w), round(h)))
}
