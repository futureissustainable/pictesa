use async_trait::async_trait;
use base64::Engine as _;
use pictesa_core::{
    common, keys, CredentialField, Ctx, GenerationRequest, InputRole, InputSlot, MediaKind,
    ModelSpec, Output, PollStatus, Provider, ProviderError, ProviderInfo, ProviderKind,
    ProviderResult, RemoteHandle, Submission, Task,
};
use serde_json::{json, Value};

use crate::util::{failure, get_json, send_json};

const BASE: &str = "https://api.minimax.io";
pub struct Minimax;

fn checked(v: Value) -> ProviderResult<Value> {
    let code = v
        .pointer("/base_resp/status_code")
        .and_then(Value::as_i64)
        .unwrap_or(0);
    if code == 0 {
        return Ok(v);
    }
    let message = v
        .pointer("/base_resp/status_msg")
        .and_then(Value::as_str)
        .unwrap_or("MiniMax API error")
        .to_string();
    Err(match code {
        1002 => ProviderError::RateLimited {
            retry_after_ms: None,
        },
        1004 => ProviderError::Auth(message),
        1008 => ProviderError::InsufficientCredits(message),
        _ => failure(message),
    })
}

async fn body(model: &ModelSpec, req: &GenerationRequest) -> ProviderResult<Value> {
    let id = model.remote_id();
    if id == "image-01" {
        let mut b = json!({"model": id, "prompt": req.prompt, "response_format":"base64", "n": req.num_outputs});
        if let Some(ratio) = req.str(keys::ASPECT_RATIO) {
            b["aspect_ratio"] = json!(ratio);
        }
        if let Some(input) = req.input(InputRole::Reference) {
            b["subject_reference"] =
                json!([{"type":"character", "image_file": input.url_or_data_uri().await?}]);
        }
        return Ok(b);
    }
    if id.starts_with("MiniMax-H3") {
        let mut content = vec![json!({"type":"text","text":req.prompt})];
        for (role, tag) in [
            (InputRole::StartFrame, "first_frame"),
            (InputRole::EndFrame, "last_frame"),
        ] {
            if let Some(i) = req.input(role) {
                content.push(json!({"type":"image_url", "image_url":{"url":i.url_or_data_uri().await?}, "role":tag}));
            }
        }
        let mut b = json!({"model":id, "content":content});
        if let Some(d) = req.i64(keys::DURATION) {
            b["duration"] = json!(d);
        }
        if let Some(r) = req.str(keys::RESOLUTION) {
            b["resolution"] = json!(r);
        }
        if req.input(InputRole::StartFrame).is_none() {
            if let Some(r) = req.str(keys::ASPECT_RATIO) {
                b["ratio"] = json!(r);
            }
        }
        return Ok(b);
    }
    let mut b = json!({"model":id, "prompt":req.prompt});
    if let Some(i) = req.input(InputRole::StartFrame) {
        b["first_frame_image"] = json!(i.url_or_data_uri().await?);
    }
    if let Some(i) = req.input(InputRole::EndFrame) {
        b["last_frame_image"] = json!(i.url_or_data_uri().await?);
    }
    if let Some(d) = req.i64(keys::DURATION) {
        b["duration"] = json!(d);
    }
    if let Some(r) = req.str(keys::RESOLUTION) {
        b["resolution"] = json!(r);
    }
    if let Some(r) = req.str(keys::ASPECT_RATIO) {
        b["aspect_ratio"] = json!(r);
    }
    Ok(b)
}

fn image_outputs(v: &Value) -> ProviderResult<Vec<Output>> {
    let images = v
        .pointer("/data/image_base64")
        .and_then(Value::as_array)
        .ok_or_else(|| ProviderError::Failed("MiniMax image response has no images".into()))?;
    images
        .iter()
        .map(|v| {
            let s = v
                .as_str()
                .ok_or_else(|| ProviderError::Failed("MiniMax image is not base64".into()))?;
            let data = base64::engine::general_purpose::STANDARD
                .decode(s)
                .map_err(|_| ProviderError::Failed("MiniMax image has invalid base64".into()))?;
            Ok(Output::Bytes {
                data,
                mime: "image/jpeg".into(),
            })
        })
        .collect()
}

fn parse_v2(v: &Value) -> ProviderResult<PollStatus> {
    let task = v.get("task").unwrap_or(v);
    match task.get("status").and_then(Value::as_str).unwrap_or("") {
        "queued" => Ok(PollStatus::Queued { position: None }),
        "running" => Ok(PollStatus::Running {
            progress: None,
            message: None,
        }),
        "succeeded" => {
            let url = task
                .pointer("/content/url")
                .and_then(Value::as_str)
                .ok_or_else(|| {
                    ProviderError::Failed("MiniMax H3 succeeded without content URL".into())
                })?;
            Ok(PollStatus::Done(vec![Output::url(url)]))
        }
        "failed" => Ok(PollStatus::Failed(failure(
            task.pointer("/error/message")
                .and_then(Value::as_str)
                .unwrap_or("MiniMax H3 task failed"),
        ))),
        other => Err(ProviderError::Failed(format!(
            "unknown MiniMax H3 state: {other}"
        ))),
    }
}

