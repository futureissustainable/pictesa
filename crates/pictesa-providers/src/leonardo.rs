use std::time::Duration;

use async_trait::async_trait;
use pictesa_core::{
    keys, CredentialField, Ctx, GenerationRequest, MediaKind, ModelSpec, Output, ParamSpec,
    PollStatus, Pricing, Provider, ProviderError, ProviderInfo, ProviderKind, ProviderResult,
    RemoteHandle, Submission, Task,
};
use serde_json::{json, Value};

use crate::util::{failure, get_json, send_json};

pub struct Leonardo;
const API: &str = "https://cloud.leonardo.ai/api/rest/v1";

fn image(id: &str, name: &str) -> ModelSpec {
    ModelSpec::new("leonardo", id, name, MediaKind::Image, &[Task::TextToImage])
        .param(ParamSpec::int(keys::WIDTH, "Width", 32, 1536).default_value(1024))
        .param(ParamSpec::int(keys::HEIGHT, "Height", 32, 1536).default_value(1024))
        .param(ParamSpec::int(keys::STEPS, "Steps", 10, 60).advanced())
        .param(ParamSpec::float(keys::GUIDANCE, "Guidance", 1.0, 20.0, 0.1).advanced())
        .param(ParamSpec::seed())
        .negative()
        .max_outputs(4)
        .pricing(Pricing::Variable {
            note: "Leonardo API credits".into(),
        })
}
fn video(id: &str, name: &str) -> ModelSpec {
    ModelSpec::new("leonardo", id, name, MediaKind::Video, &[Task::TextToVideo])
        .param(
            ParamSpec::choices(
                keys::RESOLUTION,
                "Resolution",
                &["RESOLUTION_480", "RESOLUTION_720", "RESOLUTION_1080"],
            )
            .default_value("RESOLUTION_720"),
        )
        .param(ParamSpec::seed())
        .negative()
        .pricing(Pricing::Variable {
            note: "Leonardo API credits".into(),
        })
}
fn body(model: &ModelSpec, req: &GenerationRequest) -> ProviderResult<Value> {
    // shortcut: media inputs need Leonardo's presigned upload flow, add it with image-to-image and image-to-video support.
    if !req.inputs.is_empty() {
        return Err(ProviderError::NotSupported(
            "Leonardo media inputs require its separate upload workflow".into(),
        ));
    }
    if model.output == MediaKind::Image {
        // The v1 model guides list `contrast` (3, 3.5 or 4) as required; 3.5 is the web app's Medium.
        let mut b = json!({"modelId":model.remote_id(),"prompt":req.prompt,"num_images":req.num_outputs,"public":false,"contrast":3.5});
        req.copy_params(
            b.as_object_mut().unwrap(),
            &[
                (keys::WIDTH, "width"),
                (keys::HEIGHT, "height"),
                (keys::STEPS, "num_inference_steps"),
                (keys::GUIDANCE, "guidance_scale"),
                (keys::SEED, "seed"),
            ],
        );
        if let Some(v) = &req.negative_prompt {
            b["negative_prompt"] = json!(v);
        }
        Ok(b)
    } else {
        let mut b = json!({"model":model.remote_id(),"prompt":req.prompt,"isPublic":false});
        req.copy_params(
            b.as_object_mut().unwrap(),
            &[
                (keys::DURATION, "duration"),
                (keys::RESOLUTION, "resolution"),
                (keys::SEED, "seed"),
            ],
        );
        if let Some(v) = &req.negative_prompt {
            b["negativePrompt"] = json!(v);
        }
        Ok(b)
    }
}
/// Leonardo refuses flagged prompts with 403 ("content moderation filter: ...", "inappropriate
/// content"), which would otherwise read as a rejected key.
fn moderation(e: ProviderError) -> ProviderError {
    match e {
        ProviderError::Auth(m)
            if m.contains("content moderation") || m.contains("inappropriate content") =>
        {
            ProviderError::ContentPolicy(m)
        }
        e => e,
    }
}

fn parse_generation(v: &Value, kind: MediaKind) -> ProviderResult<PollStatus> {
    let g = v
        .get("generations_by_pk")
        .ok_or_else(|| ProviderError::Failed("Leonardo returned no generation".into()))?;
    match g.get("status").and_then(Value::as_str).unwrap_or("") {
        "PENDING" => Ok(PollStatus::Queued { position: None }),
        "IN_PROGRESS" | "PROCESSING" => Ok(PollStatus::Running {
            progress: None,
            message: None,
        }),
        "COMPLETE" | "COMPLETED" => {
            let mut out = Vec::new();
            for item in g
                .get("generated_images")
                .and_then(Value::as_array)
                .into_iter()
                .flatten()
            {
                let url = if kind == MediaKind::Video {
                    item.get("motionMP4URL")
                } else {
                    item.get("url")
                };
                if let Some(url) = url.and_then(Value::as_str) {
                    out.push(Output::url(url));
                }
            }
            if out.is_empty() {
                return Err(ProviderError::Failed(
                    "Leonardo completed without media".into(),
                ));
            }
            Ok(PollStatus::Done(out))
        }
        "FAILED" | "CANCELLED" => Ok(PollStatus::Failed(failure(
            g.get("error")
                .and_then(Value::as_str)
                .unwrap_or("Leonardo generation failed"),
        ))),
        state => Err(ProviderError::Failed(format!(
            "unknown Leonardo status: {state}"
        ))),
    }
}

