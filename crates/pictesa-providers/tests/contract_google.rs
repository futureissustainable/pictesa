//! Gemini API contract: the documented answers of models.list, generateContent and the Batch API
//! replayed through the adapter, checking what it sends. Docs read 29.10.2026 (markdown via
//! `<page>.md.txt`). Veo is not covered: every Veo model in the Gemini API was shut down by
//! 22.10.2026 (https://ai.google.dev/gemini-api/docs/deprecations), so no video API is documented.
mod common;

use base64::{engine::general_purpose::STANDARD, Engine as _};
use common::{start, Fake, Reply, PNG_1X1};
use pictesa_core::{
    Ctx, GenerationRequest, InputRole, MediaInput, MediaKind, ModelSpec, Output, PollStatus,
    Provider, ProviderError, RemoteHandle, Submission,
};
use pictesa_providers::google::Google;
use serde_json::{json, Value};

const HOST: &str = "generativelanguage.googleapis.com";

fn ctx(fake: &Fake) -> Ctx {
    fake.ctx(&[HOST], &[("api_key", "test-key")])
}

fn model(id: &str) -> ModelSpec {
    Google.models().into_iter().find(|m| m.remote_id() == id).unwrap()
}

fn request(params: Value, inputs: Vec<MediaInput>) -> GenerationRequest {
    GenerationRequest {
        job_id: "job-1".into(),
        model: "gemini-3.1-flash-image".into(),
        prompt: "a nano banana dish in a fancy restaurant".into(),
        negative_prompt: None,
        params: serde_json::from_value(params).unwrap(),
        inputs,
        num_outputs: 1,
    }
}

/// An input file the way the engine hands it over: a local path, no public URL.
fn input(role: InputRole) -> MediaInput {
    let path = std::env::temp_dir().join(format!("pictesa-contract-{:08x}.png", crc32fast::hash(PNG_1X1)));
    let part = path.with_extension(format!("{}.part", ulid::Ulid::new()));
    std::fs::write(&part, PNG_1X1).unwrap();
    std::fs::rename(&part, &path).unwrap();
    MediaInput { role, kind: MediaKind::Image, mime: "image/png".into(), path, url: None, width: None, height: None }
}

// https://ai.google.dev/api/generate-content#GenerateContentResponse with a Part carrying
// `inlineData` (https://ai.google.dev/api/generate-content#Blob: mimeType, base64 data).
fn image_response() -> Value {
    json!({
        "candidates": [{
            "content": { "role": "model", "parts": [
                { "text": "Here is the dish." },
                { "inlineData": { "mimeType": "image/png", "data": STANDARD.encode(PNG_1X1) } }
            ]},
            "finishReason": "STOP",
            "index": 0
        }],
        "usageMetadata": { "promptTokenCount": 12, "candidatesTokenCount": 1290, "totalTokenCount": 1302 },
        "modelVersion": "gemini-3.1-flash-image",
        "responseId": "resp-1"
    })
}

#[tokio::test]
async fn check_sends_the_key_in_the_x_goog_api_key_header_never_in_the_url() {
    // https://ai.google.dev/api/models#method:-models.list (ListModelsResponse: models[], nextPageToken)
    let fake = start(|_| Reply::json(json!({ "models": [{ "name": "models/gemini-3.8-flash", "displayName": "Gemini 3.8 Flash", "supportedGenerationMethods": ["generateContent"] }] }))).await;
    Google.check(&ctx(&fake)).await.unwrap();
    let call = &fake.calls()[0];
    assert_eq!(call.method, "GET");
    assert!(call.path.starts_with("/v1beta/models"), "{}", call.path);
    assert_eq!(call.header("x-goog-api-key"), Some("test-key"));
    assert!(!call.path.contains("key="), "the key must not travel in the URL: {}", call.path);
}

