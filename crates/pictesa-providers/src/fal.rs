//! fal.ai: a gateway with 1,500+ models. Reference implementation for gateway providers.
//!
//! - Catalog: `GET https://api.fal.ai/v1/models` (paginated, light metadata).
//! - Schema: per-endpoint OpenAPI at `https://fal.ai/api/openapi/queue/openapi.json?endpoint_id=...`.
//! - Runs: queue API at `https://queue.fal.run/{endpoint}`; status and result URLs come back on submit.

use std::collections::HashMap;

use async_trait::async_trait;
use pictesa_core::{
    collect_media_urls, schema, CredentialField, Ctx, GenerationRequest, InputRole, MediaInput,
    MediaKind, ModelSpec, Output, PollStatus, Pricing, Provider, ProviderError, ProviderInfo,
    ProviderKind, ProviderResult, RemoteHandle, Submission, Task,
};
use serde_json::{json, Map, Value};

use crate::util::{
    error_message, failure, file_size, get_json, net_error, send_json, INLINE_LIMIT,
};

pub struct Fal;

const CATALOG: &str = "https://api.fal.ai/v1/models";
const QUEUE: &str = "https://queue.fal.run";
const OPENAPI: &str = "https://fal.ai/api/openapi/queue/openapi.json";
/// https://fal.ai/docs/documentation/model-apis/file-access-controls (two-step upload).
const UPLOAD: &str = "https://rest.fal.ai/storage/upload/initiate?storage_type=fal-cdn-v3";

/// Families whose newest endpoint is tagged `featured` in the picker.
const FAMILIES: &[&str] = &[
    "kling",
    "veo",
    "sora",
    "seedance",
    "wan",
    "ltx",
    "hailuo",
    "minimax",
    "nano-banana",
    "gpt-image",
    "flux",
    "seedream",
    "imagen",
    "ideogram",
    "recraft",
    "qwen-image",
    "hunyuan",
    "luma",
    "ray",
    "pika",
    "grok",
    "z-image",
    "runway",
    "topaz",
];

/// Endpoint words that mark specialised variants, never shown as featured.
const NICHE: &[&str] = &[
    "lora",
    "train",
    "styles/",
    "deblur",
    "clean-plate",
    "action",
    "so101",
    "vector",
    "layerize",
    "inpaint",
    "tiling",
    "foley",
    "tts",
    "speech",
    "music",
    "lip-sync",
    "lipsync",
    "reframe",
    "extend",
    "upscale",
    "draft",
    "outpaint",
    "remove",
    "relight",
    "mask",
    "depth",
    "pose",
    "canny",
    "control",
    "sketch",
    "redux",
    "fill",
    "erase",
    "segment",
    "face",
];

fn auth(ctx: &Ctx) -> Option<String> {
    ctx.get("api_key").map(|k| format!("Key {k}"))
}

fn category_kind(category: &str, endpoint: &str) -> Option<(MediaKind, Vec<Task>)> {
    let up = endpoint.contains("upscale")
        || endpoint.contains("upscaler")
        || endpoint.contains("clarity")
        || endpoint.contains("topaz");
    let lips = endpoint.contains("lipsync")
        || endpoint.contains("lip-sync")
        || endpoint.contains("sync-lipsync");
    Some(match category {
        "training" if pictesa_core::lora::family(endpoint,&Value::Null).is_some() => (MediaKind::Lora, vec![Task::Training]),
        "text-to-image" => (MediaKind::Image, vec![Task::TextToImage]),
        "image-to-image" if up => (MediaKind::Image, vec![Task::Upscale]),
        "image-to-image" => (MediaKind::Image, vec![Task::ImageEdit]),
        "text-to-video" => (MediaKind::Video, vec![Task::TextToVideo]),
        "image-to-video" => (MediaKind::Video, vec![Task::ImageToVideo]),
        "video-to-video" if up => (MediaKind::Video, vec![Task::Upscale]),
        "video-to-video" if lips => (MediaKind::Video, vec![Task::LipSync]),
        "video-to-video" => (MediaKind::Video, vec![Task::VideoToVideo]),
        "video-to-audio" => (MediaKind::Audio, vec![Task::VideoToAudio]),
        "audio-to-video" => (MediaKind::Video, vec![Task::LipSync]),
        "text-to-audio" | "text-to-speech" => (MediaKind::Audio, vec![Task::TextToAudio]),
        _ => return None,
    })
}

