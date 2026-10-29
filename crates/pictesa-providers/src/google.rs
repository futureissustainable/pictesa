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
    m.batch()
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
        // A blocked prompt names its reason in promptFeedback, a blocked image in the candidate's
        // finishReason (https://ai.google.dev/api/generate-content#FinishReason).
        let reason = v
            .pointer("/promptFeedback/blockReason")
            .or_else(|| v.pointer("/candidates/0/finishReason"))
            .and_then(Value::as_str);
        Err(match reason {
            Some(r) if ["SAFETY", "PROHIBITED", "BLOCKLIST", "SPII", "RECITATION"].iter().any(|w| r.contains(w)) => {
                ProviderError::ContentPolicy(format!("Google blocked the request: {r}"))
            }
            Some(r) => failure(format!("Google returned no image ({r})")),
            None => failure("Google returned no image"),
        })
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

// REST shapes: https://ai.google.dev/api/batch-api (the SDK renames these fields).
fn batch_body(req: &GenerationRequest, body: Value) -> Value {
    json!({"batch":{"displayName":format!("pictesa-{}",req.job_id),"inputConfig":{"requests":{"requests":[
        {"request":body,"metadata":{"key":req.job_id}}
    ]}}}})
}

fn batch_handle(v: &Value) -> ProviderResult<RemoteHandle> {
    let name = v["name"]
        .as_str()
        .filter(|s| s.starts_with("batches/"))
        .ok_or_else(|| failure("Google returned no batch operation name"))?;
    Ok(RemoteHandle::with(name, json!({"batch":true})))
}

fn batch_status(v: &Value) -> ProviderResult<PollStatus> {
    if let Some(message) = v.pointer("/error/message").and_then(Value::as_str) {
        return Ok(PollStatus::Failed(failure(message)));
    }
    let state = v
        .pointer("/metadata/state")
        .and_then(Value::as_str)
        .unwrap_or("");
    if matches!(
        state,
        "BATCH_STATE_FAILED"
            | "BATCH_STATE_CANCELLED"
            | "BATCH_STATE_EXPIRED"
            | "JOB_STATE_FAILED"
            | "JOB_STATE_CANCELLED"
            | "JOB_STATE_EXPIRED"
    ) {
        return Ok(PollStatus::Failed(failure(&format!(
            "Google batch ended: {state}"
        ))));
    }
    if v["done"].as_bool() != Some(true)
        && !matches!(state, "BATCH_STATE_SUCCEEDED" | "JOB_STATE_SUCCEEDED")
    {
        return Ok(PollStatus::Running {
            progress: None,
            message: Some("Batch processing (up to 24 hours)".into()),
        });
    }
    let responses = v
        .pointer("/metadata/output/inlinedResponses/inlinedResponses")
        .or_else(|| v.pointer("/response/inlinedResponses/inlinedResponses"))
        .or_else(|| v.pointer("/response/output/inlinedResponses/inlinedResponses"))
        .and_then(Value::as_array)
        .ok_or_else(|| failure("Google batch finished without inline responses"))?;
    let mut outputs = Vec::new();
    for response in responses {
        if let Some(message) = response.pointer("/error/message").and_then(Value::as_str) {
            return Ok(PollStatus::Failed(failure(message)));
        }
        outputs.extend(image_output(&response["response"])?);
    }
    if outputs.is_empty() {
        return Err(failure("Google batch returned no images"));
    }
    Ok(PollStatus::Done(outputs))
}

