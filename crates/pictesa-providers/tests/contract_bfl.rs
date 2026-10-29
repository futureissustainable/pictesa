//! Black Forest Labs: the adapter against the documented FLUX API, replayed offline.
//! Docs: https://docs.bfl.ml/llms.txt (OpenAPI excerpts on each API reference page).
mod common;

use base64::Engine as _;
use common::{start, Call, Fake, Reply, PNG_1X1};
use pictesa_core::{Ctx, GenerationRequest, InputRole, MediaInput, MediaKind, ModelSpec, Output, PollStatus, Provider, RemoteHandle, Submission};
use pictesa_providers::bfl::Bfl;
use serde_json::{json, Value};

const POLLING_URL: &str = "https://api.eu.bfl.ai/v1/get_result?id=4c7c7a4e-5c0b-4b5a-9b53-0c0f4d1f2e9a";
// The submit host and the regional host its `polling_url` points at
// (https://docs.bfl.ml/api_integration/integration_guidelines.md).
const HOSTS: &[&str] = &["api.bfl.ai", "api.eu.bfl.ai"];

fn ctx(fake: &Fake) -> Ctx {
    fake.ctx(HOSTS, &[("api_key", "test-key")])
}

fn model(id: &str) -> ModelSpec {
    Bfl.models().into_iter().find(|m| m.remote_id() == id).expect("model in catalog")
}

