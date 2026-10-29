//! OpenAI contract: the Images, Batch, Files and Models APIs' documented answers replayed through
//! the adapter, checking what it sends. Docs read 29.10.2026 (markdown via `<page>.md`).
mod common;

use base64::{engine::general_purpose::STANDARD, Engine as _};
use common::{start, Call, Fake, Reply, PNG_1X1};
use pictesa_core::{
    Ctx, GenerationRequest, InputRole, MediaInput, MediaKind, ModelSpec, Output, PollStatus,
    Provider, ProviderError, RemoteHandle, Submission,
};
use pictesa_providers::openai::OpenAI;
use serde_json::{json, Value};

const HOST: &str = "api.openai.com";

fn ctx(fake: &Fake) -> Ctx {
    fake.ctx(&[HOST], &[("api_key", "test-key")])
}

fn model(id: &str) -> ModelSpec {
    OpenAI.models().into_iter().find(|m| m.remote_id() == id).unwrap()
}

fn request(model: &str, params: Value, inputs: Vec<MediaInput>) -> GenerationRequest {
    GenerationRequest {
        job_id: "job-1".into(),
        model: model.into(),
        prompt: "a lighthouse at dawn".into(),
        negative_prompt: None,
        params: serde_json::from_value(params).unwrap(),
        inputs,
        num_outputs: 1,
    }
}

/// An input file the way the engine hands it over: a local path, no public URL. One file per
/// content, renamed into place so parallel tests never read a half-written file.
fn input(role: InputRole, bytes: &[u8]) -> MediaInput {
    let path = std::env::temp_dir().join(format!("pictesa-contract-{:08x}.png", crc32fast::hash(bytes)));
    let part = path.with_extension(format!("{}.part", ulid::Ulid::new()));
    std::fs::write(&part, bytes).unwrap();
    std::fs::rename(&part, &path).unwrap();
    MediaInput { role, kind: MediaKind::Image, mime: "image/png".into(), path, url: None, width: None, height: None }
}

fn png_data_uri() -> String {
    format!("data:image/png;base64,{}", STANDARD.encode(PNG_1X1))
}

// https://developers.openai.com/api/reference/resources/images/methods/generate (200 example;
// placeholders filled, `url` dropped: GPT image models "always return base64-encoded images").
fn images_response() -> Value {
    json!({
        "created": 1761696000,
        "background": "opaque",
        "data": [{ "b64_json": STANDARD.encode(PNG_1X1), "revised_prompt": "a lighthouse at dawn" }],
        "output_format": "png",
        "quality": "high",
        "size": "1536x864",
        "usage": {
            "input_tokens": 50, "input_tokens_details": { "image_tokens": 0, "text_tokens": 50 },
            "output_tokens": 4160, "total_tokens": 4210,
            "output_tokens_details": { "image_tokens": 4160, "text_tokens": 0 }
        }
    })
}

fn bearer(call: &Call) -> Option<&str> {
    call.header("authorization")
}

#[tokio::test]
async fn check_lists_models_with_the_key_as_a_bearer_token() {
    // https://developers.openai.com/api/reference/resources/models/methods/list ("Response example")
    let fake = start(|_| {
        Reply::json(json!({ "object": "list", "data": [
            { "id": "model-id-0", "object": "model", "created": 1686935002, "owned_by": "organization-owner", "shutdown_date": null }
        ]}))
    })
    .await;
    OpenAI.check(&ctx(&fake)).await.unwrap();
    let calls = fake.calls();
    assert_eq!((calls[0].method.as_str(), calls[0].path.as_str()), ("GET", "/v1/models"));
    assert_eq!(bearer(&calls[0]), Some("Bearer test-key"));
}

