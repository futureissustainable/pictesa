//! The job dispatcher: claims queued jobs within each provider's concurrency limit, submits
//! them, polls until done, downloads the outputs into the library and reports every step.
//!
//! Safety rules:
//! - A job's heartbeat (`updated_at`) is refreshed every few seconds for its whole lifetime, so
//!   another process only adopts it after this one really died.
//! - The status is re-checked right before the provider is called, so a cancel never races a submit.
//! - A failed run is refunded only when the provider never billed it (rejected before or at submit,
//!   or reported the job as failed). Timeouts, lost contact and download failures keep the charge.

use std::time::Duration;

use base64::Engine as _;
use pictesa_core::{now_ms, GenerationRequest, MediaInput, Output, PollStatus, ProviderError, RemoteHandle, Submission};
use serde_json::json;
use tokio_util::sync::CancellationToken;

use crate::{
    creds::Sources,
    store::{self, JobRow},
    Engine, JobStatus,
};

/// Longest a single generation may run before we give up polling.
const MAX_RUNTIME: Duration = Duration::from_secs(3 * 3600);
// Allow the provider's 24-hour processing window plus time to finalize results.
const MAX_BATCH_RUNTIME: Duration = Duration::from_secs(25 * 3600);
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
            engine.fail_orphan_runs().await;
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
    // One scan per provider, so a long backlog on one provider never starves the others.
    let providers: Vec<(String,)> = sqlx::query_as("SELECT DISTINCT provider FROM jobs WHERE status = 'queued'").fetch_all(&engine.inner.db).await?;
    for (pid,) in providers {
        let Some(provider) = engine.inner.catalog.provider(&pid) else {
            if scoped(engine) {
                continue;
            }
            let orphans: Vec<JobRow> = sqlx::query_as("SELECT * FROM jobs WHERE status = 'queued' AND provider = ? LIMIT 50").bind(&pid).fetch_all(&engine.inner.db).await?;
            for job in orphans {
                fail(engine, &job, "not_supported", &format!("The provider {pid} is not available in this build."), true).await;
            }
            continue;
        };
        let limit = provider.info().max_concurrency.max(1);
        let running = engine.inner.running.lock().unwrap().get(&pid).copied().unwrap_or(0);
        if running >= limit {
            continue;
        }
        let queued: Vec<JobRow> = sqlx::query_as("SELECT * FROM jobs WHERE status = 'queued' AND provider = ? ORDER BY id LIMIT ?")
            .bind(&pid)
            .bind(((limit - running) * 4) as i64)
            .fetch_all(&engine.inner.db)
            .await?;
        for job in queued {
            if engine.inner.running.lock().unwrap().get(&pid).copied().unwrap_or(0) >= limit {
                break;
            }
            if scoped(engine) && !engine.inner.owned_batches.lock().unwrap().contains(&job.batch_id) {
                continue;
            }
            let now = now_ms();
            let claimed = sqlx::query("UPDATE jobs SET status = 'running', started_at = ?, updated_at = ?, message = 'Starting', owner = ? WHERE id = ? AND status = 'queued' AND (provider != 'ai-toolkit' OR NOT EXISTS (SELECT 1 FROM jobs active WHERE active.provider = 'ai-toolkit' AND (active.status = 'running' OR (active.status = 'canceled' AND active.owner IS NOT NULL AND active.updated_at > ?))))")
                .bind(now)
                .bind(now)
                .bind(&engine.inner.instance)
                .bind(&job.id)
                .bind(now - STALE_MS)
                .execute(&engine.inner.db)
                .await?;
            if claimed.rows_affected() == 1 {
                spawn_job(engine, job);
            }
        }
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
        // Heartbeat for the whole lifetime of the job, including long submits and downloads.
        let beat = {
            let (engine, id) = (engine.clone(), id.clone());
            tokio::spawn(async move {
                loop {
                    tokio::time::sleep(HEARTBEAT).await;
                    let _ = sqlx::query("UPDATE jobs SET updated_at = ? WHERE id = ? AND status = 'running' AND owner = ?")
                        .bind(now_ms())
                        .bind(&id)
                        .bind(&engine.inner.instance)
                        .execute(&engine.inner.db)
                        .await;
                }
            })
        };
        // Run the job in its own task so a panic in provider code cannot leak the slot.
        let work = {
            let (engine, job) = (engine.clone(), job.clone());
            tokio::spawn(async move { process(&engine, job, token).await })
        };
        if let Err(e) = work.await {
            tracing::error!("job {id} crashed: {e}");
            let refund = matches!(store::job_row_any(&engine.inner.db, &id).await, Ok(r) if never_sent(&r));
            fail(&engine, &job, "internal", "Pictesa hit an internal error while running this job. Retry it, and report it if it happens again.", refund).await;
        }
        beat.abort();
        let _=sqlx::query("UPDATE jobs SET owner = NULL WHERE id = ? AND owner = ? AND status IN ('succeeded','failed','canceled')")
            .bind(&id).bind(&engine.inner.instance).execute(&engine.inner.db).await;
        engine.inner.previews.lock().unwrap().remove(&id);
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

/// Marks a job failed. `refund` returns its wallet charge (only when the provider did not bill it).
async fn fail(engine: &Engine, job: &JobRow, kind: &str, message: &str, refund: bool) {
    let now = now_ms();
    let changed = sqlx::query("UPDATE jobs SET status = 'failed', error_kind = ?, error = ?, message = NULL, finished_at = ?, updated_at = ? WHERE id = ? AND status IN ('queued', 'running') AND (owner IS NULL OR owner = ?)")
        .bind(kind)
        .bind(pictesa_core::error::truncate(message, 2000))
        .bind(now)
        .bind(now)
        .bind(&job.id)
        .bind(&engine.inner.instance)
        .execute(&engine.inner.db)
        .await;
    if refund && matches!(changed, Ok(r) if r.rows_affected() == 1) {
        engine.refund_job(&job.id).await;
    }
    publish(engine, &job.id).await;
}

/// True when the provider never received this job (so a failure can be refunded).
fn never_sent(job: &JobRow) -> bool {
    job.remote.is_none() && job.submitting == 0
}

/// A timed-out request may have reached the provider: never retried, never refunded.
fn is_timeout(e: &ProviderError) -> bool {
    matches!(e, ProviderError::Transient(m) if m.starts_with("request timed out"))
}

/// Text stored for a failure. Hosted runs against a user-chosen endpoint keep only the kind, so a
/// remote server's answer is never relayed.
fn failure_text(e: &ProviderError, guarded: bool) -> String {
    if !guarded {
        return e.to_string();
    }
    match e.kind() {
        "auth" => "The provider rejected the key.",
        "insufficient_credits" => "The provider account has no credits left.",
        "content_policy" => "The provider refused the prompt or inputs.",
        "invalid_input" => "The provider refused these settings.",
        "rate_limited" => "The provider is rate limiting.",
        _ => "The provider could not complete the run.",
    }
    .to_string()
}

async fn is_canceled(engine: &Engine, job: &JobRow, token: &CancellationToken) -> bool {
    if token.is_cancelled() {
        return true;
    }
    matches!(store::job_row_any(&engine.inner.db, &job.id).await, Ok(r) if r.status() == JobStatus::Canceled)
}

async fn process(engine: &Engine, job: JobRow, token: CancellationToken) {
    publish(engine, &job.id).await;
    let Some(provider) = engine.inner.catalog.provider(&job.provider) else {
        return fail(engine, &job, "not_supported", &format!("The provider {} is not available in this build.", job.provider), never_sent(&job)).await;
    };
    // Use exactly the credential source the job was created (and paid) with.
    let (ctx, guarded) = match engine.resolve_with(&job.user_id, provider.as_ref(), Sources::for_billing(&job.billing)).await {
        Ok(r) if Engine::is_configured(&provider.info(), &r) => {
            let guarded = engine.guarded(&r);
            (engine.ctx_from(r), guarded)
        }
        Ok(_) => return fail(engine, &job, "auth", "The key this job was created with is no longer available. Add it again in Settings > Providers and retry.", never_sent(&job)).await,
        Err(e) => return fail(engine, &job, "auth", &e.to_string(), never_sent(&job)).await,
    };
    let spec = match engine.model(&job.user_id, &job.model).await {
        Ok(s) => s,
        Err(e) => return fail(engine, &job, "not_supported", &e.to_string(), never_sent(&job)).await,
    };

    // Resume a job that was already submitted before a restart; otherwise submit it now.
    if job.remote.as_deref() == Some(DONE) {
        return fail(engine, &job, "interrupted", "Pictesa stopped while saving this job's results. Retry it to run it again.", false).await;
    }
    let mut handle: Option<RemoteHandle> = job.remote.as_deref().and_then(|r| serde_json::from_str(r).ok());
    let mut outputs: Option<Vec<Output>> = None;
    if handle.is_none() {
        if job.submitting == 1 {
            // A submit was in flight when its process died: the provider may have the job, so do not repeat it.
            return fail(engine, &job, "interrupted", "Pictesa stopped before the provider confirmed this job. Retry it to run it again.", false).await;
        }
        let req = match build_request(engine, &job, &spec).await {
            Ok(r) => r,
            Err(e) => return fail(engine, &job, "invalid_input", &e, true).await,
        };
        let mut attempt = 0;
        loop {
            attempt += 1;
            // Mark the submit in flight; this fails when the job was canceled or adopted meanwhile.
            let started = sqlx::query("UPDATE jobs SET attempts = attempts + 1, submitting = 1, message = 'Submitting', updated_at = ? WHERE id = ? AND status = 'running' AND owner = ?")
                .bind(now_ms())
                .bind(&job.id)
                .bind(&engine.inner.instance)
                .execute(&engine.inner.db)
                .await;
            if !matches!(started, Ok(r) if r.rows_affected() == 1) {
                return publish(engine, &job.id).await;
            }
            let result = if job.provider == "local" {
                tokio::select! {
                    result = provider.submit(&ctx, &spec, &req) => result,
                    _ = token.cancelled() => return publish(engine, &job.id).await,
                }
            } else {
                provider.submit(&ctx, &spec, &req).await
            };
            // Record the outcome before anything else, so a cancel or crash is judged correctly.
            let (remote, still_in_flight) = match &result {
                Ok(Submission::Done(_)) => (Some(DONE.to_string()), false),
                Ok(Submission::Pending(h)) => (Some(serde_json::to_string(h).unwrap_or_default()), false),
                Err(e) if is_timeout(e) => (None, true),
                Err(_) => (None, false),
            };
            if !still_in_flight {
                let _ = sqlx::query("UPDATE jobs SET submitting = 0, remote = coalesce(?, remote), updated_at = ? WHERE id = ?")
                    .bind(remote)
                    .bind(now_ms())
                    .bind(&job.id)
                    .execute(&engine.inner.db)
                    .await;
            }
            match result {
                Ok(Submission::Done(out)) => {
                    outputs = Some(out);
                    break;
                }
                Ok(Submission::Pending(h)) => {
                    let _ = sqlx::query("UPDATE jobs SET message = 'Submitted' WHERE id = ?").bind(&job.id).execute(&engine.inner.db).await;
                    handle = Some(h);
                    break;
                }
                // A timeout may have reached the provider: retrying could run (and bill) it twice.
                Err(e) if is_timeout(&e) => {
                    return fail(engine, &job, "interrupted", "The provider did not answer in time, and the job may have started there. Check its dashboard before retrying.", false).await
                }
                Err(e) if e.is_retryable() && attempt < MAX_SUBMIT_ATTEMPTS => {
                    let wait = e.retry_after().unwrap_or(Duration::from_millis(1500 * 2u64.pow(attempt)));
                    set_progress(engine, &job, None, Some(format!("Provider busy, retrying in {}s", wait.as_secs().max(1)))).await;
                    tokio::select! { _ = tokio::time::sleep(wait) => {}, _ = token.cancelled() => return publish(engine, &job.id).await }
                }
                Err(e) => return fail(engine, &job, e.kind(), &failure_text(&e, guarded), true).await,
            }
        }
        publish(engine, &job.id).await;
    }

    if outputs.is_none() {
        let handle = handle.expect("handle set when not done");
        let batch = handle.is_batch();
        let runtime = if batch || job.output == "lora" { MAX_BATCH_RUNTIME } else { MAX_RUNTIME };
        let started_at = job.started_at.unwrap_or_else(now_ms);
        let mut attempt: u32 = 0;
        let mut errors: u32 = 0;
        let mut last: Option<(Option<f32>, Option<String>)> = None;
        loop {
            if is_canceled(engine, &job, &token).await {
                let _ = provider.cancel(&ctx, &handle).await;
                return publish(engine, &job.id).await;
            }
            if now_ms().saturating_sub(started_at) as u64 > runtime.as_millis() as u64 {
                let _ = provider.cancel(&ctx, &handle).await;
                let message = if batch { "The batch provider did not finish within 25 hours." } else { "The provider did not finish within 3 hours." };
                return fail(engine, &job, "timeout", message, false).await;
            }
            tokio::select! {
                _ = tokio::time::sleep(provider.poll_interval_for(&handle, attempt)) => {}
                _ = token.cancelled() => continue,
            }
            attempt += 1;
            let polled = tokio::select! {
                r = provider.poll(&ctx, &handle) => r,
                _ = token.cancelled() => continue,
            };
            match polled {
                Ok(PollStatus::Done(out)) => {
                    outputs = Some(out);
                    break;
                }
                // The provider reports the job failed; providers do not bill failed generations.
                Ok(PollStatus::Failed(e)) => return fail(engine, &job, e.kind(), &failure_text(&e, guarded), true).await,
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
                    if let Some((bytes, mime)) = provider.preview(&handle) {
                        let fresh = {
                            let mut previews = engine.inner.previews.lock().unwrap();
                            let changed = previews.get(&job.id).is_none_or(|(b, _, _)| *b != bytes);
                            if changed {
                                let version = previews.get(&job.id).map(|p| p.2 + 1).unwrap_or(1);
                                previews.insert(job.id.clone(), (bytes, mime, version));
                                Some(version)
                            } else {
                                None
                            }
                        };
                        if let Some(version) = fresh {
                            engine.emit(&job.user_id, "preview", json!({ "job_id": job.id, "version": version, "url": format!("/api/generations/{}/preview?v={version}", job.id) }));
                        }
                    }
                    let msg = message.or_else(|| Some("Generating".into()));
                    if last.as_ref() != Some(&(progress, msg.clone())) {
                        set_progress(engine, &job, progress, msg.clone()).await;
                        last = Some((progress, msg));
                    }
                }
                Err(e) if e.is_retryable() || matches!(e, ProviderError::Failed(_)) && errors < 3 => {
                    errors += 1;
                    if errors > MAX_POLL_ERRORS {
                        return fail(engine, &job, "lost_contact", &format!("Lost contact with the provider: {}", failure_text(&e, guarded)), false).await;
                    }
                    if let Some(wait) = e.retry_after() {
                        tokio::time::sleep(wait).await;
                    }
                }
                Err(e) => return fail(engine, &job, e.kind(), &failure_text(&e, guarded), false).await,
            }
        }
    }

    let outputs = outputs.unwrap_or_default();
    if outputs.is_empty() {
        return fail(engine, &job, "failed", "The provider finished without returning any media.", false).await;
    }
    // The provider finished (and billed). Keep the results even if a cancel arrived too late.
    set_progress(engine, &job, Some(0.99), Some("Saving".into())).await;
    let mut saved = 0;
    let mut last_error = None;
    for out in outputs {
        match save_output(engine, &job, out, guarded).await {
            Ok(_) => saved += 1,
            Err(e) => {
                tracing::warn!("saving output of {} failed: {e:#}", job.id);
                last_error = Some(e);
            }
        }
    }
    if saved == 0 {
        let _ = sqlx::query("UPDATE jobs SET status = 'running' WHERE id = ? AND status = 'canceled'").bind(&job.id).execute(&engine.inner.db).await;
        return fail(engine, &job, "download_failed", &format!("Could not download the result: {:#}", last_error.unwrap()), false).await;
    }
    // Before the job reads as finished, so whoever waits for it sees the real cost.
    if let Some(usd) = ctx.reported_cost() {
        record_actual_cost(engine, &job, usd).await;
    }
    let now = now_ms();
    let _ = sqlx::query("UPDATE jobs SET status = 'succeeded', progress = 1.0, message = NULL, finished_at = ?, updated_at = ? WHERE id = ? AND status IN ('running', 'canceled') AND owner = ?")
        .bind(now)
        .bind(now)
        .bind(&job.id)
        .bind(&engine.inner.instance)
        .execute(&engine.inner.db)
        .await;
    publish(engine, &job.id).await;
}

