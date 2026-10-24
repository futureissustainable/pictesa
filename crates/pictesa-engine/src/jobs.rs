//! Models, generations and the asset library: the operations behind the API, CLI and MCP tools.

use std::{collections::HashMap, time::Duration};

use pictesa_core::{keys, now_ms, ModelSpec, Pricing, Task};
use serde_json::{json, Map, Value};

use crate::{
    files::Stored,
    store::{self, AssetRow, JobRow},
    Asset, AssetFilter, CreateGeneration, Created, Engine, EngineError, Job, JobFilter, ModelFilter, ModelView, Result,
};

pub const MAX_COUNT: u32 = 16;

impl Engine {
    // ---------- models ----------

    pub async fn models(&self, user: &str, f: &ModelFilter) -> Result<Vec<ModelView>> {
        let mut configured = HashMap::new();
        for p in self.inner.catalog.providers() {
            let info = p.info();
            let resolved = self.resolve_fields(user, p.as_ref()).await?;
            configured.insert(info.id.clone(), Self::is_configured(&info, &resolved));
        }
        let q = f.q.as_deref().map(str::to_lowercase).filter(|q| !q.is_empty());
        let task: Option<Task> = f.task.as_deref().and_then(|t| serde_json::from_value(json!(t)).ok());
        let mut out: Vec<ModelView> = self
            .inner
            .catalog
            .all()
            .into_iter()
            .filter(|m| f.output.as_deref().is_none_or(|o| m.output.as_str() == o))
            .filter(|m| f.provider.as_deref().is_none_or(|p| m.provider == p))
            .filter(|m| task.is_none_or(|t| m.tasks.contains(&t)))
            .filter(|m| f.featured != Some(true) || m.tags.iter().any(|t| t == "featured"))
            .filter(|m| {
                q.as_deref().is_none_or(|q| {
                    m.name.to_lowercase().contains(q) || m.id.to_lowercase().contains(q) || m.vendor.as_deref().is_some_and(|v| v.to_lowercase().contains(q))
                })
            })
            .map(|spec| {
                let ok = configured.get(&spec.provider).copied().unwrap_or(false);
                ModelView { spec, configured: ok }
            })
            .filter(|v| f.configured != Some(true) || v.configured)
            .collect();
        out.sort_by_key(|v| (!v.configured, !v.spec.tags.iter().any(|t| t == "featured")));
        Ok(out)
    }

    /// Full spec for one model (fetches its schema on first use).
    pub async fn model(&self, user: &str, id: &str) -> Result<ModelSpec> {
        let spec = self.inner.catalog.find(id).ok_or_else(|| EngineError::not_found("model"))?;
        if spec.complete {
            return Ok(spec);
        }
        let provider = self.inner.catalog.provider(&spec.provider).ok_or_else(|| EngineError::not_found("provider"))?;
        let ctx = self.ctx_for(user, provider.as_ref()).await?;
        self.inner.catalog.resolve(&self.inner.db, &ctx, id).await.map_err(|e| EngineError::invalid(format!("could not load {id}: {e:#}"), Some("model")))
    }

    pub async fn refresh_models_for(&self, user: &str, provider_id: &str) -> Result<usize> {
        let provider = self.inner.catalog.provider(provider_id).ok_or_else(|| EngineError::not_found("provider"))?;
        let ctx = self.ctx_for(user, provider.as_ref()).await?;
        self.inner.catalog.refresh(&self.inner.db, provider.as_ref(), &ctx).await.map_err(EngineError::Internal)
    }

    /// Re-runs discovery everywhere it can work. Failures (no key, server down) are skipped.
    pub async fn refresh_models(&self, user: &str) -> usize {
        let mut total = 0;
        for p in self.inner.catalog.providers().to_vec() {
            match self.refresh_models_for(user, &p.info().id).await {
                Ok(n) => total += n,
                Err(e) => tracing::debug!("discovery skipped for {}: {e}", p.info().id),
            }
        }
        total
    }

    pub(crate) async fn refresh_stale_catalogs(&self, user: &str) {
        for p in self.inner.catalog.providers().to_vec() {
            let id = p.info().id;
            let stale = self.inner.catalog.catalog_age(&id).is_none_or(|age| age > crate::catalog::CATALOG_TTL_MS);
            if stale {
                if let Err(e) = self.refresh_models_for(user, &id).await {
                    tracing::debug!("discovery skipped for {id}: {e}");
                }
            }
        }
    }

