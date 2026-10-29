use crate::util::{failure, get_json, send_json};
use async_trait::async_trait;
use base64::Engine;
use pictesa_core::{
    collect_media_urls, schema, CredentialField, Ctx, GenerationRequest, MediaKind, ModelSpec,
    Output, PollStatus, Pricing, Provider, ProviderError, ProviderInfo, ProviderKind,
    ProviderResult, RemoteHandle, Submission, Task,
};
use serde_json::{json, Value};

pub struct Replicate;
const API: &str = "https://api.replicate.com/v1";

fn spec(id: &str, output: MediaKind, complete: bool) -> ModelSpec {
    let tasks = if output == MediaKind::Video {
        vec![Task::TextToVideo, Task::ImageToVideo]
    } else {
        vec![Task::TextToImage, Task::ImageEdit]
    };
    let mut m = ModelSpec::new("replicate", id, &crate::util::slug_name(id), output, &tasks);
    m.vendor = Some(id.split('/').next().map(crate::util::owner_name).unwrap_or_else(|| "Replicate".into()));
    m.complete = complete;
    m.pricing = Some(Pricing::Variable {
        note: "Replicate model pricing".into(),
    });
    m
}
fn output_media(v: &Value, want: MediaKind, key: &str) -> ProviderResult<Vec<Output>> {
    let mut out = Vec::new();
    for url in collect_media_urls(v, Some(want)) {
        if let Some((mime, encoded)) = url
            .strip_prefix("data:")
            .and_then(|s| s.split_once(";base64,"))
        {
            out.push(Output::Bytes {
                data: base64::engine::general_purpose::STANDARD
                    .decode(encoded)
                    .map_err(|e| ProviderError::Failed(e.to_string()))?,
                mime: mime.into(),
            });
        } else {
            let headers = if url::Url::parse(&url)
                .ok()
                .and_then(|u| u.host_str().map(String::from))
                .as_deref()
                == Some("api.replicate.com")
            {
                vec![("Authorization".into(), format!("Bearer {key}"))]
            } else {
                Vec::new()
            };
            out.push(Output::Url { url, headers });
        }
    }
    if out.is_empty() {
        Err(ProviderError::Failed(
            "Replicate finished without media".into(),
        ))
    } else {
        Ok(out)
    }
}
/// Cog publishes enum fields as `"allOf": [{"$ref": "#/components/schemas/aspect_ratio"}]` beside
/// their default (https://replicate.com/docs/reference/openapi#model-schemas). The shared schema
/// reader follows only a bare `$ref`, so the referenced enum is inlined into the field first.
fn inline_enum_refs(input: &Value, defs: Option<&Value>) -> Value {
    let mut input = input.clone();
    if let Some(props) = input.get_mut("properties").and_then(Value::as_object_mut) {
        for field in props.values_mut() {
            let target = field
                .get("allOf")
                .and_then(Value::as_array)
                .filter(|all| all.len() == 1)
                .and_then(|all| all[0].get("$ref")?.as_str())
                .and_then(|r| defs?.get(r.rsplit('/').next()?))
                .and_then(Value::as_object)
                .cloned();
            if let (Some(target), Some(field)) = (target, field.as_object_mut()) {
                field.remove("allOf");
                for (k, v) in target {
                    field.entry(k).or_insert(v);
                }
            }
        }
    }
    input
}
fn prediction_body(
    model: &ModelSpec,
    version: &str,
    input: serde_json::Map<String, Value>,
) -> Value {
    json!({"version":format!("{}:{version}",model.remote_id()),"input":input})
}

