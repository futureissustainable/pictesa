use async_trait::async_trait;
use base64::Engine;
use pictesa_core::{
    keys, CredentialField, Ctx, GenerationRequest, InputRole, InputSlot, MediaKind, ModelSpec,
    Output, ParamSpec, PollStatus, Pricing, Provider, ProviderError, ProviderInfo, ProviderKind,
    ProviderResult, RemoteHandle, Submission, Task,
};
use reqwest::multipart::{Form, Part};
use serde_json::{json, Value};

use crate::util::{failure, get_json, net_error, send_json};

pub struct OpenAI;
const API: &str = "https://api.openai.com/v1";

fn image_spec(id: &str) -> ModelSpec {
    let mut m = ModelSpec::new(
        "openai",
        id,
        id,
        MediaKind::Image,
        &[Task::TextToImage, Task::ImageEdit],
    );
    m.vendor = Some("OpenAI".into());
    m.inputs = vec![
        InputSlot::new(InputRole::Reference, MediaKind::Image, 16),
        InputSlot::new(InputRole::Mask, MediaKind::Image, 1),
    ];
    m.params = vec![
        ParamSpec::choices(
            keys::ASPECT_RATIO,
            "Aspect ratio",
            if id.starts_with("gpt-image-2") {
                &["1:1", "3:2", "2:3", "16:9", "9:16"]
            } else {
                &["1:1", "3:2", "2:3"]
            },
        ),
        ParamSpec::choices(
            keys::QUALITY,
            "Quality",
            if id.contains("2.5") {
                &["low", "medium", "high", "xhigh", "max"]
            } else {
                &["low", "medium", "high"]
            },
        ),
        ParamSpec::choices(keys::OUTPUT_FORMAT, "Format", &["png", "jpeg", "webp"]),
    ];
    m.max_outputs = 10;
    m.pricing = Some(Pricing::Variable {
        note: "OpenAI image token pricing".into(),
    });
    m.batch()
}
fn video_spec(id: &str) -> ModelSpec {
    let mut m = ModelSpec::new(
        "openai",
        id,
        id,
        MediaKind::Video,
        &[Task::TextToVideo, Task::ImageToVideo],
    );
    m.vendor = Some("OpenAI".into());
    m.inputs = vec![InputSlot::new(InputRole::StartFrame, MediaKind::Image, 1)];
    m.params = vec![
        ParamSpec::choices(keys::DURATION, "Duration", &["4", "8", "12", "16", "20"]),
        ParamSpec::choices(
            keys::RESOLUTION,
            "Size",
            if id == "sora-2" {
                &["1280x720", "720x1280"]
            } else {
                &["1280x720", "720x1280", "1920x1080", "1080x1920"]
            },
        ),
    ];
    m.pricing = Some(Pricing::Variable {
        note: "OpenAI Sora per-second pricing".into(),
    });
    m
}
fn image_size(req: &GenerationRequest) -> Option<String> {
    if let (Some(w), Some(h)) = (req.u64(keys::WIDTH), req.u64(keys::HEIGHT)) {
        return Some(format!("{w}x{h}"));
    }
    req.str(keys::ASPECT_RATIO).map(|r| {
        match r {
            "16:9" => "1536x864",
            "9:16" => "864x1536",
            "3:2" => "1536x1024",
            "2:3" => "1024x1536",
            _ => "1024x1024",
        }
        .into()
    })
}
fn image_body(model: &ModelSpec, req: &GenerationRequest) -> Value {
    let mut body = json!({"model":model.remote_id(),"prompt":req.prompt,"n":req.num_outputs});
    if let Some(size) = image_size(req) {
        body["size"] = json!(size);
    }
    let obj = body.as_object_mut().unwrap();
    req.copy_params(
        obj,
        &[
            (keys::QUALITY, "quality"),
            (keys::OUTPUT_FORMAT, "output_format"),
        ],
    );
    body
}
fn image_outputs(v: &Value, requested_format: Option<&str>) -> ProviderResult<Vec<Output>> {
    let mut out = Vec::new();
    let mime = match v
        .get("output_format")
        .and_then(Value::as_str)
        .or(requested_format)
        .unwrap_or("png")
    {
        "jpeg" => "image/jpeg",
        "webp" => "image/webp",
        _ => "image/png",
    };
    for item in v
        .get("data")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
    {
        if let Some(b) = item.get("b64_json").and_then(Value::as_str) {
            out.push(Output::Bytes {
                data: base64::engine::general_purpose::STANDARD
                    .decode(b)
                    .map_err(|e| ProviderError::Failed(format!("invalid image data: {e}")))?,
                mime: mime.into(),
            });
        } else if let Some(url) = item.get("url").and_then(Value::as_str) {
            out.push(Output::url(url));
        }
    }
    if out.is_empty() {
        Err(ProviderError::Failed("OpenAI returned no image".into()))
    } else {
        Ok(out)
    }
}

