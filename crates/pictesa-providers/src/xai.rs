use crate::util::{failure, get_json, send_json};
use async_trait::async_trait;
use base64::Engine;
use pictesa_core::{
    keys, CredentialField, Ctx, GenerationRequest, InputRole, InputSlot, MediaKind, ModelSpec,
    Output, ParamSpec, PollStatus, Pricing, Provider, ProviderError, ProviderInfo, ProviderKind,
    ProviderResult, RemoteHandle, Submission, Task,
};
use serde_json::{json, Value};

pub struct Xai;
const API: &str = "https://api.x.ai/v1";

fn outputs(v: &Value) -> ProviderResult<Vec<Output>> {
    let mut out = Vec::new();
    for x in v
        .get("data")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
    {
        if let Some(url) = x.get("url").and_then(Value::as_str) {
            out.push(Output::url(url));
        } else if let Some(b) = x.get("b64_json").and_then(Value::as_str) {
            out.push(Output::Bytes {
                data: base64::engine::general_purpose::STANDARD
                    .decode(b)
                    .map_err(|e| ProviderError::Failed(e.to_string()))?,
                mime: x
                    .get("mime_type")
                    .and_then(Value::as_str)
                    .unwrap_or("image/jpeg")
                    .into(),
            });
        }
    }
    if out.is_empty() {
        Err(failure("xAI returned no image"))
    } else {
        Ok(out)
    }
}
fn base_body(model: &ModelSpec, req: &GenerationRequest) -> Value {
    let mut body = json!({"model":model.remote_id(),"prompt":req.prompt});
    if model.output == MediaKind::Image {
        body["n"] = json!(req.num_outputs);
        req.copy_params(
            body.as_object_mut().unwrap(),
            &[
                (keys::ASPECT_RATIO, "aspect_ratio"),
                (keys::RESOLUTION, "resolution"),
                (keys::QUALITY, "quality"),
            ],
        );
    } else {
        req.copy_params(
            body.as_object_mut().unwrap(),
            &[
                (keys::DURATION, "duration"),
                (keys::ASPECT_RATIO, "aspect_ratio"),
                (keys::RESOLUTION, "resolution"),
                (keys::GENERATE_AUDIO, "generate_audio"),
            ],
        );
    }
    body
}

