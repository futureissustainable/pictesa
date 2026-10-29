//! Magnific (formerly Freepik) AI API: the adapter against the documented API, replayed offline.
//! Docs: https://docs.magnific.com/llms.txt; request and response schemas from the OpenAPI file
//! linked on https://docs.magnific.com/authentication.md
//! (https://storage.googleapis.com/fc-freepik-pro-rev1-eu-api-specs/magnific-api-v1-openapi.yaml).
mod common;

use common::{start, Call, Fake, Reply};
use pictesa_core::{Ctx, GenerationRequest, InputRole, MediaInput, MediaKind, ModelSpec, Output, PollStatus, Provider, ProviderError, RemoteHandle, Submission};
use pictesa_providers::freepik::Freepik;
use serde_json::{json, Value};

const TASK: &str = "046b6c7f-0b8a-43b9-b35d-6489e6daee91";

fn ctx(fake: &Fake) -> Ctx {
    fake.ctx(&["api.magnific.com"], &[("api_key", "test-key")])
}

fn model(id: &str) -> ModelSpec {
    Freepik.models().into_iter().find(|m| m.remote_id() == id).expect("model in catalog")
}

fn request(model: &str, params: Value, inputs: Vec<MediaInput>) -> GenerationRequest {
    GenerationRequest {
        job_id: "contract".into(),
        model: model.into(),
        prompt: "A majestic eagle soaring over snow-capped mountains at golden hour".into(),
        negative_prompt: None,
        params: params.as_object().cloned().unwrap_or_default(),
        inputs,
        num_outputs: 1,
    }
}


/// A file with these bytes, named by their hash so reruns reuse it; written by rename so a test
/// running in parallel never reads it half-written.
fn file(bytes: &[u8]) -> std::path::PathBuf {
    let path = std::env::temp_dir().join(format!("pictesa-contract-{:08x}.png", crc32fast::hash(bytes)));
    if std::fs::read(&path).ok().as_deref() != Some(bytes) {
        let tmp = path.with_extension(format!("{}.tmp", ulid::Ulid::new()));
        std::fs::write(&tmp, bytes).unwrap();
        std::fs::rename(&tmp, &path).unwrap();
    }
    path
}
fn start_frame(url: &str) -> MediaInput {
    let path = file(common::PNG_1X1);
    MediaInput { role: InputRole::StartFrame, kind: MediaKind::Image, mime: "image/png".into(), path, url: Some(url.into()), width: None, height: None }
}

// task-detail examples ("Success - Task created / in progress / completed / failed") shared by the
// AI endpoints in the OpenAPI file.
fn task(status: &str, generated: &[&str]) -> Value {
    json!({ "data": { "generated": generated, "task_id": TASK, "status": status } })
}

fn assert_only_documented(call: &Call, documented: &[&str]) {
    for key in call.json().as_object().unwrap().keys() {
        assert!(documented.contains(&key.as_str()), "{} sent `{key}`, which the endpoint does not document", call.path);
    }
}

async fn submit(fake: &Fake, id: &str, params: Value, inputs: Vec<MediaInput>) -> RemoteHandle {
    match Freepik.submit(&ctx(fake), &model(id), &request(id, params, inputs)).await.expect("submit") {
        Submission::Pending(handle) => handle,
        Submission::Done(_) => panic!("Magnific queues every task"),
    }
}

#[tokio::test]
async fn check_lists_mystic_tasks_with_the_key_header() {
    // "Mystic - List tasks" (GET /v1/ai/mystic); key in `x-magnific-api-key`,
    // https://docs.magnific.com/authentication.md
    let fake = start(|_| Reply::json(json!({ "data": [{ "task_id": TASK, "status": "COMPLETED" }] }))).await;
    Freepik.check(&ctx(&fake)).await.expect("check");
    let call = &fake.calls()[0];
    assert_eq!((call.method.as_str(), call.path.as_str()), ("GET", "/v1/ai/mystic"));
    assert_eq!(call.header("x-magnific-api-key"), Some("test-key"));
    assert_eq!(call.header("authorization"), None);
}

fn outcome(result: Result<PollStatus, ProviderError>) -> String {
    match result {
        Ok(PollStatus::Queued { .. }) => "queued".into(),
        Ok(PollStatus::Running { .. }) => "running".into(),
        Ok(PollStatus::Done(out)) => format!("done:{}", out.len()),
        Ok(PollStatus::Failed(e)) => format!("failed:{}", e.kind()),
        Err(e) => format!("error:{}", e.kind()),
    }
}

