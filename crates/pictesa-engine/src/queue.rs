//! The job dispatcher: claims queued jobs within each provider's concurrency limit, submits
//! them, polls until done, downloads the outputs into the library and reports every step.

use std::time::Duration;

use base64::Engine as _;
use pictesa_core::{now_ms, GenerationRequest, MediaInput, Output, PollStatus, ProviderError, RemoteHandle, Submission};
use serde_json::json;
use tokio_util::sync::CancellationToken;

use crate::{
    store::{self, JobRow},
    Engine, JobStatus,
};

/// Longest a single generation may run before we give up polling.
const MAX_RUNTIME: Duration = Duration::from_secs(3 * 3600);
const MAX_SUBMIT_ATTEMPTS: u32 = 4;
const MAX_POLL_ERRORS: u32 = 30;
/// A running job whose heartbeat is older than this has lost its process and can be adopted.
const STALE_MS: i64 = 45_000;
/// How often a working job refreshes its heartbeat.
const HEARTBEAT: Duration = Duration::from_secs(5);

fn scoped(engine: &Engine) -> bool {
    engine.inner.scoped.load(std::sync::atomic::Ordering::SeqCst)
}

pub async fn run(engine: Engine) {
    let mut last_recovery: Option<tokio::time::Instant> = None;
    loop {
        if !scoped(&engine) && last_recovery.is_none_or(|t| t.elapsed() > Duration::from_secs(60)) {
            if let Err(e) = engine.recover().await {
                tracing::error!("job recovery failed: {e:#}");
            }
            last_recovery = Some(tokio::time::Instant::now());
        }
        if let Err(e) = dispatch_once(&engine).await {
            tracing::error!("dispatcher error: {e:#}");
        }
        // Wake on new jobs or finished ones; poll the table every few seconds as a fallback
        // (another process, like the CLI, may have queued work in the same database).
        let _ = tokio::time::timeout(Duration::from_secs(3), engine.inner.wake.notified()).await;
    }
}

async fn dispatch_once(engine: &Engine) -> anyhow::Result<()> {
    let queued: Vec<JobRow> = sqlx::query_as("SELECT * FROM jobs WHERE status = 'queued' ORDER BY id LIMIT 100").fetch_all(&engine.inner.db).await?;
    for job in queued {
        if scoped(engine) && !engine.inner.owned_batches.lock().unwrap().contains(&job.batch_id) {
            continue;
        }
        let Some(provider) = engine.inner.catalog.provider(&job.provider) else {
            fail(engine, &job, "not_supported", &format!("provider {} is not available in this build", job.provider)).await;
            continue;
        };
        let limit = provider.info().max_concurrency.max(1);
        {
            let running = engine.inner.running.lock().unwrap();
            if running.get(&job.provider).copied().unwrap_or(0) >= limit {
                continue;
            }
        }
        let now = now_ms();
        let claimed = sqlx::query("UPDATE jobs SET status = 'running', started_at = ?, updated_at = ?, message = 'Starting' WHERE id = ? AND status = 'queued'")
            .bind(now)
            .bind(now)
            .bind(&job.id)
            .execute(&engine.inner.db)
            .await?;
        if claimed.rows_affected() != 1 {
            continue;
        }
        spawn_job(engine, job);
    }
    Ok(())
}

pub(crate) fn spawn_job(engine: &Engine, job: JobRow) {
    *engine.inner.running.lock().unwrap().entry(job.provider.clone()).or_default() += 1;
    let token = CancellationToken::new();
    engine.inner.cancels.lock().unwrap().insert(job.id.clone(), token.clone());
    let engine = engine.clone();
    tokio::spawn(async move {
        let id = job.id.clone();
        let provider = job.provider.clone();
        process(&engine, job, token).await;
        engine.inner.cancels.lock().unwrap().remove(&id);
        if let Some(n) = engine.inner.running.lock().unwrap().get_mut(&provider) {
            *n = n.saturating_sub(1);
        }
        engine.inner.wake.notify_one();
    });
}

async fn publish(engine: &Engine, job_id: &str) {
    if let Ok(row) = store::job_row_any(&engine.inner.db, job_id).await {
        if let Ok(view) = store::job_view(&engine.inner.db, &row).await {
            engine.emit(&row.user_id, "job", json!(view));
        }
    }
}

