use async_trait::async_trait;
use base64::Engine;
use pictesa_core::{
    keys, CredentialField, Ctx, GenerationRequest, InputRole, InputSlot, MediaKind, ModelSpec,
    Output, ParamSpec, Pricing, Provider, ProviderError, ProviderInfo, ProviderKind,
    ProviderResult, Submission, Task,
};
use serde_json::{json, Value};

use crate::util::{get_json, send_json};

pub struct OpenRouter;
const API: &str = "https://openrouter.ai/api/v1";

fn spec(id: &str, name: &str) -> ModelSpec {
    let mut m = ModelSpec::new(
        "openrouter",
        id,
        name,
        MediaKind::Image,
        &[Task::TextToImage, Task::ImageEdit],
    );
    m.vendor = Some(id.split('/').next().unwrap_or("OpenRouter").to_string());
    m.inputs = vec![InputSlot::new(InputRole::Reference, MediaKind::Image, 10)];
    m.params = vec![
        ParamSpec::choices(
            keys::ASPECT_RATIO,
            "Aspect ratio",
            &["1:1", "16:9", "9:16", "4:3", "3:4", "3:2", "2:3"],
        ),
        ParamSpec::choices(keys::QUALITY, "Quality", &["low", "medium", "high"]),
    ];
    m.pricing = Some(Pricing::Variable {
        note: "OpenRouter model pricing".into(),
    });
    m
}

fn parse_image(v: &Value) -> ProviderResult<Vec<Output>> {
    let mut out = Vec::new();
    if let Some(images) = v
        .pointer("/choices/0/message/images")
        .and_then(Value::as_array)
    {
        for image in images {
            if let Some(uri) = image
                .pointer("/image_url/url")
                .or_else(|| image.get("url"))
                .and_then(Value::as_str)
            {
                if let Some((mime, encoded)) = uri
                    .strip_prefix("data:")
                    .and_then(|s| s.split_once(";base64,"))
                {
                    let data = base64::engine::general_purpose::STANDARD
                        .decode(encoded)
                        .map_err(|e| ProviderError::Failed(format!("invalid image data: {e}")))?;
                    out.push(Output::Bytes {
                        data,
                        mime: mime.into(),
                    });
                } else if uri.starts_with("https://") {
                    out.push(Output::url(uri));
                }
            }
        }
    }
    if out.is_empty() {
        Err(ProviderError::Failed("OpenRouter returned no image".into()))
    } else {
        Ok(out)
    }
}

/// What OpenRouter charged for this answer. Every response carries it in `usage.cost`, in credits,
/// and one OpenRouter credit is one US dollar.
fn charged(v: &Value) -> Option<f64> {
    v.pointer("/usage/cost").and_then(Value::as_f64)
}

fn chat_body(model: &ModelSpec, req: &GenerationRequest, refs: Vec<String>) -> Value {
    let mut parts = vec![json!({"type":"text", "text":req.prompt})];
    parts.extend(
        refs.into_iter()
            .map(|url| json!({"type":"image_url", "image_url":{"url":url}})),
    );
    let mut body = json!({"model":model.remote_id(), "messages":[{"role":"user", "content":parts}], "modalities":["image","text"]});
    let mut config = serde_json::Map::new();
    req.copy_params(
        &mut config,
        &[
            (keys::ASPECT_RATIO, "aspect_ratio"),
            (keys::QUALITY, "quality"),
        ],
    );
    if !config.is_empty() {
        body["image_config"] = Value::Object(config);
    }
    body
}

