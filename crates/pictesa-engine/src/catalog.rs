//! The model catalog: built-in specs plus live discovery from gateways and local servers.
//! Discovered lists and resolved specs are cached in SQLite so startup is instant.

use std::{
    collections::HashMap,
    sync::{Arc, RwLock},
};

use pictesa_core::{Ctx, ModelSpec, Provider};
use sqlx::SqlitePool;

/// Discovered catalogs older than this are refreshed in the background.
pub const CATALOG_TTL_MS: i64 = 24 * 3600 * 1000;
/// Resolved model schemas are refreshed after a week.
pub const RESOLVED_TTL_MS: i64 = 7 * 24 * 3600 * 1000;

pub struct Catalog {
    providers: Vec<Arc<dyn Provider>>,
    discovered: RwLock<HashMap<String, (i64, Vec<ModelSpec>)>>,
    resolved: RwLock<HashMap<String, ModelSpec>>,
}

impl Catalog {
    pub fn new(providers: Vec<Arc<dyn Provider>>) -> Self {
        Self { providers, discovered: RwLock::new(HashMap::new()), resolved: RwLock::new(HashMap::new()) }
    }

    pub fn providers(&self) -> &[Arc<dyn Provider>] {
        &self.providers
    }

    pub fn provider(&self, id: &str) -> Option<Arc<dyn Provider>> {
        self.providers.iter().find(|p| p.info().id == id).cloned()
    }

    pub async fn load_cache(&self, db: &SqlitePool) {
        let rows: Vec<(String, String, String, i64)> =
            sqlx::query_as("SELECT provider, scope, data, fetched_at FROM model_cache").fetch_all(db).await.unwrap_or_default();
        let mut discovered = self.discovered.write().unwrap();
        let mut resolved = self.resolved.write().unwrap();
        for (provider, scope, data, at) in rows {
            if scope == "catalog" {
                if let Ok(list) = serde_json::from_str::<Vec<ModelSpec>>(&data) {
                    discovered.insert(provider, (at, list));
                }
            } else if pictesa_core::now_ms() - at < RESOLVED_TTL_MS {
                if let Ok(spec) = serde_json::from_str::<ModelSpec>(&data) {
                    resolved.insert(scope, spec);
                }
            }
        }
    }

    /// Every model of one provider: built-in specs first, then discovered ones not already listed.
    pub fn provider_models(&self, provider: &dyn Provider) -> Vec<ModelSpec> {
        let id = provider.info().id;
        let mut list = provider.models();
        let discovered = self.discovered.read().unwrap();
        if let Some((_, found)) = discovered.get(&id) {
            let known: std::collections::HashSet<String> = list.iter().map(|m| m.id.clone()).collect();
            list.extend(found.iter().filter(|m| !known.contains(&m.id)).cloned());
        }
        let resolved = self.resolved.read().unwrap();
        for m in list.iter_mut() {
            if !m.complete {
                if let Some(full) = resolved.get(&m.id) {
                    let tags = std::mem::take(&mut m.tags);
                    *m = full.clone();
                    for t in tags {
                        if !m.tags.contains(&t) {
                            m.tags.push(t);
                        }
                    }
                }
            }
        }
        list
    }

    pub fn all(&self) -> Vec<ModelSpec> {
        self.providers.iter().flat_map(|p| self.provider_models(p.as_ref())).collect()
    }

    pub fn find(&self, id: &str) -> Option<ModelSpec> {
        let (pid, _) = pictesa_core::split_model_id(id)?;
        let provider = self.provider(pid)?;
        if let Some(m) = self.resolved.read().unwrap().get(id) {
            return Some(m.clone());
        }
        self.provider_models(provider.as_ref()).into_iter().find(|m| m.id == id)
    }

    /// Returns the full spec, fetching the schema when the model was only discovered.
    pub async fn resolve(&self, db: &SqlitePool, ctx: &Ctx, id: &str) -> anyhow::Result<ModelSpec> {
        let spec = self.find(id).ok_or_else(|| anyhow::anyhow!("unknown model {id}"))?;
        if spec.complete {
            return Ok(spec);
        }
        let provider = self.provider(&spec.provider).ok_or_else(|| anyhow::anyhow!("unknown provider {}", spec.provider))?;
        let mut full = provider.resolve(ctx, &spec).await.map_err(|e| anyhow::anyhow!("{e}"))?;
        full.complete = true;
        for t in &spec.tags {
            if !full.tags.contains(t) {
                full.tags.push(t.clone());
            }
        }
        let data = serde_json::to_string(&full)?;
        sqlx::query("INSERT OR REPLACE INTO model_cache (provider, scope, data, fetched_at) VALUES (?, ?, ?, ?)")
            .bind(&full.provider)
            .bind(&full.id)
            .bind(data)
            .bind(pictesa_core::now_ms())
            .execute(db)
            .await?;
        self.resolved.write().unwrap().insert(full.id.clone(), full.clone());
        Ok(full)
    }

    pub fn catalog_age(&self, provider: &str) -> Option<i64> {
        self.discovered.read().unwrap().get(provider).map(|(at, _)| pictesa_core::now_ms() - at)
    }

    /// Runs discovery for one provider and stores the result. Returns the model count.
    pub async fn refresh(&self, db: &SqlitePool, provider: &dyn Provider, ctx: &Ctx) -> anyhow::Result<usize> {
        let found = provider.discover(ctx).await.map_err(|e| anyhow::anyhow!("{e}"))?;
        let n = found.len();
        let now = pictesa_core::now_ms();
        sqlx::query("INSERT OR REPLACE INTO model_cache (provider, scope, data, fetched_at) VALUES (?, 'catalog', ?, ?)")
            .bind(provider.info().id)
            .bind(serde_json::to_string(&found)?)
            .bind(now)
            .execute(db)
            .await?;
        self.discovered.write().unwrap().insert(provider.info().id, (now, found));
        Ok(n)
    }
}
