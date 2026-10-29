use async_trait::async_trait;
use pictesa_core::{
    common, keys, CredentialField, Ctx, GenerationRequest, InputRole, InputSlot, MediaInput,
    MediaKind, ModelSpec, Output, PollStatus, Provider, ProviderError, ProviderInfo, ProviderKind,
    ProviderResult, RemoteHandle, Submission, Task,
};
use serde_json::{json, Value};

use crate::util::{failure, get_json, send_json};

const BASE: &str = "https://api.lumalabs.ai/dream-machine/v1/generations";

pub struct Luma;

fn public_url(input: &MediaInput) -> ProviderResult<&str> {
    input
        .url
        .as_deref()
        .filter(|u| u.starts_with("https://"))
        .ok_or_else(|| ProviderError::InvalidInput("Luma requires a public HTTPS input URL".into()))
}

fn body(model: &ModelSpec, req: &GenerationRequest) -> ProviderResult<Value> {
    let mut b = json!({"model": model.remote_id(), "prompt": req.prompt});
    if let Some(ratio) = req.str(keys::ASPECT_RATIO) {
        b["aspect_ratio"] = json!(ratio);
    }
    if model.output == MediaKind::Video {
        if let Some(r) = req.str(keys::RESOLUTION) {
            b["resolution"] = json!(r);
        }
        if let Some(d) = req.i64(keys::DURATION) {
            b["duration"] = json!(format!("{d}s"));
        }
        if let Some(looped) = req.bool(keys::LOOP) {
            b["loop"] = json!(looped);
        }
        let mut frames = serde_json::Map::new();
        for (role, key) in [
            (InputRole::StartFrame, "frame0"),
            (InputRole::EndFrame, "frame1"),
        ] {
            if let Some(input) = req.input(role) {
                frames.insert(
                    key.into(),
                    json!({"type":"image", "url": public_url(input)?}),
                );
            }
        }
        if !frames.is_empty() {
            b["keyframes"] = Value::Object(frames);
        }
    } else {
        if let Some(f) = req.str(keys::OUTPUT_FORMAT) {
            b["format"] = json!(f);
        }
        let urls: Vec<&str> = req
            .inputs(InputRole::Reference)
            .map(public_url)
            .collect::<ProviderResult<_>>()?;
        let weight = req.f64(keys::STRENGTH).unwrap_or(0.85);
        let refs: Vec<Value> = urls
            .iter()
            .map(|url| json!({"url":url,"weight":weight}))
            .collect();
        if !refs.is_empty() {
            match req.str("reference_mode").unwrap_or("image") {
                "style" => b["style_ref"] = json!(refs),
                "character" => b["character_ref"] = json!({"identity0":{"images":urls}}),
                "modify" => {
                    if refs.len() != 1 {
                        return Err(ProviderError::InvalidInput(
                            "Luma modify accepts one source image".into(),
                        ));
                    }
                    b["modify_image_ref"] = refs[0].clone();
                }
                _ => b["image_ref"] = json!(refs),
            }
        }
    }
    Ok(b)
}

fn parse_generation(v: &Value, kind: MediaKind) -> ProviderResult<PollStatus> {
    match v.get("state").and_then(Value::as_str).unwrap_or("") {
        "queued" | "pending" => Ok(PollStatus::Queued { position: None }),
        "dreaming" | "processing" => Ok(PollStatus::Running {
            progress: None,
            message: None,
        }),
        "completed" => {
            let key = if kind == MediaKind::Video {
                "video"
            } else {
                "image"
            };
            let url = v
                .pointer(&format!("/assets/{key}"))
                .and_then(Value::as_str)
                .ok_or_else(|| {
                    ProviderError::Failed(format!("Luma completed without {key} URL"))
                })?;
            Ok(PollStatus::Done(vec![Output::url(url)]))
        }
        "failed" => {
            let reason = v
                .get("failure_reason")
                .and_then(Value::as_str)
                .unwrap_or("Luma generation failed");
            // Luma's errors page lists "Contains blacklisted words" under moderation failures.
            Ok(PollStatus::Failed(if reason.contains("blacklisted") {
                ProviderError::ContentPolicy(reason.into())
            } else {
                failure(reason)
            }))
        }
        other => Err(ProviderError::Failed(format!(
            "unknown Luma state: {other}"
        ))),
    }
}