/// Stores the charge the provider reported as the run's actual cost. Only runs on the person's own
/// key change: wallet-paid runs keep what the wallet took, and free runs stay free.
async fn record_actual_cost(engine: &Engine, job: &JobRow, usd: f64) {
    let _ = sqlx::query("UPDATE spend_runs SET actual_micros = ? WHERE job_id = ? AND user_id = ? AND actual_micros IS NULL")
        .bind(crate::billing::to_micros(usd))
        .bind(&job.id)
        .bind(&job.user_id)
        .execute(&engine.inner.db)
        .await;
    let _ = sqlx::query("UPDATE jobs SET cost_usd = ? WHERE id = ? AND charged_micros = 0").bind(usd).bind(&job.id).execute(&engine.inner.db).await;
}

/// Providers that only accept inputs as public HTTPS links.
const NEEDS_PUBLIC_LINKS: &[&str] = &["luma", "freepik"];

async fn build_request(engine: &Engine, job: &JobRow, spec: &pictesa_core::ModelSpec) -> Result<GenerationRequest, String> {
    let mut inputs = Vec::new();
    for input in job.inputs_list() {
        let row = store::asset_row(&engine.inner.db, &job.user_id, &input.asset_id).await.map_err(|_| "An input file was deleted.".to_string())?;
        let path = engine.inner.files.path(&row.hash, &row.ext);
        // Hosted servers hand providers a short-lived signed link; local inputs are sent as bytes,
        // or parked on fal's storage for providers that only take links.
        let mut url = engine.inner.cfg.public_url.as_ref().map(|base| signed_file_url(engine, base, &row.id));
        if url.is_none() && NEEDS_PUBLIC_LINKS.contains(&job.provider.as_str()) {
            url = Some(engine.temporary_link(&job.user_id, &path, &row.mime, &row.hash).await.map_err(|e| {
                format!("{} needs inputs as public links. Set up fal (used to host them temporarily) or run Pictesa with --public-url. ({e})", spec.name)
            })?);
        }
        inputs.push(MediaInput {
            role: input.role,
            kind: store::parse_kind(&row.kind),
            mime: row.mime.clone(),
            path,
            url,
            width: row.width.map(|w| w as u32),
            height: row.height.map(|h| h as u32),
        });
    }
    let suffix = job.preset.as_deref().and_then(|p| engine.preset(p)).map(|p| p.prompt_suffix).unwrap_or_default();
    let mut params=job.params_map();
    engine.lora_request_params(&job.user_id,&job.provider,&mut params).await.map_err(|e|e.to_string())?;
    Ok(GenerationRequest {
        job_id: job.id.clone(),
        model: spec.remote_id().to_string(),
        prompt: crate::jobs::with_preset(&job.prompt, &suffix),
        negative_prompt: job.negative_prompt.clone(),
        params,
        inputs,
        num_outputs: job.count.max(1) as u32,
    })
}

