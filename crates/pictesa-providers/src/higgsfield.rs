use std::time::Duration;

use async_trait::async_trait;
use pictesa_core::{
    collect_media_urls, common, json_response, keys, CredentialField, Ctx, GenerationRequest,
    InputRole, InputSlot, MediaInput, MediaKind, ModelSpec, Output, ParamSpec, PollStatus, Pricing,
    Provider, ProviderError, ProviderInfo, ProviderKind, ProviderResult, RemoteHandle, Submission,
    Task,
};
use serde_json::{json, Value};

use crate::util::{failure, net_error, API_TIMEOUT};

pub struct Higgsfield;
const API: &str = "https://api.higgsfield.ai";

fn auth(ctx: &Ctx) -> ProviderResult<String> {
    Ok(format!(
        "Key {}:{}",
        ctx.require("key_id")?,
        ctx.require("key_secret")?
    ))
}
/// Sends a Higgsfield API call. Higgsfield answers 403 for an empty balance (docs: Errors and
/// retries; the official SDK reads every 403 as NotEnoughCreditsError), not for a bad key.
async fn call(req: reqwest::RequestBuilder) -> ProviderResult<Value> {
    let resp = req.timeout(API_TIMEOUT).send().await.map_err(net_error)?;
    if resp.status() == reqwest::StatusCode::FORBIDDEN {
        let text = resp.text().await.unwrap_or_default();
        return Err(ProviderError::InsufficientCredits(
            pictesa_core::error::truncate(&text, 600),
        ));
    }
    json_response(resp).await
}

fn image(id: &str, name: &str, edit: bool) -> ModelSpec {
    let tasks = if edit {
        vec![Task::ImageEdit]
    } else {
        vec![Task::TextToImage]
    };
    let mut m = ModelSpec::new("higgsfield", id, name, MediaKind::Image, &tasks)
        .param(common::aspect_ratio(
            &["1:1", "4:3", "3:4", "16:9", "9:16", "3:2", "2:3"],
            "1:1",
        ))
        .param(common::resolution(
            if id.contains("soul") {
                &["720p", "1080p"]
            } else {
                &["1k", "2k"]
            },
            if id.contains("soul") { "720p" } else { "1k" },
        ))
        .param(ParamSpec::seed())
        .pricing(Pricing::Variable {
            note: "Higgsfield USD endpoint pricing".into(),
        });
    if edit {
        m = m.input(InputSlot::new(InputRole::Reference, MediaKind::Image, 3).required());
    }
    m
}
fn video(id: &str, name: &str, mode: &str) -> ModelSpec {
    let tasks = match mode {
        "t2v" => vec![Task::TextToVideo],
        "v2v" => vec![Task::VideoToVideo],
        "refs" => vec![Task::TextToVideo, Task::ImageToVideo],
        _ => vec![Task::ImageToVideo],
    };
    let mut m = ModelSpec::new("higgsfield", id, name, MediaKind::Video, &tasks)
        .param(common::resolution(
            if id.contains("cinema-studio") {
                &["480p", "720p"]
            } else {
                &["480p", "720p", "1080p"]
            },
            "720p",
        ))
        .pricing(Pricing::Variable {
            note: "Higgsfield USD endpoint pricing".into(),
        });
    // The video schemas reject unknown fields: none has a seed, and image-to-video takes its
    // framing from the image, so it has no aspect_ratio.
    if mode != "v2v" {
        m = m.param(ParamSpec::int(keys::DURATION, "Duration (seconds)", 4, 30).default_value(5));
        if mode != "i2v" {
            m = m.param(common::aspect_ratio(
                &["16:9", "9:16", "1:1", "4:3", "3:4", "21:9"],
                "16:9",
            ));
        }
        m = m.param(common::generate_audio(true));
    }
    match mode {
        "i2v" => {
            m = m
                .input(InputSlot::new(InputRole::StartFrame, MediaKind::Image, 1).required())
                .input(InputSlot::new(InputRole::EndFrame, MediaKind::Image, 1))
        }
        "refs" => {
            m = m
                .input(InputSlot::new(InputRole::Reference, MediaKind::Image, 30))
                .input(InputSlot::new(InputRole::Video, MediaKind::Video, 10))
                .input(InputSlot::new(InputRole::Audio, MediaKind::Audio, 10))
        }
        "v2v" => {
            m = m
                .input(InputSlot::new(InputRole::Video, MediaKind::Video, 1).required())
                .input(InputSlot::new(InputRole::Reference, MediaKind::Image, 5))
                .param(ParamSpec::text("preset_id", "Style preset ID"))
        }
        _ => {}
    }
    m
}

