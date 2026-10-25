//! End-to-end engine flows on the free mock provider: the real queue, files, recipes and billing.
//! Each test uses its own temporary data folder. Environment variables are process-wide, so the
//! mock is enabled once and the tests share it.

use std::{collections::HashSet, time::Duration};

use pictesa_core::{InputRole, MediaKind};
use pictesa_engine::{
    characters::CharacterInput,
    recipes::{RecipeRun, RunRecipe},
    Config, CreateGeneration, Engine, EngineError, JobStatus, Mode, LOCAL_USER,
};
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
    std::env::set_var("PICTESA_DENY_PAID", "1");
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

async fn finish(e: &Engine, run: RecipeRun) -> RecipeRun {
    let mut current = run;
    for _ in 0..240 {
        if current.status != "running" {
            break;
        }
        tokio::time::sleep(Duration::from_millis(250)).await;
        current = e.recipe_run(LOCAL_USER, &current.id).await.unwrap();
    }
    current
}

fn run_request(values: serde_json::Value, inputs: &[(&str, &str)]) -> RunRecipe {
    let mut req = RunRecipe { values: values.as_object().cloned().unwrap_or_default(), ..Default::default() };
    for (key, asset) in inputs {
        req.inputs.entry(key.to_string()).or_default().push(asset.to_string());
    }
    req
}

#[tokio::test(flavor = "multi_thread")]
async fn shots_and_face_swap_run_end_to_end_on_the_mock() {
    let e = engine("apps", Mode::Local).await;
    let photo = e.upload(LOCAL_USER, &sized_png(64, 32), Some("scene.png"), None).await.unwrap().id;
    // Shots: nine different shots in one batch, returned together as the run's results.
    let run = finish(&e, e.run_recipe(LOCAL_USER, "photo/shots", run_request(json!({}), &[("photo", &photo)])).await.unwrap()).await;
    assert_eq!(run.status, "succeeded", "{:?}", run.error);
    assert_eq!((run.step_jobs.len(), run.results.len()), (1, 9));
    let (mut prompts, mut batches) = (HashSet::new(), HashSet::new());
    for id in &run.results {
        let job = e.job(LOCAL_USER, id).await.unwrap();
        assert_eq!((job.status, job.model.as_str()), (JobStatus::Succeeded, "mock:image"), "{:?}", job.error);
        assert_eq!(job.inputs[0].asset_id, photo);
        assert_eq!((job.outputs[0].width, job.outputs[0].height), (Some(768), Some(384)), "shots keep the photo's shape");
        prompts.insert(job.prompt);
        batches.insert(job.batch_id);
    }
    assert_eq!(prompts.len(), 9, "nine different shots");
    assert_eq!(batches.len(), 1, "the set is queued as one batch");

    // Face swap needs both photos and passes the target first, as its prompt says.
    let face = e.upload(LOCAL_USER, &sized_png(16, 16), Some("face.png"), None).await.unwrap().id;
    let missing = e.run_recipe(LOCAL_USER, "photo/face-swap", run_request(json!({}), &[("face", &face)])).await;
    assert!(matches!(missing, Err(EngineError::Validation { field: Some(ref f), .. }) if f == "inputs.target"), "{missing:?}");
    let run = finish(&e, e.run_recipe(LOCAL_USER, "photo/face-swap", run_request(json!({}), &[("face", &face), ("target", &photo)])).await.unwrap()).await;
    assert_eq!(run.status, "succeeded", "{:?}", run.error);
    let job = e.job(LOCAL_USER, &run.results[0]).await.unwrap();
    assert_eq!(job.inputs.iter().map(|i| i.asset_id.as_str()).collect::<Vec<_>>(), [photo.as_str(), face.as_str()]);
    assert_eq!((job.outputs[0].width, job.outputs[0].height), (Some(768), Some(384)), "the result keeps the target's shape");
}

