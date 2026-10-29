//! Comfy Router contract: Router's documented answers replayed through the real adapter, checking
//! what it sends. api.comfy.org and docs.comfy.org resolve to the local stand-in; no key or network
//! is used. References: https://docs.comfy.org/development/comfy-router/queue,
//! https://docs.comfy.org/development/comfy-router/reference and
//! https://docs.comfy.org/development/comfy-router/errors. Model schemas are the published documents
//! at https://docs.comfy.org/router-schemas/{provider}/{model}.json (same as `GET .../openapi.json`).

mod common;

use std::sync::{
    atomic::{AtomicUsize, Ordering},
    Arc,
};

use base64::Engine as _;
use common::{start, Call, Fake, Reply, PNG_1X1};
use pictesa_core::{Ctx, GenerationRequest, InputRole, MediaInput, MediaKind, ModelSpec, Output, PollStatus, Provider, ProviderError, RemoteHandle, Submission, Task};
use pictesa_providers::comfy_router::ComfyRouter;
use serde_json::{json, Value};

fn ctx(fake: &Fake) -> Ctx {
    fake.ctx(&["api.comfy.org", "docs.comfy.org"], &[("api_key", "test-key")])
}

fn request(prompt: &str, params: Value, inputs: Vec<MediaInput>) -> GenerationRequest {
    GenerationRequest {
        job_id: "job-1".into(),
        model: "m".into(),
        prompt: prompt.into(),
        negative_prompt: None,
        params: params.as_object().cloned().unwrap_or_default(),
        inputs,
        num_outputs: 1,
    }
}

/// An input file in its own temporary directory, removed on drop.
struct TempInput(MediaInput);