#[async_trait]
impl Provider for OpenRouter {
    fn info(&self) -> ProviderInfo {
        ProviderInfo {
            id: "openrouter".into(),
            name: "OpenRouter".into(),
            kind: ProviderKind::Gateway,
            fields: vec![
                CredentialField::api_key(&["OPENROUTER_API_KEY"]),
                CredentialField::base_url(API, &["OPENROUTER_BASE_URL"]),
            ],
            signup_url: Some("https://openrouter.ai/keys".into()),
            docs_url: Some(
                "https://openrouter.ai/docs/guides/overview/multimodal/image-generation".into(),
            ),
            max_concurrency: 4,
        }
    }
    fn models(&self) -> Vec<ModelSpec> {
        [
            ("google/gemini-3.1-flash-image", "Nano Banana 2"),
            ("google/gemini-3-pro-image", "Nano Banana Pro"),
            ("google/gemini-2.5-flash-image", "Nano Banana"),
            ("openai/gpt-5.4-image-2", "GPT 5.4 Image 2"),
        ]
        .into_iter()
        .map(|(id, name)| spec(id, name))
        .collect()
    }
    async fn discover(&self, ctx: &Ctx) -> ProviderResult<Vec<ModelSpec>> {
        let v = get_json(
            ctx.http
                .get(format!("{}/models", ctx.base_url(API)))
                .bearer_auth(ctx.api_key()?),
        )
        .await?;
        Ok(v.get("data")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .filter_map(|m| {
                let modes = m.pointer("/architecture/output_modalities")?.as_array()?;
                if !modes.iter().any(|x| x.as_str() == Some("image")) {
                    return None;
                }
                let id = m.get("id")?.as_str()?;
                if id.starts_with("openrouter/auto") {
                    return None;
                }
                let mut s = spec(id, m.get("name").and_then(Value::as_str).unwrap_or(id));
                s.description = m
                    .get("description")
                    .and_then(Value::as_str)
                    .map(String::from);
                if let Some(created) = m.get("created").and_then(Value::as_i64) {
                    s.meta = json!({ "date": pictesa_core::utc_date(created) });
                }
                Some(s)
            })
            .collect())
    }
    async fn check(&self, ctx: &Ctx) -> ProviderResult<()> {
        get_json(
            ctx.http
                .get(format!("{}/key", ctx.base_url(API)))
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
        let mut refs = Vec::new();
        for input in req.inputs(InputRole::Reference) {
            refs.push(input.url_or_data_uri().await?);
        }
        let body = chat_body(model, req, refs);
        let v = send_json(
            ctx.http
                .post(format!("{}/chat/completions", ctx.base_url(API)))
                .bearer_auth(ctx.api_key()?),
            &body,
        )
        .await?;
        let outputs = parse_image(&v)?;
        if let Some(usd) = charged(&v) {
            ctx.report_cost(usd);
        }
        Ok(Submission::Done(outputs))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn builds_chat_with_reference_and_config() {
        let mut params = serde_json::Map::new();
        params.insert(keys::ASPECT_RATIO.into(), json!("16:9"));
        let req = GenerationRequest {
            job_id: "t".into(),
            model: "google/gemini-3.1-flash-image".into(),
            prompt: "draw".into(),
            negative_prompt: None,
            params,
            inputs: vec![],
            num_outputs: 1,
        };
        let body = chat_body(
            &spec("google/gemini-3.1-flash-image", "Nano Banana"),
            &req,
            vec!["https://example.com/ref.png".into()],
        );
        assert_eq!(body["modalities"], json!(["image", "text"]));
        assert_eq!(
            body["messages"][0]["content"][1]["image_url"]["url"],
            "https://example.com/ref.png"
        );
        assert_eq!(body["image_config"]["aspect_ratio"], "16:9");
    }
    #[test]
    fn reads_the_charge_from_usage() {
        let v = json!({"choices":[], "usage":{"prompt_tokens":12,"completion_tokens":1290,"cost":0.0387}});
        assert_eq!(charged(&v), Some(0.0387));
        assert_eq!(charged(&json!({"choices":[]})), None);
    }
    #[test]
    fn parses_recorded_chat_image() {
        let v = json!({"choices":[{"message":{"images":[{"image_url":{"url":"data:image/png;base64,YQ=="}}]}}]});
        assert!(
            matches!(&parse_image(&v).unwrap()[0], Output::Bytes { data, mime } if data == b"a" && mime == "image/png")
        );
    }
}
