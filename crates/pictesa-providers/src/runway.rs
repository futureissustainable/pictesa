use async_trait::async_trait;
use pictesa_core::{
    common, keys, CredentialField, Ctx, GenerationRequest, InputRole, InputSlot, MediaInput,
    MediaKind, ModelSpec, Output, ParamSpec, PollStatus, Pricing, Provider, ProviderError,
    ProviderInfo, ProviderKind, ProviderResult, RemoteHandle, Submission, Task,
};
use serde_json::{json, Value};

use crate::util::{failure, file_size, get_json, net_error, send_json};

const BASE: &str = "https://api.dev.runwayml.com/v1";
const VERSION: &str = "2024-11-06";

pub struct Runway;

fn auth(ctx: &Ctx, req: reqwest::RequestBuilder) -> ProviderResult<reqwest::RequestBuilder> {
    Ok(req
        .bearer_auth(ctx.api_key()?)
        .header("X-Runway-Version", VERSION))
}

async fn media_uri(ctx: &Ctx, input: &MediaInput) -> ProviderResult<String> {
    if let Some(url) = &input.url {
        return Ok(url.clone());
    }
    if file_size(input) <= 3 * 1024 * 1024 {
        return input.data_uri().await;
    }
    let upload = send_json(
        auth(ctx, ctx.http.post(format!("{BASE}/uploads")))?,
        &json!({"filename":input.file_name(),"type":"ephemeral"}),
    )
    .await?;
    let url = upload
        .get("uploadUrl")
        .and_then(Value::as_str)
        .ok_or_else(|| ProviderError::Failed("Runway upload has no URL".into()))?;
    let uri = upload
        .get("runwayUri")
        .and_then(Value::as_str)
        .ok_or_else(|| ProviderError::Failed("Runway upload has no URI".into()))?;
    let fields = upload
        .get("fields")
        .and_then(Value::as_object)
        .ok_or_else(|| ProviderError::Failed("Runway upload has no form fields".into()))?;
    let mut form = reqwest::multipart::Form::new();
    for (k, v) in fields {
        let val = v
            .as_str()
            .ok_or_else(|| ProviderError::Failed("Runway upload field is not text".into()))?;
        form = form.text(k.to_owned(), val.to_owned());
    }
    let file = tokio::fs::File::open(&input.path)
        .await
        .map_err(|e| ProviderError::InvalidInput(format!("cannot read input: {e}")))?;
    let size = file_size(input);
    let stream = futures::stream::try_unfold(file, |mut file| async move {
        use tokio::io::AsyncReadExt;
        let mut chunk = vec![0u8; 64 * 1024];
        let n = file.read(&mut chunk).await?;
        if n == 0 {
            Ok::<_, std::io::Error>(None)
        } else {
            chunk.truncate(n);
            Ok(Some((bytes::Bytes::from(chunk), file)))
        }
    });
    let part =
        reqwest::multipart::Part::stream_with_length(reqwest::Body::wrap_stream(stream), size)
            .file_name(input.file_name())
            .mime_str(&input.mime)
            .map_err(|_| ProviderError::InvalidInput("invalid input MIME type".into()))?;
    let response = ctx
        .http
        .post(url)
        .multipart(form.part("file", part))
        .send()
        .await
        .map_err(net_error)?;
    if !response.status().is_success() {
        let status = response.status();
        let message = response.text().await.map_err(net_error)?;
        return Err(ProviderError::from_http(status.as_u16(), &message, None));
    }
    Ok(uri.to_owned())
}

async fn body(ctx: &Ctx, model: &ModelSpec, req: &GenerationRequest) -> ProviderResult<Value> {
    let mut b = json!({"model": model.remote_id(), "promptText": req.prompt});
    if model.remote_id() == "magnific_video_upscaler_creative" {
        let video = req
            .input(InputRole::Video)
            .ok_or_else(|| ProviderError::InvalidInput("source video is required".into()))?;
        b.as_object_mut().unwrap().remove("promptText");
        b["videoUri"] = json!(media_uri(ctx, video).await?);
        if let Some(resolution) = req.str(keys::RESOLUTION) {
            b["resolution"] = json!(resolution);
        }
    } else if model.output == MediaKind::Video {
        let ratio = req.str(keys::ASPECT_RATIO).unwrap_or("16:9");
        if model.remote_id() == "gen4.5"
            && req.input(InputRole::StartFrame).is_none()
            && ratio == "1:1"
        {
            return Err(ProviderError::InvalidInput(
                "Gen-4.5 text to video does not support square output".into(),
            ));
        }
        b["ratio"] = json!(match ratio {
            "9:16" => "720:1280",
            "1:1" => "960:960",
            _ => "1280:720",
        });
        b["duration"] = json!(req.i64(keys::DURATION).unwrap_or(5));
        if let Some(image) = req.input(InputRole::StartFrame) {
            b["promptImage"] = json!(media_uri(ctx, image).await?);
        }
    } else {
        b["ratio"] = json!(req.str(keys::RESOLUTION).unwrap_or("1360:768"));
        if let Some(seed) = req.u64(keys::SEED) {
            b["seed"] = json!(seed);
        }
        let mut refs = Vec::new();
        for (i, input) in req.inputs(InputRole::Reference).enumerate() {
            refs.push(json!({"uri": media_uri(ctx, input).await?, "tag": format!("ref{i}")}));
        }
        if !refs.is_empty() {
            b["referenceImages"] = json!(refs);
        }
    }
    Ok(b)
}