#[tokio::test(flavor = "multi_thread")]
async fn brand_kits_and_paired_steps_flow_through_recipes() {
    let e = engine("apps-brand", Mode::Local).await;
    let logo = e.upload(LOCAL_USER, &sized_png(20, 20), Some("logo.png"), None).await.unwrap().id;
    let product = e.upload(LOCAL_USER, &sized_png(30, 40), Some("product.png"), None).await.unwrap().id;
    let kit = CharacterInput { name: Some("Oak and Co".into()), asset_ids: Some(vec![logo.clone()]), kind: Some("brand".into()), colors: Some(vec!["#0B6E4F".into(), "#f2c14e".into()]), words: Some("calm, natural".into()), ..Default::default() };
    let kit = e.create_character(LOCAL_USER, kit).await.unwrap();
    assert_eq!(kit.colors, ["#0b6e4f", "#f2c14e"]);
    let bad = CharacterInput { name: Some("Bad".into()), asset_ids: Some(vec![logo.clone()]), colors: Some(vec!["green".into()]), ..Default::default() };
    assert!(e.create_character(LOCAL_USER, bad).await.is_err(), "colors are hex codes");

    // Product placement: a described scene or a scene photo is needed; the brand kit joins the step.
    let refused = e.run_recipe(LOCAL_USER, "product/placement", run_request(json!({}), &[("product", &product)])).await;
    assert!(matches!(refused, Err(EngineError::Validation { field: Some(ref f), .. }) if f == "values.where"), "{refused:?}");
    let mut req = run_request(json!({ "where": "on a marble counter" }), &[("product", &product)]);
    req.character = Some(kit.id.clone());
    let run = finish(&e, e.run_recipe(LOCAL_USER, "product/placement", req).await.unwrap()).await;
    assert_eq!(run.status, "succeeded", "{:?}", run.error);
    let job = e.job(LOCAL_USER, &run.results[0]).await.unwrap();
    assert!(job.prompt.starts_with("Place the exact product from the first image into a new photo: on a marble counter."), "{}", job.prompt);
    assert!(job.prompt.ends_with("Brand look: calm, natural. Brand colors: #0b6e4f and #f2c14e. Keep the look of the brand reference images."), "{}", job.prompt);
    assert_eq!(job.inputs.iter().map(|i| i.asset_id.as_str()).collect::<Vec<_>>(), [product.as_str(), logo.as_str()], "brand images follow the person's files");

    // Explainer: every scene line becomes a still, and each clip starts from its own still.
    let mut req = run_request(json!({ "scenes": "1. A kettle on a stove\n2. Steam rising\n3. Tea poured into a cup", "duration": "5" }), &[]);
    req.character = Some(kit.id.clone());
    let run = finish(&e, e.run_recipe(LOCAL_USER, "video/explainer", req).await.unwrap()).await;
    assert_eq!(run.status, "succeeded", "{:?}", run.error);
    assert_eq!(run.step_jobs.iter().map(Vec::len).collect::<Vec<_>>(), [3, 3]);
    assert_eq!(run.results, run.step_jobs[1]);
    for (still, clip) in run.step_jobs[0].iter().zip(&run.step_jobs[1]) {
        let (still, clip) = (e.job(LOCAL_USER, still).await.unwrap(), e.job(LOCAL_USER, clip).await.unwrap());
        assert_eq!((clip.output, clip.status), (MediaKind::Video, JobStatus::Succeeded), "{:?}", clip.error);
        let start = clip.inputs.iter().find(|i| i.role == InputRole::StartFrame).map(|i| i.asset_id.clone());
        assert_eq!(start, Some(still.outputs[0].id.clone()), "clips pair with their own still");
        let scene = still.prompt.split(',').next().unwrap();
        assert!(clip.prompt.starts_with(scene), "{} / {}", still.prompt, clip.prompt);
        assert!(clip.prompt.contains("Brand look: calm, natural."), "a brand kit reaches models without image references");
    }
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

#[tokio::test(flavor = "multi_thread")]
async fn edit_canvas_flows_run_on_the_mock_and_record_the_source() {
    use pictesa_core::InputRole;
    use pictesa_engine::{edits::Padding, AssetFilter, JobInput};
    let e = engine("edit", Mode::Local).await;
    let source = e.upload(LOCAL_USER, &sized_png(64, 32), Some("photo.png"), None).await.unwrap();
    let run = |model: &str, prompt: &str, inputs: Vec<JobInput>| {
        let mut r = req(model, prompt);
        r.inputs = inputs;
        r
    };
    let input = |asset_id: &str, role| JobInput { asset_id: asset_id.into(), role };

    // Inpaint: a painted canvas export becomes a full-size mask that remembers its source.
    let mut stroke = image::RgbaImage::new(32, 16);
    for (x, y) in [(3, 3), (4, 3), (5, 3), (3, 4), (4, 4), (5, 4), (3, 5), (4, 5), (5, 5)] {
        stroke.put_pixel(x, y, image::Rgba([242, 105, 61, 255]));
    }
    let mut painted = Vec::new();
    image::DynamicImage::ImageRgba8(stroke).write_to(&mut std::io::Cursor::new(&mut painted), image::ImageFormat::Png).unwrap();
    let mask = e.save_mask(LOCAL_USER, &source.id, painted).await.unwrap();
    assert_eq!((mask.width, mask.height, mask.source.as_str()), (Some(64), Some(32), "edit"));
    assert_eq!(mask.made_from.as_deref(), Some(source.id.as_str()));
    let inpaint = run("mock:image", "a red door", vec![input(&source.id, InputRole::Reference), input(&mask.id, InputRole::Mask)]);
    let created = e.create_generation(LOCAL_USER, inpaint).await.expect("a mask is accepted");
    let job = e.wait_job(LOCAL_USER, &created.jobs[0].id, Duration::from_secs(90)).await.unwrap();
    assert_eq!(job.status, JobStatus::Succeeded, "{:?}", job.error);
    assert!(job.inputs.iter().any(|i| i.asset_id == source.id && i.role == InputRole::Reference), "the source is recorded");
    let out = &job.outputs[0];
    assert_eq!((out.width, out.height), (Some(768), Some(384)), "edits keep the source's shape");

    // Expand: the padded canvas and its mask are helpers of the source.
    let grown = e.expand_canvas(LOCAL_USER, &source.id, Padding { left: 16, right: 16, top: 0, bottom: 0 }).await.unwrap();
    assert_eq!((grown.image.width, grown.image.height), (Some(96), Some(32)));
    assert_eq!((grown.mask.width, grown.mask.height), (Some(96), Some(32)));
    let expand = run("mock:image", "", vec![input(&grown.image.id, InputRole::Reference), input(&grown.mask.id, InputRole::Mask)]);
    let job = e.create_generation(LOCAL_USER, expand).await.unwrap();
    let job = e.wait_job(LOCAL_USER, &job.jobs[0].id, Duration::from_secs(90)).await.unwrap();
    assert_eq!(job.status, JobStatus::Succeeded, "{:?}", job.error);
    assert_eq!((job.outputs[0].width, job.outputs[0].height), (Some(768), Some(256)));

    // Remove background: the result keeps transparency.
    let cut = e.create_generation(LOCAL_USER, run("mock:remove-background", "", vec![input(&source.id, InputRole::Reference)])).await.unwrap();
    let job = e.wait_job(LOCAL_USER, &cut.jobs[0].id, Duration::from_secs(90)).await.unwrap();
    assert_eq!(job.status, JobStatus::Succeeded, "{:?}", job.error);
    let (path, mime, _) = e.asset_file(Some(LOCAL_USER), &job.outputs[0].id).await.unwrap();
    assert_eq!(mime, "image/png");
    let cutout = image::open(&path).unwrap().to_rgba8();
    assert_eq!(cutout.get_pixel(0, 0).0[3], 0, "the background is transparent");
    assert_eq!(cutout.get_pixel(cutout.width() / 2, cutout.height() / 2).0[3], 255, "the subject is opaque");
    // Its thumbnail shows the cut-out on a checkerboard, not on black.
    let thumb = image::open(e.asset_thumb(Some(LOCAL_USER), &job.outputs[0].id).await.unwrap()).unwrap().to_rgb8();
    assert!(thumb.get_pixel(0, 0).0.iter().all(|c| *c > 200));

    // Helpers stay out of the library; the results and the source are there.
    let (listed, _) = e.assets(LOCAL_USER, &AssetFilter::default()).await.unwrap();
    assert!(listed.iter().all(|a| a.source != "edit"));
    assert!(listed.iter().any(|a| a.id == source.id));
    let helpers = AssetFilter { source: Some("edit".into()), ..Default::default() };
    assert_eq!(e.assets(LOCAL_USER, &helpers).await.unwrap().0.len(), 3);

    // Another user cannot paint on this image.
    assert!(e.save_mask("someone-else", &source.id, sized_png(4, 4)).await.is_err());
}

fn sized_png(w: u32, h: u32) -> Vec<u8> {
    let mut out = Vec::new();
    let img = image::RgbImage::from_pixel(w, h, image::Rgb([90, 120, 160]));
    image::DynamicImage::ImageRgb8(img).write_to(&mut std::io::Cursor::new(&mut out), image::ImageFormat::Png).unwrap();
    out
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
}