/// Best-effort maker name from the endpoint path.
pub fn vendor_for(endpoint: &str) -> Option<&'static str> {
    let e = endpoint.to_ascii_lowercase();
    let table: &[(&str, &str)] = &[
        ("kling", "Kuaishou"),
        ("veo", "Google"),
        ("imagen", "Google"),
        ("nano-banana", "Google"),
        ("gemini", "Google"),
        ("sora", "OpenAI"),
        ("gpt-image", "OpenAI"),
        ("seedance", "ByteDance"),
        ("seedream", "ByteDance"),
        ("bytedance", "ByteDance"),
        ("wan", "Alibaba"),
        ("qwen", "Alibaba"),
        ("ltx", "Lightricks"),
        ("hailuo", "MiniMax"),
        ("minimax", "MiniMax"),
        ("flux", "Black Forest Labs"),
        ("ideogram", "Ideogram"),
        ("recraft", "Recraft"),
        ("hunyuan", "Tencent"),
        ("luma", "Luma"),
        ("ray", "Luma"),
        ("pika", "Pika"),
        ("grok", "xAI"),
        ("runway", "Runway"),
        ("topaz", "Topaz Labs"),
        ("stable-diffusion", "Stability AI"),
        ("sdxl", "Stability AI"),
        ("z-image", "Alibaba Tongyi"),
    ];
    table.iter().find(|(k, _)| e.contains(k)).map(|(_, v)| *v)
}

fn pricing_from(v: &Value) -> Option<Pricing> {
    price_of(v.get("prices")?.as_array()?.first()?)
}

fn price_of(p: &Value) -> Option<Pricing> {
    let usd = p.get("unit_price")?.as_f64()?;
    let unit = p
        .get("unit")
        .and_then(Value::as_str)
        .unwrap_or("")
        .to_ascii_lowercase();
    Some(if unit == "step" || unit == "steps" {
        Pricing::PerStep { usd }
    } else if unit == "training" || unit == "trainings" || unit == "run" || unit == "runs" {
        Pricing::PerRun { usd }
    } else if unit.starts_with("second") {
        Pricing::PerSecond { usd }
    } else if unit.starts_with("image") {
        Pricing::PerImage { usd }
    } else if unit.starts_with("video") {
        Pricing::PerVideo { usd }
    } else if unit.contains("megapixel") || unit == "mp" {
        Pricing::PerMegapixel { usd }
    } else if unit == "units" && usd == 1.0 {
        // Token-billed models come back as "$1 per units", a placeholder that says nothing.
        Pricing::Variable {
            note: "Priced by usage on fal".into(),
        }
    } else {
        // fal names units in the plural ("compute seconds", "credits"); "1000 characters" stays.
        let unit = if unit.starts_with(|c: char| c.is_ascii_digit()) { &unit } else { unit.strip_suffix('s').unwrap_or(&unit) };
        Pricing::Variable {
            note: format!("${usd} per {unit}"),
        }
    })
}

async fn upload(ctx: &Ctx, input: &MediaInput) -> ProviderResult<String> {
    if let Some(url) = &input.url {
        return Ok(url.clone());
    }
    if file_size(input) <= INLINE_LIMIT {
        return input.data_uri().await;
    }
    let Some(key) = auth(ctx) else {
        return input.data_uri().await;
    };
    let init = send_json(
        ctx.http.post(UPLOAD).header("Authorization", &key),
        &json!({ "content_type": input.mime, "file_name": input.file_name() }),
    )
    .await?;
    let (Some(put), Some(file_url)) = (
        init.get("upload_url").and_then(Value::as_str),
        init.get("file_url").and_then(Value::as_str),
    ) else {
        return input.data_uri().await;
    };
    let resp = ctx
        .http
        .put(put)
        .header("Content-Type", &input.mime)
        .body(input.bytes().await?)
        .send()
        .await
        .map_err(net_error)?;
    if !resp.status().is_success() {
        return Err(ProviderError::Transient(format!(
            "fal upload failed with HTTP {}",
            resp.status()
        )));
    }
    Ok(file_url.to_string())
}

#[async_trait]
impl Provider for Fal {
    fn info(&self) -> ProviderInfo {
        ProviderInfo {
            id: "fal".into(),
            name: "fal".into(),
            kind: ProviderKind::Gateway,
            fields: vec![CredentialField::api_key(&["FAL_KEY", "FAL_API_KEY"])],
            signup_url: Some("https://fal.ai/dashboard/keys".into()),
            docs_url: Some("https://docs.fal.ai".into()),
            max_concurrency: 8,
        }
    }

    fn models(&self) -> Vec<ModelSpec> {
        let mut m = ModelSpec::new("fal", "fal-ai/flux-lora-fast-training", "FLUX.1 LoRA fast training", MediaKind::Lora, &[Task::Training]);
        m.complete = false; m.prompt = false;
        vec![m]
    }

