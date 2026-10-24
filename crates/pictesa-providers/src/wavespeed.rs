use std::time::Duration;

use async_trait::async_trait;
use base64::Engine;
use pictesa_core::{
    collect_media_urls, common, keys, schema, CredentialField, Ctx, GenerationRequest, InputRole,
    InputSlot, MediaKind, ModelSpec, Output, ParamSpec, PollStatus, Pricing, Provider,
    ProviderError, ProviderInfo, ProviderKind, ProviderResult, RemoteHandle, Submission, Task,
};
use serde_json::{json, Value};

use crate::util::{failure, get_json, ratio_to_size, send_json};

pub struct WaveSpeed;
const API: &str = "https://api.wavespeed.ai/api/v3";

fn checked(v: Value) -> ProviderResult<Value> {
    if v.get("code").and_then(Value::as_i64).unwrap_or(200) != 200 {
        return Err(failure(
            v.get("message")
                .and_then(Value::as_str)
                .unwrap_or("WaveSpeed API error"),
        ));
    }
    Ok(v)
}
fn spec(id: &str, kind: MediaKind, tasks: &[Task]) -> ModelSpec {
    ModelSpec::new("wavespeed", id, id, kind, tasks)
        .param(ParamSpec::seed())
        .pricing(Pricing::Variable {
            note: "WaveSpeed model-specific pricing".into(),
        })
}
fn catalog(v: &Value) -> Vec<ModelSpec> {
    v.get("data")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|item| {
            let id = item.get("model_id")?.as_str()?;
            let typ = item.get("type").and_then(Value::as_str).unwrap_or("");
            let (kind, tasks) = match typ {
                "text-to-image" => (MediaKind::Image, vec![Task::TextToImage]),
                "image-to-image" => (MediaKind::Image, vec![Task::ImageEdit]),
                "text-to-video" => (MediaKind::Video, vec![Task::TextToVideo]),
                "image-to-video" => (MediaKind::Video, vec![Task::ImageToVideo]),
                _ => return None,
            };
            let mut m = spec(id, kind, &tasks);
            m.name = item
                .get("name")
                .and_then(Value::as_str)
                .unwrap_or(id)
                .into();
            m.description = item
                .get("description")
                .and_then(Value::as_str)
                .map(String::from);
            if let Some(schema) = item
                .pointer("/api_schema/api_schemas")
                .and_then(Value::as_array)
                .and_then(|a| {
                    a.iter()
                        .find(|s| s.get("type").and_then(Value::as_str) == Some("model_run"))
                })
            {
                if let Some(input) = schema.get("request_schema") {
                    let read = schema::read_input_schema(input, None, kind);
                    m.params = read.params;
                    m.inputs = read.inputs;
                    m.prompt = read.has_prompt;
                    m.negative_prompt = read.has_negative_prompt;
                    m.max_outputs = read.max_outputs;
                    m.meta = json!({"bindings":read.bindings});
                }
            }
            Some(m)
        })
        .collect()
}
fn body(model: &ModelSpec, req: &GenerationRequest) -> Value {
    if let Some(bindings) = model.meta.get("bindings") {
        return Value::Object(schema::body_from_bindings(bindings, req));
    }
    let mut b = json!({"prompt":req.prompt});
    if let Some(seed) = req.u64(keys::SEED) {
        b["seed"] = json!(seed);
    }
    if model.remote_id() == "wavespeed-ai/z-image/turbo" {
        if let Some(ratio) = req.str(keys::ASPECT_RATIO) {
            if let Some((w, h)) = ratio_to_size(ratio, 1024) {
                b["size"] = json!(format!("{w}*{h}"));
            }
        }
        req.copy_params(
            b.as_object_mut().unwrap(),
            &[
                (keys::STRENGTH, "strength"),
                (keys::OUTPUT_FORMAT, "output_format"),
            ],
        );
    } else if model.remote_id() == "wavespeed-ai/hunyuan-video/t2v" {
        if let Some(ratio) = req.str(keys::ASPECT_RATIO) {
            b["size"] = json!(if ratio == "9:16" {
                "720*1280"
            } else {
                "1280*720"
            });
        }
        req.copy_params(
            b.as_object_mut().unwrap(),
            &[(keys::STEPS, "num_inference_steps")],
        );
    }
    b
}
async fn fill_inputs(
    model: &ModelSpec,
    req: &GenerationRequest,
    body: &mut Value,
) -> ProviderResult<()> {
    if let Some(bindings) = model.meta.get("bindings") {
        for (role, field, array) in schema::input_bindings(bindings) {
            let mut values = Vec::new();
            for input in req.inputs(role) {
                values.push(input.url_or_data_uri().await?);
            }
            if !values.is_empty() {
                body[&field] = if array {
                    json!(values)
                } else {
                    json!(values[0])
                };
            }
        }
    } else if model.remote_id() == "wavespeed-ai/z-image/turbo" {
        if let Some(input) = req.input(InputRole::Reference) {
            body["image"] = json!(input
                .url
                .as_deref()
                .filter(|u| u.starts_with("https://"))
                .ok_or_else(|| ProviderError::InvalidInput(
                    "Z-Image reference needs a public HTTPS URL".into()
                ))?);
        }
    }
    Ok(())
}
fn parse_result(v: &Value, kind: MediaKind) -> ProviderResult<PollStatus> {
    let data = v
        .get("data")
        .ok_or_else(|| ProviderError::Failed("WaveSpeed response has no data".into()))?;
    match data.get("status").and_then(Value::as_str).unwrap_or("") {
        "created" => Ok(PollStatus::Queued { position: None }),
        "processing" => Ok(PollStatus::Running {
            progress: None,
            message: None,
        }),
        "completed" => {
            let urls = collect_media_urls(&data["outputs"], Some(kind));
            if urls.is_empty() {
                return Err(ProviderError::Failed(
                    "WaveSpeed completed without media".into(),
                ));
            }
            let out = urls
                .into_iter()
                .map(|url| {
                    if let Some((mime, encoded)) = url
                        .strip_prefix("data:")
                        .and_then(|s| s.split_once(";base64,"))
                    {
                        Ok(Output::Bytes {
                            data: base64::engine::general_purpose::STANDARD
                                .decode(encoded)
                                .map_err(|e| {
                                    ProviderError::Failed(format!(
                                        "invalid WaveSpeed base64 output: {e}"
                                    ))
                                })?,
                            mime: mime.into(),
                        })
                    } else {
                        Ok(Output::url(url))
                    }
                })
                .collect::<ProviderResult<Vec<_>>>()?;
            Ok(PollStatus::Done(out))
        }
        "failed" | "cancelled" | "canceled" | "timeout" | "deleted" => {
            Ok(PollStatus::Failed(failure(
                data.get("error")
                    .and_then(Value::as_str)
                    .unwrap_or("WaveSpeed task failed"),
            )))
        }
        state => Err(ProviderError::Failed(format!(
            "unknown WaveSpeed state: {state}"
        ))),
    }
}