#[tokio::test]
async fn discovery_offers_current_gpt_image_models_and_the_catalog_has_no_shut_down_sora() {
    // https://developers.openai.com/api/reference/resources/models/methods/list (Model shape);
    // https://developers.openai.com/api/docs/deprecations: gpt-image-1 shut down 23.10.2026, and
    // the Videos API with sora-2 and sora-2-pro on 24.09.2026.
    let fake = start(|_| {
        Reply::json(json!({ "object": "list", "data": [
            { "id": "gpt-image-2.5-sunburst", "object": "model", "created": 1757289600, "owned_by": "system", "shutdown_date": null },
            { "id": "gpt-image-2.5-sunburst-2026-09-08", "object": "model", "created": 1757289600, "owned_by": "system", "shutdown_date": null },
            { "id": "gpt-image-1.5", "object": "model", "created": 1765843200, "owned_by": "system", "shutdown_date": "2026-12-01" },
            { "id": "gpt-image-1", "object": "model", "created": 1745366400, "owned_by": "system", "shutdown_date": "2026-10-23" },
            { "id": "gpt-6-sol", "object": "model", "created": 1767225600, "owned_by": "system", "shutdown_date": null }
        ]}))
    })
    .await;
    let ids: Vec<String> = OpenAI.discover(&ctx(&fake)).await.unwrap().into_iter().map(|m| m.id).collect();
    assert_eq!(ids, ["openai:gpt-image-2.5-sunburst", "openai:gpt-image-1.5"]);
    let video: Vec<String> = OpenAI.models().into_iter().filter(|m| m.output != MediaKind::Image).map(|m| m.id).collect();
    assert!(video.is_empty(), "the Videos API is shut down, yet the catalog offers {video:?}");
}

#[tokio::test]
async fn generation_posts_the_documented_json_and_decodes_the_base64_image() {
    let fake = start(|_| Reply::json(images_response())).await;
    let req = request("gpt-image-2.5-sunburst", json!({ "aspect_ratio": "16:9", "quality": "xhigh" }), vec![]);
    let Submission::Done(out) = OpenAI.submit(&ctx(&fake), &model("gpt-image-2.5-sunburst"), &req).await.unwrap() else {
        panic!("generations answer synchronously")
    };
    assert!(matches!(&out[..], [Output::Bytes { data, mime }] if data == PNG_1X1 && mime == "image/png"));
    let call = &fake.calls()[0];
    assert_eq!((call.method.as_str(), call.path.as_str()), ("POST", "/v1/images/generations"));
    assert_eq!(bearer(call), Some("Bearer test-key"));
    let body = call.json();
    // Required: model and prompt. `quality` xhigh is valid for the 2.5 models only.
    assert_eq!((body["model"].as_str(), body["prompt"].as_str()), (Some("gpt-image-2.5-sunburst"), Some("a lighthouse at dawn")));
    assert_eq!(body["n"], 1);
    assert_eq!(body["quality"], "xhigh");
    // Custom sizes: both edges divisible by 16, ratio within 1:3..3:1, at most 3840x2160.
    let size = body["size"].as_str().unwrap();
    let (w, h) = size.split_once('x').map(|(w, h)| (w.parse::<u32>().unwrap(), h.parse::<u32>().unwrap())).unwrap();
    assert!(w % 16 == 0 && h % 16 == 0 && w <= 3 * h && h <= 3 * w && w * h <= 3840 * 2160, "{size}");
    assert!(w > h, "16:9 is landscape: {size}");
    assert!(body.get("response_format").is_none(), "unsupported for GPT image models");
}

#[tokio::test]
async fn edit_sends_images_and_mask_as_image_url_objects() {
    // https://developers.openai.com/api/reference/resources/images/methods/edit: JSON body with
    // `images: [{image_url}]` (URL or base64 data URL) and `mask: {image_url}`.
    let fake = start(|_| Reply::json(images_response())).await;
    let mut mask_png = std::io::Cursor::new(Vec::new());
    image::GrayImage::from_pixel(1, 1, image::Luma([255])).write_to(&mut mask_png, image::ImageFormat::Png).unwrap();
    let req = request(
        "gpt-image-2.5-sunburst",
        json!({}),
        vec![input(InputRole::Reference, PNG_1X1), input(InputRole::Mask, mask_png.get_ref())],
    );
    let Submission::Done(out) = OpenAI.submit(&ctx(&fake), &model("gpt-image-2.5-sunburst"), &req).await.unwrap() else {
        panic!("edits answer synchronously")
    };
    assert_eq!(out.len(), 1);
    let call = &fake.calls()[0];
    assert_eq!((call.method.as_str(), call.path.as_str()), ("POST", "/v1/images/edits"));
    assert!(call.header("content-type").unwrap().starts_with("application/json"));
    let body = call.json();
    assert_eq!(body["images"], json!([{ "image_url": png_data_uri() }]));
    assert!(body["mask"]["image_url"].as_str().unwrap().starts_with("data:image/png;base64,"));
    assert!(body.get("image").is_none(), "the JSON body has no singular `image`");
}

/// The JSONL line inside the multipart upload to `/v1/files`.
fn uploaded_line(call: &Call) -> Value {
    let text = String::from_utf8_lossy(&call.body);
    let line = text.lines().find(|l| l.starts_with('{') && l.contains("custom_id")).expect("a JSONL request line");
    serde_json::from_str(line).unwrap()
}