    async fn discover(&self, ctx: &Ctx) -> ProviderResult<Vec<ModelSpec>> {
        let mut out = Vec::new();
        let mut newest: HashMap<String, (String, usize)> = HashMap::new();
        let mut cursor: Option<String> = None;
        for _ in 0..40 {
            let mut url = format!("{CATALOG}?limit=100&status=active");
            if let Some(c) = &cursor {
                url.push_str("&cursor=");
                url.push_str(c);
            }
            let mut req = ctx.http.get(&url);
            if let Some(k) = auth(ctx) {
                req = req.header("Authorization", k);
            }
            let page = get_json(req).await?;
            for m in page
                .get("models")
                .and_then(Value::as_array)
                .into_iter()
                .flatten()
            {
                let Some(endpoint) = m.get("endpoint_id").and_then(Value::as_str) else {
                    continue;
                };
                let md = m.get("metadata").cloned().unwrap_or(Value::Null);
                let category = md.get("category").and_then(Value::as_str).unwrap_or("");
                let Some((output, tasks)) = category_kind(category, endpoint) else {
                    continue;
                };
                let name = md
                    .get("display_name")
                    .and_then(Value::as_str)
                    .unwrap_or(endpoint);
                let mut spec = ModelSpec::new("fal", endpoint, name, output, &tasks);
                spec.vendor = vendor_for(endpoint).map(String::from);
                spec.description = md
                    .get("description")
                    .and_then(Value::as_str)
                    .map(String::from);
                spec.complete = false;
                spec.tags.push(category.to_string());
                spec.meta = json!({
                    "category": category,
                    "thumbnail_url": md.get("thumbnail_url"),
                    "date": md.get("date"),
                    "group": md.pointer("/group/key"),
                    "license": md.get("license_type"),
                });
                let date = md
                    .get("date")
                    .and_then(Value::as_str)
                    .unwrap_or("")
                    .to_string();
                let lower = endpoint.to_ascii_lowercase();
                let core = matches!(
                    category,
                    "text-to-image" | "image-to-image" | "text-to-video" | "image-to-video"
                );
                let niche = NICHE.iter().any(|w| lower.contains(w));
                if let Some(family) = FAMILIES
                    .iter()
                    .find(|f| lower.contains(*f))
                    .filter(|_| core && !niche)
                {
                    let group = format!("{family}:{category}");
                    let idx = out.len();
                    match newest.get(&group) {
                        Some((d, _)) if *d >= date => {}
                        _ => {
                            newest.insert(group, (date, idx));
                        }
                    }
                }
                out.push(spec);
            }
            if page.get("has_more").and_then(Value::as_bool) != Some(true) {
                break;
            }
            cursor = page
                .get("next_cursor")
                .and_then(Value::as_str)
                .map(String::from);
            if cursor.is_none() {
                break;
            }
        }
        for (_, idx) in newest.values() {
            out[*idx].tags.push("featured".into());
        }
        if let Some(key) = auth(ctx) {
            attach_prices(ctx, &key, &mut out).await;
        }
        Ok(out)
    }

    async fn resolve(&self, ctx: &Ctx, model: &ModelSpec) -> ProviderResult<ModelSpec> {
        let endpoint = model.remote_id();
        let doc = get_json(ctx.http.get(OPENAPI).query(&[("endpoint_id", endpoint)])).await?;
        let schemas = doc
            .pointer("/components/schemas")
            .cloned()
            .unwrap_or(Value::Null);
        let input_ref = doc
            .pointer(&format!(
                "/paths/{}/post/requestBody/content/application~1json/schema",
                json_pointer_escape(&format!("/{endpoint}"))
            ))
            .cloned()
            .or_else(|| {
                schemas.as_object().and_then(|m| {
                    m.iter()
                        .find(|(k, _)| k.ends_with("Input"))
                        .map(|(_, v)| v.clone())
                })
            })
            .ok_or_else(|| {
                ProviderError::NotSupported(format!("fal published no input schema for {endpoint}"))
            })?;
        let mut spec = resolve_schema(model, &doc, &input_ref, &schemas)?;
        if spec.pricing.is_none() {
            if let Some(key) = auth(ctx) {
                let req = ctx
                    .http
                    .get(format!("{CATALOG}/pricing"))
                    .query(&[("endpoint_id", endpoint)])
                    .header("Authorization", key);
                if let Ok(p) = get_json(req).await {
                    spec.pricing = pricing_from(&p);
                }
            }
        }
        if spec.inputs.iter().any(|i| i.role == InputRole::StartFrame)
            && !spec.tasks.contains(&Task::ImageToVideo)
        {
            spec.tasks.push(Task::ImageToVideo);
        }
        spec.complete = true;
        Ok(spec)
    }