async fn set_progress(engine: &Engine, job: &JobRow, progress: Option<f32>, message: Option<String>) {
    let _ = sqlx::query("UPDATE jobs SET progress = ?, message = ?, updated_at = ? WHERE id = ? AND status = 'running'")
        .bind(progress.map(|p| p.clamp(0.0, 1.0) as f64))
        .bind(message)
        .bind(now_ms())
        .bind(&job.id)
        .execute(&engine.inner.db)
        .await;
    publish(engine, &job.id).await;
}

async fn fail(engine: &Engine, job: &JobRow, kind: &str, message: &str) {
    let now = now_ms();
    let _ = sqlx::query("UPDATE jobs SET status = 'failed', error_kind = ?, error = ?, message = NULL, finished_at = ?, updated_at = ? WHERE id = ? AND status IN ('queued', 'running')")
        .bind(kind)
        .bind(pictesa_core::error::truncate(message, 2000))
        .bind(now)
        .bind(now)
        .bind(&job.id)
        .execute(&engine.inner.db)
        .await;
    publish(engine, &job.id).await;
}

async fn is_canceled(engine: &Engine, job: &JobRow, token: &CancellationToken) -> bool {
    if token.is_cancelled() {
        return true;
    }
    matches!(store::job_row_any(&engine.inner.db, &job.id).await, Ok(r) if r.status() == JobStatus::Canceled)
}

