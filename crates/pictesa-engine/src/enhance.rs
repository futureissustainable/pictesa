//! "Improve prompt": rewrites a short idea into a detailed image or video prompt with a text model.
//! Runs on the person's OpenRouter key. The default model is free; `PICTESA_ENHANCE_MODEL` overrides it.

use pictesa_core::MediaKind;
use serde_json::{json, Value};

use crate::{Engine, EngineError, Result};

/// Free models tried in order when one is busy (free tiers are rate limited).
const FREE_MODELS: &[&str] = &[
    "google/gemma-4-26b-a4b-it:free",
    "nvidia/nemotron-3.5-super-vl:free",
    "nvidia/nemotron-3-ultra-550b-a55b:free",
    "dots-studio/dots-3-note-preview:free",
    "nvidia/nemotron-3.5-lightning:free",
    "liquid/lfm-2.5-2.6b:free",
];

fn system_prompt(output: MediaKind) -> &'static str {
    match output {
        MediaKind::Video => "You write prompts for AI video models. Rewrite the user's idea as one vivid paragraph under 120 words: \
subject and action first, then setting, camera move and lens, lighting, mood and pace. Describe one continuous shot. \
Keep every named person, product, brand, color and detail the user gave, and keep the user's language. \
Do not add text overlays, logos or people that were not asked for. Reply with only the final prompt between <prompt> and </prompt>.",
        _ => "You write prompts for AI image models. Rewrite the user's idea as one vivid paragraph under 100 words: \
subject first, then composition, setting, lighting, lens or medium, color and mood. \
Keep every named person, product, brand, color and detail the user gave, and keep the user's language. \
Do not add text, logos or people that were not asked for. Reply with only the final prompt between <prompt> and </prompt>.",
    }
}

/// The text inside the last `<prompt>...</prompt>`; or the whole reply when it is short and
/// clearly not the model thinking aloud.
fn extract_prompt(text: &str) -> Option<String> {
    let clean = |s: &str| s.trim().trim_matches(|c| c == '"' || c == '\u{201c}' || c == '\u{201d}').trim().to_string();
    if let Some(start) = text.rfind("<prompt>") {
        let rest = &text[start + 8..];
        let body = rest.split("</prompt>").next().unwrap_or(rest);
        let body = clean(body);
        return (!body.is_empty() && rest.contains("</prompt>")).then_some(body);
    }
    let body = clean(text);
    let lower = body.to_lowercase();
    let thinking = ["the user", "we need", "let's", "count words", "i need to", "okay,"].iter().any(|w| lower.starts_with(w) || lower.contains(&format!("\n{w}")));
    (!body.is_empty() && !thinking && body.split_whitespace().count() <= 180).then_some(body)
}

impl Engine {
    /// Whether "Improve prompt" can run for this user (an OpenRouter key it may use is set).
    pub async fn enhance_available(&self, user: &str) -> Result<bool> {
        let Some(provider) = self.inner.catalog.provider("openrouter") else { return Ok(false) };
        let sources = if self.mode() == crate::Mode::Hosted { crate::creds::Sources::Stored } else { crate::creds::Sources::Any };
        let resolved = self.resolve_with(user, provider.as_ref(), sources).await?;
        Ok(Self::is_configured(&provider.info(), &resolved))
    }

    pub async fn enhance_prompt(&self, user: &str, prompt: &str, output: MediaKind) -> Result<String> {
        let prompt = prompt.trim();
        if prompt.is_empty() {
            return Err(EngineError::invalid("Write an idea first.", Some("prompt")));
        }
        if prompt.chars().count() > 4000 {
            return Err(EngineError::invalid("The prompt is too long to improve.", Some("prompt")));
        }
        let provider = self.inner.catalog.provider("openrouter").ok_or_else(|| EngineError::invalid("Prompt help needs the OpenRouter provider.", None))?;
        // Hosted: only the person's own key; the operator's key is never spent on prompt help.
        let sources = if self.mode() == crate::Mode::Hosted { crate::creds::Sources::Stored } else { crate::creds::Sources::Any };
        let ctx = self.ctx_from(self.resolve_with(user, provider.as_ref(), sources).await?);
        let key = ctx.get("api_key").ok_or_else(|| EngineError::invalid("Add your OpenRouter key in Settings > Providers to use prompt help (a free model is used).", None))?;
        let models: Vec<String> = match std::env::var("PICTESA_ENHANCE_MODEL").ok().filter(|m| !m.is_empty()) {
            Some(m) => vec![m],
            None => FREE_MODELS.iter().map(|m| m.to_string()).collect(),
        };
        let mut last = String::new();
        for model in &models {
            let body = json!({
                "model": model,
                "messages": [
                    { "role": "system", "content": system_prompt(output) },
                    { "role": "user", "content": prompt }
                ],
                "temperature": 0.7,
                "max_tokens": 1500,
                "reasoning": { "exclude": true },
            });
            let resp = match self
                .inner
                .http
                .post("https://openrouter.ai/api/v1/chat/completions")
                .bearer_auth(key)
                .header("X-Title", "Pictesa")
                .json(&body)
                .timeout(std::time::Duration::from_secs(60))
                .send()
                .await
            {
                Ok(r) => r,
                Err(e) => {
                    last = e.to_string();
                    continue;
                }
            };
            let status = resp.status();
            let v: Value = resp.json().await.unwrap_or(Value::Null);
            if !status.is_success() {
                last = format!("{status}: {}", v.pointer("/error/message").and_then(Value::as_str).unwrap_or("request failed"));
                // Busy, restricted or gone: try the next model. A rejected key (401) stops here.
                if status.as_u16() != 401 {
                    continue;
                }
                return Err(EngineError::invalid(format!("Prompt help failed ({last})"), None));
            }
            let text = v.pointer("/choices/0/message/content").and_then(Value::as_str).unwrap_or("");
            if let Some(answer) = extract_prompt(text) {
                return Ok(answer);
            }
            last = format!("{model} gave no usable answer");
        }
        Err(EngineError::invalid(
            format!("The free prompt models are busy ({last}). Try again soon, or set PICTESA_ENHANCE_MODEL to a paid OpenRouter model such as deepseek/deepseek-v4.1-flash."),
            None,
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::extract_prompt;

    #[test]
    fn takes_the_tagged_answer_and_rejects_thinking() {
        assert_eq!(extract_prompt("thinking...\n<prompt>A cabin at dawn.</prompt>").as_deref(), Some("A cabin at dawn."));
        assert_eq!(extract_prompt("The user wants a prompt. We need to"), None);
        assert_eq!(extract_prompt("<prompt>cut off"), None);
        assert_eq!(extract_prompt("A cabin on a lake at dawn, slow push in.").as_deref(), Some("A cabin on a lake at dawn, slow push in."));
    }
}
