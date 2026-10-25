//! Models, generations and the asset library: the operations behind the API, CLI and MCP tools.

use std::{collections::HashMap, time::Duration};

use pictesa_core::{keys, now_ms, ModelSpec, Pricing, Task};
use serde_json::{json, Map, Value};

use crate::{
    files::Stored,
    store::{self, AssetRow, JobRow},
    Asset, AssetFilter, CreateGeneration, Created, Engine, EngineError, ImportMetadata, Job, JobFilter, ModelFilter, ModelPrefs, ModelView, Result,
};

pub const MAX_COUNT: u32 = 16;
const MAX_PINNED: usize = 100;
const PINNED_KEY: &str = "models.pinned";

/// A validated, priced request ready to be queued.
struct Planned {
    spec: ModelSpec,
    prompt: String,
    negative: Option<String>,
    params_json: String,
    inputs_json: String,
    preset: Option<String>,
    billing: &'static str,
    /// (outputs, cost estimate in USD, wallet charge in micro-dollars) per provider request.
    runs: Vec<(u32, Option<f64>, i64)>,
}
/// Tags a provider sets on models that cannot run right now (missing local nodes or files).
pub const UNAVAILABLE_TAGS: &[&str] = &["unavailable", "needs-nodes", "needs-models"];

impl Engine {
    // ---------- models ----------

    pub async fn models(&self, user: &str, f: &ModelFilter) -> Result<Vec<ModelView>> {
        self.check_local_catalogs(user).await;
        let mut configured = HashMap::new();
        let mut billed = HashMap::new();
        for p in self.inner.catalog.providers() {
            let info = p.info();
            let resolved = self.resolve_fields(user, p.as_ref()).await?;
            configured.insert(info.id.clone(), Self::is_configured(&info, &resolved));
            billed.insert(info.id.clone(), Self::uses_platform(&info, &resolved));
        }
        let markup = self.inner.billing.markup;
        let q = f.q.as_deref().map(str::to_lowercase).filter(|q| !q.is_empty());
        let task: Option<Task> = f.task.as_deref().and_then(|t| serde_json::from_value(json!(t)).ok());
        let arenas = self.arenas();
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
            .map(|mut spec| {
                // A local template whose nodes or model files are missing is listed but not runnable.
                let unavailable = spec.tags.iter().any(|t| UNAVAILABLE_TAGS.contains(&t.as_str()));
                let ok = configured.get(&spec.provider).copied().unwrap_or(false) && !unavailable;
                let is_billed = billed.get(&spec.provider).copied().unwrap_or(false);
                if is_billed {
                    spec.pricing = spec.pricing.map(|p| with_markup(p, markup));
                }
                let released = spec.meta.get("date").and_then(Value::as_str).and_then(|d| d.get(..10)).map(String::from);
                // Provider bindings are internal and heavy; the full spec comes from GET /api/models/{id}.
                spec.meta = Value::Null;
                ModelView { arena: Vec::new(), family: arenas.family(&spec), spec, configured: ok, billed: is_billed, released }
            })
            .filter(|v| f.configured != Some(true) || v.configured)
            .map(|mut v| {
                v.arena = arenas.scores(&v.spec);
                v
            })
            .collect();
        out.sort_by_key(|v| (!v.configured, !v.spec.tags.iter().any(|t| t == "featured")));
        match f.sort.as_deref() {
            Some("arena") => out.sort_by(|a, b| b.rating(task).unwrap_or(f64::MIN).total_cmp(&a.rating(task).unwrap_or(f64::MIN))),
            Some("newest") => out.sort_by(|a, b| b.released.cmp(&a.released)),
            Some("name") => out.sort_by_cached_key(|v| v.spec.name.to_lowercase()),
            _ => {}
        }
        Ok(out)
    }

    /// The user's pinned models and the models of their latest generations.
    pub async fn model_prefs(&self, user: &str) -> Result<ModelPrefs> {
        let pinned: Option<(String,)> = sqlx::query_as("SELECT value FROM settings WHERE user_id = ? AND key = ?").bind(user).bind(PINNED_KEY).fetch_optional(&self.inner.db).await?;
        let recent: Vec<(String,)> = sqlx::query_as("SELECT model FROM jobs WHERE user_id = ? GROUP BY model ORDER BY MAX(created_at) DESC LIMIT 8").bind(user).fetch_all(&self.inner.db).await?;
        Ok(ModelPrefs { pinned: pinned.and_then(|(v,)| serde_json::from_str(&v).ok()).unwrap_or_default(), recent: recent.into_iter().map(|(m,)| m).collect() })
    }

    pub async fn set_pinned_models(&self, user: &str, ids: Vec<String>) -> Result<ModelPrefs> {
        if ids.len() > MAX_PINNED || ids.iter().any(|id| id.is_empty() || id.len() > 300) {
            return Err(EngineError::invalid(format!("Pin up to {MAX_PINNED} models."), Some("pinned")));
        }
        let mut seen = std::collections::HashSet::new();
        let ids: Vec<String> = ids.into_iter().filter(|id| seen.insert(id.clone())).collect();
        store::put_setting(&self.inner.db, user, PINNED_KEY, &json!(ids).to_string()).await?;
        self.model_prefs(user).await
    }