fn request(model: &str, params: Value, inputs: Vec<MediaInput>) -> GenerationRequest {
    GenerationRequest {
        job_id: "contract".into(),
        model: model.into(),
        prompt: "a red panda on a bamboo beam".into(),
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
    let mime = if kind == MediaKind::Video { "video/mp4" } else { "image/png" };
    MediaInput { role, kind, mime: mime.into(), path, url: url.map(str::to_string), width: None, height: None }
}

fn png_base64() -> String {
    base64::engine::general_purpose::STANDARD.encode(PNG_1X1)
}

// AsyncResponse, https://docs.bfl.ml/api-reference/utility/generate-an-image-with-flux-3.md
fn submitted() -> Reply {
    Reply::json(json!({ "id": "4c7c7a4e-5c0b-4b5a-9b53-0c0f4d1f2e9a", "polling_url": POLLING_URL, "cost": 4.0, "input_mp": 0.0, "output_mp": 1.05 }))
}

fn body_keys(call: &Call) -> Vec<String> {
    call.json().as_object().unwrap().keys().cloned().collect()
}

fn assert_only_documented(call: &Call, documented: &[&str]) {
    for key in body_keys(call) {
        assert!(documented.contains(&key.as_str()), "{} sent `{key}`, which the endpoint does not document", call.path);
    }
}

async fn submit(fake: &Fake, model_id: &str, params: Value, inputs: Vec<MediaInput>) -> RemoteHandle {
    match Bfl.submit(&ctx(fake), &model(model_id), &request(model_id, params, inputs)).await.expect("submit") {
        Submission::Pending(handle) => handle,
        Submission::Done(_) => panic!("BFL queues every generation"),
    }
}

#[tokio::test]
async fn check_reads_the_credit_balance_with_the_key_in_x_key() {
    // CreditsResponse, https://docs.bfl.ml/api-reference/get-the-users-credits.md
    let fake = start(|_| Reply::json(json!({ "credits": 1250.5 }))).await;
    Bfl.check(&ctx(&fake)).await.expect("check");
    let call = &fake.calls()[0];
    assert_eq!((call.method.as_str(), call.host.as_str(), call.path.as_str()), ("GET", "api.bfl.ai", "/v1/credits"));
    assert_eq!(call.header("x-key"), Some("test-key"));
    assert_eq!(call.header("authorization"), None);
}

#[tokio::test]
async fn flux_2_edit_sends_numbered_input_images_and_polls_the_returned_url() {
    let fake = start(|call| {
        if call.method == "POST" {
            submitted()
        } else {
            // Ready result, https://docs.bfl.ml/flux_3/flux3_image_generate.md ("Results and errors")
            Reply::json(json!({ "id": "4c7c7a4e-5c0b-4b5a-9b53-0c0f4d1f2e9a", "status": "Ready", "result": { "sample": "https://delivery.eu1.bfl.ai/results/ab/cd/sample.jpeg?se=2026-10-29T12%3A00%3A00Z&sig=x", "prompt": "a red panda", "duration": 7.1 }, "progress": null, "details": null, "preview": null }))
        }
    })
    .await;
    let inputs = vec![input(InputRole::Reference, MediaKind::Image, None), input(InputRole::Reference, MediaKind::Image, Some("https://example.com/second.png"))];
    let handle = submit(&fake, "flux-2-pro", json!({ "width": 1440, "height": 816, "seed": 7, "output_format": "png" }), inputs).await;

    let sent = &fake.calls()[0];
    assert_eq!((sent.method.as_str(), sent.host.as_str(), sent.path.as_str()), ("POST", "api.bfl.ai", "/v1/flux-2-pro"));
    assert_eq!(sent.header("x-key"), Some("test-key"));
    let body = sent.json();
    assert_eq!(body["prompt"], "a red panda on a bamboo beam");
    assert_eq!(body["input_image"], png_base64());
    assert_eq!(body["input_image_2"], "https://example.com/second.png");
    assert_eq!((body["width"].as_i64(), body["height"].as_i64(), body["seed"].as_i64()), (Some(1440), Some(816), Some(7)));
    assert_eq!(body["output_format"], "png");
    // Flux2Inputs, https://docs.bfl.ml/api-reference/models/generate-or-edit-an-image-with-flux2-[pro].md
    assert_only_documented(sent, &["user", "prompt", "disable_pup", "input_image", "input_image_2", "input_image_3", "input_image_4", "input_image_5", "input_image_6", "input_image_7", "input_image_8", "seed", "width", "height", "safety_tolerance", "output_format", "webhook_url", "webhook_secret"]);

    let PollStatus::Done(outputs) = Bfl.poll(&ctx(&fake), &handle).await.expect("poll") else { panic!("Ready is done") };
    let polled = &fake.calls()[1];
    assert_eq!((polled.method.as_str(), polled.host.as_str()), ("GET", "api.eu.bfl.ai"), "the returned polling_url, not a rebuilt one");
    assert_eq!(polled.path, "/v1/get_result?id=4c7c7a4e-5c0b-4b5a-9b53-0c0f4d1f2e9a");
    assert_eq!(polled.header("x-key"), Some("test-key"));
    match &outputs[..] {
        // "don't send your x-key header to it" (the signed result URL)
        [Output::Url { url, headers }] => {
            assert!(url.starts_with("https://delivery.eu1.bfl.ai/results/"));
            assert!(headers.is_empty(), "no key travels to the delivery host");
        }
        other => panic!("one URL output, got {other:?}"),
    }
}

#[tokio::test]
async fn flux_3_image_sends_only_fields_the_endpoint_accepts() {
    // FLUX 3 Image rejects unknown fields with 422 ("additionalProperties: false"),
    // https://docs.bfl.ml/api-reference/utility/generate-an-image-with-flux-3.md
    let fake = start(|_| submitted()).await;
    submit(&fake, "flux-3-image", json!({ "aspect_ratio": "16:9", "resolution": "2k" }), vec![input(InputRole::Reference, MediaKind::Image, Some("https://example.com/ref.png"))]).await;
    let sent = &fake.calls()[0];
    assert_eq!(sent.path, "/v1/flux-3-image");
    let body = sent.json();
    assert_eq!(body["images"], json!(["https://example.com/ref.png"]));
    assert_eq!((body["aspect_ratio"].as_str(), body["resolution"].as_str()), (Some("16:9"), Some("2k")));
    assert_only_documented(sent, &["user", "prompt", "images", "aspect_ratio", "resolution", "width", "height", "safety_tolerance", "grounding", "version"]);
}

#[tokio::test]
async fn flux_3_video_names_the_mode_and_its_media_field() {
    // Flux3VideoT2VInputs / I2VInputs / V2VInputs, all "additionalProperties: false",
    // https://docs.bfl.ml/api-reference/utility/generate-a-video-with-flux-3.md
    const COMMON: &[&str] = &["user", "prompt", "aspect_ratio", "duration", "resolution", "version", "generate_audio", "safety_tolerance", "draft", "mode"];
    let fake = start(|_| submitted()).await;
    let params = json!({ "duration": 8, "aspect_ratio": "9:16", "resolution": "fhd", "generate_audio": false });

    submit(&fake, "flux-3-video", params.clone(), vec![]).await;
    let start_frame = input(InputRole::StartFrame, MediaKind::Image, Some("https://example.com/start.png"));
    let end_frame = input(InputRole::EndFrame, MediaKind::Image, None);
    submit(&fake, "flux-3-video", params.clone(), vec![end_frame, start_frame]).await;
    submit(&fake, "flux-3-video", params, vec![input(InputRole::Video, MediaKind::Video, Some("https://example.com/clip.mp4"))]).await;

    let calls = fake.calls();
    assert!(calls.iter().all(|c| c.path == "/v1/flux-3-video"));
    let (t2v, i2v, v2v) = (calls[0].json(), calls[1].json(), calls[2].json());
    assert_eq!(t2v["mode"], "t2v");
    assert_eq!((t2v["duration"].as_i64(), t2v["aspect_ratio"].as_str(), t2v["resolution"].as_str(), t2v["generate_audio"].as_bool()), (Some(8), Some("9:16"), Some("fhd"), Some(false)));
    assert_only_documented(&calls[0], COMMON);
    assert_eq!(i2v["mode"], "i2v");
    assert_eq!(i2v["keyframes"], json!(["https://example.com/start.png", png_base64()]), "start frame first, end frame last");
    assert_only_documented(&calls[1], &[COMMON, &["keyframes"]].concat());
    assert_eq!(v2v["mode"], "v2v");
    assert_eq!(v2v["start_video"], "https://example.com/clip.mp4");
    assert_only_documented(&calls[2], &[COMMON, &["start_video"]].concat());
}

fn poll_reply(status: u16, body: Value) -> impl Fn(&Call) -> Reply + Send + Sync + 'static {
    move |call| if call.method == "POST" { submitted() } else { Reply::status(status, body.clone()) }
}