#[async_trait]
impl Provider for Xai {
    fn info(&self) -> ProviderInfo {
        ProviderInfo {
            id: "xai".into(),
            name: "xAI".into(),
            kind: ProviderKind::Cloud,
            fields: vec![CredentialField::api_key(&["XAI_API_KEY"])],
            signup_url: Some("https://console.x.ai".into()),
            docs_url: Some("https://docs.x.ai/developers/model-capabilities/imagine".into()),
            max_concurrency: 4,
        }
    }
    fn models(&self) -> Vec<ModelSpec> {
        let mut out = Vec::new();
        for (id, price) in [
            ("grok-imagine-image-2.0", 0.04),
            ("grok-imagine-image", 0.02),
            ("grok-imagine-image-quality", 0.05),
        ] {
            let mut m = ModelSpec::new(
                "xai",
                id,
                id,
                MediaKind::Image,
                &[Task::TextToImage, Task::ImageEdit],
            );
            m.vendor = Some("xAI".into());
            m.inputs = vec![InputSlot::new(
                InputRole::Reference,
                MediaKind::Image,
                if id.ends_with("2.0") { 5 } else { 1 },
            )];
            m.params = vec![
                ParamSpec::choices(
                    keys::ASPECT_RATIO,
                    "Aspect ratio",
                    &[
                        "1:1", "16:9", "9:16", "4:3", "3:4", "3:2", "2:3", "21:9", "5:2",
                    ],
                ),
                ParamSpec::choices(keys::RESOLUTION, "Resolution", &["1k", "2k"]),
            ];
            if id == "grok-imagine-image-2.0" {
                m.params.push(ParamSpec::choices(
                    keys::QUALITY,
                    "Quality",
                    &["auto", "low", "medium"],
                ));
            }
            m.max_outputs = 10;
            m.pricing = Some(if id == "grok-imagine-image-2.0" {
                Pricing::Variable {
                    note: "from $0.04/image; resolution, quality and input images add cost".into(),
                }
            } else {
                Pricing::PerImage { usd: price }
            });
            out.push(m);
        }
        for (id, price) in [
            ("grok-imagine-video-1.5", 0.08),
            ("grok-imagine-video-1.5-lite", 0.02),
            ("grok-imagine-video", 0.05),
        ] {
            let mut m = ModelSpec::new(
                "xai",
                id,
                id,
                MediaKind::Video,
                if id == "grok-imagine-video" {
                    &[Task::TextToVideo, Task::ImageToVideo, Task::VideoToVideo]
                } else {
                    &[Task::TextToVideo, Task::ImageToVideo]
                },
            );
            m.vendor = Some("xAI".into());
            m.inputs = vec![InputSlot::new(InputRole::StartFrame, MediaKind::Image, 1)];
            if id == "grok-imagine-video-1.5" {
                m.inputs.extend([
                    InputSlot::new(InputRole::EndFrame, MediaKind::Image, 1),
                    InputSlot::new(InputRole::Reference, MediaKind::Image, 14),
                ]);
            }
            if id == "grok-imagine-video" {
                m.inputs
                    .push(InputSlot::new(InputRole::Video, MediaKind::Video, 1));
            }
            m.params = vec![
                ParamSpec::int(keys::DURATION, "Duration", 1, 15),
                ParamSpec::choices(
                    keys::ASPECT_RATIO,
                    "Aspect ratio",
                    &["16:9", "9:16", "1:1", "4:3", "3:4"],
                ),
                ParamSpec::choices(keys::RESOLUTION, "Resolution", &["480p", "720p", "1080p"]),
                ParamSpec::boolean(keys::GENERATE_AUDIO, "Generate audio"),
            ];
            m.pricing = Some(Pricing::PerSecond { usd: price });
            out.push(m);
        }
        out
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
        let mut body = base_body(model, req);
        let endpoint;
        if model.output == MediaKind::Image {
            let mut refs = Vec::new();
            for input in req.inputs(InputRole::Reference) {
                refs.push(json!({"type":"image_url","url":input.url_or_data_uri().await?}));
            }
            endpoint = if refs.is_empty() {
                "images/generations"
            } else if refs.len() == 1 {
                body["image"] = refs.remove(0);
                "images/edits"
            } else {
                body["images"] = json!(refs);
                "images/edits"
            };
        } else {
            endpoint = if let Some(input) = req.input(InputRole::Video) {
                if model.remote_id() != "grok-imagine-video" {
                    return Err(ProviderError::NotSupported(
                        "xAI video editing uses grok-imagine-video".into(),
                    ));
                }
                body = json!({"model":model.remote_id(),"prompt":req.prompt,"video":{"url":input.url_or_data_uri().await?}});
                "videos/edits"
            } else {
                "videos/generations"
            };
            if let Some(input) = req.input(InputRole::StartFrame) {
                body["image"] = json!({"type":"image_url","url":input.url_or_data_uri().await?});
            }
            if let Some(input) = req.input(InputRole::EndFrame) {
                body["last_frame"] =
                    json!({"type":"image_url","url":input.url_or_data_uri().await?});
            }
            let mut refs = Vec::new();
            for input in req.inputs(InputRole::Reference) {
                refs.push(json!({"type":"image_url","url":input.url_or_data_uri().await?}));
            }
            if !refs.is_empty() {
                body["reference_images"] = json!(refs);
            }
        }
        let v = send_json(
            ctx.http
                .post(format!("{API}/{endpoint}"))
                .bearer_auth(ctx.api_key()?),
            &body,
        )
        .await?;
        if let Some(id) = v.get("request_id").and_then(Value::as_str) {
            return Ok(Submission::Pending(RemoteHandle::with(
                id,
                json!({"output":model.output}),
            )));
        }
        if model.output == MediaKind::Image {
            Ok(Submission::Done(outputs(&v)?))
        } else {
            Err(ProviderError::Failed(
                "xAI returned no video request id".into(),
            ))
        }
    }
    async fn poll(&self, ctx: &Ctx, handle: &RemoteHandle) -> ProviderResult<PollStatus> {
        let video = handle.data.get("output").and_then(Value::as_str) == Some("video");
        let kind = if video { "videos" } else { "images" };
        let v = get_json(
            ctx.http
                .get(format!("{API}/{kind}/{}", handle.id))
                .bearer_auth(ctx.api_key()?),
        )
        .await?;
        match v.get("status").and_then(Value::as_str).unwrap_or("") {
            "pending" => Ok(PollStatus::Running {
                progress: None,
                message: None,
            }),
            "failed" => Ok(PollStatus::Failed(failure(
                v.pointer("/error/message")
                    .and_then(Value::as_str)
                    .unwrap_or("xAI generation failed"),
            ))),
            "done" if video => {
                // A video that breaks moderation still ends "done", with respect_moderation false and
                // an empty url (https://docs.x.ai/developers/rest-api-reference/inference/videos).
                if v.pointer("/video/respect_moderation").and_then(Value::as_bool) == Some(false) {
                    return Ok(PollStatus::Failed(ProviderError::ContentPolicy(
                        "xAI moderation withheld the video".into(),
                    )));
                }
                let url = v
                    .pointer("/video/url")
                    .and_then(Value::as_str)
                    .filter(|u| !u.is_empty())
                    .ok_or_else(|| ProviderError::Failed("xAI returned no video URL".into()))?;
                Ok(PollStatus::Done(vec![Output::url(url)]))
            }
            "done" => Ok(PollStatus::Done(outputs(&v)?)),
            other => Err(ProviderError::Failed(format!("unknown xAI state: {other}"))),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn parses_recorded_response() {
        assert!(matches!(
            &outputs(&json!({"data":[{"url":"https://example.com/a.jpg"}]})).unwrap()[0],
            Output::Url { .. }
        ));
    }
    #[test]
    fn maps_video_params() {
        let mut params = serde_json::Map::new();
        params.insert(keys::DURATION.into(), json!(8));
        params.insert(keys::GENERATE_AUDIO.into(), json!(false));
        let req = GenerationRequest {
            job_id: "t".into(),
            model: "grok-imagine-video-1.5".into(),
            prompt: "move".into(),
            negative_prompt: None,
            params,
            inputs: vec![],
            num_outputs: 1,
        };
        let model = Xai
            .models()
            .into_iter()
            .find(|m| m.remote_id() == "grok-imagine-video-1.5")
            .unwrap();
        let body = base_body(&model, &req);
        assert_eq!(body["duration"], 8);
        assert_eq!(body["generate_audio"], false);
    }
}
