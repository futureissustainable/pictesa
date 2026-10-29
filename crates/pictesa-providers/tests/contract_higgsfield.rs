//! Higgsfield contract: the documented answers of api.higgsfield.ai replayed through the adapter,
//! and what the adapter sends checked against the published API.
//! Docs: https://docs.higgsfield.ai/docs/llms.txt and the model pages it links (read 29.10.2026).

mod common;

use common::{start, Reply, PNG_1X1};
use pictesa_core::{GenerationRequest, InputRole, MediaInput, MediaKind, ModelSpec, Output, PollStatus, Provider, ProviderError, Submission};
use pictesa_providers::higgsfield::Higgsfield;
use serde_json::{json, Value};

const HOST: &str = "api.higgsfield.ai";
const REQUEST: &str = "d7e6c0f3-6699-4f6c-bb45-2ad7fd9158ff";

/// Test values for the two credential fields the adapter declares (key ID and secret).
fn creds() -> Vec<(String, String)> {
    let fields = Higgsfield.info().fields;
    assert_eq!(fields.len(), 2, "Higgsfield credentials are a key ID and a secret");
    fields.iter().map(|f| (f.key.clone(), format!("test-{}", f.key))).collect()
}

fn ctx(fake: &common::Fake, hosts: &[&str]) -> pictesa_core::Ctx {
    let creds = creds();
    let pairs: Vec<(&str, &str)> = creds.iter().map(|(k, v)| (k.as_str(), v.as_str())).collect();
    fake.ctx(hosts, &pairs)
}

/// https://docs.higgsfield.ai/docs/authentication.md: `Authorization: Key YOUR_KEY_ID:YOUR_KEY_SECRET`
fn expected_auth() -> String {
    let creds = creds();
    let id = &creds.iter().find(|(k, _)| k == "key_id").unwrap().1;
    let secret = &creds.iter().find(|(k, _)| k == "key_secret").unwrap().1;
    format!("Key {id}:{secret}")
}

fn spec(id: &str) -> ModelSpec {
    Higgsfield.models().into_iter().find(|m| m.remote_id() == id).unwrap()
}

/// A request as the engine hands it over: params checked and filled with the model's defaults.
fn request(model: &ModelSpec, params: Value, inputs: Vec<MediaInput>) -> GenerationRequest {
    let mut p = params.as_object().cloned().unwrap_or_default();
    for spec in &model.params {
        if let Some(d) = &spec.default {
            p.entry(spec.key.clone()).or_insert(d.clone());
        }
    }
    GenerationRequest { job_id: "job-1".into(), model: model.remote_id().into(), prompt: "A quiet alpine lake at sunrise".into(), negative_prompt: None, params: p, inputs, num_outputs: 1 }
}

fn hosted(role: InputRole, kind: MediaKind, url: &str) -> MediaInput {
    MediaInput { role, kind, mime: "image/jpeg".into(), path: "unused".into(), url: Some(url.into()), width: None, height: None }
}

/// The submission answer. https://docs.higgsfield.ai/docs/concepts/requests.md ("Initial response")
fn accepted() -> Value {
    json!({
        "status": "queued",
        "request_id": REQUEST,
        "status_url": format!("https://api.higgsfield.ai/requests/{REQUEST}/status"),
        "cancel_url": format!("https://api.higgsfield.ai/requests/{REQUEST}/cancel"),
        "expires_after": 604800
    })
}

#[tokio::test]
async fn check_reads_usage_with_the_key_authorization_header() {
    // https://docs.higgsfield.ai/docs/enterprise/usage-and-billing.md (GET /models/usage and its example)
    let fake = start(|_| Reply::json(json!({ "time_series": [], "total": 0, "page": 1, "limit": 50 }))).await;
    Higgsfield.check(&ctx(&fake, &[HOST])).await.unwrap();
    let call = &fake.calls()[0];
    assert_eq!((call.method.as_str(), call.path.as_str()), ("GET", "/models/usage"));
    assert_eq!(call.header("authorization"), Some(expected_auth().as_str()));
}