/// Validity of signed input links handed to providers.
pub const FILE_LINK_TTL_MS: i64 = 6 * 3600 * 1000;

pub(crate) fn signed_file_url(engine: &Engine, base: &str, asset_id: &str) -> String {
    let exp = now_ms() + FILE_LINK_TTL_MS;
    format!("{}/files/{asset_id}?e={exp}&k={}", base.trim_end_matches('/'), file_key(engine, asset_id, exp))
}

/// Unguessable, expiring key so providers can fetch inputs by URL in hosted mode.
pub fn file_key(engine: &Engine, asset_id: &str, exp: i64) -> String {
    engine.inner.crypto.sign(&format!("{asset_id}.{exp}"))
}

/// Marker stored in `remote` once a synchronous provider has returned results.
const DONE: &str = "done";

async fn save_output(engine: &Engine, job: &JobRow, out: Output, guarded: bool) -> anyhow::Result<()> {
    let client = if guarded { &engine.inner.download_guarded } else { &engine.inner.download };
    let stored = match out {
        Output::File { path, mime, cleanup_dir } => {
            let result=async {
                let file=tokio::fs::File::open(&path).await?;
                engine.inner.files.store_stream(tokio_util::io::ReaderStream::new(file),Some(&mime),path.to_str(),true).await
            }.await;
            let _=tokio::fs::remove_dir_all(cleanup_dir).await;
            result?
        },
        Output::Bytes { data, mime } => engine.inner.files.store_bytes(&data, Some(&mime), None, true).await?,
        Output::Url { url, .. } if url.starts_with("data:") => {
            let (meta, b64) = url.split_once(',').ok_or_else(|| anyhow::anyhow!("bad data URI"))?;
            let mime = meta.trim_start_matches("data:").split(';').next().unwrap_or("").to_string();
            let data = base64::engine::general_purpose::STANDARD.decode(b64.trim())?;
            engine.inner.files.store_bytes(&data, Some(&mime), None, true).await?
        }
        Output::Url { url, headers } => {
            let mut last = None;
            let mut stored = None;
            if guarded && url::Url::parse(&url).map(|u| crate::netguard::literal_private(&u)).unwrap_or(true) {
                anyhow::bail!("the result link points to a private address");
            }
            for attempt in 0..3 {
                let mut req = client.get(&url);
                for (k, v) in &headers {
                    req = req.header(k, v);
                }
                match req.send().await {
                    Ok(resp) if resp.status().is_success() => match engine.inner.files.store_response(resp, if job.output == "lora" { Some("weights.safetensors") } else { Some(&url) }, true).await {
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
    if job.output == "lora" && stored.kind != pictesa_core::MediaKind::Lora { anyhow::bail!("Trainer returned the wrong output file type."); }
    let asset=engine.insert_asset(&job.user_id, &stored, "generated", Some(job), None).await?;
    if let Err(e)=engine.register_lora(job,&asset).await {
        let _=sqlx::query("DELETE FROM assets WHERE id=?").bind(&asset.id).execute(&engine.inner.db).await;
        return Err(anyhow::anyhow!(e));
    }
    Ok(())
}

impl Engine {
    /// Uploads a local file to fal's storage (with the user's fal key) and returns its public link.
    /// Cached per file hash for the life of the process.
    pub(crate) async fn temporary_link(&self, user: &str, path: &std::path::Path, mime: &str, hash: &str) -> Result<String, String> {
        // fal keeps uploads for a limited time, so cached links are reused for a few hours only.
        static CACHE: std::sync::OnceLock<std::sync::Mutex<std::collections::HashMap<String, (String, i64)>>> = std::sync::OnceLock::new();
        let cache = CACHE.get_or_init(Default::default);
        if let Some((u, at)) = cache.lock().unwrap().get(hash) {
            if now_ms() - at < 3 * 3600 * 1000 {
                return Ok(u.clone());
            }
        }
        let fal = self.inner.catalog.provider("fal").ok_or("fal is not available")?;
        let ctx = self.ctx_for(user, fal.as_ref()).await.map_err(|e| e.to_string())?;
        let key = ctx.get("api_key").ok_or("fal is not set up")?.to_string();
        let init: serde_json::Value = self
            .inner
            .http
            .post("https://rest.alpha.fal.ai/storage/upload/initiate?storage_type=fal-cdn-v3")
            .header("Authorization", format!("Key {key}"))
            .json(&json!({ "content_type": mime, "file_name": path.file_name().and_then(|n| n.to_str()).unwrap_or("input") }))
            .send()
            .await
            .map_err(|e| e.to_string())?
            .error_for_status()
            .map_err(|e| e.to_string())?
            .json()
            .await
            .map_err(|e| e.to_string())?;
        let (Some(put), Some(file_url)) = (init.get("upload_url").and_then(|v| v.as_str()), init.get("file_url").and_then(|v| v.as_str())) else {
            return Err("fal storage gave no upload link".into());
        };
        // Streamed from disk: a large video never sits in memory.
        let file = tokio::fs::File::open(path).await.map_err(|e| e.to_string())?;
        let len = file.metadata().await.map_err(|e| e.to_string())?.len();
        let body = reqwest::Body::wrap_stream(tokio_util::io::ReaderStream::new(file));
        self.inner
            .download
            .put(put)
            .header("Content-Type", mime)
            .header("Content-Length", len)
            .body(body)
            .send()
            .await
            .map_err(|e| e.to_string())?
            .error_for_status()
            .map_err(|e| e.to_string())?;
        cache.lock().unwrap().insert(hash.to_string(), (file_url.to_string(), now_ms()));
        Ok(file_url.to_string())
    }

    /// Adopts running jobs whose process died (stale heartbeat): resumes the ones the provider
    /// accepted, restarts never-submitted ones, and fails the ones interrupted mid-submit without
    /// a refund (the provider may have billed).
    pub(crate) async fn recover(&self) -> anyhow::Result<()> {
        let running: Vec<JobRow> = sqlx::query_as("SELECT * FROM jobs WHERE status = 'running' AND updated_at < ?")
            .bind(now_ms() - STALE_MS)
            .fetch_all(&self.inner.db)
            .await?;
        for job in running {
            if self.inner.cancels.lock().unwrap().contains_key(&job.id) {
                continue;
            }
            // Take ownership atomically so two processes never adopt the same job; the old owner's
            // writes stop matching from here on.
            let taken = sqlx::query("UPDATE jobs SET updated_at = ?, owner = ? WHERE id = ? AND status = 'running' AND updated_at = ?")
                .bind(now_ms())
                .bind(&self.inner.instance)
                .bind(&job.id)
                .bind(job.updated_at)
                .execute(&self.inner.db)
                .await?;
            if taken.rows_affected() != 1 {
                continue;
            }
            let mut job = job;
            job.owner = Some(self.inner.instance.clone());
            // Accepted jobs resume polling; jobs whose submits were all refused start over;
            // `process` fails the ambiguous ones (a submit in flight, or results half saved).
            spawn_job(self, job);
        }
        Ok(())
    }
}