#[tokio::test]
async fn mystic_submits_and_every_task_status_maps_to_a_poll_state() {
    // POST /v1/ai/mystic answers the in-progress task; GET /v1/ai/mystic/{task-id} reports
    // CREATED, IN_PROGRESS, COMPLETED (with `generated` URLs) or FAILED. https://docs.magnific.com/api-reference/mystic/post-mystic.md,
    // https://docs.magnific.com/api-reference/mystic/get-mystic-task.md
    let completed = json!({ "data": { "generated": ["https://ai-statics.freepik.com/completed_task_image.jpg"], "task_id": TASK, "status": "COMPLETED", "has_nsfw": [false] } });
    let cases = [("CREATED", task("CREATED", &[]), "queued"), ("IN_PROGRESS", task("IN_PROGRESS", &[]), "running"), ("COMPLETED", completed, "done:1"), ("FAILED", task("FAILED", &[]), "failed:failed")];
    let mut wrong = Vec::new();
    for (name, body, want) in cases {
        let fake = start(move |call| if call.method == "POST" { Reply::json(task("IN_PROGRESS", &[])) } else { Reply::json(body.clone()) }).await;
        let handle = submit(&fake, "mystic", json!({ "aspect_ratio": "16:9", "resolution": "4k" }), vec![]).await;
        let result = Freepik.poll(&ctx(&fake), &handle).await;
        if let Ok(PollStatus::Done(out)) = &result {
            assert!(matches!(&out[..], [Output::Url { url, .. }] if url == "https://ai-statics.freepik.com/completed_task_image.jpg"));
        }
        let got = outcome(result);
        if got != want {
            wrong.push(format!("{name}: {got}, want {want}"));
        }
        let calls = fake.calls();
        assert_eq!((calls[0].method.as_str(), calls[0].path.as_str()), ("POST", "/v1/ai/mystic"));
        assert_eq!(calls[0].header("x-magnific-api-key"), Some("test-key"));
        let body = calls[0].json();
        assert_eq!((body["aspect_ratio"].as_str(), body["resolution"].as_str()), (Some("widescreen_16_9"), Some("4k")));
        assert_only_documented(&calls[0], &["prompt", "webhook_url", "structure_reference", "structure_strength", "style_reference", "adherence", "hdr", "resolution", "aspect_ratio", "model", "creative_detailing", "engine", "fixed_generation", "filter_nsfw", "styling"]);
        assert_eq!((calls[1].method.as_str(), calls[1].path.as_str()), ("GET", format!("/v1/ai/mystic/{TASK}").as_str()));
        assert_eq!(calls[1].header("x-magnific-api-key"), Some("test-key"));
    }
    assert!(wrong.is_empty(), "{wrong:#?}");
}

#[tokio::test]
async fn video_endpoints_get_their_documented_fields_and_are_polled_on_their_own_route() {
    let fake = start(|call| Reply::json(task(if call.method == "POST" { "CREATED" } else { "IN_PROGRESS" }, &[]))).await;
    // POST /v1/ai/text-to-video/runway-4-5: prompt, ratio (pixel pairs), duration.
    let runway = submit(&fake, "text-to-video/runway-4-5", json!({ "aspect_ratio": "9:16", "duration": 10 }), vec![]).await;
    // POST /v1/ai/image-to-video/veo-3-1-fast: image (Base64 or HTTPS URL), prompt, aspect_ratio, ...
    submit(&fake, "image-to-video/veo-3-1-fast", json!({ "aspect_ratio": "9:16", "duration": 6, "resolution": "1080p", "generate_audio": false }), vec![start_frame("https://example.com/first.png")]).await;
    // POST /v1/ai/image-to-video/ltx-2-pro: image_url, fps as an integer (25 or 50), ...
    submit(&fake, "image-to-video/ltx-2-pro", json!({ "duration": 8, "resolution": "1440p", "fps": "50", "generate_audio": true }), vec![start_frame("https://example.com/first.png")]).await;
    Freepik.poll(&ctx(&fake), &runway).await.expect("poll");

    let calls = fake.calls();
    assert_eq!(calls[0].path, "/v1/ai/text-to-video/runway-4-5");
    let body = calls[0].json();
    assert_eq!((body["ratio"].as_str(), body["duration"].as_i64()), (Some("720:1280"), Some(10)));
    assert_only_documented(&calls[0], &["webhook_url", "prompt", "ratio", "duration"]);

    assert_eq!(calls[1].path, "/v1/ai/image-to-video/veo-3-1-fast");
    let body = calls[1].json();
    assert_eq!((body["image"].as_str(), body["aspect_ratio"].as_str(), body["duration"].as_i64()), (Some("https://example.com/first.png"), Some("9:16"), Some(6)));
    assert_eq!((body["resolution"].as_str(), body["generate_audio"].as_bool()), (Some("1080p"), Some(false)));
    assert_only_documented(&calls[1], &["webhook_url", "image", "image_end", "prompt", "negative_prompt", "duration", "resolution", "aspect_ratio", "generate_audio", "seed"]);

    assert_eq!(calls[2].path, "/v1/ai/image-to-video/ltx-2-pro");
    let body = calls[2].json();
    assert_eq!((body["image_url"].as_str(), body["fps"].as_i64(), body["resolution"].as_str()), (Some("https://example.com/first.png"), Some(50), Some("1440p")));
    assert_only_documented(&calls[2], &["webhook_url", "prompt", "generate_audio", "seed", "image_url", "resolution", "duration", "fps"]);

    // GET /v1/ai/text-to-video/runway-4-5/{task-id}
    assert_eq!((calls[3].method.as_str(), calls[3].path.as_str()), ("GET", format!("/v1/ai/text-to-video/runway-4-5/{TASK}").as_str()));
}

#[tokio::test]
async fn documented_errors_map_to_their_meaning() {
    // Shared 400 / 401 / 500 / 503 responses in the OpenAPI file. 402 and 429 are not documented.
    let cases = [
        (400, json!({ "message": "Parameter ':attribute' is not valid" }), "invalid_input"),
        (401, json!({ "message": "Invalid API key" }), "auth"),
        (500, json!({ "message": "Internal Server Error" }), "transient"),
        (503, json!({ "message": "Service Unavailable. Please try again later." }), "transient"),
    ];
    let mut wrong = Vec::new();
    for (status, body, want) in cases {
        let fake = start(move |_| Reply::status(status, body.clone())).await;
        let err = Freepik.submit(&ctx(&fake), &model("mystic"), &request("mystic", json!({}), vec![])).await.expect_err("error status");
        if err.kind() != want {
            wrong.push(format!("HTTP {status}: {}, want {want}", err.kind()));
        }
    }
    assert!(wrong.is_empty(), "{wrong:#?}");
}