#[async_trait]
impl Provider for Replicate {
    fn info(&self) -> ProviderInfo {
        ProviderInfo {
            id: "replicate".into(),
            name: "Replicate".into(),
            kind: ProviderKind::Gateway,
            fields: vec![CredentialField::api_key(&["REPLICATE_API_TOKEN"])],
            signup_url: Some("https://replicate.com/account/api-tokens".into()),
            docs_url: Some("https://replicate.com/docs/reference/http".into()),
            max_concurrency: 4,
        }
    }
    fn models(&self) -> Vec<ModelSpec> {
        vec![
            spec("google/nano-banana-2", MediaKind::Image, false),
            spec("black-forest-labs/flux-2-pro", MediaKind::Image, false),
            spec("bytedance/seedance-2.0", MediaKind::Video, false),
            spec("google/veo-3.1-fast", MediaKind::Video, false),
        ]
    }
    async fn discover(&self, ctx: &Ctx) -> ProviderResult<Vec<ModelSpec>> {
        let mut out = Vec::new();
        for (slug, kind) in [
            ("text-to-image", MediaKind::Image),
            ("text-to-video", MediaKind::Video),
            ("image-to-video", MediaKind::Video),
        ] {
            let v = get_json(
                ctx.http
                    .get(format!("{API}/collections/{slug}"))
                    .bearer_auth(ctx.api_key()?),
            )
            .await?;
            for m in v
                .get("models")
                .and_then(Value::as_array)
                .into_iter()
                .flatten()
            {
                let (Some(owner), Some(name)) = (
                    m.get("owner").and_then(Value::as_str),
                    m.get("name").and_then(Value::as_str),
                ) else {
                    continue;
                };
                let mut s = spec(&format!("{owner}/{name}"), kind, false);
                s.name = crate::util::slug_name(name);
                s.description = m
                    .get("description")
                    .and_then(Value::as_str)
                    .map(String::from);
                out.push(s);
            }
        }
        out.sort_by(|a, b| a.id.cmp(&b.id));
        out.dedup_by(|a, b| a.id == b.id);
        Ok(out)
    }
    async fn resolve(&self, ctx: &Ctx, model: &ModelSpec) -> ProviderResult<ModelSpec> {
        let v = get_json(
            ctx.http
                .get(format!("{API}/models/{}", model.remote_id()))
                .bearer_auth(ctx.api_key()?),
        )
        .await?;
        let version = v.get("latest_version").ok_or_else(|| {
            ProviderError::NotSupported("Replicate model has no latest version".into())
        })?;
        let doc = version.get("openapi_schema").ok_or_else(|| {
            ProviderError::NotSupported("Replicate model has no input schema".into())
        })?;
        let input = doc.pointer("/components/schemas/Input").ok_or_else(|| {
            ProviderError::NotSupported("Replicate model has no Input schema".into())
        })?;
        let defs = doc.pointer("/components/schemas");
        let read = schema::read_input_schema(&inline_enum_refs(input, defs), defs, model.output);
        let mut s = model.clone();
        s.params = read.params;
        s.inputs = read.inputs;
        s.prompt = read.has_prompt;
        s.negative_prompt = read.has_negative_prompt;
        s.max_outputs = read.max_outputs;
        s.complete = true;
        s.meta = json!({"bindings":read.bindings,"version":version.get("id"),"official":v.get("visibility").and_then(Value::as_str)==Some("public") && v.get("run_count").is_some()});
        Ok(s)
    }
    async fn check(&self, ctx: &Ctx) -> ProviderResult<()> {
        get_json(
            ctx.http
                .get(format!("{API}/account"))
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
        let s = if model.meta.get("bindings").is_none() {
            self.resolve(ctx, model).await?
        } else {
            model.clone()
        };
        let bindings = &s.meta["bindings"];
        let mut input = schema::body_from_bindings(bindings, req);
        for (role, field, array) in schema::input_bindings(bindings) {
            let mut urls = Vec::new();
            for media in req.inputs(role) {
                urls.push(media.url_or_data_uri().await?);
            }
            if !urls.is_empty() {
                input.insert(field, if array { json!(urls) } else { json!(urls[0]) });
            }
        }
        let version = s
            .meta
            .get("version")
            .and_then(Value::as_str)
            .ok_or_else(|| ProviderError::NotSupported("Replicate model has no version".into()))?;
        let v = send_json(
            ctx.http
                .post(format!("{API}/predictions"))
                .bearer_auth(ctx.api_key()?),
            &prediction_body(&s, version, input),
        )
        .await?;
        let id = v
            .get("id")
            .and_then(Value::as_str)
            .ok_or_else(|| ProviderError::Failed("Replicate returned no prediction id".into()))?;
        Ok(Submission::Pending(RemoteHandle::with(
            id,
            json!({"output":model.output}),
        )))
    }
    async fn poll(&self, ctx: &Ctx, handle: &RemoteHandle) -> ProviderResult<PollStatus> {
        let v = get_json(
            ctx.http
                .get(format!("{API}/predictions/{}", handle.id))
                .bearer_auth(ctx.api_key()?),
        )
        .await?;
        let want = handle
            .data
            .get("output")
            .and_then(|x| serde_json::from_value::<MediaKind>(x.clone()).ok())
            .unwrap_or(MediaKind::Image);
        match v.get("status").and_then(Value::as_str).unwrap_or("") {
            "starting" => Ok(PollStatus::Queued { position: None }),
            "processing" => Ok(PollStatus::Running {
                progress: None,
                message: None,
            }),
            "succeeded" => Ok(PollStatus::Done(output_media(
                &v["output"],
                want,
                ctx.api_key()?,
            )?)),
            "failed" | "canceled" => Ok(PollStatus::Failed(failure(
                v.get("error")
                    .and_then(Value::as_str)
                    .unwrap_or("Replicate prediction failed"),
            ))),
            other => Err(ProviderError::Failed(format!(
                "unknown Replicate state: {other}"
            ))),
        }
    }
    async fn cancel(&self, ctx: &Ctx, handle: &RemoteHandle) -> ProviderResult<()> {
        get_json(
            ctx.http
                .post(format!("{API}/predictions/{}/cancel", handle.id))
                .bearer_auth(ctx.api_key()?),
        )
        .await?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn parses_recorded_output() {
        assert!(
            matches!(&output_media(&json!(["https://replicate.delivery/a.png"]),MediaKind::Image,"secret").unwrap()[0],Output::Url{headers,..} if headers.is_empty())
        );
    }
    #[test]
    fn builds_versioned_prediction() {
        let body = prediction_body(
            &spec("black-forest-labs/flux-2-pro", MediaKind::Image, true),
            "abc",
            serde_json::Map::from_iter([("prompt".into(), json!("draw"))]),
        );
        assert_eq!(
            body,
            json!({"version":"black-forest-labs/flux-2-pro:abc","input":{"prompt":"draw"}})
        );
    }
}
