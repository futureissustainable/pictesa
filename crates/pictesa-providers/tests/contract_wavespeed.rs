//! WaveSpeed contract: WaveSpeed's documented answers replayed through the real adapter, checking
//! what it sends. api.wavespeed.ai resolves to the local stand-in; no key or network is used.
//! Reference: https://wavespeed.ai/docs/rest-api. WaveSpeed documents no cancel route for a
//! running task (https://wavespeed.ai/docs/delete-task skips created and processing tasks).

mod common;

use std::sync::{
    atomic::{AtomicUsize, Ordering},
    Arc,
};

use common::{start, Fake, Reply};
use pictesa_core::{Ctx, GenerationRequest, InputRole, MediaInput, MediaKind, Output, PollStatus, Provider, ProviderError, RemoteHandle, Submission, Task};
use pictesa_providers::wavespeed::WaveSpeed;
use serde_json::{json, Value};

fn ctx(fake: &Fake) -> Ctx {
    fake.ctx(&["api.wavespeed.ai"], &[("api_key", "test-key")])
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

/// List Models, https://wavespeed.ai/docs/list-models (200 example), plus an image-to-video entry of
/// the same shape whose fields are the ones https://wavespeed.ai/docs/create-video-from-image sends.
fn catalog() -> Value {
    json!({ "code": 200, "message": "success", "data": [
        { "model_id": "wavespeed-ai/hunyuan-video/t2v", "name": "wavespeed-ai/hunyuan-video/t2v", "base_price": 0.4,
          "description": "The advanced text-to-video model for generating high-definition videos", "type": "text-to-video",
          "api_schema": { "api_schemas": [{ "type": "model_run", "method": "POST", "server": "https://api.wavespeed.ai", "api_path": "/api/v3/wavespeed-ai/hunyuan-video/t2v",
            "request_schema": { "properties": {
                "prompt": { "description": "The positive prompt for the generation.", "type": "string" },
                "size": { "default": "1280*720", "enum": ["1280*720", "720*1280"], "type": "string" },
                "seed": { "type": "integer" },
                "num_inference_steps": { "default": 30, "minimum": 2, "maximum": 30, "type": "integer" }
            }, "required": ["prompt"] } }] } },
        { "model_id": "wavespeed-ai/minimax-h3/image-to-video", "name": "MiniMax H3 I2V", "base_price": 0.1,
          "description": "Animate an image", "type": "image-to-video",
          "api_schema": { "api_schemas": [{ "type": "model_run", "method": "POST", "server": "https://api.wavespeed.ai", "api_path": "/api/v3/wavespeed-ai/minimax-h3/image-to-video",
            "request_schema": { "properties": {
                "prompt": { "type": "string" },
                "image": { "type": "string", "description": "Source image URL" },
                "duration": { "type": "integer", "default": 5 },
                "resolution": { "type": "string", "enum": ["480p", "540p", "768p", "1080p"], "default": "768p" }
            }, "required": ["prompt", "image"] } }] } }
    ] })
}

/// Submit answer, https://wavespeed.ai/docs/submit-task.
fn submitted(id: &str, model: &str) -> Value {
    json!({ "code": 200, "message": "success", "data": { "id": id, "model": model, "status": "created",
        "urls": { "get": format!("https://api.wavespeed.ai/api/v3/predictions/{id}/result") }, "created_at": "2024-01-01T12:00:00.000Z" } })
}

/// Result answer, https://wavespeed.ai/docs/get-result and https://wavespeed.ai/docs/rest-api.
fn result(id: &str, data: Value) -> Value {
    let mut data = data;
    data["id"] = json!(id);
    json!({ "code": 200, "message": "success", "data": data })
}

#[tokio::test]
async fn check_lists_models_with_a_bearer_token() {
    let fake = start(|_| Reply::json(catalog())).await;
    WaveSpeed.check(&ctx(&fake)).await.unwrap();
    let call = &fake.calls()[0];
    assert_eq!((call.method.as_str(), call.path.as_str()), ("GET", "/api/v3/models"));
    // https://wavespeed.ai/docs/api-authentication
    assert_eq!(call.header("authorization"), Some("Bearer test-key"));
}

#[tokio::test]
async fn discover_reads_each_model_run_schema() {
    let fake = start(|_| Reply::json(catalog())).await;
    let specs = WaveSpeed.discover(&ctx(&fake)).await.unwrap();
    let t2v = specs.iter().find(|s| s.id == "wavespeed:wavespeed-ai/hunyuan-video/t2v").unwrap();
    assert_eq!((t2v.output, t2v.tasks.clone()), (MediaKind::Video, vec![Task::TextToVideo]));
    assert!(t2v.params.iter().any(|p| p.key == "size") && t2v.params.iter().any(|p| p.key == "num_inference_steps"));
    let i2v = specs.iter().find(|s| s.id == "wavespeed:wavespeed-ai/minimax-h3/image-to-video").unwrap();
    assert!(i2v.inputs.iter().any(|s| s.role == InputRole::StartFrame && s.min == 1), "image is the required start frame");
}

/// The image model of the docs' own example: `POST /api/v3/{model_id}`, then every result state.
#[tokio::test]
async fn image_model_submits_and_polls_to_the_outputs() {
    let polls = Arc::new(AtomicUsize::new(0));
    let seen = polls.clone();
    let fake = start(move |call| match call.method.as_str() {
        "POST" => Reply::json(submitted("pred_abc123", "wavespeed-ai/z-image/turbo")),
        _ => Reply::json(result("pred_abc123", match seen.fetch_add(1, Ordering::SeqCst) {
            0 => json!({ "status": "created" }),
            1 => json!({ "status": "processing" }),
            _ => json!({ "status": "completed", "outputs": ["https://d1q70pf5vjeyhc.cloudfront.net/predictions/pred_abc123/1.jpeg"], "timings": { "inference": 2500 } }),
        })),
    })
    .await;
    let ctx = ctx(&fake);
    let model = WaveSpeed.models().into_iter().find(|m| m.id == "wavespeed:wavespeed-ai/z-image/turbo").unwrap();
    let req = request("A cat wearing a space suit", json!({ "aspect_ratio": "1:1" }), vec![]);
    let Submission::Pending(handle) = WaveSpeed.submit(&ctx, &model, &req).await.unwrap() else { panic!("WaveSpeed queues") };
    let call = &fake.calls()[0];
    assert_eq!((call.method.as_str(), call.path.as_str()), ("POST", "/api/v3/wavespeed-ai/z-image/turbo"));
    assert_eq!(call.header("authorization"), Some("Bearer test-key"));
    assert_eq!(call.json(), json!({ "prompt": "A cat wearing a space suit", "size": "1024*1024" }));
    assert_eq!(handle.id, "pred_abc123");

    assert!(matches!(WaveSpeed.poll(&ctx, &handle).await.unwrap(), PollStatus::Queued { .. }));
    assert!(matches!(WaveSpeed.poll(&ctx, &handle).await.unwrap(), PollStatus::Running { .. }));
    let PollStatus::Done(out) = WaveSpeed.poll(&ctx, &handle).await.unwrap() else { panic!("completed") };
    assert!(matches!(&out[..], [Output::Url { url, .. }] if url.ends_with("/1.jpeg")));
    let poll = &fake.calls()[1];
    assert_eq!((poll.method.as_str(), poll.path.as_str()), ("GET", "/api/v3/predictions/pred_abc123/result"));
    assert_eq!(poll.header("authorization"), Some("Bearer test-key"));
}

/// Image-to-video from the catalog schema: the start frame goes in `image`
/// (body of https://wavespeed.ai/docs/create-video-from-image).
#[tokio::test]
async fn image_to_video_sends_the_image_url() {
    let fake = start(|call| match (call.method.as_str(), call.path.as_str()) {
        ("GET", "/api/v3/models") => Reply::json(catalog()),
        ("POST", _) => Reply::json(submitted("pred_i2v", "wavespeed-ai/minimax-h3/image-to-video")),
        _ => Reply::json(result("pred_i2v", json!({ "status": "completed", "outputs": ["https://d1q70pf5vjeyhc.cloudfront.net/predictions/pred_i2v/output.mp4"] }))),
    })
    .await;
    let ctx = ctx(&fake);
    let spec = WaveSpeed.discover(&ctx).await.unwrap().into_iter().find(|s| s.id == "wavespeed:wavespeed-ai/minimax-h3/image-to-video").unwrap();
    let frame = MediaInput {
        role: InputRole::StartFrame,
        kind: MediaKind::Image,
        mime: "image/jpeg".into(),
        path: "/nonexistent".into(),
        url: Some("https://your-uploaded-image.jpg".into()),
        width: None,
        height: None,
    };
    let req = request("The cat slowly turns its head and blinks", json!({ "duration": 5, "resolution": "768p" }), vec![frame]);
    let Submission::Pending(handle) = WaveSpeed.submit(&ctx, &spec, &req).await.unwrap() else { panic!("WaveSpeed queues") };
    let submit = fake.calls().into_iter().find(|c| c.method == "POST").unwrap();
    assert_eq!(submit.path, "/api/v3/wavespeed-ai/minimax-h3/image-to-video");
    assert_eq!(
        submit.json(),
        json!({ "prompt": "The cat slowly turns its head and blinks", "image": "https://your-uploaded-image.jpg", "duration": 5, "resolution": "768p" })
    );
    let PollStatus::Done(out) = WaveSpeed.poll(&ctx, &handle).await.unwrap() else { panic!("completed") };
    assert!(matches!(&out[..], [Output::Url { url, .. }] if url.ends_with("output.mp4")));
}

/// Failure terminal statuses and their task codes (https://wavespeed.ai/docs/error-codes), and the
/// rule that any status outside the documented list means "keep polling"
/// (https://wavespeed.ai/docs/errors-retries-timeouts).
#[tokio::test]
async fn result_states_follow_the_status_table() {
    let fake = start(|call| {
        let id = call.path.split('/').nth(4).unwrap_or("").to_string();
        Reply::json(result(&id, match id.as_str() {
            "media" => json!({ "model": "model-name", "status": "failed", "code": 1402, "error": "Failed to download image from the provided URL.", "outputs": [] }),
            "credits" => json!({ "status": "failed", "code": 1407, "error": "Error description" }),
            "moderated" => json!({ "status": "failed", "code": 1200, "error": "Error description" }),
            "timeout" => json!({ "status": "timeout" }),
            "deleted" => json!({ "model": "wavespeed-ai/z-image/turbo", "status": "deleted" }),
            _ => json!({ "status": "queued" }),
        }))
    })
    .await;
    let ctx = ctx(&fake);
    let poll = |id: &'static str| {
        let ctx = ctx.clone();
        async move { WaveSpeed.poll(&ctx, &RemoteHandle::with(id, json!({ "kind": "image" }))).await.unwrap() }
    };
    assert!(matches!(poll("media").await, PollStatus::Failed(ProviderError::Failed(m)) if m.starts_with("Failed to download image")));
    assert!(matches!(poll("credits").await, PollStatus::Failed(ProviderError::InsufficientCredits(_))));
    assert!(matches!(poll("moderated").await, PollStatus::Failed(ProviderError::ContentPolicy(_))));
    assert!(matches!(poll("timeout").await, PollStatus::Failed(_)));
    assert!(matches!(poll("deleted").await, PollStatus::Failed(_)));
    assert!(matches!(poll("other").await, PollStatus::Running { .. }));
}

