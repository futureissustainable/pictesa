use async_trait::async_trait;
use pictesa_core::{
    common, keys, CredentialField, Ctx, GenerationRequest, InputRole, InputSlot, MediaInput,
    MediaKind, ModelSpec, Output, ParamSpec, PollStatus, Pricing, Provider, ProviderError,
    ProviderInfo, ProviderKind, ProviderResult, RemoteHandle, Submission, Task,
};
use serde_json::{json, Value};

use crate::util::{get_json, net_error};

const BASE: &str = "https://api.stability.ai/v2beta/stable-image";
pub struct Stability;

async fn part(input: &MediaInput) -> ProviderResult<reqwest::multipart::Part> {
    reqwest::multipart::Part::bytes(input.bytes().await?)
        .file_name(input.file_name())
        .mime_str(&input.mime)
        .map_err(|_| ProviderError::InvalidInput("invalid input MIME type".into()))
}

async fn form(
    model: &ModelSpec,
    req: &GenerationRequest,
) -> ProviderResult<reqwest::multipart::Form> {
    let id = model.remote_id();
    if [
        "fast",
        "creative",
        "conservative",
        "inpaint",
        "search-and-replace",
    ]
    .contains(&id)
        && req.input(InputRole::Reference).is_none()
    {
        return Err(ProviderError::InvalidInput(
            "source image is required".into(),
        ));
    }
    if id == "inpaint" && req.input(InputRole::Mask).is_none() {
        return Err(ProviderError::InvalidInput(
            "inpaint mask is required".into(),
        ));
    }
    let mut f = reqwest::multipart::Form::new();
    if model.prompt {
        f = f.text("prompt", req.prompt.clone());
    }
    if model.remote_id() != "sd3.5-flash" {
        if let Some(v) = &req.negative_prompt {
            f = f.text("negative_prompt", v.clone());
        }
    }
    let sd3_image = id.starts_with("sd3.5-") && req.input(InputRole::Reference).is_some();
    if let Some(v) = req.str(keys::ASPECT_RATIO).filter(|_| !sd3_image) {
        f = f.text("aspect_ratio", v.to_string());
    }
    if sd3_image {
        f = f.text("mode", "image-to-image");
    }
    if let Some(v) = req.str(keys::OUTPUT_FORMAT) {
        f = f.text("output_format", v.to_string());
    }
    if let Some(v) = req.u64(keys::SEED) {
        f = f.text("seed", v.to_string());
    }
    if let Some(v) = req.f64(keys::GUIDANCE) {
        f = f.text("cfg_scale", v.to_string());
    }
    // Ultra and SD 3.5 require `strength` whenever an image is sent.
    if (id.starts_with("sd3.5-") || id == "ultra") && req.input(InputRole::Reference).is_some() {
        f = f.text(
            "strength",
            req.f64(keys::STRENGTH).unwrap_or(0.5).to_string(),
        );
    }
    if model.remote_id().starts_with("sd3.5-") {
        f = f.text("model", model.remote_id().to_string());
    }
    if let Some(input) = req.input(InputRole::Reference) {
        f = f.part("image", part(input).await?);
    }
    if let Some(input) = req.input(InputRole::Mask) {
        f = f.part("mask", part(input).await?);
    }
    Ok(f)
}

fn endpoint(id: &str) -> &'static str {
    match id {
        "ultra" => "generate/ultra",
        "core" => "generate/core",
        "fast" => "upscale/fast",
        "creative" => "upscale/creative",
        "conservative" => "upscale/conservative",
        "inpaint" => "edit/inpaint",
        "search-and-replace" => "edit/search-and-replace",
        _ => "generate/sd3",
    }
}

fn upscale_id(v: &Value) -> ProviderResult<&str> {
    v.get("id")
        .and_then(Value::as_str)
        .ok_or_else(|| ProviderError::Failed("Stability upscale returned no id".into()))
}

