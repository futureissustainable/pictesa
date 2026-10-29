//! Alibaba Model Studio (DashScope) contract: the documented answers of the Wan video, Qwen Image
//! and task APIs replayed through the adapter, checking what it sends. Docs read 29.10.2026 from
//! https://www.alibabacloud.com/help/en/model-studio/. The adapter uses the Singapore domain
//! dashscope-intl.aliyuncs.com, which the docs say "remains fully functional".
mod common;

use base64::{engine::general_purpose::STANDARD, Engine as _};
use common::{start, Fake, Reply, PNG_1X1};
use pictesa_core::{
    Ctx, GenerationRequest, InputRole, MediaInput, MediaKind, ModelSpec, Output, PollStatus,
    Provider, ProviderError, RemoteHandle, Submission,
};
use pictesa_providers::alibaba::Alibaba;
use serde_json::{json, Value};

const HOST: &str = "dashscope-intl.aliyuncs.com";

fn ctx(fake: &Fake) -> Ctx {
    fake.ctx(&[HOST], &[("api_key", "test-key")])
}

fn model(id: &str) -> ModelSpec {
    Alibaba.models().into_iter().find(|m| m.remote_id() == id).unwrap()
}

fn request(model: &str, params: Value, inputs: Vec<MediaInput>) -> GenerationRequest {
    GenerationRequest {
        job_id: "job-1".into(),
        model: model.into(),
        prompt: "a kitten running in the moonlight".into(),
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

// Task creation "Successful response" (identical on every async API page).
fn created() -> Reply {
    Reply::json(json!({ "output": { "task_status": "PENDING", "task_id": "0385dc79-5ff8-4d82-bcb6-xxxxxx" }, "request_id": "4909100c-7b5a-9f92-bfe5-xxxxxx" }))
}

/// Submits one request against the stand-in and returns the handle and what was sent.
async fn submit(model_id: &str, req: GenerationRequest) -> (RemoteHandle, common::Call) {
    let fake = start(|_| created()).await;
    let Submission::Pending(handle) = Alibaba.submit(&ctx(&fake), &model(model_id), &req).await.unwrap() else {
        panic!("DashScope HTTP calls are asynchronous")
    };
    let call = fake.calls().remove(0);
    assert_eq!(call.method, "POST");
    assert_eq!(call.header("authorization"), Some("Bearer test-key"));
    // "If this request header is missing, the error 'current user api does not support synchronous calls' is returned."
    assert_eq!(call.header("x-dashscope-async"), Some("enable"));
    (handle, call)
}

#[tokio::test]
async fn check_reads_an_unknown_task_with_the_key_as_a_bearer_token() {
    // https://www.alibabacloud.com/help/en/model-studio/text-to-video-api-reference ("Task query
    // expired": UNKNOWN = "The task does not exist or its status is unknown").
    let fake = start(|_| Reply::json(json!({ "request_id": "a4de7c32-7057-9f82-8581-xxxxxx", "output": { "task_id": "00000000-0000-0000-0000-000000000000", "task_status": "UNKNOWN" } }))).await;
    Alibaba.check(&ctx(&fake)).await.unwrap();
    let call = &fake.calls()[0];
    assert_eq!((call.method.as_str(), call.path.as_str()), ("GET", "/api/v1/tasks/00000000-0000-0000-0000-000000000000"));
    assert_eq!(call.header("authorization"), Some("Bearer test-key"));
}

#[tokio::test]
async fn check_reports_a_bad_key_as_auth_and_an_overdue_account_as_missing_credit() {
    // https://www.alibabacloud.com/help/en/model-studio/error-code: 401 InvalidApiKey "Invalid
    // API-key provided."; 400 Arrearage "Access denied, please make sure your account is in good
    // standing." (overdue payment: recharge, a new key does not help).
    let cases: Vec<(u16, Value, fn(&ProviderError) -> bool)> = vec![
        (401, json!({ "code": "InvalidApiKey", "message": "Invalid API-key provided.", "request_id": "7438d53d-6eb8-4596-8835-xxxxxx" }), |e| matches!(e, ProviderError::Auth(_))),
        (400, json!({ "code": "Arrearage", "message": "Access denied, please make sure your account is in good standing.", "request_id": "7438d53d-6eb8-4596-8835-xxxxxx" }), |e| matches!(e, ProviderError::InsufficientCredits(_))),
    ];
    for (status, body, expected) in cases {
        let fake = start(move |_| Reply::status(status, body.clone())).await;
        let e = Alibaba.check(&ctx(&fake)).await.unwrap_err();
        assert!(expected(&e), "{status}: {e:?}");
    }
}

#[tokio::test]
async fn wan_text_to_video_sends_a_pixel_size_and_queues_the_task() {
    // https://www.alibabacloud.com/help/en/model-studio/text-to-video-api-reference ("HTTP (wan2.6
    // and earlier)"): POST /api/v1/services/aigc/video-generation/video-synthesis; input.prompt,
    // input.negative_prompt; parameters.size "width*height" (720p tier 9:16 = 720*1280), duration
    // 2-15 for wan2.6-t2v, prompt_extend.
    let mut req = request("wan2.6-t2v", json!({ "aspect_ratio": "9:16", "resolution": "720P", "duration": 10, "enhance_prompt": true }), vec![]);
    req.negative_prompt = Some("flowers".into());
    let (handle, call) = submit("wan2.6-t2v", req).await;
    assert_eq!(handle.id, "0385dc79-5ff8-4d82-bcb6-xxxxxx");
    assert_eq!(call.path, "/api/v1/services/aigc/video-generation/video-synthesis");
    assert_eq!(
        call.json(),
        json!({ "model": "wan2.6-t2v", "input": { "prompt": "a kitten running in the moonlight", "negative_prompt": "flowers" },
                "parameters": { "size": "720*1280", "duration": 10, "prompt_extend": true } })
    );
}

#[tokio::test]
async fn wan_first_frame_video_uses_the_video_synthesis_endpoint_with_img_url() {
    // https://www.alibabacloud.com/help/en/model-studio/image-to-video-api-reference (Wan 2.1-2.6
    // first frame): POST /api/v1/services/aigc/video-generation/video-synthesis; input.img_url as a
    // URL or data:{MIME_type};base64,{data}; parameters.resolution 720P|1080P for wan2.6-i2v.
    let req = request("wan2.6-i2v", json!({ "resolution": "720P", "duration": 5 }), vec![input(InputRole::StartFrame)]);
    let (_, call) = submit("wan2.6-i2v", req).await;
    assert_eq!(call.path, "/api/v1/services/aigc/video-generation/video-synthesis");
    let body = call.json();
    assert_eq!(body["input"]["img_url"], png_data_uri());
    assert_eq!((body["parameters"]["resolution"].as_str(), body["parameters"]["duration"].as_u64()), (Some("720P"), Some(5)));
}

#[tokio::test]
async fn wan_keyframes_use_the_image2video_endpoint_with_first_and_last_frame() {
    // https://www.alibabacloud.com/help/en/model-studio/image-to-video-by-first-and-last-frame-api-reference:
    // POST /api/v1/services/aigc/image2video/video-synthesis; input.first_frame_url, last_frame_url;
    // parameters.resolution 480P|720P|1080P for wan2.2-kf2v-flash.
    let req = request("wan2.2-kf2v-flash", json!({ "resolution": "480P" }), vec![input(InputRole::StartFrame), input(InputRole::EndFrame)]);
    let (_, call) = submit("wan2.2-kf2v-flash", req).await;
    assert_eq!(call.path, "/api/v1/services/aigc/image2video/video-synthesis");
    let body = call.json();
    assert_eq!((body["input"]["first_frame_url"].as_str(), body["input"]["last_frame_url"].as_str()), (Some(png_data_uri().as_str()), Some(png_data_uri().as_str())));
    assert_eq!(body["parameters"]["resolution"], "480P");
}

#[tokio::test]
async fn qwen_image_sends_one_user_message_with_text_and_images() {
    // https://www.alibabacloud.com/help/en/model-studio/qwen-image-generation-and-editing-api-reference
    // ("DashScope asynchronous API"): POST /api/v1/services/aigc/image-generation/generation;
    // input.messages = exactly one {role: "user", content}; content holds one {text} and, for
    // image-to-image, 1-3 {image} (URL or data URI); parameters n 1-6, size "w*h", negative_prompt, seed.
    let mut req = request("qwen-image-3.0-pro", json!({ "resolution": "1024*1536", "seed": 42 }), vec![input(InputRole::Reference)]);
    req.negative_prompt = Some("blurry text".into());
    req.num_outputs = 2;
    let (handle, call) = submit("qwen-image-3.0-pro", req).await;
    assert_eq!(call.path, "/api/v1/services/aigc/image-generation/generation");
    let body = call.json();
    let messages = body["input"]["messages"].as_array().unwrap();
    assert_eq!(messages.len(), 1, "single-round only");
    assert_eq!(messages[0]["role"], "user");
    let content = messages[0]["content"].as_array().unwrap();
    assert_eq!(content.iter().filter(|c| c.get("text").is_some()).count(), 1, "exactly one text object");
    assert!(content.contains(&json!({ "text": "a kitten running in the moonlight" })));
    assert!(content.contains(&json!({ "image": png_data_uri() })));
    assert_eq!(body["parameters"], json!({ "n": 2, "size": "1024*1536", "negative_prompt": "blurry text", "seed": 42 }));
    assert_eq!(handle.data["kind"], "image");
}

async fn poll(kind: MediaKind, answer: Value) -> PollStatus {
    let fake = start(move |_| Reply::json(answer.clone())).await;
    let handle = RemoteHandle::with("eff1443c-ccab-4676-aad3-xxxxxx", json!({ "kind": kind }));
    let handle: RemoteHandle = serde_json::from_str(&serde_json::to_string(&handle).unwrap()).unwrap();
    let status = Alibaba.poll(&ctx(&fake), &handle).await.unwrap();
    let call = &fake.calls()[0];
    assert_eq!((call.method.as_str(), call.path.as_str()), ("GET", "/api/v1/tasks/eff1443c-ccab-4676-aad3-xxxxxx"));
    assert_eq!(call.header("authorization"), Some("Bearer test-key"));
    status
}

#[tokio::test]
async fn task_polls_map_pending_running_and_succeeded_video_and_image() {
    // https://www.alibabacloud.com/help/en/model-studio/text-to-video-api-reference (Step 2 and
    // "Task executed successfully": output.video_url) and
    // https://www.alibabacloud.com/help/en/model-studio/qwen-image-generation-and-editing-api-reference
    // ("Task succeeded": output.choices[].message.content[].image).
    let state = |s: &str| json!({ "request_id": "caa62a12-8841-41a6-8af2-xxxxxx", "output": { "task_id": "eff1443c-ccab-4676-aad3-xxxxxx", "task_status": s } });
    assert!(matches!(poll(MediaKind::Video, state("PENDING")).await, PollStatus::Queued { .. }));
    assert!(matches!(poll(MediaKind::Video, state("RUNNING")).await, PollStatus::Running { .. }));

    let video = json!({
        "request_id": "caa62a12-8841-41a6-8af2-xxxxxx",
        "output": { "task_id": "eff1443c-ccab-4676-aad3-xxxxxx", "task_status": "SUCCEEDED", "submit_time": "2025-09-29 14:18:52.331",
                    "scheduled_time": "2025-09-29 14:18:59.290", "end_time": "2025-09-29 14:23:39.407", "orig_prompt": "a kitten running in the moonlight",
                    "video_url": "https://dashscope-result-sh.oss-accelerate.aliyuncs.com/xxx.mp4?Expires=xxx" },
        "usage": { "duration": 10, "input_video_duration": 0, "output_video_duration": 10, "video_count": 1, "ratio": "16:9", "SR": 720 }
    });
    let PollStatus::Done(out) = poll(MediaKind::Video, video).await else { panic!("video") };
    assert!(matches!(&out[..], [Output::Url { url, .. }] if url == "https://dashscope-result-sh.oss-accelerate.aliyuncs.com/xxx.mp4?Expires=xxx"));

    let image = json!({
        "output": { "task_id": "17d7d840-82b9-485b-a954-724d06bc88d2", "task_status": "SUCCEEDED", "submit_time": "2026-08-07 15:50:14.837",
                    "scheduled_time": "2026-08-07 15:50:14.884", "end_time": "2026-08-07 15:50:33.607", "rewrite_status": "not_use",
                    "choices": [{ "finish_reason": "stop", "message": { "role": "assistant", "content": [
                        { "image": "https://dashscope-result-sz.oss-cn-shenzhen.aliyuncs.com/xxx.png?Expires=xxx", "type": "image" }
                    ]}}] },
        "usage": { "output_height": 1024, "output_width": 1024, "input_image_count": 0, "input_image_type": "qima_input_1k", "output_image_count": 1, "output_image_type": "qima_output_1k" },
        "request_id": "2bd94002-5624-9129-916b-fbdde107b4ba"
    });
    let PollStatus::Done(out) = poll(MediaKind::Image, image).await else { panic!("image") };
    assert!(matches!(&out[..], [Output::Url { url, .. }] if url == "https://dashscope-result-sz.oss-cn-shenzhen.aliyuncs.com/xxx.png?Expires=xxx"));
}

#[tokio::test]
async fn failed_tasks_report_the_code_and_message_from_output() {
    // https://www.alibabacloud.com/help/en/model-studio/text-to-video-api-reference ("Task execution
    // failed": output.code / output.message); DataInspectionFailed "Input or output data may contain
    // inappropriate content." is a content block (https://www.alibabacloud.com/help/en/model-studio/error-code).
    let failed = |code: &str, message: &str| json!({ "request_id": "e5d70b02-ebd3-98ce-9fe8-759d7d7b107d", "output": { "task_id": "86ecf553-d340-4e21-af6e-a0c6a421c010", "task_status": "FAILED", "code": code, "message": message } });
    let PollStatus::Failed(e) = poll(MediaKind::Video, failed("InvalidParameter", "The size does not match xxxxxx")).await else { panic!("failed") };
    assert!(e.to_string().contains("The size does not match"), "the provider's reason reaches the user: {e}");
    let PollStatus::Failed(e) = poll(MediaKind::Image, failed("DataInspectionFailed", "Output data may contain inappropriate content.")).await else { panic!("blocked") };
    assert!(matches!(e, ProviderError::ContentPolicy(_)), "{e:?}");
}

#[tokio::test]
async fn http_errors_map_to_auth_credits_rate_limits_and_transient_failures() {
    // https://www.alibabacloud.com/help/en/model-studio/error-code: 401 InvalidApiKey; 403
    // AllocationQuota.FreeTierOnly and 429 PrepaidBillOverdue are billing states (retrying does not
    // help); 429 Throttling.RateQuota is a rate limit; 500 InternalError.
    let cases: Vec<(u16, &str, &str, fn(&ProviderError) -> bool)> = vec![
        (401, "InvalidApiKey", "Invalid API-key provided.", |e| matches!(e, ProviderError::Auth(_))),
        (403, "AllocationQuota.FreeTierOnly", "The free tier of the model has been exhausted. If you want to continue access the model on a paid basis, please disable the \"use free tier only\" mode in the management console.", |e| matches!(e, ProviderError::InsufficientCredits(_))),
        (429, "PrepaidBillOverdue", "The prepaid bill is overdue.", |e| matches!(e, ProviderError::InsufficientCredits(_))),
        (429, "Throttling.RateQuota", "Requests rate limit exceeded, please try again later.", |e| matches!(e, ProviderError::RateLimited { .. })),
        (500, "InternalError", "An internal error has occured, please try again later or contact service support.", |e| matches!(e, ProviderError::Transient(_))),
    ];
    for (status, code, message, expected) in cases {
        let fake = start(move |_| Reply::status(status, json!({ "code": code, "message": message, "request_id": "7438d53d-6eb8-4596-8835-xxxxxx" }))).await;
        let e = Alibaba.submit(&ctx(&fake), &model("wan2.6-t2v"), &request("wan2.6-t2v", json!({}), vec![])).await.unwrap_err();
        assert!(expected(&e), "{status} {code}: {e:?}");
    }
}
