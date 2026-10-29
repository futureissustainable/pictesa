use async_trait::async_trait;
use pictesa_core::{
    common, keys, CredentialField, Ctx, GenerationRequest, InputRole, InputSlot, MediaInput,
    MediaKind, ModelSpec, Output, ParamSpec, PollStatus, Provider, ProviderError, ProviderInfo,
    ProviderKind, ProviderResult, RemoteHandle, Submission, Task,
};
use serde_json::{json, Value};

use crate::util::{get_json, net_error, send_json};

const BASE: &str = "https://api.ideogram.ai/v1/ideogram-v3";
pub struct Ideogram;

fn path(req: &GenerationRequest) -> &'static str {
    if req.input(InputRole::Mask).is_some() {
        "edit"
    } else if req.input(InputRole::StartFrame).is_some() {
        "remix"
    } else {
        "generate"
    }
}

async fn part(input: &MediaInput) -> ProviderResult<reqwest::multipart::Part> {
    reqwest::multipart::Part::bytes(input.bytes().await?)
        .file_name(input.file_name())
        .mime_str(&input.mime)
        .map_err(|_| ProviderError::InvalidInput("invalid image MIME type".into()))
}

async fn form(req: &GenerationRequest) -> ProviderResult<reqwest::multipart::Form> {
    if req.input(InputRole::Mask).is_some() && req.input(InputRole::StartFrame).is_none() {
        return Err(ProviderError::InvalidInput(
            "Ideogram edit requires a source image".into(),
        ));
    }
    if req.str("reference_type") != Some("style") && req.inputs(InputRole::Reference).count() > 1 {
        return Err(ProviderError::InvalidInput(
            "Ideogram accepts one character reference".into(),
        ));
    }
    let mut f = reqwest::multipart::Form::new()
        .text("prompt", req.prompt.clone())
        .text("num_images", req.num_outputs.to_string());
    if let Some(v) = req.str(keys::ASPECT_RATIO) {
        f = f.text("aspect_ratio", v.replace(':', "x"));
    }
    if let Some(v) = req.str(keys::QUALITY) {
        f = f.text("rendering_speed", v.to_string());
    }
    if let Some(v) = req.str(keys::STYLE) {
        f = f.text("style_type", v.to_string());
    }
    if let Some(v) = req.u64(keys::SEED) {
        f = f.text("seed", v.to_string());
    }
    if let Some(v) = &req.negative_prompt {
        f = f.text("negative_prompt", v.clone());
    }
    if let Some(v) = req.i64(keys::STRENGTH) {
        f = f.text("image_weight", v.to_string());
    }
    if let Some(input) = req.input(InputRole::StartFrame) {
        f = f.part("image", part(input).await?);
    }
    if let Some(input) = req.input(InputRole::Mask) {
        let mask = reqwest::multipart::Part::bytes(black_edits(&input.bytes().await?)?)
            .file_name("mask.png")
            .mime_str("image/png")
            .map_err(|_| ProviderError::InvalidInput("invalid mask".into()))?;
        f = f.part("mask", mask);
    }
    let field = if req.str("reference_type") == Some("style") {
        "style_reference_images"
    } else {
        "character_reference_images"
    };
    for input in req.inputs(InputRole::Reference) {
        f = f.part(field, part(input).await?);
    }
    Ok(f)
}

/// Ideogram edits the mask's black regions; Pictesa masks are white where to change.
fn black_edits(mask: &[u8]) -> ProviderResult<Vec<u8>> {
    let mut gray = image::load_from_memory(mask)
        .map_err(|e| ProviderError::InvalidInput(format!("the mask is not a readable image: {e}")))?
        .to_luma8();
    image::imageops::invert(&mut gray);
    let mut png = std::io::Cursor::new(Vec::new());
    gray.write_to(&mut png, image::ImageFormat::Png)
        .map_err(|e| ProviderError::InvalidInput(format!("could not prepare the mask: {e}")))?;
    Ok(png.into_inner())
}

fn outputs(v: &Value) -> ProviderResult<Vec<Output>> {
    let items = v
        .get("data")
        .and_then(Value::as_array)
        .ok_or_else(|| ProviderError::Failed("Ideogram returned no images".into()))?;
    if items.is_empty() {
        return Err(ProviderError::Failed(
            "Ideogram returned an empty image list".into(),
        ));
    }
    // Images that fail the safety check come back with an empty `url`.
    let out: Vec<Output> = items
        .iter()
        .filter_map(|v| v.get("url").and_then(Value::as_str).filter(|u| !u.is_empty()).map(Output::url))
        .collect();
    if !out.is_empty() {
        Ok(out)
    } else if items.iter().any(|v| v.get("is_image_safe") == Some(&Value::Bool(false))) {
        Err(ProviderError::ContentPolicy("Ideogram withheld the image: it failed the safety check".into()))
    } else {
        Err(ProviderError::Failed("Ideogram image has no URL".into()))
    }
}