#[tokio::test]
async fn discovery_follows_next_page_token_and_finds_nano_banana_models() {
    // https://ai.google.dev/api/models#method:-models.list: 50 models a page by default, at most
    // 1000; "If this field [nextPageToken] is omitted, there are no more pages." Model resource
    // fields from https://ai.google.dev/api/models#Model; gemini-nano-banana-2.1 (no "image" in its
    // id) from https://ai.google.dev/gemini-api/docs/models/gemini-nano-banana-2.1.
    let fake = start(|call| {
        if call.path.contains("pageToken=page-2") {
            Reply::json(json!({ "models": [
                { "name": "models/gemini-nano-banana-2.1", "baseModelId": "gemini-nano-banana-2.1", "version": "2.1", "displayName": "Nano Banana 2.1",
                  "supportedGenerationMethods": ["generateContent", "countTokens", "batchGenerateContent"] }
            ]}))
        } else {
            Reply::json(json!({ "models": [
                { "name": "models/gemini-3-pro-image", "baseModelId": "gemini-3-pro-image", "version": "3", "displayName": "Nano Banana Pro",
                  "supportedGenerationMethods": ["generateContent", "countTokens", "batchGenerateContent"] },
                { "name": "models/gemini-3.8-flash", "baseModelId": "gemini-3.8-flash", "version": "3.8", "displayName": "Gemini 3.8 Flash",
                  "supportedGenerationMethods": ["generateContent", "countTokens"] },
                { "name": "models/gemini-embedding-001", "baseModelId": "gemini-embedding-001", "version": "001", "displayName": "Gemini Embedding",
                  "supportedGenerationMethods": ["embedContent"] }
            ], "nextPageToken": "page-2" }))
        }
    })
    .await;
    let found: Vec<(String, MediaKind)> = Google.discover(&ctx(&fake)).await.unwrap().into_iter().map(|m| (m.id, m.output)).collect();
    assert_eq!(found, [("google:gemini-3-pro-image".to_string(), MediaKind::Image), ("google:gemini-nano-banana-2.1".to_string(), MediaKind::Image)]);
    let calls = fake.calls();
    assert_eq!(calls.len(), 2, "one request per page");
    assert!(calls.iter().all(|c| c.header("x-goog-api-key") == Some("test-key")));
}

#[tokio::test]
async fn generation_sends_image_config_and_decodes_inline_data() {
    // https://ai.google.dev/api/generate-content#method:-models.generatecontent (POST
    // v1beta/{model=models/*}:generateContent); GenerationConfig.responseModalities and
    // imageConfig {aspectRatio, imageSize: 512|1K|2K|4K} (https://ai.google.dev/api/generate-content#ImageConfig).
    let fake = start(|_| Reply::json(image_response())).await;
    let req = request(json!({ "aspect_ratio": "16:9", "resolution": "2K" }), vec![]);
    let Submission::Done(out) = Google.submit(&ctx(&fake), &model("gemini-3.1-flash-image"), &req).await.unwrap() else {
        panic!("generateContent answers synchronously")
    };
    assert!(matches!(&out[..], [Output::Bytes { data, mime }] if data == PNG_1X1 && mime == "image/png"));
    let call = &fake.calls()[0];
    assert_eq!((call.method.as_str(), call.path.as_str()), ("POST", "/v1beta/models/gemini-3.1-flash-image:generateContent"));
    assert_eq!(call.header("x-goog-api-key"), Some("test-key"));
    let body = call.json();
    assert_eq!(body["contents"][0]["parts"], json!([{ "text": "a nano banana dish in a fancy restaurant" }]));
    assert!(body["generationConfig"]["responseModalities"].as_array().unwrap().contains(&json!("IMAGE")));
    assert_eq!(body["generationConfig"]["imageConfig"], json!({ "aspectRatio": "16:9", "imageSize": "2K" }));
}

#[tokio::test]
async fn edit_sends_reference_images_as_inline_data_parts() {
    // https://ai.google.dev/api/generate-content#Part (`inlineData` Blob: mimeType + base64 data)
    let fake = start(|_| Reply::json(image_response())).await;
    let req = request(json!({}), vec![input(InputRole::Reference), input(InputRole::Reference)]);
    Google.submit(&ctx(&fake), &model("gemini-3.1-flash-image"), &req).await.unwrap();
    let parts = fake.calls()[0].json()["contents"][0]["parts"].clone();
    let blob = json!({ "inlineData": { "mimeType": "image/png", "data": STANDARD.encode(PNG_1X1) } });
    assert_eq!(parts, json!([{ "text": "a nano banana dish in a fancy restaurant" }, blob, blob]));
}

