use async_trait::async_trait;
use base64::Engine;
use pictesa_core::{
    keys, CredentialField, Ctx, GenerationRequest, InputRole, InputSlot, MediaKind, ModelSpec,
    Output, ParamSpec, PollStatus, Pricing, Provider, ProviderError, ProviderInfo, ProviderKind,
    ProviderResult, RemoteHandle, Submission, Task,
};
use serde_json::{json, Value};

use crate::util::{failure, get_json, send_json};

pub struct Google;
const API: &str = "https://generativelanguage.googleapis.com/v1beta";

fn image_spec(id: &str, name: &str) -> ModelSpec {
    let mut m = ModelSpec::new(
        "google",
        id,
        name,
        MediaKind::Image,
        &[Task::TextToImage, Task::ImageEdit],
    );
    m.vendor = Some("Google".into());
    m.inputs = vec![InputSlot::new(InputRole::Reference, MediaKind::Image, 14)];
    m.params = vec![ParamSpec::choices(
        keys::ASPECT_RATIO,
        "Aspect ratio",
        &["1:1", "16:9", "9:16", "4:3", "3:4", "3:2", "2:3"],
    )];
    if !id.starts_with("gemini-2.5-") && !id.contains("flash-lite") {
        m.params.push(ParamSpec::choices(
            keys::RESOLUTION,
            "Resolution",
            &["1K", "2K", "4K"],
        ));
    }
    m.pricing = Some(Pricing::Variable {
        note: "Google image token pricing".into(),
    });
    m
}
fn video_spec(id: &str) -> ModelSpec {
    let mut m = ModelSpec::new(
        "google",
        id,
        id,
        MediaKind::Video,
        &[Task::TextToVideo, Task::ImageToVideo],
    );
    m.vendor = Some("Google".into());
    m.inputs = vec![
        InputSlot::new(InputRole::StartFrame, MediaKind::Image, 1),
        InputSlot::new(InputRole::EndFrame, MediaKind::Image, 1),
    ];
    m.params = vec![
        ParamSpec::choices(keys::ASPECT_RATIO, "Aspect ratio", &["16:9", "9:16"]),
        ParamSpec::enumeration(
            keys::DURATION,
            "Duration",
            &[(4, "4s"), (6, "6s"), (8, "8s")],
        ),
        ParamSpec::choices(keys::RESOLUTION, "Resolution", &["720p", "1080p", "4k"]),
    ];
    m.pricing = Some(Pricing::Variable {
        note: "Google Veo video pricing".into(),
    });
    m
}
fn image_output(v: &Value) -> ProviderResult<Vec<Output>> {
    let mut out = Vec::new();
    for c in v
        .get("candidates")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
    {
        for p in c
            .pointer("/content/parts")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
        {
            if let (Some(mime), Some(data)) = (
                p.pointer("/inlineData/mimeType").and_then(Value::as_str),
                p.pointer("/inlineData/data").and_then(Value::as_str),
            ) {
                out.push(Output::Bytes {
                    data: base64::engine::general_purpose::STANDARD
                        .decode(data)
                        .map_err(|e| ProviderError::Failed(format!("invalid image data: {e}")))?,
                    mime: mime.into(),
                });
            }
        }
    }
    if out.is_empty() {
        Err(failure(
            v.pointer("/promptFeedback/blockReason")
                .and_then(Value::as_str)
                .unwrap_or("Google returned no image"),
        ))
    } else {
        Ok(out)
    }
}
fn video_output(v: &Value, key: &str) -> ProviderResult<Vec<Output>> {
    let arr = v
        .pointer("/response/generateVideoResponse/generatedSamples")
        .or_else(|| v.pointer("/response/generatedVideos"))
        .and_then(Value::as_array);
    let out: Vec<_> = arr
        .into_iter()
        .flatten()
        .filter_map(|x| x.pointer("/video/uri").and_then(Value::as_str))
        .map(|url| Output::Url {
            url: url.into(),
            headers: vec![("x-goog-api-key".into(), key.into())],
        })
        .collect();
    if out.is_empty() {
        Err(failure("Google operation finished without video"))
    } else {
        Ok(out)
    }
}
fn image_body(req: &GenerationRequest, parts: Vec<Value>) -> Value {
    let mut config = json!({"responseModalities":["IMAGE"]});
    let mut image_config = serde_json::Map::new();
    req.copy_params(
        &mut image_config,
        &[
            (keys::ASPECT_RATIO, "aspectRatio"),
            (keys::RESOLUTION, "imageSize"),
        ],
    );
    if !image_config.is_empty() {
        config["imageConfig"] = Value::Object(image_config);
    }
    json!({"contents":[{"role":"user","parts":parts}],"generationConfig":config})
}