#[async_trait]
impl Provider for Ideogram {
    fn info(&self) -> ProviderInfo {
        ProviderInfo {
            id: "ideogram".into(),
            name: "Ideogram".into(),
            kind: ProviderKind::Cloud,
            fields: vec![CredentialField::api_key(&["IDEOGRAM_API_KEY"])],
            signup_url: Some("https://ideogram.ai/platform".into()),
            docs_url: Some("https://developer.ideogram.ai/v1/api-reference".into()),
            max_concurrency: 4,
        }
    }
    fn models(&self) -> Vec<ModelSpec> {
        vec![ModelSpec::new(
            "ideogram",
            "ideogram-v3",
            "Ideogram 3.0",
            MediaKind::Image,
            &[Task::TextToImage, Task::ImageEdit],
        )
        // The image to remix or edit; the role is shared with video start frames.
        .input(InputSlot { label: Some("Image to edit".into()), ..InputSlot::new(InputRole::StartFrame, MediaKind::Image, 1) })
        .input(InputSlot::new(InputRole::Mask, MediaKind::Image, 1))
        .input(InputSlot::new(InputRole::Reference, MediaKind::Image, 10))
        .param(common::aspect_ratio(
            &["1:1", "16:9", "9:16", "4:3", "3:4", "3:2", "2:3"],
            "1:1",
        ))
        .param(
            ParamSpec::choices(
                keys::QUALITY,
                "Speed",
                &["FLASH", "TURBO", "DEFAULT", "QUALITY"],
            )
            .default_value("DEFAULT"),
        )
        .param(
            ParamSpec::choices(
                keys::STYLE,
                "Style",
                &["AUTO", "GENERAL", "REALISTIC", "DESIGN", "FICTION"],
            )
            .default_value("AUTO"),
        )
        .param(
            ParamSpec::choices("reference_type", "Reference type", &["character", "style"])
                .default_value("character"),
        )
        .param(ParamSpec::int(keys::STRENGTH, "Image weight", 1, 100).advanced())
        .param(ParamSpec::seed())
        .max_outputs(8)]
    }
    async fn check(&self, ctx: &Ctx) -> ProviderResult<()> {
        get_json(
            ctx.http
                .get("https://api.ideogram.ai/datasets")
                .header("Api-Key", ctx.api_key()?),
        )
        .await?;
        Ok(())
    }
    async fn submit(
        &self,
        ctx: &Ctx,
        _model: &ModelSpec,
        req: &GenerationRequest,
    ) -> ProviderResult<Submission> {
        let key = ctx.api_key()?;
        let endpoint = path(req);
        let v = if endpoint == "generate" && req.input(InputRole::Reference).is_none() {
            let mut b = json!({"prompt":req.prompt, "num_images":req.num_outputs});
            if let Some(v) = req.str(keys::ASPECT_RATIO) {
                b["aspect_ratio"] = json!(v.replace(':', "x"));
            }
            if let Some(v) = req.str(keys::QUALITY) {
                b["rendering_speed"] = json!(v);
            }
            if let Some(v) = req.str(keys::STYLE) {
                b["style_type"] = json!(v);
            }
            if let Some(v) = req.u64(keys::SEED) {
                b["seed"] = json!(v);
            }
            if let Some(v) = &req.negative_prompt {
                b["negative_prompt"] = json!(v);
            }
            send_json(
                ctx.http
                    .post(format!("{BASE}/{endpoint}"))
                    .header("Api-Key", key),
                &b,
            )
            .await?
        } else {
            let resp = ctx
                .http
                .post(format!("{BASE}/{endpoint}"))
                .header("Api-Key", key)
                .multipart(form(req).await?)
                .send()
                .await
                .map_err(net_error)?;
            pictesa_core::json_response(resp).await?
        };
        Ok(Submission::Done(outputs(&v)?))
    }
    async fn poll(&self, _ctx: &Ctx, _handle: &RemoteHandle) -> ProviderResult<PollStatus> {
        Err(ProviderError::NotSupported(
            "Ideogram v3 calls are synchronous".into(),
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::Map;
    #[test]
    fn response_and_route() {
        let req = GenerationRequest {
            job_id: "x".into(),
            model: "ideogram-v3".into(),
            prompt: "a cat".into(),
            negative_prompt: None,
            params: Map::new(),
            inputs: vec![],
            num_outputs: 1,
        };
        assert_eq!(path(&req), "generate");
        assert!(
            matches!(outputs(&json!({"data":[{"url":"https://example.com/a.png"}]})).unwrap(), v if v.len() == 1)
        );
    }
}