#[async_trait]
impl Provider for WaveSpeed {
    fn info(&self) -> ProviderInfo {
        ProviderInfo {
            id: "wavespeed".into(),
            name: "WaveSpeed AI".into(),
            kind: ProviderKind::Gateway,
            fields: vec![CredentialField::api_key(&["WAVESPEED_API_KEY"])],
            signup_url: Some("https://wavespeed.ai/accesskey".into()),
            docs_url: Some("https://wavespeed.ai/docs/rest-api".into()),
            max_concurrency: 4,
        }
    }
    fn models(&self) -> Vec<ModelSpec> {
        vec![
            spec(
                "wavespeed-ai/z-image/turbo",
                MediaKind::Image,
                &[Task::TextToImage, Task::ImageEdit],
            )
            .input(InputSlot::new(InputRole::Reference, MediaKind::Image, 1))
            .param(common::aspect_ratio(
                &["1:1", "16:9", "9:16", "4:3", "3:4", "3:2", "2:3"],
                "1:1",
            ))
            .param(ParamSpec::float(keys::STRENGTH, "Strength", 0.0, 1.0, 0.05).default_value(0.6))
            .param(common::output_format("jpeg")),
            spec(
                "wavespeed-ai/hunyuan-video/t2v",
                MediaKind::Video,
                &[Task::TextToVideo],
            )
            .param(common::aspect_ratio(&["16:9", "9:16"], "16:9"))
            .param(common::steps(2, 30, 30)),
        ]
    }
    async fn discover(&self, ctx: &Ctx) -> ProviderResult<Vec<ModelSpec>> {
        Ok(catalog(&checked(
            get_json(
                ctx.http
                    .get(format!("{API}/models"))
                    .bearer_auth(ctx.api_key()?),
            )
            .await?,
        )?))
    }
    async fn check(&self, ctx: &Ctx) -> ProviderResult<()> {
        checked(
            get_json(
                ctx.http
                    .get(format!("{API}/models"))
                    .bearer_auth(ctx.api_key()?),
            )
            .await?,
        )?;
        Ok(())
    }
    async fn submit(
        &self,
        ctx: &Ctx,
        model: &ModelSpec,
        req: &GenerationRequest,
    ) -> ProviderResult<Submission> {
        let mut b = body(model, req);
        fill_inputs(model, req, &mut b).await?;
        let v = checked(
            send_json(
                ctx.http
                    .post(format!("{API}/{}", model.remote_id()))
                    .bearer_auth(ctx.api_key()?),
                &b,
            )
            .await
            .map_err(|e| match e {
                ProviderError::Transient(_) => ProviderError::Failed("WaveSpeed submission outcome is unknown; inspect prediction history before retrying".into()),
                other => other,
            })?,
        )?;
        let id = v
            .pointer("/data/id")
            .and_then(Value::as_str)
            .ok_or_else(|| ProviderError::Failed("WaveSpeed returned no prediction id".into()))?;
        Ok(Submission::Pending(RemoteHandle::with(
            id,
            json!({"kind":model.output}),
        )))
    }
    async fn poll(&self, ctx: &Ctx, handle: &RemoteHandle) -> ProviderResult<PollStatus> {
        let v = checked(
            get_json(
                ctx.http
                    .get(format!("{API}/predictions/{}/result", handle.id))
                    .bearer_auth(ctx.api_key()?),
            )
            .await?,
        )?;
        let kind = handle
            .data
            .get("kind")
            .and_then(|v| serde_json::from_value(v.clone()).ok())
            .unwrap_or(MediaKind::Image);
        parse_result(&v, kind)
    }
    fn poll_interval(&self, attempt: u32) -> Duration {
        Duration::from_secs((2 + attempt as u64).min(10))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn reads_model_schema_and_result() {
        let v = json!({"data":[{"model_id":"wavespeed-ai/z-image/turbo","type":"text-to-image","api_schema":{"api_schemas":[{"type":"model_run","request_schema":{"type":"object","properties":{"prompt":{"type":"string"},"size":{"type":"string","enum":["1024*1024"]}}}}]}}]});
        let m = catalog(&v);
        assert_eq!(m.len(), 1);
        assert!(m[0].params.iter().any(|p| p.key == "size"));
        let result =
            json!({"data":{"status":"completed","outputs":["https://example.test/image.png"]}});
        assert!(matches!(
            parse_result(&result, MediaKind::Image).unwrap(),
            PollStatus::Done(_)
        ));
        let inline =
            json!({"data":{"status":"completed","outputs":["data:image/png;base64,YQ=="]}});
        assert!(
            matches!(parse_result(&inline,MediaKind::Image).unwrap(),PollStatus::Done(v) if matches!(&v[0],Output::Bytes{data,..} if data==b"a"))
        );
    }
    #[test]
    fn static_models_map_documented_size_and_steps() {
        let req:GenerationRequest=serde_json::from_value(json!({"job_id":"x","model":"x","prompt":"sea","params":{"aspect_ratio":"9:16","steps":20},"num_outputs":1})).unwrap();
        let catalog = WaveSpeed.models();
        let z = body(&catalog[0], &req);
        let hunyuan = body(&catalog[1], &req);
        assert_eq!(z["size"], "576*1024");
        assert_eq!(hunyuan["size"], "720*1280");
        assert_eq!(hunyuan["num_inference_steps"], 20);
        assert!(hunyuan.get("duration").is_none());
    }
}
