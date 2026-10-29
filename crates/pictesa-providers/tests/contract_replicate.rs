//! Replicate contract: Replicate's documented answers replayed through the real adapter, checking
//! what it sends. api.replicate.com resolves to the local stand-in; no key or network is used.
//! Reference: https://replicate.com/docs/reference/http

mod common;

use std::sync::{
    atomic::{AtomicUsize, Ordering},
    Arc,
};

use common::{start, Fake, Reply, PNG_1X1};
use pictesa_core::{Ctx, GenerationRequest, InputRole, MediaInput, MediaKind, ModelSpec, Output, ParamKind, PollStatus, Provider, ProviderError, RemoteHandle, Submission, Task};
use pictesa_providers::replicate::Replicate;
use serde_json::{json, Value};

fn ctx(fake: &Fake) -> Ctx {
    fake.ctx(&["api.replicate.com", "replicate.delivery"], &[("api_key", "test-key")])
}

fn request(prompt: &str, params: Value, inputs: Vec<MediaInput>, num_outputs: u32) -> GenerationRequest {
    GenerationRequest {
        job_id: "job-1".into(),
        model: "m".into(),
        prompt: prompt.into(),
        negative_prompt: None,
        params: params.as_object().cloned().unwrap_or_default(),
        inputs,
        num_outputs,
    }
}

/// An input file in its own temporary directory, removed on drop.
struct TempInput(MediaInput);