    async fn check(&self, ctx: &Ctx) -> ProviderResult<()> {
        let key = auth(ctx).ok_or_else(|| ProviderError::Auth("missing api_key".into()))?;
        get_json(
            ctx.http
                .get(format!("{CATALOG}/pricing"))
                .query(&[("endpoint_id", "fal-ai/flux/dev")])
                .header("Authorization", key),
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
        let key = auth(ctx).ok_or_else(|| {
            ProviderError::Auth("missing api_key; add a fal key in Settings".into())
        })?;
        let bindings = model.meta.get("bindings").cloned().unwrap_or(Value::Null);
        let mut body: Map<String, Value> = schema::body_from_bindings(&bindings, req);
        if model.output == MediaKind::Lora {
            let temp = crate::training::Temp::new(ctx)?;
            let archive = crate::training::archive(req, &temp.0).await?;
            let zip_url = upload_archive(ctx, &archive).await?;
            body = training_body(model, req, &zip_url)?;
        } else if let Some(loras) = req.params.get("loras").and_then(Value::as_array) {
            let mut attached = Vec::new();
            for lora in loras {
                let path = lora.get("path").and_then(Value::as_str).ok_or_else(||ProviderError::InvalidInput("Missing LoRA file.".into()))?;
                let url = match lora.get("url").and_then(Value::as_str) {
                    Some(url)=>url.to_string(),
                    None=>upload_archive(ctx,std::path::Path::new(path)).await?,
                };
                attached.push(lora_binding(&url,lora.get("strength").and_then(Value::as_f64).unwrap_or(1.0)));
            }
            body.insert("loras".into(), json!(attached));
        }
        for (role, field, array) in schema::input_bindings(&bindings) {
            let mut urls = Vec::new();
            for input in req.inputs(role) {
                urls.push(upload(ctx, input).await?);
            }
            if urls.is_empty() {
                continue;
            }
            body.insert(field, if array { json!(urls) } else { json!(urls[0]) });
        }
        let answer = send_json(
            ctx.http
                .post(format!("{QUEUE}/{}", model.remote_id()))
                .header("Authorization", key),
            &Value::Object(body),
        )
        .await?;
        let id = answer
            .get("request_id")
            .and_then(Value::as_str)
            .ok_or_else(|| {
                ProviderError::Failed(format!("fal returned no request id: {answer}"))
            })?;
        Ok(Submission::Pending(RemoteHandle::with(
            id,
            json!({
                "status_url": answer.get("status_url"),
                "response_url": answer.get("response_url"),
                "cancel_url": answer.get("cancel_url"),
                "output": model.output,
                "lora_output": model.meta.get("lora_output"),
            }),
        )))
    }

    async fn poll(&self, ctx: &Ctx, handle: &RemoteHandle) -> ProviderResult<PollStatus> {
        let key = auth(ctx).ok_or_else(|| ProviderError::Auth("missing api_key".into()))?;
        let status_url = handle
            .data_str("status_url")
            .ok_or_else(|| ProviderError::Failed("job has no status URL".into()))?;
        let status = get_json(ctx.http.get(status_url).query(&[("logs", "1")]).header("Authorization", &key)).await?;
        match status.get("status").and_then(Value::as_str).unwrap_or("") {
            "IN_QUEUE" => Ok(PollStatus::Queued {
                position: status
                    .get("queue_position")
                    .and_then(Value::as_u64)
                    .map(|p| p as u32),
            }),
            "IN_PROGRESS" => {
                let log = status.get("logs").and_then(Value::as_array).and_then(|logs|logs.last()).and_then(|l|l.get("message")).and_then(Value::as_str);
                Ok(PollStatus::Running { progress:log.and_then(pictesa_core::lora::progress), message:log.map(|l|l.chars().take(300).collect()) })
            },
            "COMPLETED" => {
                if let Some(err) = status.get("error").and_then(Value::as_str) {
                    return Ok(PollStatus::Failed(failure(err)));
                }
                let response_url = handle
                    .data_str("response_url")
                    .ok_or_else(|| ProviderError::Failed("job has no result URL".into()))?;
                let resp = ctx
                    .http
                    .get(response_url)
                    .header("Authorization", &key)
                    .send()
                    .await
                    .map_err(net_error)?;
                let code = resp.status();
                let text = resp.text().await.map_err(net_error)?;
                let body: Value =
                    serde_json::from_str(&text).unwrap_or(Value::String(text.clone()));
                if !code.is_success() {
                    let msg = error_message(&body)
                        .unwrap_or_else(|| pictesa_core::error::truncate(&text, 400));
                    return Ok(PollStatus::Failed(
                        match ProviderError::from_http(code.as_u16(), &msg, None) {
                            ProviderError::InvalidInput(m) => failure(m),
                            other => other,
                        },
                    ));
                }
                let want = handle
                    .data
                    .get("output")
                    .and_then(|o| serde_json::from_value::<MediaKind>(o.clone()).ok());
                if want == Some(MediaKind::Lora) {
                    let key = handle.data_str("lora_output").ok_or_else(||ProviderError::Failed("Missing trainer output schema.".into()))?;
                    return Ok(match training_output(&body, key) {
                        Ok(url)=>PollStatus::Done(vec![Output::url(url)]),
                        Err(e)=>PollStatus::Failed(e),
                    });
                }
                let urls = collect_media_urls(&body, want);
                if urls.is_empty() {
                    return Ok(PollStatus::Failed(ProviderError::Failed(format!(
                        "fal finished without media: {}",
                        pictesa_core::error::truncate(&body.to_string(), 300)
                    ))));
                }
                Ok(PollStatus::Done(
                    urls.into_iter().map(Output::url).collect(),
                ))
            }
            other => Ok(PollStatus::Running {
                progress: None,
                message: Some(other.to_lowercase()),
            }),
        }
    }

    async fn cancel(&self, ctx: &Ctx, handle: &RemoteHandle) -> ProviderResult<()> {
        if let (Some(key), Some(url)) = (auth(ctx), handle.data_str("cancel_url")) {
            let _ = ctx.http.put(url).header("Authorization", key).send().await;
        }
        Ok(())
    }
}

/// Prices looked up in the last day (`None`: the pricing API has none), so a refresh only asks
/// about new models: a full pass takes minutes at the API's pace.
fn price_cache() -> &'static std::sync::Mutex<HashMap<String, (Option<Pricing>, std::time::Instant)>> {
    static CACHE: std::sync::OnceLock<std::sync::Mutex<HashMap<String, (Option<Pricing>, std::time::Instant)>>> = std::sync::OnceLock::new();
    CACHE.get_or_init(Default::default)
}