#[tokio::test]
async fn batch_uploads_a_jsonl_file_creates_a_24h_batch_and_reads_its_output_file() {
    // https://developers.openai.com/api/docs/guides/batch: GPT Image 2.5 supports batch on
    // /v1/images/generations; input lines are {custom_id, method, url, body}; output lines are
    // {id, custom_id, response: {status_code, request_id, body}, error}.
    let output_line = json!({
        "id": "batch_req_123", "custom_id": "job-1",
        "response": { "status_code": 200, "request_id": "req_123", "body": images_response() },
        "error": null
    });
    let fake = start(move |call| match (call.method.as_str(), call.path.as_str()) {
        // https://developers.openai.com/api/reference/resources/files/methods/create
        ("POST", "/v1/files") => Reply::json(json!({
            "id": "file-abc123", "bytes": 120, "created_at": 1761696000, "filename": "pictesa-batch.jsonl",
            "object": "file", "purpose": "batch", "status": "uploaded"
        })),
        // https://developers.openai.com/api/reference/resources/batches/methods/create ("Response example")
        ("POST", "/v1/batches") => Reply::json(json!({
            "id": "batch_abc123", "object": "batch", "endpoint": "/v1/images/generations", "errors": null,
            "input_file_id": "file-abc123", "completion_window": "24h", "status": "validating",
            "output_file_id": null, "error_file_id": null, "created_at": 1711471533,
            "request_counts": { "total": 0, "completed": 0, "failed": 0 }
        })),
        // https://developers.openai.com/api/reference/resources/batches/methods/retrieve
        ("GET", "/v1/batches/batch_abc123") => Reply::json(json!({
            "id": "batch_abc123", "object": "batch", "endpoint": "/v1/images/generations", "errors": null,
            "input_file_id": "file-abc123", "completion_window": "24h", "status": "completed",
            "output_file_id": "file-cvaTdG", "error_file_id": null, "created_at": 1711471533,
            "completed_at": 1711493133, "request_counts": { "total": 1, "completed": 1, "failed": 0 }
        })),
        // https://developers.openai.com/api/reference/resources/files/methods/content
        ("GET", "/v1/files/file-cvaTdG/content") => Reply::bytes("application/octet-stream", format!("{output_line}\n").as_bytes()),
        _ => Reply::status(404, json!({ "error": { "message": "unexpected", "type": "invalid_request_error" } })),
    })
    .await;
    let ctx = ctx(&fake);
    let req = request("gpt-image-2.5-sunburst", json!({ "speed": "batch" }), vec![]);
    let Submission::Pending(handle) = OpenAI.submit(&ctx, &model("gpt-image-2.5-sunburst"), &req).await.unwrap() else {
        panic!("a batch is queued")
    };
    assert_eq!(handle.id, "batch_abc123");
    let calls = fake.calls();
    let upload = &calls[0];
    assert!(upload.header("content-type").unwrap().starts_with("multipart/form-data"));
    let form = String::from_utf8_lossy(&upload.body);
    assert!(form.contains("name=\"purpose\"\r\n\r\nbatch\r\n"), "purpose must be `batch`");
    let line = uploaded_line(upload);
    assert_eq!((line["custom_id"].as_str(), line["method"].as_str(), line["url"].as_str()), (Some("job-1"), Some("POST"), Some("/v1/images/generations")));
    assert_eq!(line["body"]["model"], "gpt-image-2.5-sunburst");
    assert_eq!(calls[1].json(), json!({ "input_file_id": "file-abc123", "endpoint": "/v1/images/generations", "completion_window": "24h" }));

    // The engine stores the handle and polls it after a restart.
    let handle: RemoteHandle = serde_json::from_str(&serde_json::to_string(&handle).unwrap()).unwrap();
    let PollStatus::Done(out) = OpenAI.poll(&ctx, &handle).await.unwrap() else { panic!("completed batch") };
    assert!(matches!(&out[..], [Output::Bytes { data, .. }] if data == PNG_1X1));
    assert!(fake.calls().iter().all(|c| bearer(c) == Some("Bearer test-key")));
}

