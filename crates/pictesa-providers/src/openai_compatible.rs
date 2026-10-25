use async_trait::async_trait;
use base64::Engine;
use pictesa_core::{
    keys, CredentialField, Ctx, GenerationRequest, InputRole, InputSlot, MediaKind, ModelSpec,
    Output, ParamSpec, Pricing, Provider, ProviderError, ProviderInfo, ProviderKind,
    ProviderResult, Submission, Task,
};
use reqwest::{
    multipart::{Form, Part},
    RequestBuilder,
};
use serde_json::{json, Value};

use crate::util::{get_json, net_error, ratio_to_size, send_json};

pub struct OpenAICompatible;
// shortcut: credentials are keyed by provider ID, add instance IDs when the engine supports multiple servers.

fn api(ctx: &Ctx) -> ProviderResult<String> {
    let url = ctx.require("base_url")?.trim_end_matches('/');
    let parsed = url::Url::parse(url)
        .map_err(|_| ProviderError::InvalidInput("invalid server URL".into()))?;
    if !matches!(parsed.scheme(), "http" | "https") || parsed.host_str().is_none() {
        return Err(ProviderError::InvalidInput(
            "server URL must use HTTP or HTTPS".into(),
        ));
    }
    Ok(url.into())
}

fn auth(req: RequestBuilder, ctx: &Ctx) -> RequestBuilder {
    match ctx.get("api_key") {
        Some(key) => req.bearer_auth(key),
        None => req,
    }
}

fn spec(id: &str, name: &str) -> ModelSpec {
    ModelSpec::new(
        "openai_compatible",
        id,
        name,
        MediaKind::Image,
        &[Task::TextToImage, Task::ImageEdit],
    )
    .input(InputSlot::new(InputRole::Reference, MediaKind::Image, 4))
    .input(InputSlot::new(InputRole::Mask, MediaKind::Image, 1))
    .param(ParamSpec::int(keys::WIDTH, "Width", 64, 4096))
    .param(ParamSpec::int(keys::HEIGHT, "Height", 64, 4096))
    .param(ParamSpec::seed())
    .pricing(Pricing::Variable {
        note: "Server-specific pricing".into(),
    })
}

/// A server on this machine, the local network or a tailnet costs nothing per run.
fn is_local_server(url: &str) -> bool {
    match url::Url::parse(url).ok().as_ref().and_then(url::Url::host) {
        Some(url::Host::Domain(d)) => d == "localhost" || d.ends_with(".localhost") || d.ends_with(".local"),
        Some(url::Host::Ipv4(ip)) => {
            let [a, b, ..] = ip.octets();
            ip.is_loopback() || ip.is_private() || ip.is_link_local() || (a == 100 && (64..128).contains(&b))
        }
        Some(url::Host::Ipv6(ip)) => ip.is_loopback() || (ip.segments()[0] & 0xfe00) == 0xfc00,
        None => false,
    }
}

fn discover_models(v: &Value, prefix: Option<&str>) -> Vec<ModelSpec> {
    v.get("data")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|m| {
            let id = m.get("id")?.as_str()?;
            let output = m
                .get("output_modalities")
                .or_else(|| m.get("supported_output_modalities"));
            let capability = m
                .pointer("/capabilities/image_generation")
                .or_else(|| m.pointer("/capabilities/images"));
            let marked = output
                .or(capability)
                .or_else(|| m.get("modalities"))
                .or_else(|| m.get("supported_modalities"))
                .or_else(|| m.get("type"))
                .or_else(|| m.get("task"));
            let image_capable = match marked {
                Some(Value::Bool(b)) => *b,
                Some(v) => ["image", "diffusion", "txt2img"]
                    .iter()
                    .any(|s| v.to_string().to_ascii_lowercase().contains(s)),
                None => true,
            };
            if !image_capable {
                return None;
            }
            let label = m.get("name").and_then(Value::as_str).unwrap_or(id);
            let name = prefix.map_or(label.to_string(), |p| format!("{p}: {label}"));
            let mut model = spec(id, &name);
            if marked.is_none() {
                model.tags.push("unverified".into());
            }
            Some(model)
        })
        .collect()
}

fn body(req: &GenerationRequest, model: &ModelSpec) -> Value {
    let mut b = json!({"model":model.remote_id(),"prompt":req.prompt,"n":req.num_outputs});
    if let (Some(w), Some(h)) = (req.u64(keys::WIDTH), req.u64(keys::HEIGHT)) {
        b["size"] = json!(format!("{w}x{h}"));
    } else if let Some(ratio) = req.str(keys::ASPECT_RATIO) {
        if let Some((w, h)) = ratio_to_size(ratio, 1024) {
            b["size"] = json!(format!("{w}x{h}"));
        }
    }
    if let Some(seed) = req.u64(keys::SEED) {
        b["seed"] = json!(seed);
    }
    b
}

fn outputs(v: &Value) -> ProviderResult<Vec<Output>> {
    let mut out = Vec::new();
    for item in v
        .get("data")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
    {
        if let Some(b64) = item.get("b64_json").and_then(Value::as_str) {
            out.push(Output::Bytes {
                data: base64::engine::general_purpose::STANDARD
                    .decode(b64)
                    .map_err(|e| ProviderError::Failed(format!("invalid base64 image: {e}")))?,
                mime: "image/png".into(),
            });
        } else if let Some(url) = item.get("url").and_then(Value::as_str) {
            out.push(Output::url(url));
        }
    }
    if out.is_empty() {
        Err(ProviderError::Failed("image API returned no images".into()))
    } else {
        Ok(out)
    }
}