#[async_trait]
impl Provider for Leonardo {
    fn info(&self) -> ProviderInfo {
        ProviderInfo {
            id: "leonardo".into(),
            name: "Leonardo AI".into(),
            kind: ProviderKind::Cloud,
            fields: vec![CredentialField::api_key(&["LEONARDO_API_KEY"])],
            signup_url: Some("https://app.leonardo.ai/api-access".into()),
            docs_url: Some("https://docs.leonardo.ai/v1.0/reference/creategeneration".into()),
            max_concurrency: 4,
        }
    }
    fn models(&self) -> Vec<ModelSpec> {
        vec![
            image("7b592283-e8a7-4c5a-9ba6-d18c31f258b9", "Lucid Origin"),
            image("05ce0082-2d80-4a2d-8653-4d1c85e2418e", "Lucid Realism"),
            image("de7d3faf-762f-48e0-b3b7-9d0ac3a3fcf3", "Phoenix 1.0"),
            image("b2614463-296c-462a-9586-aafdb8f00e36", "FLUX.1 Dev"),
            video("MOTION2", "Motion 2.0"),
        ]
    }
    async fn check(&self, ctx: &Ctx) -> ProviderResult<()> {
        get_json(
            ctx.http
                .get("https://cloud.leonardo.ai/api/rest/v2/models")
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
        let route = if model.output == MediaKind::Image {
            "generations"
        } else {
            "generations-text-to-video"
        };
        let v = send_json(
            ctx.http
                .post(format!("{API}/{route}"))
                .bearer_auth(ctx.api_key()?),
            &body(model, req)?,
        )
        .await
        .map_err(moderation)?;
        let id = v
            .pointer("/sdGenerationJob/generationId")
            .or_else(|| v.pointer("/motionVideoGenerationJob/generationId"))
            .or_else(|| v.pointer("/generationId"))
            .and_then(Value::as_str)
            .ok_or_else(|| ProviderError::Failed("Leonardo returned no generation ID".into()))?;
        Ok(Submission::Pending(RemoteHandle::with(
            id,
            json!({"kind":model.output}),
        )))
    }
    async fn poll(&self, ctx: &Ctx, handle: &RemoteHandle) -> ProviderResult<PollStatus> {
        let v = get_json(
            ctx.http
                .get(format!("{API}/generations/{}", handle.id))
                .bearer_auth(ctx.api_key()?),
        )
        .await?;
        let kind = handle
            .data
            .get("kind")
            .and_then(|v| serde_json::from_value(v.clone()).ok())
            .unwrap_or(MediaKind::Image);
        parse_generation(&v, kind)
    }
    fn poll_interval(&self, _attempt: u32) -> Duration {
        Duration::from_secs(5)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn parses_image_and_motion_results() {
        let v = json!({"generations_by_pk":{"status":"COMPLETE","generated_images":[{"url":"https://cdn.test/a.png","motionMP4URL":"https://cdn.test/a.mp4"}]}});
        assert!(matches!(
            parse_generation(&v, MediaKind::Image).unwrap(),
            PollStatus::Done(_)
        ));
        assert!(matches!(
            parse_generation(&v, MediaKind::Video).unwrap(),
            PollStatus::Done(_)
        ));
    }
    #[test]
    fn maps_image_generation_fields() {
        let req:GenerationRequest=serde_json::from_value(json!({"job_id":"x","model":"7b592283-e8a7-4c5a-9ba6-d18c31f258b9","prompt":"cat","negative_prompt":"blur","params":{"width":768,"height":1024,"steps":30},"num_outputs":2})).unwrap();
        let b = body(
            &image("7b592283-e8a7-4c5a-9ba6-d18c31f258b9", "Lucid Origin"),
            &req,
        )
        .unwrap();
        assert_eq!(b["modelId"], "7b592283-e8a7-4c5a-9ba6-d18c31f258b9");
        assert_eq!(b["num_inference_steps"], 30);
        assert_eq!(b["negative_prompt"], "blur");
        assert_eq!(b["num_images"], 2);
    }
}
