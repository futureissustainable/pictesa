use std::time::Duration;

use async_trait::async_trait;
use pictesa_core::{
    common, keys, CredentialField, Ctx, GenerationRequest, InputRole, InputSlot, MediaKind,
    ModelSpec, Output, ParamSpec, PollStatus, Pricing, Provider, ProviderError, ProviderInfo,
    ProviderKind, ProviderResult, RemoteHandle, Submission, Task,
};
use serde_json::{json, Value};

use crate::util::{failure, get_json, send_json};

pub struct Freepik;
const API: &str = "https://api.magnific.com/v1/ai";

fn spec(id: &str, name: &str, kind: MediaKind, task: Task) -> ModelSpec {
    let mut m = ModelSpec::new("freepik", id, name, kind, &[task]).pricing(Pricing::Variable {
        note: "Magnific API credits".into(),
    });
    if kind == MediaKind::Video {
        m = m.param(ParamSpec::seed());
    }
    m
}
fn models() -> Vec<ModelSpec> {
    vec![
        spec("mystic", "Mystic", MediaKind::Image, Task::TextToImage)
            .param(common::aspect_ratio(
                &["1:1", "4:3", "3:4", "16:9", "9:16", "3:2", "2:3", "4:5"],
                "1:1",
            ))
            .param(common::resolution(&["1k", "2k", "4k"], "2k")),
        spec(
            "text-to-video/runway-4-5",
            "Runway Gen 4.5",
            MediaKind::Video,
            Task::TextToVideo,
        )
        .param(common::aspect_ratio(
            &["16:9", "9:16", "4:3", "1:1", "3:4"],
            "16:9",
        ))
        .param(common::duration(&[5, 8, 10], 5)),
        spec(
            "image-to-video/veo-3-1-fast",
            "Veo 3.1 Fast",
            MediaKind::Video,
            Task::ImageToVideo,
        )
        .input(InputSlot::new(InputRole::StartFrame, MediaKind::Image, 1).required())
        .param(common::aspect_ratio(&["16:9", "9:16"], "16:9"))
        .param(common::duration(&[4, 6, 8], 8))
        .param(common::resolution(&["720p", "1080p", "4k"], "720p"))
        .param(common::generate_audio(true)),
        spec(
            "image-to-video/ltx-2-pro",
            "LTX 2.0 Pro",
            MediaKind::Video,
            Task::ImageToVideo,
        )
        .input(InputSlot::new(InputRole::StartFrame, MediaKind::Image, 1).required())
        .param(common::duration(&[6, 8, 10], 6))
        .param(common::resolution(&["1080p", "1440p", "2160p"], "1080p"))
        .param(ParamSpec::choices(keys::FPS, "FPS", &["25", "50"]).default_value("25"))
        .param(common::generate_audio(false)),
    ]
}
fn mystic_ratio(r: &str) -> Option<&'static str> {
    Some(match r {
        "1:1" => "square_1_1",
        "4:3" => "classic_4_3",
        "3:4" => "traditional_3_4",
        "16:9" => "widescreen_16_9",
        "9:16" => "social_story_9_16",
        "3:2" => "standard_3_2",
        "2:3" => "portrait_2_3",
        "4:5" => "social_post_4_5",
        _ => return None,
    })
}
fn runway_ratio(r: &str) -> Option<&'static str> {
    Some(match r {
        "16:9" => "1280:720",
        "9:16" => "720:1280",
        "4:3" => "1104:832",
        "1:1" => "960:960",
        "3:4" => "832:1104",
        _ => return None,
    })
}
async fn body(model: &ModelSpec, req: &GenerationRequest) -> ProviderResult<Value> {
    let id = model.remote_id();
    let mut b = json!({"prompt":req.prompt});
    if let Some(ratio) = req.str(keys::ASPECT_RATIO) {
        b[if id.contains("runway") {
            "ratio"
        } else {
            "aspect_ratio"
        }] = json!(if id == "mystic" {
            mystic_ratio(ratio)
        } else if id.contains("runway") {
            runway_ratio(ratio)
        } else {
            Some(ratio)
        }
        .ok_or_else(|| ProviderError::InvalidInput("unsupported aspect ratio".into()))?);
    }
    req.copy_params(
        b.as_object_mut().unwrap(),
        &[
            (keys::RESOLUTION, "resolution"),
            (keys::DURATION, "duration"),
            (keys::GENERATE_AUDIO, "generate_audio"),
            (keys::SEED, "seed"),
        ],
    );
    if let Some(v) = &req.negative_prompt {
        b["negative_prompt"] = json!(v);
    }
    if id.starts_with("image-to-video/") {
        let source = req.input(InputRole::StartFrame).ok_or_else(|| {
            ProviderError::InvalidInput("video model requires a start frame".into())
        })?;
        if id.ends_with("ltx-2-pro") {
            let url = source
                .url
                .as_deref()
                .filter(|u| u.starts_with("https://"))
                .ok_or_else(|| {
                    ProviderError::InvalidInput("LTX requires a public HTTPS image URL".into())
                })?;
            b["image_url"] = json!(url);
            if let Some(fps) = req.str(keys::FPS) {
                b["fps"] = json!(fps
                    .parse::<u32>()
                    .map_err(|_| ProviderError::InvalidInput("invalid FPS".into()))?);
            }
        } else {
            b["image"] = json!(source.url_or_data_uri().await?);
        }
    } else if !req.inputs.is_empty() {
        return Err(ProviderError::NotSupported(
            "this Magnific workflow does not accept input files".into(),
        ));
    }
    Ok(b)
}
fn parse_task(v: &Value) -> ProviderResult<PollStatus> {
    let data = v
        .get("data")
        .ok_or_else(|| ProviderError::Failed("Magnific response has no data".into()))?;
    match data.get("status").and_then(Value::as_str).unwrap_or("") {
        "CREATED" => Ok(PollStatus::Queued { position: None }),
        "IN_PROGRESS" => Ok(PollStatus::Running {
            progress: None,
            message: None,
        }),
        "COMPLETED" => {
            let out: Vec<_> = data
                .get("generated")
                .and_then(Value::as_array)
                .into_iter()
                .flatten()
                .filter_map(Value::as_str)
                .map(Output::url)
                .collect();
            if out.is_empty() {
                return Err(ProviderError::Failed(
                    "Magnific completed without media".into(),
                ));
            }
            Ok(PollStatus::Done(out))
        }
        "FAILED" => Ok(PollStatus::Failed(failure(
            data.get("error")
                .and_then(Value::as_str)
                .unwrap_or("Magnific task failed"),
        ))),
        state => Err(ProviderError::Failed(format!(
            "unknown Magnific status: {state}"
        ))),
    }
}

