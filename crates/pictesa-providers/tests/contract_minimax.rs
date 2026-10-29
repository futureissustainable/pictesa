//! MiniMax contract: the documented answers of api.minimax.io replayed through the adapter, and what
//! the adapter sends checked against the published API.
//! Docs: https://platform.minimax.io/docs/llms.txt (API reference pages, read 29.10.2026).

mod common;

use base64::Engine as _;
use common::{start, Reply, PNG_1X1};
use pictesa_core::{GenerationRequest, InputRole, MediaInput, MediaKind, ModelSpec, Output, PollStatus, Provider, ProviderError, Submission};
use pictesa_providers::minimax::Minimax;
use serde_json::{json, Value};

const HOST: &str = "api.minimax.io";

fn spec(id: &str) -> ModelSpec {
    Minimax.models().into_iter().find(|m| m.remote_id() == id).unwrap()
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

fn image(role: InputRole, url: &str) -> MediaInput {
    MediaInput { role, kind: MediaKind::Image, mime: "image/jpeg".into(), path: "unused.jpg".into(), url: Some(url.into()), width: None, height: None }
}

fn local_png(role: InputRole) -> MediaInput {
    let dir = std::env::temp_dir().join(format!("pictesa-contract-minimax-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("frame.png");
    std::fs::write(&path, PNG_1X1).unwrap();
    MediaInput { role, kind: MediaKind::Image, mime: "image/png".into(), path, url: None, width: None, height: None }
}

/// V1 create answer. https://platform.minimax.io/docs/api-reference/video-generation-t2v.md (`VideoGenerationResp` example)
fn v1_created() -> Value {
    json!({ "task_id": "106916112212032", "base_resp": { "status_code": 0, "status_msg": "success" } })
}

/// V2 create answer. https://platform.minimax.io/docs/api-reference/video-generation-v2-create.md (`VideoGenerationV2Resp` example)
fn v2_created() -> Value {
    json!({ "task_id": "424010985738629" })
}

/// Submits, then polls once; `answer` serves every request after the submission.
async fn submit_then_poll(model: &ModelSpec, req: &GenerationRequest, created: Value, answer: impl Fn(&common::Call) -> Reply + Send + Sync + 'static) -> (Result<PollStatus, ProviderError>, Vec<common::Call>) {
    let fake = start(move |call| if call.method == "POST" { Reply::json(created.clone()) } else { answer(call) }).await;
    let ctx = fake.ctx(&[HOST], &[("api_key", "test-key")]);
    let Submission::Pending(handle) = Minimax.submit(&ctx, model, req).await.unwrap() else { panic!("MiniMax queues video tasks") };
    let status = Minimax.poll(&ctx, &handle).await;
    let calls = fake.calls();
    for call in &calls {
        assert_eq!(call.header("authorization"), Some("Bearer test-key"), "{}", call.path);
    }
    (status, calls)
}

#[tokio::test]
async fn check_lists_v2_tasks_with_the_bearer_key() {
    // https://platform.minimax.io/docs/api-reference/video-generation-v2-list.md (GET /v2/query/video_generation)
    let fake = start(|_| Reply::json(json!({ "items": [{ "id": "424635601932571", "model": "MiniMax-H3", "status": "succeeded", "created_at": 1785225940, "updated_at": 1785226100 }], "total": 1 }))).await;
    Minimax.check(&fake.ctx(&[HOST], &[("api_key", "test-key")])).await.unwrap();
    let call = &fake.calls()[0];
    assert_eq!((call.method.as_str(), call.path.split('?').next().unwrap()), ("GET", "/v2/query/video_generation"));
    assert_eq!(call.header("authorization"), Some("Bearer test-key"));
}

#[tokio::test]
async fn h3_text_to_video_sends_the_v2_content_array() {
    // https://platform.minimax.io/docs/api-reference/video-generation-v2-create.md: model, content,
    // resolution and duration required; text-to-video "ratio is required and cannot be adaptive".
    let model = spec("MiniMax-H3");
    let req = request(&model, "A boy playing basketball by the sea", json!({ "aspect_ratio": "9:16", "duration": 10, "resolution": "2K" }), vec![]);
    let fake = start(|_| Reply::json(v2_created())).await;
    let Submission::Pending(handle) = Minimax.submit(&fake.ctx(&[HOST], &[("api_key", "test-key")]), &model, &req).await.unwrap() else { panic!() };
    assert_eq!(handle.id, "424010985738629");
    let call = &fake.calls()[0];
    assert_eq!((call.method.as_str(), call.path.as_str()), ("POST", "/v2/video_generation"));
    assert_eq!(call.header("authorization"), Some("Bearer test-key"));
    assert_eq!(
        call.json(),
        json!({ "model": "MiniMax-H3", "content": [{ "type": "text", "text": "A boy playing basketball by the sea" }], "duration": 10, "resolution": "2K", "ratio": "9:16" })
    );
}

#[tokio::test]
async fn h3_image_to_video_tags_first_and_last_frames() {
    // Same page, `ContentItem`: image_url.url is a public URL or a `data:image/<format>;base64,` URI;
    // role first_frame / last_frame; for image-to-video the ratio follows the image.
    let model = spec("MiniMax-H3-Max");
    let inputs = vec![image(InputRole::StartFrame, "https://cdn.example.com/first.png"), local_png(InputRole::EndFrame)];
    let req = request(&model, "Pull focus to the people in the background", json!({}), inputs);
    let fake = start(|_| Reply::json(v2_created())).await;
    Minimax.submit(&fake.ctx(&[HOST], &[("api_key", "test-key")]), &model, &req).await.unwrap();
    let body = fake.calls()[0].json();
    assert_eq!(body["model"], "MiniMax-H3-Max");
    assert_eq!(body["content"][0], json!({ "type": "text", "text": "Pull focus to the people in the background" }));
    assert_eq!(body["content"][1], json!({ "type": "image_url", "image_url": { "url": "https://cdn.example.com/first.png" }, "role": "first_frame" }));
    assert_eq!(body["content"][2]["role"], "last_frame");
    assert!(body["content"][2]["image_url"]["url"].as_str().unwrap().starts_with("data:image/png;base64,"));
    assert_eq!((body["duration"].as_i64(), body["resolution"].as_str()), (Some(5), Some("768P")));
    assert!(body.get("ratio").is_none(), "image-to-video takes its ratio from the image");
}

#[tokio::test]
async fn every_documented_h3_task_status_maps_to_a_poll_status() {
    // https://platform.minimax.io/docs/api-reference/video-generation-v2-query.md (GET
    // /v2/query/video_generation/{task_id}; `VideoTask.status`: queued, running, succeeded, failed, cancelled)
    let model = spec("MiniMax-H3");
    let req = request(&model, "move", json!({}), vec![]);
    let task = |extra: Value| {
        let mut t = json!({ "id": "424010985738629", "model": "MiniMax-H3", "created_at": 1785125529, "updated_at": 1785125946, "resolution": "2K", "duration": 5, "ratio": "16:9", "task_type": "generation", "modality": "video" });
        t.as_object_mut().unwrap().extend(extra.as_object().cloned().unwrap());
        json!({ "task": t })
    };
    let succeeded = task(json!({ "status": "succeeded", "content": { "url": "https://video-product.cdn.minimax.io/inference_output/rollout/2026-07-27/6c68f487-4b33-48cb-8c92-1631f63f6682/output.mp4" } }));
    let failed = task(json!({ "status": "failed", "error": { "code": "1026", "message": "video description contains sensitive content" } }));
    let mut seen = Vec::new();
    for answer in [task(json!({ "status": "queued" })), task(json!({ "status": "running" })), succeeded, failed, task(json!({ "status": "cancelled" }))] {
        let (status, calls) = submit_then_poll(&model, &req, v2_created(), move |_| Reply::json(answer.clone())).await;
        assert_eq!((calls[1].method.as_str(), calls[1].path.as_str()), ("GET", "/v2/query/video_generation/424010985738629"));
        seen.push(status);
    }
    assert!(matches!(seen[0], Ok(PollStatus::Queued { .. })), "{:?}", seen[0]);
    assert!(matches!(seen[1], Ok(PollStatus::Running { .. })), "{:?}", seen[1]);
    assert!(matches!(&seen[2], Ok(PollStatus::Done(o)) if matches!(o.as_slice(), [Output::Url { url, .. }] if url.ends_with("/output.mp4"))), "{:?}", seen[2]);
    assert!(matches!(seen[3], Ok(PollStatus::Failed(ProviderError::ContentPolicy(_)))), "{:?}", seen[3]);
    // A cancelled task is a reported, terminal failure, not an unreadable answer.
    assert!(matches!(seen[4], Ok(PollStatus::Failed(_))), "{:?}", seen[4]);
}

#[tokio::test]
async fn h3_http_errors_map_to_their_meaning() {
    // https://platform.minimax.io/docs/api-reference/video-generation-v2-create.md (`Err400`..`Err500`
    // examples: "On error the HTTP status is the real error code ... and the body is this object").
    let model = spec("MiniMax-H3");
    let req = request(&model, "move", json!({}), vec![]);
    let err = |status: u16, kind: &str, message: &str| (status, json!({ "type": "error", "error": { "type": kind, "message": message, "http_code": status.to_string() }, "request_id": "021785229015510a2c883cf675b9804d" }));
    let cases = [
        (err(401, "authorized_error", "login fail: Please carry the API secret key in the 'Authorization' field of the request header (1004)"), "auth"),
        (err(402, "insufficient_balance_error", "insufficient balance (1008)"), "insufficient_credits"),
        (err(422, "unprocessable_entity_error", "video description contains sensitive content (1026)"), "content_policy"),
        (err(429, "rate_limit_error", "rate limit, please retry later (1002)"), "rate_limited"),
        (err(500, "server_error", "internal error (1000)"), "transient"),
        (err(400, "bad_request_error", "invalid params, content must include a non-empty text item (prompt is required) (2013)"), "invalid_input"),
    ];
    for ((status, body), kind) in cases {
        let fake = start(move |_| Reply::status(status, body.clone())).await;
        let e = Minimax.submit(&fake.ctx(&[HOST], &[("api_key", "test-key")]), &model, &req).await.unwrap_err();
        assert_eq!(e.kind(), kind, "HTTP {status}: {e:?}");
    }
}

#[tokio::test]
async fn hailuo_sends_v1_first_and_last_frame_fields() {
    // https://platform.minimax.io/docs/api-reference/video-generation-i2v.md (first_frame_image required for
    // MiniMax-Hailuo-2.3-Fast) and .../video-generation-fl2v.md (MiniMax-Hailuo-02 first_frame_image +
    // last_frame_image); both POST /v1/video_generation with duration 6 | 10 and resolution 768P | 1080P.
    let fast = spec("MiniMax-Hailuo-2.3-Fast");
    let req = request(&fast, "A little girl grow up.", json!({}), vec![image(InputRole::StartFrame, "https://filecdn.minimax.chat/public/fe9d04da-f60e-444d-a2e0-18ae743add33.jpeg")]);
    let fake = start(|_| Reply::json(v1_created())).await;
    let Submission::Pending(handle) = Minimax.submit(&fake.ctx(&[HOST], &[("api_key", "test-key")]), &fast, &req).await.unwrap() else { panic!() };
    assert_eq!(handle.id, "106916112212032");
    let call = &fake.calls()[0];
    assert_eq!((call.method.as_str(), call.path.as_str()), ("POST", "/v1/video_generation"));
    assert_eq!(
        call.json(),
        json!({ "model": "MiniMax-Hailuo-2.3-Fast", "prompt": "A little girl grow up.", "first_frame_image": "https://filecdn.minimax.chat/public/fe9d04da-f60e-444d-a2e0-18ae743add33.jpeg", "duration": 6, "resolution": "768P" })
    );

    let fl = spec("MiniMax-Hailuo-02");
    let inputs = vec![
        image(InputRole::StartFrame, "https://filecdn.minimax.chat/public/fe9d04da-f60e-444d-a2e0-18ae743add33.jpeg"),
        image(InputRole::EndFrame, "https://filecdn.minimax.chat/public/97b7cd08-764e-4b8b-a7bf-87a0bd898575.jpeg"),
    ];
    let req = request(&fl, "A little girl grow up.", json!({ "resolution": "1080P" }), inputs);
    let fake = start(|_| Reply::json(v1_created())).await;
    Minimax.submit(&fake.ctx(&[HOST], &[("api_key", "test-key")]), &fl, &req).await.unwrap();
    let body = fake.calls()[0].json();
    assert_eq!(body["last_frame_image"], "https://filecdn.minimax.chat/public/97b7cd08-764e-4b8b-a7bf-87a0bd898575.jpeg");
    assert_eq!((body["model"].as_str(), body["resolution"].as_str()), (Some("MiniMax-Hailuo-02"), Some("1080P")));
}

#[tokio::test]
async fn hailuo_success_fetches_the_file_download_url() {
    // https://platform.minimax.io/docs/api-reference/video-generation-query.md (status Preparing,
    // Queueing, Processing, Success, Fail; file_id on success) and
    // https://platform.minimax.io/docs/api-reference/file-management-retrieve.md (GET /v1/files/retrieve?file_id=).
    let model = spec("MiniMax-Hailuo-2.3");
    let req = request(&model, "A man picks up a book [Pedestal up]", json!({}), vec![]);
    let query = |status: &str| json!({ "task_id": "176843862716480", "status": status, "base_resp": { "status_code": 0, "status_msg": "success" } });
    for (status, queued) in [("Preparing", true), ("Queueing", true), ("Processing", false)] {
        let answer = query(status);
        let (polled, calls) = submit_then_poll(&model, &req, v1_created(), move |_| Reply::json(answer.clone())).await;
        assert_eq!(calls[1].path, "/v1/query/video_generation?task_id=106916112212032");
        let polled = polled.unwrap();
        assert_eq!(matches!(polled, PollStatus::Queued { .. }), queued, "{status}: {polled:?}");
        assert_eq!(matches!(polled, PollStatus::Running { .. }), !queued, "{status}: {polled:?}");
    }

    let (polled, calls) = submit_then_poll(&model, &req, v1_created(), |call| {
        if call.path.starts_with("/v1/files/retrieve") {
            // The docs' own example values, `download_url` included.
            Reply::json(json!({ "file": { "file_id": 176844028768320u64, "bytes": 5896337, "created_at": 1700469398, "filename": "output_aigc.mp4", "purpose": "video_generation", "download_url": "www.downloadurl.com" }, "base_resp": { "status_code": 0, "status_msg": "success" } }))
        } else {
            Reply::json(json!({ "task_id": "176843862716480", "status": "Success", "file_id": "176844028768320", "video_width": 1920, "video_height": 1080, "base_resp": { "status_code": 0, "status_msg": "success" } }))
        }
    })
    .await;
    assert_eq!(calls[2].path, "/v1/files/retrieve?file_id=176844028768320");
    assert!(matches!(&polled, Ok(PollStatus::Done(o)) if matches!(o.as_slice(), [Output::Url { url, .. }] if url == "www.downloadurl.com")), "{polled:?}");
}

#[tokio::test]
async fn a_failed_hailuo_task_is_a_reported_failure() {
    // https://platform.minimax.io/docs/api-reference/video-generation-query.md: `Fail`, with base_resp
    // 1026 "Contains sensitive content in the input" / 1027 "... in the generated video". Message
    // text from https://platform.minimax.io/docs/api-reference/errorcode.md.
    let model = spec("MiniMax-Hailuo-2.3");
    let req = request(&model, "move", json!({}), vec![]);
    for (code, msg, policy) in [(1027, "output new_sensitive", true), (1026, "input new_sensitive", true), (0, "success", false)] {
        let answer = json!({ "task_id": "176843862716480", "status": "Fail", "base_resp": { "status_code": code, "status_msg": msg } });
        let (polled, _) = submit_then_poll(&model, &req, v1_created(), move |_| Reply::json(answer.clone())).await;
        let failed = matches!(polled, Ok(PollStatus::Failed(_)));
        assert!(failed, "code {code}: a failed task must come back as PollStatus::Failed, got {polled:?}");
        assert_eq!(matches!(polled, Ok(PollStatus::Failed(ProviderError::ContentPolicy(_)))), policy, "code {code}: {polled:?}");
    }
}

#[tokio::test]
async fn v1_status_codes_in_a_200_answer_map_to_their_meaning() {
    // https://platform.minimax.io/docs/api-reference/video-generation-t2v.md (`BaseResp.status_code`:
    // 1002 rate limit, 1004 authentication, 1008 balance, 1026 sensitive prompt, 2049 invalid API key);
    // `status_msg` text from https://platform.minimax.io/docs/api-reference/errorcode.md.
    let model = spec("MiniMax-Hailuo-2.3");
    let req = request(&model, "move", json!({}), vec![]);
    for (code, msg, kind) in [
        (1002, "rate limit", "rate_limited"),
        (1004, "not authorized", "auth"),
        (1008, "insufficient balance", "insufficient_credits"),
        (1026, "input new_sensitive", "content_policy"),
        (2049, "invalid API Key", "auth"),
    ] {
        let fake = start(move |_| Reply::json(json!({ "task_id": "", "base_resp": { "status_code": code, "status_msg": msg } }))).await;
        let e = Minimax.submit(&fake.ctx(&[HOST], &[("api_key", "test-key")]), &model, &req).await.unwrap_err();
        assert_eq!(e.kind(), kind, "code {code}: {e:?}");
    }
}

#[tokio::test]
async fn image01_returns_the_base64_images_inline() {
    // https://platform.minimax.io/docs/api-reference/image-generation-t2i.md (POST /v1/image_generation:
    // model, prompt, aspect_ratio, response_format url | base64, n 1-9; `data.image_base64`) and
    // .../image-generation-i2i.md (subject_reference[]: type "character", image_file URL or data URL).
    let model = spec("image-01");
    let png = base64::engine::general_purpose::STANDARD.encode(PNG_1X1);
    let answer = json!({ "id": "03ff3cd0820949eb8a410056b5f21d38", "data": { "image_base64": [png, png] }, "metadata": { "failed_count": "0", "success_count": "2" }, "base_resp": { "status_code": 0, "status_msg": "success" } });
    let fake = start(move |_| Reply::json(answer.clone())).await;
    let mut req = request(&model, "A man in a white t-shirt", json!({ "aspect_ratio": "16:9" }), vec![image(InputRole::Reference, "https://cdn.hailuoai.com/prod/2025-08-12-17/video_cover/1754990600020238321-411603868533342214-cover.jpg")]);
    req.num_outputs = 2;
    let Submission::Done(outputs) = Minimax.submit(&fake.ctx(&[HOST], &[("api_key", "test-key")]), &model, &req).await.unwrap() else { panic!("image-01 answers synchronously") };
    assert_eq!(outputs.len(), 2);
    assert!(outputs.iter().all(|o| matches!(o, Output::Bytes { data, .. } if data == PNG_1X1)));
    let call = &fake.calls()[0];
    assert_eq!((call.method.as_str(), call.path.as_str()), ("POST", "/v1/image_generation"));
    assert_eq!(
        call.json(),
        json!({ "model": "image-01", "prompt": "A man in a white t-shirt", "response_format": "base64", "n": 2, "aspect_ratio": "16:9", "subject_reference": [{ "type": "character", "image_file": "https://cdn.hailuoai.com/prod/2025-08-12-17/video_cover/1754990600020238321-411603868533342214-cover.jpg" }] })
    );
}
