//! Runway contract: the documented answers of api.dev.runwayml.com replayed through the adapter,
//! and what the adapter sends checked against the published API.
//! Docs: https://docs.dev.runwayml.com/llms.txt, /api.md and /ai-context.md (read 29.10.2026).

mod common;

use std::path::PathBuf;

use common::{start, Reply, PNG_1X1};
use pictesa_core::{GenerationRequest, InputRole, MediaInput, MediaKind, ModelSpec, Output, PollStatus, Provider, ProviderError, Submission};
use pictesa_providers::runway::Runway;
use serde_json::{json, Value};

const HOST: &str = "api.dev.runwayml.com";
// The task id in Runway's own examples (https://docs.dev.runwayml.com/_llms-txt/core-api.txt, "Using the API").
const TASK: &str = "17f20503-6c24-4c16-946b-35dbbce2af2f";

fn spec(id: &str) -> ModelSpec {
    Runway.models().into_iter().find(|m| m.remote_id() == id).unwrap()
}

/// A request as the engine hands it over: params checked and filled with the model's defaults.
fn request(model: &ModelSpec, prompt: &str, params: Value, inputs: Vec<MediaInput>) -> GenerationRequest {
    let mut p = params.as_object().cloned().unwrap_or_default();
    for spec in &model.params {
        if let Some(d) = &spec.default {
            p.entry(spec.key.clone()).or_insert(d.clone());
        }
    }
    GenerationRequest { job_id: "job-1".into(), model: model.remote_id().into(), prompt: prompt.into(), negative_prompt: None, params: p, inputs, num_outputs: 1 }
}

fn input(role: InputRole, kind: MediaKind, mime: &str, path: PathBuf, url: Option<&str>) -> MediaInput {
    MediaInput { role, kind, mime: mime.into(), path, url: url.map(String::from), width: None, height: None }
}

/// Writes a scratch input file and returns its path.
fn scratch_file(name: &str, data: &[u8]) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("pictesa-contract-runway-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join(name);
    std::fs::write(&path, data).unwrap();
    path
}

/// Every Runway request carries the key as a bearer token and the required version header.
fn assert_runway_headers(call: &common::Call) {
    assert_eq!(call.header("authorization"), Some("Bearer test-key"), "{}", call.path);
    assert_eq!(call.header("x-runway-version"), Some("2024-11-06"), "{}", call.path);
}

/// Submits `req` against a stand-in that accepts every task, and returns what was posted.
async fn submit(model: &ModelSpec, req: &GenerationRequest) -> common::Call {
    // https://docs.dev.runwayml.com/api.md (POST generation endpoints, "Response 200": id, estimatedCost)
    let fake = start(|_| Reply::json(json!({ "id": TASK, "estimatedCost": { "credits": 25 } }))).await;
    let Submission::Pending(handle) = Runway.submit(&fake.ctx(&[HOST], &[("api_key", "test-key")]), model, req).await.unwrap() else { panic!("Runway queues every task") };
    assert_eq!(handle.id, TASK);
    let call = fake.calls().pop().unwrap();
    assert_runway_headers(&call);
    call
}

#[tokio::test]
async fn check_reads_the_organization_with_key_and_version_headers() {
    // https://docs.dev.runwayml.com/api.md (GET /v1/organization, "Response 200")
    let fake = start(|_| Reply::json(json!({ "tier": { "maxMonthlyCreditSpend": 1000, "models": {} }, "creditBalance": 500, "usage": { "models": {} } }))).await;
    Runway.check(&fake.ctx(&[HOST], &[("api_key", "test-key")])).await.unwrap();
    let call = &fake.calls()[0];
    assert_eq!((call.method.as_str(), call.path.as_str()), ("GET", "/v1/organization"));
    assert_runway_headers(call);
}

