use std::{
    collections::HashMap,
    sync::{Mutex, OnceLock},
    time::Duration,
};

use async_trait::async_trait;
use base64::Engine;
use pictesa_core::{
    common, keys, CredentialField, Ctx, GenerationRequest, InputRole, InputSlot, MediaKind,
    ModelSpec, Output, ParamSpec, PollStatus, Pricing, Provider, ProviderError, ProviderInfo,
    ProviderKind, ProviderResult, RemoteHandle, Submission, Task,
};
use serde_json::{json, Value};
use tokio::task::JoinHandle;

use crate::util::{failure, get_json, ratio_to_size, send_json};

pub struct A1111;
const DEFAULT: &str = "http://127.0.0.1:7860";
type Jobs = HashMap<String, JoinHandle<ProviderResult<Value>>>;
// shortcut: WebUI returns media only on its synchronous response, persist results separately if restart recovery is required.
static JOBS: OnceLock<Mutex<Jobs>> = OnceLock::new();
fn jobs() -> &'static Mutex<Jobs> {
    JOBS.get_or_init(|| Mutex::new(HashMap::new()))
}

fn spec(id: &str) -> ModelSpec {
    ModelSpec::new(
        "a1111",
        id,
        id,
        MediaKind::Image,
        &[Task::TextToImage, Task::ImageEdit],
    )
    .input(InputSlot::new(InputRole::Reference, MediaKind::Image, 1))
    .input(InputSlot::new(InputRole::Mask, MediaKind::Image, 1))
    .params([
        ParamSpec::int(keys::WIDTH, "Width", 64, 4096).default_value(512),
        ParamSpec::int(keys::HEIGHT, "Height", 64, 4096).default_value(512),
        common::steps(1, 150, 20),
        common::guidance(1.0, 30.0, 7.0),
        ParamSpec::float(keys::STRENGTH, "Denoising strength", 0.0, 1.0, 0.05).default_value(0.75),
        ParamSpec::seed(),
    ])
    .negative()
    .max_outputs(8)
    .pricing(Pricing::Local)
    .open_weights()
}

fn discover_models(v: &Value) -> Vec<ModelSpec> {
    v.as_array()
        .into_iter()
        .flatten()
        .filter_map(|m| m.get("title").and_then(Value::as_str))
        .map(spec)
        .collect()
}

async fn body(req: &GenerationRequest, model: &ModelSpec) -> ProviderResult<Value> {
    let mut b = json!({
        "prompt":req.prompt, "negative_prompt":req.negative_prompt.as_deref().unwrap_or(""),
        "batch_size":req.num_outputs, "n_iter":1,
        "override_settings":{"sd_model_checkpoint":model.remote_id()},
        "override_settings_restore_afterwards":true,
    });
    req.copy_params(
        b.as_object_mut().unwrap(),
        &[
            (keys::WIDTH, "width"),
            (keys::HEIGHT, "height"),
            (keys::STEPS, "steps"),
            (keys::GUIDANCE, "cfg_scale"),
            (keys::STRENGTH, "denoising_strength"),
            (keys::SEED, "seed"),
        ],
    );
    if b.get("width").is_none() || b.get("height").is_none() {
        if let Some(ratio) = req.str(keys::ASPECT_RATIO) {
            if let Some((w, h)) = ratio_to_size(ratio, 1024) {
                b["width"] = json!(w);
                b["height"] = json!(h);
            }
        }
    }
    if let Some(source) = req.input(InputRole::Reference) {
        b["init_images"] = json!([source.base64().await?]);
        if let Some(mask) = req.input(InputRole::Mask) {
            b["mask"] = json!(mask.base64().await?);
        }
    } else if req.input(InputRole::Mask).is_some() {
        return Err(ProviderError::InvalidInput(
            "mask requires an initial image".into(),
        ));
    }
    Ok(b)
}

fn outputs(v: &Value) -> ProviderResult<Vec<Output>> {
    let mut out = Vec::new();
    for image in v
        .get("images")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
    {
        let encoded = image
            .as_str()
            .ok_or_else(|| ProviderError::Failed("invalid image output".into()))?;
        let (mime, data) = encoded
            .strip_prefix("data:")
            .and_then(|s| s.split_once(";base64,"))
            .unwrap_or(("image/png", encoded));
        out.push(Output::Bytes {
            data: base64::engine::general_purpose::STANDARD
                .decode(data)
                .map_err(|e| ProviderError::Failed(format!("invalid image base64: {e}")))?,
            mime: mime.into(),
        });
    }
    if out.is_empty() {
        Err(ProviderError::Failed("WebUI returned no image".into()))
    } else {
        Ok(out)
    }
}

