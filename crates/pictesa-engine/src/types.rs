//! Shapes returned by the engine. They serialize exactly as documented in docs/API.md.

use pictesa_core::{CredentialField, InputRole, MediaKind, ModelSpec, ProviderKind};
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum JobStatus {
    Queued,
    Running,
    Succeeded,
    Failed,
    Canceled,
}

impl JobStatus {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Queued => "queued",
            Self::Running => "running",
            Self::Succeeded => "succeeded",
            Self::Failed => "failed",
            Self::Canceled => "canceled",
        }
    }
    pub fn parse(s: &str) -> Self {
        match s {
            "running" => Self::Running,
            "succeeded" => Self::Succeeded,
            "failed" => Self::Failed,
            "canceled" => Self::Canceled,
            _ => Self::Queued,
        }
    }
    pub fn is_final(self) -> bool {
        matches!(self, Self::Succeeded | Self::Failed | Self::Canceled)
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct JobInput {
    pub asset_id: String,
    pub role: InputRole,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct JobError {
    pub kind: String,
    pub message: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Job {
    pub id: String,
    pub batch_id: String,
    pub model: String,
    pub provider: String,
    pub model_name: String,
    pub output: MediaKind,
    pub status: JobStatus,
    pub prompt: String,
    pub negative_prompt: Option<String>,
    pub params: Map<String, Value>,
    pub inputs: Vec<JobInput>,
    pub preset: Option<String>,
    pub count: u32,
    pub progress: Option<f64>,
    pub message: Option<String>,
    pub error: Option<JobError>,
    pub cost_usd: Option<f64>,
    pub attempts: i64,
    pub created_at: i64,
    pub started_at: Option<i64>,
    pub finished_at: Option<i64>,
    pub outputs: Vec<Asset>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Asset {
    pub id: String,
    pub kind: MediaKind,
    pub mime: String,
    pub width: Option<i64>,
    pub height: Option<i64>,
    pub duration_ms: Option<i64>,
    pub size: i64,
    pub url: String,
    pub thumb_url: Option<String>,
    pub source: String,
    pub job_id: Option<String>,
    pub model: Option<String>,
    pub prompt: Option<String>,
    pub favorite: bool,
    pub created_at: i64,
    pub deleted_at: Option<i64>,
    /// Original file name for uploads and imports.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    /// Edit helpers (source `edit`): the image this mask or expanded canvas was made from.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub origin_id: Option<String>,
}

/// Body of `POST /api/generations`.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct CreateGeneration {
    pub model: String,
    #[serde(default)]
    pub prompt: String,
    #[serde(default)]
    pub negative_prompt: Option<String>,
    #[serde(default)]
    pub params: Map<String, Value>,
    #[serde(default)]
    pub inputs: Vec<JobInput>,
    #[serde(default)]
    pub count: Option<u32>,
    #[serde(default)]
    pub preset: Option<String>,
    /// Also run the same request on these models (compare). Params and inputs are adapted to each.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub compare: Vec<String>,
    /// Attach a saved character's images as reference inputs.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub character: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Created {
    pub batch_id: String,
    pub jobs: Vec<Job>,
}

#[derive(Debug, Clone, Default, Deserialize)]
pub struct JobFilter {
    pub status: Option<String>,
    pub output: Option<String>,
    pub batch: Option<String>,
    pub limit: Option<u32>,
    pub before: Option<String>,
}

#[derive(Debug, Clone, Default, Deserialize)]
pub struct AssetFilter {
    pub kind: Option<String>,
    pub favorite: Option<bool>,
    pub q: Option<String>,
    pub model: Option<String>,
    pub source: Option<String>,
    pub trash: Option<bool>,
    pub limit: Option<u32>,
    pub before: Option<String>,
}

#[derive(Debug, Clone, Default, Deserialize)]
pub struct ModelFilter {
    pub output: Option<String>,
    pub task: Option<String>,
    pub provider: Option<String>,
    pub q: Option<String>,
    pub configured: Option<bool>,
    pub featured: Option<bool>,
    /// "arena" (best rated first), "newest" or "name". Default: set-up and featured models first.
    pub sort: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct ModelView {
    #[serde(flatten)]
    pub spec: ModelSpec,
    pub configured: bool,
    /// Runs are paid from the wallet (hosted mode on platform keys); `pricing` already includes the markup.
    #[serde(default)]
    pub billed: bool,
    /// LMArena standings in the arenas that rate this model's tasks.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub arena: Vec<crate::leaderboard::ArenaScore>,
    /// When the provider published the model (YYYY-MM-DD), if it says.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub released: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct ModelPrefs {
    /// Models the user pinned, in pin order.
    pub pinned: Vec<String>,
    /// Models of the user's latest generations, newest first.
    pub recent: Vec<String>,
}

impl ModelView {
    /// The model's rating in the arena for `task`, or its first arena.
    pub fn rating(&self, task: Option<pictesa_core::Task>) -> Option<f64> {
        let key = task.and_then(|t| crate::leaderboard::ARENAS.iter().find(|(_, at)| *at == t)).map(|(k, _)| *k);
        self.arena.iter().find(|s| key.is_none_or(|k| s.arena == k)).map(|s| s.rating)
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct FieldState {
    pub set: bool,
    /// "stored", "env" or "default".
    #[serde(skip_serializing_if = "Option::is_none")]
    pub source: Option<&'static str>,
    /// Only for non-secret fields.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub value: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct ProviderState {
    pub id: String,
    pub name: String,
    pub kind: ProviderKind,
    pub fields: Vec<CredentialField>,
    pub signup_url: Option<String>,
    pub docs_url: Option<String>,
    pub configured: bool,
    pub values: Map<String, Value>,
    pub model_count: usize,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Preset {
    pub id: String,
    pub name: String,
    pub category: String,
    pub output: MediaKind,
    #[serde(default)]
    pub prompt_suffix: String,
    #[serde(default)]
    pub params: Map<String, Value>,
    #[serde(default)]
    pub description: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct User {
    pub id: String,
    pub email: Option<String>,
    pub name: Option<String>,
    pub role: String,
    pub created_at: i64,
}

#[derive(Debug, Clone, Serialize)]
pub struct TokenInfo {
    pub id: String,
    pub name: String,
    pub created_at: i64,
    pub last_used_at: Option<i64>,
}

#[derive(Debug, Clone, Serialize)]
pub struct NewToken {
    pub id: String,
    pub name: String,
    pub token: String,
}