#[async_trait]
impl Provider for Minimax {
    fn info(&self) -> ProviderInfo {
        ProviderInfo {
            id: "minimax".into(),
            name: "MiniMax".into(),
            kind: ProviderKind::Cloud,
            fields: vec![CredentialField::api_key(&["MINIMAX_API_KEY"])],
            signup_url: Some("https://platform.minimax.io/".into()),
            docs_url: Some("https://platform.minimax.io/docs/guides/video-generation".into()),
            max_concurrency: 3,
        }
    }
    fn models(&self) -> Vec<ModelSpec> {
        let mut v = Vec::new();
        for (id, name) in [("MiniMax-H3", "H3"), ("MiniMax-H3-Max", "H3 Max")] {
            v.push(
                ModelSpec::new(
                    "minimax",
                    id,
                    name,
                    MediaKind::Video,
                    &[Task::TextToVideo, Task::ImageToVideo],
                )
                .input(InputSlot::new(InputRole::StartFrame, MediaKind::Image, 1))
                .input(InputSlot::new(InputRole::EndFrame, MediaKind::Image, 1))
                .param(common::aspect_ratio(
                    &["16:9", "9:16", "1:1", "4:3", "3:4"],
                    "16:9",
                ))
                .param(common::resolution(
                    if id == "MiniMax-H3-Max" {
                        &["480P", "768P"]
                    } else {
                        &["768P", "2K"]
                    },
                    "768P",
                ))
                .param(
                    pictesa_core::ParamSpec::int(
                        keys::DURATION,
                        "Duration (seconds)",
                        if id == "MiniMax-H3-Max" { 5 } else { 4 },
                        15,
                    )
                    .default_value(5),
                ),
            );
        }
        for (id, name, required) in [
            ("MiniMax-Hailuo-2.3", "Hailuo 2.3", false),
            ("MiniMax-Hailuo-2.3-Fast", "Hailuo 2.3 Fast", true),
        ] {
            v.push(
                ModelSpec::new(
                    "minimax",
                    id,
                    name,
                    MediaKind::Video,
                    if required {
                        &[Task::ImageToVideo]
                    } else {
                        &[Task::TextToVideo, Task::ImageToVideo]
                    },
                )
                .input(if required {
                    InputSlot::new(InputRole::StartFrame, MediaKind::Image, 1).required()
                } else {
                    InputSlot::new(InputRole::StartFrame, MediaKind::Image, 1)
                })
                .param(common::duration(&[6, 10], 6))
                .param(common::resolution(&["768P", "1080P"], "768P")),
            );
        }
        v.push(
            ModelSpec::new(
                "minimax",
                "MiniMax-Hailuo-02",
                "Hailuo 02 First and Last",
                MediaKind::Video,
                &[Task::ImageToVideo],
            )
            .input(InputSlot::new(InputRole::StartFrame, MediaKind::Image, 1).required())
            .input(InputSlot::new(InputRole::EndFrame, MediaKind::Image, 1))
            .param(common::duration(&[6, 10], 6))
            .param(common::resolution(&["768P", "1080P"], "768P")),
        );
        v.push(
            ModelSpec::new(
                "minimax",
                "image-01",
                "Image 01",
                MediaKind::Image,
                &[Task::TextToImage],
            )
            .input(InputSlot::new(InputRole::Reference, MediaKind::Image, 1))
            .param(common::aspect_ratio(
                &["1:1", "16:9", "9:16", "4:3", "3:4", "3:2", "2:3"],
                "1:1",
            ))
            .max_outputs(4),
        );
        v
    }
    async fn check(&self, ctx: &Ctx) -> ProviderResult<()> {
        get_json(
            ctx.http
                .get(format!("{BASE}/v2/query/video_generation"))
                .bearer_auth(ctx.api_key()?),
        )
        .await?;
        Ok(())
    }
    async fn submit(
        &self,
        ctx: &Ctx,
        model: &ModelSpec,
        req: &GenerationRequest,
    ) -> ProviderResult<Submission> {
        let id = model.remote_id();
        let path = if id == "image-01" {
            "/v1/image_generation"
        } else if id.starts_with("MiniMax-H3") {
            "/v2/video_generation"
        } else {
            "/v1/video_generation"
        };
        let ans = checked(
            send_json(
                ctx.http
                    .post(format!("{BASE}{path}"))
                    .bearer_auth(ctx.api_key()?),
                &body(model, req).await?,
            )
            .await?,
        )?;
        if id == "image-01" {
            return Ok(Submission::Done(image_outputs(&ans)?));
        }
        let task = ans
            .get("task_id")
            .and_then(Value::as_str)
            .ok_or_else(|| ProviderError::Failed("MiniMax returned no task id".into()))?;
        Ok(Submission::Pending(RemoteHandle::with(
            task,
            json!({"v2":id.starts_with("MiniMax-H3")}),
        )))
    }
    async fn poll(&self, ctx: &Ctx, handle: &RemoteHandle) -> ProviderResult<PollStatus> {
        let key = ctx.api_key()?;
        if handle.data.get("v2").and_then(Value::as_bool) == Some(true) {
            let v = get_json(
                ctx.http
                    .get(format!("{BASE}/v2/query/video_generation/{}", handle.id))
                    .bearer_auth(key),
            )
            .await?;
            return parse_v2(&v);
        }
        let v = checked(
            get_json(
                ctx.http
                    .get(format!("{BASE}/v1/query/video_generation"))
                    .query(&[("task_id", &handle.id)])
                    .bearer_auth(key),
            )
            .await?,
        )?;
        match v.get("status").and_then(Value::as_str).unwrap_or("") {
            "Preparing" | "Queueing" => Ok(PollStatus::Queued { position: None }),
            "Processing" => Ok(PollStatus::Running {
                progress: None,
                message: None,
            }),
            "Fail" => Ok(PollStatus::Failed(failure(
                v.pointer("/base_resp/status_msg")
                    .and_then(Value::as_str)
                    .unwrap_or("MiniMax task failed"),
            ))),
            "Success" => {
                let file = v.get("file_id").and_then(Value::as_str).ok_or_else(|| {
                    ProviderError::Failed("MiniMax succeeded without file id".into())
                })?;
                let out = checked(
                    get_json(
                        ctx.http
                            .get(format!("{BASE}/v1/files/retrieve"))
                            .query(&[("file_id", file)])
                            .bearer_auth(key),
                    )
                    .await?,
                )?;
                let url = out
                    .pointer("/file/download_url")
                    .and_then(Value::as_str)
                    .ok_or_else(|| {
                        ProviderError::Failed("MiniMax file has no download URL".into())
                    })?;
                Ok(PollStatus::Done(vec![Output::url(url)]))
            }
            other => Err(ProviderError::Failed(format!(
                "unknown MiniMax state: {other}"
            ))),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use pictesa_core::MediaInput;
    use std::path::PathBuf;
    #[tokio::test]
    async fn h3_maps_frame_roles() {
        let model = Minimax.models().remove(0);
        let req = GenerationRequest {
            job_id: "x".into(),
            model: "MiniMax-H3".into(),
            prompt: "move".into(),
            negative_prompt: None,
            params: serde_json::Map::new(),
            inputs: vec![
                MediaInput {
                    role: InputRole::StartFrame,
                    kind: MediaKind::Image,
                    mime: "image/png".into(),
                    path: PathBuf::new(),
                    url: Some("https://example.com/a.png".into()),
                    width: None,
                    height: None,
                },
                MediaInput {
                    role: InputRole::EndFrame,
                    kind: MediaKind::Image,
                    mime: "image/png".into(),
                    path: PathBuf::new(),
                    url: Some("https://example.com/b.png".into()),
                    width: None,
                    height: None,
                },
            ],
            num_outputs: 1,
        };
        let b = body(&model, &req).await.unwrap();
        assert_eq!(b["content"][1]["role"], "first_frame");
        assert_eq!(b["content"][2]["role"], "last_frame");
        assert!(b.get("ratio").is_none());
    }
    #[test]
    fn recorded_responses() {
        assert!(matches!(
            checked(json!({"base_resp":{"status_code":1002,"status_msg":"rate limit"}})),
            Err(ProviderError::RateLimited { .. })
        ));
        assert!(matches!(parse_v2(&json!({"task":{"status":"succeeded","content":{"url":"https://example.com/v.mp4"}}})).unwrap(), PollStatus::Done(_)));
        assert!(matches!(
            parse_v2(&json!({"task":{"status":"failed","error":{"message":"sensitive content"}}}))
                .unwrap(),
            PollStatus::Failed(ProviderError::ContentPolicy(_))
        ));
        assert!(
            matches!(image_outputs(&json!({"data":{"image_base64":["AQID"]}})).unwrap().remove(0), Output::Bytes { data, .. } if data == [1,2,3])
        );
    }
}