/// Passes a successful answer through. Stability names moderation refusals in the error body
/// (`content_moderation` on 403, `public_figure` on 422), which the status alone does not.
async fn checked(resp: reqwest::Response) -> ProviderResult<reqwest::Response> {
    let status = resp.status();
    if status.is_success() {
        return Ok(resp);
    }
    let message = resp.text().await.map_err(net_error)?;
    let name = serde_json::from_str::<Value>(&message).ok().and_then(|v| v.get("name")?.as_str().map(str::to_string));
    Err(match name.as_deref() {
        Some("content_moderation" | "public_figure") => ProviderError::ContentPolicy(pictesa_core::error::truncate(&message, 600)),
        _ => ProviderError::from_http(status.as_u16(), &message, None),
    })
}

async fn read_image(resp: reqwest::Response, format: &str) -> ProviderResult<Output> {
    let resp = checked(resp).await?;
    // A blurred image: "the output violated the content moderation policy".
    if resp.headers().get("finish-reason").is_some_and(|v| v == "CONTENT_FILTERED") {
        return Err(ProviderError::ContentPolicy("Stability blurred the result: it violated the content moderation policy".into()));
    }
    let mime = resp
        .headers()
        .get(reqwest::header::CONTENT_TYPE)
        .and_then(|h| h.to_str().ok())
        .filter(|s| s.starts_with("image/"))
        .map(str::to_string)
        .unwrap_or_else(|| format!("image/{format}"));
    let data = resp.bytes().await.map_err(net_error)?.to_vec();
    if data.is_empty() {
        return Err(ProviderError::Failed(
            "Stability returned an empty image".into(),
        ));
    }
    Ok(Output::Bytes { data, mime })
}

