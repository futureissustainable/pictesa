use std::path::PathBuf;

use base64::Engine as _;
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

use crate::{InputRole, MediaKind, ProviderError};

/// An input file resolved by the engine to a local path. Providers read it, inline it or upload it.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MediaInput {
    pub role: InputRole,
    pub kind: MediaKind,
    pub mime: String,
    pub path: PathBuf,
    /// A public URL for the same file, when one exists (hosted mode or imported from the web).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub url: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub width: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub height: Option<u32>,
}

impl MediaInput {
    pub async fn bytes(&self) -> Result<Vec<u8>, ProviderError> {
        tokio_read(&self.path).await
    }
    pub async fn base64(&self) -> Result<String, ProviderError> {
        Ok(base64::engine::general_purpose::STANDARD.encode(self.bytes().await?))
    }
    pub async fn data_uri(&self) -> Result<String, ProviderError> {
        Ok(format!("data:{};base64,{}", self.mime, self.base64().await?))
    }
    /// The public URL when present, else an inline data URI.
    pub async fn url_or_data_uri(&self) -> Result<String, ProviderError> {
        match &self.url {
            Some(u) => Ok(u.clone()),
            None => self.data_uri().await,
        }
    }
    pub fn file_name(&self) -> String {
        self.path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_else(|| "input".into())
    }
}

async fn tokio_read(path: &std::path::Path) -> Result<Vec<u8>, ProviderError> {
    tokio::fs::read(path).await.map_err(|e| ProviderError::InvalidInput(format!("cannot read input file: {e}")))
}

/// Everything a provider needs to run one generation.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GenerationRequest {
    pub job_id: String,
    /// Provider-side model id, without the `provider:` prefix.
    pub model: String,
    #[serde(default)]
    pub prompt: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub negative_prompt: Option<String>,
    /// Shared-vocabulary parameters, already validated against the model spec and filled with defaults.
    #[serde(default)]
    pub params: Map<String, Value>,
    #[serde(default)]
    pub inputs: Vec<MediaInput>,
    /// Outputs wanted from this single request (never above the model's `max_outputs`).
    #[serde(default = "one")]
    pub num_outputs: u32,
}

fn one() -> u32 {
    1
}

impl GenerationRequest {
    pub fn str(&self, key: &str) -> Option<&str> {
        self.params.get(key).and_then(Value::as_str)
    }
    pub fn i64(&self, key: &str) -> Option<i64> {
        self.params.get(key).and_then(|v| v.as_i64().or_else(|| v.as_str().and_then(|s| s.parse().ok())))
    }
    pub fn u64(&self, key: &str) -> Option<u64> {
        self.params.get(key).and_then(|v| v.as_u64().or_else(|| v.as_str().and_then(|s| s.parse().ok())))
    }
    pub fn f64(&self, key: &str) -> Option<f64> {
        self.params.get(key).and_then(Value::as_f64)
    }
    pub fn bool(&self, key: &str) -> Option<bool> {
        self.params.get(key).and_then(Value::as_bool)
    }
    pub fn inputs(&self, role: InputRole) -> impl Iterator<Item = &MediaInput> {
        self.inputs.iter().filter(move |i| i.role == role)
    }
    pub fn input(&self, role: InputRole) -> Option<&MediaInput> {
        self.inputs(role).next()
    }
    /// Copies the listed shared parameters into a provider body, renaming them on the way.
    /// `pairs` holds `(shared_key, provider_key)`.
    pub fn copy_params(&self, body: &mut Map<String, Value>, pairs: &[(&str, &str)]) {
        for (from, to) in pairs {
            if let Some(v) = self.params.get(*from) {
                body.insert((*to).to_string(), v.clone());
            }
        }
    }
}
