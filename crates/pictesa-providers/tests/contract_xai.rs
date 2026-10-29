//! xAI contract: the documented answers of the Models, Images and Videos APIs replayed through the
//! adapter, checking what it sends. Docs read 29.10.2026 (markdown via `<page>.md`). The image
//! examples answer `"url": "..."`; these fixtures fill that placeholder with a test URL.
mod common;

use base64::{engine::general_purpose::STANDARD, Engine as _};
use common::{start, Fake, Reply, PNG_1X1};
use pictesa_core::{
    Ctx, GenerationRequest, InputRole, MediaInput, MediaKind, ModelSpec, Output, PollStatus,
    Provider, ProviderError, RemoteHandle, Submission,
};
use pictesa_providers::xai::Xai;
use serde_json::{json, Value};

const HOST: &str = "api.x.ai";

fn ctx(fake: &Fake) -> Ctx {
    fake.ctx(&[HOST], &[("api_key", "test-key")])
}

fn model(id: &str) -> ModelSpec {
    Xai.models().into_iter().find(|m| m.remote_id() == id).unwrap()
}

fn request(model: &str, params: Value, inputs: Vec<MediaInput>) -> GenerationRequest {
    GenerationRequest {
        job_id: "job-1".into(),
        model: model.into(),
        prompt: "a collage of London landmarks in a stenciled street-art style".into(),
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

fn png_data_uri() -> String {
    format!("data:image/png;base64,{}", STANDARD.encode(PNG_1X1))
}

// https://docs.x.ai/developers/rest-api-reference/inference/images ("Response example": data[].url)
fn images_response() -> Value {
    json!({ "data": [{ "url": "https://imgen.example/xai-image-0.jpeg" }] })
}

#[tokio::test]
async fn check_lists_models_with_the_key_as_a_bearer_token() {
    // https://docs.x.ai/developers/rest-api-reference/inference/models (GET /v1/models example)
    let fake = start(|_| {
        Reply::json(json!({ "data": [{ "id": "grok-420-reasoning", "aliases": [], "context_length": 256000, "created": 1768003200, "object": "model", "owned_by": "xai" }], "object": "list" }))
    })
    .await;
    Xai.check(&ctx(&fake)).await.unwrap();
    let call = &fake.calls()[0];
    assert_eq!((call.method.as_str(), call.path.as_str()), ("GET", "/v1/models"));
    assert_eq!(call.header("authorization"), Some("Bearer test-key"));
}

#[tokio::test]
async fn generation_posts_documented_fields_and_returns_the_image_url() {
    // https://docs.x.ai/developers/rest-api-reference/inference/images (POST /v1/images/generations:
    // aspect_ratio, n, prompt, model, resolution "1k"|"2k"|"1.5k");
    // https://docs.x.ai/developers/model-capabilities/images/generation#quality (low|medium|auto,
    // grok-imagine-image-2.0 only).
    let fake = start(|_| Reply::json(images_response())).await;
    let req = request("grok-imagine-image-2.0", json!({ "aspect_ratio": "16:9", "resolution": "2k", "quality": "medium" }), vec![]);
    let Submission::Done(out) = Xai.submit(&ctx(&fake), &model("grok-imagine-image-2.0"), &req).await.unwrap() else {
        panic!("generations answer synchronously")
    };
    assert!(matches!(&out[..], [Output::Url { url, .. }] if url == "https://imgen.example/xai-image-0.jpeg"));
    let call = &fake.calls()[0];
    assert_eq!((call.method.as_str(), call.path.as_str()), ("POST", "/v1/images/generations"));
    assert_eq!(call.header("authorization"), Some("Bearer test-key"));
    assert_eq!(
        call.json(),
        json!({ "model": "grok-imagine-image-2.0", "prompt": "a collage of London landmarks in a stenciled street-art style", "n": 1, "aspect_ratio": "16:9", "resolution": "2k", "quality": "medium" })
    );
}

#[tokio::test]
async fn edits_send_one_image_object_or_an_images_list() {
    // https://docs.x.ai/developers/rest-api-reference/inference/images (POST /v1/images/edits:
    // `image` {url: public or base64 data URL}, or `images` for multi-reference editing, mutually
    // exclusive; the curl example adds "type": "image_url").
    let fake = start(|_| Reply::json(images_response())).await;
    let ctx = ctx(&fake);
    let one = request("grok-imagine-image-2.0", json!({}), vec![input(InputRole::Reference)]);
    Xai.submit(&ctx, &model("grok-imagine-image-2.0"), &one).await.unwrap();
    let two = request("grok-imagine-image-2.0", json!({}), vec![input(InputRole::Reference), input(InputRole::Reference)]);
    Xai.submit(&ctx, &model("grok-imagine-image-2.0"), &two).await.unwrap();
    let calls = fake.calls();
    assert!(calls.iter().all(|c| c.path == "/v1/images/edits"));
    let (single, multi) = (calls[0].json(), calls[1].json());
    assert_eq!(single["image"]["url"], png_data_uri());
    assert!(single.get("images").is_none());
    assert_eq!(multi["images"].as_array().map(Vec::len), Some(2));
    assert!(multi["images"].as_array().unwrap().iter().all(|i| i["url"] == png_data_uri()));
    assert!(multi.get("image").is_none(), "`image` and `images` are mutually exclusive");
}

#[tokio::test]
async fn video_generation_sends_frames_and_settings_and_queues_the_request_id() {
    // https://docs.x.ai/developers/rest-api-reference/inference/videos (POST /v1/videos/generations:
    // duration 1-15, aspect_ratio, resolution 480p|720p|1080p, image {url}; answer {request_id});
    // last_frame: https://docs.x.ai/developers/model-capabilities/video/reference-to-video;
    // generate_audio: https://docs.x.ai/developers/model-capabilities/video/generation#audio.
    let fake = start(|_| Reply::json(json!({ "request_id": "a3d1008e-4544-40d4-d075-11527e794e4a" }))).await;
    let req = request(
        "grok-imagine-video-1.5",
        json!({ "duration": 8, "aspect_ratio": "16:9", "resolution": "720p", "generate_audio": false }),
        vec![input(InputRole::StartFrame), input(InputRole::EndFrame)],
    );
    let Submission::Pending(handle) = Xai.submit(&ctx(&fake), &model("grok-imagine-video-1.5"), &req).await.unwrap() else {
        panic!("video generation is asynchronous")
    };
    assert_eq!(handle.id, "a3d1008e-4544-40d4-d075-11527e794e4a");
    let call = &fake.calls()[0];
    assert_eq!((call.method.as_str(), call.path.as_str()), ("POST", "/v1/videos/generations"));
    let body = call.json();
    assert_eq!((body["model"].as_str(), body["duration"].as_u64()), (Some("grok-imagine-video-1.5"), Some(8)));
    assert_eq!((body["aspect_ratio"].as_str(), body["resolution"].as_str(), body["generate_audio"].as_bool()), (Some("16:9"), Some("720p"), Some(false)));
    assert_eq!((body["image"]["url"].as_str(), body["last_frame"]["url"].as_str()), (Some(png_data_uri().as_str()), Some(png_data_uri().as_str())));
}

async fn poll_video(answer: Value) -> PollStatus {
    let fake = start(move |_| Reply::json(answer.clone())).await;
    let handle = RemoteHandle::with("a3d1008e-4544-40d4-d075-11527e794e4a", json!({ "output": "video" }));
    let handle: RemoteHandle = serde_json::from_str(&serde_json::to_string(&handle).unwrap()).unwrap();
    let status = Xai.poll(&ctx(&fake), &handle).await.unwrap();
    let call = &fake.calls()[0];
    assert_eq!((call.method.as_str(), call.path.as_str()), ("GET", "/v1/videos/a3d1008e-4544-40d4-d075-11527e794e4a"));
    status
}

#[tokio::test]
async fn video_polls_map_pending_done_failed_and_moderated() {
    // https://docs.x.ai/developers/rest-api-reference/inference/videos (GET /v1/videos/{request_id}:
    // status pending|done|failed, progress 0-99 while pending, video.url, error {code, message};
    // respect_moderation false means "the video url field will be empty").
    assert!(matches!(poll_video(json!({ "status": "pending", "progress": 40, "model": "grok-imagine-video-1.5" })).await, PollStatus::Running { .. }));

    let done = json!({ "status": "done", "video": { "url": "https://vidgen.x.ai/xai-vidgen-bucket/xai-video-a3d1008e-4544-40d4-d075-11527e794e4a.mp4", "duration": 8, "respect_moderation": true }, "model": "grok-imagine-video-1.5" });
    let PollStatus::Done(out) = poll_video(done).await else { panic!("done") };
    assert!(matches!(&out[..], [Output::Url { url, .. }] if url == "https://vidgen.x.ai/xai-vidgen-bucket/xai-video-a3d1008e-4544-40d4-d075-11527e794e4a.mp4"));

    // https://docs.x.ai/developers/model-capabilities/video/generation (failed example)
    let failed = json!({ "status": "failed", "error": { "code": "invalid_argument", "message": "Prompt cannot be empty. Please provide a prompt." } });
    let PollStatus::Failed(e) = poll_video(failed).await else { panic!("failed") };
    assert!(e.to_string().contains("Prompt cannot be empty"), "{e}");

    let moderated = json!({ "status": "done", "video": { "url": "", "duration": 8, "respect_moderation": false }, "model": "grok-imagine-video-1.5" });
    let status = poll_video(moderated).await;
    assert!(matches!(status, PollStatus::Failed(ProviderError::ContentPolicy(_))), "{status:?}");
}

#[tokio::test]
async fn http_errors_map_to_auth_rate_limits_and_transient_failures() {
    // https://docs.x.ai/developers/debugging (401 invalid token, 403 forbidden, 429 rate limit);
    // https://docs.x.ai/developers/rest-api-reference/inference/videos (overload as HTTP 503) with
    // the cause text from https://docs.x.ai/developers/model-capabilities/video/generation. The
    // error body shape is not documented; the bodies carry the documented cause text.
    let cases: Vec<(u16, &str, fn(&ProviderError) -> bool)> = vec![
        (401, "No authorization header or an invalid authorization token was provided.", |e| matches!(e, ProviderError::Auth(_))),
        (403, "Ask your team admin for permission.", |e| matches!(e, ProviderError::Auth(_))),
        (429, "You are sending requests too frequently and have reached the rate limit.", |e| matches!(e, ProviderError::RateLimited { .. })),
        (503, "Video generation is temporarily overloaded.", |e| matches!(e, ProviderError::Transient(_))),
    ];
    for (status, cause, expected) in cases {
        let fake = start(move |_| Reply::status(status, json!({ "error": cause }))).await;
        let e = Xai.submit(&ctx(&fake), &model("grok-imagine-image-2.0"), &request("grok-imagine-image-2.0", json!({}), vec![])).await.unwrap_err();
        assert!(expected(&e), "{status}: {e:?}");
    }
}