async fn public_url(ctx: &Ctx, input: &MediaInput) -> ProviderResult<String> {
    if let Some(url) = &input.url {
        if url.starts_with("https://") {
            return Ok(url.clone());
        }
    }
    let v = call(
        ctx.http
            .post(format!("{API}/files/generate-upload-url"))
            .header("Authorization", auth(ctx)?)
            .json(&json!({"content_type":input.mime})),
    )
    .await?;
    let upload = v
        .get("upload_url")
        .and_then(Value::as_str)
        .ok_or_else(|| ProviderError::Failed("Higgsfield returned no upload URL".into()))?;
    let public = v
        .get("public_url")
        .and_then(Value::as_str)
        .ok_or_else(|| ProviderError::Failed("Higgsfield returned no public URL".into()))?;
    if !upload.starts_with("https://") || !public.starts_with("https://") {
        return Err(ProviderError::Failed(
            "Higgsfield returned an insecure upload URL".into(),
        ));
    }
    let mut put = ctx.http.put(upload).body(input.bytes().await?);
    if let Some(headers) = v.get("upload_headers").and_then(Value::as_object) {
        for (k, val) in headers {
            if let Some(s) = val.as_str() {
                put = put.header(k.as_str(), s);
            }
        }
    }
    let resp = put.send().await.map_err(net_error)?;
    if !resp.status().is_success() {
        return Err(ProviderError::from_http(
            resp.status().as_u16(),
            "presigned upload failed",
            None,
        ));
    }
    Ok(public.into())
}

async fn body(ctx: &Ctx, model: &ModelSpec, req: &GenerationRequest) -> ProviderResult<Value> {
    let id = model.remote_id();
    let mut b = json!({"prompt":req.prompt});
    req.copy_params(
        b.as_object_mut().unwrap(),
        &[
            (keys::ASPECT_RATIO, "aspect_ratio"),
            (keys::RESOLUTION, "resolution"),
            (keys::DURATION, "duration"),
            (keys::GENERATE_AUDIO, "generate_audio"),
            (keys::SEED, "seed"),
            (keys::ENHANCE_PROMPT, "enhance_prompt"),
        ],
    );
    if id.contains("genjutsu/restyle") {
        let preset = req.str("preset_id").ok_or_else(|| {
            ProviderError::InvalidInput("Genjutsu restyle needs a preset ID".into())
        })?;
        b["preset_id"] = json!(preset);
        let source = req.input(InputRole::Video).ok_or_else(|| {
            ProviderError::InvalidInput("Genjutsu restyle needs a source video".into())
        })?;
        b["video_url"] = json!(public_url(ctx, source).await?);
        b.as_object_mut().unwrap().remove("duration");
        b.as_object_mut().unwrap().remove("aspect_ratio");
        b.as_object_mut().unwrap().remove("generate_audio");
        b.as_object_mut().unwrap().remove("seed");
    } else if id.ends_with("image-to-video") {
        let source = req.input(InputRole::StartFrame).ok_or_else(|| {
            ProviderError::InvalidInput("image-to-video needs a start frame".into())
        })?;
        b["image_url"] = json!(public_url(ctx, source).await?);
        if let Some(end) = req.input(InputRole::EndFrame) {
            b["end_image_url"] = json!(public_url(ctx, end).await?);
        }
    }
    if id.ends_with("/edit")
        || id.ends_with("reference-to-video")
        || id.contains("cinema-studio")
        || id.contains("genjutsu/restyle")
    {
        let mut images = Vec::new();
        for input in req.inputs(InputRole::Reference) {
            images.push(public_url(ctx, input).await?);
        }
        if id.ends_with("/edit") && images.is_empty() {
            return Err(ProviderError::InvalidInput(
                "image editing needs reference images".into(),
            ));
        }
        if !images.is_empty() {
            b["image_urls"] = json!(images);
        }
    }
    if id.ends_with("reference-to-video") || id.contains("cinema-studio") {
        let mut videos = Vec::new();
        for input in req.inputs(InputRole::Video) {
            videos.push(public_url(ctx, input).await?);
        }
        let mut audio = Vec::new();
        for input in req.inputs(InputRole::Audio) {
            audio.push(public_url(ctx, input).await?);
        }
        if !videos.is_empty() {
            b["video_urls"] = json!(videos);
        }
        if !audio.is_empty() {
            b["audio_urls"] = json!(audio);
        }
    }
    Ok(b)
}