    // ---------- generations ----------

    pub async fn create_generation(&self, user: &str, mut req: CreateGeneration) -> Result<Created> {
        let spec = self.model(user, &req.model).await?;
        let provider = self.inner.catalog.provider(&spec.provider).ok_or_else(|| EngineError::not_found("provider"))?;
        let info = provider.info();
        let resolved = self.resolve_fields(user, provider.as_ref()).await?;
        if !Self::is_configured(&info, &resolved) {
            return Err(EngineError::invalid(format!("{} is not set up yet. Add its key in Settings > Providers.", info.name), Some("model")));
        }

        if let Some(pid) = req.preset.clone().filter(|p| !p.is_empty()) {
            let preset = self.preset(&pid).ok_or_else(|| EngineError::invalid(format!("unknown preset {pid}"), Some("preset")))?;
            if !preset.prompt_suffix.is_empty() {
                req.prompt = if req.prompt.trim().is_empty() { preset.prompt_suffix.clone() } else { format!("{}, {}", req.prompt.trim_end(), preset.prompt_suffix) };
            }
            for (k, v) in &preset.params {
                if spec.param_spec(k).is_some() && !req.params.contains_key(k) {
                    req.params.insert(k.clone(), v.clone());
                }
            }
        }

        if req.prompt.chars().count() > 20_000 {
            return Err(EngineError::invalid("The prompt is longer than 20,000 characters.", Some("prompt")));
        }
        let needs_prompt = spec.prompt && spec.tasks.iter().any(|t| matches!(t, Task::TextToImage | Task::TextToVideo | Task::TextToAudio)) && req.inputs.is_empty();
        if needs_prompt && req.prompt.trim().is_empty() {
            return Err(EngineError::invalid("Describe what to generate.", Some("prompt")));
        }

        let params = validate_params(&spec, &req.params)?;
        let inputs = self.validate_inputs(user, &spec, &req.inputs).await?;
        let count = req.count.unwrap_or(1).clamp(1, MAX_COUNT);
        let negative = req.negative_prompt.filter(|n| spec.negative_prompt && !n.trim().is_empty());

        let batch_id = crate::new_id();
        self.inner.owned_batches.lock().unwrap().insert(batch_id.clone());
        let now = now_ms();
        let params_json = serde_json::to_string(&params).map_err(anyhow::Error::from)?;
        let inputs_json = serde_json::to_string(&inputs).map_err(anyhow::Error::from)?;
        let mut jobs = Vec::new();
        let mut remaining = count;
        while remaining > 0 {
            let n = remaining.min(spec.max_outputs.max(1));
            remaining -= n;
            let id = crate::new_id();
            let cost = estimate_cost(&spec, &params, n);
            sqlx::query(
                "INSERT INTO jobs (id, user_id, batch_id, model, provider, model_name, output, status, prompt, negative_prompt, params, inputs, preset, count, cost_usd, created_at, updated_at)
                 VALUES (?, ?, ?, ?, ?, ?, ?, 'queued', ?, ?, ?, ?, ?, ?, ?, ?, ?)",
            )
            .bind(&id)
            .bind(user)
            .bind(&batch_id)
            .bind(&spec.id)
            .bind(&spec.provider)
            .bind(&spec.name)
            .bind(spec.output.as_str())
            .bind(&req.prompt)
            .bind(&negative)
            .bind(&params_json)
            .bind(&inputs_json)
            .bind(&req.preset)
            .bind(n as i64)
            .bind(cost)
            .bind(now)
            .bind(now)
            .execute(&self.inner.db)
            .await?;
            let row = store::job_row(&self.inner.db, user, &id).await?;
            let job = row.to_job(Vec::new());
            self.emit(user, "job", json!(job));
            jobs.push(job);
        }
        self.inner.wake.notify_one();
        Ok(Created { batch_id, jobs })
    }

