//! Provider credentials and the provider list shown in Settings.
//!
//! Local mode: a field resolves to the stored value, else an environment variable, else its default.
//! Hosted mode never mixes sources, so a user value can never be paired with an operator key:
//! - `byok`: the user stored a secret for this provider, so only the user's values (and defaults) are used;
//! - `platform`: otherwise, when billing is on, only the operator's environment values are used and
//!   every run is charged to the wallet;
//! - neither: the provider is not available to that user.

use std::collections::HashMap;

use pictesa_core::{Ctx, Provider};
use serde_json::{json, Map, Value};

use crate::{Engine, EngineError, ProviderState, Result};

#[derive(Debug, Clone)]
pub(crate) struct Resolved {
    pub values: HashMap<String, (String, &'static str)>,
    /// `local`, `byok` or `platform`: how runs with these values are paid.
    pub billing: &'static str,
}

/// Which credential sources a resolution may use.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Sources {
    /// Local mode: stored, then environment, then defaults.
    Any,
    /// Hosted, bring your own key: stored values and defaults only.
    Stored,
    /// Hosted, operator keys: environment values and defaults only.
    Platform,
}

impl Sources {
    pub(crate) fn for_billing(billing: &str) -> Self {
        match billing {
            "byok" => Self::Stored,
            "platform" => Self::Platform,
            _ => Self::Any,
        }
    }
}

impl Engine {
    pub(crate) async fn stored_fields(&self, user: &str, provider: &str) -> Result<HashMap<String, String>> {
        let rows: Vec<(String, String, bool)> =
            sqlx::query_as("SELECT field, value, encrypted FROM credentials WHERE user_id = ? AND provider = ?")
                .bind(user)
                .bind(provider)
                .fetch_all(&self.inner.db)
                .await?;
        let mut out = HashMap::new();
        for (field, value, encrypted) in rows {
            let plain = if encrypted { self.inner.crypto.decrypt(&value).map_err(EngineError::Internal)? } else { value };
            out.insert(field, plain);
        }
        Ok(out)
    }

    /// The sources a user's runs on this provider would use right now.
    pub(crate) async fn sources_for(&self, user: &str, provider: &dyn Provider) -> Result<Sources> {
        if self.mode() == crate::Mode::Local {
            return Ok(Sources::Any);
        }
        let info = provider.info();
        let stored = self.stored_fields(user, &info.id).await?;
        let own_secret = info.fields.iter().any(|f| f.secret && stored.get(&f.key).is_some_and(|v| !v.trim().is_empty()));
        Ok(if own_secret || !self.platform_billing() { Sources::Stored } else { Sources::Platform })
    }

    pub(crate) async fn resolve_fields(&self, user: &str, provider: &dyn Provider) -> Result<Resolved> {
        let sources = self.sources_for(user, provider).await?;
        self.resolve_with(user, provider, sources).await
    }

    pub(crate) async fn resolve_with(&self, user: &str, provider: &dyn Provider, sources: Sources) -> Result<Resolved> {
        let info = provider.info();
        let stored = if sources == Sources::Platform { HashMap::new() } else { self.stored_fields(user, &info.id).await? };
        // In hosted mode a local backend's default address is the operator's own machine: only an
        // address the operator set explicitly counts.
        let defaults_ok = !(self.mode() == crate::Mode::Hosted && info.kind == pictesa_core::ProviderKind::Local);
        let mut values = HashMap::new();
        for f in &info.fields {
            let env = || f.env.iter().find_map(|e| std::env::var(e).ok()).filter(|v| !v.trim().is_empty());
            if let Some(v) = stored.get(&f.key).filter(|v| !v.trim().is_empty()) {
                values.insert(f.key.clone(), (v.clone(), "stored"));
            } else if let Some(v) = (sources != Sources::Stored).then(env).flatten() {
                values.insert(f.key.clone(), (v, if sources == Sources::Platform { "platform" } else { "env" }));
            } else if let Some(d) = f.default.as_ref().filter(|_| defaults_ok) {
                values.insert(f.key.clone(), (d.clone(), "default"));
            }
        }
        let billing = match sources {
            Sources::Any => "local",
            Sources::Stored => "byok",
            Sources::Platform => "platform",
        };
        Ok(Resolved { values, billing })
    }