async fn process(engine: &Engine, job: JobRow, token: CancellationToken) {
    publish(engine, &job.id).await;
    let Some(provider) = engine.inner.catalog.provider(&job.provider) else { return };
    let ctx = match engine.ctx_for(&job.user_id, provider.as_ref()).await {
        Ok(c) => c,
        Err(e) => return fail(engine, &job, "auth", &e.to_string()).await,
    };
    let spec = match engine.inner.catalog.resolve(&engine.inner.db, &ctx, &job.model).await {
        Ok(s) => s,
        Err(e) => return fail(engine, &job, "not_supported", &format!("{e:#}")).await,
    };

    // Resume a job that was already submitted before a restart; otherwise submit it now.
    let mut handle: Option<RemoteHandle> = job.remote.as_deref().and_then(|r| serde_json::from_str(r).ok());
    let mut outputs: Option<Vec<Output>> = None;
    if handle.is_none() {
        let req = match build_request(engine, &job, &spec).await {
            Ok(r) => r,
            Err(e) => return fail(engine, &job, "invalid_input", &e).await,
        };
        let mut attempt = 0;
        loop {
            attempt += 1;
            let _ = sqlx::query("UPDATE jobs SET attempts = attempts + 1, message = 'Submitting', updated_at = ? WHERE id = ?").bind(now_ms()).bind(&job.id).execute(&engine.inner.db).await;
            let result = tokio::select! {
                r = provider.submit(&ctx, &spec, &req) => r,
                _ = token.cancelled() => return,
            };
            match result {
                Ok(Submission::Done(out)) => {
                    outputs = Some(out);
                    break;
                }
                Ok(Submission::Pending(h)) => {
                    let _ = sqlx::query("UPDATE jobs SET remote = ?, message = 'Submitted', updated_at = ? WHERE id = ?")
                        .bind(serde_json::to_string(&h).unwrap_or_default())
                        .bind(now_ms())
                        .bind(&job.id)
                        .execute(&engine.inner.db)
                        .await;
                    handle = Some(h);
                    break;
                }
                Err(e) if e.is_retryable() && attempt < MAX_SUBMIT_ATTEMPTS => {
                    let wait = e.retry_after().unwrap_or(Duration::from_millis(1500 * 2u64.pow(attempt)));
                    set_progress(engine, &job, None, Some(format!("Provider busy, retrying in {}s", wait.as_secs().max(1)))).await;
                    tokio::select! { _ = tokio::time::sleep(wait) => {}, _ = token.cancelled() => return }
                }
                Err(e) => return fail(engine, &job, e.kind(), &e.to_string()).await,
            }
        }
        publish(engine, &job.id).await;
    }

    if outputs.is_none() {
        let handle = handle.expect("handle set when not done");
        let started = tokio::time::Instant::now();
        let mut attempt: u32 = 0;
        let mut errors: u32 = 0;
        let mut last: Option<(Option<f32>, Option<String>)> = None;
        let mut beat = tokio::time::Instant::now();
        loop {
            if beat.elapsed() > HEARTBEAT {
                let _ = sqlx::query("UPDATE jobs SET updated_at = ? WHERE id = ? AND status = 'running'").bind(now_ms()).bind(&job.id).execute(&engine.inner.db).await;
                beat = tokio::time::Instant::now();
            }
            if is_canceled(engine, &job, &token).await {
                let _ = provider.cancel(&ctx, &handle).await;
                return publish(engine, &job.id).await;
            }
            if started.elapsed() > MAX_RUNTIME {
                let _ = provider.cancel(&ctx, &handle).await;
                return fail(engine, &job, "timeout", "The provider did not finish within 3 hours.").await;
            }
            tokio::select! { _ = tokio::time::sleep(provider.poll_interval(attempt)) => {}, _ = token.cancelled() => {
                let _ = provider.cancel(&ctx, &handle).await;
                return publish(engine, &job.id).await;
            } }
            attempt += 1;
            match provider.poll(&ctx, &handle).await {
                Ok(PollStatus::Done(out)) => {
                    outputs = Some(out);
                    break;
                }
                Ok(PollStatus::Failed(e)) => return fail(engine, &job, e.kind(), &e.to_string()).await,
                Ok(PollStatus::Queued { position }) => {
                    errors = 0;
                    let msg = Some(match position {
                        Some(p) => format!("In queue (position {})", p + 1),
                        None => "In queue".to_string(),
                    });
                    if last.as_ref() != Some(&(None, msg.clone())) {
                        set_progress(engine, &job, None, msg.clone()).await;
                        last = Some((None, msg));
                    }
                }
                Ok(PollStatus::Running { progress, message }) => {
                    errors = 0;
                    let msg = message.or_else(|| Some("Generating".into()));
                    if last.as_ref() != Some(&(progress, msg.clone())) {
                        set_progress(engine, &job, progress, msg.clone()).await;
                        last = Some((progress, msg));
                    }
                }
                Err(e) if e.is_retryable() || matches!(e, ProviderError::Failed(_)) && errors < 3 => {
                    errors += 1;
                    if errors > MAX_POLL_ERRORS {
                        return fail(engine, &job, e.kind(), &format!("Lost contact with the provider: {e}")).await;
                    }
                    if let Some(wait) = e.retry_after() {
                        tokio::time::sleep(wait).await;
                    }
                }
                Err(e) => return fail(engine, &job, e.kind(), &e.to_string()).await,
            }
        }
    }

    let outputs = outputs.unwrap_or_default();
    if outputs.is_empty() {
        return fail(engine, &job, "failed", "The provider finished without returning any media.").await;
    }
    set_progress(engine, &job, Some(0.99), Some("Saving".into())).await;
    let mut saved = 0;
    let mut last_error = None;
    for out in outputs {
        match save_output(engine, &job, out).await {
            Ok(_) => saved += 1,
            Err(e) => {
                tracing::warn!("saving output of {} failed: {e:#}", job.id);
                last_error = Some(e);
            }
        }
    }
    if saved == 0 {
        return fail(engine, &job, "failed", &format!("Could not download the result: {:#}", last_error.unwrap())).await;
    }
    let now = now_ms();
    let _ = sqlx::query("UPDATE jobs SET status = 'succeeded', progress = 1.0, message = NULL, finished_at = ?, updated_at = ? WHERE id = ? AND status = 'running'")
        .bind(now)
        .bind(now)
        .bind(&job.id)
        .execute(&engine.inner.db)
        .await;
    publish(engine, &job.id).await;
}