/// Fetches prices for the whole catalog in batches: the pricing API takes up to 50 ids per call
/// (more answers 400) and rate-limits per call, so one request per model would be throttled.
/// It answers 404 for a whole batch when any id in it has no price; such a batch is split in two
/// until the unpriced ids stand alone.
async fn attach_prices(ctx: &Ctx, key: &str, models: &mut [ModelSpec]) {
    const DAY: std::time::Duration = std::time::Duration::from_secs(24 * 3600);
    let mut prices: HashMap<String, Pricing> = HashMap::new();
    let mut ids: Vec<String> = Vec::new();
    if let Ok(cache) = price_cache().lock() {
        for m in models.iter() {
            match cache.get(m.remote_id()).filter(|(_, at)| at.elapsed() < DAY) {
                Some((price, _)) => prices.extend(price.clone().map(|p| (m.remote_id().to_string(), p))),
                None => ids.push(m.remote_id().to_string()),
            }
        }
    }
    let mut batches: Vec<&[String]> = ids.chunks(50).rev().collect();
    while let Some(batch) = batches.pop() {
        let query: Vec<(&str, &str)> = batch.iter().map(|id| ("endpoint_id", id.as_str())).collect();
        let (mut answer, mut unknown) = (None, false);
        for attempt in 0..5 {
            match get_json(ctx.http.get(format!("{CATALOG}/pricing")).query(&query).header("Authorization", key)).await {
                Ok(v) => {
                    answer = Some(v);
                    break;
                }
                Err(ProviderError::RateLimited { retry_after_ms }) => {
                    tokio::time::sleep(std::time::Duration::from_millis(retry_after_ms.unwrap_or(2000 * (attempt + 1)))).await;
                }
                Err(ProviderError::InvalidInput(d)) if d.starts_with("HTTP 404") => {
                    if batch.len() > 1 {
                        let (a, b) = batch.split_at(batch.len() / 2);
                        batches.push(b);
                        batches.push(a);
                    } else {
                        unknown = true;
                    }
                    break;
                }
                Err(e) => {
                    if batch.len() > 1 {
                        tracing::warn!("fal prices for {} models failed: {e}", batch.len());
                    }
                    break;
                }
            }
        }
        tokio::time::sleep(std::time::Duration::from_millis(1000)).await;
        let Some(v) = answer else {
            // A lone id the API does not know has no price: remember that for the day too.
            if let (true, Ok(mut cache)) = (unknown, price_cache().lock()) {
                cache.insert(batch[0].clone(), (None, std::time::Instant::now()));
            }
            continue;
        };
        let mut found: HashMap<String, Pricing> = HashMap::new();
        for p in v.get("prices").and_then(Value::as_array).into_iter().flatten() {
            if let (Some(id), Some(price)) = (p.get("endpoint_id").and_then(Value::as_str), price_of(p)) {
                found.insert(id.to_string(), price);
            }
        }
        if let Ok(mut cache) = price_cache().lock() {
            for id in batch {
                cache.insert(id.clone(), (found.get(id).cloned(), std::time::Instant::now()));
            }
        }
        prices.extend(found);
    }
    for m in models.iter_mut() {
        if let Some(p) = prices.remove(m.remote_id()) {
            m.pricing = Some(p);
        }
    }
}

