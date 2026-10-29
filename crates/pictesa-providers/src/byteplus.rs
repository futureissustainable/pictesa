use crate::util::{failure, net_error, API_TIMEOUT};
use async_trait::async_trait;
use base64::Engine;
use pictesa_core::{
    keys, CredentialField, Ctx, GenerationRequest, InputRole, InputSlot, MediaKind, ModelSpec,
    Output, ParamSpec, PollStatus, Pricing, Provider, ProviderError, ProviderInfo, ProviderKind,
    ProviderResult, RemoteHandle, Submission, Task,
};
use serde_json::{json, Value};

pub struct BytePlus;
const API: &str = "https://ark.ap-southeast.bytepluses.com/api/v3";

fn image_outputs(v: &Value) -> ProviderResult<Vec<Output>> {
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
                mime: match x.get("output_format").and_then(Value::as_str) {
                    Some("png") => "image/png",
                    _ => "image/jpeg",
                }
                .into(),
            });
        }
    }
    if out.is_empty() {
        Err(failure("BytePlus returned no image"))
    } else {
        Ok(out)
    }
}
/// Sends a ModelArk call. Its error codes (https://docs.byteplus.com/en/docs/ModelArk/error-codes)
/// name moderation and empty accounts, which the HTTP status alone does not.
async fn ark(req: reqwest::RequestBuilder) -> ProviderResult<Value> {
    let resp = req.timeout(API_TIMEOUT).send().await.map_err(net_error)?;
    if resp.status().is_success() {
        return pictesa_core::json_response(resp).await;
    }
    let status = resp.status().as_u16();
    let retry_after = resp.headers().get(reqwest::header::RETRY_AFTER).and_then(|v| v.to_str().ok()?.parse::<f64>().ok()).map(|s| (s * 1000.0) as u64);
    let text = resp.text().await.map_err(net_error)?;
    let code = serde_json::from_str::<Value>(&text).ok().and_then(|v| v.pointer("/error/code")?.as_str().map(str::to_string)).unwrap_or_default();
    let detail = pictesa_core::error::truncate(&text, 600);
    Err(if code.contains("SensitiveContentDetected") {
        ProviderError::ContentPolicy(detail)
    } else if code == "AccountOverdueError" || (code == "QuotaExceeded" && text.contains("free trial quota")) {
        ProviderError::InsufficientCredits(detail)
    } else {
        ProviderError::from_http(status, &text, retry_after)
    })
}
fn video_options(req: &GenerationRequest) -> Value {
    let mut body = json!({"content":[{"type":"text","text":req.prompt}]});
    req.copy_params(
        body.as_object_mut().unwrap(),
        &[
            (keys::ASPECT_RATIO, "ratio"),
            (keys::RESOLUTION, "resolution"),
            (keys::DURATION, "duration"),
            (keys::GENERATE_AUDIO, "generate_audio"),
        ],
    );
    body
}