fn status(v: &Value, kind: MediaKind) -> ProviderResult<PollStatus> {
    match v.get("status").and_then(Value::as_str).unwrap_or("") {
        "queued" => Ok(PollStatus::Queued { position: None }),
        "in_progress" => Ok(PollStatus::Running {
            progress: None,
            message: None,
        }),
        "completed" => {
            let field = if kind == MediaKind::Image {
                &v["images"]
            } else {
                &v["video"]
            };
            let urls = collect_media_urls(field, Some(kind));
            if urls.is_empty() {
                return Err(ProviderError::Failed(
                    "Higgsfield completed without media".into(),
                ));
            }
            Ok(PollStatus::Done(
                urls.into_iter().map(Output::url).collect(),
            ))
        }
        "nsfw" => Ok(PollStatus::Failed(ProviderError::ContentPolicy(
            v.get("error")
                .and_then(Value::as_str)
                .unwrap_or("Higgsfield moderation rejected the input or output")
                .into(),
        ))),
        "failed" | "canceled" => Ok(PollStatus::Failed(failure(
            v.pointer("/error/message")
                .and_then(Value::as_str)
                .or_else(|| v.get("error").and_then(Value::as_str))
                .unwrap_or("Higgsfield request failed"),
        ))),
        state => Err(ProviderError::Failed(format!(
            "unknown Higgsfield state: {state}"
        ))),
    }
}