async fn part(input: &pictesa_core::MediaInput) -> ProviderResult<Part> {
    Part::bytes(input.bytes().await?)
        .file_name(input.file_name())
        .mime_str(&input.mime)
        .map_err(|_| ProviderError::InvalidInput("invalid image MIME type".into()))
}

#[async_trait]
impl Provider for OpenAICompatible {
    fn info(&self) -> ProviderInfo {
        ProviderInfo {
            id: "openai_compatible".into(),
            name: "OpenAI-compatible images".into(),
            kind: ProviderKind::Gateway,
            fields: vec![
                CredentialField {
                    key: "base_url".into(),
                    label: "API base URL".into(),
                    env: vec!["OPENAI_COMPATIBLE_BASE_URL".into()],
                    secret: false,
                    optional: false,
                    default: None,
                    placeholder: Some("https://server.example/v1".into()),
                },
                CredentialField {
                    key: "api_key".into(),
                    label: "API key".into(),
                    env: vec!["OPENAI_COMPATIBLE_API_KEY".into()],
                    secret: true,
                    optional: true,
                    default: None,
                    placeholder: None,
                },
                CredentialField {
                    key: "name".into(),
                    label: "Server name".into(),
                    env: vec![],
                    secret: false,
                    optional: true,
                    default: None,
                    placeholder: Some("My server".into()),
                },
            ],
            signup_url: None,
            docs_url: Some("https://platform.openai.com/docs/api-reference/images".into()),
            max_concurrency: 2,
        }
    }
    fn models(&self) -> Vec<ModelSpec> {
        Vec::new()
    }
    async fn discover(&self, ctx: &Ctx) -> ProviderResult<Vec<ModelSpec>> {
        let base = api(ctx)?;
        let v = get_json(auth(ctx.http.get(format!("{base}/models")), ctx)).await?;
        let mut models = discover_models(&v, ctx.get("name"));
        if is_local_server(&base) {
            for m in &mut models {
                m.pricing = Some(Pricing::Local);
            }
        }
        Ok(models)
    }
    async fn check(&self, ctx: &Ctx) -> ProviderResult<()> {
        get_json(auth(ctx.http.get(format!("{}/models", api(ctx)?)), ctx)).await?;
        Ok(())
    }
    async fn submit(
        &self,
        ctx: &Ctx,
        model: &ModelSpec,
        req: &GenerationRequest,
    ) -> ProviderResult<Submission> {
        let base = api(ctx)?;
        let refs: Vec<_> = req.inputs(InputRole::Reference).collect();
        let v = if refs.is_empty() {
            if req.input(InputRole::Mask).is_some() {
                return Err(ProviderError::InvalidInput(
                    "mask requires a reference image".into(),
                ));
            }
            send_json(
                auth(ctx.http.post(format!("{base}/images/generations")), ctx),
                &body(req, model),
            )
            .await?
        } else {
            let b = body(req, model);
            let mut form = Form::new()
                .text("model", model.remote_id().to_string())
                .text("prompt", req.prompt.clone())
                .text("n", req.num_outputs.to_string());
            if let Some(size) = b.get("size").and_then(Value::as_str) {
                form = form.text("size", size.to_string());
            }
            if let Some(seed) = req.u64(keys::SEED) {
                form = form.text("seed", seed.to_string());
            }
            for input in refs {
                form = form.part("image", part(input).await?);
            }
            if let Some(mask) = req.input(InputRole::Mask) {
                form = form.part("mask", part(mask).await?);
            }
            let response = auth(ctx.http.post(format!("{base}/images/edits")), ctx)
                .multipart(form)
                .send()
                .await
                .map_err(net_error)?;
            pictesa_core::json_response(response).await?
        };
        Ok(Submission::Done(outputs(&v)?))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn servers_on_this_machine_or_network_are_local() {
        for url in ["http://127.0.0.1:1234/v1", "http://localhost:8080/v1", "http://192.168.1.20:7860/v1", "http://100.64.0.1:9000/v1", "http://[::1]:1234/v1", "http://gpu-box.local/v1"] {
            assert!(is_local_server(url), "{url}");
        }
        for url in ["https://api.example.com/v1", "http://8.8.8.8/v1", "https://openrouter.ai/api/v1"] {
            assert!(!is_local_server(url), "{url}");
        }
    }
    #[test]
    fn discovery_respects_server_capability_and_marks_unknown() {
        let models = discover_models(
            &json!({"data":[{"id":"img","output_modalities":["image"]},{"id":"text","output_modalities":["text"]},{"id":"disabled","capabilities":{"image_generation":false}},{"id":"unknown"}]}),
            Some("Lab"),
        );
        assert_eq!(models.len(), 2);
        assert_eq!(models[0].name, "Lab: img");
        assert_eq!(models[1].tags, vec!["unverified"]);
    }
    #[test]
    fn parses_b64_and_url_results() {
        let out = outputs(
            &json!({"data":[{"b64_json":"YQ=="},{"url":"https://example.test/image.png"}]}),
        )
        .unwrap();
        assert!(matches!(&out[0],Output::Bytes{data,..} if data==b"a"));
        assert!(matches!(&out[1],Output::Url{url,..} if url.ends_with("image.png")));
    }
    #[test]
    fn maps_size_seed_and_output_count() {
        let req: GenerationRequest = serde_json::from_value(json!({"job_id":"x","model":"img","prompt":"cat","params":{"width":768,"height":1024,"seed":42},"num_outputs":2})).unwrap();
        let b = body(&req, &spec("img", "Image"));
        assert_eq!(b["size"], "768x1024");
        assert_eq!(b["seed"], 42);
        assert_eq!(b["n"], 2);
    }
}