#[tokio::test]
async fn gen45_without_a_start_frame_uses_the_text_to_video_endpoint() {
    // https://docs.dev.runwayml.com/api.md: POST /v1/text_to_video, model `gen4.5`: promptText, ratio
    // (`1280:720` | `720:1280`) and duration (2-10) required. POST /v1/image_to_video requires promptImage.
    let model = spec("gen4.5");
    let call = submit(&model, &request(&model, "A timelapse on a sunny day", json!({ "aspect_ratio": "9:16" }), vec![])).await;
    assert_eq!((call.method.as_str(), call.path.as_str()), ("POST", "/v1/text_to_video"));
    assert_eq!(call.json(), json!({ "model": "gen4.5", "promptText": "A timelapse on a sunny day", "ratio": "720:1280", "duration": 5 }));
}

#[tokio::test]
async fn image_to_video_sends_the_start_frame_as_prompt_image() {
    // https://docs.dev.runwayml.com/api.md (POST /v1/image_to_video, models `gen4.5` and `gen4_turbo`:
    // promptImage is "A HTTPS URL, Runway or data URI"; ratio `1280:720` | `960:960` | ...).
    let model = spec("gen4.5");
    let hosted = input(InputRole::StartFrame, MediaKind::Image, "image/jpeg", "unused.jpg".into(), Some("https://example.com/frame.jpg"));
    let call = submit(&model, &request(&model, "move", json!({ "aspect_ratio": "1:1", "duration": 8 }), vec![hosted])).await;
    assert_eq!(call.path, "/v1/image_to_video");
    assert_eq!(call.json(), json!({ "model": "gen4.5", "promptText": "move", "ratio": "960:960", "duration": 8, "promptImage": "https://example.com/frame.jpg" }));

    // A small local file goes inline as a data URI (https://docs.dev.runwayml.com/assets/inputs.md:
    // "Data URIs must use the base64 extension", 5 MB limit for the encoded URI).
    let turbo = spec("gen4_turbo");
    let local = input(InputRole::StartFrame, MediaKind::Image, "image/png", scratch_file("small.png", PNG_1X1), None);
    let call = submit(&turbo, &request(&turbo, "move", json!({}), vec![local])).await;
    assert_eq!(call.path, "/v1/image_to_video");
    let image = call.json()["promptImage"].as_str().unwrap().to_string();
    assert!(image.starts_with("data:image/png;base64,iVBOR"), "{image}");
}

#[tokio::test]
async fn a_large_local_frame_goes_through_an_ephemeral_upload() {
    // https://docs.dev.runwayml.com/_llms-txt/core-api.txt ("Using ephemeral uploads without an SDK"):
    // POST /v1/uploads {filename, type: "ephemeral"} answers {uploadUrl, fields, runwayUri}; POST the
    // fields plus the file as `file` in a multipart form to uploadUrl; use runwayUri in the generation.
    // The docs elide uploadUrl ("https://..."); this stand-in uses a storage host of its own.
    let fake = start(|call| match call.path.as_str() {
        "/v1/uploads" => Reply::json(json!({ "uploadUrl": "https://runway-uploads.example.com/", "fields": { "key": "uploads/frame.png", "policy": "test-policy" }, "runwayUri": "runway://uploads/frame.png" })),
        "/" => Reply::status(204, Value::Null),
        _ => Reply::json(json!({ "id": TASK })),
    })
    .await;
    let model = spec("gen4_turbo");
    let big = vec![7u8; 3 * 1024 * 1024 + 1];
    let path = scratch_file("frame.png", &big);
    let local = input(InputRole::StartFrame, MediaKind::Image, "image/png", path.clone(), None);
    let ctx = fake.ctx(&[HOST, "runway-uploads.example.com"], &[("api_key", "test-key")]);
    Runway.submit(&ctx, &model, &request(&model, "move", json!({}), vec![local])).await.unwrap();
    std::fs::remove_file(path).unwrap();

    let calls = fake.calls();
    assert_eq!(calls.len(), 3);
    assert_eq!((calls[0].method.as_str(), calls[0].path.as_str()), ("POST", "/v1/uploads"));
    assert_runway_headers(&calls[0]);
    assert_eq!(calls[0].json(), json!({ "filename": "frame.png", "type": "ephemeral" }));

    let upload = &calls[1];
    assert_eq!((upload.method.as_str(), upload.host.as_str()), ("POST", "runway-uploads.example.com"));
    assert!(upload.header("content-type").is_some_and(|t| t.starts_with("multipart/form-data")));
    let form = String::from_utf8_lossy(&upload.body);
    for part in ["name=\"key\"", "uploads/frame.png", "name=\"policy\"", "test-policy", "name=\"file\""] {
        assert!(form.contains(part), "upload form misses {part}");
    }
    assert!(upload.body.len() > big.len(), "the file travels in the form");
    assert_eq!(upload.header("authorization"), None, "the API key must not reach the storage host");

    assert_eq!(calls[2].path, "/v1/image_to_video");
    assert_eq!(calls[2].json()["promptImage"], "runway://uploads/frame.png");
}