fn outcome(result: Result<PollStatus, pictesa_core::ProviderError>) -> String {
    match result {
        Ok(PollStatus::Queued { .. }) => "queued".into(),
        Ok(PollStatus::Running { .. }) => "running".into(),
        Ok(PollStatus::Done(out)) => format!("done:{}", out.len()),
        Ok(PollStatus::Failed(e)) => format!("failed:{}", e.kind()),
        Err(e) => format!("error:{}", e.kind()),
    }
}

#[tokio::test]
async fn every_documented_status_maps_to_the_right_poll_state() {
    let result = |status: &str, result: Value, details: Value| json!({ "id": "4c7c7a4e-5c0b-4b5a-9b53-0c0f4d1f2e9a", "status": status, "result": result, "progress": null, "details": details, "preview": null });
    // StatusResponse enum, https://docs.bfl.ml/api-reference/utility/get-result.md;
    // meanings, https://docs.bfl.ml/flux_3/flux3_image_generate.md ("Results and errors");
    // moderation bodies, https://docs.bfl.ml/api_integration/errors.md
    let reasons = json!({ "Moderation Reasons": ["Violence", "Sexual Content", "Self Harm"] });
    let cases = [
        ("Pending", result("Pending", Value::Null, Value::Null), "queued"),
        ("Reasoning", result("Reasoning", Value::Null, Value::Null), "running"),
        ("Generating", result("Generating", Value::Null, Value::Null), "running"),
        // FLUX 3 Video returns its .mp4 in result.sample too, https://docs.bfl.ml/flux_3/flux3_video.md
        ("Ready (video)", result("Ready", json!({ "sample": "https://delivery.eu1.bfl.ai/results/ab/cd/sample.mp4?sig=x" }), Value::Null), "done:1"),
        ("Request Moderated", result("Request Moderated", Value::Null, reasons.clone()), "failed:content_policy"),
        ("Content Moderated", result("Content Moderated", Value::Null, reasons), "failed:content_policy"),
        ("Error", result("Error", Value::Null, Value::Null), "failed:failed"),
        // "The id is unknown or has expired | Submit a new request": never a job still in the queue.
        ("Task not found", result("Task not found", Value::Null, Value::Null), "error:failed"),
    ];
    let mut wrong = Vec::new();
    for (name, body, want) in cases {
        let fake = start(poll_reply(200, body)).await;
        let handle = submit(&fake, "flux-3-image", json!({}), vec![]).await;
        let got = outcome(Bfl.poll(&ctx(&fake), &handle).await);
        if got != want {
            wrong.push(format!("{name}: {got}, want {want}"));
        }
    }
    assert!(wrong.is_empty(), "{wrong:#?}");
}

