use crate::util::{failure, get_json, send_json};
use async_trait::async_trait;
use pictesa_core::{
    keys, CredentialField, Ctx, GenerationRequest, InputRole, InputSlot, MediaKind, ModelSpec,
    Output, ParamSpec, PollStatus, Pricing, Provider, ProviderError, ProviderInfo, ProviderKind,
    ProviderResult, RemoteHandle, Submission, Task,
};
use serde_json::{json, Map, Value};

pub struct Bfl;
const API: &str = "https://api.bfl.ai/v1";

fn image_spec(id: &str, price: f64, max_refs: u32) -> ModelSpec {
    let mut m = ModelSpec::new(
        "bfl",
        id,
        &crate::util::slug_name(id),
        MediaKind::Image,
        &[Task::TextToImage, Task::ImageEdit],
    );
    m.vendor = Some("Black Forest Labs".into());
    m.inputs = vec![InputSlot::new(
        InputRole::Reference,
        MediaKind::Image,
        max_refs,
    )];
    m.params = vec![
        ParamSpec::choices(
            keys::ASPECT_RATIO,
            "Aspect ratio",
            &["1:1", "16:9", "9:16", "4:3", "3:4", "3:2", "2:3"],
        ),
        ParamSpec::choices(keys::RESOLUTION, "Resolution", &["1k", "2k", "4k"]),
        ParamSpec::int(keys::WIDTH, "Width", 256, 6256),
        ParamSpec::int(keys::HEIGHT, "Height", 256, 6256),
        ParamSpec::seed(),
        ParamSpec::choices(keys::OUTPUT_FORMAT, "Format", &["jpeg", "png", "webp"]),
    ];
    m.params.retain(|p| {
        if id == "flux-3-image" {
            p.key != keys::SEED && p.key != keys::OUTPUT_FORMAT
        } else if id.starts_with("flux-2-") {
            p.key != keys::ASPECT_RATIO && p.key != keys::RESOLUTION
        } else if id.contains("kontext") {
            p.key != keys::RESOLUTION && p.key != keys::WIDTH && p.key != keys::HEIGHT
        } else {
            true
        }
    });
    m.pricing = Some(Pricing::Variable {
        note: if price == 0.0 {
            "BFL image pricing varies by resolution".into()
        } else {
            format!("from ${price:.3} per image; resolution and input affect price")
        },
    });
    m
}
fn video_spec() -> ModelSpec {
    let mut m = ModelSpec::new(
        "bfl",
        "flux-3-video",
        "FLUX 3 Video",
        MediaKind::Video,
        &[Task::TextToVideo, Task::ImageToVideo, Task::VideoToVideo],
    );
    m.vendor = Some("Black Forest Labs".into());
    m.inputs = vec![
        InputSlot::new(InputRole::StartFrame, MediaKind::Image, 1),
        InputSlot::new(InputRole::EndFrame, MediaKind::Image, 1),
        InputSlot::new(InputRole::Video, MediaKind::Video, 1),
    ];
    m.params = vec![
        ParamSpec::int(keys::DURATION, "Duration", 5, 20),
        ParamSpec::choices(
            keys::ASPECT_RATIO,
            "Aspect ratio",
            &["auto", "16:9", "9:16", "4:3", "3:4", "1:1"],
        ),
        ParamSpec::choices(keys::RESOLUTION, "Resolution", &["hd", "fhd", "qhd", "uhd"]),
        ParamSpec::boolean(keys::GENERATE_AUDIO, "Generate audio"),
    ];
    m.pricing = Some(Pricing::PerSecond { usd: 0.17 });
    m
}
fn image_body(model: &ModelSpec, req: &GenerationRequest) -> Map<String, Value> {
    let mut body = Map::new();
    body.insert("prompt".into(), json!(req.prompt));
    let is_3 = model.remote_id().starts_with("flux-3-");
    let kontext = model.remote_id().contains("kontext");
    let mut fields = if model.remote_id() == "flux-3-image" {
        vec![]
    } else {
        vec![(keys::SEED, "seed"), (keys::OUTPUT_FORMAT, "output_format")]
    };
    if is_3 {
        fields.extend([
            (keys::ASPECT_RATIO, "aspect_ratio"),
            (keys::RESOLUTION, "resolution"),
            (keys::WIDTH, "width"),
            (keys::HEIGHT, "height"),
        ]);
    } else if kontext {
        fields.push((keys::ASPECT_RATIO, "aspect_ratio"));
    } else {
        fields.extend([(keys::WIDTH, "width"), (keys::HEIGHT, "height")]);
    }
    req.copy_params(&mut body, &fields);
    body
}
fn result(v: &Value) -> ProviderResult<Vec<Output>> {
    let url = v
        .pointer("/result/sample")
        .or_else(|| v.pointer("/result/video"))
        .or_else(|| v.pointer("/result/url"))
        .and_then(Value::as_str)
        .ok_or_else(|| ProviderError::Failed("BFL finished without media URL".into()))?;
    Ok(vec![Output::url(url)])
}