fn json_pointer_escape(s: &str) -> String {
    s.replace('~', "~0").replace('/', "~1")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn video_audio_catalog_category_is_kept() {
        assert_eq!(category_kind("video-to-audio", "fal-ai/video-to-audio"), Some((MediaKind::Audio, vec![Task::VideoToAudio])));
    }

    /// Shapes returned by `GET /v1/models/pricing` on 25.10.2026.
    #[test]
    fn prices_read_like_the_pricing_api_answers() {
        let price = |unit_price: f64, unit: &str| price_of(&json!({ "endpoint_id": "x", "unit_price": unit_price, "unit": unit, "currency": "USD" })).unwrap();
        assert!(matches!(price(0.003, "megapixels"), Pricing::PerMegapixel { usd } if usd == 0.003));
        assert!(matches!(price(0.01625, "seconds"), Pricing::PerSecond { .. }));
        assert!(matches!(price(1.0, "units"), Pricing::Variable { note } if note == "Priced by usage on fal"));
        assert!(matches!(price(0.0214, "1000 tokens"), Pricing::Variable { note } if note == "$0.0214 per 1000 tokens"));
        assert!(matches!(price(0.00125, "compute seconds"), Pricing::Variable { note } if note == "$0.00125 per compute second"));
    }
}

fn deref<'a>(node: &'a Value, schemas: &'a Value) -> &'a Value {
    node.get("$ref").and_then(Value::as_str).and_then(|r|r.strip_prefix("#/components/schemas/")).and_then(|k|schemas.get(k)).unwrap_or(node)
}

/// fal publishes preset-or-custom fields as `anyOf: [{"$ref": ImageSize}, {"type": "string", "enum": [...]}]`
/// (`image_size` in the queue OpenAPI of `fal-ai/flux/dev`). The shared reader takes the first branch,
/// an object it cannot offer, so the field would vanish; the preset branch is offered instead.
fn prefer_presets(input: &Value, schemas: &Value) -> Value {
    let mut input = deref(input, schemas).clone();
    if let Some(props) = input.get_mut("properties").and_then(Value::as_object_mut) {
        for field in props.values_mut() {
            let presets = field.get("anyOf").and_then(Value::as_array).and_then(|alts| alts.iter().map(|a| deref(a, schemas)).find(|a| a.get("enum").is_some())).cloned();
            if let (Some(Value::Object(presets)), Some(field)) = (presets, field.as_object_mut()) {
                field.remove("anyOf");
                for (k, v) in presets {
                    field.entry(k).or_insert(v);
                }
            }
        }
    }
    input
}

fn resolve_schema(model: &ModelSpec, doc: &Value, input: &Value, schemas: &Value) -> ProviderResult<ModelSpec> {
    let read = schema::read_input_schema(&prefer_presets(input, schemas), Some(schemas), model.output);
    let mut spec=model.clone();
    spec.params=read.params;spec.inputs=read.inputs;spec.prompt=read.has_prompt;spec.negative_prompt=read.has_negative_prompt;spec.max_outputs=read.max_outputs;
    spec.meta["bindings"]=read.bindings;
    let properties=deref(input,schemas).get("properties").cloned().unwrap_or(json!({}));
    if properties.get("loras").is_some_and(|p|deref(p,schemas).get("type").and_then(Value::as_str)==Some("array")) {
        spec.meta["supports_loras"]=json!(true);
        spec.meta["lora_family"]=json!(pictesa_core::lora::family(&spec.id,&spec.meta));
    }
    if model.output==MediaKind::Lora {
        let input_schema=deref(input,schemas);
        let props=properties.as_object().ok_or_else(||ProviderError::NotSupported("Trainer has no input properties.".into()))?;
        let field=["images_data_url","image_data_url","training_data_url"].into_iter().find(|k|props.contains_key(*k))
            .ok_or_else(||ProviderError::NotSupported("This trainer does not accept an image ZIP URL.".into()))?;
        let output=doc.get("paths").and_then(|paths|paths.get(format!("/{}/requests/{{request_id}}",model.remote_id()))).and_then(|p|p.pointer("/get/responses/200/content/application~1json/schema"))
            .map(|s|deref(s,schemas))
            .or_else(||schemas.as_object().and_then(|m|m.iter().find(|(k,_)|k.ends_with("Output")).map(|(_,v)|v)))
            .ok_or_else(||ProviderError::NotSupported("Trainer has no output schema.".into()))?;
        let output_key=output.get("properties").and_then(Value::as_object).and_then(|p|p.keys().find(|k|k.contains("lora") && k.ends_with("file")))
            .ok_or_else(||ProviderError::NotSupported("Trainer does not publish a LoRA weights file output.".into()))?;
        spec.meta["training_schema"]=input_schema.clone();
        spec.meta["training_zip_field"]=json!(field);
        spec.meta["lora_output"]=json!(output_key);
        spec.meta["lora_family"]=json!(pictesa_core::lora::family(&model.id,&model.meta));
        spec.params.clear();spec.prompt=false;spec.max_outputs=1;
        spec.meta["bindings"]=json!({});
        spec.inputs=vec![pictesa_core::InputSlot {min:3,..pictesa_core::InputSlot::new(InputRole::Reference,MediaKind::Image,100)}];
    }
    Ok(spec)
}

