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
async fn pins_belong_to_their_user_and_recent_models_follow_generations() {
    let e = engine("prefs", Mode::Local).await;
    let prefs = e.set_pinned_models(LOCAL_USER, vec!["mock:video".into(), "mock:image".into(), "mock:video".into()]).await.unwrap();
    assert_eq!(prefs.pinned, ["mock:video", "mock:image"], "pin order kept, repeats dropped");
    assert!(e.model_prefs("someone-else").await.unwrap().pinned.is_empty());
    assert!(e.set_pinned_models(LOCAL_USER, vec![String::new()]).await.is_err());
    for model in ["mock:video", "mock:image"] {
        e.create_generation(LOCAL_USER, req(model, "recent")).await.expect("queued");
        tokio::time::sleep(Duration::from_millis(5)).await;
    }
    assert_eq!(e.model_prefs(LOCAL_USER).await.unwrap().recent, ["mock:image", "mock:video"], "newest first");
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