#[async_trait]
impl Provider for Google {
    fn info(&self) -> ProviderInfo {
        ProviderInfo {
            id: "google".into(),
            name: "Google Gemini".into(),
            kind: ProviderKind::Cloud,
            fields: vec![CredentialField::api_key(&[
                "GEMINI_API_KEY",
                "GOOGLE_API_KEY",
            ])],
            signup_url: Some("https://aistudio.google.com/apikey".into()),
            docs_url: Some("https://ai.google.dev/gemini-api/docs/image-generation".into()),
            max_concurrency: 4,
        }
    }
    fn models(&self) -> Vec<ModelSpec> {
        [
            ("gemini-3.1-flash-lite-image", "Nano Banana 2 Lite"),
            ("gemini-3.1-flash-image", "Nano Banana 2"),
            ("gemini-3-pro-image", "Nano Banana Pro"),
            ("gemini-2.5-flash-image", "Nano Banana"),
        ]
        .into_iter()
        .map(|(id, name)| image_spec(id, name))
        .collect()
    }
    async fn discover(&self, ctx: &Ctx) -> ProviderResult<Vec<ModelSpec>> {
        let v = get_json(
            ctx.http
                .get(format!("{API}/models"))
                .header("x-goog-api-key", ctx.api_key()?),
        )
        .await?;
        Ok(v.get("models")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .filter_map(|x| {
                let id = x.get("name")?.as_str()?.trim_start_matches("models/");
                let methods = x
                    .get("supportedGenerationMethods")
                    .and_then(Value::as_array);
                if id.contains("image")
                    && methods
                        .is_some_and(|m| m.iter().any(|s| s.as_str() == Some("generateContent")))
                {
                    // Google names a model and its preview the same; keep them apart in the picker.
                    let name = x.get("displayName").and_then(Value::as_str).unwrap_or(id);
                    let name = if id.contains("preview") && !name.to_lowercase().contains("preview") { format!("{name} (preview)") } else { name.to_string() };
                    Some(image_spec(id, &name))
                } else if id.starts_with("veo-")
                    && methods
                        .is_some_and(|m| m.iter().any(|s| s.as_str() == Some("predictLongRunning")))
                {
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
                .get(format!("{API}/models?pageSize=1"))
                .header("x-goog-api-key", ctx.api_key()?),
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
            let mut parts = vec![json!({"text":req.prompt})];
            for input in req.inputs(InputRole::Reference) {
                parts.push(
                    json!({"inlineData":{"mimeType":input.mime,"data":input.base64().await?}}),
                );
            }
            let body = image_body(req, parts);
            let v = send_json(
                ctx.http
                    .post(format!(
                        "{API}/models/{}:generateContent",
                        model.remote_id()
                    ))
                    .header("x-goog-api-key", key),
                &body,
            )
            .await?;
            Ok(Submission::Done(image_output(&v)?))
        } else {
            let mut instance = json!({"prompt":req.prompt});
            if let Some(input) = req.input(InputRole::StartFrame) {
                instance["image"] =
                    json!({"inlineData":{"data":input.base64().await?,"mimeType":input.mime}});
            }
            let mut params = serde_json::Map::new();
            req.copy_params(
                &mut params,
                &[
                    (keys::ASPECT_RATIO, "aspectRatio"),
                    (keys::RESOLUTION, "resolution"),
                ],
            );
            if let Some(duration) = req.i64(keys::DURATION) {
                params.insert("durationSeconds".into(), json!(duration.to_string()));
            }
            if let Some(input) = req.input(InputRole::EndFrame) {
                if req.input(InputRole::StartFrame).is_none() {
                    return Err(ProviderError::InvalidInput(
                        "Veo end frame needs a start frame".into(),
                    ));
                }
                instance["lastFrame"] =
                    json!({"inlineData":{"data":input.base64().await?,"mimeType":input.mime}});
            }
            let v = send_json(
                ctx.http
                    .post(format!(
                        "{API}/models/{}:predictLongRunning",
                        model.remote_id()
                    ))
                    .header("x-goog-api-key", key),
                &json!({"instances":[instance],"parameters":params}),
            )
            .await?;
            let name = v
                .get("name")
                .and_then(Value::as_str)
                .ok_or_else(|| ProviderError::Failed("Google returned no operation name".into()))?;
            Ok(Submission::Pending(RemoteHandle::new(name)))
        }
    }
    async fn poll(&self, ctx: &Ctx, handle: &RemoteHandle) -> ProviderResult<PollStatus> {
        let key = ctx.api_key()?;
        let v = get_json(
            ctx.http
                .get(format!("{API}/{}", handle.id))
                .header("x-goog-api-key", key),
        )
        .await?;
        if !v.get("done").and_then(Value::as_bool).unwrap_or(false) {
            return Ok(PollStatus::Running {
                progress: None,
                message: None,
            });
        }
        if let Some(err) = v.pointer("/error/message").and_then(Value::as_str) {
            return Ok(PollStatus::Failed(failure(err)));
        }
        Ok(PollStatus::Done(video_output(&v, key)?))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn parses_recorded_inline_image() {
        let v = json!({"candidates":[{"content":{"parts":[{"inlineData":{"mimeType":"image/png","data":"YQ=="}}]}}]});
        assert!(matches!(&image_output(&v).unwrap()[0],Output::Bytes{data,..} if data==b"a"));
    }
    #[test]
    fn builds_image_config() {
        let mut params = serde_json::Map::new();
        params.insert(keys::ASPECT_RATIO.into(), json!("16:9"));
        params.insert(keys::RESOLUTION.into(), json!("2K"));
        let req = GenerationRequest {
            job_id: "t".into(),
            model: "gemini-3-pro-image".into(),
            prompt: "draw".into(),
            negative_prompt: None,
            params,
            inputs: vec![],
            num_outputs: 1,
        };
        let body = image_body(&req, vec![json!({"text":"draw"})]);
        assert_eq!(
            body["generationConfig"]["imageConfig"],
            json!({"aspectRatio":"16:9","imageSize":"2K"})
        );
    }
    #[test]
    fn parses_recorded_video_operation() {
        let v = json!({"done":true,"response":{"generateVideoResponse":{"generatedSamples":[{"video":{"uri":"https://generativelanguage.googleapis.com/v1beta/files/a:download"}}]}}});
        assert!(
            matches!(&video_output(&v,"test-key").unwrap()[0],Output::Url{headers,..} if headers==&vec![("x-goog-api-key".into(),"test-key".into())])
        );
    }
}