fn discovered(x: &Value) -> Option<ModelSpec> {
    let id = x.get("name")?.as_str()?.trim_start_matches("models/");
    let methods = x.get("supportedGenerationMethods").and_then(Value::as_array);
    // Image models carry "image" in their id, except the Nano Banana names (gemini-nano-banana-2.1).
    if (id.contains("image") || id.contains("nano-banana"))
        && methods.is_some_and(|m| m.iter().any(|s| s.as_str() == Some("generateContent")))
    {
        // Google names a model and its preview the same; keep them apart in the picker.
        let name = x.get("displayName").and_then(Value::as_str).unwrap_or(id);
        let name = if id.contains("preview") && !name.to_lowercase().contains("preview") {
            format!("{name} (preview)")
        } else {
            name.to_string()
        };
        Some(image_spec(id, &name))
    } else if id.starts_with("veo-")
        && methods.is_some_and(|m| m.iter().any(|s| s.as_str() == Some("predictLongRunning")))
    {
        Some(video_spec(id))
    } else {
        None
    }
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
        let key = ctx.api_key()?;
        let mut out = Vec::new();
        let mut page: Option<String> = None;
        // models.list answers 50 models a page unless asked for more; nextPageToken leads to the rest.
        for _ in 0..20 {
            let mut req = ctx.http.get(format!("{API}/models")).query(&[("pageSize", "1000")]);
            if let Some(token) = &page {
                req = req.query(&[("pageToken", token)]);
            }
            let v = get_json(req.header("x-goog-api-key", key)).await?;
            out.extend(v.get("models").and_then(Value::as_array).into_iter().flatten().filter_map(discovered));
            match v.get("nextPageToken").and_then(Value::as_str).filter(|t| !t.is_empty()) {
                Some(token) => page = Some(token.to_string()),
                None => break,
            }
        }
        Ok(out)
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
            if model.is_batch(&req.params) {
                let body = batch_body(req, body);
                if body.to_string().len() >= 20 * 1024 * 1024 {
                    return Err(ProviderError::InvalidInput("Gemini inline batch requests must be smaller than 20 MB. Use fewer or smaller reference images.".into()));
                }
                let v = send_json(
                    ctx.http
                        .post(format!(
                            "{API}/models/{}:batchGenerateContent",
                            model.remote_id()
                        ))
                        .header("x-goog-api-key", key),
                    &body,
                )
                .await?;
                return Ok(Submission::Pending(batch_handle(&v)?));
            }
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
        if handle.is_batch() {
            return batch_status(&v);
        }
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
    async fn cancel(&self, ctx: &Ctx, handle: &RemoteHandle) -> ProviderResult<()> {
        if handle.is_batch() {
            get_json(
                ctx.http
                    .post(format!("{API}/{}:cancel", handle.id))
                    .header("x-goog-api-key", ctx.api_key()?),
            )
            .await?;
        }
        Ok(())
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
    #[test]
    fn batch_uses_rest_request_and_operation_shapes() {
        let req = GenerationRequest {
            job_id: "job-1".into(),
            model: "google:gemini-3-pro-image".into(),
            prompt: "draw".into(),
            negative_prompt: None,
            params: serde_json::from_value(json!({"speed":"batch","resolution":"2K"})).unwrap(),
            inputs: vec![],
            num_outputs: 1,
        };
        let body = batch_body(&req, image_body(&req, vec![json!({"text":"draw"})]));
        assert_eq!(
            body,
            json!({"batch":{"displayName":"pictesa-job-1","inputConfig":{"requests":{"requests":[{
                "metadata":{"key":"job-1"},"request":{"contents":[{"role":"user","parts":[{"text":"draw"}]}],
                "generationConfig":{"responseModalities":["IMAGE"],"imageConfig":{"imageSize":"2K"}}}
            }]}}}})
        );
        let h =
            batch_handle(&json!({"name":"batches/abc","metadata":{"state":"BATCH_STATE_PENDING"}}))
                .unwrap();
        let h: RemoteHandle = serde_json::from_str(&serde_json::to_string(&h).unwrap()).unwrap();
        assert_eq!(h.id, "batches/abc");
        assert_eq!(
            Google.poll_interval_for(&h, 0),
            std::time::Duration::from_secs(60)
        );
        assert!(matches!(
            batch_status(&json!({"name":h.id,"done":false})).unwrap(),
            PollStatus::Running { .. }
        ));
        let response = json!({"done":true,"metadata":{"state":"BATCH_STATE_SUCCEEDED","output":{"inlinedResponses":{"inlinedResponses":[{
            "metadata":{"key":"job-1"},"response":{"candidates":[{"content":{"parts":[{"inlineData":{"mimeType":"image/png","data":"YQ=="}}]}}]}
        }]}}}});
        assert!(
            matches!(batch_status(&response).unwrap(), PollStatus::Done(out) if matches!(&out[0], Output::Bytes { data, mime } if data == b"a" && mime == "image/png"))
        );
        for response in [
            json!({"done":true,"error":{"code":1,"message":"cancelled"}}),
            json!({"done":true,"response":{"inlinedResponses":{"inlinedResponses":[{"error":{"code":3,"message":"bad input"}}]}}}),
            json!({"metadata":{"state":"BATCH_STATE_EXPIRED"},"done":true}),
        ] {
            assert!(matches!(
                batch_status(&response).unwrap(),
                PollStatus::Failed(_)
            ));
        }
        assert!(batch_status(&json!({"done":true,"response":{}})).is_err());
    }
}
