use std::{collections::HashMap, time::Duration};

use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::{GenerationRequest, MediaKind, ModelSpec, ProviderError};

pub type ProviderResult<T> = Result<T, ProviderError>;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ProviderKind {
    /// A model maker's own API (OpenAI, Google, Runway, ...).
    Cloud,
    /// A marketplace that hosts many models behind one key (fal, Replicate, Comfy Router, ...).
    Gateway,
    /// Runs on the user's own machine or server (ComfyUI, stable-diffusion.cpp).
    Local,
}

/// One field the user fills in Settings to enable a provider.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CredentialField {
    /// Storage key, e.g. `api_key` or `base_url`.
    pub key: String,
    pub label: String,
    /// Environment variables read when the field is not stored (first match wins).
    #[serde(default)]
    pub env: Vec<String>,
    /// Secrets are encrypted at rest and never sent back to the browser.
    pub secret: bool,
    #[serde(default)]
    pub optional: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub default: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub placeholder: Option<String>,
}

impl CredentialField {
    pub fn api_key(env: &[&str]) -> Self {
        Self {
            key: "api_key".into(),
            label: "API key".into(),
            env: env.iter().map(|s| s.to_string()).collect(),
            secret: true,
            optional: false,
            default: None,
            placeholder: None,
        }
    }
    pub fn base_url(default: &str, env: &[&str]) -> Self {
        Self {
            key: "base_url".into(),
            label: "Server URL".into(),
            env: env.iter().map(|s| s.to_string()).collect(),
            secret: false,
            optional: true,
            default: Some(default.into()),
            placeholder: Some(default.into()),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProviderInfo {
    pub id: String,
    pub name: String,
    pub kind: ProviderKind,
    pub fields: Vec<CredentialField>,
    /// Where users create a key.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub signup_url: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub docs_url: Option<String>,
    /// Most requests this provider should run at once.
    pub max_concurrency: usize,
}

/// Per-call context handed to a provider by the engine.
#[derive(Clone)]
pub struct Ctx {
    pub http: reqwest::Client,
    /// Resolved credential fields (stored value, else environment, else default).
    pub creds: HashMap<String, String>,
}

impl Ctx {
    pub fn get(&self, key: &str) -> Option<&str> {
        self.creds.get(key).map(String::as_str).filter(|s| !s.trim().is_empty())
    }
    pub fn require(&self, key: &str) -> ProviderResult<&str> {
        self.get(key).ok_or_else(|| ProviderError::Auth(format!("missing {key}; add it in Settings > Providers")))
    }
    pub fn api_key(&self) -> ProviderResult<&str> {
        self.require("api_key")
    }
    pub fn base_url(&self, default: &str) -> String {
        self.get("base_url").unwrap_or(default).trim_end_matches('/').to_string()
    }
}

/// Opaque provider-side handle stored with the job so polling survives restarts.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct RemoteHandle {
    pub id: String,
    /// Anything else the provider needs later (status URL, model id, region, ...).
    #[serde(default, skip_serializing_if = "Value::is_null")]
    pub data: Value,
}

impl RemoteHandle {
    pub fn new(id: impl Into<String>) -> Self {
        Self { id: id.into(), data: Value::Null }
    }
    pub fn with(id: impl Into<String>, data: Value) -> Self {
        Self { id: id.into(), data }
    }
    pub fn data_str(&self, key: &str) -> Option<&str> {
        self.data.get(key).and_then(Value::as_str)
    }
}

/// A finished output. URLs are downloaded by the engine with the given headers.
#[derive(Debug, Clone)]
pub enum Output {
    Url { url: String, headers: Vec<(String, String)> },
    Bytes { data: Vec<u8>, mime: String },
}

impl Output {
    pub fn url(url: impl Into<String>) -> Self {
        Self::Url { url: url.into(), headers: Vec::new() }
    }
}

#[derive(Debug, Clone)]
pub enum Submission {
    /// The provider queued the job; poll it with this handle.
    Pending(RemoteHandle),
    /// The provider answered synchronously.
    Done(Vec<Output>),
}

#[derive(Debug, Clone)]
pub enum PollStatus {
    Queued { position: Option<u32> },
    Running { progress: Option<f32>, message: Option<String> },
    Done(Vec<Output>),
    Failed(ProviderError),
}

/// A backend that runs generations. Implementations live in `pictesa-providers`.
#[async_trait]
pub trait Provider: Send + Sync + 'static {
    fn info(&self) -> ProviderInfo;

    /// Built-in catalog. Must be cheap: it is called on every model listing.
    fn models(&self) -> Vec<ModelSpec>;

    /// Optional live discovery (a local ComfyUI's workflows, a gateway's full catalog).
    async fn discover(&self, _ctx: &Ctx) -> ProviderResult<Vec<ModelSpec>> {
        Ok(Vec::new())
    }

    /// Fills in a discovered model's parameters and inputs (fetches its schema) when
    /// `complete` is false. Called once per model and cached by the engine.
    async fn resolve(&self, _ctx: &Ctx, model: &ModelSpec) -> ProviderResult<ModelSpec> {
        Ok(model.clone())
    }

    /// Verifies credentials or reachability with the cheapest call available.
    async fn check(&self, ctx: &Ctx) -> ProviderResult<()>;

    async fn submit(&self, ctx: &Ctx, model: &ModelSpec, req: &GenerationRequest) -> ProviderResult<Submission>;

    async fn poll(&self, _ctx: &Ctx, handle: &RemoteHandle) -> ProviderResult<PollStatus> {
        Err(ProviderError::NotSupported(format!("{} does not queue jobs (handle {})", self.info().id, handle.id)))
    }

    async fn cancel(&self, _ctx: &Ctx, _handle: &RemoteHandle) -> ProviderResult<()> {
        Ok(())
    }

    /// Delay before the next poll. Video providers can ask for longer waits.
    fn poll_interval(&self, attempt: u32) -> Duration {
        Duration::from_millis((800 + 400 * attempt as u64).min(5_000))
    }
}

/// Reads an HTTP response, turning error statuses into [`ProviderError`] and parsing JSON on success.
pub async fn json_response(resp: reqwest::Response) -> ProviderResult<Value> {
    let status = resp.status();
    let retry_after = resp
        .headers()
        .get(reqwest::header::RETRY_AFTER)
        .and_then(|v| v.to_str().ok())
        .and_then(|s| s.parse::<f64>().ok())
        .map(|s| (s * 1000.0) as u64);
    let text = resp.text().await.map_err(ProviderError::from)?;
    if !status.is_success() {
        return Err(ProviderError::from_http(status.as_u16(), &text, retry_after));
    }
    if text.trim().is_empty() {
        return Ok(Value::Null);
    }
    serde_json::from_str(&text).map_err(|e| ProviderError::Failed(format!("unexpected response ({e}): {}", crate::error::truncate(&text, 300))))
}

/// Finds a string at a JSON pointer, tolerating missing fields.
pub fn pointer_str<'a>(v: &'a Value, ptr: &str) -> Option<&'a str> {
    v.pointer(ptr).and_then(Value::as_str)
}