#[async_trait]
impl Provider for Higgsfield {
    fn info(&self) -> ProviderInfo {
        ProviderInfo {
            id: "higgsfield".into(),
            name: "Higgsfield".into(),
            kind: ProviderKind::Cloud,
            fields: vec![
                CredentialField {
                    key: "key_id".into(),
                    label: "API key ID".into(),
                    env: vec!["HF_API_KEY_ID".into()],
                    secret: true,
                    optional: false,
                    default: None,
                    placeholder: None,
                },
                CredentialField {
                    key: "key_secret".into(),
                    label: "API key secret".into(),
                    env: vec!["HF_API_KEY_SECRET".into()],
                    secret: true,
                    optional: false,
                    default: None,
                    placeholder: None,
                },
            ],
            signup_url: Some("https://console.higgsfield.ai".into()),
            docs_url: Some("https://docs.higgsfield.ai/docs/llms.txt".into()),
            max_concurrency: 4,
        }
    }
    fn models(&self) -> Vec<ModelSpec> {
        // shortcut: catalog only includes workflows with verified field mappings, add more as their schemas are checked.
        vec![
            image("higgsfield-ai/soul/standard", "SOUL", false),
            image("higgsfield-ai/soul/v2/standard", "SOUL 2", false),
            image("alibaba/qwen-image-3/text-to-image", "Qwen Image 3", false),
            image("alibaba/qwen-image-3/edit", "Qwen Image 3 Edit", true),
            video(
                "bytedance/seedance-2.5/text-to-video",
                "Seedance 2.5 Text",
                "t2v",
            ),
            video(
                "bytedance/seedance-2.5/image-to-video",
                "Seedance 2.5 Image",
                "i2v",
            ),
            video(
                "bytedance/seedance-2.5/reference-to-video",
                "Seedance 2.5 References",
                "refs",
            ),
            video("higgsfield/cinema-studio/4.0", "Cinema Studio 4.0", "refs"),
            video(
                "higgsfield/genjutsu/restyle/v1.0",
                "Genjutsu Restyle",
                "v2v",
            ),
        ]
    }
    async fn check(&self, ctx: &Ctx) -> ProviderResult<()> {
        call(
            ctx.http
                .get(format!("{API}/models/usage"))
                .header("Authorization", auth(ctx)?),
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
        let b = body(ctx, model, req).await?;
        let v = call(
            ctx.http
                .post(format!("{API}/{}", model.remote_id()))
                .header("Authorization", auth(ctx)?)
                .header("Idempotency-Key", &req.job_id)
                .json(&b),
        )
        .await?;
        let id = v
            .get("request_id")
            .and_then(Value::as_str)
            .ok_or_else(|| ProviderError::Failed("Higgsfield returned no request ID".into()))?;
        let url = v
            .get("status_url")
            .and_then(Value::as_str)
            .ok_or_else(|| ProviderError::Failed("Higgsfield returned no status URL".into()))?;
        if !url::Url::parse(url)
            .is_ok_and(|u| u.scheme() == "https" && u.host_str() == Some("api.higgsfield.ai"))
        {
            return Err(ProviderError::Failed(
                "Higgsfield returned an invalid status URL".into(),
            ));
        }
        Ok(Submission::Pending(RemoteHandle::with(
            id,
            json!({"status_url":url,"kind":model.output}),
        )))
    }
    async fn poll(&self, ctx: &Ctx, handle: &RemoteHandle) -> ProviderResult<PollStatus> {
        let url = handle
            .data_str("status_url")
            .ok_or_else(|| ProviderError::Failed("Higgsfield handle has no status URL".into()))?;
        if !url::Url::parse(url)
            .is_ok_and(|u| u.scheme() == "https" && u.host_str() == Some("api.higgsfield.ai"))
        {
            return Err(ProviderError::Failed(
                "Higgsfield handle has an invalid status URL".into(),
            ));
        }
        let kind = handle
            .data
            .get("kind")
            .and_then(|v| serde_json::from_value(v.clone()).ok())
            .unwrap_or(MediaKind::Image);
        let v = call(ctx.http.get(url).header("Authorization", auth(ctx)?)).await?;
        status(&v, kind)
    }
    async fn cancel(&self, ctx: &Ctx, handle: &RemoteHandle) -> ProviderResult<()> {
        let response = ctx
            .http
            .post(format!("{API}/requests/{}/cancel", handle.id))
            .header("Authorization", auth(ctx)?)
            .send()
            .await
            .map_err(net_error)?;
        if !response.status().is_success() {
            return Err(ProviderError::from_http(
                response.status().as_u16(),
                "Higgsfield cancel failed",
                None,
            ));
        }
        Ok(())
    }
    fn poll_interval(&self, attempt: u32) -> Duration {
        Duration::from_secs((2 + attempt as u64).min(10))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use pictesa_core::MediaInput;
    #[test]
    fn parses_recorded_request_states() {
        let done = json!({"status":"completed","request_id":"x","images":[{"url":"https://cdn.example.test/a.png"}]});
        assert!(matches!(
            status(&done, MediaKind::Image).unwrap(),
            PollStatus::Done(_)
        ));
        let blocked = json!({"status":"nsfw","error":"content moderation"});
        assert!(matches!(
            status(&blocked, MediaKind::Image).unwrap(),
            PollStatus::Failed(ProviderError::ContentPolicy(_))
        ));
    }
    #[tokio::test]
    async fn maps_seedance_frame_urls_without_upload() {
        let ctx = Ctx::new(reqwest::Client::new(), std::collections::HashMap::new());
        let mut req:GenerationRequest=serde_json::from_value(json!({"job_id":"x","model":"bytedance/seedance-2.5/image-to-video","prompt":"move","params":{"duration":8,"generate_audio":true},"inputs":[],"num_outputs":1})).unwrap();
        req.inputs.push(MediaInput {
            role: InputRole::StartFrame,
            kind: MediaKind::Image,
            mime: "image/png".into(),
            path: std::path::PathBuf::new(),
            url: Some("https://example.test/start.png".into()),
            width: None,
            height: None,
        });
        let b = body(
            &ctx,
            &video(
                "bytedance/seedance-2.5/image-to-video",
                "Seedance 2.5 Image",
                "i2v",
            ),
            &req,
        )
        .await
        .unwrap();
        assert_eq!(b["image_url"], "https://example.test/start.png");
        assert_eq!(b["duration"], 8);
        assert_eq!(b["generate_audio"], true);
    }
}
