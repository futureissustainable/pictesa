use async_trait::async_trait;
use base64::Engine;
use pictesa_core::{
    keys, CredentialField, Ctx, GenerationRequest, InputRole, InputSlot, MediaKind, ModelSpec,
    Output, ParamSpec, PollStatus, Pricing, Provider, ProviderError, ProviderInfo, ProviderKind,
    ProviderResult, RemoteHandle, Submission, Task,
};
use reqwest::multipart::{Form, Part};
use serde_json::{json, Value};

use crate::util::{failure, get_json, net_error, send_json};

pub struct OpenAI;
const API: &str = "https://api.openai.com/v1";

fn image_spec(id: &str) -> ModelSpec {
    let mut m = ModelSpec::new(
        "openai",
        id,
        id,
        MediaKind::Image,
        &[Task::TextToImage, Task::ImageEdit],
    );
    m.vendor = Some("OpenAI".into());
    m.inputs = vec![
        InputSlot::new(InputRole::Reference, MediaKind::Image, 16),
        InputSlot::new(InputRole::Mask, MediaKind::Image, 1),
    ];
    m.params = vec![
        ParamSpec::choices(
            keys::ASPECT_RATIO,
            "Aspect ratio",
            if id.starts_with("gpt-image-2") {
                &["1:1", "3:2", "2:3", "16:9", "9:16"]
            } else {
                &["1:1", "3:2", "2:3"]
            },
        ),
        ParamSpec::choices(
            keys::QUALITY,
            "Quality",
            if id.contains("2.5") {
                &["low", "medium", "high", "xhigh", "max"]
            } else {
                &["low", "medium", "high"]
            },
        ),
        ParamSpec::choices(keys::OUTPUT_FORMAT, "Format", &["png", "jpeg", "webp"]),
    ];
    m.max_outputs = 10;
    m.pricing = Some(Pricing::Variable {
        note: "OpenAI image token pricing".into(),
    });
    m
}
fn video_spec(id: &str) -> ModelSpec {
    let mut m = ModelSpec::new(
        "openai",
        id,
        id,
        MediaKind::Video,
        &[Task::TextToVideo, Task::ImageToVideo],
    );
    m.vendor = Some("OpenAI".into());
    m.inputs = vec![InputSlot::new(InputRole::StartFrame, MediaKind::Image, 1)];
    m.params = vec![
        ParamSpec::choices(keys::DURATION, "Duration", &["4", "8", "12", "16", "20"]),
        ParamSpec::choices(
            keys::RESOLUTION,
            "Size",
            if id == "sora-2" {
                &["1280x720", "720x1280"]
            } else {
                &["1280x720", "720x1280", "1920x1080", "1080x1920"]
            },
        ),
    ];
    m.pricing = Some(Pricing::Variable {
        note: "OpenAI Sora per-second pricing".into(),
    });
    m
}
fn image_size(req: &GenerationRequest) -> Option<String> {
    if let (Some(w), Some(h)) = (req.u64(keys::WIDTH), req.u64(keys::HEIGHT)) {
        return Some(format!("{w}x{h}"));
    }
    req.str(keys::ASPECT_RATIO).map(|r| {
        match r {
            "16:9" => "1536x864",
            "9:16" => "864x1536",
            "3:2" => "1536x1024",
            "2:3" => "1024x1536",
            _ => "1024x1024",
        }
        .into()
    })
}
fn image_body(model: &ModelSpec, req: &GenerationRequest) -> Value {
    let mut body = json!({"model":model.remote_id(),"prompt":req.prompt,"n":req.num_outputs});
    if let Some(size) = image_size(req) {
        body["size"] = json!(size);
    }
    let obj = body.as_object_mut().unwrap();
    req.copy_params(
        obj,
        &[
            (keys::QUALITY, "quality"),
            (keys::OUTPUT_FORMAT, "output_format"),
        ],
    );
    body
}
fn image_outputs(v: &Value, requested_format: Option<&str>) -> ProviderResult<Vec<Output>> {
    let mut out = Vec::new();
    let mime = match v
        .get("output_format")
        .and_then(Value::as_str)
        .or(requested_format)
        .unwrap_or("png")
    {
        "jpeg" => "image/jpeg",
        "webp" => "image/webp",
        _ => "image/png",
    };
    for item in v
        .get("data")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
    {
        if let Some(b) = item.get("b64_json").and_then(Value::as_str) {
            out.push(Output::Bytes {
                data: base64::engine::general_purpose::STANDARD
                    .decode(b)
                    .map_err(|e| ProviderError::Failed(format!("invalid image data: {e}")))?,
                mime: mime.into(),
            });
        } else if let Some(url) = item.get("url").and_then(Value::as_str) {
            out.push(Output::url(url));
        }
    }
    if out.is_empty() {
        Err(ProviderError::Failed("OpenAI returned no image".into()))
    } else {
        Ok(out)
    }
}