async fn image_request(
    model: &ModelSpec,
    req: &GenerationRequest,
) -> ProviderResult<(String, Value)> {
    let mut body = image_body(model, req);
    let refs: Vec<_> = req.inputs(InputRole::Reference).collect();
    let endpoint = if refs.is_empty() {
        "/images/generations"
    } else {
        "/images/edits"
    };
    if !refs.is_empty() {
        let mut images = Vec::new();
        for input in refs {
            // Hosted file links expire before the batch's 24-hour processing window.
            let url = if model.is_batch(&req.params) {
                input.data_uri().await?
            } else {
                input.url_or_data_uri().await?
            };
            images.push(json!({"image_url":url}));
        }
        body["images"] = json!(images);
        if let Some(mask) = req.input(InputRole::Mask) {
            // Converted to OpenAI's convention, so it always travels inline.
            let png = alpha_mask(&mask.bytes().await?)?;
            let uri = format!("data:image/png;base64,{}", base64::engine::general_purpose::STANDARD.encode(png));
            body["mask"] = json!({"image_url":uri});
        }
    } else if req.input(InputRole::Mask).is_some() {
        return Err(ProviderError::InvalidInput(
            "mask needs a reference image".into(),
        ));
    }
    Ok((endpoint.into(), body))
}

// https://developers.openai.com/api/docs/guides/batch
fn batch_line(job: &str, endpoint: &str, body: Value) -> Value {
    json!({"custom_id":job,"method":"POST","url":format!("/v1{endpoint}"),"body":body})
}

fn batch_body(file: &str, endpoint: &str) -> Value {
    json!({"input_file_id":file,"endpoint":format!("/v1{endpoint}"),"completion_window":"24h"})
}

fn batch_handle(
    v: &Value,
    req: &GenerationRequest,
    input_file: &str,
) -> ProviderResult<RemoteHandle> {
    let id = v["id"]
        .as_str()
        .ok_or_else(|| failure("OpenAI returned no batch id"))?;
    Ok(RemoteHandle::with(
        id,
        json!({"batch":true,"custom_id":req.job_id,
        "input_file_id":input_file,"output_format":req.str(keys::OUTPUT_FORMAT)}),
    ))
}

fn batch_outputs(text: &str, handle: &RemoteHandle) -> ProviderResult<Vec<Output>> {
    for line in text.lines().filter(|s| !s.trim().is_empty()) {
        let v: Value = serde_json::from_str(line)
            .map_err(|_| failure("OpenAI returned invalid batch JSONL"))?;
        if v["custom_id"].as_str() != handle.data_str("custom_id") {
            continue;
        }
        if let Some(message) = v.pointer("/error/message").and_then(Value::as_str) {
            return Err(failure(message));
        }
        let status = v
            .pointer("/response/status_code")
            .and_then(Value::as_u64)
            .ok_or_else(|| failure("OpenAI batch response has no status code"))?;
        let body = &v["response"]["body"];
        if !(200..300).contains(&status) {
            return Err(ProviderError::from_http(
                status as u16,
                &body.to_string(),
                None,
            ));
        }
        return image_outputs(body, handle.data_str("output_format"));
    }
    Err(failure("OpenAI batch has no result for this job"))
}

async fn batch_file(ctx: &Ctx, id: &str) -> ProviderResult<String> {
    let resp = ctx
        .http
        .get(format!("{API}/files/{id}/content"))
        .timeout(crate::util::API_TIMEOUT)
        .bearer_auth(ctx.api_key()?)
        .send()
        .await
        .map_err(net_error)?;
    let status = resp.status();
    let text = resp.text().await.map_err(net_error)?;
    if !status.is_success() {
        return Err(ProviderError::from_http(status.as_u16(), &text, None));
    }
    Ok(text)
}