/// Polls a stored batch handle against one documented Batch object (plus its files).
async fn poll_batch(batch: Value, files: Vec<(&'static str, String)>) -> PollStatus {
    let fake = start(move |call| {
        if call.path == "/v1/batches/batch_abc123" {
            return Reply::json(batch.clone());
        }
        match files.iter().find(|(id, _)| call.path == format!("/v1/files/{id}/content")) {
            Some((_, text)) => Reply::bytes("application/octet-stream", text.as_bytes()),
            None => Reply::status(404, json!({ "error": { "message": "unexpected" } })),
        }
    })
    .await;
    let handle = RemoteHandle::with("batch_abc123", json!({ "batch": true, "custom_id": "job-1", "input_file_id": "file-abc123" }));
    OpenAI.poll(&ctx(&fake), &handle).await.unwrap()
}

#[tokio::test]
async fn batch_states_map_to_running_or_failed() {
    // https://developers.openai.com/api/docs/guides/batch: status table, and expired requests
    // written to the error file as {"response": null, "error": {"code": "batch_expired", ...}}.
    let batch = |status: &str| json!({ "id": "batch_abc123", "object": "batch", "status": status, "output_file_id": null, "error_file_id": null, "errors": null });
    for status in ["validating", "in_progress", "finalizing", "cancelling"] {
        assert!(matches!(poll_batch(batch(status), vec![]).await, PollStatus::Running { .. }), "{status}");
    }
    let mut failed = batch("failed");
    failed["errors"] = json!({ "object": "list", "data": [{ "code": "invalid_request", "line": 1, "message": "Model gpt-image-2.5-sunburst does not support /v1/images/edits", "param": "url" }] });
    let PollStatus::Failed(e) = poll_batch(failed, vec![]).await else { panic!("failed batch") };
    assert!(e.to_string().contains("does not support"), "{e}");

    let mut expired = batch("expired");
    expired["error_file_id"] = json!("file-err1");
    let line = r#"{"id": "batch_req_123", "custom_id": "job-1", "response": null, "error": {"code": "batch_expired", "message": "This request could not be executed before the completion window expired."}}"#;
    let PollStatus::Failed(e) = poll_batch(expired, vec![("file-err1", format!("{line}\n"))]).await else { panic!("expired batch") };
    assert!(e.to_string().contains("completion window expired"), "{e}");
}

#[tokio::test]
async fn cancel_posts_to_the_batch_cancel_endpoint() {
    // https://developers.openai.com/api/reference/resources/batches/methods/cancel
    let fake = start(|_| Reply::json(json!({ "id": "batch_abc123", "object": "batch", "status": "cancelling" }))).await;
    let handle = RemoteHandle::with("batch_abc123", json!({ "batch": true, "custom_id": "job-1" }));
    OpenAI.cancel(&ctx(&fake), &handle).await.unwrap();
    let call = &fake.calls()[0];
    assert_eq!((call.method.as_str(), call.path.as_str()), ("POST", "/v1/batches/batch_abc123/cancel"));
}

#[tokio::test]
async fn http_errors_map_to_auth_credits_rate_limits_and_transient_failures() {
    // https://developers.openai.com/api/docs/guides/error-codes: 401 bad key; 429 with code
    // credit_balance_exhausted (type insufficient_quota) is billing, not a rate limit; 429 rate
    // limit with Retry-After; 503 server_is_overloaded.
    let cases: Vec<(Reply, fn(&ProviderError) -> bool)> = vec![
        (
            Reply::status(401, json!({ "error": { "message": "Incorrect API key provided: test-key.", "type": "invalid_request_error", "param": null, "code": "invalid_api_key" } })),
            |e| matches!(e, ProviderError::Auth(_)),
        ),
        (
            Reply::status(429, json!({ "error": { "message": "Your organization has no prepaid credits remaining.", "type": "insufficient_quota", "param": null, "code": "credit_balance_exhausted" } })),
            |e| matches!(e, ProviderError::InsufficientCredits(_)),
        ),
        (
            Reply::status(429, json!({ "error": { "message": "Rate limit reached for requests", "type": "rate_limit_error", "param": null, "code": "rate_limit_exceeded" } })).header("retry-after", "2"),
            |e| matches!(e, ProviderError::RateLimited { retry_after_ms: Some(2000) }),
        ),
        (
            Reply::status(503, json!({ "error": { "message": "The model is temporarily overloaded.", "type": "service_unavailable_error", "param": null, "code": "server_is_overloaded" } })),
            |e| matches!(e, ProviderError::Transient(_)),
        ),
    ];
    for (reply, expected) in cases {
        let reply = std::sync::Mutex::new(Some(reply));
        let fake = start(move |_| reply.lock().unwrap().take().unwrap()).await;
        let req = request("gpt-image-2", json!({}), vec![]);
        let e = OpenAI.submit(&ctx(&fake), &model("gpt-image-2"), &req).await.unwrap_err();
        assert!(expected(&e), "{e:?}");
    }
}