    async fn validate_inputs(&self, user: &str, spec: &ModelSpec, inputs: &[crate::JobInput]) -> Result<Vec<crate::JobInput>> {
        let mut per_role: HashMap<pictesa_core::InputRole, u32> = HashMap::new();
        for (i, input) in inputs.iter().enumerate() {
            let field = format!("inputs.{i}");
            let slot = spec
                .accepts(input.role)
                .ok_or_else(|| EngineError::invalid(format!("{} does not take a {:?} input.", spec.name, input.role).to_lowercase(), Some(&field)))?;
            let row = store::asset_row(&self.inner.db, user, &input.asset_id).await.map_err(|_| EngineError::invalid("That file is no longer in the library.", Some(&field)))?;
            if row.deleted_at.is_some() {
                return Err(EngineError::invalid("That file is in the trash.", Some(&field)));
            }
            if store::parse_kind(&row.kind) != slot.kind {
                return Err(EngineError::invalid(format!("This slot needs an {} file.", slot.kind.as_str()), Some(&field)));
            }
            let n = per_role.entry(input.role).or_default();
            *n += 1;
            if *n > slot.max {
                return Err(EngineError::invalid(format!("{} takes at most {} {} file(s) here.", spec.name, slot.max, slot.kind.as_str()), Some(&field)));
            }
        }
        for slot in &spec.inputs {
            if per_role.get(&slot.role).copied().unwrap_or(0) < slot.min {
                let label = slot.label.clone().unwrap_or_else(|| format!("{:?}", slot.role));
                return Err(EngineError::invalid(format!("{} needs: {label}.", spec.name), Some("inputs")));
            }
        }
        Ok(inputs.to_vec())
    }

    pub async fn jobs(&self, user: &str, f: &JobFilter) -> Result<(Vec<Job>, Option<String>)> {
        let limit = f.limit.unwrap_or(50).clamp(1, 200) as i64;
        let rows = sqlx::query_as::<_, JobRow>(
            "SELECT * FROM jobs WHERE user_id = ?
               AND (? IS NULL OR status = ?) AND (? IS NULL OR output = ?) AND (? IS NULL OR batch_id = ?)
               AND (? IS NULL OR id < ?)
             ORDER BY id DESC LIMIT ?",
        )
        .bind(user)
        .bind(&f.status)
        .bind(&f.status)
        .bind(&f.output)
        .bind(&f.output)
        .bind(&f.batch)
        .bind(&f.batch)
        .bind(&f.before)
        .bind(&f.before)
        .bind(limit + 1)
        .fetch_all(&self.inner.db)
        .await?;
        let next = (rows.len() as i64 > limit).then(|| rows[limit as usize - 1].id.clone());
        let mut out = Vec::new();
        for r in rows.iter().take(limit as usize) {
            out.push(store::job_view(&self.inner.db, r).await?);
        }
        Ok((out, next))
    }

    pub async fn job(&self, user: &str, id: &str) -> Result<Job> {
        let row = store::job_row(&self.inner.db, user, id).await.map_err(|_| EngineError::not_found("generation"))?;
        store::job_view(&self.inner.db, &row).await
    }

    pub async fn cancel(&self, user: &str, id: &str) -> Result<Job> {
        let row = store::job_row(&self.inner.db, user, id).await.map_err(|_| EngineError::not_found("generation"))?;
        if !row.status().is_final() {
            let now = now_ms();
            sqlx::query("UPDATE jobs SET status = 'canceled', message = NULL, finished_at = ?, updated_at = ? WHERE id = ? AND status IN ('queued', 'running')")
                .bind(now)
                .bind(now)
                .bind(id)
                .execute(&self.inner.db)
                .await?;
            if let Some(token) = self.inner.cancels.lock().unwrap().get(id) {
                token.cancel();
            }
        }
        let job = self.job(user, id).await?;
        self.emit(user, "job", json!(job));
        Ok(job)
    }

    pub async fn retry(&self, user: &str, id: &str) -> Result<Created> {
        let row = store::job_row(&self.inner.db, user, id).await.map_err(|_| EngineError::not_found("generation"))?;
        // The preset is already folded into the stored prompt and params.
        self.create_generation(
            user,
            CreateGeneration {
                model: row.model.clone(),
                prompt: row.prompt.clone(),
                negative_prompt: row.negative_prompt.clone(),
                params: row.params_map(),
                inputs: row.inputs_list(),
                count: Some(row.count as u32),
                preset: None,
            },
        )
        .await
    }

    pub async fn delete_job(&self, user: &str, id: &str) -> Result<()> {
        let row = store::job_row(&self.inner.db, user, id).await.map_err(|_| EngineError::not_found("generation"))?;
        if !row.status().is_final() {
            self.cancel(user, id).await?;
        }
        sqlx::query("DELETE FROM jobs WHERE id = ? AND user_id = ?").bind(id).bind(user).execute(&self.inner.db).await?;
        Ok(())
    }