#[tokio::test]
async fn gen4_image_sends_tagged_reference_images() {
    // https://docs.dev.runwayml.com/api.md (POST /v1/text_to_image, model `gen4_image`): ratio from the
    // listed sizes; referenceImages[].uri and .tag, the tag "3-16 characters, start with a letter, and
    // use only letters, digits, and underscores".
    let model = spec("gen4_image");
    let refs = ["https://example.com/a.jpg", "https://example.com/b.jpg"].map(|u| input(InputRole::Reference, MediaKind::Image, "image/jpeg", "unused.jpg".into(), Some(u)));
    let call = submit(&model, &request(&model, "@ref0 meets @ref1", json!({ "seed": 42 }), refs.to_vec())).await;
    assert_eq!(call.path, "/v1/text_to_image");
    let body = call.json();
    assert_eq!((body["model"].as_str(), body["promptText"].as_str(), body["ratio"].as_str(), body["seed"].as_u64()), (Some("gen4_image"), Some("@ref0 meets @ref1"), Some("1360:768"), Some(42)));
    let images = body["referenceImages"].as_array().unwrap();
    assert_eq!(images.iter().map(|i| i["uri"].as_str().unwrap()).collect::<Vec<_>>(), ["https://example.com/a.jpg", "https://example.com/b.jpg"]);
    for tag in images.iter().map(|i| i["tag"].as_str().unwrap()) {
        let ok = (3..=16).contains(&tag.len()) && tag.starts_with(|c: char| c.is_ascii_alphabetic()) && tag.chars().all(|c| c.is_ascii_alphanumeric() || c == '_');
        assert!(ok, "invalid Runway reference tag {tag}");
    }
}

#[tokio::test]
async fn video_upscale_sends_the_source_video_without_a_prompt() {
    // https://docs.dev.runwayml.com/api.md (POST /v1/video_upscale, model `magnific_video_upscaler_creative`:
    // videoUri required, resolution `720p` | `1k` | `2k` | `4k`; no promptText field).
    let model = spec("magnific_video_upscaler_creative");
    let video = input(InputRole::Video, MediaKind::Video, "video/mp4", "unused.mp4".into(), Some("https://example.com/clip.mp4"));
    let call = submit(&model, &request(&model, "", json!({ "resolution": "4k" }), vec![video])).await;
    assert_eq!(call.path, "/v1/video_upscale");
    assert_eq!(call.json(), json!({ "model": "magnific_video_upscaler_creative", "videoUri": "https://example.com/clip.mp4", "resolution": "4k" }));
}