    /// Full spec for one model (fetches its schema on first use).
    pub async fn model(&self, user: &str, id: &str) -> Result<ModelSpec> {
        let spec = self.inner.catalog.find(id).ok_or_else(|| EngineError::not_found("model"))?;
        if spec.complete {
            return Ok(spec);
        }
        let provider = self.inner.catalog.provider(&spec.provider).ok_or_else(|| EngineError::not_found("provider"))?;
        let ctx = self.catalog_ctx(user, provider.as_ref()).await?;
        self.inner.catalog.resolve(&self.inner.db, &ctx, id).await.map_err(|e| EngineError::invalid(format!("could not load {id}: {e:#}"), Some("model")))
    }

    pub async fn refresh_models_for(&self, user: &str, provider_id: &str) -> Result<usize> {
        let provider = self.inner.catalog.provider(provider_id).ok_or_else(|| EngineError::not_found("provider"))?;
        let ctx = self.catalog_ctx(user, provider.as_ref()).await?;
        let n = self.inner.catalog.refresh(&self.inner.db, provider.as_ref(), &ctx).await.map_err(EngineError::Internal)?;
        // Load full settings for featured models in the background so the picker shows inputs and prices at once.
        let engine = self.clone();
        let user = user.to_string();
        let pid = provider_id.to_string();
        tokio::spawn(async move {
            let Some(provider) = engine.inner.catalog.provider(&pid) else { return };
            let pending: Vec<String> = engine
                .inner
                .catalog
                .provider_models(provider.as_ref())
                .into_iter()
                .filter(|m| !m.complete && m.tags.iter().any(|t| t == "featured"))
                .map(|m| m.id)
                .collect();
            let Ok(ctx) = engine.catalog_ctx(&user, provider.as_ref()).await else { return };
            use futures::StreamExt;
            futures::stream::iter(pending)
                .for_each_concurrent(4, |id| {
                    let (engine, ctx) = (engine.clone(), ctx.clone());
                    async move {
                        if let Err(e) = engine.inner.catalog.resolve(&engine.inner.db, &ctx, &id).await {
                            tracing::debug!("could not preload {id}: {e:#}");
                        }
                    }
                })
                .await;
        });
        Ok(n)
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
            let info = p.info();
            let id = info.id;
            let ttl = if info.kind == pictesa_core::ProviderKind::Local { crate::catalog::LOCAL_CATALOG_TTL_MS } else { crate::catalog::CATALOG_TTL_MS };
            let stale = self.inner.catalog.catalog_age(&id).is_none_or(|age| age > ttl);
            if stale {
                if let Err(e) = self.refresh_models_for(user, &id).await {
                    tracing::debug!("discovery skipped for {id}: {e}");
                }
            }
        }
    }

    /// Checks local servers whose model list is over a minute old, for at most a few seconds, so a
    /// ComfyUI started (or given new model files) after Pictesa shows up in the next model list.
    async fn check_local_catalogs(&self, user: &str) {
        let mut checked = self.inner.local_checks.lock().await;
        let now = pictesa_core::now_ms();
        let due: Vec<String> = self
            .inner
            .catalog
            .providers()
            .iter()
            .map(|p| p.info())
            .filter(|info| info.kind == pictesa_core::ProviderKind::Local)
            .map(|info| info.id)
            .filter(|id| checked.get(id).is_none_or(|at| now - at > crate::catalog::LOCAL_CATALOG_TTL_MS))
            .collect();
        let runs = due.iter().map(|id| tokio::time::timeout(std::time::Duration::from_secs(3), self.refresh_models_for(user, id)));
        futures::future::join_all(runs).await;
        // A server that is down counts as checked too, so it is not asked again on every list.
        checked.extend(due.into_iter().map(|id| (id, now)));
    }

    // ---------- generations ----------

    /// Validates, prices and queues a generation (or a comparison across models) in one transaction:
    /// either every run is queued and paid, or nothing is.
    pub async fn create_generation(&self, user: &str, req: CreateGeneration) -> Result<Created> {
        let mut plans = Vec::new();
        if req.compare.is_empty() {
            plans.push(self.plan(user, req).await?);
        } else {
            if req.compare.len() > 7 {
                return Err(EngineError::invalid("Compare at most 8 models at once.", Some("compare")));
            }
            let mut models = vec![req.model.clone()];
            for m in &req.compare {
                if !models.contains(m) {
                    models.push(m.clone());
                }
            }
            for (i, m) in models.iter().enumerate() {
                // Errors of the extra models point at their place in `compare`.
                let blame = |e: EngineError| match e {
                    EngineError::Validation { message, .. } if i > 0 => EngineError::Validation { message, field: Some(format!("compare.{}", i - 1)) },
                    other => other,
                };
                let spec = self.model(user, m).await.map_err(blame)?;
                let mut one = req.clone();
                one.model = spec.id.clone();
                one.compare.clear();
                one.params = req
                    .params
                    .iter()
                    .filter_map(|(k, v)| spec.param_spec(k).and_then(|p| crate::recipes::fit_param(p, v)).map(|v| (k.clone(), v)))
                    .collect();
                one.inputs = req.inputs.iter().filter(|i| spec.accepts(i.role).is_some()).cloned().collect();
                plans.push(self.plan(user, one).await.map_err(blame)?);
            }
        }
        self.commit(user, plans, None).await
    }

    /// Plans every expanded run before committing anything. Dry runs never reserve money or queue.
    pub async fn estimate_batch(&self, user: &str, req: crate::batches::BatchRequest) -> Result<crate::batches::BatchEstimate> {
        let plans = self.batch_plans(user, &req).await?;
        let mut tx = self.inner.db.begin_with("BEGIN IMMEDIATE").await?;
        self.authorize_plans(user, &plans, &mut tx, false, now_ms()).await?;
        tx.rollback().await?;
        Ok(batch_estimate(&plans))
    }

    async fn batch_plans(&self, user: &str, req: &crate::batches::BatchRequest) -> Result<Vec<Planned>> {
        let spec = self.model(user, &req.generation.model).await?;
        let mut plans = Vec::new();
        for one in crate::batches::expand(req, &spec)? { plans.push(self.plan(user, one).await?); }
        Ok(plans)
    }

    pub async fn create_batch(&self, user: &str, req: crate::batches::BatchRequest) -> Result<Created> {
        if req.dry_run { return Err(EngineError::invalid("Use the batch estimate method for a dry run.", Some("dry_run"))); }
        let plans = self.batch_plans(user, &req).await?;
        self.commit(user, plans, None).await
    }

    /// Four fresh seeds with the finished run's model, settings, inputs and preset.
    pub async fn variations(&self, user: &str, id: &str) -> Result<Created> {
        let job = self.job(user, id).await?;
        if job.status != crate::JobStatus::Succeeded { return Err(EngineError::invalid("Choose a finished result.", Some("job"))); }
        let spec = self.model(user, &job.model).await?;
        let mut generation = CreateGeneration { model: job.model, prompt: job.prompt, negative_prompt: job.negative_prompt, params: job.params, inputs: job.inputs, preset: job.preset, ..Default::default() };
        if let Some(seed) = crate::batches::seed_param(&spec) {
            let previous = generation.params.get(&seed.key).and_then(Value::as_u64);
            let (min, max) = match seed.kind {
                pictesa_core::ParamKind::Int { min, max, .. } => (min.max(0) as u64, max.max(0) as u64),
                _ => (0, u32::MAX as u64),
            };
            let max = max.min(crate::batches::MAX_SAFE_SEED);
            let available = max.saturating_sub(min) + 1;
            if max < min || available < 4 + u64::from(previous.is_some_and(|n| n >= min && n <= max)) {
                return Err(EngineError::invalid("This model does not allow four new seed values.", Some("seed")));
            }
            let mut values = Vec::new();
            while values.len() < 4 {
                let value = min + rand::random::<u64>() % available;
                if Some(value) != previous && !values.contains(&json!(value)) { values.push(json!(value)); }
            }
            let request = crate::batches::BatchRequest { generation, grid: vec![crate::batches::GridAxis { key: seed.key.clone(), values }], ..Default::default() };
            self.create_batch(user, request).await
        } else {
            generation.count = Some(4);
            self.create_generation(user, generation).await
        }
    }

    /// Queues an already-validated request inside an existing batch (recipes use this through `create_generation`).
    async fn plan(&self, user: &str, mut req: CreateGeneration) -> Result<Planned> {
        let spec = self.model(user, &req.model).await?;
        if let Some(cid) = req.character.clone().filter(|c| !c.is_empty()) {
            let slot = spec.accepts(pictesa_core::InputRole::Reference).filter(|s| s.kind == pictesa_core::MediaKind::Image).ok_or_else(|| {
                EngineError::invalid(format!("{} does not take reference images, so it cannot use a character.", spec.name), Some("character"))
            })?;
            let already = req.inputs.iter().filter(|i| i.role == pictesa_core::InputRole::Reference).count() as u32;
            let room = slot.max.saturating_sub(already);
            req.inputs.extend(self.character_inputs(user, &cid, room).await?);
        }
        let provider = self.inner.catalog.provider(&spec.provider).ok_or_else(|| EngineError::not_found("provider"))?;
        let info = provider.info();
        let resolved = self.resolve_fields(user, provider.as_ref()).await?;
        if !Self::is_configured(&info, &resolved) {
            return Err(EngineError::invalid(format!("{} is not set up yet. Add its key in Settings > Providers.", info.name), Some("model")));
        }
        if spec.tags.iter().any(|t| UNAVAILABLE_TAGS.contains(&t.as_str())) {
            let why = if spec.tags.iter().any(|t| t == "unavailable") { "its server cannot be reached" } else { "its server is missing model files or nodes (see the model's details)" };
            return Err(EngineError::invalid(format!("{} cannot run right now: {why}.", spec.name), Some("model")));
        }
        let platform = resolved.billing == "platform";
        // Safety switch for demos and tests: refuse anything that can cost money.
        if std::env::var("PICTESA_DENY_PAID").is_ok_and(|v| v == "1") {
            let free = matches!(spec.pricing, Some(Pricing::Local)) || info.kind == pictesa_core::ProviderKind::Local;
            if !free {
                return Err(EngineError::invalid("Paid generations are turned off on this server (PICTESA_DENY_PAID=1). Use a local or mock model.", Some("model")));
            }
        }

        // The stored prompt stays the person's own text; the preset's words are added when the
        // request is sent, so "reuse settings" never doubles them.
        let mut full_prompt = req.prompt.clone();
        if let Some(pid) = req.preset.clone().filter(|p| !p.is_empty()) {
            let preset = self.preset(&pid).ok_or_else(|| EngineError::invalid(format!("unknown preset {pid}"), Some("preset")))?;
            full_prompt = with_preset(&req.prompt, &preset.prompt_suffix);
            for (k, v) in &preset.params {
                if spec.param_spec(k).is_some() && !req.params.contains_key(k) {
                    req.params.insert(k.clone(), v.clone());
                }
            }
        }

        if full_prompt.chars().count() > 20_000 {
            return Err(EngineError::invalid("The prompt is longer than 20,000 characters.", Some("prompt")));
        }
        let needs_prompt = spec.prompt && spec.tasks.iter().any(|t| matches!(t, Task::TextToImage | Task::TextToVideo | Task::TextToAudio)) && req.inputs.is_empty();
        if needs_prompt && full_prompt.trim().is_empty() {
            return Err(EngineError::invalid("Describe what to generate.", Some("prompt")));
        }

        let params = validate_params(&spec, &req.params)?;
        let inputs = self.validate_inputs(user, &spec, &req.inputs).await?;
        let count = req.count.unwrap_or(1).clamp(1, MAX_COUNT);
        let negative = req.negative_prompt.filter(|n| spec.negative_prompt && !n.trim().is_empty());

        // Split the count into provider requests and price each one.
        let markup = self.inner.billing.markup;
        let mut runs = Vec::new();
        let mut remaining = count;
        while remaining > 0 {
            let n = remaining.min(spec.max_outputs.max(1));
            remaining -= n;
            let cost = if info.kind == pictesa_core::ProviderKind::Local && !platform { Some(0.0) } else { estimate_cost(&spec, &params, n) };
            let charge = if platform && cost != Some(0.0) {
                let usd = cost.ok_or_else(|| EngineError::invalid(format!("{} has no fixed price, so it can only run with your own {} key.", spec.name, info.name), Some("model")))?;
                crate::billing::to_micros(usd * markup).max(1)
            } else {
                0
            };
            runs.push((n, cost, charge));
        }
        Ok(Planned {
            params_json: serde_json::to_string(&params).map_err(anyhow::Error::from)?,
            inputs_json: serde_json::to_string(&inputs).map_err(anyhow::Error::from)?,
            spec,
            prompt: req.prompt,
            negative,
            preset: req.preset,
            billing: resolved.billing,
            runs,
        })
    }

    async fn authorize_plans(&self, user: &str, plans: &[Planned], tx: &mut sqlx::Transaction<'_, sqlx::Sqlite>, reserve: bool, now: i64) -> Result<()> {
        let total: i64 = plans.iter().flat_map(|p| p.runs.iter().map(|r| r.2)).sum();
        let budget_total: i64 = plans.iter().flat_map(|p| p.runs.iter().map(|(_, cost, charge)| {
            if *charge > 0 { *charge } else { cost.map(crate::billing::to_micros).unwrap_or(0) }
        })).sum();
        let unknown = plans.iter().any(|p| p.runs.iter().any(|(_, cost, _)| cost.is_none()));
        if budget_total > 0 || unknown { self.check_budget(tx, user, budget_total, unknown, now).await?; }
        if total > 0 {
            let wallet: Option<(i64, bool)> = sqlx::query_as("SELECT balance_micros, locked FROM wallets WHERE user_id = ?").bind(user).fetch_optional(&mut **tx).await?;
            let (balance, locked) = wallet.unwrap_or((0, false));
            if locked {
                return Err(EngineError::Forbidden("Billing is paused on this account. Contact the operator.".into()));
            }
            if !reserve {
                if balance < total { return Err(EngineError::invalid(format!("This batch costs about ${:.2} and your balance is ${:.2}.", total as f64 / 1e6, balance as f64 / 1e6), Some("billing"))); }
                return Ok(());
            }
            let paid = sqlx::query("UPDATE wallets SET balance_micros = balance_micros - ? WHERE user_id = ? AND locked = 0 AND balance_micros >= ?")
                .bind(total)
                .bind(user)
                .bind(total)
                .execute(&mut **tx)
                .await?;
            if paid.rows_affected() != 1 {
                return Err(EngineError::invalid(
                    format!("This run costs about ${:.2} and your balance is ${:.2}. Top up in Settings > Billing, or add your own key.", total as f64 / 1e6, balance as f64 / 1e6),
                    Some("billing"),
                ));
            }
        }
        Ok(())
    }

    async fn commit(&self, user: &str, plans: Vec<Planned>, batch: Option<String>) -> Result<Created> {
        let batch_id = batch.unwrap_or_else(crate::new_id);
        let mut tx = self.inner.db.begin_with("BEGIN IMMEDIATE").await?;
        let now = now_ms();
        self.authorize_plans(user, &plans, &mut tx, true, now).await?;
        let mut ids = Vec::new();
        for plan in &plans {
            for (n, cost, charge) in &plan.runs {
                let id = crate::new_id();
                if *charge > 0 {
                    sqlx::query("INSERT INTO ledger (id, user_id, amount_micros, reason, job_id, created_at) VALUES (?, ?, ?, ?, ?, ?)")
                        .bind(format!("charge:{id}"))
                        .bind(user)
                        .bind(-charge)
                        .bind(format!("{} run", plan.spec.name))
                        .bind(&id)
                        .bind(now)
                        .execute(&mut *tx)
                        .await?;
                }
                sqlx::query(
                    "INSERT INTO jobs (id, user_id, batch_id, model, provider, model_name, output, status, prompt, negative_prompt, params, inputs, preset, count, cost_usd, charged_micros, billing, created_at, updated_at)
                     VALUES (?, ?, ?, ?, ?, ?, ?, 'queued', ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)",
                )
                .bind(&id)
                .bind(user)
                .bind(&batch_id)
                .bind(&plan.spec.id)
                .bind(&plan.spec.provider)
                .bind(&plan.spec.name)
                .bind(plan.spec.output.as_str())
                .bind(&plan.prompt)
                .bind(&plan.negative)
                .bind(&plan.params_json)
                .bind(&plan.inputs_json)
                .bind(&plan.preset)
                .bind(*n as i64)
                .bind(if *charge > 0 { Some(*charge as f64 / 1e6) } else { *cost })
                .bind(charge)
                .bind(plan.billing)
                .bind(now)
                .bind(now)
                .execute(&mut *tx)
                .await?;
                sqlx::query("INSERT INTO spend_runs (job_id, user_id, model, provider, estimated_micros, actual_micros, created_at) VALUES (?, ?, ?, ?, ?, ?, ?)")
                    .bind(&id).bind(user).bind(&plan.spec.id).bind(&plan.spec.provider)
                    .bind(cost.map(crate::billing::to_micros))
                    .bind(if *charge > 0 { Some(*charge) } else if *cost == Some(0.0) { Some(0) } else { None })
                    .bind(now).execute(&mut *tx).await?;
                ids.push(id);
            }
        }
        tx.commit().await?;
        self.inner.owned_batches.lock().unwrap().insert(batch_id.clone());
        let mut jobs = Vec::new();
        for id in ids {
            let job = store::job_row(&self.inner.db, user, &id).await?.to_job(Vec::new());
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
            // Judged in the same statement: refunded only when no submit is in flight and the
            // provider never accepted the job. A submit starting right after sees the status and stops.
            let done: Option<(bool,)> = sqlx::query_as("UPDATE jobs SET status = 'canceled', message = NULL, finished_at = ?, updated_at = ? WHERE id = ? AND status IN ('queued', 'running') RETURNING (remote IS NULL AND submitting = 0)")
                .bind(now)
                .bind(now)
                .bind(id)
                .fetch_optional(&self.inner.db)
                .await?;
            if let Some(token) = self.inner.cancels.lock().unwrap().get(id) {
                token.cancel();
            }
            // Refund only when the provider never got the job; a started run may already be billed upstream.
            if matches!(done, Some((true,))) {
                self.refund_job(id).await;
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
                preset: row.preset.clone(),
                compare: Vec::new(),
                character: None,
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
        self.insert_named_asset(user, stored, source, job, source_url, None, None).await
    }

    /// `made_from` is set on edit helpers: the asset they were made from. An upload or import of a
    /// file the user already has (same content, not in the trash) returns that asset, not a copy.
    #[allow(clippy::too_many_arguments)]
    pub(crate) async fn insert_named_asset(&self, user: &str, stored: &Stored, source: &str, job: Option<&JobRow>, source_url: Option<&str>, name: Option<&str>, made_from: Option<&str>) -> Result<Asset> {
        let id = crate::new_id();
        let name = name.map(|n| n.chars().filter(|c| !c.is_control()).take(200).collect::<String>()).filter(|n| !n.trim().is_empty());
        // Generated outputs and edit helpers always get their own row.
        let always_new = job.is_some() || source == crate::edits::HELPER_SOURCE;
        sqlx::query(
            "INSERT INTO assets (id, user_id, job_id, kind, mime, hash, ext, size, width, height, duration_ms, has_thumb, source, model, prompt, source_url, created_at, name, made_from)
             SELECT ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?
             WHERE ? OR NOT EXISTS (SELECT 1 FROM assets WHERE user_id = ? AND hash = ? AND deleted_at IS NULL AND source != 'edit')",
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
        .bind(&name)
        .bind(made_from)
        .bind(always_new)
        .bind(user)
        .bind(&stored.hash)
        .execute(&self.inner.db)
        .await?;
        let asset = if always_new {
            store::asset_row(&self.inner.db, user, &id).await?.to_asset()
        } else {
            let row = sqlx::query_as::<_, AssetRow>("SELECT * FROM assets WHERE user_id = ? AND hash = ? AND deleted_at IS NULL AND source != 'edit' ORDER BY id LIMIT 1")
                .bind(user).bind(&stored.hash).fetch_one(&self.inner.db).await?;
            sqlx::query("UPDATE assets SET name = coalesce(name, ?), source_url = coalesce(source_url, ?) WHERE id = ? AND user_id = ?")
                .bind(&name).bind(source_url).bind(&row.id).bind(user).execute(&self.inner.db).await?;
            store::asset_row(&self.inner.db, user, &row.id).await?.to_asset()
        };
        self.emit(user, "asset", json!(asset));
        Ok(asset)
    }

    pub async fn upload(&self, user: &str, bytes: &[u8], file_name: Option<&str>, mime: Option<&str>) -> Result<Asset> {
        if bytes.is_empty() {
            return Err(EngineError::invalid("The file is empty.", Some("file")));
        }
        let stored = self.inner.files.store_bytes(bytes, mime, file_name, false).await.map_err(|e| EngineError::invalid(format!("{e:#}"), Some("file")))?;
        self.insert_named_asset(user, &stored, "upload", None, None, file_name, None).await
    }

    /// Stores an upload straight from the request body stream (never buffered in memory).
    pub async fn upload_stream<S>(&self, user: &str, stream: S, file_name: Option<&str>) -> Result<Asset>
    where
        S: futures::Stream<Item = std::result::Result<bytes::Bytes, std::io::Error>> + Unpin,
    {
        let stored = self.inner.files.store_stream(stream, None, file_name, false).await.map_err(|e| EngineError::invalid(format!("{e:#}"), Some("file")))?;
        self.insert_named_asset(user, &stored, "upload", None, None, file_name, None).await
    }

    pub async fn import_url(&self, user: &str, url: &str) -> Result<Asset> {
        self.import_url_with_metadata(user, url, &ImportMetadata::default()).await
    }

    pub async fn import_url_with_metadata(&self, user: &str, url: &str, metadata: &ImportMetadata) -> Result<Asset> {
        metadata.validate()?;
        let parsed = url::Url::parse(url).map_err(|_| EngineError::invalid("That is not a valid URL.", Some("url")))?;
        if !matches!(parsed.scheme(), "http" | "https") {
            return Err(EngineError::invalid("Only http and https URLs can be imported.", Some("url")));
        }
        let hosted = self.mode() == crate::Mode::Hosted;
        let resp = if hosted {
            // Hosted: only public addresses, checked on every redirect, with the checked address pinned.
            crate::netguard::fetch_public(parsed.clone(), "pictesa").await.map_err(|e| EngineError::invalid(format!("Cannot import this URL: {e}."), Some("url")))?
        } else {
            self.inner.download.get(parsed.clone()).send().await.map_err(|e| EngineError::invalid(format!("Download failed: {e}"), Some("url")))?
        };
        if !resp.status().is_success() {
            return Err(EngineError::invalid(if hosted { "The download failed.".to_string() } else { format!("Download failed with HTTP {}.", resp.status()) }, Some("url")));
        }
        let stored = self.inner.files.store_response(resp, Some(parsed.path()), false).await.map_err(|e| EngineError::invalid(format!("{e:#}"), Some("url")))?;
        let name = parsed.path_segments().and_then(|mut s| s.next_back()).filter(|n| !n.is_empty()).map(String::from);
        let asset = self.insert_named_asset(user, &stored, "import", None, metadata.source_url.as_deref().or(Some(url)), name.as_deref(), None).await?;
        self.fill_asset_metadata(user, &asset.id, metadata).await
    }

    /// Fills missing provenance only. Reimports cannot overwrite earlier prompts or settings.
    pub async fn fill_asset_metadata(&self, user: &str, id: &str, metadata: &ImportMetadata) -> Result<Asset> {
        metadata.validate()?;
        if metadata.is_empty() {
            return Ok(store::asset_row(&self.inner.db, user, id).await.map_err(|_| EngineError::not_found("file"))?.to_asset());
        }
        let mut tx = self.inner.db.begin().await?;
        let updated = sqlx::query("UPDATE assets SET prompt = coalesce(nullif(prompt, ''), nullif(?, '')),
            negative_prompt = coalesce(nullif(negative_prompt, ''), nullif(?, '')),
            origin = coalesce(nullif(origin, ''), nullif(?, '')),
            model = coalesce(nullif(model, ''), nullif(?, '')),
            model_label = coalesce(nullif(model_label, ''), nullif(?, '')),
            source_url = coalesce(nullif(source_url, ''), nullif(?, ''))
            WHERE id = ? AND user_id = ?")
            .bind(&metadata.prompt).bind(&metadata.negative_prompt).bind(&metadata.origin)
            .bind(&metadata.model).bind(&metadata.model_label).bind(&metadata.source_url)
            .bind(id).bind(user).execute(&mut *tx).await?;
        if updated.rows_affected() == 0 { return Err(EngineError::not_found("file")); }
        // The update above holds the write lock, so concurrent reimports cannot lose keys.
        let params: String = sqlx::query_scalar("SELECT params FROM assets WHERE id = ? AND user_id = ?")
            .bind(id).bind(user).fetch_one(&mut *tx).await?;
        let mut params: Map<String, Value> = serde_json::from_str(&params).map_err(anyhow::Error::from)?;
        for (key, value) in &metadata.params { params.entry(key.clone()).or_insert_with(|| value.clone()); }
        sqlx::query("UPDATE assets SET params = ? WHERE id = ? AND user_id = ?")
            .bind(serde_json::to_string(&params).map_err(anyhow::Error::from)?).bind(id).bind(user).execute(&mut *tx).await?;
        tx.commit().await?;
        let asset = store::asset_row(&self.inner.db, user, id).await?.to_asset();
        self.emit(user, "asset", json!(asset));
        Ok(asset)
    }

    pub async fn upload_with_metadata(&self, user: &str, bytes: &[u8], name: Option<&str>, mime: Option<&str>, metadata: &ImportMetadata) -> Result<Asset> {
        metadata.validate()?;
        let asset = self.upload(user, bytes, name, mime).await?;
        self.fill_asset_metadata(user, &asset.id, metadata).await
    }

    pub async fn assets(&self, user: &str, f: &AssetFilter) -> Result<(Vec<Asset>, Option<String>)> {
        for cost in [f.cost_min, f.cost_max].into_iter().flatten() {
            if !cost.is_finite() || cost < 0.0 { return Err(EngineError::invalid("Costs must be finite and nonnegative.", Some("cost_min"))); }
        }
        if matches!((f.cost_min, f.cost_max), (Some(min), Some(max)) if min > max) {
            return Err(EngineError::invalid("Minimum cost must not exceed maximum cost.", Some("cost_min")));
        }
        let limit = f.limit.unwrap_or(60).clamp(1, 500) as i64;
        let trash = f.trash.unwrap_or(false);
        let fts = f.q.as_deref().and_then(store::fts_query);
        let rows = sqlx::query_as::<_, AssetRow>(
            "SELECT a.*, (SELECT batch_id FROM jobs WHERE jobs.id = a.job_id AND jobs.user_id = a.user_id) AS batch_id FROM assets a LEFT JOIN spend_runs s ON s.job_id = a.job_id AND s.user_id = a.user_id
             WHERE a.user_id = ?
               AND ((? AND a.deleted_at IS NOT NULL) OR (NOT ? AND a.deleted_at IS NULL))
               AND (? IS NULL OR EXISTS (SELECT 1 FROM collection_assets ca JOIN collections c ON c.id = ca.collection_id WHERE ca.asset_id = a.id AND c.id = ? AND c.user_id = a.user_id))
               AND (? IS NULL OR EXISTS (SELECT 1 FROM asset_tags t WHERE t.asset_id = a.id AND t.tag = ?))
               AND (? IS NULL OR s.provider = ?)
               AND (? IS NULL OR COALESCE(s.actual_micros, s.estimated_micros) >= ? * 1000000)
               AND (? IS NULL OR COALESCE(s.actual_micros, s.estimated_micros) <= ? * 1000000)
               AND (? IS NULL OR a.kind = ?)
               AND (? IS NULL OR a.favorite = ?)
               AND (? IS NULL OR a.model = ?)
               AND ((? IS NULL AND a.source != 'edit') OR a.source = ?)
               AND (? IS NULL OR a.id < ?)
               AND (? IS NULL OR a.rowid IN (SELECT rowid FROM assets_fts WHERE assets_fts MATCH ?))
             ORDER BY a.id DESC LIMIT ?",
        )
        .bind(user)
        .bind(trash)
        .bind(trash)
        .bind(&f.collection).bind(&f.collection)
        .bind(&f.tag).bind(&f.tag)
        .bind(&f.provider).bind(&f.provider)
        .bind(f.cost_min).bind(f.cost_min)
        .bind(f.cost_max).bind(f.cost_max)
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

    /// Path, MIME type and download name of an asset's original file. A user sees their own files
    /// and the files on boards they can open.
    pub async fn asset_file(&self, user: Option<&str>, id: &str) -> Result<(std::path::PathBuf, String, String)> {
        let row = match user {
            Some(u) => self.visible_asset_row(u, id).await,
            None => Ok(sqlx::query_as::<_, AssetRow>("SELECT * FROM assets WHERE id = ? AND deleted_at IS NULL").bind(id).fetch_one(&self.inner.db).await?),
        }
        .map_err(|_| EngineError::not_found("file"))?;
        let name = format!("pictesa-{}.{}", row.id.to_lowercase(), row.ext);
        Ok((self.inner.files.path(&row.hash, &row.ext), row.mime, name))
    }

    pub async fn asset_thumb(&self, user: Option<&str>, id: &str) -> Result<std::path::PathBuf> {
        let row = match user {
            Some(u) => self.visible_asset_row(u, id).await,
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

    /// True when `key` is a valid, unexpired signed key for this asset (provider input links in hosted mode).
    pub fn file_key_ok(&self, asset_id: &str, key: &str, exp: i64) -> bool {
        if exp < now_ms() || exp > now_ms() + crate::queue::FILE_LINK_TTL_MS + 60_000 {
            return false;
        }
        let expected = crate::queue::file_key(self, asset_id, exp);
        expected.len() == key.len() && expected.bytes().zip(key.bytes()).fold(0u8, |a, (x, y)| a | (x ^ y)) == 0
    }
}

/// The prompt sent to the provider: the person's text plus the preset's words.
pub fn with_preset(prompt: &str, suffix: &str) -> String {
    match (prompt.trim().is_empty(), suffix.trim().is_empty()) {
        (_, true) => prompt.to_string(),
        (true, false) => suffix.to_string(),
        (false, false) => format!("{}, {}", prompt.trim_end().trim_end_matches(','), suffix),
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
            // Only models that declare passthrough accept settings outside their spec.
            None if spec.meta.get("passthrough").and_then(Value::as_bool) == Some(true) => {
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

fn with_markup(p: Pricing, m: f64) -> Pricing {
    let r = |usd: f64| (usd * m * 10_000.0).round() / 10_000.0;
    match p {
        Pricing::PerImage { usd } => Pricing::PerImage { usd: r(usd) },
        Pricing::PerSecond { usd } => Pricing::PerSecond { usd: r(usd) },
        Pricing::PerVideo { usd } => Pricing::PerVideo { usd: r(usd) },
        Pricing::PerMegapixel { usd } => Pricing::PerMegapixel { usd: r(usd) },
        other => other,
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
    let usd = (usd * 10_000.0).round() / 10_000.0;
    Some(if spec.is_batch(params) { usd * 0.5 } else { usd })
}


impl ImportMetadata {
    pub fn is_empty(&self) -> bool {
        let ImportMetadata { prompt, negative_prompt, origin, model, model_label, params, source_url } = self;
        [prompt, negative_prompt, origin, model, model_label, source_url].iter().all(|v| v.as_deref().is_none_or(str::is_empty)) && params.is_empty()
    }

    pub fn validate(&self) -> Result<()> {
        for (field, value, max) in [
            ("prompt", &self.prompt, 20_000), ("negative_prompt", &self.negative_prompt, 20_000),
            ("origin", &self.origin, 200), ("model", &self.model, 500),
            ("model_label", &self.model_label, 500), ("source_url", &self.source_url, 4000),
        ] {
            if value.as_ref().is_some_and(|v| v.chars().count() > max) {
                return Err(EngineError::invalid(format!("{field} is longer than {max} characters."), Some(field)));
            }
        }
        if self.params.len() > 64 || serde_json::to_vec(&self.params).map_err(anyhow::Error::from)?.len() > 64 * 1024 {
            return Err(EngineError::invalid("Import params must fit in 64 KB and contain at most 64 keys.", Some("params")));
        }
        Ok(())
    }
}

/// Result of turning a ComfyUI workflow into a model.
#[derive(Debug, Clone, serde::Serialize)]
pub struct ImportedWorkflow {
    pub model: String,
    pub manifest: Value,
    pub notes: Vec<String>,
}

impl Engine {
    /// Saves any ComfyUI workflow (API format) as a template with an inferred manifest and returns
    /// its model id. The model then appears in the picker, the CLI and the MCP tools.
    pub async fn import_comfy_workflow(&self, user: &str, name: &str, workflow: Value) -> Result<ImportedWorkflow> {
        // Writes files on the server and adds a model for every account, so hosted instances keep
        // it to the administrator. Checked here so the API, MCP and CLI all enforce it.
        if self.mode() == crate::Mode::Hosted && !self.user(user).await.is_ok_and(|u| u.role == "admin") {
            return Err(EngineError::Forbidden("Only the administrator can add ComfyUI workflows.".into()));
        }
        let name = name.trim();
        if name.is_empty() || name.chars().count() > 80 {
            return Err(EngineError::invalid("Give the workflow a short name.", Some("name")));
        }
        let provider = self.inner.catalog.provider("comfyui").ok_or_else(|| EngineError::not_found("ComfyUI provider"))?;
        let ctx = self.catalog_ctx(user, provider.as_ref()).await?;
        let dir = pictesa_providers::comfy_import::import_dir(&ctx).ok_or_else(|| EngineError::invalid("No folder for ComfyUI workflows is configured.", None))?;
        let (n, wf) = (name.to_string(), workflow);
        let (id, manifest, notes) = tokio::task::spawn_blocking(move || pictesa_providers::comfy_import::import(&dir, &n, &wf))
            .await
            .map_err(|e| EngineError::Internal(e.into()))?
            .map_err(|e| EngineError::invalid(e, Some("workflow")))?;
        let _ = self.refresh_models_for(user, "comfyui").await;
        Ok(ImportedWorkflow { model: format!("comfyui:{id}"), manifest, notes })
    }
}

fn batch_estimate(plans: &[Planned]) -> crate::batches::BatchEstimate {
    let total = plans.iter().flat_map(|p| &p.runs).try_fold(0.0, |total, (_, cost, charge)| {
        if *charge > 0 { Some(total + *charge as f64 / 1e6) } else { cost.map(|cost| total + cost) }
    });
    crate::batches::BatchEstimate { run_count: plans.iter().map(|p| p.runs.len()).sum(), total_estimated_cost_usd: total, max_runs: crate::batches::MAX_BATCH_RUNS }
}