#[async_trait]
impl Provider for Luma {
    fn info(&self) -> ProviderInfo {
        ProviderInfo {
            id: "luma".into(),
            name: "Luma Dream Machine".into(),
            kind: ProviderKind::Cloud,
            fields: vec![CredentialField::api_key(&[
                "LUMAAI_API_KEY",
                "LUMA_API_KEY",
            ])],
            signup_url: Some("https://lumalabs.ai/dream-machine/api".into()),
            docs_url: Some("https://docs.lumalabs.ai/docs/api".into()),
            max_concurrency: 4,
        }
    }
    fn models(&self) -> Vec<ModelSpec> {
        let mut v = Vec::new();
        for (id, name) in [("ray-2", "Ray 2"), ("ray-flash-2", "Ray 2 Flash")] {
            v.push(
                ModelSpec::new(
                    "luma",
                    id,
                    name,
                    MediaKind::Video,
                    &[Task::TextToVideo, Task::ImageToVideo],
                )
                .input(InputSlot::new(InputRole::StartFrame, MediaKind::Image, 1))
                .input(InputSlot::new(InputRole::EndFrame, MediaKind::Image, 1))
                .param(common::aspect_ratio(
                    &["1:1", "16:9", "9:16", "4:3", "3:4", "21:9", "9:21"],
                    "16:9",
                ))
                .param(common::resolution(&["540p", "720p", "1080p", "4k"], "720p"))
                .param(common::duration(&[5, 9], 5))
                .param(pictesa_core::ParamSpec::boolean(keys::LOOP, "Loop").default_value(false)),
            );
        }
        for (id, name) in [
            ("photon-1", "Photon 1"),
            ("photon-flash-1", "Photon Flash 1"),
        ] {
            v.push(
                ModelSpec::new(
                    "luma",
                    id,
                    name,
                    MediaKind::Image,
                    &[Task::TextToImage, Task::ImageEdit],
                )
                .input(InputSlot::new(InputRole::Reference, MediaKind::Image, 4))
                .param(common::aspect_ratio(
                    &["1:1", "16:9", "9:16", "4:3", "3:4", "21:9", "9:21"],
                    "16:9",
                ))
                .param(
                    pictesa_core::ParamSpec::choices(
                        keys::OUTPUT_FORMAT,
                        "Format",
                        &["jpg", "png"],
                    )
                    .default_value("jpg"),
                )
                .param(
                    pictesa_core::ParamSpec::choices(
                        "reference_mode",
                        "Reference use",
                        &["image", "style", "character", "modify"],
                    )
                    .default_value("image"),
                )
                .param(
                    pictesa_core::ParamSpec::float(
                        keys::STRENGTH,
                        "Reference weight",
                        0.0,
                        1.0,
                        0.01,
                    )
                    .default_value(0.85)
                    .advanced(),
                ),
            );
        }
        v
    }
    async fn check(&self, ctx: &Ctx) -> ProviderResult<()> {
        get_json(
            ctx.http
                .get(BASE)
                .query(&[("limit", "1")])
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
        let path = if model.output == MediaKind::Image {
            "/image"
        } else {
            "/video"
        };
        let answer = send_json(
            ctx.http
                .post(format!("{BASE}{path}"))
                .bearer_auth(ctx.api_key()?),
            &body(model, req)?,
        )
        .await?;
        let id = answer
            .get("id")
            .and_then(Value::as_str)
            .ok_or_else(|| ProviderError::Failed("Luma returned no generation id".into()))?;
        Ok(Submission::Pending(RemoteHandle::with(
            id,
            json!({"kind": model.output}),
        )))
    }
    async fn poll(&self, ctx: &Ctx, handle: &RemoteHandle) -> ProviderResult<PollStatus> {
        let v = get_json(
            ctx.http
                .get(format!("{BASE}/{}", handle.id))
                .bearer_auth(ctx.api_key()?),
        )
        .await?;
        let kind = handle
            .data
            .get("kind")
            .and_then(|v| serde_json::from_value::<MediaKind>(v.clone()).ok())
            .unwrap_or(MediaKind::Video);
        parse_generation(&v, kind)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use pictesa_core::MediaInput;
    use std::path::PathBuf;
    #[test]
    fn maps_keyframes_and_duration() {
        let mut req = GenerationRequest {
            job_id: "x".into(),
            model: "ray-2".into(),
            prompt: "move".into(),
            negative_prompt: None,
            params: serde_json::Map::new(),
            inputs: vec![],
            num_outputs: 1,
        };
        for (role, url) in [
            (InputRole::StartFrame, "https://example.com/first.png"),
            (InputRole::EndFrame, "https://example.com/last.png"),
        ] {
            req.inputs.push(MediaInput {
                role,
                kind: MediaKind::Image,
                mime: "image/png".into(),
                path: PathBuf::new(),
                url: Some(url.into()),
                width: None,
                height: None,
            });
        }
        req.params.insert(keys::DURATION.into(), json!(9));
        req.params.insert(keys::LOOP.into(), json!(true));
        let model = Luma.models().remove(0);
        let b = body(&model, &req).unwrap();
        assert_eq!(
            b["keyframes"]["frame1"]["url"],
            "https://example.com/last.png"
        );
        assert_eq!(b["duration"], "9s");
        assert_eq!(b["loop"], true);
    }
    #[test]
    fn maps_photon_modify_reference() {
        let model = Luma
            .models()
            .into_iter()
            .find(|m| m.remote_id() == "photon-1")
            .unwrap();
        let mut req = GenerationRequest {
            job_id: "x".into(),
            model: "photon-1".into(),
            prompt: "change flowers".into(),
            negative_prompt: None,
            params: serde_json::Map::new(),
            inputs: vec![MediaInput {
                role: InputRole::Reference,
                kind: MediaKind::Image,
                mime: "image/png".into(),
                path: PathBuf::new(),
                url: Some("https://example.com/flowers.png".into()),
                width: None,
                height: None,
            }],
            num_outputs: 1,
        };
        req.params.insert("reference_mode".into(), json!("modify"));
        let b = body(&model, &req).unwrap();
        assert_eq!(
            b["modify_image_ref"]["url"],
            "https://example.com/flowers.png"
        );
        assert!(b.get("image_ref").is_none());
    }
    #[test]
    fn response_fixtures() {
        assert!(matches!(
            parse_generation(&json!({"state":"dreaming"}), MediaKind::Video).unwrap(),
            PollStatus::Running { .. }
        ));
        assert!(matches!(
            parse_generation(
                &json!({"state":"completed","assets":{"video":"https://example.com/v.mp4"}}),
                MediaKind::Video
            )
            .unwrap(),
            PollStatus::Done(_)
        ));
        assert!(matches!(
            parse_generation(
                &json!({"state":"failed","failure_reason":"safety policy"}),
                MediaKind::Video
            )
            .unwrap(),
            PollStatus::Failed(ProviderError::ContentPolicy(_))
        ));
    }
}
