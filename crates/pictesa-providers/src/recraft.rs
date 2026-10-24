use async_trait::async_trait;
use base64::Engine as _;
use pictesa_core::{
    common, keys, CredentialField, Ctx, GenerationRequest, MediaKind, ModelSpec, Output, ParamSpec,
    PollStatus, Pricing, Provider, ProviderError, ProviderInfo, ProviderKind, ProviderResult,
    RemoteHandle, Submission, Task,
};
use serde_json::Value;

use crate::util::{get_json, net_error};

const BASE: &str = "https://external.api.recraft.ai/v1";
pub struct Recraft;

fn fields(model: &ModelSpec, req: &GenerationRequest) -> Vec<(&'static str, String)> {
    let mut out = vec![
        ("model", model.remote_id().to_string()),
        ("prompt", req.prompt.clone()),
        ("n", req.num_outputs.to_string()),
    ];
    if let Some(v) = req.str(keys::ASPECT_RATIO) {
        out.push(("size", v.to_string()));
    }
    if let Some(v) = req.str(keys::STYLE) {
        out.push(("style", v.to_string()));
    }
    if let Some(v) = req.u64(keys::SEED) {
        out.push(("random_seed", v.to_string()));
    }
    if let Some(v) = &req.negative_prompt {
        out.push(("negative_prompt", v.clone()));
    }
    if let Some(v) = req.str(keys::OUTPUT_FORMAT) {
        out.push(("image_format", v.to_string()));
    }
    out
}

fn form(model: &ModelSpec, req: &GenerationRequest) -> reqwest::multipart::Form {
    fields(model, req)
        .into_iter()
        .fold(reqwest::multipart::Form::new(), |form, (k, v)| {
            form.text(k, v)
        })
}

fn outputs(v: &Value, vector: bool) -> ProviderResult<Vec<Output>> {
    let data = v
        .get("data")
        .and_then(Value::as_array)
        .ok_or_else(|| ProviderError::Failed("Recraft returned no data array".into()))?;
    if data.is_empty() {
        return Err(ProviderError::Failed("Recraft returned no images".into()));
    }
    data.iter()
        .map(|item| {
            if let Some(url) = item.get("url").and_then(Value::as_str) {
                return Ok(Output::url(url));
            }
            let encoded = item
                .get("b64_json")
                .and_then(Value::as_str)
                .ok_or_else(|| ProviderError::Failed("Recraft image has no URL or data".into()))?;
            let data = base64::engine::general_purpose::STANDARD
                .decode(encoded)
                .map_err(|_| ProviderError::Failed("Recraft returned invalid base64".into()))?;
            Ok(Output::Bytes {
                data,
                mime: if vector { "image/svg+xml" } else { "image/png" }.into(),
            })
        })
        .collect()
}

#[async_trait]
impl Provider for Recraft {
    fn info(&self) -> ProviderInfo {
        ProviderInfo {
            id: "recraft".into(),
            name: "Recraft".into(),
            kind: ProviderKind::Cloud,
            fields: vec![CredentialField::api_key(&["RECRAFT_API_KEY"])],
            signup_url: Some("https://www.recraft.ai/".into()),
            docs_url: Some("https://www.recraft.ai/docs".into()),
            max_concurrency: 4,
        }
    }
    fn models(&self) -> Vec<ModelSpec> {
        [
            ("recraftv4_1", "Recraft V4.1", false, 0.035),
            ("recraftv4_1_pro", "Recraft V4.1 Pro", false, 0.21),
            ("recraftv4_1_vector", "Recraft V4.1 Vector", true, 0.08),
            (
                "recraftv4_1_pro_vector",
                "Recraft V4.1 Pro Vector",
                true,
                0.30,
            ),
        ]
        .into_iter()
        .map(|(id, name, vector, price)| {
            let mut m = ModelSpec::new("recraft", id, name, MediaKind::Image, &[Task::TextToImage])
                .param(common::aspect_ratio(
                    &["1:1", "16:9", "9:16", "4:3", "3:4", "3:2", "2:3"],
                    "1:1",
                ))
                .param(ParamSpec::text(keys::STYLE, "Style").advanced())
                .param(ParamSpec::seed())
                .max_outputs(4)
                .pricing(Pricing::PerImage { usd: price });
            if !vector {
                m = m.param(
                    ParamSpec::choices(keys::OUTPUT_FORMAT, "Format", &["png", "webp"])
                        .default_value("png"),
                );
            }
            m
        })
        .collect()
    }
    async fn check(&self, ctx: &Ctx) -> ProviderResult<()> {
        get_json(
            ctx.http
                .get(format!("{BASE}/users/me"))
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
        let vector = model.remote_id().contains("vector");
        let endpoint = if vector {
            "generations/vector"
        } else {
            "generations"
        };
        let resp = ctx
            .http
            .post(format!("{BASE}/images/{endpoint}"))
            .bearer_auth(ctx.api_key()?)
            .multipart(form(model, req))
            .send()
            .await
            .map_err(net_error)?;
        let v = pictesa_core::json_response(resp).await?;
        Ok(Submission::Done(outputs(&v, vector)?))
    }
    async fn poll(&self, _ctx: &Ctx, _handle: &RemoteHandle) -> ProviderResult<PollStatus> {
        Err(ProviderError::NotSupported(
            "Recraft image generation is synchronous".into(),
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    #[test]
    fn maps_shared_keys_to_recraft_fields() {
        let model = Recraft.models().remove(0);
        let mut req = GenerationRequest {
            job_id: "x".into(),
            model: model.remote_id().into(),
            prompt: "a cat".into(),
            negative_prompt: None,
            params: serde_json::Map::new(),
            inputs: vec![],
            num_outputs: 2,
        };
        req.params.insert(keys::ASPECT_RATIO.into(), json!("16:9"));
        req.params
            .insert(keys::STYLE.into(), json!("realistic_image"));
        let got = fields(&model, &req);
        assert!(got.contains(&("size", "16:9".into())));
        assert!(got.contains(&("style", "realistic_image".into())));
        assert!(got.contains(&("n", "2".into())));
    }
    #[test]
    fn response_contract() {
        assert!(matches!(
            outputs(
                &json!({"data":[{"url":"https://example.com/a.svg","image_id":"abc"}]}),
                true
            )
            .unwrap()
            .remove(0),
            Output::Url { .. }
        ));
        assert!(
            matches!(outputs(&json!({"data":[{"b64_json":"AQID","image_id":"abc"}]}), false).unwrap().remove(0), Output::Bytes { data, .. } if data == [1,2,3])
        );
    }
}