#[async_trait]
impl Provider for BytePlus {
    fn info(&self) -> ProviderInfo {
        ProviderInfo {
            id: "byteplus".into(),
            name: "BytePlus ModelArk".into(),
            kind: ProviderKind::Cloud,
            fields: vec![CredentialField::api_key(&[
                "BYTEPLUS_API_KEY",
                "ARK_API_KEY",
            ])],
            signup_url: Some(
                "https://console.byteplus.com/ark/region:ap-southeast-1/apikey".into(),
            ),
            docs_url: Some("https://docs.byteplus.com/en/docs/modelark/quick-start".into()),
            max_concurrency: 4,
        }
    }
    fn models(&self) -> Vec<ModelSpec> {
        let mut out = Vec::new();
        for id in [
            "dola-seedream-5-0-pro-260628",
            "dola-seedream-5-0-flash-260915",
            "seedream-4-5-251128",
        ] {
            let mut m = ModelSpec::new(
                "byteplus",
                id,
                id,
                MediaKind::Image,
                &[Task::TextToImage, Task::ImageEdit],
            );
            m.vendor = Some("ByteDance".into());
            m.inputs = vec![InputSlot::new(InputRole::Reference, MediaKind::Image, 10)];
            m.params = vec![
                ParamSpec::choices(
                    keys::ASPECT_RATIO,
                    "Aspect ratio",
                    &["1:1", "16:9", "9:16", "4:3", "3:4", "3:2", "2:3"],
                ),
                ParamSpec::choices(keys::RESOLUTION, "Resolution", &["1K", "1.5K", "2K"]),
                ParamSpec::choices(keys::OUTPUT_FORMAT, "Format", &["jpeg", "png"]),
            ];
            m.pricing = Some(Pricing::Variable {
                note: "BytePlus image token pricing".into(),
            });
            out.push(m);
        }
        for id in [
            "dreamina-seedance-2-5-260628",
            "dreamina-seedance-2-0-260128",
            "dreamina-seedance-2-0-fast-260128",
            "seedance-1-5-pro-251215",
        ] {
            let mut m = ModelSpec::new(
                "byteplus",
                id,
                id,
                MediaKind::Video,
                &[Task::TextToVideo, Task::ImageToVideo, Task::VideoToVideo],
            );
            m.vendor = Some("ByteDance".into());
            m.inputs = vec![
                InputSlot::new(InputRole::StartFrame, MediaKind::Image, 1),
                InputSlot::new(InputRole::EndFrame, MediaKind::Image, 1),
                InputSlot::new(
                    InputRole::Reference,
                    MediaKind::Image,
                    if id.contains("2-5") { 30 } else { 9 },
                ),
                InputSlot::new(
                    InputRole::Video,
                    MediaKind::Video,
                    if id.contains("2-5") { 10 } else { 3 },
                ),
                InputSlot::new(
                    InputRole::Audio,
                    MediaKind::Audio,
                    if id.contains("2-5") { 10 } else { 3 },
                ),
            ];
            m.params = vec![
                ParamSpec::choices(
                    keys::ASPECT_RATIO,
                    "Aspect ratio",
                    &["16:9", "9:16", "1:1", "4:3", "3:4"],
                ),
                ParamSpec::choices(keys::RESOLUTION, "Resolution", &["480p", "720p", "1080p"]),
                ParamSpec::int(
                    keys::DURATION,
                    "Duration",
                    4,
                    if id.contains("2-5") { 30 } else { 15 },
                ),
                ParamSpec::boolean(keys::GENERATE_AUDIO, "Generate audio"),
            ];
            m.pricing = Some(Pricing::Variable {
                note: "BytePlus video token pricing".into(),
            });
            out.push(m);
        }
        out
    }
    async fn check(&self, ctx: &Ctx) -> ProviderResult<()> {
        // Listing models needs Access Key auth on another host; listing tasks takes the API key.
        ark(ctx
            .http
            .get(format!("{API}/contents/generations/tasks?page_size=1"))
            .bearer_auth(ctx.api_key()?))
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
            let prompt = if let Some(r) = req.str(keys::ASPECT_RATIO) {
                format!("{} Aspect ratio {r}.", req.prompt)
            } else {
                req.prompt.clone()
            };
            let mut body = json!({"model":model.remote_id(),"prompt":prompt,"response_format":"url","watermark":false});
            if let Some(size) = req.str(keys::RESOLUTION) {
                body["size"] = json!(size);
            }
            if let (Some(w), Some(h)) = (req.u64(keys::WIDTH), req.u64(keys::HEIGHT)) {
                body["size"] = json!(format!("{w}x{h}"));
            }
            if let Some(fmt) = req.str(keys::OUTPUT_FORMAT) {
                body["output_format"] = json!(fmt);
            }
            let mut refs = Vec::new();
            for input in req.inputs(InputRole::Reference) {
                refs.push(input.url_or_data_uri().await?);
            }
            if refs.len() == 1 {
                body["image"] = json!(refs[0]);
            } else if !refs.is_empty() {
                body["image"] = json!(refs);
            }
            let v = ark(ctx
                .http
                .post(format!("{API}/images/generations"))
                .bearer_auth(key)
                .json(&body))
            .await?;
            Ok(Submission::Done(image_outputs(&v)?))
        } else {
            if req.input(InputRole::StartFrame).is_some()
                && (req.inputs(InputRole::Reference).next().is_some()
                    || req.inputs(InputRole::Video).next().is_some()
                    || req.inputs(InputRole::Audio).next().is_some())
            {
                return Err(ProviderError::InvalidInput(
                    "BytePlus frame inputs cannot be mixed with omni references".into(),
                ));
            }
            let mut body = video_options(req);
            let content = body["content"].as_array_mut().unwrap();
            for (role, label) in [
                (InputRole::StartFrame, "first_frame"),
                (InputRole::EndFrame, "last_frame"),
                (InputRole::Reference, "reference_image"),
            ] {
                for input in req.inputs(role) {
                    content.push(json!({"type":"image_url","image_url":{"url":input.url_or_data_uri().await?},"role":label}));
                }
            }
            for input in req.inputs(InputRole::Video) {
                let url = input.url.as_ref().ok_or_else(|| {
                    ProviderError::InvalidInput("BytePlus video references need public URLs".into())
                })?;
                content.push(
                    json!({"type":"video_url","video_url":{"url":url},"role":"reference_video"}),
                );
            }
            for input in req.inputs(InputRole::Audio) {
                content.push(json!({"type":"audio_url","audio_url":{"url":input.url_or_data_uri().await?},"role":"reference_audio"}));
            }
            body["model"] = json!(model.remote_id());
            let v = ark(ctx
                .http
                .post(format!("{API}/contents/generations/tasks"))
                .bearer_auth(key)
                .json(&body))
            .await?;
            let id = v
                .get("id")
                .and_then(Value::as_str)
                .ok_or_else(|| ProviderError::Failed("BytePlus returned no task id".into()))?;
            Ok(Submission::Pending(RemoteHandle::new(id)))
        }
    }
    async fn poll(&self, ctx: &Ctx, handle: &RemoteHandle) -> ProviderResult<PollStatus> {
        let v = ark(ctx
            .http
            .get(format!("{API}/contents/generations/tasks/{}", handle.id))
            .bearer_auth(ctx.api_key()?))
        .await?;
        match v.get("status").and_then(Value::as_str).unwrap_or("") {
            "queued" => Ok(PollStatus::Queued { position: None }),
            "running" => Ok(PollStatus::Running {
                progress: None,
                message: None,
            }),
            "succeeded" => {
                let url = v
                    .pointer("/content/video_url")
                    .and_then(Value::as_str)
                    .ok_or_else(|| {
                        ProviderError::Failed("BytePlus returned no video URL".into())
                    })?;
                Ok(PollStatus::Done(vec![Output::url(url)]))
            }
            "failed" | "expired" | "cancelled" => Ok(PollStatus::Failed(failure(
                v.pointer("/error/message")
                    .and_then(Value::as_str)
                    .unwrap_or("BytePlus task failed"),
            ))),
            other => Err(ProviderError::Failed(format!(
                "unknown BytePlus state: {other}"
            ))),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn parses_recorded_image() {
        assert!(matches!(
            &image_outputs(&json!({"data":[{"url":"https://example.com/a.jpg"}]})).unwrap()[0],
            Output::Url { .. }
        ));
    }
    #[test]
    fn maps_video_options_to_top_level() {
        let mut params = serde_json::Map::new();
        params.insert(keys::ASPECT_RATIO.into(), json!("16:9"));
        params.insert(keys::DURATION.into(), json!(8));
        params.insert(keys::GENERATE_AUDIO.into(), json!(false));
        let req = GenerationRequest {
            job_id: "t".into(),
            model: "dreamina-seedance-2-5-260628".into(),
            prompt: "move".into(),
            negative_prompt: None,
            params,
            inputs: vec![],
            num_outputs: 1,
        };
        let body = video_options(&req);
        assert_eq!(body["ratio"], "16:9");
        assert_eq!(body["duration"], 8);
        assert_eq!(body["generate_audio"], false);
        assert_eq!(body["content"][0]["text"], "move");
    }
}
