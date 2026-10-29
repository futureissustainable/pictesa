//! Luma Dream Machine contract: the documented answers of api.lumalabs.ai replayed through the
//! adapter, and what the adapter sends checked against the published API.
//! Docs: https://docs.lumalabs.ai/llms.txt (API reference, guides and errors, read 29.10.2026).

mod common;

use common::{start, Reply};
use pictesa_core::{GenerationRequest, InputRole, MediaInput, MediaKind, ModelSpec, Output, PollStatus, Provider, ProviderError, Submission};
use pictesa_providers::luma::Luma;
use serde_json::{json, Value};

const HOST: &str = "api.lumalabs.ai";
const ID: &str = "123e4567-e89b-12d3-a456-426614174000";

fn model(id: &str) -> ModelSpec {
    Luma.models().into_iter().find(|m| m.remote_id() == id).unwrap()
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

fn image_at(role: InputRole, url: Option<&str>) -> MediaInput {
    MediaInput { role, kind: MediaKind::Image, mime: "image/jpeg".into(), path: "unused.jpg".into(), url: url.map(String::from), width: None, height: None }
}

/// The Generation object Luma returns from create and get.
/// https://docs.lumalabs.ai/reference/creategeneration (schema `Generation`, example)
fn generation(state: &str, extra: Value) -> Value {
    let mut g = json!({ "id": ID, "state": state, "failure_reason": null, "created_at": "2023-06-01T12:00:00Z", "model": "ray-2" });
    g.as_object_mut().unwrap().extend(extra.as_object().cloned().unwrap_or_default());
    g
}

/// Submits `req` and polls once; the stand-in answers the poll with `polled`.
async fn submit_then_poll(model: &ModelSpec, req: &GenerationRequest, polled: Value) -> PollStatus {
    let fake = start(move |call| match call.method.as_str() {
        "POST" => Reply::status(201, generation("queued", json!({}))),
        _ => Reply::json(polled.clone()),
    })
    .await;
    let ctx = fake.ctx(&[HOST], &[("api_key", "test-key")]);
    let Submission::Pending(handle) = Luma.submit(&ctx, model, req).await.unwrap() else { panic!("Luma queues every generation") };
    assert_eq!(handle.id, ID);
    let status = Luma.poll(&ctx, &handle).await.unwrap();
    let poll = &fake.calls()[1];
    assert_eq!((poll.method.as_str(), poll.path.as_str()), ("GET", format!("/dream-machine/v1/generations/{ID}").as_str()));
    assert_eq!(poll.header("authorization"), Some("Bearer test-key"));
    status
}

#[tokio::test]
async fn check_lists_generations_with_the_key_as_a_bearer_token() {
    // https://docs.lumalabs.ai/reference/listgenerations (example `ListGenerationResponse`)
    let fake = start(|_| Reply::json(json!({ "has_more": false, "count": 1, "limit": 1, "offset": 0, "generations": [generation("completed", json!({ "assets": { "video": "https://example.com/video.mp4" } }))] }))).await;
    Luma.check(&fake.ctx(&[HOST], &[("api_key", "test-key")])).await.unwrap();
    let call = &fake.calls()[0];
    assert_eq!((call.method.as_str(), call.path.as_str()), ("GET", "/dream-machine/v1/generations?limit=1"));
    assert_eq!(call.header("authorization"), Some("Bearer test-key"));
}

#[tokio::test]
async fn text_to_video_posts_the_documented_ray_fields() {
    // https://docs.lumalabs.ai/reference/creategeneration (POST /generations/video, `GenerationRequest`):
    // model required; resolution "540p".."4k"; duration "5s" | "9s"; loop boolean.
    let model = model("ray-2");
    let req = request(&model, "an old lady laughing underwater", json!({ "duration": 9, "loop": true }), vec![]);
    let fake = start(|_| Reply::status(201, generation("queued", json!({})))).await;
    Luma.submit(&fake.ctx(&[HOST], &[("api_key", "test-key")]), &model, &req).await.unwrap();
    let call = &fake.calls()[0];
    assert_eq!((call.method.as_str(), call.path.as_str()), ("POST", "/dream-machine/v1/generations/video"));
    assert_eq!(call.header("authorization"), Some("Bearer test-key"));
    assert_eq!(
        call.json(),
        json!({ "model": "ray-2", "prompt": "an old lady laughing underwater", "aspect_ratio": "16:9", "resolution": "720p", "duration": "9s", "loop": true })
    );
}

#[tokio::test]
async fn image_to_video_sends_start_and_end_frames_as_image_keyframes() {
    // https://docs.lumalabs.ai/docs/video-generation ("With start and end keyframes")
    let model = model("ray-flash-2");
    let inputs = vec![image_at(InputRole::StartFrame, Some("https://example.com/first.jpg")), image_at(InputRole::EndFrame, Some("https://example.com/last.jpg"))];
    let req = request(&model, "A tiger walking in snow", json!({}), inputs);
    let fake = start(|_| Reply::status(201, generation("queued", json!({})))).await;
    Luma.submit(&fake.ctx(&[HOST], &[("api_key", "test-key")]), &model, &req).await.unwrap();
    let body = fake.calls()[0].json();
    assert_eq!(body["model"], "ray-flash-2");
    assert_eq!(
        body["keyframes"],
        json!({ "frame0": { "type": "image", "url": "https://example.com/first.jpg" }, "frame1": { "type": "image", "url": "https://example.com/last.jpg" } })
    );
}

#[tokio::test]
async fn a_frame_without_a_public_url_is_refused_before_calling_luma() {
    // https://docs.lumalabs.ai/docs/video-generation: "You should upload and use your own cdn image
    // urls, currently this is the only way to pass an image".
    let model = model("ray-2");
    let req = request(&model, "A tiger walking in snow", json!({}), vec![image_at(InputRole::StartFrame, None)]);
    let fake = start(|_| Reply::status(201, generation("queued", json!({})))).await;
    let err = Luma.submit(&fake.ctx(&[HOST], &[("api_key", "test-key")]), &model, &req).await.unwrap_err();
    assert!(matches!(err, ProviderError::InvalidInput(_)), "{err:?}");
    assert!(fake.calls().is_empty());
}

#[tokio::test]
async fn photon_references_use_the_documented_field_for_each_mode() {
    // https://docs.lumalabs.ai/docs/image-generation (Image, Style and Character Reference, Modify Image)
    // and https://docs.lumalabs.ai/reference/generateimage (POST /generations/image, `ImageGenerationRequest`).
    let url = "https://storage.cdn-luma.com/dream_machine/7e4fe07f-1dfd-4921-bc97-4bcf5adea39a/video_0_thumb.jpg";
    let cases = [
        ("image", "image_ref", json!([{ "url": url, "weight": 0.85 }])),
        ("style", "style_ref", json!([{ "url": url, "weight": 0.85 }])),
        ("character", "character_ref", json!({ "identity0": { "images": [url] } })),
        ("modify", "modify_image_ref", json!({ "url": url, "weight": 0.85 })),
    ];
    let model = model("photon-1");
    for (mode, field, expected) in cases {
        let req = request(&model, "sunglasses", json!({ "reference_mode": mode }), vec![image_at(InputRole::Reference, Some(url))]);
        let fake = start(|_| Reply::status(201, generation("queued", json!({})))).await;
        Luma.submit(&fake.ctx(&[HOST], &[("api_key", "test-key")]), &model, &req).await.unwrap();
        let call = &fake.calls()[0];
        assert_eq!(call.path, "/dream-machine/v1/generations/image", "{mode}");
        let body = call.json();
        assert_eq!((body["model"].as_str(), body["format"].as_str()), (Some("photon-1"), Some("jpg")), "{mode}");
        assert_eq!(body[field], expected, "{mode}");
    }
}

#[tokio::test]
async fn every_documented_state_maps_to_a_poll_status() {
    // `State` enum: queued, dreaming, completed, failed (https://docs.lumalabs.ai/reference/getgeneration).
    let video = model("ray-2");
    let req = request(&video, "a car", json!({}), vec![]);
    assert!(matches!(submit_then_poll(&video, &req, generation("queued", json!({}))).await, PollStatus::Queued { .. }));
    assert!(matches!(submit_then_poll(&video, &req, generation("dreaming", json!({}))).await, PollStatus::Running { .. }));
    // https://docs.lumalabs.ai/docs/video-generation ("Example Response")
    let done = submit_then_poll(&video, &req, generation("completed", json!({ "assets": { "video": "https://example.com/video.mp4" } }))).await;
    assert!(matches!(&done, PollStatus::Done(o) if matches!(o.as_slice(), [Output::Url { url, .. }] if url == "https://example.com/video.mp4")), "{done:?}");
    // https://docs.lumalabs.ai/docs/errors ("After submission", **Job**)
    let failed = submit_then_poll(&video, &req, generation("failed", json!({ "failure_reason": "Job failed" }))).await;
    assert!(matches!(failed, PollStatus::Failed(ProviderError::Failed(_))), "{failed:?}");

    // https://docs.lumalabs.ai/docs/image-generation ("Example Response": assets.video null, assets.image set)
    let photon = model("photon-1");
    let req = request(&photon, "man as a warrior", json!({}), vec![]);
    let done = submit_then_poll(&photon, &req, generation("completed", json!({ "type": "image", "assets": { "video": null, "image": "https://image.jpg" } }))).await;
    assert!(matches!(&done, PollStatus::Done(o) if matches!(o.as_slice(), [Output::Url { url, .. }] if url == "https://image.jpg")), "{done:?}");
}

#[tokio::test]
async fn moderation_failure_reasons_read_as_content_policy() {
    // https://docs.lumalabs.ai/docs/errors lists these `failure_reason` values under **Moderation**.
    let video = model("ray-2");
    let req = request(&video, "a car", json!({}), vec![]);
    for reason in ["Contains blacklisted words", "Frame moderation failed", "Advanced prompt moderation failed"] {
        let status = submit_then_poll(&video, &req, generation("failed", json!({ "failure_reason": reason }))).await;
        assert!(matches!(status, PollStatus::Failed(ProviderError::ContentPolicy(_))), "{reason}: {status:?}");
    }
}
