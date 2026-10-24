use std::time::Duration;

/// Errors a provider reports. The engine uses the variant to decide whether to retry.
#[derive(Debug, Clone, thiserror::Error, serde::Serialize, serde::Deserialize)]
#[serde(tag = "kind", content = "detail", rename_all = "snake_case")]
pub enum ProviderError {
    /// Missing or rejected credentials. Never retried.
    #[error("authentication failed: {0}")]
    Auth(String),
    /// The provider asked us to slow down. Retried after the delay.
    #[error("rate limited")]
    RateLimited { retry_after_ms: Option<u64> },
    /// The account has no credits or quota left. Never retried.
    #[error("insufficient credits: {0}")]
    InsufficientCredits(String),
    /// The request was refused by a safety filter. Never retried.
    #[error("blocked by content policy: {0}")]
    ContentPolicy(String),
    /// The input does not fit the model (bad size, missing image, ...). Never retried.
    #[error("invalid input: {0}")]
    InvalidInput(String),
    /// The model or option is not available through this provider.
    #[error("not supported: {0}")]
    NotSupported(String),
    /// Network trouble or a 5xx answer. Retried with backoff.
    #[error("temporary failure: {0}")]
    Transient(String),
    /// The remote job failed for a reason the provider explained.
    #[error("generation failed: {0}")]
    Failed(String),
}

impl ProviderError {
    pub fn is_retryable(&self) -> bool {
        matches!(self, Self::RateLimited { .. } | Self::Transient(_))
    }

    pub fn retry_after(&self) -> Option<Duration> {
        match self {
            Self::RateLimited { retry_after_ms: Some(ms) } => Some(Duration::from_millis(*ms)),
            _ => None,
        }
    }

    /// Short machine name for storage and API answers.
    pub fn kind(&self) -> &'static str {
        match self {
            Self::Auth(_) => "auth",
            Self::RateLimited { .. } => "rate_limited",
            Self::InsufficientCredits(_) => "insufficient_credits",
            Self::ContentPolicy(_) => "content_policy",
            Self::InvalidInput(_) => "invalid_input",
            Self::NotSupported(_) => "not_supported",
            Self::Transient(_) => "transient",
            Self::Failed(_) => "failed",
        }
    }

    /// Maps an HTTP status and body from a provider API to the right variant.
    pub fn from_http(status: u16, body: &str, retry_after_ms: Option<u64>) -> Self {
        let detail = truncate(body, 600);
        match status {
            401 | 403 => Self::Auth(detail),
            402 => Self::InsufficientCredits(detail),
            408 | 425 | 500..=599 => Self::Transient(format!("HTTP {status}: {detail}")),
            429 => Self::RateLimited { retry_after_ms },
            400 | 404 | 409 | 413 | 415 | 422 => {
                let lower = body.to_ascii_lowercase();
                if ["safety", "content policy", "moderation", "nsfw", "flagged", "inappropriate"]
                    .iter()
                    .any(|w| lower.contains(w))
                {
                    Self::ContentPolicy(detail)
                } else {
                    Self::InvalidInput(format!("HTTP {status}: {detail}"))
                }
            }
            _ => Self::Failed(format!("HTTP {status}: {detail}")),
        }
    }
}

impl From<reqwest::Error> for ProviderError {
    fn from(e: reqwest::Error) -> Self {
        if let Some(status) = e.status() {
            return Self::from_http(status.as_u16(), &e.to_string(), None);
        }
        Self::Transient(e.to_string())
    }
}

impl From<serde_json::Error> for ProviderError {
    fn from(e: serde_json::Error) -> Self {
        Self::Failed(format!("unexpected response: {e}"))
    }
}

pub fn truncate(s: &str, max: usize) -> String {
    if s.len() <= max {
        return s.to_string();
    }
    let mut end = max;
    while !s.is_char_boundary(end) {
        end -= 1;
    }
    format!("{}…", &s[..end])
}
