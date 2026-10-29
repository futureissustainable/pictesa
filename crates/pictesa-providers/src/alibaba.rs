use async_trait::async_trait;
use pictesa_core::{
    common, keys, CredentialField, Ctx, GenerationRequest, InputRole, InputSlot, MediaKind,
    ModelSpec, Output, ParamSpec, PollStatus, Provider, ProviderError, ProviderInfo, ProviderKind,
    ProviderResult, RemoteHandle, Submission, Task,
};
use serde_json::{json, Value};

use crate::util::{failure, net_error, API_TIMEOUT};

const BASE: &str = "https://dashscope-intl.aliyuncs.com/api/v1";
pub struct Alibaba;

fn base(ctx: &Ctx) -> String {
    ctx.base_url(BASE)
}

fn checked(v: Value) -> ProviderResult<Value> {
    let Some(code) = v
        .get("code")
        .and_then(Value::as_str)
        .filter(|s| !s.is_empty())
    else {
        return Ok(v);
    };
    let message = v
        .get("message")
        .and_then(Value::as_str)
        .unwrap_or(code)
        .to_string();
    Err(code_error(code, message))
}

/// Billing states DashScope reports as 400, 403 or 429 (overdue account, free tier only, spent
/// budget); retrying or a new key does not help (https://www.alibabacloud.com/help/en/model-studio/error-code).
const BILLING: &[&str] = &["Arrearage", "BillOverdue", "BudgetLimitExceeded", "FreeTierOnly", "CommodityNotPurchased"];

fn code_error(code: &str, message: String) -> ProviderError {
    if code.contains("InvalidApiKey") || code.contains("Unauthorized") {
        ProviderError::Auth(message)
    } else if BILLING.iter().any(|c| code.contains(c)) {
        ProviderError::InsufficientCredits(message)
    } else if code.contains("Throttling") || code.contains("RateLimit") {
        ProviderError::RateLimited {
            retry_after_ms: None,
        }
    } else if code.contains("Balance") || code.contains("Quota") {
        ProviderError::InsufficientCredits(message)
    } else if code.contains("DataInspection") || code.contains("IPInfringement") {
        ProviderError::ContentPolicy(message)
    } else {
        failure(message)
    }
}

/// Reads a DashScope answer; error statuses keep their billing code, the rest map by HTTP status.
async fn answer(resp: reqwest::Response) -> ProviderResult<Value> {
    let status = resp.status();
    if status.is_success() {
        return checked(pictesa_core::json_response(resp).await?);
    }
    let retry_after = resp
        .headers()
        .get(reqwest::header::RETRY_AFTER)
        .and_then(|v| v.to_str().ok())
        .and_then(|s| s.parse::<f64>().ok())
        .map(|s| (s * 1000.0) as u64);
    let text = resp.text().await.map_err(net_error)?;
    let v: Value = serde_json::from_str(&text).unwrap_or_default();
    Err(match v.get("code").and_then(Value::as_str) {
        Some(code) if BILLING.iter().any(|c| code.contains(c)) => {
            ProviderError::InsufficientCredits(v["message"].as_str().unwrap_or(code).to_string())
        }
        _ => ProviderError::from_http(status.as_u16(), &text, retry_after),
    })
}

async fn call(req: reqwest::RequestBuilder) -> ProviderResult<Value> {
    answer(req.timeout(API_TIMEOUT).send().await.map_err(net_error)?).await
}

fn video_size(ratio: &str, resolution: &str) -> &'static str {
    match (ratio, resolution) {
        ("9:16", "1080P") => "1080*1920",
        ("1:1", "1080P") => "1440*1440",
        ("9:16", _) => "720*1280",
        ("1:1", _) => "960*960",
        (_, "1080P") => "1920*1080",
        _ => "1280*720",
    }
}

async fn body(model: &ModelSpec, req: &GenerationRequest) -> ProviderResult<Value> {
    let id = model.remote_id();
    if model.output == MediaKind::Image {
        let mut content = vec![json!({"text":req.prompt})];
        for input in req.inputs(InputRole::Reference) {
            content.push(json!({"image":input.url_or_data_uri().await?}));
        }
        let mut b = json!({"model":id, "input":{"messages":[{"role":"user","content":content}]}, "parameters":{"n":req.num_outputs}});
        if let Some(size) = req.str(keys::RESOLUTION) {
            b["parameters"]["size"] = json!(size);
        }
        if let Some(negative) = &req.negative_prompt {
            b["parameters"]["negative_prompt"] = json!(negative);
        }
        if let Some(seed) = req.u64(keys::SEED) {
            b["parameters"]["seed"] = json!(seed);
        }
        return Ok(b);
    }
    let mut b = json!({"model":id, "input":{"prompt":req.prompt}, "parameters":{}});
    if let Some(negative) = &req.negative_prompt {
        b["input"]["negative_prompt"] = json!(negative);
    }
    if let Some(start) = req.input(InputRole::StartFrame) {
        b["input"][if id.contains("kf2v") {
            "first_frame_url"
        } else {
            "img_url"
        }] = json!(start.url_or_data_uri().await?);
    }
    if let Some(end) = req.input(InputRole::EndFrame) {
        b["input"]["last_frame_url"] = json!(end.url_or_data_uri().await?);
    }
    if id.contains("t2v") {
        b["parameters"]["size"] = json!(video_size(
            req.str(keys::ASPECT_RATIO).unwrap_or("16:9"),
            req.str(keys::RESOLUTION).unwrap_or("720P")
        ));
    } else if let Some(r) = req.str(keys::RESOLUTION) {
        b["parameters"]["resolution"] = json!(r);
    }
    if let Some(d) = req.i64(keys::DURATION) {
        b["parameters"]["duration"] = json!(d);
    }
    if let Some(v) = req.bool(keys::ENHANCE_PROMPT) {
        b["parameters"]["prompt_extend"] = json!(v);
    }
    Ok(b)
}

