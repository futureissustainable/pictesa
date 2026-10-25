//! End-to-end engine flows on the free mock provider: the real queue, files, recipes and billing.
//! Each test uses its own temporary data folder. Environment variables are process-wide, so the
//! mock is enabled once and the tests share it.

use std::time::Duration;

use pictesa_engine::{Config, CreateGeneration, Engine, JobStatus, Mode, LOCAL_USER};
use serde_json::{json, Map};

fn temp_dir(name: &str) -> std::path::PathBuf {
    let dir = std::env::temp_dir().join(format!("pictesa-test-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    dir
}

async fn engine(name: &str, mode: Mode) -> Engine {
    // Set before any engine reads them, and identical in every test, so parallel tests agree.
    std::env::set_var("PICTESA_MOCK", "1");
    std::env::set_var("PICTESA_MOCK_KEY", "platform-test-key");
    std::env::set_var("PICTESA_BILLING", "1");
    let mut cfg = Config::local(temp_dir(name));
    cfg.mode = mode;
    let e = Engine::open(cfg).await.expect("engine opens");
    e.start();
    e
}

fn req(model: &str, prompt: &str) -> CreateGeneration {
    CreateGeneration { model: model.into(), prompt: prompt.into(), ..Default::default() }
}

#[tokio::test(flavor = "multi_thread")]
async fn image_generation_lands_in_the_library() {
    let e = engine("image", Mode::Local).await;
    let mut r = req("mock:image", "a cabin in fog");
    r.count = Some(3);
    r.params = Map::from_iter([("aspect_ratio".to_string(), json!("16:9"))]);
    let created = e.create_generation(LOCAL_USER, r).await.expect("queued");
    assert_eq!(created.jobs.len(), 1, "mock takes up to 4 outputs per request");
    let job = e.wait_job(LOCAL_USER, &created.jobs[0].id, Duration::from_secs(30)).await.unwrap();
    assert_eq!(job.status, JobStatus::Succeeded, "{:?}", job.error);
    assert_eq!(job.outputs.len(), 3);
    let first = &job.outputs[0];
    assert_eq!((first.width, first.height), (Some(768), Some(432)));
    let (path, mime, _) = e.asset_file(Some(LOCAL_USER), &first.id).await.unwrap();
    assert!(path.exists());
    assert_eq!(mime, "image/png");
}

#[tokio::test(flavor = "multi_thread")]
async fn compare_runs_every_model_in_one_batch_with_adapted_params() {
    let e = engine("compare", Mode::Local).await;
    let mut r = req("mock:image", "compare");
    r.compare = vec!["mock:video".into()];
    r.params = Map::from_iter([("aspect_ratio".to_string(), json!("21:9")), ("duration".to_string(), json!(5))]);
    let created = e.create_generation(LOCAL_USER, r).await.expect("queued");
    assert_eq!(created.jobs.len(), 2);
    assert!(created.jobs.iter().all(|j| j.batch_id == created.batch_id));
    let video = created.jobs.iter().find(|j| j.model == "mock:video").unwrap();
    assert_eq!(video.params.get("duration"), Some(&json!(4)), "5 s snaps to the nearest offered length");
    assert_eq!(video.params.get("aspect_ratio"), Some(&json!("16:9")), "an unsupported ratio falls back to the default");
}

#[tokio::test(flavor = "multi_thread")]
async fn bad_input_is_refused_with_the_field_named() {
    let e = engine("validation", Mode::Local).await;
    let mut r = req("mock:video", "x");
    r.params = Map::from_iter([("duration".to_string(), json!("forever"))]);
    match e.create_generation(LOCAL_USER, r).await {
        Err(pictesa_engine::EngineError::Validation { field, .. }) => assert_eq!(field.as_deref(), Some("params.duration")),
        other => panic!("expected a validation error, got {other:?}"),
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn hosted_platform_runs_are_charged_and_refunded_when_refused() {
    let e = engine("billing", Mode::Hosted).await;
    let user = e.signup("tester@example.com", "correct horse battery", None).await.unwrap();
    // No balance: refused before anything is queued.
    assert!(e.create_generation(&user.id, req("mock:image", "paid")).await.is_err());
    e.grant("tester@example.com", 1.0, "test").await.unwrap();
    assert!(e.grant("tester@example.com", 1.0, "test").await.is_err(), "grants are once per account and name");

    let ok = e.create_generation(&user.id, req("mock:image", "paid")).await.unwrap();
    let job = e.wait_job(&user.id, &ok.jobs[0].id, Duration::from_secs(30)).await.unwrap();
    assert_eq!(job.status, JobStatus::Succeeded);

    // The mock refuses this prompt at submit, so the charge comes back.
    let refused = e.create_generation(&user.id, req("mock:image", "fail please")).await.unwrap();
    let job = e.wait_job(&user.id, &refused.jobs[0].id, Duration::from_secs(30)).await.unwrap();
    assert_eq!(job.status, JobStatus::Failed);

    let wallet = e.billing_view(&user.id).await.unwrap();
    // $0.01 per image times the 1.25 markup was kept once; the refused run was refunded.
    assert!((wallet.balance_usd - 0.9875).abs() < 1e-9, "balance {}", wallet.balance_usd);
}

#[tokio::test(flavor = "multi_thread")]
async fn canceling_during_a_busy_retry_refunds() {
    let e = engine("busy", Mode::Hosted).await;
    let user = e.signup("busy@example.com", "correct horse battery", None).await.unwrap();
    e.grant("busy@example.com", 1.0, "test").await.unwrap();
    // The mock answers "rate limited" to this prompt, so the job waits between submit attempts.
    let created = e.create_generation(&user.id, req("mock:image", "busy please")).await.unwrap();
    let id = created.jobs[0].id.clone();
    for _ in 0..40 {
        if e.job(&user.id, &id).await.unwrap().message.as_deref().is_some_and(|m| m.contains("retrying")) {
            break;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    let job = e.cancel(&user.id, &id).await.unwrap();
    assert_eq!(job.status, JobStatus::Canceled);
    let wallet = e.billing_view(&user.id).await.unwrap();
    assert!((wallet.balance_usd - 1.0).abs() < 1e-9, "nothing ran, so nothing is charged: balance {}", wallet.balance_usd);
}

#[tokio::test(flavor = "multi_thread")]
async fn recipes_chain_steps_through_the_queue() {
    let e = engine("recipe", Mode::Local).await;
    let run = e
        .run_recipe(
            LOCAL_USER,
            "photo/new-angle",
            pictesa_engine::recipes::RunRecipe {
                values: Map::from_iter([("angle".to_string(), json!("a low angle"))]),
                inputs: [("photo".to_string(), vec![e.upload(LOCAL_USER, &png(), Some("in.png"), None).await.unwrap().id])].into(),
                models: [("angle".to_string(), "mock:image".to_string())].into(),
            },
        )
        .await
        .unwrap();
    let mut current = run;
    for _ in 0..60 {
        if current.status != "running" {
            break;
        }
        tokio::time::sleep(Duration::from_millis(500)).await;
        current = e.recipe_run(LOCAL_USER, &current.id).await.unwrap();
    }
    assert_eq!(current.status, "succeeded", "{:?}", current.error);
    let job = e.job(LOCAL_USER, &current.jobs[0]).await.unwrap();
    assert_eq!(job.inputs.len(), 1, "the photo is passed to the step");
}

#[tokio::test(flavor = "multi_thread")]
async fn hosted_workflow_import_is_for_the_administrator_only() {
    let e = engine("import", Mode::Hosted).await;
    let admin = e.signup("admin@example.com", "correct horse battery", None).await.unwrap();
    let user = e.signup("user@example.com", "correct horse battery", None).await.unwrap();
    let graph = json!({ "1": { "class_type": "SaveImage", "inputs": {} } });
    let refused = e.import_comfy_workflow(&user.id, "Mine", graph.clone()).await;
    assert!(matches!(refused, Err(pictesa_engine::EngineError::Forbidden(_))), "{refused:?}");
    // The administrator gets past the check (the graph itself may still be refused).
    let admin_try = e.import_comfy_workflow(&admin.id, "Mine", graph).await;
    assert!(!matches!(admin_try, Err(pictesa_engine::EngineError::Forbidden(_))), "{admin_try:?}");
}

/// A tiny valid PNG, generated so the test needs no fixture file.
fn png() -> Vec<u8> {
    let mut out = Vec::new();
    let img = image::RgbImage::from_pixel(8, 8, image::Rgb([242, 105, 61]));
    image::DynamicImage::ImageRgb8(img).write_to(&mut std::io::Cursor::new(&mut out), image::ImageFormat::Png).unwrap();
    out
}

#[tokio::test(flavor = "multi_thread")]
async fn batch_speed_halves_estimates_and_hosted_wallet_charges() {
    let e = engine("batch-speed", Mode::Hosted).await;
    let user = e.signup("batch@example.com", "correct horse battery", None).await.unwrap();
    e.grant("batch@example.com", 1.0, "test").await.unwrap();
    let standard = e.create_generation(&user.id, req("mock:image", "standard")).await.unwrap();
    let mut request = req("mock:image", "batch");
    request.params.insert("speed".into(), json!("batch"));
    let batch = e.create_generation(&user.id, request).await.unwrap();
    assert_eq!(standard.jobs[0].cost_usd, Some(0.0125));
    assert_eq!(batch.jobs[0].cost_usd, Some(0.00625));
    let job = e.wait_job(&user.id, &batch.jobs[0].id, Duration::from_secs(30)).await.unwrap();
    assert_eq!(job.status, JobStatus::Succeeded, "{:?}", job.error);
    assert_eq!(job.outputs.len(), 1);
    // BillingView rounds to four decimals; verify the actual stored micro-dollar amounts.
    let balance: i64 = sqlx::query_scalar("SELECT balance_micros FROM wallets WHERE user_id = ?")
        .bind(&user.id).fetch_one(e.db()).await.unwrap();
    let charge: i64 = sqlx::query_scalar("SELECT amount_micros FROM ledger WHERE job_id = ?")
        .bind(&job.id).fetch_one(e.db()).await.unwrap();
    assert_eq!(balance, 981_250, "standard $0.0125 plus batch $0.00625");
    assert_eq!(charge, -6250);
}

#[tokio::test(flavor = "multi_thread")]
async fn imports_deduplicate_fill_metadata_and_search_with_user_isolation() {
    use pictesa_engine::{AssetFilter, ImportMetadata};
    let e = engine("import-metadata", Mode::Hosted).await;
    let user = e.signup("import@example.com", "correct horse battery", None).await.unwrap();
    let first = e.upload(&user.id, &png(), Some("outside.png"), None).await.unwrap();
    let meta = ImportMetadata {
        prompt: Some("a lighthouse in moonlight".into()), negative_prompt: Some("fog".into()),
        origin: Some("gemini-api".into()), model: Some("mock:image".into()), model_label: Some("Outside model".into()),
        source_url: Some("https://example.com/source".into()),
        params: serde_json::from_value(json!({"seed":42,"nested":{"quality":"high"},"nullable":null,"quoted.\"key":true})).unwrap(),
    };
    let saved = e.upload_with_metadata(&user.id, &png(), Some("again.png"), None, &meta).await.unwrap();
    assert_eq!(saved.id, first.id);
    assert_eq!(saved.prompt, meta.prompt);
    assert_eq!(saved.negative_prompt, meta.negative_prompt);
    assert_eq!(saved.origin, meta.origin);
    assert_eq!(saved.model_label, meta.model_label);
    assert_eq!(saved.source_url, meta.source_url);
    assert_eq!(saved.params, meta.params);
    assert!(saved.job_id.is_none());
    let conflicting = ImportMetadata { prompt: Some("replacement".into()), params: serde_json::from_value(json!({"seed":9,"new":3})).unwrap(), ..Default::default() };
    let saved = e.upload_with_metadata(&user.id, &png(), None, None, &conflicting).await.unwrap();
    assert_eq!(saved.prompt, meta.prompt);
    assert_eq!(saved.params["seed"], 42);
    assert_eq!(saved.params["new"], 3);
    let (found, _) = e.assets(&user.id, &AssetFilter { q: Some("lighthouse".into()), ..Default::default() }).await.unwrap();
    assert_eq!(found.len(), 1);
    assert_eq!(found[0].model, meta.model);
    let (all, _) = e.assets(&user.id, &AssetFilter::default()).await.unwrap();
    assert_eq!(all.len(), 1);
    let other = e.signup("other-import@example.com", "correct horse battery", None).await.unwrap();
    assert!(e.fill_asset_metadata(&other.id, &first.id, &meta).await.is_err());
    let separate = e.upload(&other.id, &png(), None, None).await.unwrap();
    assert_ne!(separate.id, first.id);
    assert!(separate.prompt.is_none());
    let too_large = ImportMetadata { prompt: Some("x".repeat(20_001)), ..Default::default() };
    assert!(e.upload_with_metadata(&user.id, &png(), None, None, &too_large).await.is_err());
}

#[tokio::test(flavor = "multi_thread")]
async fn accepted_batch_resumes_after_restart_beyond_the_standard_runtime() {
    use pictesa_core::{Ctx, GenerationRequest, Provider, Submission};
    std::env::set_var("PICTESA_MOCK", "1");
    let config = Config::local(temp_dir("batch-restart"));
    let e = Engine::open(config.clone()).await.unwrap();
    let mut request = req("mock:image", "restart batch");
    request.params.insert("speed".into(), json!("batch"));
    let created = e.create_generation(LOCAL_USER, request).await.unwrap();
    let job = &created.jobs[0];
    let provider = pictesa_providers::mock::Mock;
    let context = Ctx { http: reqwest::Client::new(), creds: Default::default() };
    let spec = e.model(LOCAL_USER, "mock:image").await.unwrap();
    let submitted = provider.submit(&context, &spec, &GenerationRequest {
        job_id: job.id.clone(), model: "image".into(), prompt: job.prompt.clone(),
        negative_prompt: None, params: job.params.clone(), inputs: vec![], num_outputs: 1,
    }).await.unwrap();
    let Submission::Pending(handle) = submitted else { panic!("mock must queue"); };
    sqlx::query("UPDATE jobs SET status = 'running', remote = ?, attempts = 1, started_at = ?, updated_at = 0, owner = 'dead-process' WHERE id = ?")
        .bind(serde_json::to_string(&handle).unwrap())
        .bind(pictesa_core::now_ms() - 4 * 3600 * 1000).bind(&job.id).execute(e.db()).await.unwrap();
    let id = job.id.clone();
    drop(e);
    let restarted = Engine::open(config).await.unwrap();
    restarted.start();
    let result = restarted.wait_job(LOCAL_USER, &id, Duration::from_secs(30)).await.unwrap();
    assert_eq!(result.status, JobStatus::Succeeded, "{:?}", result.error);
    assert_eq!(result.attempts, 1, "accepted batch must be polled, never resubmitted");
    assert_eq!(result.outputs.len(), 1);
    assert_eq!(result.cost_usd, Some(0.005));
}