#[async_trait]
impl Provider for Freepik {
    fn info(&self) -> ProviderInfo {
        ProviderInfo {
            id: "freepik".into(),
            name: "Magnific / Freepik".into(),
            kind: ProviderKind::Cloud,
            fields: vec![CredentialField::api_key(&[
                "MAGNIFIC_API_KEY",
                "FREEPIK_API_KEY",
            ])],
            signup_url: Some("https://www.magnific.com/user/organization/api-keys".into()),
            docs_url: Some("https://docs.magnific.com/llms.txt".into()),
            max_concurrency: 3,
        }
    }
    fn models(&self) -> Vec<ModelSpec> {
        models()
    }
    async fn check(&self, ctx: &Ctx) -> ProviderResult<()> {
        get_json(
            ctx.http
                .get(format!("{API}/mystic"))
                .header("x-magnific-api-key", ctx.api_key()?),
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
        let v = send_json(
            ctx.http
                .post(format!("{API}/{}", model.remote_id()))
                .header("x-magnific-api-key", ctx.api_key()?),
            &body(model, req).await?,
        )
        .await?;
        let id = v
            .pointer("/data/task_id")
            .and_then(Value::as_str)
            .ok_or_else(|| ProviderError::Failed("Magnific returned no task ID".into()))?;
        Ok(Submission::Pending(RemoteHandle::with(
            id,
            json!({"route":model.remote_id()}),
        )))
    }
    async fn poll(&self, ctx: &Ctx, handle: &RemoteHandle) -> ProviderResult<PollStatus> {
        let route = handle
            .data_str("route")
            .ok_or_else(|| ProviderError::Failed("Magnific handle has no route".into()))?;
        let v = get_json(
            ctx.http
                .get(format!("{API}/{route}/{}", handle.id))
                .header("x-magnific-api-key", ctx.api_key()?),
        )
        .await?;
        parse_task(&v)
    }
    fn poll_interval(&self, _attempt: u32) -> Duration {
        Duration::from_secs(5)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn parses_mystic_ratio_and_task() {
        assert_eq!(mystic_ratio("9:16"), Some("social_story_9_16"));
        let v = json!({"data":{"task_id":"x","status":"COMPLETED","generated":["https://cdn.test/a.png"]}});
        assert!(matches!(parse_task(&v).unwrap(), PollStatus::Done(_)));
    }
    #[tokio::test]
    async fn maps_mystic_and_runway_ratios() {
        let req: GenerationRequest = serde_json::from_value(json!({"job_id":"x","model":"mystic","prompt":"cat","params":{"aspect_ratio":"9:16","duration":8},"num_outputs":1})).unwrap();
        let catalog = models();
        let mystic = body(&catalog[0], &req).await.unwrap();
        let runway = body(&catalog[1], &req).await.unwrap();
        assert_eq!(mystic["aspect_ratio"], "social_story_9_16");
        assert_eq!(runway["ratio"], "720:1280");
        assert_eq!(runway["duration"], 8);
    }
}
