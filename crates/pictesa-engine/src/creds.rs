//! Provider credentials and the provider list shown in Settings.
//!
//! A field resolves to: the user's stored value, else an environment variable, else the
//! field default. In hosted mode the server operator's environment keys act as platform keys
//! for every user, which is how "run it through us" works without users bringing keys.

use std::collections::HashMap;

use pictesa_core::{Ctx, Provider};
use serde_json::{json, Map, Value};

use crate::{Engine, EngineError, ProviderState, Result};

#[derive(Debug, Clone)]
pub(crate) struct Resolved {
    pub values: HashMap<String, (String, &'static str)>,
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

    pub(crate) async fn resolve_fields(&self, user: &str, provider: &dyn Provider) -> Result<Resolved> {
        let info = provider.info();
        let stored = self.stored_fields(user, &info.id).await?;
        let mut values = HashMap::new();
        for f in &info.fields {
            if let Some(v) = stored.get(&f.key).filter(|v| !v.trim().is_empty()) {
                values.insert(f.key.clone(), (v.clone(), "stored"));
            } else if let Some(v) = f.env.iter().find_map(|e| std::env::var(e).ok()).filter(|v| !v.trim().is_empty()) {
                values.insert(f.key.clone(), (v, "env"));
            } else if let Some(d) = &f.default {
                values.insert(f.key.clone(), (d.clone(), "default"));
            }
        }
        Ok(Resolved { values })
    }

    /// Builds the per-call context for a provider and user.
    pub async fn ctx_for(&self, user: &str, provider: &dyn Provider) -> Result<Ctx> {
        let resolved = self.resolve_fields(user, provider).await?;
        Ok(Ctx { http: self.inner.http.clone(), creds: resolved.values.into_iter().map(|(k, (v, _))| (k, v)).collect() })
    }

    pub(crate) fn is_configured(info: &pictesa_core::ProviderInfo, resolved: &Resolved) -> bool {
        info.fields.iter().all(|f| f.optional || resolved.values.contains_key(&f.key))
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
        let engine = self.clone();
        let (owner, pid) = (user.to_string(), provider_id.to_string());
        tokio::spawn(async move {
            let _ = engine.refresh_models_for(&owner, &pid).await;
        });
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
        Ok(provider.check(&ctx).await.map_err(|e| e.to_string()))
    }
}