#[tokio::test]
async fn every_documented_task_status_maps_to_a_poll_status() {
    // https://docs.dev.runwayml.com/api.md (GET /v1/tasks/{id}): status PENDING, THROTTLED, CANCELLED,
    // RUNNING, FAILED, SUCCEEDED; `output` on success, `failure` and `failureCode` on failure.
    // Failure codes: https://docs.dev.runwayml.com/errors/task-failures.md
    let base = json!({ "id": TASK, "createdAt": "2024-06-27T19:49:32.334Z" });
    let with = |extra: Value| {
        let mut v = base.clone();
        v.as_object_mut().unwrap().extend(extra.as_object().cloned().unwrap());
        v
    };
    let cases = [
        with(json!({ "status": "PENDING" })),
        with(json!({ "status": "THROTTLED" })),
        with(json!({ "status": "RUNNING", "progress": 0.4 })),
        // https://docs.dev.runwayml.com/_llms-txt/core-api.txt ("API Output Formats")
        with(json!({ "status": "SUCCEEDED", "output": ["https://dnznrvs05pmza.cloudfront.net/output.mp4?_jwt=..."] })),
        with(json!({ "status": "FAILED", "failureCode": "SAFETY.INPUT.TEXT" })),
        with(json!({ "status": "FAILED", "failureCode": "INTERNAL.BAD_OUTPUT.01" })),
        with(json!({ "status": "CANCELLED" })),
    ];
    let mut seen = Vec::new();
    for task in cases {
        let fake = start(move |_| Reply::json(task.clone())).await;
        let status = Runway.poll(&fake.ctx(&[HOST], &[("api_key", "test-key")]), &pictesa_core::RemoteHandle::new(TASK)).await.unwrap();
        let call = &fake.calls()[0];
        assert_eq!((call.method.as_str(), call.path.as_str()), ("GET", format!("/v1/tasks/{TASK}").as_str()));
        assert_runway_headers(call);
        seen.push(status);
    }
    assert!(matches!(seen[0], PollStatus::Queued { .. }) && matches!(seen[1], PollStatus::Queued { .. }), "{seen:?}");
    assert!(matches!(seen[2], PollStatus::Running { .. }), "{:?}", seen[2]);
    assert!(matches!(&seen[3], PollStatus::Done(o) if matches!(o.as_slice(), [Output::Url { url, .. }] if url.ends_with("output.mp4?_jwt=..."))), "{:?}", seen[3]);
    assert!(matches!(seen[4], PollStatus::Failed(ProviderError::ContentPolicy(_))), "{:?}", seen[4]);
    assert!(matches!(seen[5], PollStatus::Failed(ProviderError::Failed(_))), "{:?}", seen[5]);
    assert!(matches!(seen[6], PollStatus::Failed(_)), "{:?}", seen[6]);
}

#[tokio::test]
async fn cancel_deletes_the_task() {
    // https://docs.dev.runwayml.com/api.md (DELETE /v1/tasks/{id}: "Tasks that are running, pending, or
    // throttled can be canceled by invoking this method").
    let fake = start(|_| Reply::status(204, Value::Null)).await;
    Runway.cancel(&fake.ctx(&[HOST], &[("api_key", "test-key")]), &pictesa_core::RemoteHandle::new(TASK)).await.unwrap();
    let call = &fake.calls()[0];
    assert_eq!((call.method.as_str(), call.path.as_str()), ("DELETE", format!("/v1/tasks/{TASK}").as_str()));
    assert_runway_headers(call);
}

#[tokio::test]
async fn documented_http_errors_map_to_their_meaning() {
    // https://docs.dev.runwayml.com/errors/errors.md: 400 carries a JSON `error` member; 401 invalid key;
    // 429 too many requests (may retry); 502/503 shedding load, 504 overloaded (may retry).
    let model = spec("gen4.5");
    let req = request(&model, "move", json!({}), vec![]);
    for (status, check) in [
        (400u16, (|e: &ProviderError| matches!(e, ProviderError::InvalidInput(_))) as fn(&ProviderError) -> bool),
        (401, |e| matches!(e, ProviderError::Auth(_))),
        (429, |e| matches!(e, ProviderError::RateLimited { .. })),
        (502, |e| matches!(e, ProviderError::Transient(_))),
        (503, |e| matches!(e, ProviderError::Transient(_))),
        (504, |e| matches!(e, ProviderError::Transient(_))),
    ] {
        let fake = start(move |_| Reply::status(status, json!({ "error": "Runway error" }))).await;
        let err = Runway.submit(&fake.ctx(&[HOST], &[("api_key", "test-key")]), &model, &req).await.unwrap_err();
        assert!(check(&err), "HTTP {status}: {err:?}");
    }
}