fn parse_task(v: &Value, kind: MediaKind) -> ProviderResult<PollStatus> {
    let out = v
        .get("output")
        .ok_or_else(|| ProviderError::Failed("DashScope returned no output".into()))?;
    match out.get("task_status").and_then(Value::as_str).unwrap_or("") {
        "PENDING" => Ok(PollStatus::Queued { position: None }),
        "RUNNING" | "SUSPENDED" => Ok(PollStatus::Running {
            progress: None,
            message: None,
        }),
        // The failure's code and message sit in `output`
        // (https://www.alibabacloud.com/help/en/model-studio/text-to-video-api-reference).
        "FAILED" | "CANCELED" => Ok(PollStatus::Failed(code_error(
            out.get("code").or_else(|| v.get("code")).and_then(Value::as_str).unwrap_or(""),
            out.get("message")
                .or_else(|| v.get("message"))
                .and_then(Value::as_str)
                .unwrap_or("DashScope task failed")
                .to_string(),
        ))),
        "SUCCEEDED" => {
            let urls: Vec<&str> = if kind == MediaKind::Video {
                out.get("video_url")
                    .and_then(Value::as_str)
                    .into_iter()
                    .collect()
            } else if let Some(choices) = out.get("choices").and_then(Value::as_array) {
                choices
                    .iter()
                    .flat_map(|c| {
                        c.pointer("/message/content")
                            .and_then(Value::as_array)
                            .into_iter()
                            .flatten()
                    })
                    .filter_map(|c| c.get("image").and_then(Value::as_str))
                    .collect()
            } else {
                out.get("results")
                    .and_then(Value::as_array)
                    .into_iter()
                    .flatten()
                    .filter_map(|r| r.get("url").and_then(Value::as_str))
                    .collect()
            };
            if urls.is_empty() {
                return Err(ProviderError::Failed(
                    "DashScope succeeded without media URL".into(),
                ));
            }
            Ok(PollStatus::Done(
                urls.into_iter().map(Output::url).collect(),
            ))
        }
        other => Err(ProviderError::Failed(format!(
            "unknown DashScope task state: {other}"
        ))),
    }
}