async fn build_request(engine: &Engine, job: &JobRow, spec: &pictesa_core::ModelSpec) -> Result<GenerationRequest, String> {
    let mut inputs = Vec::new();
    for input in job.inputs_list() {
        let row = store::asset_row(&engine.inner.db, &job.user_id, &input.asset_id).await.map_err(|_| "An input file was deleted.".to_string())?;
        let path = engine.inner.files.path(&row.hash, &row.ext);
        let url = engine.inner.cfg.public_url.as_ref().map(|base| format!("{}/files/{}?k={}", base.trim_end_matches('/'), row.id, file_key(engine, &row.id)));
        inputs.push(MediaInput {
            role: input.role,
            kind: store::parse_kind(&row.kind),
            mime: row.mime.clone(),
            path,
            url: url.or(row.source_url.clone()),
            width: row.width.map(|w| w as u32),
            height: row.height.map(|h| h as u32),
        });
    }
    Ok(GenerationRequest {
        job_id: job.id.clone(),
        model: spec.remote_id().to_string(),
        prompt: job.prompt.clone(),
        negative_prompt: job.negative_prompt.clone(),
        params: job.params_map(),
        inputs,
        num_outputs: job.count.max(1) as u32,
    })
}

/// Unguessable per-file key so providers can fetch inputs by URL in hosted mode.
pub fn file_key(engine: &Engine, asset_id: &str) -> String {
    engine.inner.crypto.sign(asset_id)
}

async fn save_output(engine: &Engine, job: &JobRow, out: Output) -> anyhow::Result<()> {
    let stored = match out {
        Output::Bytes { data, mime } => engine.inner.files.store_bytes(&data, Some(&mime), None).await?,
        Output::Url { url, headers } if url.starts_with("data:") => {
            let (meta, b64) = url.split_once(',').ok_or_else(|| anyhow::anyhow!("bad data URI"))?;
            let mime = meta.trim_start_matches("data:").split(';').next().unwrap_or("").to_string();
            let data = base64::engine::general_purpose::STANDARD.decode(b64.trim())?;
            let _ = headers;
            engine.inner.files.store_bytes(&data, Some(&mime), None).await?
        }
        Output::Url { url, headers } => {
            let mut last = None;
            let mut stored = None;
            for attempt in 0..3 {
                let mut req = engine.inner.download.get(&url);
                for (k, v) in &headers {
                    req = req.header(k, v);
                }
                match req.send().await {
                    Ok(resp) if resp.status().is_success() => match engine.inner.files.store_response(resp, Some(&url)).await {
                        Ok(s) => {
                            stored = Some(s);
                            break;
                        }
                        Err(e) => last = Some(e),
                    },
                    Ok(resp) => last = Some(anyhow::anyhow!("HTTP {}", resp.status())),
                    Err(e) => last = Some(e.into()),
                }
                tokio::time::sleep(Duration::from_secs(2 << attempt)).await;
            }
            stored.ok_or_else(|| last.unwrap_or_else(|| anyhow::anyhow!("download failed")))?
        }
    };
    engine.insert_asset(&job.user_id, &stored, "generated", Some(job), None).await?;
    Ok(())
}

impl Engine {
    /// Adopts running jobs whose process died (stale heartbeat): resumes the ones the provider
    /// already accepted and fails the ones interrupted mid-submit (re-submitting could charge twice).
    pub(crate) async fn recover(&self) -> anyhow::Result<()> {
        let running: Vec<JobRow> = sqlx::query_as("SELECT * FROM jobs WHERE status = 'running' AND updated_at < ?")
            .bind(now_ms() - STALE_MS)
            .fetch_all(&self.inner.db)
            .await?;
        for job in running {
            if self.inner.cancels.lock().unwrap().contains_key(&job.id) {
                continue;
            }
            // Take ownership atomically so two servers never adopt the same job.
            let taken = sqlx::query("UPDATE jobs SET updated_at = ? WHERE id = ? AND status = 'running' AND updated_at = ?")
                .bind(now_ms())
                .bind(&job.id)
                .bind(job.updated_at)
                .execute(&self.inner.db)
                .await?;
            if taken.rows_affected() != 1 {
                continue;
            }
            if job.remote.is_some() {
                spawn_job(self, job);
            } else {
                fail(self, &job, "interrupted", "Pictesa stopped before the provider confirmed this job. Retry it to run it again.").await;
            }
        }
        Ok(())
    }
}