    /// Builds the per-call context for a provider and user.
    pub async fn ctx_for(&self, user: &str, provider: &dyn Provider) -> Result<Ctx> {
        let resolved = self.resolve_fields(user, provider).await?;
        Ok(self.ctx_from(resolved))
    }

    pub(crate) fn ctx_from(&self, resolved: Resolved) -> Ctx {
        let guard = self.guarded(&resolved);
        let http = if guard { self.inner.http_guarded.clone() } else { self.inner.http.clone() };
        Ctx { http, creds: resolved.values.into_iter().map(|(k, (v, _))| (k, v)).collect() }
    }

    /// Hosted runs against an endpoint a user chose must not reach private addresses.
    pub(crate) fn guarded(&self, resolved: &Resolved) -> bool {
        self.mode() == crate::Mode::Hosted && resolved.values.iter().any(|(k, (v, src))| *src == "stored" && (k.contains("url") || v.contains("://")))
    }

    pub(crate) fn is_configured(info: &pictesa_core::ProviderInfo, resolved: &Resolved) -> bool {
        info.fields.iter().all(|f| f.optional || resolved.values.contains_key(&f.key))
    }

    /// True when a run would use the operator's keys (and so be charged to the wallet).
    pub(crate) fn uses_platform(_info: &pictesa_core::ProviderInfo, resolved: &Resolved) -> bool {
        resolved.billing == "platform"
    }

    pub async fn provider_state(&self, user: &str, provider: &dyn Provider) -> Result<ProviderState> {
        let info = provider.info();
        let resolved = self.resolve_fields(user, provider).await?;
        let mut values = Map::new();
        for f in &info.fields {
            let state = match resolved.values.get(&f.key) {
                Some((v, source)) => json!({ "set": true, "source": source, "value": if f.secret { Value::Null } else { json!(v) } }),
                None => json!({ "set": false }),
            };
            values.insert(f.key.clone(), state);
        }
        let configured = Self::is_configured(&info, &resolved);
        let model_count = self.inner.catalog.provider_models(provider).len();
        Ok(ProviderState {
            id: info.id,
            name: info.name,
            kind: info.kind,
            fields: info.fields,
            signup_url: info.signup_url,
            docs_url: info.docs_url,
            configured,
            values,
            model_count,
        })
    }

    pub async fn providers(&self, user: &str) -> Result<Vec<ProviderState>> {
        let mut out = Vec::new();
        for p in self.inner.catalog.providers() {
            out.push(self.provider_state(user, p.as_ref()).await?);
        }
        Ok(out)
    }

    fn provider_or_404(&self, id: &str) -> Result<std::sync::Arc<dyn Provider>> {
        self.inner.catalog.provider(id).ok_or_else(|| EngineError::not_found("provider"))
    }