/// Collects every media URL in a provider answer (`images[].url`, `video.url`, `output: [..]`, ...),
/// in document order and without duplicates. Works for most gateway responses.
pub fn collect_media_urls(v: &Value, want: Option<MediaKind>) -> Vec<String> {
    fn looks_media(url: &str, want: Option<MediaKind>, hint: Option<&str>) -> bool {
        if !(url.starts_with("http://") || url.starts_with("https://") || url.starts_with("data:")) {
            return false;
        }
        let lower = url.split('?').next().unwrap_or(url).to_ascii_lowercase();
        let ext_kind = if [".png", ".jpg", ".jpeg", ".webp", ".gif", ".avif", ".bmp", ".tiff"].iter().any(|e| lower.ends_with(e)) {
            Some(MediaKind::Image)
        } else if [".mp4", ".webm", ".mov", ".mkv", ".m4v"].iter().any(|e| lower.ends_with(e)) {
            Some(MediaKind::Video)
        } else if [".mp3", ".wav", ".ogg", ".m4a", ".flac", ".aac"].iter().any(|e| lower.ends_with(e)) {
            Some(MediaKind::Audio)
        } else {
            hint.and_then(MediaKind::from_mime)
        };
        match (want, ext_kind) {
            (Some(w), Some(k)) => w == k,
            _ => true,
        }
    }
    fn walk(v: &Value, want: Option<MediaKind>, out: &mut Vec<String>, key: &str) {
        match v {
            Value::String(s) => {
                let keyed = key.is_empty() || key.contains("url") || key == "output" || key == "uri" || key == "video" || key == "image" || key == "audio";
                if keyed && looks_media(s, want, None) && !out.contains(s) {
                    out.push(s.clone());
                }
            }
            Value::Array(items) => items.iter().for_each(|i| walk(i, want, out, key)),
            Value::Object(map) => {
                let hint = map.get("content_type").or_else(|| map.get("mime_type")).and_then(Value::as_str);
                if let Some(Value::String(u)) = map.get("url") {
                    if looks_media(u, want, hint) && !out.contains(u) {
                        out.push(u.clone());
                    }
                }
                for (k, child) in map {
                    if k == "url" { continue; }
                    // Skip echoed inputs so a reference image is never mistaken for a result.
                    if k == "input" || k.ends_with("_input") || k == "logs" { continue; }
                    walk(child, want, out, k);
                }
            }
            _ => {}
        }
    }
    let mut out = Vec::new();
    walk(v, want, &mut out, "");
    out
}
