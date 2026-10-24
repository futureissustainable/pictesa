//! Pictesa core: the shared vocabulary between the engine, the server and every provider.
//!
//! A *provider* is a backend that can run generations (fal, OpenAI, a local ComfyUI, ...).
//! A *model* is one thing a provider can run, described by [`ModelSpec`]: which media it
//! produces, which media it accepts and which parameters it exposes. The UI, the CLI and the
//! MCP tools are all generated from these specs, so adding a model never needs UI code.

pub mod error;
pub mod model;
pub mod provider;
pub mod request;
pub mod schema;

pub use error::ProviderError;
pub use model::*;
pub use provider::*;
pub use request::*;

/// Milliseconds since the Unix epoch.
pub fn now_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or_default()
}

/// Splits a full model id `provider:model` into its two parts.
pub fn split_model_id(full: &str) -> Option<(&str, &str)> {
    let (provider, model) = full.split_once(':')?;
    (!provider.is_empty() && !model.is_empty()).then_some((provider, model))
}