#[tokio::test]
async fn every_catalog_model_sends_only_fields_its_documented_schema_accepts() {
    // Each model page publishes the endpoint's complete JSON schema; the video and Qwen schemas set
    // `additionalProperties: false`, so any other field is a 422.
    let start_frame = hosted(InputRole::StartFrame, MediaKind::Image, "https://example.com/input.jpg");
    let reference = hosted(InputRole::Reference, MediaKind::Image, "https://example.com/reference.jpg");
    let source = hosted(InputRole::Video, MediaKind::Video, "https://example.com/source.mp4");
    let cases: [(&str, &[&str], &[&str], Vec<MediaInput>); 9] = [
        // https://docs.higgsfield.ai/docs/models/soul-standard/generate.md
        ("higgsfield-ai/soul/standard", &["prompt"], &["prompt", "aspect_ratio", "enhance_prompt", "style_id", "style_strength", "resolution", "seed", "custom_reference_id", "custom_reference_strength", "image_reference_url", "batch_size"], vec![]),
        // https://docs.higgsfield.ai/docs/models/soul-2/generate.md
        ("higgsfield-ai/soul/v2/standard", &["prompt"], &["prompt", "aspect_ratio", "resolution", "enhance_prompt", "custom_reference_id", "custom_reference_strength", "style_id", "style_strength", "seed", "batch_size"], vec![]),
        // https://docs.higgsfield.ai/docs/models/qwen-image-3/text-to-image.md
        ("alibaba/qwen-image-3/text-to-image", &["prompt"], &["seed", "prompt", "resolution", "aspect_ratio", "prompt_extend", "enable_thinking", "negative_prompt", "prompt_extend_mode"], vec![]),
        // https://docs.higgsfield.ai/docs/models/qwen-image-3/edit.md
        ("alibaba/qwen-image-3/edit", &["prompt", "image_urls"], &["seed", "prompt", "image_urls", "resolution", "aspect_ratio", "prompt_extend", "enable_thinking", "negative_prompt", "prompt_extend_mode"], vec![reference.clone()]),
        // https://docs.higgsfield.ai/docs/models/seedance-2-5/text-to-video.md
        ("bytedance/seedance-2.5/text-to-video", &["prompt"], &["prompt", "duration", "resolution", "aspect_ratio", "bitrate_mode", "output_format", "generate_audio"], vec![]),
        // https://docs.higgsfield.ai/docs/models/seedance-2-5/image-to-video.md
        ("bytedance/seedance-2.5/image-to-video", &["image_url"], &["prompt", "duration", "image_url", "resolution", "bitrate_mode", "end_image_url", "generate_audio"], vec![start_frame]),
        // https://docs.higgsfield.ai/docs/models/seedance-2-5/reference-to-video.md
        ("bytedance/seedance-2.5/reference-to-video", &["image_urls"], &["prompt", "duration", "audio_urls", "image_urls", "resolution", "video_urls", "aspect_ratio", "bitrate_mode", "generate_audio"], vec![reference.clone()]),
        // https://docs.higgsfield.ai/docs/models/cinema-studio-4/generate.md
        ("higgsfield/cinema-studio/4.0", &["prompt"], &["era", "genre", "light", "pacing", "prompt", "duration", "audio_urls", "image_urls", "resolution", "video_urls", "camera_lens", "aspect_ratio", "camera_model", "color_palette", "generate_audio", "camera_aperture", "camera_movement"], vec![]),
        // https://docs.higgsfield.ai/docs/models/genjutsu/restyle.md
        ("higgsfield/genjutsu/restyle/v1.0", &["video_url", "preset_id"], &["prompt", "preset_id", "video_url", "image_urls", "resolution"], vec![source]),
    ];
    assert_eq!(cases.len(), Higgsfield.models().len(), "every catalog model has a documented schema case");
    for (id, required, allowed, inputs) in cases {
        let model = spec(id);
        // Every setting the catalog offers is filled: defaults, plus a value for those without one.
        let mut extra = serde_json::Map::new();
        for p in model.params.iter().filter(|p| p.default.is_none()) {
            extra.insert(p.key.clone(), if p.key == "seed" { json!(7) } else { json!("3fa85f64-5717-4562-b3fc-2c963f66afa6") });
        }
        let req = request(&model, Value::Object(extra), inputs);
        let fake = start(|_| Reply::json(accepted())).await;
        let Submission::Pending(handle) = Higgsfield.submit(&ctx(&fake, &[HOST]), &model, &req).await.unwrap() else { panic!("{id}") };
        assert_eq!(handle.id, REQUEST);
        let call = &fake.calls()[0];
        assert_eq!((call.method.as_str(), call.path.as_str()), ("POST", format!("/{id}").as_str()));
        assert_eq!(call.header("authorization"), Some(expected_auth().as_str()), "{id}");
        // https://docs.higgsfield.ai/docs/concepts/idempotency.md: one key per generation submission.
        assert_eq!(call.header("idempotency-key"), Some("job-1"), "{id}");
        let body = call.json();
        let keys: Vec<&str> = body.as_object().unwrap().keys().map(String::as_str).collect();
        for key in &keys {
            assert!(allowed.contains(key), "{id} sends `{key}`, which its schema does not accept: {body}");
        }
        for key in required {
            assert!(keys.contains(key), "{id} misses required `{key}`: {body}");
        }
    }
}