impl TempInput {
    fn new(role: InputRole) -> Self {
        let dir = std::env::temp_dir().join(format!("pictesa-contract-comfy-router-{}", ulid::Ulid::new()));
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

fn model(id: &str, output: MediaKind, task: Task) -> ModelSpec {
    let mut m = ModelSpec::new("comfy_router", id, id, output, &[task]);
    m.complete = false;
    m
}

fn path_only(path: &str) -> &str {
    path.split('?').next().unwrap_or(path)
}

fn png_base64() -> String {
    base64::engine::general_purpose::STANDARD.encode(PNG_1X1)
}

/// The `201` submit answer, https://docs.comfy.org/development/comfy-router/queue#queue-response-shapes.
fn submitted(model: &str, id: &str) -> Value {
    let base = format!("https://api.comfy.org/v2/models/{model}/requests/{id}");
    json!({ "request_id": id, "status": "IN_QUEUE", "queue_position": 3, "status_url": format!("{base}/status"), "response_url": base, "cancel_url": format!("{base}/cancel") })
}

fn status(id: &str, state: &str, extra: Value) -> Value {
    let mut v = json!({ "request_id": id, "status": state, "status_url": "...", "response_url": "...", "cancel_url": "..." });
    v.as_object_mut().unwrap().extend(extra.as_object().cloned().unwrap_or_default());
    v
}

/// A `RouterErrorResponse` with its bucket in `X-Comfy-Error-Type`.
fn router_error(code: u16, bucket: &str, detail: Value) -> Reply {
    Reply::status(code, json!({ "detail": detail, "error_type": bucket })).header("x-comfy-error-type", bucket)
}

fn is_uuid(s: &str) -> bool {
    let parts: Vec<&str> = s.split('-').collect();
    parts.iter().map(|p| p.len()).collect::<Vec<_>>() == [8, 4, 4, 4, 12] && s.chars().all(|c| c == '-' || c.is_ascii_hexdigit())
}

#[tokio::test]
async fn check_lists_one_model_with_the_api_key_header() {
    // https://docs.comfy.org/development/comfy-router/discover-models (catalog example).
    let fake = start(|_| Reply::json(json!({ "data": [{ "id": "bfl/flux-2-pro", "provider": "bfl", "model": "flux-2-pro", "billing": { "charges_on_policy_rejection": "no" } }], "has_more": true, "next_cursor": "example-cursor", "limit": 1 }))).await;
    ComfyRouter.check(&ctx(&fake)).await.unwrap();
    let call = &fake.calls()[0];
    assert_eq!((call.method.as_str(), call.host.as_str(), call.path.as_str()), ("GET", "api.comfy.org", "/v2/models?limit=1"));
    assert_eq!(call.header("x-api-key"), Some("test-key"));
}

#[tokio::test]
async fn discover_walks_the_cursor_until_has_more_is_false() {
    let fake = start(|call| {
        if call.path.contains("cursor=") {
            Reply::json(json!({ "data": [{ "id": "wan/wan2.7-i2v", "provider": "wan", "model": "wan2.7-i2v", "billing": { "charges_on_policy_rejection": "yes" } }], "has_more": false, "limit": 100 }))
        } else {
            // A cursor holding characters that must be URL-encoded (the docs say to encode it).
            Reply::json(json!({ "data": [{ "id": "bfl/flux-2-pro", "provider": "bfl", "model": "flux-2-pro", "billing": { "charges_on_policy_rejection": "no" } }], "has_more": true, "next_cursor": "a+b/c=", "limit": 100 }))
        }
    })
    .await;
    let specs = ComfyRouter.discover(&ctx(&fake)).await.unwrap();
    let paths: Vec<String> = fake.calls().into_iter().map(|c| c.path).collect();
    assert_eq!(paths, ["/v2/models?limit=100", "/v2/models?limit=100&cursor=a%2Bb%2Fc%3D"]);
    let ids: Vec<&str> = specs.iter().map(|s| s.id.as_str()).collect();
    assert_eq!(ids, ["comfy_router:bfl/flux-2-pro", "comfy_router:wan/wan2.7-i2v"]);
    assert_eq!(specs[1].meta["billing"]["charges_on_policy_rejection"], "yes");
}

/// bfl/flux-2-pro end to end: schema, queued submit with an `Idempotency-Key`, each state, collect.
/// Its `input_image` is "Base64 encoded image", so the reference goes as raw base64.
#[tokio::test]
async fn image_model_queues_polls_and_collects() {
    const ID: &str = "6f1a1a6e-6a53-4a5f-9d3a-2b3b0a1f9c21";
    let polls = Arc::new(AtomicUsize::new(0));
    let seen = polls.clone();
    let fake = start(move |call| match (call.method.as_str(), path_only(&call.path)) {
        ("GET", "/v2/models/bfl/flux-2-pro/openapi.json") => Reply::bytes("application/json", include_bytes!("fixtures/contract/comfy_router/bfl__flux-2-pro.json")),
        ("POST", "/v2/models/bfl/flux-2-pro/requests") => Reply::status(201, submitted("bfl/flux-2-pro", ID)),
        ("GET", p) if p.ends_with("/status") => match seen.fetch_add(1, Ordering::SeqCst) {
            0 => Reply::json(status(ID, "IN_QUEUE", json!({ "queue_position": 3 }))),
            1 => Reply::json(status(ID, "IN_PROGRESS", json!({ "queue_position": 0 }))).header("x-comfy-estimated-duration-seconds", "12"),
            _ => Reply::json(status(ID, "COMPLETED", json!({}))),
        },
        // Output example, https://docs.comfy.org/development/comfy-router/models/black-forest-labs/flux-2-pro/code.
        ("GET", p) if p == format!("/v2/models/bfl/flux-2-pro/requests/{ID}") => Reply::json(json!({
            "cost": null, "id": "b2e0c1a4-0f2f-4a55-9f2e-2f9a1c0d4e77", "progress": null, "status": "Ready",
            "result": { "cost": null, "duration": 3.4, "end_time": 1767225603.4, "prompt": "A watercolor painting of a lighthouse at dawn, soft light on the water",
                "sample": "https://example.invalid/bfl/flux-pro-1.1/sample.png", "seed": 2784347701u64, "start_time": 1767225600 }
        })),
        _ => router_error(404, "model_not_found", json!("unexpected call")),
    })
    .await;
    let ctx = ctx(&fake);

    let spec = ComfyRouter.resolve(&ctx, &model("bfl/flux-2-pro", MediaKind::Image, Task::TextToImage)).await.unwrap();
    assert!(spec.inputs.iter().any(|s| s.role == InputRole::Reference));
    let reference = TempInput::new(InputRole::Reference);
    let req = request("A single red maple leaf on a plain white background.", json!({ "output_format": "png" }), vec![reference.0.clone()]);
    let Submission::Pending(handle) = ComfyRouter.submit(&ctx, &spec, &req).await.unwrap() else { panic!("queued") };

    let submit = fake.calls().into_iter().find(|c| c.method == "POST").unwrap();
    assert_eq!(submit.header("x-api-key"), Some("test-key"));
    assert_eq!(submit.header("content-type"), Some("application/json"));
    assert!(is_uuid(submit.header("idempotency-key").unwrap()), "Idempotency-Key is a UUID");
    assert_eq!(submit.json(), json!({ "prompt": "A single red maple leaf on a plain white background.", "output_format": "png", "input_image": png_base64() }));

    assert!(matches!(ComfyRouter.poll(&ctx, &handle).await.unwrap(), PollStatus::Queued { position: Some(3) }));
    assert!(matches!(ComfyRouter.poll(&ctx, &handle).await.unwrap(), PollStatus::Running { message: Some(m), .. } if m.contains("12 s")));
    let PollStatus::Done(out) = ComfyRouter.poll(&ctx, &handle).await.unwrap() else { panic!("completed") };
    assert!(matches!(&out[..], [Output::Url { url, .. }] if url == "https://example.invalid/bfl/flux-pro-1.1/sample.png"));
    let reads: Vec<Call> = fake.calls().into_iter().filter(|c| c.method == "GET" && c.path.contains("/requests/")).collect();
    assert!(reads.iter().all(|c| c.header("x-api-key") == Some("test-key")), "status and result URLs use the submit's key");

    // A retried submit of the same job reuses its key, so Router replays instead of running twice.
    ComfyRouter.submit(&ctx, &spec, &req).await.unwrap();
    let keys: Vec<String> = fake.calls().into_iter().filter(|c| c.method == "POST").map(|c| c.header("idempotency-key").unwrap().to_string()).collect();
    assert_eq!(keys[0], keys[1]);
}

/// wan/wan2.7-i2v takes its first frame as a `media` item of type `first_frame`, with a data URL
/// allowed, inside the DashScope `input` / `parameters` body of the model page's example
/// (https://docs.comfy.org/development/comfy-router/models/wan/wan2-7-i2v/code).
#[tokio::test]
async fn image_to_video_sends_the_first_frame_as_media() {
    const ID: &str = "0b3c1d2e-3f40-4a5b-8c6d-7e8f9a0b1c2d";
    let fake = start(|call| match (call.method.as_str(), path_only(&call.path)) {
        ("GET", "/v2/models/wan/wan2.7-i2v/openapi.json") => Reply::bytes("application/json", include_bytes!("fixtures/contract/comfy_router/wan__wan2.7-i2v.json")),
        ("POST", _) => Reply::status(201, submitted("wan/wan2.7-i2v", ID)),
        ("GET", p) if p.ends_with("/status") => Reply::json(status(ID, "COMPLETED", json!({}))),
        _ => Reply::json(json!({
            "output": { "task_id": "0385dc79-5ff8-4d82-bcb6-7c1a9f2e4d60", "task_status": "SUCCEEDED", "submit_time": "2027-01-01T00:00:00.000Z",
                "scheduled_time": "2027-01-01T00:00:01.000Z", "end_time": "2027-01-01T00:01:04.000Z",
                "orig_prompt": "the robot lowers its raised arm", "actual_prompt": "the robot lowers its raised arm, smooth studio lighting",
                "video_url": "https://.../generated.mp4" },
            "request_id": "7574ee8f-38a3-4b1e-9280-11c33ab46e51", "usage": { "SR": 720, "duration": 5 }
        })),
    })
    .await;
    let ctx = ctx(&fake);
    let spec = ComfyRouter.resolve(&ctx, &model("wan/wan2.7-i2v", MediaKind::Video, Task::ImageToVideo)).await.unwrap();
    assert!(spec.tasks.contains(&Task::ImageToVideo));
    let frame = TempInput::new(InputRole::StartFrame);
    let req = request("the robot lowers its raised arm and steps toward the camera", json!({ "resolution": "720P", "duration": 5 }), vec![frame.0.clone()]);
    let Submission::Pending(handle) = ComfyRouter.submit(&ctx, &spec, &req).await.unwrap() else { panic!("queued") };
    let submit = fake.calls().into_iter().find(|c| c.method == "POST").unwrap();
    assert_eq!(submit.path, "/v2/models/wan/wan2.7-i2v/requests");
    assert_eq!(
        submit.json(),
        json!({
            "input": { "prompt": "the robot lowers its raised arm and steps toward the camera",
                "media": [{ "type": "first_frame", "url": format!("data:image/png;base64,{}", png_base64()) }] },
            "parameters": { "resolution": "720P", "duration": 5 }
        })
    );
    let PollStatus::Done(out) = ComfyRouter.poll(&ctx, &handle).await.unwrap() else { panic!("completed") };
    assert!(matches!(&out[..], [Output::Url { url, .. }] if url == "https://.../generated.mp4"));
}

/// A model whose partner answers with raw bytes cannot be queued: `403` / `not_enabled` says to run
/// it on the synchronous route (reference, `POST .../requests` 403), whose `200` is the bytes under
/// the partner's own `Content-Type`. Schema: elevenlabs/eleven_sfx_v2 (output `*/*` raw audio).
#[tokio::test]
async fn unqueueable_models_run_on_the_synchronous_route() {
    let fake = start(|call| match (call.method.as_str(), path_only(&call.path)) {
        ("GET", _) => Reply::bytes("application/json", include_bytes!("fixtures/contract/comfy_router/elevenlabs__eleven_sfx_v2.json")),
        ("POST", "/v2/models/elevenlabs/eleven_sfx_v2/requests") => router_error(403, "not_enabled", json!("This model answers with raw bytes and cannot be queued; call POST /v2/models/elevenlabs/eleven_sfx_v2 instead.")),
        ("POST", "/v2/models/elevenlabs/eleven_sfx_v2") => Reply::bytes("audio/mpeg", b"ID3\x04\x00fake-mp3"),
        _ => router_error(404, "model_not_found", json!("unexpected call")),
    })
    .await;
    let ctx = ctx(&fake);
    let spec = ComfyRouter.resolve(&ctx, &model("elevenlabs/eleven_sfx_v2", MediaKind::Audio, Task::TextToAudio)).await.unwrap();
    let req = request("A distant rumble of thunder rolling across a valley.", json!({}), vec![]);
    let Submission::Done(out) = ComfyRouter.submit(&ctx, &spec, &req).await.unwrap() else { panic!("synchronous") };
    assert!(matches!(&out[..], [Output::Bytes { data, mime }] if data.starts_with(b"ID3") && mime == "audio/mpeg"));
    let posts: Vec<Call> = fake.calls().into_iter().filter(|c| c.method == "POST").collect();
    assert_eq!(posts.len(), 2);
    assert_eq!(posts[1].json()["text"], "A distant rumble of thunder rolling across a valley.");
    assert_eq!(posts[0].header("idempotency-key"), posts[1].header("idempotency-key"), "same logical call, same key");
}

/// Status answers carry `Retry-After`, Router's hint for when polling again is worth it; earlier
/// engine polls are answered from the last status without a request.
#[tokio::test]
async fn status_polls_wait_for_retry_after() {
    let fake = start(|_| Reply::json(status("1d2e3f40-0000-4000-8000-000000000001", "IN_QUEUE", json!({ "queue_position": 5 }))).header("retry-after", "30")).await;
    let handle = RemoteHandle::with("1d2e3f40-0000-4000-8000-000000000001", json!({ "model": "bfl/flux-2-pro", "output": "image" }));
    let ctx = ctx(&fake);
    for _ in 0..3 {
        assert!(matches!(ComfyRouter.poll(&ctx, &handle).await.unwrap(), PollStatus::Queued { position: Some(5) }));
    }
    assert_eq!(fake.calls().len(), 1);
    assert_eq!(fake.calls()[0].path, "/v2/models/bfl/flux-2-pro/requests/1d2e3f40-0000-4000-8000-000000000001/status");
}

/// A request that did not succeed is `COMPLETED` with an `error_type`; a finished request is kept
/// 24 hours, then the reads answer `410`.
#[tokio::test]
async fn completed_requests_with_an_error_type_fail() {
    let fake = start(|call| {
        let id = call.path.split('/').nth(6).unwrap_or("").to_string();
        let reading_status = call.path.ends_with("/status");
        match (id.as_str(), reading_status) {
            ("2a000000-0000-4000-8000-00000000000a", true) => Reply::json(status(&id, "COMPLETED", json!({ "error_type": "content_policy_violation" }))),
            ("2a000000-0000-4000-8000-00000000000a", false) => router_error(400, "content_policy_violation", json!("The provider refused the request on content-policy grounds.")),
            ("2b000000-0000-4000-8000-00000000000b", true) => Reply::json(status(&id, "COMPLETED", json!({ "error_type": "cancelled" }))),
            ("2b000000-0000-4000-8000-00000000000b", false) => router_error(409, "cancelled", json!("The request was cancelled.")),
            _ => router_error(410, "result_expired", json!("The result is no longer stored.")),
        }
    })
    .await;
    let ctx = ctx(&fake);
    let handle = |id: &str| RemoteHandle::with(id, json!({ "model": "bfl/flux-2-pro", "output": "image" }));
    assert!(matches!(ComfyRouter.poll(&ctx, &handle("2a000000-0000-4000-8000-00000000000a")).await.unwrap(), PollStatus::Failed(ProviderError::ContentPolicy(_))));
    assert!(matches!(ComfyRouter.poll(&ctx, &handle("2b000000-0000-4000-8000-00000000000b")).await.unwrap(), PollStatus::Failed(ProviderError::Failed(m)) if m == "cancelled"));
    assert!(matches!(ComfyRouter.poll(&ctx, &handle("2c000000-0000-4000-8000-00000000000c")).await.unwrap(), PollStatus::Failed(_)));
}

/// Submit errors: buckets from the reference's error table, statuses from the queue route
/// (`POST .../requests`), bodies as `RouterErrorResponse` / the validation `detail[]`.
#[tokio::test]
async fn submit_errors_map_to_their_buckets() {
    let fake = start(|call| match call.json()["prompt"].as_str().unwrap_or("") {
        _ if call.method == "GET" => Reply::bytes("application/json", include_bytes!("fixtures/contract/comfy_router/bfl__flux-2-pro.json")),
        "401" => router_error(401, "unauthorized", json!("Missing or invalid credentials.")),
        "402" => router_error(402, "insufficient_credits", json!("The calling workspace does not have enough credits to run the model.")),
        "403" => router_error(403, "forbidden", json!("The credential is valid but is not entitled to this model or this operation.")),
        "409" => router_error(409, "concurrency_limit_exceeded", json!("The original call for this key is still running.")).header("retry-after", "3"),
        "422" => Reply::status(422, json!({ "detail": [{ "loc": ["body", "prompt"], "msg": "Field required", "type": "missing" }] })).header("x-comfy-error-type", "invalid_input"),
        "429" => router_error(429, "rate_limited", json!("The caller has spent an allowance measured over a window.")).header("retry-after", "7"),
        "429-backlog" => router_error(429, "queue_backlog_full", json!("The caller already has too many queued requests waiting to run.")),
        _ => router_error(503, "service_unavailable", json!("A service Comfy Router depends on is temporarily unavailable.")),
    })
    .await;
    let ctx = ctx(&fake);
    let spec = ComfyRouter.resolve(&ctx, &model("bfl/flux-2-pro", MediaKind::Image, Task::TextToImage)).await.unwrap();
    for (case, kind) in [
        ("401", "auth"),
        ("402", "insufficient_credits"),
        ("403", "auth"),
        ("409", "rate_limited"),
        ("422", "invalid_input"),
        ("429", "rate_limited"),
        ("429-backlog", "rate_limited"),
        ("503", "transient"),
    ] {
        let err = ComfyRouter.submit(&ctx, &spec, &request(case, json!({}), vec![])).await.unwrap_err();
        assert_eq!(err.kind(), kind, "{case}: {err:?}");
        if case == "429" {
            assert!(matches!(err, ProviderError::RateLimited { retry_after_ms: Some(7_000) }));
        }
    }
}

/// `PUT cancel_url` with the key; `202` `CANCELLATION_REQUESTED` (queue guide, "Cancel").
#[tokio::test]
async fn cancel_puts_the_cancel_url() {
    let fake = start(|_| Reply::status(202, json!({ "request_id": "3d000000-0000-4000-8000-00000000000d", "status": "CANCELLATION_REQUESTED" }))).await;
    let base = "https://api.comfy.org/v2/models/bfl/flux-2-pro/requests/3d000000-0000-4000-8000-00000000000d";
    let handle = RemoteHandle::with("3d000000-0000-4000-8000-00000000000d", json!({ "model": "bfl/flux-2-pro", "cancel_url": format!("{base}/cancel") }));
    ComfyRouter.cancel(&ctx(&fake), &handle).await.unwrap();
    let call = &fake.calls()[0];
    assert_eq!((call.method.as_str(), call.path.as_str()), ("PUT", "/v2/models/bfl/flux-2-pro/requests/3d000000-0000-4000-8000-00000000000d/cancel"));
    assert_eq!(call.header("x-api-key"), Some("test-key"));
}