#[tokio::test]
async fn a_blocked_prompt_or_image_reads_as_content_policy() {
    // https://ai.google.dev/api/generate-content#PromptFeedback (blockReason, no candidates) and
    // #FinishReason (IMAGE_SAFETY, IMAGE_PROHIBITED_CONTENT, NO_IMAGE on the candidate).
    let cases = [
        (json!({ "promptFeedback": { "blockReason": "PROHIBITED_CONTENT" } }), true),
        (json!({ "candidates": [{ "content": { "role": "model", "parts": [] }, "finishReason": "IMAGE_SAFETY", "index": 0 }] }), true),
        (json!({ "candidates": [{ "finishReason": "IMAGE_PROHIBITED_CONTENT", "index": 0 }] }), true),
        (json!({ "candidates": [{ "content": { "role": "model", "parts": [{ "text": "I can only describe it." }] }, "finishReason": "NO_IMAGE", "index": 0 }] }), false),
    ];
    for (answer, blocked) in cases {
        let reply = answer.clone();
        let fake = start(move |_| Reply::json(reply.clone())).await;
        let e = Google.submit(&ctx(&fake), &model("gemini-3.1-flash-image"), &request(json!({}), vec![])).await.unwrap_err();
        assert_eq!(matches!(e, ProviderError::ContentPolicy(_)), blocked, "{answer} -> {e:?}");
        assert!(!e.is_retryable(), "{e:?}");
    }
}

#[tokio::test]
async fn batch_enqueues_inlined_requests_and_reads_inlined_responses() {
    // https://ai.google.dev/api/batch-api#method:-models.batchgeneratecontent: body
    // {batch: {displayName, inputConfig: {requests: {requests: [{request, metadata}]}}}}, answer an
    // Operation named batches/{id}; batches.get returns the Operation; on success the REST guide
    // reads `.response.inlinedResponses` (https://ai.google.dev/gemini-api/docs/batch-api).
    let done = json!({
        "name": "batches/123456",
        "metadata": { "model": "models/gemini-3.1-flash-image", "displayName": "pictesa-job-1", "state": "BATCH_STATE_SUCCEEDED",
                      "batchStats": { "requestCount": "1", "successfulRequestCount": "1" } },
        "done": true,
        "response": { "inlinedResponses": { "inlinedResponses": [{ "metadata": { "key": "job-1" }, "response": image_response() }] } }
    });
    let polls = std::sync::atomic::AtomicUsize::new(0);
    let fake = start(move |call| match (call.method.as_str(), call.path.as_str()) {
        ("POST", "/v1beta/models/gemini-3.1-flash-image:batchGenerateContent") => Reply::json(json!({
            "name": "batches/123456",
            "metadata": { "model": "models/gemini-3.1-flash-image", "displayName": "pictesa-job-1", "state": "BATCH_STATE_PENDING" }
        })),
        ("GET", "/v1beta/batches/123456") if polls.fetch_add(1, std::sync::atomic::Ordering::SeqCst) == 0 => Reply::json(json!({
            "name": "batches/123456", "metadata": { "state": "BATCH_STATE_RUNNING", "batchStats": { "requestCount": "1", "pendingRequestCount": "1" } }, "done": false
        })),
        ("GET", "/v1beta/batches/123456") => Reply::json(done.clone()),
        _ => Reply::status(404, json!({ "error": { "code": 404, "message": "unexpected", "status": "NOT_FOUND" } })),
    })
    .await;
    let ctx = ctx(&fake);
    let req = request(json!({ "speed": "batch", "resolution": "2K" }), vec![]);
    let Submission::Pending(handle) = Google.submit(&ctx, &model("gemini-3.1-flash-image"), &req).await.unwrap() else {
        panic!("a batch is queued")
    };
    let submitted = fake.calls()[0].json();
    let batch = &submitted["batch"];
    assert!(batch["displayName"].is_string(), "displayName is required");
    let inlined = &batch["inputConfig"]["requests"]["requests"][0];
    assert_eq!(inlined["metadata"]["key"], "job-1");
    assert_eq!(inlined["request"]["generationConfig"]["imageConfig"]["imageSize"], "2K");
    assert_eq!(inlined["request"]["contents"][0]["parts"][0]["text"], "a nano banana dish in a fancy restaurant");

    let handle: RemoteHandle = serde_json::from_str(&serde_json::to_string(&handle).unwrap()).unwrap();
    assert_eq!(handle.id, "batches/123456");
    assert!(matches!(Google.poll(&ctx, &handle).await.unwrap(), PollStatus::Running { .. } | PollStatus::Queued { .. }));
    let PollStatus::Done(out) = Google.poll(&ctx, &handle).await.unwrap() else { panic!("succeeded batch") };
    assert!(matches!(&out[..], [Output::Bytes { data, .. }] if data == PNG_1X1));
    assert!(fake.calls().iter().all(|c| c.header("x-goog-api-key") == Some("test-key")));
}