#[async_trait]
impl Provider for A1111 {
    fn info(&self) -> ProviderInfo {
        ProviderInfo {
            id: "a1111".into(),
            name: "Stable Diffusion WebUI".into(),
            kind: ProviderKind::Local,
            fields: vec![CredentialField::base_url(DEFAULT, &["A1111_BASE_URL"])],
            signup_url: None,
            docs_url: Some(
                "https://github.com/AUTOMATIC1111/stable-diffusion-webui/wiki/API".into(),
            ),
            max_concurrency: 1,
        }
    }
    fn models(&self) -> Vec<ModelSpec> {
        Vec::new()
    }
    async fn discover(&self, ctx: &Ctx) -> ProviderResult<Vec<ModelSpec>> {
        Ok(discover_models(
            &get_json(
                ctx.http
                    .get(format!("{}/sdapi/v1/sd-models", ctx.base_url(DEFAULT))),
            )
            .await?,
        ))
    }
    async fn check(&self, ctx: &Ctx) -> ProviderResult<()> {
        get_json(
            ctx.http
                .get(format!("{}/sdapi/v1/sd-models", ctx.base_url(DEFAULT))),
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
        let b = body(req, model).await?;
        let route = if req.input(InputRole::Reference).is_some() {
            "img2img"
        } else {
            "txt2img"
        };
        let url = format!("{}/sdapi/v1/{route}", ctx.base_url(DEFAULT));
        let client = ctx.http.clone();
        let id = ulid::Ulid::new().to_string();
        let task = tokio::spawn(async move { send_json(client.post(url), &b).await });
        jobs()
            .lock()
            .map_err(|_| ProviderError::Failed("WebUI jobs lock poisoned".into()))?
            .insert(id.clone(), task);
        Ok(Submission::Pending(RemoteHandle::new(id)))
    }
    async fn poll(&self, ctx: &Ctx, handle: &RemoteHandle) -> ProviderResult<PollStatus> {
        let finished = {
            let mut map = jobs()
                .lock()
                .map_err(|_| ProviderError::Failed("WebUI jobs lock poisoned".into()))?;
            if map.get(&handle.id).is_some_and(JoinHandle::is_finished) {
                map.remove(&handle.id)
            } else {
                None
            }
        };
        if let Some(task) = finished {
            return Ok(
                match task
                    .await
                    .map_err(|e| ProviderError::Transient(format!("WebUI task ended: {e}")))?
                {
                    Ok(v) => PollStatus::Done(outputs(&v)?),
                    Err(e) => PollStatus::Failed(e),
                },
            );
        }
        if !jobs()
            .lock()
            .map_err(|_| ProviderError::Failed("WebUI jobs lock poisoned".into()))?
            .contains_key(&handle.id)
        {
            return Ok(PollStatus::Failed(failure(
                "WebUI request was interrupted; its synchronous result is unavailable",
            )));
        }
        let v = get_json(
            ctx.http
                .get(format!("{}/sdapi/v1/progress", ctx.base_url(DEFAULT))),
        )
        .await?;
        let progress = v
            .get("progress")
            .and_then(Value::as_f64)
            .map(|p| p.clamp(0.0, 1.0) as f32);
        Ok(PollStatus::Running {
            progress,
            message: v
                .pointer("/state/sampling_step")
                .and_then(Value::as_u64)
                .map(|n| format!("Step {n}")),
        })
    }
    fn poll_interval(&self, _attempt: u32) -> Duration {
        Duration::from_secs(2)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use pictesa_core::MediaInput;
    #[tokio::test]
    async fn maps_checkpoint_and_img2img() {
        let mut req: GenerationRequest = serde_json::from_value(json!({"job_id":"a","model":"checkpoint","prompt":"cat","inputs":[],"params":{"steps":30},"num_outputs":2})).unwrap();
        let path = std::env::temp_dir().join(format!("pictesa-a1111-{}", ulid::Ulid::new()));
        tokio::fs::write(&path, b"a").await.unwrap();
        req.inputs.push(MediaInput {
            role: InputRole::Reference,
            kind: MediaKind::Image,
            mime: "image/png".into(),
            path: path.clone(),
            url: None,
            width: None,
            height: None,
        });
        req.inputs.push(MediaInput {
            role: InputRole::Mask,
            kind: MediaKind::Image,
            mime: "image/png".into(),
            path: path.clone(),
            url: None,
            width: None,
            height: None,
        });
        let b = body(&req, &spec("checkpoint [hash]")).await.unwrap();
        tokio::fs::remove_file(path).await.unwrap();
        assert_eq!(
            b["override_settings"]["sd_model_checkpoint"],
            "checkpoint [hash]"
        );
        assert_eq!(b["steps"], 30);
        assert_eq!(b["batch_size"], 2);
        assert_eq!(b["init_images"][0], "YQ==");
        assert_eq!(b["mask"], "YQ==");
    }
    #[test]
    fn parses_webui_images() {
        let out = outputs(&json!({"images":["data:image/png;base64,YQ=="]})).unwrap();
        assert!(matches!(&out[0],Output::Bytes{data,..} if data==b"a"));
    }
}