    /// Waits until a job finishes or the timeout passes, then returns its latest state.
    pub async fn wait_job(&self, user: &str, id: &str, timeout: Duration) -> Result<Job> {
        let mut rx = self.subscribe();
        let deadline = tokio::time::Instant::now() + timeout;
        loop {
            let job = self.job(user, id).await?;
            if job.status.is_final() {
                return Ok(job);
            }
            let wait = tokio::time::timeout_at(deadline.min(tokio::time::Instant::now() + Duration::from_secs(2)), rx.recv()).await;
            if tokio::time::Instant::now() >= deadline {
                return self.job(user, id).await;
            }
            if let Ok(Err(tokio::sync::broadcast::error::RecvError::Closed)) = wait {
                return self.job(user, id).await;
            }
        }
    }

    // ---------- assets ----------

    pub(crate) async fn insert_asset(&self, user: &str, stored: &Stored, source: &str, job: Option<&JobRow>, source_url: Option<&str>) -> Result<Asset> {
        let id = crate::new_id();
        sqlx::query(
            "INSERT INTO assets (id, user_id, job_id, kind, mime, hash, ext, size, width, height, duration_ms, has_thumb, source, model, prompt, source_url, created_at)
             VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)",
        )
        .bind(&id)
        .bind(user)
        .bind(job.map(|j| j.id.clone()))
        .bind(stored.kind.as_str())
        .bind(&stored.mime)
        .bind(&stored.hash)
        .bind(&stored.ext)
        .bind(stored.size as i64)
        .bind(stored.width.map(i64::from))
        .bind(stored.height.map(i64::from))
        .bind(stored.duration_ms.map(|d| d as i64))
        .bind(stored.has_thumb)
        .bind(source)
        .bind(job.map(|j| j.model.clone()))
        .bind(job.map(|j| j.prompt.clone()))
        .bind(source_url)
        .bind(now_ms())
        .execute(&self.inner.db)
        .await?;
        let asset = store::asset_row(&self.inner.db, user, &id).await?.to_asset();
        self.emit(user, "asset", json!(asset));
        Ok(asset)
    }

    pub async fn upload(&self, user: &str, bytes: &[u8], file_name: Option<&str>, mime: Option<&str>) -> Result<Asset> {
        if bytes.is_empty() {
            return Err(EngineError::invalid("The file is empty.", Some("file")));
        }
        let stored = self.inner.files.store_bytes(bytes, mime, file_name).await.map_err(|e| EngineError::invalid(format!("{e:#}"), Some("file")))?;
        self.insert_asset(user, &stored, "upload", None, None).await
    }

    pub async fn import_url(&self, user: &str, url: &str) -> Result<Asset> {
        let parsed = url::Url::parse(url).map_err(|_| EngineError::invalid("That is not a valid URL.", Some("url")))?;
        if !matches!(parsed.scheme(), "http" | "https") {
            return Err(EngineError::invalid("Only http and https URLs can be imported.", Some("url")));
        }
        if self.mode() == crate::Mode::Hosted && is_private_host(&parsed) {
            return Err(EngineError::invalid("Private network addresses cannot be imported.", Some("url")));
        }
        let resp = self.inner.download.get(parsed.clone()).send().await.map_err(|e| EngineError::invalid(format!("Download failed: {e}"), Some("url")))?;
        if !resp.status().is_success() {
            return Err(EngineError::invalid(format!("Download failed with HTTP {}.", resp.status()), Some("url")));
        }
        let stored = self.inner.files.store_response(resp, Some(parsed.path())).await.map_err(|e| EngineError::invalid(format!("{e:#}"), Some("url")))?;
        self.insert_asset(user, &stored, "import", None, Some(url)).await
    }

    pub async fn assets(&self, user: &str, f: &AssetFilter) -> Result<(Vec<Asset>, Option<String>)> {
        let limit = f.limit.unwrap_or(60).clamp(1, 500) as i64;
        let trash = f.trash.unwrap_or(false);
        let fts = f.q.as_deref().and_then(store::fts_query);
        let rows = sqlx::query_as::<_, AssetRow>(
            "SELECT a.* FROM assets a
             WHERE a.user_id = ?
               AND ((? AND a.deleted_at IS NOT NULL) OR (NOT ? AND a.deleted_at IS NULL))
               AND (? IS NULL OR a.kind = ?)
               AND (? IS NULL OR a.favorite = ?)
               AND (? IS NULL OR a.model = ?)
               AND (? IS NULL OR a.source = ?)
               AND (? IS NULL OR a.id < ?)
               AND (? IS NULL OR a.rowid IN (SELECT rowid FROM assets_fts WHERE assets_fts MATCH ?))
             ORDER BY a.id DESC LIMIT ?",
        )
        .bind(user)
        .bind(trash)
        .bind(trash)
        .bind(&f.kind)
        .bind(&f.kind)
        .bind(f.favorite)
        .bind(f.favorite)
        .bind(&f.model)
        .bind(&f.model)
        .bind(&f.source)
        .bind(&f.source)
        .bind(&f.before)
        .bind(&f.before)
        .bind(&fts)
        .bind(&fts)
        .bind(limit + 1)
        .fetch_all(&self.inner.db)
        .await?;
        let next = (rows.len() as i64 > limit).then(|| rows[limit as usize - 1].id.clone());
        Ok((rows.iter().take(limit as usize).map(AssetRow::to_asset).collect(), next))
    }

    pub async fn asset(&self, user: &str, id: &str) -> Result<(Asset, Option<Job>)> {
        let row = store::asset_row(&self.inner.db, user, id).await.map_err(|_| EngineError::not_found("file"))?;
        let job = match &row.job_id {
            Some(j) => self.job(user, j).await.ok(),
            None => None,
        };
        Ok((row.to_asset(), job))
    }

    /// Path, MIME type and download name of an asset's original file.
    pub async fn asset_file(&self, user: Option<&str>, id: &str) -> Result<(std::path::PathBuf, String, String)> {
        let row = match user {
            Some(u) => store::asset_row(&self.inner.db, u, id).await,
            None => Ok(sqlx::query_as::<_, AssetRow>("SELECT * FROM assets WHERE id = ?").bind(id).fetch_one(&self.inner.db).await?),
        }
        .map_err(|_| EngineError::not_found("file"))?;
        let name = format!("pictesa-{}.{}", row.id.to_lowercase(), row.ext);
        Ok((self.inner.files.path(&row.hash, &row.ext), row.mime, name))
    }

    pub async fn asset_thumb(&self, user: Option<&str>, id: &str) -> Result<std::path::PathBuf> {
        let row = match user {
            Some(u) => store::asset_row(&self.inner.db, u, id).await,
            None => Ok(sqlx::query_as::<_, AssetRow>("SELECT * FROM assets WHERE id = ?").bind(id).fetch_one(&self.inner.db).await?),
        }
        .map_err(|_| EngineError::not_found("file"))?;
        if !row.has_thumb {
            return Err(EngineError::not_found("thumbnail"));
        }
        Ok(self.inner.files.thumb(&row.hash))
    }

    pub async fn set_favorite(&self, user: &str, id: &str, favorite: bool) -> Result<Asset> {
        let done = sqlx::query("UPDATE assets SET favorite = ? WHERE id = ? AND user_id = ?").bind(favorite).bind(id).bind(user).execute(&self.inner.db).await?;
        if done.rows_affected() == 0 {
            return Err(EngineError::not_found("file"));
        }
        let asset = store::asset_row(&self.inner.db, user, id).await?.to_asset();
        self.emit(user, "asset", json!(asset));
        Ok(asset)
    }

    pub async fn trash(&self, user: &str, id: &str, permanent: bool) -> Result<()> {
        let row = store::asset_row(&self.inner.db, user, id).await.map_err(|_| EngineError::not_found("file"))?;
        if permanent {
            sqlx::query("DELETE FROM assets WHERE id = ?").bind(id).execute(&self.inner.db).await?;
            let (others,): (i64,) = sqlx::query_as("SELECT count(*) FROM assets WHERE hash = ?").bind(&row.hash).fetch_one(&self.inner.db).await?;
            if others == 0 {
                self.inner.files.remove(&row.hash, &row.ext).await;
            }
            let mut gone = row.to_asset();
            gone.deleted_at = Some(now_ms());
            self.emit(user, "asset", json!({ "id": id, "deleted": true, "permanent": true, "asset": gone }));
        } else {
            sqlx::query("UPDATE assets SET deleted_at = ? WHERE id = ? AND user_id = ?").bind(now_ms()).bind(id).bind(user).execute(&self.inner.db).await?;
            let asset = store::asset_row(&self.inner.db, user, id).await?.to_asset();
            self.emit(user, "asset", json!(asset));
        }
        Ok(())
    }

    pub async fn restore(&self, user: &str, id: &str) -> Result<Asset> {
        let done = sqlx::query("UPDATE assets SET deleted_at = NULL WHERE id = ? AND user_id = ?").bind(id).bind(user).execute(&self.inner.db).await?;
        if done.rows_affected() == 0 {
            return Err(EngineError::not_found("file"));
        }
        let asset = store::asset_row(&self.inner.db, user, id).await?.to_asset();
        self.emit(user, "asset", json!(asset));
        Ok(asset)
    }

    /// True when `key` is the signed key for this asset (public file URLs in hosted mode).
    pub fn file_key_ok(&self, asset_id: &str, key: &str) -> bool {
        crate::queue::file_key(self, asset_id) == key
    }
}