#[async_trait]
impl Provider for OpenAI {
    fn info(&self) -> ProviderInfo {
        ProviderInfo {
            id: "openai".into(),
            name: "OpenAI".into(),
            kind: ProviderKind::Cloud,
            fields: vec![CredentialField::api_key(&["OPENAI_API_KEY"])],
            signup_url: Some("https://platform.openai.com/api-keys".into()),
            docs_url: Some("https://developers.openai.com/api/docs/guides/image-generation".into()),
            max_concurrency: 4,
        }
    }
    fn models(&self) -> Vec<ModelSpec> {
        [
            "gpt-image-2.5-sunburst",
            "gpt-image-2.5-flare",
            "gpt-image-2",
        ]
        .into_iter()
        .map(image_spec)
        .chain(["sora-2", "sora-2-pro"].into_iter().map(video_spec))
        .collect()
    }
    async fn discover(&self, ctx: &Ctx) -> ProviderResult<Vec<ModelSpec>> {
        let v = get_json(
            ctx.http
                .get(format!("{API}/models"))
                .bearer_auth(ctx.api_key()?),
        )
        .await?;
        Ok(v.get("data")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .filter_map(|x| x.get("id").and_then(Value::as_str))
            .filter_map(|id| {
                if id.starts_with("gpt-image-") && !id.contains("2026-") && id != "gpt-image-1" {
                    Some(image_spec(id))
                } else if id == "sora-2" || id == "sora-2-pro" {
                    Some(video_spec(id))
                } else {
                    None
                }
            })
            .collect())
    }
    async fn check(&self, ctx: &Ctx) -> ProviderResult<()> {
        get_json(
            ctx.http
                .get(format!("{API}/models"))
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
        let key = ctx.api_key()?;
        if model.output == MediaKind::Image {
            let mut body = image_body(model, req);
            let refs: Vec<_> = req.inputs(InputRole::Reference).collect();
            let endpoint = if refs.is_empty() {
                "generations"
            } else {
                "edits"
            };
            if !refs.is_empty() {
                let mut images = Vec::new();
                for input in refs {
                    images.push(json!({"image_url":input.url_or_data_uri().await?}));
                }
                body["images"] = json!(images);
                if let Some(mask) = req.input(InputRole::Mask) {
                    body["mask"] = json!({"image_url":mask.url_or_data_uri().await?});
                }
            } else if req.input(InputRole::Mask).is_some() {
                return Err(ProviderError::InvalidInput(
                    "mask needs a reference image".into(),
                ));
            }
            let v = send_json(
                ctx.http
                    .post(format!("{API}/images/{endpoint}"))
                    .bearer_auth(key),
                &body,
            )
            .await?;
            Ok(Submission::Done(image_outputs(
                &v,
                req.str(keys::OUTPUT_FORMAT),
            )?))
        } else {
            let mut form = Form::new()
                .text("model", model.remote_id().to_string())
                .text("prompt", req.prompt.clone());
            if let Some(seconds) = req
                .str(keys::DURATION)
                .map(String::from)
                .or_else(|| req.i64(keys::DURATION).map(|n| n.to_string()))
            {
                form = form.text("seconds", seconds);
            }
            if let Some(size) = req.str(keys::RESOLUTION) {
                form = form.text("size", size.to_string());
            }
            if let Some(input) = req.input(InputRole::StartFrame) {
                let part = Part::bytes(input.bytes().await?)
                    .file_name(input.file_name())
                    .mime_str(&input.mime)
                    .map_err(|e| ProviderError::InvalidInput(e.to_string()))?;
                form = form.part("input_reference", part);
            }
            let v = get_json(
                ctx.http
                    .post(format!("{API}/videos"))
                    .bearer_auth(key)
                    .multipart(form),
            )
            .await?;
            let id = v
                .get("id")
                .and_then(Value::as_str)
                .ok_or_else(|| ProviderError::Failed("OpenAI returned no video id".into()))?;
            Ok(Submission::Pending(RemoteHandle::new(id)))
        }
    }
    async fn poll(&self, ctx: &Ctx, handle: &RemoteHandle) -> ProviderResult<PollStatus> {
        let v = get_json(
            ctx.http
                .get(format!("{API}/videos/{}", handle.id))
                .bearer_auth(ctx.api_key()?),
        )
        .await?;
        match v.get("status").and_then(Value::as_str).unwrap_or("") {
            "queued" => Ok(PollStatus::Queued { position: None }),
            "in_progress" => Ok(PollStatus::Running {
                progress: v
                    .get("progress")
                    .and_then(Value::as_f64)
                    .map(|n| (n / 100.0) as f32),
                message: None,
            }),
            "completed" => Ok(PollStatus::Done(vec![Output::Url {
                url: format!("{API}/videos/{}/content", handle.id),
                headers: vec![("Authorization".into(), format!("Bearer {}", ctx.api_key()?))],
            }])),
            "failed" => Ok(PollStatus::Failed(failure(
                v.pointer("/error/message")
                    .and_then(Value::as_str)
                    .unwrap_or("OpenAI video failed"),
            ))),
            other => Err(ProviderError::Failed(format!(
                "unknown OpenAI video state: {other}"
            ))),
        }
    }
    async fn cancel(&self, ctx: &Ctx, handle: &RemoteHandle) -> ProviderResult<()> {
        let r = ctx
            .http
            .post(format!("{API}/videos/{}/cancel", handle.id))
            .bearer_auth(ctx.api_key()?)
            .send()
            .await
            .map_err(net_error)?;
        pictesa_core::json_response(r).await?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn parses_recorded_image() {
        let v = json!({"data":[{"b64_json":"YQ=="}]});
        assert!(matches!(&image_outputs(&v,None).unwrap()[0],Output::Bytes{data,..} if data==b"a"));
        assert!(
            matches!(&image_outputs(&v,Some("jpeg")).unwrap()[0],Output::Bytes{mime,..} if mime=="image/jpeg")
        );
    }
    #[test]
    fn maps_size_and_quality() {
        let mut params = serde_json::Map::new();
        params.insert(keys::ASPECT_RATIO.into(), json!("16:9"));
        params.insert(keys::QUALITY.into(), json!("max"));
        let req = GenerationRequest {
            job_id: "t".into(),
            model: "gpt-image-2.5-sunburst".into(),
            prompt: "draw".into(),
            negative_prompt: None,
            params,
            inputs: vec![],
            num_outputs: 2,
        };
        let body = image_body(&image_spec("gpt-image-2.5-sunburst"), &req);
        assert_eq!(body["size"], "1536x864");
        assert_eq!(body["quality"], "max");
        assert_eq!(body["n"], 2);
    }
}