/// HTTP errors (https://wavespeed.ai/docs/rest-api, error table; 403 body from
/// https://wavespeed.ai/docs/api-authentication). Polls retry 429 and 5xx; a 5xx on submit is never
/// retried because the task may exist (https://wavespeed.ai/docs/errors-retries-timeouts).
#[tokio::test]
async fn http_errors_map_to_their_kinds() {
    let fake = start(|call| {
        let case = if call.method == "POST" { call.json()["prompt"].as_str().unwrap_or("").to_string() } else { call.path.split('/').nth(4).unwrap_or("").to_string() };
        match case.as_str() {
            "401" => Reply::status(401, json!({ "code": 401, "message": "Unauthorized" })),
            "403" => Reply::status(403, json!({ "code": 403,
                "message": "This API key was created by a Billing member, which cannot run models or read predictions. Use a key created by an Owner, an Admin, or a Developer instead. See https://wavespeed.ai/docs/api-authentication",
                "error_code": "API_KEY_PERMISSION_FORBIDDEN", "required_scope": "inference", "key_scope": "billing" })),
            "429" => Reply::status(429, json!({ "code": 429, "message": "Rate limit exceeded" })),
            _ => Reply::status(500, json!({ "code": 500, "message": "Server error" })),
        }
    })
    .await;
    let ctx = ctx(&fake);
    let model = WaveSpeed.models().into_iter().next().unwrap();
    for (case, kind) in [("401", "auth"), ("403", "auth"), ("429", "rate_limited"), ("500", "failed")] {
        let err = WaveSpeed.submit(&ctx, &model, &request(case, json!({}), vec![])).await.unwrap_err();
        assert_eq!(err.kind(), kind, "submit {case}: {err:?}");
    }
    for (case, kind) in [("429", "rate_limited"), ("500", "transient")] {
        let err = WaveSpeed.poll(&ctx, &RemoteHandle::new(case)).await.unwrap_err();
        assert_eq!(err.kind(), kind, "poll {case}: {err:?}");
        assert!(err.is_retryable());
    }
}