#[tokio::test]
async fn failed_expired_or_cancelled_batches_end_the_job() {
    // https://ai.google.dev/api/batch-api#BatchState and #Operation: when done, either `error`
    // (google.rpc.Status) or `response` is set; one request's failure is InlinedResponse.error.
    let cases = [
        json!({ "name": "batches/123456", "done": true, "metadata": { "state": "BATCH_STATE_FAILED" }, "error": { "code": 3, "message": "Request payload is invalid." } }),
        json!({ "name": "batches/123456", "done": true, "metadata": { "state": "BATCH_STATE_EXPIRED" } }),
        json!({ "name": "batches/123456", "done": true, "metadata": { "state": "BATCH_STATE_CANCELLED" } }),
        json!({ "name": "batches/123456", "done": true, "metadata": { "state": "BATCH_STATE_SUCCEEDED" },
                "response": { "inlinedResponses": { "inlinedResponses": [{ "metadata": { "key": "job-1" }, "error": { "code": 3, "message": "Unsupported image size." } }] } } }),
    ];
    for answer in cases {
        let reply = answer.clone();
        let fake = start(move |_| Reply::json(reply.clone())).await;
        let handle = RemoteHandle::with("batches/123456", json!({ "batch": true }));
        assert!(matches!(Google.poll(&ctx(&fake), &handle).await.unwrap(), PollStatus::Failed(_)), "{answer}");
    }
}

#[tokio::test]
async fn cancel_posts_to_the_batch_cancel_method() {
    // https://ai.google.dev/api/batch-api#method:-batches.cancel ("the response body is an empty JSON object")
    let fake = start(|_| Reply::json(json!({}))).await;
    Google.cancel(&ctx(&fake), &RemoteHandle::with("batches/123456", json!({ "batch": true }))).await.unwrap();
    let call = &fake.calls()[0];
    assert_eq!((call.method.as_str(), call.path.as_str()), ("POST", "/v1beta/batches/123456:cancel"));
    assert_eq!(call.header("x-goog-api-key"), Some("test-key"));
}

#[tokio::test]
async fn http_errors_map_to_auth_credits_rate_limits_and_transient_failures() {
    // Statuses: https://ai.google.dev/gemini-api/docs/api-errors (401 bad key, 402 Prepay credit
    // balance depleted, 403 permission, 429 rate limit or quota, 503 overloaded) and
    // https://ai.google.dev/gemini-api/docs/billing ("Requests then fail with an HTTP 402").
    // Body: {"error": google.rpc.Status} (https://ai.google.dev/api/files#v1beta.Status).
    let cases: Vec<(u16, &str, fn(&ProviderError) -> bool)> = vec![
        (401, "UNAUTHENTICATED", |e| matches!(e, ProviderError::Auth(_))),
        (403, "PERMISSION_DENIED", |e| matches!(e, ProviderError::Auth(_))),
        (402, "PAYMENT_REQUIRED", |e| matches!(e, ProviderError::InsufficientCredits(_))),
        (429, "RESOURCE_EXHAUSTED", |e| matches!(e, ProviderError::RateLimited { .. })),
        (503, "UNAVAILABLE", |e| matches!(e, ProviderError::Transient(_))),
    ];
    for (status, name, expected) in cases {
        let fake = start(move |_| Reply::status(status, json!({ "error": { "code": status, "message": format!("{name} from the stand-in"), "status": name } }))).await;
        let e = Google.submit(&ctx(&fake), &model("gemini-3.1-flash-image"), &request(json!({}), vec![])).await.unwrap_err();
        assert!(expected(&e), "{status}: {e:?}");
    }
}