async fn poll_batch(ctx: &Ctx, handle: &RemoteHandle) -> ProviderResult<PollStatus> {
    let v = get_json(
        ctx.http
            .get(format!("{API}/batches/{}", handle.id))
            .bearer_auth(ctx.api_key()?),
    )
    .await?;
    match v["status"].as_str().unwrap_or("") {
        "validating" | "in_progress" | "finalizing" | "cancelling" => Ok(PollStatus::Running {
            progress: None,
            message: Some("Batch processing (up to 24 hours)".into()),
        }),
        "completed" | "failed" | "expired" | "cancelled" => {
            if let Some(file) = v["output_file_id"].as_str() {
                return Ok(PollStatus::Done(batch_outputs(
                    &batch_file(ctx, file).await?,
                    handle,
                )?));
            }
            if let Some(file) = v["error_file_id"].as_str() {
                return match batch_outputs(&batch_file(ctx, file).await?, handle) {
                    Ok(out) => Ok(PollStatus::Done(out)),
                    Err(e) => Ok(PollStatus::Failed(e)),
                };
            }
            let message = v
                .pointer("/errors/data/0/message")
                .and_then(Value::as_str)
                .unwrap_or("OpenAI batch ended without images");
            if v["status"] == "failed" {
                Ok(PollStatus::Failed(failure(message)))
            } else {
                Err(failure(message))
            }
        }
        other => Err(failure(&format!("Unknown OpenAI batch state: {other}"))),
    }
}