impl TempInput {
    fn new(role: InputRole) -> Self {
        let dir = std::env::temp_dir().join(format!("pictesa-contract-replicate-{}", ulid::Ulid::new()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("frame.png");
        std::fs::write(&path, PNG_1X1).unwrap();
        Self(MediaInput { role, kind: MediaKind::Image, mime: "image/png".into(), path, url: None, width: Some(1), height: Some(1) })
    }
}

impl Drop for TempInput {
    fn drop(&mut self) {
        if let Some(dir) = self.0.path.parent() {
            let _ = std::fs::remove_dir_all(dir);
        }
    }
}

fn model(id: &str, output: MediaKind, tasks: &[Task]) -> ModelSpec {
    let mut m = ModelSpec::new("replicate", id, id, output, tasks);
    m.complete = false;
    m
}

/// A prediction object, https://replicate.com/docs/reference/http#predictions.get.
fn prediction(id: &str, model: &str, status: &str, output: Value, error: Value) -> Value {
    json!({
        "id": id, "model": model, "version": "d9fbf23e0a34bb6134f86d3e24f48c0dd96332378da307b5e9b60a9d47e4a9c2",
        "input": { "prompt": "a lion" }, "logs": "", "output": output, "error": error, "status": status,
        "created_at": "2023-09-08T16:19:34.765994Z", "source": "api", "data_removed": false,
        "urls": {
            "web": format!("https://replicate.com/p/{id}"),
            "get": format!("https://api.replicate.com/v1/predictions/{id}"),
            "cancel": format!("https://api.replicate.com/v1/predictions/{id}/cancel")
        }
    })
}

/// `models.get` for black-forest-labs/flux-schnell. The Input properties are the ones printed at
/// https://replicate.com/docs/reference/openapi#model-schemas (enum fields as `allOf` + `$ref`);
/// the referenced enums and the Output come from https://replicate.com/black-forest-labs/flux-schnell/api/schema.
fn flux_schnell() -> Value {
    json!({
        "url": "https://replicate.com/black-forest-labs/flux-schnell", "owner": "black-forest-labs", "name": "flux-schnell",
        "description": "The fastest image generation model tailored for local development and personal use",
        "visibility": "public", "run_count": 500000000,
        "latest_version": {
            "id": "d9fbf23e0a34bb6134f86d3e24f48c0dd96332378da307b5e9b60a9d47e4a9c2",
            "created_at": "2025-01-01T00:00:00Z", "cog_version": "0.14.0",
            "openapi_schema": { "components": { "schemas": {
                "Input": { "type": "object", "title": "Input", "required": ["prompt"], "properties": {
                    "seed": { "type": "integer", "title": "Seed", "x-order": 3, "description": "Random seed. Set for reproducible generation" },
                    "prompt": { "type": "string", "title": "Prompt", "x-order": 0, "description": "Prompt for generated image" },
                    "go_fast": { "type": "boolean", "title": "Go Fast", "default": true, "x-order": 7, "description": "Run faster predictions with model optimized for speed (currently fp8 quantized); disable to run in original bf16" },
                    "megapixels": { "allOf": [{ "$ref": "#/components/schemas/megapixels" }], "default": "1", "x-order": 8, "description": "Approximate number of megapixels for generated image" },
                    "num_outputs": { "type": "integer", "title": "Num Outputs", "default": 1, "maximum": 4, "minimum": 1, "x-order": 2, "description": "Number of outputs to generate" },
                    "aspect_ratio": { "allOf": [{ "$ref": "#/components/schemas/aspect_ratio" }], "default": "1:1", "x-order": 1, "description": "Aspect ratio for the generated image" },
                    "output_format": { "allOf": [{ "$ref": "#/components/schemas/output_format" }], "default": "webp", "x-order": 4, "description": "Format of the output images" },
                    "output_quality": { "type": "integer", "title": "Output Quality", "default": 80, "maximum": 100, "minimum": 0, "x-order": 5, "description": "Quality when saving the output images, from 0 to 100. 100 is best quality, 0 is lowest quality. Not relevant for .png outputs" },
                    "disable_safety_checker": { "type": "boolean", "title": "Disable Safety Checker", "default": false, "x-order": 6, "description": "Disable safety checker for generated images." }
                } },
                "Output": { "type": "array", "items": { "type": "string", "format": "uri" }, "title": "Output" },
                "aspect_ratio": { "enum": ["1:1", "16:9", "21:9", "3:2", "2:3", "4:5", "5:4", "3:4", "4:3", "9:16", "9:21"], "type": "string", "title": "aspect_ratio", "description": "An enumeration." },
                "output_format": { "enum": ["webp", "jpg", "png"], "type": "string", "title": "output_format", "description": "An enumeration." },
                "megapixels": { "enum": ["1", "0.25"], "type": "string", "title": "megapixels", "description": "An enumeration." }
            } } }
        }
    })
}

#[tokio::test]
async fn check_reads_the_account_with_a_bearer_token() {
    // https://replicate.com/docs/reference/http#account.get
    let fake = start(|_| Reply::json(json!({ "type": "organization", "username": "acme", "name": "Acme Corp, Inc.", "github_url": "https://github.com/acme" }))).await;
    Replicate.check(&ctx(&fake)).await.unwrap();
    let call = &fake.calls()[0];
    assert_eq!((call.method.as_str(), call.path.as_str()), ("GET", "/v1/account"));
    assert_eq!(call.header("authorization"), Some("Bearer test-key"));
}

#[tokio::test]
async fn discover_lists_the_generation_collections() {
    // Collection object, https://replicate.com/docs/reference/http#collections.get, holding model
    // objects shaped like https://replicate.com/docs/reference/http#models.get.
    let fake = start(|call| {
        let slug = call.path.rsplit('/').next().unwrap_or("");
        let models = match slug {
            "text-to-image" => json!([{ "url": "https://replicate.com/black-forest-labs/flux-schnell", "owner": "black-forest-labs", "name": "flux-schnell", "description": "Fast image model", "visibility": "public", "run_count": 1 }]),
            "text-to-video" | "image-to-video" => json!([{ "url": "https://replicate.com/google/veo-3.1-fast", "owner": "google", "name": "veo-3.1-fast", "description": "Video model", "visibility": "public", "run_count": 1 }]),
            _ => json!([]),
        };
        Reply::json(json!({ "name": slug, "slug": slug, "description": "", "models": models }))
    })
    .await;
    let specs = Replicate.discover(&ctx(&fake)).await.unwrap();
    let paths: Vec<String> = fake.calls().into_iter().map(|c| c.path).collect();
    assert_eq!(paths, ["/v1/collections/text-to-image", "/v1/collections/text-to-video", "/v1/collections/image-to-video"]);
    let ids: Vec<(&str, MediaKind)> = specs.iter().map(|s| (s.id.as_str(), s.output)).collect();
    assert_eq!(ids, [("replicate:black-forest-labs/flux-schnell", MediaKind::Image), ("replicate:google/veo-3.1-fast", MediaKind::Video)]);
}

/// Image model end to end: the version schema, a versioned prediction, every state, the outputs.
#[tokio::test]
async fn image_model_runs_as_a_versioned_prediction() {
    let polls = Arc::new(AtomicUsize::new(0));
    let seen = polls.clone();
    let fake = start(move |call| match (call.method.as_str(), call.path.as_str()) {
        ("GET", "/v1/models/black-forest-labs/flux-schnell") => Reply::json(flux_schnell()),
        // Async create answers with a `starting` prediction,
        // https://replicate.com/docs/topics/predictions/create-a-prediction#async-mode-default.
        ("POST", "/v1/predictions") => Reply::status(201, prediction("gm3qorzdhgbfurvjtvhg6dckhu", "black-forest-labs/flux-schnell", "starting", Value::Null, Value::Null)),
        ("GET", "/v1/predictions/gm3qorzdhgbfurvjtvhg6dckhu") => {
            let (status, output) = match seen.fetch_add(1, Ordering::SeqCst) {
                0 => ("starting", Value::Null),
                1 => ("processing", Value::Null),
                // Example output, https://replicate.com/black-forest-labs/flux-schnell/llms.txt.
                _ => ("succeeded", json!(["https://replicate.delivery/yhqm/hcDDSNf633zeDUz9sWkKfaftcfJLWIvuhn9vfCFWmufxelmemA/out-0.webp"])),
            };
            Reply::json(prediction("gm3qorzdhgbfurvjtvhg6dckhu", "black-forest-labs/flux-schnell", status, output, Value::Null))
        }
        _ => Reply::status(404, json!({ "detail": "Not found." })),
    })
    .await;
    let ctx = ctx(&fake);

    let spec = Replicate.resolve(&ctx, &model("black-forest-labs/flux-schnell", MediaKind::Image, &[Task::TextToImage])).await.unwrap();
    assert_eq!(spec.max_outputs, 4, "num_outputs maximum");
    // Enum fields published through `allOf` + `$ref` stay choosable.
    let aspect = spec.params.iter().find(|p| p.key == "aspect_ratio").expect("aspect_ratio offered");
    assert!(matches!(&aspect.kind, ParamKind::Enum { options } if options.len() == 11 && options[1].value == "16:9"));
    assert_eq!(aspect.default, Some(json!("1:1")));
    assert!(spec.params.iter().any(|p| p.key == "output_format"));

    let req = request("a lion", json!({ "aspect_ratio": "16:9", "seed": 7 }), vec![], 2);
    let Submission::Pending(handle) = Replicate.submit(&ctx, &spec, &req).await.unwrap() else { panic!("predictions are polled") };
    let create = fake.calls().into_iter().find(|c| c.method == "POST").unwrap();
    assert_eq!(create.header("authorization"), Some("Bearer test-key"));
    assert_eq!(
        create.json(),
        json!({
            // `{owner}/{name}:{version_id}`, https://replicate.com/docs/reference/http#predictions.create-request-body-version
            "version": "black-forest-labs/flux-schnell:d9fbf23e0a34bb6134f86d3e24f48c0dd96332378da307b5e9b60a9d47e4a9c2",
            "input": { "prompt": "a lion", "aspect_ratio": "16:9", "seed": 7, "num_outputs": 2 }
        })
    );

    assert!(matches!(Replicate.poll(&ctx, &handle).await.unwrap(), PollStatus::Queued { .. }));
    assert!(matches!(Replicate.poll(&ctx, &handle).await.unwrap(), PollStatus::Running { .. }));
    let PollStatus::Done(out) = Replicate.poll(&ctx, &handle).await.unwrap() else { panic!("succeeded") };
    // Files on replicate.delivery are public URLs, so the key is not attached to the download.
    assert!(matches!(&out[..], [Output::Url { url, headers }] if url.ends_with("/out-0.webp") && headers.is_empty()));
}

/// Video model with a start frame. Input schema of google/veo-3.1-fast from
/// https://replicate.com/google/veo-3.1-fast/api/schema; files go as data URLs
/// (https://replicate.com/docs/reference/http#predictions.create-request-body-input).
#[tokio::test]
async fn image_to_video_sends_the_frame_as_a_data_url() {
    let fake = start(|call| match (call.method.as_str(), call.path.as_str()) {
        ("GET", "/v1/models/google/veo-3.1-fast") => Reply::json(json!({
            "url": "https://replicate.com/google/veo-3.1-fast", "owner": "google", "name": "veo-3.1-fast", "visibility": "public", "run_count": 1,
            "latest_version": { "id": "34f441e461c62a424ca14087798d91bcb2fb071b631f3365da6465e2ab083615", "openapi_schema": { "components": { "schemas": {
                "Input": { "type": "object", "title": "Input", "required": ["prompt"], "properties": {
                    "seed": { "type": "integer", "title": "Seed", "x-order": 8, "nullable": true, "description": "Random seed. Omit for random generations" },
                    "image": { "type": "string", "title": "Image", "format": "uri", "x-order": 3, "nullable": true, "description": "Input image to start generating from. Ideal images are 16:9 or 9:16 and 1280x720 or 720x1280, depending on the aspect ratio you choose." },
                    "prompt": { "type": "string", "title": "Prompt", "x-order": 0, "description": "Text prompt for video generation" },
                    "duration": { "enum": [4, 6, 8], "type": "integer", "title": "duration", "description": "Video duration in seconds", "default": 8, "x-order": 2 },
                    "last_frame": { "type": "string", "title": "Last Frame", "format": "uri", "x-order": 4, "nullable": true, "description": "Ending image for interpolation. When provided with an input image, creates a transition between the two images." },
                    "resolution": { "enum": ["720p", "1080p"], "type": "string", "title": "resolution", "description": "Resolution of the generated video", "default": "1080p", "x-order": 6 },
                    "aspect_ratio": { "enum": ["16:9", "9:16"], "type": "string", "title": "aspect_ratio", "description": "Video aspect ratio", "default": "16:9", "x-order": 1 },
                    "generate_audio": { "type": "boolean", "title": "Generate Audio", "default": true, "x-order": 7, "description": "Generate audio with the video" },
                    "negative_prompt": { "type": "string", "title": "Negative Prompt", "x-order": 5, "nullable": true, "description": "Description of what to exclude from the generated video" }
                } },
                "Output": { "type": "string", "title": "Output", "format": "uri" }
            } } } }
        })),
        ("POST", "/v1/predictions") => Reply::status(201, prediction("btfhsk0brdrme0cswz1svpa1t0", "google/veo-3.1-fast", "starting", Value::Null, Value::Null)),
        // Example output, https://replicate.com/google/veo-3.1-fast/llms.txt.
        _ => Reply::json(prediction("btfhsk0brdrme0cswz1svpa1t0", "google/veo-3.1-fast", "succeeded",
            json!("https://replicate.delivery/xezq/aOYeXeCcnHhDiELUfr7GJR0IkfcGabYRUWiTShhvdJCTEQ9VB/tmpafbwmta5.mp4"), Value::Null)),
    })
    .await;
    let ctx = ctx(&fake);
    let spec = Replicate.resolve(&ctx, &model("google/veo-3.1-fast", MediaKind::Video, &[Task::TextToVideo, Task::ImageToVideo])).await.unwrap();
    assert!(spec.inputs.iter().any(|s| s.role == InputRole::StartFrame));
    assert!(spec.inputs.iter().any(|s| s.role == InputRole::EndFrame));

    let frame = TempInput::new(InputRole::StartFrame);
    let req = request("the seed spins", json!({ "duration": 8, "resolution": "720p" }), vec![frame.0.clone()], 1);
    let Submission::Pending(handle) = Replicate.submit(&ctx, &spec, &req).await.unwrap() else { panic!("predictions are polled") };
    let input = fake.calls().into_iter().find(|c| c.method == "POST").unwrap().json()["input"].clone();
    assert_eq!(input["prompt"], "the seed spins");
    assert_eq!(input["duration"], 8);
    assert!(input["image"].as_str().unwrap().starts_with("data:image/png;base64,"));
    assert!(input.get("last_frame").is_none());

    let PollStatus::Done(out) = Replicate.poll(&ctx, &handle).await.unwrap() else { panic!("succeeded") };
    assert!(matches!(&out[..], [Output::Url { url, .. }] if url.ends_with("tmpafbwmta5.mp4")));
}

/// `failed` carries `error`; `canceled` is terminal too
/// (https://replicate.com/docs/reference/http#predictions.get, `status` values).
#[tokio::test]
async fn failed_and_canceled_predictions_end_the_job() {
    let fake = start(|call| {
        let id = call.path.rsplit('/').next().unwrap_or("");
        Reply::json(match id {
            "pfailed" => prediction(id, "black-forest-labs/flux-schnell", "failed", Value::Null, json!("CUDA out of memory")),
            _ => prediction(id, "black-forest-labs/flux-schnell", "canceled", Value::Null, Value::Null),
        })
    })
    .await;
    let ctx = ctx(&fake);
    let failed = Replicate.poll(&ctx, &RemoteHandle::with("pfailed", json!({ "output": "image" }))).await.unwrap();
    assert!(matches!(failed, PollStatus::Failed(ProviderError::Failed(m)) if m == "CUDA out of memory"));
    let canceled = Replicate.poll(&ctx, &RemoteHandle::with("pcanceled", json!({ "output": "image" }))).await.unwrap();
    assert!(matches!(canceled, PollStatus::Failed(_)));
}

/// https://replicate.com/docs/reference/http#predictions.cancel
#[tokio::test]
async fn cancel_posts_to_the_cancel_route() {
    let fake = start(|call| Reply::json(prediction(call.path.split('/').nth(3).unwrap_or(""), "google/veo-3.1-fast", "canceled", Value::Null, Value::Null))).await;
    Replicate.cancel(&ctx(&fake), &RemoteHandle::new("gm3qorzdhgbfurvjtvhg6dckhu")).await.unwrap();
    let call = &fake.calls()[0];
    assert_eq!((call.method.as_str(), call.path.as_str()), ("POST", "/v1/predictions/gm3qorzdhgbfurvjtvhg6dckhu/cancel"));
    assert_eq!(call.header("authorization"), Some("Bearer test-key"));
}

/// The reference documents the 429 body (https://replicate.com/docs/reference/http#rate-limits);
/// 401 and 5xx carry the same `detail` shape.
#[tokio::test]
async fn create_errors_map_to_their_kinds() {
    let fake = start(|call| match call.json()["input"]["prompt"].as_str().unwrap_or("") {
        _ if call.method == "GET" => Reply::json(flux_schnell()),
        "401" => Reply::status(401, json!({ "detail": "Invalid token." })),
        "429" => Reply::status(429, json!({ "detail": "Request was throttled. Expected available in 1 second." })),
        _ => Reply::status(500, json!({ "detail": "Internal server error" })),
    })
    .await;
    let ctx = ctx(&fake);
    // The model is resolved by submit itself (its schema is fetched first).
    let spec = model("black-forest-labs/flux-schnell", MediaKind::Image, &[Task::TextToImage]);
    for (case, kind) in [("401", "auth"), ("429", "rate_limited"), ("500", "transient")] {
        let err = Replicate.submit(&ctx, &spec, &request(case, json!({}), vec![], 1)).await.unwrap_err();
        assert_eq!(err.kind(), kind, "HTTP {case}: {err:?}");
    }
}
