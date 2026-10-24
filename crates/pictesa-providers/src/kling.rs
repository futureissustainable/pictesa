use std::time::{SystemTime, UNIX_EPOCH};

use async_trait::async_trait;
use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine as _};
use hmac::{Hmac, Mac};
use pictesa_core::{
    common, keys, CredentialField, Ctx, GenerationRequest, InputRole, InputSlot, MediaInput,
    MediaKind, ModelSpec, Output, ParamSpec, PollStatus, Provider, ProviderError, ProviderInfo,
    ProviderKind, ProviderResult, RemoteHandle, Submission, Task,
};
use serde_json::{json, Value};
use sha2::Sha256;

use crate::util::{failure, get_json, send_json};

const BASE: &str = "https://api-singapore.klingai.com";
pub struct Kling;

fn token(access: &str, secret: &str, now: u64) -> ProviderResult<String> {
    let head = URL_SAFE_NO_PAD.encode(br#"{"alg":"HS256","typ":"JWT"}"#);
    let payload = URL_SAFE_NO_PAD
        .encode(json!({"iss":access,"exp":now+1800,"nbf":now.saturating_sub(5)}).to_string());
    let signed = format!("{head}.{payload}");
    let mut mac = Hmac::<Sha256>::new_from_slice(secret.as_bytes())
        .map_err(|_| ProviderError::Auth("invalid Kling secret key".into()))?;
    mac.update(signed.as_bytes());
    Ok(format!(
        "{signed}.{}",
        URL_SAFE_NO_PAD.encode(mac.finalize().into_bytes())
    ))
}

fn auth(ctx: &Ctx) -> ProviderResult<String> {
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_err(|_| ProviderError::Failed("system clock is before epoch".into()))?
        .as_secs();
    token(ctx.require("access_key")?, ctx.require("secret_key")?, now)
}

fn checked(v: Value) -> ProviderResult<Value> {
    if v.get("code").and_then(Value::as_i64).unwrap_or(0) != 0 {
        let message = v
            .get("message")
            .and_then(Value::as_str)
            .unwrap_or("Kling API error");
        let lower = message.to_ascii_lowercase();
        if lower.contains("rate limit") || lower.contains("too many requests") {
            return Err(ProviderError::RateLimited {
                retry_after_ms: None,
            });
        }
        if lower.contains("invalid key")
            || lower.contains("invalid token")
            || lower.contains("unauthorized")
        {
            return Err(ProviderError::Auth(message.into()));
        }
        return Err(failure(message));
    }
    Ok(v)
}

async fn image(input: &MediaInput) -> ProviderResult<String> {
    match &input.url {
        Some(url) if url.starts_with("https://") => Ok(url.clone()),
        _ => input.base64().await,
    }
}

async fn body(model: &ModelSpec, req: &GenerationRequest) -> ProviderResult<Value> {
    let mut b = json!({"model_name":model.remote_id(), "prompt":req.prompt});
    if let Some(v) = &req.negative_prompt {
        b["negative_prompt"] = json!(v);
    }
    if let Some(v) = req.str(keys::ASPECT_RATIO) {
        b["aspect_ratio"] = json!(v);
    }
    if let Some(v) = req.str(keys::RESOLUTION) {
        b[if model.output == MediaKind::Video {
            "mode"
        } else {
            "resolution"
        }] = json!(v);
    }
    if model.output == MediaKind::Video {
        if let Some(v) = req.i64(keys::DURATION) {
            b["duration"] = json!(v.to_string());
        }
        if let Some(v) = req.bool(keys::GENERATE_AUDIO) {
            b["sound"] = json!(if v { "on" } else { "off" });
        }
        if let Some(v) = req.input(InputRole::StartFrame) {
            b["image"] = json!(image(v).await?);
        }
        if let Some(v) = req.input(InputRole::EndFrame) {
            b["image_tail"] = json!(image(v).await?);
        }
    } else {
        b["n"] = json!(req.num_outputs);
        if let Some(v) = req.input(InputRole::Reference) {
            b["image"] = json!(image(v).await?);
        }
    }
    Ok(b)
}

fn parse_task(v: &Value, kind: MediaKind) -> ProviderResult<PollStatus> {
    let d = v
        .get("data")
        .ok_or_else(|| ProviderError::Failed("Kling response has no data".into()))?;
    match d.get("task_status").and_then(Value::as_str).unwrap_or("") {
        "submitted" => Ok(PollStatus::Queued { position: None }),
        "processing" => Ok(PollStatus::Running {
            progress: None,
            message: None,
        }),
        "failed" => Ok(PollStatus::Failed(failure(
            d.get("task_status_msg")
                .and_then(Value::as_str)
                .unwrap_or("Kling task failed"),
        ))),
        "succeed" | "succeeded" => {
            let key = if kind == MediaKind::Video {
                "videos"
            } else {
                "images"
            };
            let urls: Vec<Output> = d
                .pointer(&format!("/task_result/{key}"))
                .and_then(Value::as_array)
                .into_iter()
                .flatten()
                .filter_map(|v| v.get("url").and_then(Value::as_str))
                .map(Output::url)
                .collect();
            if urls.is_empty() {
                return Err(ProviderError::Failed(
                    "Kling succeeded without media URLs".into(),
                ));
            }
            Ok(PollStatus::Done(urls))
        }
        other => Err(ProviderError::Failed(format!(
            "unknown Kling task state: {other}"
        ))),
    }
}

#[async_trait]
impl Provider for Kling {
    fn info(&self) -> ProviderInfo {
        ProviderInfo {
            id: "kling".into(),
            name: "Kling AI".into(),
            kind: ProviderKind::Cloud,
            fields: vec![
                CredentialField {
                    key: "access_key".into(),
                    label: "Access key".into(),
                    env: vec!["KLING_ACCESS_KEY".into()],
                    secret: true,
                    optional: false,
                    default: None,
                    placeholder: None,
                },
                CredentialField {
                    key: "secret_key".into(),
                    label: "Secret key".into(),
                    env: vec!["KLING_SECRET_KEY".into()],
                    secret: true,
                    optional: false,
                    default: None,
                    placeholder: None,
                },
            ],
            signup_url: Some("https://kling.ai/dev/api-key".into()),
            docs_url: Some("https://kling.ai/document-api/api/get-started/authentication".into()),
            max_concurrency: 4,
        }
    }
    fn models(&self) -> Vec<ModelSpec> {
        let mut v = Vec::new();
        for (id, name, max) in [
            ("kling-v3", "Kling 3.0", 15),
            ("kling-v2-6", "Kling 2.6", 10),
        ] {
            v.push(
                ModelSpec::new(
                    "kling",
                    id,
                    name,
                    MediaKind::Video,
                    &[Task::TextToVideo, Task::ImageToVideo],
                )
                .input(InputSlot::new(InputRole::StartFrame, MediaKind::Image, 1))
                .input(InputSlot::new(InputRole::EndFrame, MediaKind::Image, 1))
                .param(common::aspect_ratio(&["16:9", "9:16", "1:1"], "16:9"))
                .param(common::resolution(
                    if max == 15 {
                        &["std", "pro", "4k"]
                    } else {
                        &["std", "pro"]
                    },
                    "std",
                ))
                .param(
                    ParamSpec::int(keys::DURATION, "Duration (seconds)", 3, max).default_value(5),
                )
                .param(common::generate_audio(false)),
            );
        }
        v.push(
            ModelSpec::new(
                "kling",
                "kling-v2-1",
                "Kling Image 2.1",
                MediaKind::Image,
                &[Task::TextToImage, Task::ImageEdit],
            )
            .input(InputSlot::new(InputRole::Reference, MediaKind::Image, 1))
            .param(common::aspect_ratio(
                &["16:9", "9:16", "1:1", "4:3", "3:4", "3:2", "2:3", "21:9"],
                "1:1",
            ))
            .param(common::resolution(&["1k", "2k"], "1k"))
            .max_outputs(9),
        );
        v
    }
    async fn check(&self, ctx: &Ctx) -> ProviderResult<()> {
        let v = get_json(
            ctx.http
                .get(format!("{BASE}/v1/images/generations"))
                .query(&[("pageNum", "1"), ("pageSize", "1")])
                .bearer_auth(auth(ctx)?),
        )
        .await?;
        checked(v)?;
        Ok(())
    }
    async fn submit(
        &self,
        ctx: &Ctx,
        model: &ModelSpec,
        req: &GenerationRequest,
    ) -> ProviderResult<Submission> {
        let route = if model.output == MediaKind::Image {
            "images/generations"
        } else if req.input(InputRole::StartFrame).is_some()
            || req.input(InputRole::EndFrame).is_some()
        {
            "videos/image2video"
        } else {
            "videos/text2video"
        };
        let v = checked(
            send_json(
                ctx.http
                    .post(format!("{BASE}/v1/{route}"))
                    .bearer_auth(auth(ctx)?),
                &body(model, req).await?,
            )
            .await?,
        )?;
        let id = v
            .pointer("/data/task_id")
            .and_then(Value::as_str)
            .ok_or_else(|| ProviderError::Failed("Kling returned no task id".into()))?;
        Ok(Submission::Pending(RemoteHandle::with(
            id,
            json!({"route":route,"kind":model.output}),
        )))
    }
    async fn poll(&self, ctx: &Ctx, handle: &RemoteHandle) -> ProviderResult<PollStatus> {
        let route = handle
            .data_str("route")
            .ok_or_else(|| ProviderError::Failed("Kling handle has no route".into()))?;
        let kind = handle
            .data
            .get("kind")
            .and_then(|v| serde_json::from_value::<MediaKind>(v.clone()).ok())
            .unwrap_or(MediaKind::Video);
        let v = checked(
            get_json(
                ctx.http
                    .get(format!("{BASE}/v1/{route}/{}", handle.id))
                    .bearer_auth(auth(ctx)?),
            )
            .await?,
        )?;
        parse_task(&v, kind)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use pictesa_core::MediaInput;
    use std::path::PathBuf;
    #[test]
    fn jwt_and_task_fixtures() {
        let signed = token("access", "secret", 1_700_000_000).unwrap();
        assert_eq!(signed, "eyJhbGciOiJIUzI1NiIsInR5cCI6IkpXVCJ9.eyJleHAiOjE3MDAwMDE4MDAsImlzcyI6ImFjY2VzcyIsIm5iZiI6MTY5OTk5OTk5NX0.as6jODCc-mOoU62l6myejbnucRhrSAwkOe-wCm5Udmg");
        assert!(matches!(parse_task(&json!({"data":{"task_status":"succeed","task_result":{"videos":[{"url":"https://example.com/a.mp4"}]}}}), MediaKind::Video).unwrap(), PollStatus::Done(_)));
        assert!(matches!(
            parse_task(
                &json!({"data":{"task_status":"failed","task_status_msg":"safety issue"}}),
                MediaKind::Video
            )
            .unwrap(),
            PollStatus::Failed(ProviderError::ContentPolicy(_))
        ));
    }
    #[tokio::test]
    async fn maps_end_frame_and_audio() {
        let model = Kling.models().remove(0);
        let mut req = GenerationRequest {
            job_id: "x".into(),
            model: "kling-v3".into(),
            prompt: "move".into(),
            negative_prompt: None,
            params: serde_json::Map::new(),
            inputs: vec![],
            num_outputs: 1,
        };
        req.params.insert(keys::GENERATE_AUDIO.into(), json!(true));
        req.params.insert(keys::DURATION.into(), json!(8));
        req.inputs.push(MediaInput {
            role: InputRole::EndFrame,
            kind: MediaKind::Image,
            mime: "image/png".into(),
            path: PathBuf::new(),
            url: Some("https://example.com/last.png".into()),
            width: None,
            height: None,
        });
        let b = body(&model, &req).await.unwrap();
        assert_eq!(b["image_tail"], "https://example.com/last.png");
        assert_eq!(b["sound"], "on");
        assert_eq!(b["duration"], "8");
    }
}
