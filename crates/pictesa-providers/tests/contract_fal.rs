//! fal contract: fal's documented answers replayed through the real adapter, checking what it
//! sends. Every fal host resolves to the local stand-in; no key or network is used.

mod common;

use std::sync::{
    atomic::{AtomicUsize, Ordering},
    Arc,
};

use common::{start, Fake, Reply, PNG_1X1};
use pictesa_core::{
    Ctx, GenerationRequest, InputRole, MediaInput, MediaKind, ModelSpec, Output, ParamKind, PollStatus, Pricing, Provider, ProviderError, RemoteHandle, Submission, Task,
};
use pictesa_providers::fal::Fal;
use serde_json::{json, Value};

/// Every host the adapter may call, including the legacy upload host, so a wrong host still lands
/// on the stand-in (and fails the test) instead of reaching fal.
const HOSTS: &[&str] = &["queue.fal.run", "api.fal.ai", "fal.ai", "rest.fal.ai", "rest.alpha.fal.ai", "v3b.fal.media"];

fn ctx(fake: &Fake) -> Ctx {
    fake.ctx(HOSTS, &[("api_key", "test-key")])
}

fn request(model: &str, prompt: &str, params: Value, inputs: Vec<MediaInput>, num_outputs: u32) -> GenerationRequest {
    GenerationRequest {
        job_id: "job-1".into(),
        model: model.into(),
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
    fn new(role: InputRole, bytes: &[u8]) -> Self {
        let dir = std::env::temp_dir().join(format!("pictesa-contract-fal-{}", ulid::Ulid::new()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("frame.png");
        std::fs::write(&path, bytes).unwrap();
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

fn model(id: &str, output: MediaKind, task: Task) -> ModelSpec {
    let mut m = ModelSpec::new("fal", id, id, output, &[task]);
    m.complete = false;
    m
}

fn path_only(path: &str) -> &str {
    path.split('?').next().unwrap_or(path)
}

fn query_pairs(path: &str) -> Vec<(String, String)> {
    url::Url::parse(&format!("https://x{path}")).unwrap().query_pairs().map(|(k, v)| (k.into_owned(), v.into_owned())).collect()
}

// Pricing answer, https://fal.ai/docs/platform-apis/v1/models/pricing (200 example).
fn flux_dev_price() -> Value {
    json!({ "prices": [{ "endpoint_id": "fal-ai/flux/dev", "unit_price": 0.025, "unit": "image", "currency": "USD" }], "next_cursor": null, "has_more": false })
}

#[tokio::test]
async fn check_sends_the_key_with_the_key_prefix_to_the_pricing_api() {
    let fake = start(|_| Reply::json(flux_dev_price())).await;
    Fal.check(&ctx(&fake)).await.unwrap();
    // Auth format: https://fal.ai/docs/api-reference/platform-apis/authentication ("Authorization: Key YOUR_API_KEY").
    let call = &fake.calls()[0];
    assert_eq!((call.method.as_str(), call.host.as_str(), path_only(&call.path)), ("GET", "api.fal.ai", "/v1/models/pricing"));
    assert_eq!(query_pairs(&call.path), vec![("endpoint_id".to_string(), "fal-ai/flux/dev".to_string())]);
    assert_eq!(call.header("authorization"), Some("Key test-key"));
}

#[tokio::test]
async fn discover_follows_the_cursor_and_prices_the_catalog() {
    let fake = start(|call| {
        if path_only(&call.path) == "/v1/models/pricing" {
            return Reply::json(flux_dev_price());
        }
        // Model search, https://fal.ai/docs/platform-apis/v1/models (200 example; the second page
        // repeats its shape for an image-to-video endpoint).
        if call.path.contains("cursor=") {
            Reply::json(json!({ "models": [{ "endpoint_id": "fal-ai/kling-video/v2.1/standard/image-to-video", "metadata": {
                "display_name": "Kling 2.1 Standard", "category": "image-to-video", "description": "Image to video", "status": "active",
                "tags": [], "updated_at": "2025-01-15T12:00:00Z", "is_favorited": false, "thumbnail_url": "https://fal.media/files/example.jpg",
                "model_url": "https://fal.run/fal-ai/kling-video/v2.1/standard/image-to-video", "date": "2025-06-01T00:00:00Z", "highlighted": false, "pinned": false } }],
                "next_cursor": null, "has_more": false }))
        } else {
            Reply::json(json!({ "models": [{ "endpoint_id": "fal-ai/flux/dev", "metadata": {
                "display_name": "FLUX.1 [dev]", "category": "text-to-image", "description": "Fast text-to-image generation", "status": "active",
                "tags": ["fast", "pro"], "updated_at": "2025-01-15T12:00:00Z", "is_favorited": false, "thumbnail_url": "https://fal.media/files/example.jpg",
                "model_url": "https://fal.run/fal-ai/flux/dev", "date": "2024-08-01T00:00:00Z", "highlighted": false, "pinned": false } }],
                "next_cursor": "Mg==", "has_more": true }))
        }
    })
    .await;
    let specs = Fal.discover(&ctx(&fake)).await.unwrap();

    let calls = fake.calls();
    assert_eq!(path_only(&calls[0].path), "/v1/models");
    assert!(query_pairs(&calls[1].path).contains(&("cursor".into(), "Mg==".into())), "second page asks for next_cursor: {}", calls[1].path);
    let flux = specs.iter().find(|s| s.id == "fal:fal-ai/flux/dev").unwrap();
    assert_eq!((flux.output, flux.tasks.clone()), (MediaKind::Image, vec![Task::TextToImage]));
    assert!(matches!(flux.pricing, Some(Pricing::PerImage { usd }) if usd == 0.025));
    let kling = specs.iter().find(|s| s.id == "fal:fal-ai/kling-video/v2.1/standard/image-to-video").unwrap();
    assert_eq!((kling.output, kling.tasks.clone()), (MediaKind::Video, vec![Task::ImageToVideo]));
    // Pricing takes repeated endpoint_id parameters (1-50 per call).
    let pricing = calls.iter().find(|c| path_only(&c.path) == "/v1/models/pricing").unwrap();
    let mut asked: Vec<String> = query_pairs(&pricing.path).into_iter().filter(|(k, _)| k == "endpoint_id").map(|(_, v)| v).collect();
    asked.sort();
    assert_eq!(asked, ["fal-ai/flux/dev", "fal-ai/kling-video/v2.1/standard/image-to-video"]);
}

/// An image model end to end: schema, submit, every queue state, then the result.
#[tokio::test]
async fn image_model_runs_through_the_queue() {
    let polls = Arc::new(AtomicUsize::new(0));
    let seen = polls.clone();
    let fake = start(move |call| match (call.host.as_str(), path_only(&call.path)) {
        // Published queue OpenAPI, https://fal.ai/api/openapi/queue/openapi.json?endpoint_id=fal-ai/flux/dev
        // (documented at https://fal.ai/docs/documentation/development/document-your-model).
        ("fal.ai", "/api/openapi/queue/openapi.json") => Reply::bytes("application/json", include_bytes!("fixtures/contract/fal/openapi-fal-ai__flux__dev.json")),
        ("api.fal.ai", _) => Reply::json(flux_dev_price()),
        // Submit and status answers, https://fal.ai/docs/documentation/model-apis/inference/queue.
        ("queue.fal.run", "/fal-ai/flux/dev") => Reply::json(json!({
            "request_id": "764cabcf-b745-4b3e-ae38-1200304cf45b",
            "response_url": "https://queue.fal.run/fal-ai/flux/requests/764cabcf-b745-4b3e-ae38-1200304cf45b",
            "status_url": "https://queue.fal.run/fal-ai/flux/requests/764cabcf-b745-4b3e-ae38-1200304cf45b/status",
            "cancel_url": "https://queue.fal.run/fal-ai/flux/requests/764cabcf-b745-4b3e-ae38-1200304cf45b/cancel",
            "queue_position": 0
        })),
        ("queue.fal.run", "/fal-ai/flux/requests/764cabcf-b745-4b3e-ae38-1200304cf45b/status") => match seen.fetch_add(1, Ordering::SeqCst) {
            0 => Reply::status(202, json!({ "status": "IN_QUEUE", "request_id": "764cabcf-b745-4b3e-ae38-1200304cf45b", "queue_position": 2,
                "response_url": "https://queue.fal.run/fal-ai/flux/requests/764cabcf-b745-4b3e-ae38-1200304cf45b" })),
            1 => Reply::status(202, json!({ "status": "IN_PROGRESS", "request_id": "764cabcf-b745-4b3e-ae38-1200304cf45b",
                "response_url": "https://queue.fal.run/fal-ai/flux/requests/764cabcf-b745-4b3e-ae38-1200304cf45b",
                "logs": [{ "message": "Loading model weights...", "timestamp": "2026-02-17T10:30:01.123Z" }, { "message": "Generating image...", "timestamp": "2026-02-17T10:30:02.456Z" }] })),
            _ => Reply::json(json!({ "status": "COMPLETED", "request_id": "764cabcf-b745-4b3e-ae38-1200304cf45b",
                "response_url": "https://queue.fal.run/fal-ai/flux/requests/764cabcf-b745-4b3e-ae38-1200304cf45b",
                "logs": [{ "message": "Done.", "timestamp": "2026-02-17T10:30:05.789Z" }], "metrics": { "inference_time": 3.42 } })),
        },
        ("queue.fal.run", "/fal-ai/flux/requests/764cabcf-b745-4b3e-ae38-1200304cf45b") => Reply::json(json!({
            "images": [{ "url": "https://v3.fal.media/files/rabbit/abc123.png", "width": 1024, "height": 1024, "content_type": "image/png" }],
            "prompt": "a sunset over mountains", "seed": 42, "has_nsfw_concepts": [false]
        })),
        _ => Reply::status(404, json!({ "detail": "unexpected call" })),
    })
    .await;
    let ctx = ctx(&fake);

    let spec = Fal.resolve(&ctx, &model("fal-ai/flux/dev", MediaKind::Image, Task::TextToImage)).await.unwrap();
    assert!(spec.prompt && spec.complete);
    assert_eq!(spec.max_outputs, 4, "num_images maximum");
    // `image_size` is `anyOf` [ImageSize object, preset enum]; the presets stay choosable.
    let size = spec.params.iter().find(|p| p.key == "image_size").expect("image_size offered");
    assert!(matches!(&size.kind, ParamKind::Enum { options } if options.iter().any(|o| o.value == "landscape_16_9")));
    assert_eq!(size.default, Some(json!("landscape_4_3")));

    let req = request("fal-ai/flux/dev", "a sunset over mountains", json!({ "seed": 42, "image_size": "landscape_16_9" }), vec![], 2);
    let Submission::Pending(handle) = Fal.submit(&ctx, &spec, &req).await.unwrap() else { panic!("fal queues") };
    let submit = fake.calls().into_iter().find(|c| c.method == "POST").unwrap();
    assert_eq!((submit.host.as_str(), submit.path.as_str()), ("queue.fal.run", "/fal-ai/flux/dev"));
    assert_eq!(submit.header("authorization"), Some("Key test-key"));
    assert_eq!(submit.json(), json!({ "prompt": "a sunset over mountains", "num_images": 2, "seed": 42, "image_size": "landscape_16_9" }));

    assert!(matches!(Fal.poll(&ctx, &handle).await.unwrap(), PollStatus::Queued { position: Some(2) }));
    assert!(matches!(Fal.poll(&ctx, &handle).await.unwrap(), PollStatus::Running { message: Some(m), .. } if m == "Generating image..."));
    let PollStatus::Done(out) = Fal.poll(&ctx, &handle).await.unwrap() else { panic!("completed") };
    assert!(matches!(&out[..], [Output::Url { url, .. }] if url == "https://v3.fal.media/files/rabbit/abc123.png"));

    let status_calls: Vec<_> = fake.calls().into_iter().filter(|c| c.path.contains("/status")).collect();
    assert!(status_calls.iter().all(|c| c.method == "GET" && c.header("authorization") == Some("Key test-key")));
    assert!(query_pairs(&status_calls[0].path).contains(&("logs".into(), "1".into())));
}

/// An image-to-video endpoint: the start frame goes into `image_url`, polling uses the
/// owner/app URLs fal returns (the endpoint path answers 405), and the result is `video.url`.
#[tokio::test]
async fn image_to_video_sends_the_start_frame_and_reads_the_video() {
    const ID: &str = "2d0f7b2e-1c55-4c64-9a4a-0f6e6f2b8c11";
    let fake = start(|call| match (call.host.as_str(), path_only(&call.path)) {
        // https://fal.ai/api/openapi/queue/openapi.json?endpoint_id=fal-ai/kling-video/v2.1/standard/image-to-video
        ("fal.ai", _) => Reply::bytes("application/json", include_bytes!("fixtures/contract/fal/openapi-fal-ai__kling-video__v2.1__standard__image-to-video.json")),
        // Pricing 404 example, https://fal.ai/docs/platform-apis/v1/models/pricing.
        ("api.fal.ai", _) => Reply::status(404, json!({ "error": { "type": "not_found", "message": "Resource not found" } })),
        ("queue.fal.run", "/fal-ai/kling-video/v2.1/standard/image-to-video") => Reply::json(json!({
            "request_id": ID,
            "response_url": format!("https://queue.fal.run/fal-ai/kling-video/requests/{ID}"),
            "status_url": format!("https://queue.fal.run/fal-ai/kling-video/requests/{ID}/status"),
            "cancel_url": format!("https://queue.fal.run/fal-ai/kling-video/requests/{ID}/cancel"),
            "queue_position": 0
        })),
        ("queue.fal.run", p) if p.ends_with("/status") => Reply::json(json!({ "status": "COMPLETED", "request_id": ID, "metrics": { "inference_time": 61.0 } })),
        // Output example from the same OpenAPI document.
        ("queue.fal.run", p) if p == format!("/fal-ai/kling-video/requests/{ID}") => Reply::json(json!({ "video": { "url": "https://v3.fal.media/files/koala/17e3xh08J4_PkHS_0cbwF_output.mp4" } })),
        _ => Reply::status(405, json!({ "detail": "Method Not Allowed" })),
    })
    .await;
    let ctx = ctx(&fake);

    let spec = Fal.resolve(&ctx, &model("fal-ai/kling-video/v2.1/standard/image-to-video", MediaKind::Video, Task::ImageToVideo)).await.unwrap();
    let slot = spec.inputs.iter().find(|s| s.role == InputRole::StartFrame).expect("image_url is the start frame");
    assert_eq!(slot.min, 1, "image_url is required");
    assert!(spec.negative_prompt);

    let frame = TempInput::new(InputRole::StartFrame, PNG_1X1);
    let req = request("fal-ai/kling-video/v2.1/standard/image-to-video", "waves crash on rocks", json!({ "duration": "5" }), vec![frame.0.clone()], 1);
    let Submission::Pending(handle) = Fal.submit(&ctx, &spec, &req).await.unwrap() else { panic!("fal queues") };
    let body = fake.calls().into_iter().find(|c| c.method == "POST").unwrap().json();
    assert_eq!(body["prompt"], "waves crash on rocks");
    assert_eq!(body["duration"], "5");
    // Small local files go inline as data URIs, https://fal.ai/docs/documentation/model-apis/fal-cdn#data-uris.
    let expected = format!("data:image/png;base64,{}", base64_png());
    assert_eq!(body["image_url"], expected.as_str());

    let PollStatus::Done(out) = Fal.poll(&ctx, &handle).await.unwrap() else { panic!("completed") };
    assert!(matches!(&out[..], [Output::Url { url, .. }] if url.ends_with("_output.mp4")));
    let gets: Vec<String> = fake.calls().into_iter().filter(|c| c.method == "GET" && c.host == "queue.fal.run").map(|c| path_only(&c.path).to_string()).collect();
    assert_eq!(gets, [format!("/fal-ai/kling-video/requests/{ID}/status"), format!("/fal-ai/kling-video/requests/{ID}")]);
}

fn base64_png() -> String {
    use base64::Engine as _;
    base64::engine::general_purpose::STANDARD.encode(PNG_1X1)
}

/// Files above the inline limit go to the CDN first: initiate, PUT the bytes, pass `file_url`.
/// Host and body from https://fal.ai/docs/documentation/model-apis/file-access-controls
/// (two-step upload with `POST https://rest.fal.ai/storage/upload/initiate?storage_type=fal-cdn-v3`).
#[tokio::test]
async fn large_inputs_are_uploaded_to_the_documented_cdn_host() {
    let fake = start(|call| match (call.method.as_str(), call.host.as_str()) {
        ("GET", "fal.ai") => Reply::bytes("application/json", include_bytes!("fixtures/contract/fal/openapi-fal-ai__kling-video__v2.1__standard__image-to-video.json")),
        ("POST", host) if host.starts_with("rest.") => Reply::json(json!({
            "upload_url": "https://v3b.fal.media/files/upload/presigned-abc",
            "file_url": "https://v3b.fal.media/files/b/0a98f887/YYbm6L.png"
        })),
        ("PUT", "v3b.fal.media") => Reply::json(json!({})),
        ("POST", "queue.fal.run") => Reply::json(json!({ "request_id": "0b7a5a0e-3c3c-4d7f-8b0a-6a1f0c2b9d10" })),
        _ => Reply::status(404, json!({ "detail": "Not Found" })),
    })
    .await;
    let mut big = PNG_1X1.to_vec();
    big.resize(4 * 1024 * 1024 + 1, 0);
    let frame = TempInput::new(InputRole::StartFrame, &big);
    let ctx = ctx(&fake);
    let spec = Fal.resolve(&ctx, &model("fal-ai/kling-video/v2.1/standard/image-to-video", MediaKind::Video, Task::ImageToVideo)).await.unwrap();
    let req = request(spec.remote_id(), "zoom in", json!({}), vec![frame.0.clone()], 1);
    Fal.submit(&ctx, &spec, &req).await.unwrap();

    let calls: Vec<_> = fake.calls().into_iter().filter(|c| c.method != "GET").collect();
    let initiate = &calls[0];
    assert_eq!((initiate.method.as_str(), initiate.host.as_str(), initiate.path.as_str()), ("POST", "rest.fal.ai", "/storage/upload/initiate?storage_type=fal-cdn-v3"));
    assert_eq!(initiate.header("authorization"), Some("Key test-key"));
    assert_eq!(initiate.json(), json!({ "content_type": "image/png", "file_name": "frame.png" }));
    let put = &calls[1];
    assert_eq!((put.method.as_str(), put.path.as_str()), ("PUT", "/files/upload/presigned-abc"));
    assert_eq!(put.header("content-type"), Some("image/png"));
    assert_eq!(put.body.len(), big.len());
    assert_eq!(calls[2].json()["image_url"], "https://v3b.fal.media/files/b/0a98f887/YYbm6L.png");
}

fn queued(request_id: &str) -> RemoteHandle {
    let base = format!("https://queue.fal.run/fal-ai/flux/requests/{request_id}");
    RemoteHandle::with(
        request_id,
        json!({ "status_url": format!("{base}/status"), "response_url": base, "cancel_url": format!("{base}/cancel"), "output": "image" }),
    )
}

/// A failed request is `COMPLETED` with `error`, or its result read returns the model's error.
#[tokio::test]
async fn failed_requests_surface_the_documented_errors() {
    let fake = start(|call| match path_only(&call.path) {
        // Status fields `error` / `error_type`, https://fal.ai/docs/api-reference/inference-api/queue/status
        // with a type from https://fal.ai/docs/documentation/model-apis/request-errors.
        "/fal-ai/flux/requests/req-timeout/status" => Reply::json(json!({ "status": "COMPLETED", "request_id": "req-timeout", "error": "Request timed out", "error_type": "request_timeout" })),
        "/fal-ai/flux/requests/req-policy/status" => Reply::json(json!({ "status": "COMPLETED", "request_id": "req-policy" })),
        // Result 422, https://fal.ai/docs/documentation/model-apis/errors#content_policy_violation.
        "/fal-ai/flux/requests/req-policy" => Reply::status(422, json!({ "detail": [{
            "loc": ["body", "prompt"],
            "msg": "The content could not be processed because it contained material flagged by a content checker.",
            "type": "content_policy_violation", "url": "https://docs.fal.ai/errors#content_policy_violation",
            "input": "a prompt containing forbidden content", "categories": ["sexual"] }] })),
        _ => Reply::status(404, json!({ "status": "NOT_FOUND" })),
    })
    .await;
    let ctx = ctx(&fake);
    let timed_out = Fal.poll(&ctx, &queued("req-timeout")).await.unwrap();
    assert!(matches!(timed_out, PollStatus::Failed(ProviderError::Failed(m)) if m == "Request timed out"));
    let refused = Fal.poll(&ctx, &queued("req-policy")).await.unwrap();
    assert!(matches!(refused, PollStatus::Failed(ProviderError::ContentPolicy(m)) if m.contains("flagged by a content checker")));
}

/// Submit answers per https://fal.ai/docs/api-reference/inference-api/queue/submit
/// (`DetailError` bodies for 401/403/500, `QueueSubmitError` for 429).
#[tokio::test]
async fn submit_errors_map_to_their_kinds() {
    let fake = start(|call| match call.json()["prompt"].as_str().unwrap_or("") {
        _ if call.host == "fal.ai" => Reply::bytes("application/json", include_bytes!("fixtures/contract/fal/openapi-fal-ai__flux__dev.json")),
        _ if call.host == "api.fal.ai" => Reply::json(flux_dev_price()),
        "401" => Reply::status(401, json!({ "detail": "The credentials are not valid, or the app requires authentication.", "type": "rejected_key" })),
        "403" => Reply::status(403, json!({ "detail": "The key cannot use this model.", "type": "permission_denied" })),
        "429" => Reply::status(429, json!({ "request_id": null, "error": "The queue has fal_max_queue_length requests or more." })),
        _ => Reply::status(500, json!({ "detail": "An internal error occurred. Retry the request." })),
    })
    .await;
    let ctx = ctx(&fake);
    let spec = Fal.resolve(&ctx, &model("fal-ai/flux/dev", MediaKind::Image, Task::TextToImage)).await.unwrap();
    for (case, kind) in [("401", "auth"), ("403", "auth"), ("429", "rate_limited"), ("500", "transient")] {
        let err = Fal.submit(&ctx, &spec, &request("fal-ai/flux/dev", case, json!({}), vec![], 1)).await.unwrap_err();
        assert_eq!(err.kind(), kind, "HTTP {case}: {err:?}");
    }
}

/// Cancel is `PUT cancel_url` with the key, https://fal.ai/docs/documentation/model-apis/inference/queue#cancel-a-request.
#[tokio::test]
async fn cancel_puts_the_cancel_url() {
    let fake = start(|_| Reply::status(202, json!({ "status": "CANCELLATION_REQUESTED" }))).await;
    Fal.cancel(&ctx(&fake), &queued("req-cancel")).await.unwrap();
    let call = &fake.calls()[0];
    assert_eq!((call.method.as_str(), call.path.as_str()), ("PUT", "/fal-ai/flux/requests/req-cancel/cancel"));
    assert_eq!(call.header("authorization"), Some("Key test-key"));
}