/// Checks params against the spec, drops nulls, fills defaults.
pub fn validate_params(spec: &ModelSpec, given: &Map<String, Value>) -> Result<Map<String, Value>> {
    let mut out = Map::new();
    for (k, v) in given {
        if v.is_null() || v.as_str().is_some_and(|s| s.is_empty()) {
            continue;
        }
        if k == keys::NEGATIVE_PROMPT {
            continue;
        }
        match spec.param_spec(k) {
            Some(p) => {
                let v = coerce(p, v);
                p.validate(&v).map_err(|m| EngineError::invalid(m, Some(&format!("params.{k}"))))?;
                out.insert(k.clone(), v);
            }
            None if spec.params.is_empty() => {
                out.insert(k.clone(), v.clone());
            }
            None => return Err(EngineError::invalid(format!("{} has no setting called {k}.", spec.name), Some(&format!("params.{k}")))),
        }
    }
    for p in &spec.params {
        if !out.contains_key(&p.key) {
            if let Some(d) = &p.default {
                out.insert(p.key.clone(), d.clone());
            }
        }
    }
    Ok(out)
}

/// Accepts "5" for an int param and 5 for a string enum, as forms and agents send both.
fn coerce(p: &pictesa_core::ParamSpec, v: &Value) -> Value {
    use pictesa_core::ParamKind::*;
    match (&p.kind, v) {
        (Int { .. } | Seed, Value::String(s)) => s.trim().parse::<i64>().map(Value::from).unwrap_or(v.clone()),
        (Float { .. }, Value::String(s)) => s.trim().parse::<f64>().map(Value::from).unwrap_or(v.clone()),
        (Bool, Value::String(s)) => match s.as_str() {
            "true" | "1" | "yes" => Value::Bool(true),
            "false" | "0" | "no" => Value::Bool(false),
            _ => v.clone(),
        },
        (Enum { options }, _) => options
            .iter()
            .find(|o| &o.value == v || o.value.to_string().trim_matches('"') == v.to_string().trim_matches('"'))
            .map(|o| o.value.clone())
            .unwrap_or(v.clone()),
        _ => v.clone(),
    }
}