#[tokio::test]
async fn a_503_that_carries_a_task_status_is_read_as_that_status() {
    // "A failed task can come back as HTTP 503 with a normal JSON body. Read status from the body
    // before treating a 503 as a retryable outage." https://docs.bfl.ml/flux_3/flux3_image_generate.md
    let failed = json!({ "id": "4c7c7a4e-5c0b-4b5a-9b53-0c0f4d1f2e9a", "status": "Error", "result": null, "progress": null, "details": null, "preview": null });
    let fake = start(poll_reply(503, failed)).await;
    let handle = submit(&fake, "flux-3-image", json!({}), vec![]).await;
    assert_eq!(outcome(Bfl.poll(&ctx(&fake), &handle).await), "failed:failed");

    // 503 Service Unavailable without a task body stays a retryable outage, https://docs.bfl.ml/api_integration/errors.md
    let fake = start(poll_reply(503, json!({ "detail": "Service Unavailable" }))).await;
    let handle = submit(&fake, "flux-3-image", json!({}), vec![]).await;
    assert_eq!(outcome(Bfl.poll(&ctx(&fake), &handle).await), "error:transient");
}

#[tokio::test]
async fn submit_errors_map_to_their_documented_meaning() {
    // Status meanings, https://docs.bfl.ml/api_integration/errors.md. The page gives no bodies except
    // the 422 HTTPValidationError shape; the others carry the documented meaning as `detail`.
    let cases = [
        (402, json!({ "detail": "Your account has insufficient credits or your payment method needs to be updated." }), "insufficient_credits"),
        (403, json!({ "detail": "Your API key does not have permission to access the specified resource." }), "auth"),
        (422, json!({ "detail": [{ "loc": ["body", "seed"], "msg": "Extra inputs are not permitted", "type": "extra_forbidden" }] }), "invalid_input"),
        (429, json!({ "detail": "You've exceeded the rate limits for your account." }), "rate_limited"),
        (500, json!({ "detail": "An unexpected error occurred on our servers." }), "transient"),
    ];
    let mut wrong = Vec::new();
    for (status, body, want) in cases {
        let fake = start(move |_| Reply::status(status, body.clone())).await;
        let err = Bfl.submit(&ctx(&fake), &model("flux-2-pro"), &request("flux-2-pro", json!({}), vec![])).await.expect_err("error status");
        if err.kind() != want {
            wrong.push(format!("HTTP {status}: {}, want {want}", err.kind()));
        }
    }
    assert!(wrong.is_empty(), "{wrong:#?}");
}