#[tokio::test]
async fn a_local_input_is_uploaded_through_a_presigned_url() {
    // https://docs.higgsfield.ai/docs/concepts/file-uploads.md: POST /files/generate-upload-url
    // {content_type}; PUT the file to upload_url with every upload_headers entry and without the API
    // credentials; pass public_url as the model's input URL.
    let fake = start(|call| match call.path.as_str() {
        "/files/generate-upload-url" => Reply::json(json!({
            "public_url": "https://cdn.example.com/input/example.jpeg",
            "upload_url": "https://storage.example.com/presigned-upload-url",
            "content_type": "image/png",
            "upload_headers": { "Content-Type": "image/png", "x-amz-tagging": "retention=temporary" }
        })),
        "/presigned-upload-url" => Reply::status(200, Value::Null),
        _ => Reply::json(accepted()),
    })
    .await;
    let dir = std::env::temp_dir().join(format!("pictesa-contract-higgsfield-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("frame.png");
    std::fs::write(&path, PNG_1X1).unwrap();
    let local = MediaInput { role: InputRole::StartFrame, kind: MediaKind::Image, mime: "image/png".into(), path, url: None, width: None, height: None };
    let model = spec("bytedance/seedance-2.5/image-to-video");
    Higgsfield.submit(&ctx(&fake, &[HOST, "storage.example.com"]), &model, &request(&model, json!({}), vec![local])).await.unwrap();

    let calls = fake.calls();
    assert_eq!(calls.len(), 3);
    assert_eq!((calls[0].method.as_str(), calls[0].path.as_str()), ("POST", "/files/generate-upload-url"));
    assert_eq!(calls[0].header("authorization"), Some(expected_auth().as_str()));
    assert_eq!(calls[0].json(), json!({ "content_type": "image/png" }));
    let put = &calls[1];
    assert_eq!((put.method.as_str(), put.host.as_str()), ("PUT", "storage.example.com"));
    assert_eq!((put.header("content-type"), put.header("x-amz-tagging")), (Some("image/png"), Some("retention=temporary")));
    assert_eq!(put.header("authorization"), None, "credentials must not reach the storage URL");
    assert_eq!(put.body, PNG_1X1);
    assert_eq!(calls[2].json()["image_url"], "https://cdn.example.com/input/example.jpeg");
}

#[tokio::test]
async fn every_documented_request_status_maps_to_a_poll_status() {
    // https://docs.higgsfield.ai/docs/concepts/requests.md (lifecycle table, "Completed output") and
    // https://docs.higgsfield.ai/docs/api-reference/requests/get-request-status.md (`RequestStatus`).
    let image = spec("higgsfield-ai/soul/v2/standard");
    let video = spec("bytedance/seedance-2.5/text-to-video");
    let cases = [
        (&image, json!({ "status": "queued", "request_id": REQUEST })),
        (&image, json!({ "status": "in_progress", "request_id": REQUEST })),
        (&image, json!({ "status": "completed", "request_id": REQUEST, "expires_at": "2026-10-31T12:05:12.481Z", "images": [{ "url": "https://cdn.example.com/image-1.jpg" }, { "url": "https://cdn.example.com/image-2.jpg" }] })),
        (&video, json!({ "status": "completed", "request_id": REQUEST, "video": { "url": "https://cdn.example.com/video.mp4" } })),
        // https://docs.higgsfield.ai/docs/concepts/errors.md ("Terminal failures")
        (&image, json!({ "status": "failed", "request_id": REQUEST, "error": "Generation failed" })),
        // `nsfw`: "Input or output was rejected by content moderation"; `error` is optional.
        (&image, json!({ "status": "nsfw", "request_id": REQUEST, "error": null })),
        (&image, json!({ "status": "canceled", "request_id": REQUEST })),
    ];
    let mut seen = Vec::new();
    for (model, answer) in cases {
        let fake = start(move |call| if call.method == "POST" { Reply::json(accepted()) } else { Reply::json(answer.clone()) }).await;
        let ctx = ctx(&fake, &[HOST]);
        let Submission::Pending(handle) = Higgsfield.submit(&ctx, model, &request(model, json!({}), vec![])).await.unwrap() else { panic!() };
        seen.push(Higgsfield.poll(&ctx, &handle).await.unwrap());
        let poll = &fake.calls()[1];
        assert_eq!((poll.method.as_str(), poll.path.as_str()), ("GET", format!("/requests/{REQUEST}/status").as_str()));
        assert_eq!(poll.header("authorization"), Some(expected_auth().as_str()));
    }
    let urls = |s: &PollStatus| match s {
        PollStatus::Done(o) => o.iter().map(|o| match o { Output::Url { url, .. } => url.clone(), other => panic!("{other:?}") }).collect::<Vec<_>>(),
        other => panic!("not done: {other:?}"),
    };
    assert!(matches!(seen[0], PollStatus::Queued { .. }), "{:?}", seen[0]);
    assert!(matches!(seen[1], PollStatus::Running { .. }), "{:?}", seen[1]);
    assert_eq!(urls(&seen[2]), ["https://cdn.example.com/image-1.jpg", "https://cdn.example.com/image-2.jpg"]);
    assert_eq!(urls(&seen[3]), ["https://cdn.example.com/video.mp4"]);
    assert!(matches!(seen[4], PollStatus::Failed(ProviderError::Failed(_))), "{:?}", seen[4]);
    assert!(matches!(seen[5], PollStatus::Failed(ProviderError::ContentPolicy(_))), "{:?}", seen[5]);
    assert!(matches!(seen[6], PollStatus::Failed(_)), "{:?}", seen[6]);
}

#[tokio::test]
async fn cancel_posts_to_the_request_cancel_endpoint() {
    // https://docs.higgsfield.ai/docs/api-reference/requests/cancel-a-queued-request.md (202, no body)
    let fake = start(|_| Reply { status: 202, content_type: "application/json".into(), body: vec![], headers: vec![] }).await;
    Higgsfield.cancel(&ctx(&fake, &[HOST]), &pictesa_core::RemoteHandle::new(REQUEST)).await.unwrap();
    let call = &fake.calls()[0];
    assert_eq!((call.method.as_str(), call.path.as_str()), ("POST", format!("/requests/{REQUEST}/cancel").as_str()));
    assert_eq!(call.header("authorization"), Some(expected_auth().as_str()));
}

#[tokio::test]
async fn documented_http_errors_map_to_their_meaning() {
    // https://docs.higgsfield.ai/docs/concepts/errors.md: FastAPI `detail` envelope; 401 missing or
    // invalid credentials; 403 insufficient balance; 422 validation; 429 after `Retry-After`; 503 later.
    // The 403 text is the official JS SDK's (higgsfield-ai/higgsfield-js src/errors.ts, which reads
    // every 403 as NotEnoughCreditsError).
    let model = spec("higgsfield-ai/soul/v2/standard");
    let req = request(&model, json!({}), vec![]);
    let cases = [
        (Reply::status(401, json!({ "detail": "Invalid credentials" })), "auth"),
        (Reply::status(403, json!({ "detail": "Not enough credits" })), "insufficient_credits"),
        (Reply::status(422, json!({ "detail": [{ "type": "missing", "loc": ["body", "prompt"], "msg": "Field required" }] })), "invalid_input"),
        (Reply::status(429, json!({ "detail": "account_queue_full" })).header("retry-after", "30"), "rate_limited"),
        (Reply::status(503, json!({ "detail": "admission_control_unavailable" })), "transient"),
    ];
    for (reply, kind) in cases {
        let reply = std::sync::Mutex::new(Some(reply));
        let fake = start(move |_| reply.lock().unwrap().take().unwrap_or_else(|| Reply::status(500, json!({})))).await;
        let e = Higgsfield.submit(&ctx(&fake, &[HOST]), &model, &req).await.unwrap_err();
        assert_eq!(e.kind(), kind, "{e:?}");
        if kind == "rate_limited" {
            assert_eq!(e.retry_after(), Some(std::time::Duration::from_secs(30)));
        }
    }
}