#[async_trait]
impl Provider for Alibaba {
    fn info(&self) -> ProviderInfo {
        ProviderInfo {
            id: "alibaba".into(),
            name: "Alibaba Model Studio".into(),
            kind: ProviderKind::Cloud,
            fields: vec![
                CredentialField::api_key(&["DASHSCOPE_API_KEY"]),
                CredentialField::base_url(BASE, &["DASHSCOPE_BASE_URL"]),
            ],
            signup_url: Some("https://modelstudio.console.aliyun.com/".into()),
            docs_url: Some("https://help.aliyun.com/en/model-studio/".into()),
            max_concurrency: 4,
        }
    }
    fn models(&self) -> Vec<ModelSpec> {
        vec![
            ModelSpec::new(
                "alibaba",
                "wan2.6-t2v",
                "Wan 2.6 Text to Video",
                MediaKind::Video,
                &[Task::TextToVideo],
            )
            .param(common::aspect_ratio(&["16:9", "9:16", "1:1"], "16:9"))
            .param(common::resolution(&["720P", "1080P"], "720P"))
            .param(ParamSpec::int(keys::DURATION, "Duration (seconds)", 2, 15).default_value(5))
            .param(ParamSpec::boolean(keys::ENHANCE_PROMPT, "Enhance prompt")),
            ModelSpec::new(
                "alibaba",
                "wan2.6-i2v",
                "Wan 2.6 Image to Video",
                MediaKind::Video,
                &[Task::ImageToVideo],
            )
            .input(InputSlot::new(InputRole::StartFrame, MediaKind::Image, 1).required())
            .param(common::resolution(&["720P", "1080P"], "720P"))
            .param(common::duration(&[5, 10], 5)),
            ModelSpec::new(
                "alibaba",
                "wan2.2-kf2v-flash",
                "Wan 2.2 Keyframes",
                MediaKind::Video,
                &[Task::ImageToVideo],
            )
            .input(InputSlot::new(InputRole::StartFrame, MediaKind::Image, 1).required())
            .input(InputSlot::new(InputRole::EndFrame, MediaKind::Image, 1).required())
            .param(common::resolution(&["480P", "720P", "1080P"], "720P")),
            ModelSpec::new(
                "alibaba",
                "wan2.6-t2i",
                "Wan 2.6 Image",
                MediaKind::Image,
                &[Task::TextToImage],
            )
            .param(common::resolution(&["1280*1280", "1440*1440"], "1280*1280"))
            .max_outputs(4),
            ModelSpec::new(
                "alibaba",
                "qwen-image-3.0-pro",
                "Qwen Image 3 Pro",
                MediaKind::Image,
                &[Task::TextToImage, Task::ImageEdit],
            )
            .input(InputSlot::new(InputRole::Reference, MediaKind::Image, 3))
            .param(common::resolution(
                &["1024*1024", "1536*1024", "1024*1536", "2048*2048"],
                "1024*1024",
            ))
            .max_outputs(6),
        ]
    }
    async fn check(&self, ctx: &Ctx) -> ProviderResult<()> {
        let url = format!("{}/tasks/00000000-0000-0000-0000-000000000000", base(ctx));
        let resp = ctx
            .http
            .get(url)
            .bearer_auth(ctx.api_key()?)
            .send()
            .await
            .map_err(net_error)?;
        if resp.status() == reqwest::StatusCode::NOT_FOUND {
            return Ok(());
        }
        answer(resp).await.map(|_| ())
    }
    async fn submit(
        &self,
        ctx: &Ctx,
        model: &ModelSpec,
        req: &GenerationRequest,
    ) -> ProviderResult<Submission> {
        let id = model.remote_id();
        // Only the keyframe models keep the image2video path; Wan text and first-frame video share one.
        let path = if model.output == MediaKind::Image {
            "services/aigc/image-generation/generation"
        } else if id.contains("kf2v") {
            "services/aigc/image2video/video-synthesis"
        } else {
            "services/aigc/video-generation/video-synthesis"
        };
        let answer = call(
            ctx.http
                .post(format!("{}/{path}", base(ctx)))
                .bearer_auth(ctx.api_key()?)
                .header("X-DashScope-Async", "enable")
                .json(&body(model, req).await?),
        )
        .await?;
        let task = answer
            .pointer("/output/task_id")
            .and_then(Value::as_str)
            .ok_or_else(|| {
                ProviderError::Failed(format!(
                    "DashScope returned no task id: {}",
                    crate::util::error_message(&answer).unwrap_or_default()
                ))
            })?;
        Ok(Submission::Pending(RemoteHandle::with(
            task,
            json!({"kind":model.output}),
        )))
    }
    async fn poll(&self, ctx: &Ctx, handle: &RemoteHandle) -> ProviderResult<PollStatus> {
        let v = call(
            ctx.http
                .get(format!("{}/tasks/{}", base(ctx), handle.id))
                .bearer_auth(ctx.api_key()?),
        )
        .await?;
        let kind = handle
            .data
            .get("kind")
            .and_then(|v| serde_json::from_value::<MediaKind>(v.clone()).ok())
            .unwrap_or(MediaKind::Video);
        parse_task(&v, kind)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[tokio::test]
    async fn maps_wan_video_size_and_qwen_prompt() {
        let mut req = GenerationRequest {
            job_id: "x".into(),
            model: "wan2.6-t2v".into(),
            prompt: "a cat".into(),
            negative_prompt: None,
            params: serde_json::Map::new(),
            inputs: vec![],
            num_outputs: 1,
        };
        req.params.insert(keys::ASPECT_RATIO.into(), json!("9:16"));
        req.params.insert(keys::RESOLUTION.into(), json!("1080P"));
        let models = Alibaba.models();
        let b = body(&models[0], &req).await.unwrap();
        assert_eq!(b["parameters"]["size"], "1080*1920");
        req.model = "qwen-image-3.0-pro".into();
        req.params.clear();
        let b = body(models.last().unwrap(), &req).await.unwrap();
        assert_eq!(b["input"]["messages"][0]["content"][0]["text"], "a cat");
    }
    #[test]
    fn recorded_task_shapes() {
        assert!(matches!(
            checked(json!({"code":"Throttling.RateQuota","message":"try later"})),
            Err(ProviderError::RateLimited { .. })
        ));
        assert!(matches!(parse_task(&json!({"output":{"task_status":"SUCCEEDED","video_url":"https://example.com/a.mp4"}}), MediaKind::Video).unwrap(), PollStatus::Done(_)));
        assert!(matches!(parse_task(&json!({"output":{"task_status":"SUCCEEDED","choices":[{"message":{"content":[{"image":"https://example.com/a.png"}]}}]}}), MediaKind::Image).unwrap(), PollStatus::Done(_)));
        assert!(matches!(
            parse_task(
                &json!({"output":{"task_status":"FAILED"},"message":"safety violation"}),
                MediaKind::Image
            )
            .unwrap(),
            PollStatus::Failed(ProviderError::ContentPolicy(_))
        ));
    }
}