pub fn training_body(model: &ModelSpec, req: &GenerationRequest, zip_url: &str) -> ProviderResult<Map<String,Value>> {
    let opts=crate::training::options(req)?;
    let props=model.meta.pointer("/training_schema/properties").and_then(Value::as_object).ok_or_else(||ProviderError::NotSupported("Trainer has no published schema.".into()))?;
    let field=model.meta.get("training_zip_field").and_then(Value::as_str).ok_or_else(||ProviderError::NotSupported("Trainer needs a ZIP URL field.".into()))?;
    let mut body=Map::new(); body.insert(field.into(),json!(zip_url));
    let steps=["steps","number_of_steps"].into_iter().find(|k|props.contains_key(*k));
    if let Some(key)=steps {body.insert(key.into(),json!(opts.steps));}
    let trigger=["trigger_word","trigger_phrase","default_caption"].into_iter().find(|k|props.contains_key(*k)).ok_or_else(||ProviderError::NotSupported("Trainer has no trigger word setting.".into()))?;
    body.insert(trigger.into(),json!(opts.trigger_word));
    if props.contains_key("learning_rate") {body.insert("learning_rate".into(),json!(opts.learning_rate));}
    else if opts.learning_rate!=0.0001 {return Err(ProviderError::InvalidInput("This trainer fixes its learning rate. Use the default.".into()));}
    if let Some(key)=["rank","lora_rank"].into_iter().find(|k|props.contains_key(*k)) {body.insert(key.into(),json!(opts.rank));}
    else if opts.rank!=16 {return Err(ProviderError::InvalidInput("This trainer fixes its rank. Use the default.".into()));}
    for (key,value) in &body {
        if let Some(n)=value.as_f64() {
            let spec=&props[key];
            if spec.get("minimum").and_then(Value::as_f64).is_some_and(|min|n<min) || spec.get("maximum").and_then(Value::as_f64).is_some_and(|max|n>max) {
                return Err(ProviderError::InvalidInput(format!("{key} is outside this trainer's published range.")));
            }
        }
    }
    for key in model.meta.pointer("/training_schema/required").and_then(Value::as_array).into_iter().flatten().filter_map(Value::as_str) {
        if !body.contains_key(key) && props.get(key).and_then(|v|v.get("default")).is_none() {return Err(ProviderError::NotSupported(format!("This trainer requires an unsupported setting: {key}.")));}
    }
    Ok(body)
}

fn training_output(body:&Value, key:&str)->ProviderResult<String> {
    let file=body.get(key).ok_or_else(||ProviderError::Failed("Trainer finished without LoRA weights.".into()))?;
    let url=file.get("url").and_then(Value::as_str).or_else(||file.as_str()).filter(|s|s.starts_with("https://"))
        .ok_or_else(||ProviderError::Failed("Trainer returned no HTTPS LoRA weights URL.".into()))?;
    Ok(url.into())
}

async fn upload_archive(ctx:&Ctx,path:&std::path::Path)->ProviderResult<String> {
    let key=auth(ctx).ok_or_else(||ProviderError::Auth("missing fal key".into()))?;
    let mime=if path.extension().is_some_and(|e|e=="zip") {"application/zip"} else {"application/x-safetensors"};
    let init=send_json(ctx.http.post(UPLOAD).header("Authorization",&key),&json!({"content_type":mime,"file_name":path.file_name().and_then(|n|n.to_str()).unwrap_or("input")})).await?;
    let put=init.get("upload_url").and_then(Value::as_str).ok_or_else(||ProviderError::Failed("fal returned no upload URL.".into()))?;
    let url=init.get("file_url").and_then(Value::as_str).ok_or_else(||ProviderError::Failed("fal returned no file URL.".into()))?;
    let file=tokio::fs::File::open(path).await.map_err(|_|ProviderError::InvalidInput("Could not read training file.".into()))?;
    let size=file.metadata().await.map_err(|_|ProviderError::InvalidInput("Could not stat training file.".into()))?.len();
    use futures::stream::TryStreamExt;
    let stream=futures::stream::try_unfold(file,|mut file|async move {
        use tokio::io::AsyncReadExt;
        let mut chunk=vec![0;65536];let n=file.read(&mut chunk).await?;
        if n==0 {Ok::<_,std::io::Error>(None)}else {chunk.truncate(n);Ok(Some((chunk,file)))}
    }).map_ok(bytes::Bytes::from);
    let resp=ctx.http.put(put).header("Content-Type",mime).header("Content-Length",size).body(reqwest::Body::wrap_stream(stream)).send().await.map_err(net_error)?;
    if !resp.status().is_success() {return Err(ProviderError::Failed(format!("fal upload failed (HTTP {}).",resp.status())));}
    Ok(url.into())
}