/// Rough price in dollars for one request, from the model's published pricing.
pub fn estimate_cost(spec: &ModelSpec, params: &Map<String, Value>, outputs: u32) -> Option<f64> {
    let seconds = || {
        params
            .get(keys::DURATION)
            .and_then(|v| v.as_f64().or_else(|| v.as_str().and_then(|s| s.trim_end_matches('s').parse().ok())))
            .unwrap_or(5.0)
    };
    let usd = match spec.pricing.as_ref()? {
        Pricing::PerImage { usd } => usd * outputs as f64,
        Pricing::PerVideo { usd } => usd * outputs as f64,
        Pricing::PerSecond { usd } => usd * seconds() * outputs as f64,
        Pricing::PerMegapixel { usd } => {
            let mp = match params.get(keys::RESOLUTION).and_then(Value::as_str) {
                Some("4K") | Some("4k") => 16.0,
                Some("2K") | Some("2k") => 4.0,
                _ => 1.0,
            };
            usd * mp * outputs as f64
        }
        Pricing::Local => 0.0,
        Pricing::Variable { .. } => return None,
    };
    Some((usd * 10_000.0).round() / 10_000.0)
}

fn is_private_host(u: &url::Url) -> bool {
    match u.host() {
        Some(url::Host::Ipv4(ip)) => ip.is_private() || ip.is_loopback() || ip.is_link_local() || ip.is_unspecified(),
        Some(url::Host::Ipv6(ip)) => ip.is_loopback() || ip.is_unspecified() || (ip.segments()[0] & 0xfe00) == 0xfc00,
        Some(url::Host::Domain(d)) => d == "localhost" || d.ends_with(".local") || d.ends_with(".internal"),
        None => true,
    }
}
