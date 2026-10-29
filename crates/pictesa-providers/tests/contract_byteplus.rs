//! BytePlus ModelArk (Seedream images, Seedance video): the adapter against the documented API,
//! replayed offline. Docs: https://docs.byteplus.com/en/docs/ModelArk/ (pages cited per fixture).
mod common;

use base64::Engine as _;
use common::{start, Fake, Reply, PNG_1X1};
use pictesa_core::{Ctx, GenerationRequest, InputRole, MediaInput, MediaKind, ModelSpec, Output, PollStatus, Provider, ProviderError, RemoteHandle, Submission};
use pictesa_providers::byteplus::BytePlus;
use serde_json::{json, Value};

const HOST: &str = "ark.ap-southeast.bytepluses.com";
const TASK: &str = "cgt-20261029120000-x7k2p";

fn ctx(fake: &Fake) -> Ctx {
    fake.ctx(&[HOST], &[("api_key", "test-key")])
}

fn model(id: &str) -> ModelSpec {
    BytePlus.models().into_iter().find(|m| m.remote_id() == id).expect("model in catalog")
}

fn request(model: &str, params: Value, inputs: Vec<MediaInput>) -> GenerationRequest {
    GenerationRequest {
        job_id: "contract".into(),
        model: model.into(),
        prompt: "a kitten yawning at the camera".into(),
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
fn input(role: InputRole, kind: MediaKind, url: Option<&str>) -> MediaInput {
    let path = file(PNG_1X1);
    let mime = match kind {
        MediaKind::Video => "video/mp4",
        MediaKind::Audio => "audio/wav",
        _ => "image/png",
    };
    MediaInput { role, kind, mime: mime.into(), path, url: url.map(str::to_string), width: None, height: None }
}

fn png_data_uri() -> String {
    // "data:image/<image format>;base64,<Base64 encoding>", format in lowercase,
    // https://docs.byteplus.com/en/docs/ModelArk/1541523 (`image`)
    format!("data:image/png;base64,{}", base64::engine::general_purpose::STANDARD.encode(PNG_1X1))
}

// Error body: `error.code` and `error.message` (https://docs.byteplus.com/en/docs/ModelArk/1541523,
// response `error`), codes, types and messages from https://docs.byteplus.com/en/docs/ModelArk/error-codes
fn api_error(code: &str, message: &str, kind: &str) -> Value {
    json!({ "error": { "code": code, "message": message, "type": kind } })
}

fn video_task() -> Reply {
    // Create a video generation task returns its `id`,
    // https://docs.byteplus.com/en/docs/ModelArk/create-video-generation-task-api
    Reply::json(json!({ "id": TASK }))
}

async fn submit_video(fake: &Fake, inputs: Vec<MediaInput>) -> RemoteHandle {
    let id = "dreamina-seedance-2-0-260128";
    let params = json!({ "aspect_ratio": "16:9", "duration": 8, "resolution": "720p", "generate_audio": true });
    match BytePlus.submit(&ctx(fake), &model(id), &request(id, params, inputs)).await.expect("submit") {
        Submission::Pending(handle) => handle,
        Submission::Done(_) => panic!("video is a task"),
    }
}

#[tokio::test]
async fn check_lists_video_tasks_with_the_api_key() {
    // GET .../contents/generations/tasks "supports only API Key authentication"; listing models is
    // ListFoundationModels on another host with Access Key auth only,
    // https://docs.byteplus.com/en/docs/ModelArk/Querying_a_list_of_video_generation_tasks and
    // https://docs.byteplus.com/en/docs/ModelArk/list-foundation-models-api
    let fake = start(|_| Reply::json(json!({ "items": [], "total": 0 }))).await;
    BytePlus.check(&ctx(&fake)).await.expect("check");
    let call = &fake.calls()[0];
    assert_eq!(call.method, "GET");
    assert_eq!(call.path.split('?').next(), Some("/api/v3/contents/generations/tasks"));
    assert_eq!(call.header("authorization"), Some("Bearer test-key"));
}

#[tokio::test]
async fn seedream_edit_sends_references_and_returns_the_image_url() {
    // Image generation API, https://docs.byteplus.com/en/docs/ModelArk/1541523
    let fake = start(|_| {
        Reply::json(json!({
            "model": "dola-seedream-5-0-pro-260628",
            "created": 1761710400,
            "data": [{ "url": "https://ark-content-generation-v2-ap-southeast-1.tos-ap-southeast-1.volces.com/dola-seedream-5-0-pro/0217617104.png", "size": "2048x1152", "output_format": "png" }],
            "usage": { "generated_images": 1, "output_tokens": 9216, "total_tokens": 9216 }
        }))
    })
    .await;
    let id = "dola-seedream-5-0-pro-260628";
    let inputs = vec![input(InputRole::Reference, MediaKind::Image, None), input(InputRole::Reference, MediaKind::Image, Some("https://example.com/b.png"))];
    let req = request(id, json!({ "aspect_ratio": "16:9", "resolution": "1.5K", "output_format": "png" }), inputs);
    let Submission::Done(outputs) = BytePlus.submit(&ctx(&fake), &model(id), &req).await.expect("submit") else { panic!("images answer synchronously") };

    let call = &fake.calls()[0];
    assert_eq!((call.method.as_str(), call.path.as_str()), ("POST", "/api/v3/images/generations"));
    assert_eq!(call.header("authorization"), Some("Bearer test-key"));
    let body = call.json();
    assert_eq!(body["model"], id);
    // Method 1 for `size`: a resolution level, with the aspect ratio described in the prompt.
    assert_eq!(body["size"], "1.5K");
    let prompt = body["prompt"].as_str().unwrap();
    assert!(prompt.starts_with("a kitten yawning at the camera") && prompt.contains("16:9"), "{prompt}");
    assert_eq!(body["image"], json!([png_data_uri(), "https://example.com/b.png"]));
    assert_eq!((body["output_format"].as_str(), body["response_format"].as_str()), (Some("png"), Some("url")));
    assert!(matches!(&outputs[..], [Output::Url { url, .. }] if url.ends_with("/0217617104.png")));
}

#[tokio::test]
async fn seedance_frames_go_in_content_with_their_roles() {
    let fake = start(|_| video_task()).await;
    let frames = vec![input(InputRole::StartFrame, MediaKind::Image, Some("https://example.com/first.png")), input(InputRole::EndFrame, MediaKind::Image, None)];
    let handle = submit_video(&fake, frames).await;
    assert_eq!(handle.id, TASK);

    let call = &fake.calls()[0];
    assert_eq!((call.method.as_str(), call.path.as_str()), ("POST", "/api/v3/contents/generations/tasks"));
    assert_eq!(call.header("authorization"), Some("Bearer test-key"));
    let body = call.json();
    // Request body and example, https://docs.byteplus.com/en/docs/ModelArk/create-video-generation-task-api
    assert_eq!(body["model"], "dreamina-seedance-2-0-260128");
    assert_eq!(
        body["content"],
        json!([
            { "type": "text", "text": "a kitten yawning at the camera" },
            { "type": "image_url", "image_url": { "url": "https://example.com/first.png" }, "role": "first_frame" },
            { "type": "image_url", "image_url": { "url": png_data_uri() }, "role": "last_frame" },
        ])
    );
    assert_eq!((body["ratio"].as_str(), body["duration"].as_i64(), body["resolution"].as_str(), body["generate_audio"].as_bool()), (Some("16:9"), Some(8), Some("720p"), Some(true)));
}

#[tokio::test]
async fn seedance_omni_references_use_the_documented_types_and_roles() {
    // `reference_image`, `reference_video`, `reference_audio` roles with image_url / video_url /
    // audio_url objects, https://docs.byteplus.com/en/docs/ModelArk/create-video-generation-task-api
    let fake = start(|_| video_task()).await;
    let refs = vec![
        input(InputRole::Reference, MediaKind::Image, Some("https://example.com/cat.png")),
        input(InputRole::Video, MediaKind::Video, Some("https://example.com/move.mp4")),
        input(InputRole::Audio, MediaKind::Audio, Some("https://example.com/purr.wav")),
    ];
    submit_video(&fake, refs).await;
    let content = fake.calls()[0].json()["content"].clone();
    assert_eq!(content[1], json!({ "type": "image_url", "image_url": { "url": "https://example.com/cat.png" }, "role": "reference_image" }));
    assert_eq!(content[2], json!({ "type": "video_url", "video_url": { "url": "https://example.com/move.mp4" }, "role": "reference_video" }));
    assert_eq!(content[3], json!({ "type": "audio_url", "audio_url": { "url": "https://example.com/purr.wav" }, "role": "reference_audio" }));
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
async fn every_task_status_maps_to_the_right_poll_state() {
    // Retrieve a video generation task: statuses queued, running, cancelled, succeeded, failed,
    // expired; `content.video_url`; `error.code` / `error.message`,
    // https://docs.byteplus.com/en/docs/ModelArk/Querying_the_information_about_a_video_generation_task
    let task = |status: &str, extra: Value| {
        let mut v = json!({ "id": TASK, "model": "dreamina-seedance-2-0-260128", "status": status, "error": null, "created_at": 1761710400, "updated_at": 1761710460 });
        v.as_object_mut().unwrap().extend(extra.as_object().unwrap().clone());
        v
    };
    let video = "https://ark-content-generation-ap-southeast-1.tos-ap-southeast-1.volces.com/dreamina-seedance-2-0/0217617104.mp4";
    let cases = [
        ("queued", task("queued", json!({})), "queued"),
        ("running", task("running", json!({})), "running"),
        ("succeeded", task("succeeded", json!({ "content": { "video_url": video }, "usage": { "completion_tokens": 108900, "total_tokens": 108900 }, "resolution": "720p", "ratio": "16:9", "duration": 8 })), "done:1"),
        ("failed, moderated output", task("failed", json!({ "error": api_error("OutputVideoSensitiveContentDetected", "The request failed because the output video may contain sensitive information.Request ID: 20261029120000ABC", "BadRequest")["error"].clone() })), "failed:content_policy"),
        ("failed, internal", task("failed", json!({ "error": api_error("InternalServiceError", "The service encountered an unexpected internal error. Please retry later. Request ID: 20261029120000ABC", "InternalServerError")["error"].clone() })), "failed:failed"),
        ("expired", task("expired", json!({})), "failed:failed"),
        ("cancelled", task("cancelled", json!({})), "failed:failed"),
    ];
    let mut wrong = Vec::new();
    for (name, body, want) in cases {
        let fake = start(move |call| if call.method == "POST" { video_task() } else { Reply::json(body.clone()) }).await;
        let handle = submit_video(&fake, vec![]).await;
        let got = outcome(BytePlus.poll(&ctx(&fake), &handle).await);
        let polled = &fake.calls()[1];
        assert_eq!((polled.method.as_str(), polled.path.as_str()), ("GET", format!("/api/v3/contents/generations/tasks/{TASK}").as_str()));
        assert_eq!(polled.header("authorization"), Some("Bearer test-key"));
        if got != want {
            wrong.push(format!("{name}: {got}, want {want}"));
        }
    }
    assert!(wrong.is_empty(), "{wrong:#?}");
}

#[tokio::test]
async fn documented_error_codes_map_to_their_meaning() {
    // https://docs.byteplus.com/en/docs/ModelArk/error-codes
    let cases = [
        (401, api_error("AuthenticationError", "The API key or AK/SK in the request is missing or invalid. Request ID: 20261029120000ABC.", "Unauthorized"), "auth"),
        (403, api_error("AccountOverdueError", "The request failed because your account has an overdue balance. Request ID: 20261029120000ABC.", "Forbidden"), "insufficient_credits"),
        (429, api_error("QuotaExceeded", "Your account [2100012345] has exhausted its free trial quota for the [dola-seedream-5-0-pro] model. Request ID: 20261029120000ABC.", "TooManyRequests"), "insufficient_credits"),
        (429, api_error("ModelAccountRpmRateLimitExceeded", "RPM (Requests Per Minute) limit of the model is exceeded. Request ID: 20261029120000ABC.", "TooManyRequests"), "rate_limited"),
        (400, api_error("InputImageSensitiveContentDetected", "The request failed because the input image may contain sensitive information.Request ID: 20261029120000ABC.", "BadRequest"), "content_policy"),
        (400, api_error("OutputImageSensitiveContentDetected.PolicyViolation", "The request failed because the output image may be related to copyright restrictions. Request ID: 20261029120000ABC.", "BadRequest"), "content_policy"),
        (400, api_error("InvalidParameter", "One or more parameters specified in the request are not valid. Request ID: 20261029120000ABC.", "BadRequest"), "invalid_input"),
        (500, api_error("InternalServiceError", "The service encountered an unexpected internal error. Please retry later. Request ID: 20261029120000ABC.", "InternalServerError"), "transient"),
    ];
    let id = "dola-seedream-5-0-pro-260628";
    let mut wrong = Vec::new();
    for (status, body, want) in cases {
        let code = body["error"]["code"].as_str().unwrap().to_string();
        let fake = start(move |_| Reply::status(status, body.clone())).await;
        let err = BytePlus.submit(&ctx(&fake), &model(id), &request(id, json!({}), vec![])).await.expect_err("error status");
        if err.kind() != want {
            wrong.push(format!("HTTP {status} {code}: {}, want {want}", err.kind()));
        }
    }
    assert!(wrong.is_empty(), "{wrong:#?}");
}