#[cfg(test)]
mod training_tests {
    use super::*;
    /// Published OpenAPI responses downloaded on 25.10.2026; no training calls.
    #[test]
    fn published_trainer_shapes_map_zip_settings_and_weights() {
        for (id,raw,zip,step,trigger,output) in [
            ("flux-lora-fast-training",include_str!("../tests/fixtures/training/flux-lora-fast-training.json"),"images_data_url","steps","trigger_word","diffusers_lora_file"),
            ("qwen-image-trainer",include_str!("../tests/fixtures/training/qwen-image-trainer.json"),"image_data_url","steps","trigger_phrase","lora_file"),
            ("qwen-image-2512-trainer",include_str!("../tests/fixtures/training/qwen-image-2512-trainer.json"),"image_data_url","steps","default_caption","diffusers_lora_file"),
            ("wan-trainer/t2v",include_str!("../tests/fixtures/training/wan-trainer-t2v.json"),"training_data_url","number_of_steps","trigger_phrase","lora_file"),
            ("wan-trainer",include_str!("../tests/fixtures/training/wan-trainer.json"),"training_data_url","number_of_steps","trigger_phrase","lora_file"),
        ] {
            let doc:Value=serde_json::from_str(raw).unwrap();let schemas=&doc["components"]["schemas"];
            let input=schemas.as_object().unwrap().iter().find(|(k,_)|k.ends_with("Input")).unwrap().1;
            let model=ModelSpec::new("fal",&format!("fal-ai/{id}"),id,MediaKind::Lora,&[Task::Training]);
            let spec=resolve_schema(&model,&doc,input,schemas).unwrap();
            let req=GenerationRequest{job_id:"j".into(),model:id.into(),prompt:String::new(),negative_prompt:None,params:json!({"_training":{"trigger_word":"maya","family":spec.meta["lora_family"]}}).as_object().unwrap().clone(),inputs:vec![],num_outputs:1};
            let body=training_body(&spec,&req,"https://example.com/images.zip").unwrap();
            assert_eq!(body[zip],"https://example.com/images.zip");assert_eq!(body[step],1000);assert_eq!(body[trigger],"maya");
            assert!(!body.contains_key("_training"));assert!(!body.contains_key("rank"));
            assert_eq!(spec.meta["lora_output"],output);
            assert!(schema::input_bindings(&spec.meta["bindings"]).is_empty(),"training ZIP must not be overwritten by an inference image binding");
            assert_eq!(training_output(&json!({output:{"url":"https://example.com/weights.safetensors"},"config_file":{"url":"https://example.com/config.yaml"}}),output).unwrap(),"https://example.com/weights.safetensors");
            assert!(training_output(&json!({"config_file":{"url":"https://example.com/config.yaml"}}),output).is_err());
        }
    }
}

#[cfg(test)]
mod lora_tests {
    use super::*;
    #[test]
    fn lora_capability_comes_from_schema_and_has_a_family() {
        let model=ModelSpec::new("fal","fal-ai/flux-lora","FLUX LoRA",MediaKind::Image,&[Task::TextToImage]);
        let doc=json!({});
        let schema=json!({"type":"object","properties":{"prompt":{"type":"string"},"loras":{"type":"array","items":{"type":"object","properties":{"path":{"type":"string"},"scale":{"type":"number"}}}}}});
        let spec=resolve_schema(&model,&doc,&schema,&Value::Null).unwrap();
        assert_eq!(lora_binding("https://example.com/maya.safetensors",0.75),json!({"path":"https://example.com/maya.safetensors","scale":0.75}));
        assert_eq!(spec.meta["supports_loras"],true);assert_eq!(spec.meta["lora_family"],"flux1");
        let absent=resolve_schema(&model,&doc,&json!({"type":"object","properties":{"prompt":{"type":"string"}}}),&Value::Null).unwrap();
        assert_ne!(absent.meta["supports_loras"],true);
        assert!(matches!(price_of(&json!({"unit_price":0.001,"unit":"steps"})),Some(Pricing::PerStep{usd}) if usd==0.001));
    }
}


fn lora_binding(url:&str,strength:f64)->Value {json!({"path":url,"scale":strength})}
