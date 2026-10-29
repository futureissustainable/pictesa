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
    let resp = req
        .timeout(API_TIMEOUT)
        .json(body)
        .send()
        .await
        .map_err(net_error)?;
    json_response(resp).await
}

pub fn net_error(e: reqwest::Error) -> ProviderError {
    ProviderError::from(e)
}

/// A readable name for a model id such as `flux-2-pro` or `google/veo-3.1-fast`: the last part,
/// words capitalised, known acronyms in capitals ("FLUX 2 Pro", "Veo 3.1 Fast", "Klein 4B").
pub fn slug_name(id: &str) -> String {
    let last = id.rsplit('/').next().unwrap_or(id);
    let words: Vec<String> = last
        .split(['-', '_'])
        .filter(|w| !w.is_empty())
        .map(|w| match w.to_ascii_lowercase().as_str() {
            "flux" | "sdxl" | "sd" | "ltx" | "gpt" | "ai" | "hd" => w.to_ascii_uppercase(),
            l if l.ends_with('b') && l[..l.len() - 1].parse::<f64>().is_ok() => w.to_ascii_uppercase(),
            _ if w.starts_with(|c: char| c.is_ascii_digit()) => w.to_string(),
            _ => {
                let mut chars = w.chars();
                chars.next().map(|f| f.to_uppercase().collect::<String>() + chars.as_str()).unwrap_or_default()
            }
        })
        .collect();
    if words.is_empty() { id.to_string() } else { words.join(" ") }
}

/// The company behind a model owner slug, for gateways that name models `owner/model`.
pub fn owner_name(owner: &str) -> String {
    match owner {
        "black-forest-labs" => "Black Forest Labs".into(),
        "bytedance" => "ByteDance".into(),
        "openai" => "OpenAI".into(),
        "stability-ai" => "Stability AI".into(),
        "ideogram-ai" => "Ideogram".into(),
        "recraft-ai" => "Recraft".into(),
        "minimax" => "MiniMax".into(),
        "kwaivgi" => "Kuaishou".into(),
        "wan-video" => "Wan".into(),
        _ => slug_name(owner),
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
                    .filter_map(|e| {
                        e.get("msg")
                            .or_else(|| e.get("message"))
                            .and_then(Value::as_str)
                            .map(String::from)
                    })
                    .collect();
                (!parts.is_empty()).then(|| parts.join("; "))
            }
            Value::Object(_) => v.get("message").and_then(Value::as_str).map(String::from),
            _ => None,
        }
    };
    v.get("detail")
        .and_then(pick)
        .or_else(|| v.get("error").and_then(pick))
        .or_else(|| v.get("message").and_then(pick))
}

/// Classifies a provider-reported failure message.
pub fn failure(msg: impl Into<String>) -> ProviderError {
    let msg = msg.into();
    let lower = msg.to_ascii_lowercase();
    if [
        "safety",
        "content policy",
        "moderation",
        "nsfw",
        "flagged",
        "sensitive",
    ]
    .iter()
    .any(|w| lower.contains(w))
    {
        ProviderError::ContentPolicy(msg)
    } else if pictesa_core::error::out_of_credit(&lower) {
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

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn transport_errors_name_the_server_not_the_url() {
        // Some APIs take the key as a query parameter; it must never reach a message.
        let e = reqwest::Client::new().get("http://127.0.0.1:1/v1/models?key=secret-key").send().await.unwrap_err();
        let text = net_error(e).to_string();
        assert!(text.contains("nothing answered at http://127.0.0.1:1"), "{text}");
        assert!(!text.contains("secret-key") && !text.contains("/v1/models"), "{text}");
        let e = reqwest::Client::new().get("not a url").send().await.unwrap_err();
        assert!(net_error(e).to_string().contains("not a valid URL"));
    }

    #[test]
    fn model_ids_read_as_names() {
        assert_eq!(slug_name("flux-2-flex"), "FLUX 2 Flex");
        assert_eq!(slug_name("black-forest-labs/flux-2-pro"), "FLUX 2 Pro");
        assert_eq!(slug_name("google/veo-3.1-fast"), "Veo 3.1 Fast");
        assert_eq!(slug_name("flux-2-klein-4b"), "FLUX 2 Klein 4B");
        assert_eq!(owner_name("black-forest-labs"), "Black Forest Labs");
    }
}