#[async_trait]
impl Provider for OpenAI {
    fn info(&self) -> ProviderInfo {
        ProviderInfo {
            id: "openai".into(),
            name: "OpenAI".into(),
            kind: ProviderKind::Cloud,
            fields: vec![CredentialField::api_key(&["OPENAI_API_KEY"])],
            signup_url: Some("https://platform.openai.com/api-keys".into()),
            docs_url: Some("https://developers.openai.com/api/docs/guides/image-generation".into()),
            max_concurrency: 4,
        }
    }
    fn models(&self) -> Vec<ModelSpec> {
        [
            "gpt-image-2.5-sunburst",
            "gpt-image-2.5-flare",
            "gpt-image-2",
        ]
        .into_iter()
        .map(image_spec)
        .chain(["sora-2", "sora-2-pro"].into_iter().map(video_spec))
        .collect()
    }
    async fn discover(&self, ctx: &Ctx) -> ProviderResult<Vec<ModelSpec>> {
        let v = get_json(
            ctx.http
                .get(format!("{API}/models"))
                .bearer_auth(ctx.api_key()?),
        )
        .await?;
        Ok(v.get("data")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .filter_map(|x| x.get("id").and_then(Value::as_str))
            .filter_map(|id| {
                if id.starts_with("gpt-image-") && !id.contains("2026-") && id != "gpt-image-1" {
                    Some(image_spec(id))
                } else if id == "sora-2" || id == "sora-2-pro" {
                    Some(video_spec(id))
                } else {
                    None
                }
            })
            .collect())
    }
    async fn check(&self, ctx: &Ctx) -> ProviderResult<()> {
        get_json(
            ctx.http
                .get(format!("{API}/models"))
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
        let key = ctx.api_key()?;
        if model.output == MediaKind::Image {
            let (endpoint, body) = image_request(model, req).await?;
            if model.is_batch(&req.params) {
                let line = batch_line(&req.job_id, &endpoint, body);
                let file = Part::text(format!("{line}\n"))
                    .file_name("pictesa-batch.jsonl")
                    .mime_str("application/jsonl")
                    .map_err(|e| ProviderError::InvalidInput(e.to_string()))?;
                let uploaded = get_json(
                    ctx.http
                        .post(format!("{API}/files"))
                        .bearer_auth(key)
                        .multipart(Form::new().text("purpose", "batch").part("file", file)),
                )
                .await?;
                let file_id = uploaded["id"]
                    .as_str()
                    .ok_or_else(|| failure("OpenAI returned no batch input file id"))?;
                let v = send_json(
                    ctx.http.post(format!("{API}/batches")).bearer_auth(key),
                    &batch_body(file_id, &endpoint),
                )
                .await?;
                return Ok(Submission::Pending(batch_handle(&v, req, file_id)?));
            }
            let v = send_json(
                ctx.http.post(format!("{API}{endpoint}")).bearer_auth(key),
                &body,
            )
            .await?;
            Ok(Submission::Done(image_outputs(
                &v,
                req.str(keys::OUTPUT_FORMAT),
            )?))
        } else {
            let mut form = Form::new()
                .text("model", model.remote_id().to_string())
                .text("prompt", req.prompt.clone());
            if let Some(seconds) = req
                .str(keys::DURATION)
                .map(String::from)
                .or_else(|| req.i64(keys::DURATION).map(|n| n.to_string()))
            {
                form = form.text("seconds", seconds);
            }
            if let Some(size) = req.str(keys::RESOLUTION) {
                form = form.text("size", size.to_string());
            }
            if let Some(input) = req.input(InputRole::StartFrame) {
                let part = Part::bytes(input.bytes().await?)
                    .file_name(input.file_name())
                    .mime_str(&input.mime)
                    .map_err(|e| ProviderError::InvalidInput(e.to_string()))?;
                form = form.part("input_reference", part);
            }
            let v = get_json(
                ctx.http
                    .post(format!("{API}/videos"))
                    .bearer_auth(key)
                    .multipart(form),
            )
            .await?;
            let id = v
                .get("id")
                .and_then(Value::as_str)
                .ok_or_else(|| ProviderError::Failed("OpenAI returned no video id".into()))?;
            Ok(Submission::Pending(RemoteHandle::new(id)))
        }
    }
    async fn poll(&self, ctx: &Ctx, handle: &RemoteHandle) -> ProviderResult<PollStatus> {
        if handle.data.get("batch").and_then(Value::as_bool) == Some(true) {
            return poll_batch(ctx, handle).await;
        }
        let v = get_json(
            ctx.http
                .get(format!("{API}/videos/{}", handle.id))
                .bearer_auth(ctx.api_key()?),
        )
        .await?;
        match v.get("status").and_then(Value::as_str).unwrap_or("") {
            "queued" => Ok(PollStatus::Queued { position: None }),
            "in_progress" => Ok(PollStatus::Running {
                progress: v
                    .get("progress")
                    .and_then(Value::as_f64)
                    .map(|n| (n / 100.0) as f32),
                message: None,
            }),
            "completed" => Ok(PollStatus::Done(vec![Output::Url {
                url: format!("{API}/videos/{}/content", handle.id),
                headers: vec![("Authorization".into(), format!("Bearer {}", ctx.api_key()?))],
            }])),
            "failed" => Ok(PollStatus::Failed(failure(
                v.pointer("/error/message")
                    .and_then(Value::as_str)
                    .unwrap_or("OpenAI video failed"),
            ))),
            other => Err(ProviderError::Failed(format!(
                "unknown OpenAI video state: {other}"
            ))),
        }
    }
    async fn cancel(&self, ctx: &Ctx, handle: &RemoteHandle) -> ProviderResult<()> {
        let resource = if handle.data.get("batch").and_then(Value::as_bool) == Some(true) {
            "batches"
        } else {
            "videos"
        };
        let r = ctx
            .http
            .post(format!("{API}/{resource}/{}/cancel", handle.id))
            .bearer_auth(ctx.api_key()?)
            .send()
            .await
            .map_err(net_error)?;
        pictesa_core::json_response(r).await?;
        Ok(())
    }
}

/// OpenAI edits the mask's transparent pixels; Pictesa masks are white where to change. Masks that
/// already carry transparency are taken as OpenAI-style and sent as they are.
fn alpha_mask(bytes: &[u8]) -> ProviderResult<Vec<u8>> {
    let img = image::load_from_memory(bytes).map_err(|e| ProviderError::InvalidInput(format!("the mask is not a readable image: {e}")))?;
    let has_transparency = img.color().has_alpha() && img.to_rgba8().pixels().any(|p| p.0[3] < 255);
    let out = if has_transparency {
        img.to_rgba8()
    } else {
        let gray = img.to_luma8();
        image::RgbaImage::from_fn(gray.width(), gray.height(), |x, y| image::Rgba([0, 0, 0, if gray.get_pixel(x, y).0[0] >= 128 { 0 } else { 255 }]))
    };
    let mut png = std::io::Cursor::new(Vec::new());
    out.write_to(&mut png, image::ImageFormat::Png).map_err(|e| ProviderError::InvalidInput(format!("could not prepare the mask: {e}")))?;
    Ok(png.into_inner())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn white_mask_areas_become_transparent_for_openai() {
        let mut mask = image::GrayImage::new(4, 2);
        mask.put_pixel(1, 0, image::Luma([255]));
        let mut png = std::io::Cursor::new(Vec::new());
        mask.write_to(&mut png, image::ImageFormat::Png).unwrap();
        let out = image::load_from_memory(&alpha_mask(png.get_ref()).unwrap()).unwrap().to_rgba8();
        assert_eq!(out.get_pixel(1, 0).0[3], 0, "painted: edit here");
        assert_eq!(out.get_pixel(0, 0).0[3], 255, "unpainted: keep");
    }
    #[test]
    fn parses_recorded_image() {
        let v = json!({"data":[{"b64_json":"YQ=="}]});
        assert!(matches!(&image_outputs(&v,None).unwrap()[0],Output::Bytes{data,..} if data==b"a"));
        assert!(
            matches!(&image_outputs(&v,Some("jpeg")).unwrap()[0],Output::Bytes{mime,..} if mime=="image/jpeg")
        );
    }
    #[test]
    fn maps_size_and_quality() {
        let mut params = serde_json::Map::new();
        params.insert(keys::ASPECT_RATIO.into(), json!("16:9"));
        params.insert(keys::QUALITY.into(), json!("max"));
        let req = GenerationRequest {
            job_id: "t".into(),
            model: "gpt-image-2.5-sunburst".into(),
            prompt: "draw".into(),
            negative_prompt: None,
            params,
            inputs: vec![],
            num_outputs: 2,
        };
        let body = image_body(&image_spec("gpt-image-2.5-sunburst"), &req);
        assert_eq!(body["size"], "1536x864");
        assert_eq!(body["quality"], "max");
        assert_eq!(body["n"], 2);
    }
    #[tokio::test]
    async fn batch_requests_use_documented_jsonl_for_generations_and_edits() {
        let model = image_spec("gpt-image-2.5-sunburst");
        let mut req = GenerationRequest {
            job_id: "job-1".into(),
            model: model.id.clone(),
            prompt: "draw".into(),
            negative_prompt: None,
            params: serde_json::from_value(json!({"speed":"batch","output_format":"webp"}))
                .unwrap(),
            inputs: vec![],
            num_outputs: 2,
        };
        let (endpoint, body) = image_request(&model, &req).await.unwrap();
        assert_eq!(
            batch_line(&req.job_id, &endpoint, body),
            json!({
                "custom_id":"job-1","method":"POST","url":"/v1/images/generations",
                "body":{"model":"gpt-image-2.5-sunburst","prompt":"draw","n":2,"output_format":"webp"}
            })
        );
        let path = std::env::temp_dir().join(format!("pictesa-openai-batch-{}", ulid::Ulid::new()));
        tokio::fs::write(&path, b"a").await.unwrap();
        req.inputs = vec![pictesa_core::MediaInput {
            role: InputRole::Reference,
            kind: MediaKind::Image,
            mime: "image/png".into(),
            path: path.clone(),
            url: Some("https://example.com/ref.png".into()),
            width: None,
            height: None,
        }];
        let (endpoint, body) = image_request(&model, &req).await.unwrap();
        assert_eq!(
            batch_line(&req.job_id, &endpoint, body)["url"],
            "/v1/images/edits"
        );
        assert_eq!(
            batch_body("file-1", &endpoint),
            json!({"input_file_id":"file-1","endpoint":"/v1/images/edits","completion_window":"24h"})
        );
        let (_, body) = image_request(&model, &req).await.unwrap();
        assert_eq!(
            body["images"],
            json!([{"image_url":"data:image/png;base64,YQ=="}])
        );
        // Masks are converted to OpenAI's transparent-means-edit form, so they always go inline.
        let mask_path = path.with_extension("mask.png");
        image::GrayImage::from_pixel(2, 1, image::Luma([255])).save(&mask_path).unwrap();
        let mut mask = req.inputs[0].clone();
        mask.role = InputRole::Mask;
        mask.path = mask_path.clone();
        req.inputs.push(mask);
        let (_, body) = image_request(&model, &req).await.unwrap();
        assert!(body["mask"]["image_url"].as_str().unwrap().starts_with("data:image/png;base64,"));
        req.params.insert(keys::SPEED.into(), json!("standard"));
        let (_, body) = image_request(&model, &req).await.unwrap();
        assert_eq!(
            body["images"],
            json!([{"image_url":"https://example.com/ref.png"}])
        );
        tokio::fs::remove_file(&path).await.unwrap();
        tokio::fs::remove_file(&mask_path).await.unwrap();
        let h = batch_handle(&json!({"id":"batch-1"}), &req, "file-1").unwrap();
        let h: RemoteHandle = serde_json::from_str(&serde_json::to_string(&h).unwrap()).unwrap();
        let text = concat!(
            r#"{"custom_id":"other","response":{"status_code":200,"body":{}}}"#,
            "\n",
            r#"{"custom_id":"job-1","response":{"status_code":200,"body":{"data":[{"b64_json":"YQ=="}]}},"error":null}"#
        );
        assert!(
            matches!(&batch_outputs(text, &h).unwrap()[0], Output::Bytes { data, mime } if data == b"a" && mime == "image/webp")
        );
        assert_eq!(
            OpenAI.poll_interval_for(&h, 0),
            std::time::Duration::from_secs(60)
        );
        for text in [
            r#"{"custom_id":"job-1","response":null,"error":{"code":"batch_expired","message":"expired"}}"#,
            r#"{"custom_id":"job-1","response":{"status_code":400,"body":{"error":{"message":"bad input"}}}}"#,
            "invalid json",
            "",
        ] {
            assert!(batch_outputs(text, &h).is_err());
        }
    }
}