    /// Stores (or clears, with an empty string) credential fields. Unknown fields are rejected.
    pub async fn set_credentials(&self, user: &str, provider_id: &str, values: &Map<String, Value>) -> Result<ProviderState> {
        let provider = self.provider_or_404(provider_id)?;
        let info = provider.info();
        let now = pictesa_core::now_ms();
        for (key, value) in values {
            let field = info
                .fields
                .iter()
                .find(|f| &f.key == key)
                .ok_or_else(|| EngineError::invalid(format!("{} has no field {key}", info.name), Some(key)))?;
            let text = match value {
                Value::Null => String::new(),
                Value::String(s) => s.trim().to_string(),
                other => other.to_string(),
            };
            if text.is_empty() {
                sqlx::query("DELETE FROM credentials WHERE user_id = ? AND provider = ? AND field = ?")
                    .bind(user)
                    .bind(provider_id)
                    .bind(key)
                    .execute(&self.inner.db)
                    .await?;
                continue;
            }
            if text.len() > 8192 {
                return Err(EngineError::invalid(format!("{} is too long", field.label), Some(key)));
            }
            // Hosted users may not point the server at its own files.
            if self.mode() == crate::Mode::Hosted && !field.secret && (key.contains("dir") || key.contains("path")) {
                return Err(EngineError::invalid(format!("{} is managed by the server operator.", field.label), Some(key)));
            }
            // Hosted users may only point providers at public HTTPS servers.
            if self.mode() == crate::Mode::Hosted && !field.secret && (text.contains("://") || key.contains("url")) {
                let url = url::Url::parse(&text).map_err(|_| EngineError::invalid(format!("{} must be a URL", field.label), Some(key)))?;
                if url.scheme() != "https" {
                    return Err(EngineError::invalid(format!("{} must use https", field.label), Some(key)));
                }
                crate::netguard::public_address(&url).await.map_err(|e| EngineError::invalid(format!("{}: {e}", field.label), Some(key)))?;
            }
            let (stored, encrypted) = if field.secret { (self.inner.crypto.encrypt(&text)?, true) } else { (text, false) };
            sqlx::query("INSERT OR REPLACE INTO credentials (user_id, provider, field, value, encrypted, updated_at) VALUES (?, ?, ?, ?, ?, ?)")
                .bind(user)
                .bind(provider_id)
                .bind(key)
                .bind(stored)
                .bind(encrypted)
                .bind(now)
                .execute(&self.inner.db)
                .await?;
        }
        // A new key can unlock discovery (gateway catalogs), so refresh in the background.
        // Hosted catalogs are shared, so only the operator's configuration may shape them.
        if self.mode() == crate::Mode::Local {
            let engine = self.clone();
            let (owner, pid) = (user.to_string(), provider_id.to_string());
            tokio::spawn(async move {
                let _ = engine.refresh_models_for(&owner, &pid).await;
            });
        }
        self.provider_state(user, provider.as_ref()).await
    }

    pub async fn clear_credentials(&self, user: &str, provider_id: &str) -> Result<()> {
        self.provider_or_404(provider_id)?;
        sqlx::query("DELETE FROM credentials WHERE user_id = ? AND provider = ?").bind(user).bind(provider_id).execute(&self.inner.db).await?;
        Ok(())
    }

    /// Runs the provider's cheapest authenticated call.
    pub async fn check_provider(&self, user: &str, provider_id: &str) -> Result<std::result::Result<(), String>> {
        let provider = self.provider_or_404(provider_id)?;
        let ctx = self.ctx_for(user, provider.as_ref()).await?;
        Ok(provider.check(&ctx).await.map_err(|e| match self.mode() {
            crate::Mode::Local => e.to_string(),
            // Hosted: never echo a remote server's answer back to the user.
            crate::Mode::Hosted => match e.kind() {
                "auth" => "The key was rejected.".to_string(),
                "insufficient_credits" => "The account has no credits left.".to_string(),
                "rate_limited" => "The provider is rate limiting; try again soon.".to_string(),
                _ => "The provider could not be reached with these settings.".to_string(),
            },
        }))
    }

    /// Context for shared catalog work (discovery and schema loading). Hosted catalogs are shared
    /// by every user, so only the operator's configuration may shape them.
    pub(crate) async fn catalog_ctx(&self, user: &str, provider: &dyn Provider) -> Result<Ctx> {
        match self.mode() {
            crate::Mode::Local => self.ctx_for(user, provider).await,
            crate::Mode::Hosted => Ok(self.ctx_from(self.resolve_with("", provider, Sources::Platform).await?)),
        }
    }
}