#[async_trait]
impl Provider for Stability {
    fn info(&self) -> ProviderInfo {
        ProviderInfo {
            id: "stability".into(),
            name: "Stability AI".into(),
            kind: ProviderKind::Cloud,
            fields: vec![CredentialField::api_key(&["STABILITY_API_KEY"])],
            signup_url: Some("https://platform.stability.ai/".into()),
            docs_url: Some("https://platform.stability.ai/docs/api-reference".into()),
            max_concurrency: 4,
        }
    }
    fn models(&self) -> Vec<ModelSpec> {
        let mut v = Vec::new();
        for (id, name, price) in [
            ("ultra", "Stable Image Ultra", 0.08),
            ("core", "Stable Image Core", 0.03),
            ("sd3.5-large", "SD 3.5 Large", 0.065),
            ("sd3.5-large-turbo", "SD 3.5 Large Turbo", 0.04),
            ("sd3.5-medium", "SD 3.5 Medium", 0.035),
            ("sd3.5-flash", "SD 3.5 Flash", 0.025),
        ] {
            let mut m = ModelSpec::new(
                "stability",
                id,
                name,
                MediaKind::Image,
                if id == "core" {
                    &[Task::TextToImage]
                } else {
                    &[Task::TextToImage, Task::ImageEdit]
                },
            )
            .param(common::aspect_ratio(
                &["1:1", "16:9", "9:16", "3:2", "2:3", "4:5", "5:4", "21:9"],
                "1:1",
            ))
            .param(common::output_format("png"))
            .param(ParamSpec::seed())
            .pricing(Pricing::PerImage { usd: price });
            if id != "core" {
                m = m.input(InputSlot::new(InputRole::Reference, MediaKind::Image, 1));
            }
            if id != "sd3.5-flash" {
                m = m.negative();
            }
            if id.starts_with("sd3.5-") {
                m = m.param(
                    ParamSpec::float(keys::STRENGTH, "Image strength", 0.0, 1.0, 0.01)
                        .default_value(0.5)
                        .advanced(),
                );
            }
            v.push(m);
        }
        v.push(
            ModelSpec::new(
                "stability",
                "fast",
                "Fast Upscale",
                MediaKind::Image,
                &[Task::Upscale],
            )
            .input(InputSlot::new(InputRole::Reference, MediaKind::Image, 1).required())
            .no_prompt()
            .param(common::output_format("png"))
            .pricing(Pricing::PerImage { usd: 0.02 }),
        );
        v.push(
            ModelSpec::new(
                "stability",
                "creative",
                "Creative Upscale",
                MediaKind::Image,
                &[Task::Upscale],
            )
            .input(InputSlot::new(InputRole::Reference, MediaKind::Image, 1).required())
            .param(common::output_format("png"))
            .pricing(Pricing::PerImage { usd: 0.60 }),
        );
        v.push(
            ModelSpec::new(
                "stability",
                "conservative",
                "Conservative Upscale",
                MediaKind::Image,
                &[Task::Upscale],
            )
            .input(InputSlot::new(InputRole::Reference, MediaKind::Image, 1).required())
            .param(common::output_format("png"))
            .pricing(Pricing::PerImage { usd: 0.40 }),
        );
        v.push(
            ModelSpec::new(
                "stability",
                "inpaint",
                "Inpaint",
                MediaKind::Image,
                &[Task::ImageEdit],
            )
            .input(InputSlot::new(InputRole::Reference, MediaKind::Image, 1).required())
            .input(InputSlot::new(InputRole::Mask, MediaKind::Image, 1).required())
            .param(common::output_format("png")),
        );
        v.push(
            ModelSpec::new(
                "stability",
                "search-and-replace",
                "Search and Replace",
                MediaKind::Image,
                &[Task::ImageEdit],
            )
            .input(InputSlot::new(InputRole::Reference, MediaKind::Image, 1).required())
            .param(ParamSpec::text("search_prompt", "Find"))
            .param(common::output_format("png")),
        );
        v
    }
    async fn check(&self, ctx: &Ctx) -> ProviderResult<()> {
        get_json(
            ctx.http
                .get("https://api.stability.ai/v1/user/balance")
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
        let id = model.remote_id();
        let mut f = form(model, req).await?;
        if id == "search-and-replace" {
            let search = req
                .str("search_prompt")
                .ok_or_else(|| ProviderError::InvalidInput("search prompt is required".into()))?;
            f = f.text("search_prompt", search.to_string());
        }
        let format = req.str(keys::OUTPUT_FORMAT).unwrap_or("png");
        let resp = ctx
            .http
            .post(format!("{BASE}/{}", endpoint(id)))
            .bearer_auth(ctx.api_key()?)
            .header(
                reqwest::header::ACCEPT,
                if id == "creative" { "application/json" } else { "image/*" },
            )
            .multipart(f)
            .send()
            .await
            .map_err(net_error)?;
        // Creative upscale is the only asynchronous endpoint here.
        if id == "creative" {
            let v = pictesa_core::json_response(checked(resp).await?).await?;
            let task = upscale_id(&v)?;
            return Ok(Submission::Pending(RemoteHandle::with(
                task,
                json!({"format":format}),
            )));
        }
        Ok(Submission::Done(vec![read_image(resp, format).await?]))
    }
    async fn poll(&self, ctx: &Ctx, handle: &RemoteHandle) -> ProviderResult<PollStatus> {
        let format = handle.data_str("format").unwrap_or("png");
        let resp = ctx
            .http
            .get(format!("https://api.stability.ai/v2beta/results/{}", handle.id))
            .bearer_auth(ctx.api_key()?)
            .header(reqwest::header::ACCEPT, "image/*")
            .send()
            .await
            .map_err(net_error)?;
        if resp.status() == reqwest::StatusCode::ACCEPTED {
            return Ok(PollStatus::Running {
                progress: None,
                message: None,
            });
        }
        Ok(PollStatus::Done(vec![read_image(resp, format).await?]))
    }
    /// "Rate-limiting or other errors may occur if you poll more than once every 10 seconds."
    fn poll_interval(&self, _attempt: u32) -> std::time::Duration {
        std::time::Duration::from_secs(10)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[tokio::test]
    async fn async_response_and_required_input() {
        assert_eq!(
            upscale_id(&json!({"id":"stability-job"})).unwrap(),
            "stability-job"
        );
        let model = Stability
            .models()
            .into_iter()
            .find(|m| m.remote_id() == "inpaint")
            .unwrap();
        let req = GenerationRequest {
            job_id: "x".into(),
            model: "inpaint".into(),
            prompt: "change the sky".into(),
            negative_prompt: None,
            params: serde_json::Map::new(),
            inputs: vec![],
            num_outputs: 1,
        };
        assert!(matches!(
            form(&model, &req).await,
            Err(ProviderError::InvalidInput(_))
        ));
    }
}