fn parse_task(v: &Value) -> ProviderResult<PollStatus> {
    match v.get("status").and_then(Value::as_str).unwrap_or("") {
        "PENDING" | "THROTTLED" => Ok(PollStatus::Queued { position: None }),
        "RUNNING" => Ok(PollStatus::Running {
            progress: None,
            message: None,
        }),
        "SUCCEEDED" => {
            let urls: Vec<Output> = v
                .get("output")
                .and_then(Value::as_array)
                .into_iter()
                .flatten()
                .filter_map(Value::as_str)
                .map(Output::url)
                .collect();
            if urls.is_empty() {
                return Err(ProviderError::Failed(
                    "Runway task succeeded without output".into(),
                ));
            }
            Ok(PollStatus::Done(urls))
        }
        "FAILED" | "CANCELLED" | "ABORTED" => Ok(PollStatus::Failed(failure(
            v.get("failure")
                .and_then(Value::as_str)
                .or_else(|| v.get("failureCode").and_then(Value::as_str))
                .unwrap_or("Runway task failed"),
        ))),
        other => Err(ProviderError::Failed(format!(
            "unknown Runway task status: {other}"
        ))),
    }
}

#[async_trait]
impl Provider for Runway {
    fn info(&self) -> ProviderInfo {
        ProviderInfo {
            id: "runway".into(),
            name: "Runway".into(),
            kind: ProviderKind::Cloud,
            fields: vec![CredentialField::api_key(&[
                "RUNWAYML_API_SECRET",
                "RUNWAY_API_KEY",
            ])],
            signup_url: Some("https://dev.runwayml.com/".into()),
            docs_url: Some("https://docs.dev.runwayml.com/api/".into()),
            max_concurrency: 4,
        }
    }
    fn models(&self) -> Vec<ModelSpec> {
        let mut v = Vec::new();
        for (id, name, t2v) in [
            ("gen4.5", "Gen-4.5", true),
            ("gen4_turbo", "Gen-4 Turbo", false),
        ] {
            let mut m = ModelSpec::new(
                "runway",
                id,
                name,
                MediaKind::Video,
                if t2v {
                    &[Task::TextToVideo, Task::ImageToVideo]
                } else {
                    &[Task::ImageToVideo]
                },
            )
            .input(if t2v {
                InputSlot::new(InputRole::StartFrame, MediaKind::Image, 1)
            } else {
                InputSlot::new(InputRole::StartFrame, MediaKind::Image, 1).required()
            })
            .param(common::aspect_ratio(&["16:9", "9:16", "1:1"], "16:9"))
            .param(if t2v {
                ParamSpec::int(keys::DURATION, "Duration (seconds)", 2, 10).default_value(5)
            } else {
                common::duration(&[5, 10], 5)
            })
            .pricing(Pricing::PerSecond {
                usd: if t2v { 0.12 } else { 0.05 },
            });
            if t2v {
                m.description =
                    Some("Text mode supports 16:9 and 9:16; square needs a start frame.".into());
            }
            v.push(m);
        }
        for (id, name, requires_ref) in [
            ("gen4_image", "Gen-4 Image", false),
            ("gen4_image_turbo", "Gen-4 Image Turbo", true),
        ] {
            let mut m = ModelSpec::new(
                "runway",
                id,
                name,
                MediaKind::Image,
                if requires_ref {
                    &[Task::ImageEdit]
                } else {
                    &[Task::TextToImage, Task::ImageEdit]
                },
            )
            .param(
                ParamSpec::choices(
                    keys::RESOLUTION,
                    "Size",
                    &[
                        "1360:768",
                        "1080:1440",
                        "1440:1080",
                        "1080:1920",
                        "1920:1080",
                        "1080:1080",
                    ],
                )
                .default_value("1360:768"),
            )
            .param(ParamSpec::seed())
            .pricing(if requires_ref {
                Pricing::PerImage { usd: 0.02 }
            } else {
                Pricing::Variable {
                    note: "$0.05 at 720p, $0.08 at 1080p".into(),
                }
            });
            m = m.input(if requires_ref {
                InputSlot::new(InputRole::Reference, MediaKind::Image, 3).required()
            } else {
                InputSlot::new(InputRole::Reference, MediaKind::Image, 3)
            });
            v.push(m);
        }
        v.push(
            ModelSpec::new(
                "runway",
                "magnific_video_upscaler_creative",
                "Magnific Video Upscaler",
                MediaKind::Video,
                &[Task::Upscale],
            )
            .input(InputSlot::new(InputRole::Video, MediaKind::Video, 1).required())
            .no_prompt()
            .param(common::resolution(&["720p", "1k", "2k", "4k"], "2k"))
            .pricing(Pricing::Variable {
                note: "Per output frame: $0.007 at 720p/1K, $0.009 at 2K, $0.012 at 4K".into(),
            }),
        );
        v
    }
    async fn check(&self, ctx: &Ctx) -> ProviderResult<()> {
        get_json(auth(ctx, ctx.http.get(format!("{BASE}/organization")))?).await?;
        Ok(())
    }
    async fn submit(
        &self,
        ctx: &Ctx,
        model: &ModelSpec,
        req: &GenerationRequest,
    ) -> ProviderResult<Submission> {
        let path = if model.remote_id() == "magnific_video_upscaler_creative" {
            "video_upscale"
        } else if model.output == MediaKind::Video {
            "image_to_video"
        } else {
            "text_to_image"
        };
        let answer = send_json(
            auth(ctx, ctx.http.post(format!("{BASE}/{path}")))?,
            &body(ctx, model, req).await?,
        )
        .await?;
        let id = answer
            .get("id")
            .and_then(Value::as_str)
            .ok_or_else(|| ProviderError::Failed("Runway returned no task id".into()))?;
        Ok(Submission::Pending(RemoteHandle::new(id)))
    }
    async fn poll(&self, ctx: &Ctx, handle: &RemoteHandle) -> ProviderResult<PollStatus> {
        parse_task(
            &get_json(auth(
                ctx,
                ctx.http.get(format!("{BASE}/tasks/{}", handle.id)),
            )?)
            .await?,
        )
    }
    async fn cancel(&self, ctx: &Ctx, handle: &RemoteHandle) -> ProviderResult<()> {
        let resp = auth(ctx, ctx.http.delete(format!("{BASE}/tasks/{}", handle.id)))?
            .send()
            .await
            .map_err(crate::util::net_error)?;
        if resp.status().is_success() || resp.status() == reqwest::StatusCode::NOT_FOUND {
            Ok(())
        } else {
            Err(ProviderError::from_http(
                resp.status().as_u16(),
                "Runway cancellation failed",
                None,
            ))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use pictesa_core::MediaInput;
    use serde_json::Map;
    use std::{collections::HashMap, path::PathBuf};
    #[tokio::test]
    async fn maps_upscale_and_image_input() {
        let ctx = Ctx::new(reqwest::Client::new(), HashMap::new());
        let mut req = GenerationRequest {
            job_id: "x".into(),
            model: "gen4.5".into(),
            prompt: "move".into(),
            negative_prompt: None,
            params: Map::new(),
            inputs: vec![MediaInput {
                role: InputRole::StartFrame,
                kind: MediaKind::Image,
                mime: "image/png".into(),
                path: PathBuf::new(),
                url: Some("https://example.com/a.png".into()),
                width: None,
                height: None,
            }],
            num_outputs: 1,
        };
        let model = Runway
            .models()
            .into_iter()
            .find(|m| m.remote_id() == "gen4.5")
            .unwrap();
        let b = body(&ctx, &model, &req).await.unwrap();
        assert_eq!(b["promptImage"], "https://example.com/a.png");
        assert_eq!(b["ratio"], "1280:720");
        req.model = "magnific_video_upscaler_creative".into();
        req.inputs[0].role = InputRole::Video;
        req.inputs[0].kind = MediaKind::Video;
        let upscale = Runway
            .models()
            .into_iter()
            .find(|m| m.remote_id() == req.model)
            .unwrap();
        let b = body(&ctx, &upscale, &req).await.unwrap();
        assert_eq!(b["videoUri"], "https://example.com/a.png");
        assert!(b.get("ratio").is_none());
    }
    #[test]
    fn task_fixtures() {
        assert!(matches!(
            parse_task(&json!({"status":"PENDING"})).unwrap(),
            PollStatus::Queued { .. }
        ));
        assert!(
            matches!(parse_task(&json!({"status":"SUCCEEDED","output":["https://cdn.example/a.mp4"]})).unwrap(), PollStatus::Done(v) if v.len() == 1)
        );
        assert!(matches!(
            parse_task(&json!({"status":"FAILED","failure":"safety check"})).unwrap(),
            PollStatus::Failed(ProviderError::ContentPolicy(_))
        ));
    }
}