#[async_trait]
impl Provider for Bfl {
    fn info(&self) -> ProviderInfo {
        ProviderInfo {
            id: "bfl".into(),
            name: "Black Forest Labs".into(),
            kind: ProviderKind::Cloud,
            fields: vec![CredentialField::api_key(&["BFL_API_KEY"])],
            signup_url: Some("https://dashboard.bfl.ai".into()),
            docs_url: Some("https://docs.bfl.ai".into()),
            max_concurrency: 4,
        }
    }
    fn models(&self) -> Vec<ModelSpec> {
        let mut m = vec![
            image_spec("flux-3-image", 0.0, 10),
            image_spec("flux-3-image-klein", 0.0, 10),
            image_spec("flux-2-pro", 0.03, 8),
            image_spec("flux-2-max", 0.07, 8),
            image_spec("flux-2-flex", 0.05, 8),
            image_spec("flux-2-klein-4b", 0.014, 4),
            image_spec("flux-2-klein-9b", 0.015, 4),
            image_spec("flux-kontext-pro", 0.04, 4),
            image_spec("flux-kontext-max", 0.08, 4),
        ];
        m.push(video_spec());
        m
    }
    async fn check(&self, ctx: &Ctx) -> ProviderResult<()> {
        get_json(
            ctx.http
                .get(format!("{API}/credits"))
                .header("x-key", ctx.api_key()?),
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
        let mut body = if model.output == MediaKind::Image {
            Value::Object(image_body(model, req))
        } else {
            let mut b = json!({"prompt":req.prompt});
            req.copy_params(
                b.as_object_mut().unwrap(),
                &[
                    (keys::DURATION, "duration"),
                    (keys::ASPECT_RATIO, "aspect_ratio"),
                    (keys::RESOLUTION, "resolution"),
                    (keys::GENERATE_AUDIO, "generate_audio"),
                ],
            );
            b
        };
        if model.output == MediaKind::Image {
            let mut refs = Vec::new();
            for input in req.inputs(InputRole::Reference) {
                refs.push(if let Some(url) = &input.url {
                    url.clone()
                } else {
                    input.base64().await?
                });
            }
            if model.remote_id().starts_with("flux-3-") {
                if !refs.is_empty() {
                    body["images"] = json!(refs);
                }
            } else {
                for (i, url) in refs.iter().enumerate() {
                    let name = if i == 0 {
                        "input_image".to_string()
                    } else {
                        format!("input_image_{}", i + 1)
                    };
                    body[&name] = json!(url);
                }
            }
        } else if let Some(input) = req.input(InputRole::Video) {
            body["mode"] = json!("v2v");
            body["start_video"] = json!(if let Some(url) = &input.url {
                url.clone()
            } else {
                input.base64().await?
            });
        } else {
            let mut frames = Vec::new();
            for role in [InputRole::StartFrame, InputRole::EndFrame] {
                if let Some(input) = req.input(role) {
                    frames.push(if let Some(url) = &input.url {
                        url.clone()
                    } else {
                        input.base64().await?
                    });
                }
            }
            if frames.is_empty() {
                body["mode"] = json!("t2v");
            } else {
                body["mode"] = json!("i2v");
                body["keyframes"] = json!(frames);
            }
        }
        let v = send_json(
            ctx.http
                .post(format!("{API}/{}", model.remote_id()))
                .header("x-key", ctx.api_key()?),
            &body,
        )
        .await?;
        let id = v
            .get("id")
            .and_then(Value::as_str)
            .ok_or_else(|| ProviderError::Failed("BFL returned no id".into()))?;
        let url = v
            .get("polling_url")
            .and_then(Value::as_str)
            .ok_or_else(|| ProviderError::Failed("BFL returned no polling URL".into()))?;
        Ok(Submission::Pending(RemoteHandle::with(
            id,
            json!({"polling_url":url}),
        )))
    }
    async fn poll(&self, ctx: &Ctx, handle: &RemoteHandle) -> ProviderResult<PollStatus> {
        let url = handle
            .data_str("polling_url")
            .ok_or_else(|| ProviderError::Failed("BFL handle has no polling URL".into()))?;
        let v = get_json(ctx.http.get(url).header("x-key", ctx.api_key()?)).await?;
        match v.get("status").and_then(Value::as_str).unwrap_or("") {
            "Pending" | "Task not found" => Ok(PollStatus::Queued { position: None }),
            "Processing" => Ok(PollStatus::Running {
                progress: v.get("progress").and_then(Value::as_f64).map(|n| n as f32),
                message: None,
            }),
            "Ready" => Ok(PollStatus::Done(result(&v)?)),
            "Error" | "Content Moderated" | "Request Moderated" => Ok(PollStatus::Failed(failure(
                v.get("error")
                    .and_then(Value::as_str)
                    .unwrap_or("BFL generation failed"),
            ))),
            other => Err(ProviderError::Failed(format!("unknown BFL state: {other}"))),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn parses_recorded_result() {
        assert!(matches!(
            &result(&json!({"status":"Ready","result":{"sample":"https://example.com/a.png"}}))
                .unwrap()[0],
            Output::Url { .. }
        ));
    }
    #[test]
    fn maps_flux_3_image_without_unsupported_seed() {
        let mut params = Map::new();
        params.insert(keys::ASPECT_RATIO.into(), json!("16:9"));
        params.insert(keys::SEED.into(), json!(42));
        let req = GenerationRequest {
            job_id: "t".into(),
            model: "flux-3-image".into(),
            prompt: "draw".into(),
            negative_prompt: None,
            params,
            inputs: vec![],
            num_outputs: 1,
        };
        let body = image_body(&image_spec("flux-3-image", 0.0, 10), &req);
        assert_eq!(body["aspect_ratio"], "16:9");
        assert!(!body.contains_key("seed"));
    }
}
