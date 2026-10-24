//! stable-diffusion.cpp: the `sd-server` of leejet/stable-diffusion.cpp (default `http://127.0.0.1:1234`).
//!
//! The server keeps one model loaded (from the command line or picked in its web UI) and exposes it
//! through the native async API, which Pictesa uses:
//! - `GET /sdcpp/v1/capabilities`: the loaded model, its modes (`img_gen`, `vid_gen`), defaults,
//!   samplers, limits and ESRGAN upscalers. Each mode becomes a model; each upscaler too.
//! - `POST /sdcpp/v1/img_gen` and `/vid_gen` queue a job; `GET /sdcpp/v1/jobs/{id}` reports queue
//!   position, step progress (from the preview metadata) and the base64 result;
//!   `POST /sdcpp/v1/jobs/{id}/cancel` stops it.
//! - `POST /sdcpp/v1/upscale` runs an ESRGAN model synchronously.
//! - `GET /sdcpp/v1/models/status` is the reachability check.

use std::collections::HashMap;

use async_trait::async_trait;
use base64::Engine as _;
use pictesa_core::{
    common, json_response, keys, CredentialField, Ctx, GenerationRequest, InputRole, InputSlot, MediaInput, MediaKind, ModelSpec, Output,
    ParamSpec, PollStatus, Pricing, Provider, ProviderError, ProviderInfo, ProviderKind, ProviderResult, RemoteHandle, Submission, Task,
};
use serde_json::{json, Map, Value};

use crate::util::{failure, net_error, API_TIMEOUT};

pub struct SdCpp;

const ID: &str = "sdcpp";
const DEFAULT_URL: &str = "http://127.0.0.1:1234";
/// Upscaling answers synchronously; large images take a while on CPU.
const UPSCALE_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(600);

/// A model id part from a file stem: lowercase letters, digits, `.`, `_` and `-`.
fn slug(s: &str) -> String {
    let mut out: String = s
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-') { c.to_ascii_lowercase() } else { '-' })
        .collect();
    while out.contains("--") {
        out = out.replace("--", "-");
    }
    out.trim_matches('-').to_string()
}

fn num(v: Option<&Value>) -> Option<f64> {
    v.and_then(Value::as_f64)
}

fn ratio_of(w: f64, h: f64) -> &'static str {
    let r = w / h;
    common::IMAGE_RATIOS
        .iter()
        .min_by(|a, b| {
            let d = |s: &str| {
                let (x, y) = s.split_once(':').unwrap();
                (x.parse::<f64>().unwrap() / y.parse::<f64>().unwrap() - r).abs()
            };
            d(a).partial_cmp(&d(b)).unwrap()
        })
        .copied()
        .unwrap_or("1:1")
}

/// Whether the loaded model edits from reference images (`ref_images`) rather than img2img (`init_image`).
/// Command-line loads report every feature, so the file name decides there.
fn uses_ref_images(caps: &Value, stem: &str) -> bool {
    let feats = caps.pointer("/features_by_mode/img_gen");
    let has = |k: &str| feats.and_then(|f| f.get(k)).and_then(Value::as_bool).unwrap_or(false);
    let all_features = caps.pointer("/runtime/recipe").is_none_or(Value::is_null) || caps.pointer("/runtime/origin").and_then(Value::as_str) == Some("command_line");
    if !has("ref_images") {
        return false;
    }
    if !has("init_image") || !all_features {
        return true;
    }
    let s = stem.to_ascii_lowercase();
    ["kontext", "edit", "klein", "flux2", "flux-2", "flux3", "flux-3", "qwen-image-2", "boogu"].iter().any(|k| s.contains(k))
}

/// Builds the Pictesa models for what the server has loaded.
/// Steps and guidance for few-step distillations, recognized by file name: SDXS takes one step;
/// Turbo, Lightning, LCM, Hyper, Flash and schnell take the count in the name (`…_8step`) or 4.
fn few_step_defaults(stem: &str) -> Option<(i64, f64)> {
    let name = stem.to_lowercase();
    if name.contains("sdxs") {
        return Some((1, 1.0));
    }
    if !["turbo", "lightning", "lcm", "hyper", "flash", "schnell"].iter().any(|f| name.contains(f)) {
        return None;
    }
    let named = name.match_indices("step").find_map(|(i, _)| {
        let head = name[..i].trim_end_matches(['-', '_']);
        let digits = head.len() - head.trim_end_matches(|c: char| c.is_ascii_digit()).len();
        head[head.len() - digits..].parse::<i64>().ok().filter(|n| (1..=16).contains(n))
    });
    Some((named.unwrap_or(4), 1.0))
}

pub(crate) fn specs_from(caps: &Value) -> Vec<ModelSpec> {
    let mut out = Vec::new();
    let stem = caps.pointer("/model/stem").and_then(Value::as_str).unwrap_or("");
    // Shown by its stem ("sdxs"); the file name stays in the description.
    let file = caps.pointer("/model/name").and_then(Value::as_str).unwrap_or(stem);
    let name = stem;
    let modes = caps.get("supported_modes").and_then(Value::as_array).cloned().unwrap_or_default();
    let modes: Vec<&str> = modes.iter().filter_map(Value::as_str).collect();
    let limits = caps.get("limits").cloned().unwrap_or(Value::Null);
    let preview = if caps.get("preview_modes").and_then(Value::as_array).is_some_and(|m| m.iter().any(|x| x == "proj")) { "proj" } else { "none" };
    let samplers: Vec<String> = caps.get("samplers").and_then(Value::as_array).into_iter().flatten().filter_map(|s| s.as_str().map(String::from)).collect();
    let schedulers: Vec<String> = caps.get("schedulers").and_then(Value::as_array).into_iter().flatten().filter_map(|s| s.as_str().map(String::from)).collect();
    if !stem.is_empty() {
        for mode in &modes {
            let (output, kind_suffix) = match *mode {
                "img_gen" => (MediaKind::Image, ""),
                "vid_gen" => (MediaKind::Video, if modes.contains(&"img_gen") { "-video" } else { "" }),
                _ => continue,
            };
            let defaults = caps.pointer(&format!("/defaults_by_mode/{mode}")).cloned().unwrap_or(Value::Null);
            let feats = caps.pointer(&format!("/features_by_mode/{mode}")).cloned().unwrap_or(Value::Null);
            let has = |k: &str| feats.get(k).and_then(Value::as_bool).unwrap_or(false);
            let (w, h) = (num(defaults.get("width")).unwrap_or(512.0), num(defaults.get("height")).unwrap_or(512.0));
            let video = output == MediaKind::Video;
            let multiple = if video { 16 } else { 64 };
            let id = format!("{}{kind_suffix}", slug(stem));
            let mut tasks = vec![if video { Task::TextToVideo } else { Task::TextToImage }];
            let mut s = ModelSpec::new(ID, &id, &format!("{name}{}", if video && kind_suffix.is_empty() { "" } else if video { " (video)" } else { "" }), output, &[]);
            s.pricing = Some(Pricing::Local);
            s.open_weights = true;
            s.negative_prompt = true;
            s.description = Some(format!("The model sd-server has loaded ({file}). Load another one in the sd-server web UI, then refresh the model list."));
            let ref_mode = !video && uses_ref_images(caps, stem);
            if video {
                if has("init_image") {
                    s.inputs.push(InputSlot { label: Some("Start frame".into()), ..InputSlot::new(InputRole::StartFrame, MediaKind::Image, 1) });
                    tasks.push(Task::ImageToVideo);
                }
                if has("end_image") {
                    s.inputs.push(InputSlot { label: Some("End frame".into()), ..InputSlot::new(InputRole::EndFrame, MediaKind::Image, 1) });
                }
            } else if ref_mode {
                s.inputs.push(InputSlot { label: Some("Reference images".into()), ..InputSlot::new(InputRole::Reference, MediaKind::Image, 4) });
                tasks.push(Task::ImageEdit);
            } else if has("init_image") {
                s.inputs.push(InputSlot { label: Some("Source image (img2img)".into()), ..InputSlot::new(InputRole::Reference, MediaKind::Image, 1) });
                tasks.push(Task::ImageEdit);
                if has("mask_image") {
                    s.inputs.push(InputSlot { label: Some("Inpainting mask".into()), ..InputSlot::new(InputRole::Mask, MediaKind::Image, 1) });
                }
            }
            s.tasks = tasks;
            let ratios: &[&str] = if video { common::VIDEO_RATIOS } else { common::IMAGE_RATIOS };
            let default_ratio = ratio_of(w, h);
            let ratio_default = if ratios.contains(&default_ratio) { default_ratio } else { ratios[0] };
            s.params.push(common::aspect_ratio(ratios, ratio_default));
            if video {
                let fps = num(defaults.get("fps")).unwrap_or(16.0).max(1.0);
                let frames = num(defaults.get("video_frames")).unwrap_or(33.0);
                let seconds = ((frames - 1.0) / fps).round().clamp(1.0, 10.0) as i64;
                s.params.push(common::duration(&[1, 2, 3, 4, 5, 6, 8, 10], seconds));
                s.params.push(ParamSpec::int(keys::FPS, "Frame rate", 4, 60).default_value(fps as i64).advanced());
            }
            let mut steps = defaults.pointer("/sample_params/sample_steps").and_then(Value::as_i64).unwrap_or(20);
            let mut cfg = num(defaults.pointer("/sample_params/guidance/txt_cfg")).unwrap_or(7.0);
            // A server started without --steps/--cfg-scale reports 20 and 7 for any model, which
            // smears few-step distillations; their own settings apply unless the server set others.
            if steps == 20 && cfg == 7.0 {
                if let Some((s, g)) = few_step_defaults(stem) {
                    (steps, cfg) = (s, g);
                }
            }
            s.params.push(common::steps(1, 150, steps.clamp(1, 150)));
            s.params.push(common::guidance(0.0, 30.0, cfg.clamp(0.0, 30.0)));
            if !video && !ref_mode && has("init_image") {
                let strength = num(defaults.get("strength")).unwrap_or(0.75);
                s.params.push(
                    ParamSpec::float(keys::STRENGTH, "Strength", 0.0, 1.0, 0.01)
                        .default_value(strength)
                        .advanced()
                        .help("How much the source image changes (img2img only)."),
                );
            }
            let mut sampler_opts: Vec<&str> = vec!["default"];
            sampler_opts.extend(samplers.iter().map(String::as_str));
            s.params.push(ParamSpec::choices("sampler", "Sampler", &sampler_opts).default_value("default").advanced());
            let mut sched_opts: Vec<&str> = vec!["default"];
            sched_opts.extend(schedulers.iter().map(String::as_str));
            s.params.push(ParamSpec::choices("scheduler", "Scheduler", &sched_opts).default_value("default").advanced());
            s.params.push(ParamSpec::seed());
            s.max_outputs = if video { 1 } else { limits.get("max_batch_count").and_then(Value::as_u64).unwrap_or(1).clamp(1, 4) as u32 };
            let format = if video {
                let formats = caps.pointer("/output_formats_by_mode/vid_gen").and_then(Value::as_array).cloned().unwrap_or_default();
                ["webm", "webp", "avi"].into_iter().find(|f| formats.iter().any(|x| x == f)).unwrap_or("webm")
            } else {
                "png"
            };
            s.meta = json!({
                "mode": mode,
                "stem": stem,
                "area": w * h,
                "multiple": multiple,
                "ref_images": ref_mode,
                "preview": preview,
                "format": format,
                "min": [num(limits.get("min_width")).unwrap_or(64.0), num(limits.get("min_height")).unwrap_or(64.0)],
                "max": [num(limits.get("max_width")).unwrap_or(4096.0), num(limits.get("max_height")).unwrap_or(4096.0)],
            });
            out.push(s);
        }
    }
    if caps.get("upscale").and_then(Value::as_bool) == Some(true) {
        for u in caps.get("upscalers").and_then(Value::as_array).into_iter().flatten() {
            if u.get("image_upscale").and_then(Value::as_bool) != Some(true) {
                continue;
            }
            let Some(name) = u.get("name").and_then(Value::as_str) else { continue };
            let mut s = ModelSpec::new(ID, &format!("upscale-{}", slug(name)), &format!("{name} upscale"), MediaKind::Image, &[Task::Upscale]);
            s.prompt = false;
            s.pricing = Some(Pricing::Local);
            s.open_weights = true;
            s.description = Some(format!("Upscales an image with the ESRGAN model {name} on sd-server."));
            s.inputs.push(InputSlot::new(InputRole::Reference, MediaKind::Image, 1).required());
            s.params.push(
                ParamSpec::int("repeats", "Passes", 1, 4).default_value(1).advanced().help("Runs the upscaler again on its own output; each pass multiplies the size."),
            );
            s.meta = json!({ "mode": "upscale", "upscaler": name });
            out.push(s);
        }
    }
    out
}

fn round_to(x: f64, m: f64) -> f64 {
    ((x / m).round().max(1.0)) * m
}

/// The native request for one generation. `files` holds inputs as data URIs per role.
pub(crate) fn build_request(meta: &Value, req: &GenerationRequest, files: &HashMap<InputRole, Vec<String>>, dims: Option<(u32, u32)>) -> Value {
    let video = meta.get("mode").and_then(Value::as_str) == Some("vid_gen");
    let mut body = Map::new();
    body.insert("prompt".into(), json!(req.prompt));
    if let Some(neg) = req.negative_prompt.as_deref().filter(|n| !n.trim().is_empty()) {
        body.insert("negative_prompt".into(), json!(neg));
    }
    let multiple = num(meta.get("multiple")).unwrap_or(64.0);
    let area = num(meta.get("area")).unwrap_or(512.0 * 512.0);
    let ratio = match req.str(keys::ASPECT_RATIO) {
        Some(r) if r != "auto" => r.split_once(':').and_then(|(a, b)| Some(a.trim().parse::<f64>().ok()? / b.trim().parse::<f64>().ok()?)),
        _ => None,
    }
    .or_else(|| dims.map(|(w, h)| w as f64 / h.max(1) as f64))
    .unwrap_or(1.0);
    let (min_w, min_h) = (num(meta.pointer("/min/0")).unwrap_or(64.0), num(meta.pointer("/min/1")).unwrap_or(64.0));
    let (max_w, max_h) = (num(meta.pointer("/max/0")).unwrap_or(4096.0), num(meta.pointer("/max/1")).unwrap_or(4096.0));
    let (w, h) = match (req.i64(keys::WIDTH), req.i64(keys::HEIGHT)) {
        (Some(w), Some(h)) => (w as f64, h as f64),
        _ => ((area * ratio).sqrt(), (area / ratio).sqrt()),
    };
    body.insert("width".into(), json!(round_to(w, multiple).clamp(min_w, max_w) as u64));
    body.insert("height".into(), json!(round_to(h, multiple).clamp(min_h, max_h) as u64));
    body.insert("seed".into(), json!(req.i64(keys::SEED).unwrap_or(-1)));
    let mut sample = Map::new();
    if let Some(steps) = req.i64(keys::STEPS) {
        sample.insert("sample_steps".into(), json!(steps));
    }
    if let Some(m) = req.str("sampler").filter(|m| *m != "default") {
        sample.insert("sample_method".into(), json!(m));
    }
    if let Some(s) = req.str("scheduler").filter(|s| *s != "default") {
        sample.insert("scheduler".into(), json!(s));
    }
    if let Some(cfg) = req.f64(keys::GUIDANCE) {
        sample.insert("guidance".into(), json!({ "txt_cfg": cfg }));
    }
    if !sample.is_empty() {
        body.insert("sample_params".into(), Value::Object(sample));
    }
    let first = |role: InputRole| files.get(&role).and_then(|l| l.first()).cloned();
    if video {
        let fps = req.i64(keys::FPS).unwrap_or(16).max(1);
        let seconds = req.f64(keys::DURATION).or_else(|| req.i64(keys::DURATION).map(|d| d as f64)).unwrap_or(2.0);
        // The server keeps 4n+1 frames.
        let frames = (((seconds * fps as f64) as i64) / 4) * 4 + 1;
        body.insert("video_frames".into(), json!(frames.max(5)));
        body.insert("fps".into(), json!(fps));
        if let Some(img) = first(InputRole::StartFrame) {
            body.insert("init_image".into(), json!(img));
        }
        if let Some(img) = first(InputRole::EndFrame) {
            body.insert("end_image".into(), json!(img));
        }
    } else {
        body.insert("batch_count".into(), json!(req.num_outputs.max(1)));
        if meta.get("ref_images").and_then(Value::as_bool) == Some(true) {
            if let Some(list) = files.get(&InputRole::Reference).filter(|l| !l.is_empty()) {
                body.insert("ref_images".into(), json!(list));
            }
        } else if let Some(img) = first(InputRole::Reference) {
            body.insert("init_image".into(), json!(img));
            if let Some(s) = req.f64(keys::STRENGTH) {
                body.insert("strength".into(), json!(s));
            }
            if let Some(mask) = first(InputRole::Mask) {
                body.insert("mask_image".into(), json!(mask));
            }
        }
    }
    body.insert("preview".into(), json!(meta.get("preview").and_then(Value::as_str).unwrap_or("none")));
    body.insert("preview_interval".into(), json!(1));
    body.insert("output_format".into(), json!(meta.get("format").and_then(Value::as_str).unwrap_or(if video { "webm" } else { "png" })));
    Value::Object(body)
}

fn image_mime(format: &str) -> String {
    match format {
        "jpeg" | "jpg" => "image/jpeg",
        "webp" => "image/webp",
        _ => "image/png",
    }
    .to_string()
}

fn decode(b64: &str) -> ProviderResult<Vec<u8>> {
    base64::engine::general_purpose::STANDARD.decode(b64.trim()).map_err(|e| ProviderError::Failed(format!("sd-server sent bad base64: {e}")))
}

/// Maps a job record from `GET /sdcpp/v1/jobs/{id}`.
pub(crate) fn job_status(job: &Value) -> ProviderResult<PollStatus> {
    let status = job.get("status").and_then(Value::as_str).unwrap_or("");
    Ok(match status {
        "queued" => PollStatus::Queued { position: job.get("queue_position").and_then(Value::as_u64).map(|p| p as u32) },
        "generating" => match job.get("preview") {
            Some(p) if p.get("total_steps").and_then(Value::as_u64).unwrap_or(0) > 0 => {
                let step = p.get("step").and_then(Value::as_u64).unwrap_or(0);
                let total = p.get("total_steps").and_then(Value::as_u64).unwrap_or(1).max(1);
                let pass = p.get("pass").and_then(Value::as_u64).unwrap_or(1);
                let message = if pass > 1 { format!("Pass {pass}, step {step}/{total}") } else { format!("Step {step}/{total}") };
                PollStatus::Running { progress: Some((step as f32 / total as f32).clamp(0.0, 1.0)), message: Some(message) }
            }
            _ => PollStatus::Running { progress: None, message: None },
        },
        "completed" => {
            let result = job.get("result").cloned().unwrap_or(Value::Null);
            let mut out = Vec::new();
            if let Some(images) = result.get("images").and_then(Value::as_array) {
                let mime = image_mime(result.get("output_format").and_then(Value::as_str).unwrap_or("png"));
                for im in images {
                    if let Some(b) = im.get("b64_json").and_then(Value::as_str) {
                        out.push(Output::Bytes { data: decode(b)?, mime: mime.clone() });
                    }
                }
            } else if let Some(b) = result.get("b64_json").and_then(Value::as_str) {
                let mime = result.get("mime_type").and_then(Value::as_str).unwrap_or("video/webm").to_string();
                out.push(Output::Bytes { data: decode(b)?, mime });
            }
            if out.is_empty() {
                PollStatus::Failed(ProviderError::Failed("sd-server finished without a result".into()))
            } else {
                PollStatus::Done(out)
            }
        }
        "failed" => {
            let msg = job.pointer("/error/message").and_then(Value::as_str).unwrap_or("generation failed");
            PollStatus::Failed(failure(format!("sd-server: {msg}")))
        }
        "cancelled" => PollStatus::Failed(ProviderError::Failed("cancelled".into())),
        other => PollStatus::Running { progress: None, message: Some(other.to_string()) },
    })
}

/// sd-server errors carry `{"error": "...", "code": "..."}`.
fn server_error(status: u16, body: &str) -> ProviderError {
    let v: Value = serde_json::from_str(body).unwrap_or(Value::Null);
    let msg = v.get("error").and_then(Value::as_str).unwrap_or(body).to_string();
    match v.get("code").and_then(Value::as_str) {
        Some("model_loading") => ProviderError::Transient(format!("sd-server is loading a model: {msg}")),
        Some("model_not_loaded") => ProviderError::NotSupported("sd-server has no model loaded; load one in its web UI".into()),
        Some("model_busy") => ProviderError::Transient(msg),
        _ => ProviderError::from_http(status, &msg, None),
    }
}

async fn call(rb: reqwest::RequestBuilder) -> ProviderResult<Value> {
    let resp = rb.send().await.map_err(net_error)?;
    let status = resp.status();
    if status.is_success() {
        return json_response(resp).await;
    }
    let text = resp.text().await.map_err(net_error)?;
    Err(server_error(status.as_u16(), &text))
}

async fn data_uri(input: &MediaInput) -> ProviderResult<String> {
    input.data_uri().await
}

#[async_trait]
impl Provider for SdCpp {
    fn info(&self) -> ProviderInfo {
        ProviderInfo {
            id: ID.into(),
            name: "stable-diffusion.cpp".into(),
            kind: ProviderKind::Local,
            fields: vec![CredentialField::base_url(DEFAULT_URL, &["SDCPP_URL", "SD_SERVER_URL"])],
            signup_url: Some("https://github.com/leejet/stable-diffusion.cpp/releases".into()),
            docs_url: Some("https://github.com/leejet/stable-diffusion.cpp/blob/master/examples/server/api.md".into()),
            max_concurrency: 2,
        }
    }

    /// The loaded model is only known to the running server; see `discover()`.
    fn models(&self) -> Vec<ModelSpec> {
        Vec::new()
    }

    async fn discover(&self, ctx: &Ctx) -> ProviderResult<Vec<ModelSpec>> {
        let base = ctx.base_url(DEFAULT_URL);
        let mut caps = call(ctx.http.get(format!("{base}/sdcpp/v1/capabilities")).timeout(API_TIMEOUT)).await?;
        if caps.get("runtime").is_none() {
            if let Ok(status) = call(ctx.http.get(format!("{base}/sdcpp/v1/models/status")).timeout(API_TIMEOUT)).await {
                caps["runtime"] = status;
            }
        }
        Ok(specs_from(&caps))
    }

    async fn check(&self, ctx: &Ctx) -> ProviderResult<()> {
        let base = ctx.base_url(DEFAULT_URL);
        let status = match call(ctx.http.get(format!("{base}/sdcpp/v1/models/status")).timeout(API_TIMEOUT)).await {
            Ok(s) => s,
            // Servers before model management: the capabilities route still answers.
            Err(ProviderError::InvalidInput(m)) if m.starts_with("HTTP 404") => {
                call(ctx.http.get(format!("{base}/sdcpp/v1/capabilities")).timeout(API_TIMEOUT)).await?;
                return Ok(());
            }
            Err(e) => return Err(e),
        };
        match status.get("state").and_then(Value::as_str) {
            Some("ready") | None => Ok(()),
            Some("loading") => Err(ProviderError::Transient("sd-server is loading a model".into())),
            Some("error") => Err(ProviderError::NotSupported(format!(
                "sd-server failed to load a model: {}",
                status.get("error").and_then(Value::as_str).unwrap_or("unknown error")
            ))),
            Some(_) => Err(ProviderError::NotSupported("sd-server is running but no model is loaded; load one in its web UI".into())),
        }
    }

    async fn submit(&self, ctx: &Ctx, model: &ModelSpec, req: &GenerationRequest) -> ProviderResult<Submission> {
        let base = ctx.base_url(DEFAULT_URL);
        let meta = &model.meta;
        let mode = meta.get("mode").and_then(Value::as_str).unwrap_or("img_gen");
        if mode == "upscale" {
            let input = req.input(InputRole::Reference).ok_or_else(|| ProviderError::InvalidInput("attach the image to upscale".into()))?;
            let body = json!({
                "image": data_uri(input).await?,
                "upscaler": meta.get("upscaler"),
                "repeats": req.i64("repeats").unwrap_or(1).clamp(1, 4),
                "output_format": "png",
            });
            let v = call(ctx.http.post(format!("{base}/sdcpp/v1/upscale")).timeout(UPSCALE_TIMEOUT).json(&body)).await?;
            let b64 = v.pointer("/images/0/b64_json").and_then(Value::as_str).ok_or_else(|| ProviderError::Failed("sd-server returned no image".into()))?;
            let mime = image_mime(v.get("output_format").and_then(Value::as_str).unwrap_or("png"));
            return Ok(Submission::Done(vec![Output::Bytes { data: decode(b64)?, mime }]));
        }
        // The server holds one model at a time; refuse rather than run on another one.
        let caps = call(ctx.http.get(format!("{base}/sdcpp/v1/capabilities")).timeout(API_TIMEOUT)).await?;
        let loaded = caps.pointer("/model/stem").and_then(Value::as_str).unwrap_or("");
        let wanted = meta.get("stem").and_then(Value::as_str).unwrap_or("");
        if loaded != wanted {
            let now = if loaded.is_empty() { "no model".to_string() } else { loaded.to_string() };
            return Err(ProviderError::NotSupported(format!("sd-server now has {now} loaded, not {wanted}; refresh the model list")));
        }
        let mut files: HashMap<InputRole, Vec<String>> = HashMap::new();
        let mut dims = None;
        for input in &req.inputs {
            if dims.is_none() && matches!(input.role, InputRole::Reference | InputRole::StartFrame) {
                dims = input.width.zip(input.height);
            }
            files.entry(input.role).or_default().push(data_uri(input).await?);
        }
        let body = build_request(meta, req, &files, dims);
        let answer = call(ctx.http.post(format!("{base}/sdcpp/v1/{mode}")).timeout(API_TIMEOUT).json(&body)).await?;
        let id = answer.get("id").and_then(Value::as_str).ok_or_else(|| ProviderError::Failed(format!("sd-server returned no job id: {answer}")))?;
        Ok(Submission::Pending(RemoteHandle::with(id, json!({ "base_url": base, "mode": mode }))))
    }

    async fn poll(&self, ctx: &Ctx, handle: &RemoteHandle) -> ProviderResult<PollStatus> {
        let base = handle.data_str("base_url").map(String::from).unwrap_or_else(|| ctx.base_url(DEFAULT_URL));
        let resp = ctx.http.get(format!("{base}/sdcpp/v1/jobs/{}", handle.id)).timeout(API_TIMEOUT).send().await.map_err(net_error)?;
        if matches!(resp.status().as_u16(), 404 | 410) {
            return Ok(PollStatus::Failed(ProviderError::Failed("sd-server no longer knows this job (was it restarted?)".into())));
        }
        let status = resp.status();
        if !status.is_success() {
            let text = resp.text().await.map_err(net_error)?;
            return Err(server_error(status.as_u16(), &text));
        }
        job_status(&json_response(resp).await?)
    }

    async fn cancel(&self, ctx: &Ctx, handle: &RemoteHandle) -> ProviderResult<()> {
        let base = handle.data_str("base_url").map(String::from).unwrap_or_else(|| ctx.base_url(DEFAULT_URL));
        let _ = ctx.http.post(format!("{base}/sdcpp/v1/jobs/{}/cancel", handle.id)).timeout(API_TIMEOUT).send().await;
        Ok(())
    }

    fn poll_interval(&self, attempt: u32) -> std::time::Duration {
        std::time::Duration::from_millis((500 + 250 * attempt as u64).min(2_000))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Recorded from sd-server (release master-999-e0ed724, CPU) with an SDXS checkpoint and Real-ESRGAN.
    fn fixture() -> Value {
        serde_json::from_str(include_str!("../tests/fixtures/sdcpp_server.json")).unwrap()
    }

    fn req(params: Value, n: u32) -> GenerationRequest {
        GenerationRequest {
            job_id: "j".into(),
            model: "sdxs".into(),
            prompt: "a red fox in snow".into(),
            negative_prompt: Some("blurry".into()),
            params: params.as_object().cloned().unwrap_or_default(),
            inputs: vec![],
            num_outputs: n,
        }
    }

    #[test]
    fn loaded_model_and_upscaler_become_models() {
        let specs = specs_from(&fixture()["capabilities"]);
        let ids: Vec<&str> = specs.iter().map(|s| s.id.as_str()).collect();
        assert_eq!(ids, vec!["sdcpp:sdxs", "sdcpp:upscale-realesrgan_x4plus"]);
        let img = &specs[0];
        assert_eq!(img.output, MediaKind::Image);
        assert_eq!(img.tasks, vec![Task::TextToImage, Task::ImageEdit]);
        assert_eq!(img.max_outputs, 4);
        assert_eq!(img.param_spec(keys::STEPS).unwrap().default, Some(json!(1)), "server default");
        assert_eq!(img.param_spec(keys::GUIDANCE).unwrap().default, Some(json!(1.0)));
        assert!(img.param_spec("sampler").unwrap().validate(&json!("euler_a")).is_ok());
        assert_eq!(img.inputs[0].role, InputRole::Reference);
        assert_eq!(img.meta["ref_images"], false, "an SD1-class checkpoint does img2img");
        let up = &specs[1];
        assert_eq!((up.tasks.clone(), up.prompt), (vec![Task::Upscale], false));
    }

    #[test]
    fn few_step_models_get_their_own_defaults_from_a_stock_server() {
        let defaults = |stem: &str, steps: i64, cfg: f64| {
            let mut caps = fixture()["capabilities"].clone();
            caps["model"] = json!({ "name": format!("{stem}.safetensors"), "stem": stem });
            caps["defaults_by_mode"]["img_gen"]["sample_params"] = json!({ "sample_steps": steps, "guidance": { "txt_cfg": cfg } });
            let img = specs_from(&caps).remove(0);
            (img.param_spec(keys::STEPS).unwrap().default.clone(), img.param_spec(keys::GUIDANCE).unwrap().default.clone())
        };
        assert_eq!(defaults("sdxs", 20, 7.0), (Some(json!(1)), Some(json!(1.0))));
        assert_eq!(defaults("sd_xl_turbo_1.0_fp16", 20, 7.0), (Some(json!(4)), Some(json!(1.0))));
        assert_eq!(defaults("sdxl_lightning_8step", 20, 7.0), (Some(json!(8)), Some(json!(1.0))));
        assert_eq!(defaults("v1-5-pruned-emaonly", 20, 7.0), (Some(json!(20)), Some(json!(7.0))), "regular models keep the stock values");
        assert_eq!(defaults("sdxl_lightning_4step", 6, 2.0), (Some(json!(6)), Some(json!(2.0))), "the server's own settings win");
    }

    #[test]
    fn video_models_get_frames_and_start_frames() {
        let mut caps = fixture()["capabilities"].clone();
        caps["model"] = json!({"name": "wan2.1_t2v_1.3B_fp16.safetensors", "stem": "wan2.1_t2v_1.3B_fp16", "path": "/m/wan.safetensors"});
        caps["supported_modes"] = json!(["vid_gen"]);
        caps["defaults_by_mode"]["vid_gen"] = json!({"width": 832, "height": 480, "video_frames": 33, "fps": 16, "sample_params": {"sample_steps": 20, "guidance": {"txt_cfg": 6.0}}});
        caps["features_by_mode"]["vid_gen"] = json!({"init_image": true, "end_image": false});
        caps["output_formats_by_mode"]["vid_gen"] = json!(["webm", "webp", "avi"]);
        let specs = specs_from(&caps);
        let v = &specs[0];
        assert_eq!(v.id, "sdcpp:wan2.1_t2v_1.3b_fp16");
        assert_eq!(v.output, MediaKind::Video);
        assert_eq!(v.tasks, vec![Task::TextToVideo, Task::ImageToVideo]);
        assert_eq!(v.param_spec(keys::DURATION).unwrap().default, Some(json!(2)));
        let mut files = HashMap::new();
        files.insert(InputRole::StartFrame, vec!["data:image/png;base64,AA==".to_string()]);
        let body = build_request(&v.meta, &req(json!({"duration": 3, "fps": 16, "aspect_ratio": "16:9"}), 1), &files, None);
        assert_eq!(body["video_frames"], 49, "3 s at 16 fps, 4n+1");
        assert_eq!(body["init_image"], "data:image/png;base64,AA==");
        assert_eq!(body["output_format"], "webm");
        assert_eq!((body["width"].clone(), body["height"].clone()), (json!(848), json!(480)));
    }

    #[test]
    fn image_request_maps_shared_params() {
        let specs = specs_from(&fixture()["capabilities"]);
        let mut files = HashMap::new();
        files.insert(InputRole::Reference, vec!["data:image/png;base64,AA==".to_string()]);
        let params = json!({"aspect_ratio": "16:9", "steps": 4, "guidance": 1.5, "seed": 7, "sampler": "euler_a", "scheduler": "default", "strength": 0.6});
        let body = build_request(&specs[0].meta, &req(params, 2), &files, None);
        assert_eq!(
            body,
            json!({
                "prompt": "a red fox in snow", "negative_prompt": "blurry", "width": 704, "height": 384, "seed": 7,
                "sample_params": {"sample_steps": 4, "sample_method": "euler_a", "guidance": {"txt_cfg": 1.5}},
                "batch_count": 2, "init_image": "data:image/png;base64,AA==", "strength": 0.6,
                "preview": "proj", "preview_interval": 1, "output_format": "png"
            })
        );
        // Without a seed the server picks one.
        let body = build_request(&specs[0].meta, &req(json!({}), 1), &HashMap::new(), None);
        assert_eq!(body["seed"], -1);
        assert_eq!((body["width"].clone(), body["height"].clone()), (json!(512), json!(512)));
    }

    #[test]
    fn recorded_jobs_map_to_poll_states() {
        let f = fixture();
        match job_status(&f["generating"]).unwrap() {
            PollStatus::Running { progress: Some(p), message: Some(m) } => {
                assert_eq!(p, 1.0);
                assert_eq!(m, "Step 1/1");
            }
            other => panic!("{other:?}"),
        }
        match job_status(&f["completed"]).unwrap() {
            PollStatus::Done(out) => match &out[0] {
                Output::Bytes { data, mime } => {
                    assert_eq!(mime, "image/png");
                    assert!(data.starts_with(b"\x89PNG"));
                }
                other => panic!("{other:?}"),
            },
            other => panic!("{other:?}"),
        }
        let queued = json!({"id": "job_x", "status": "queued", "queue_position": 2});
        assert!(matches!(job_status(&queued).unwrap(), PollStatus::Queued { position: Some(2) }));
        let failed = json!({"status": "failed", "error": {"code": "generation_failed", "message": "generate_image returned empty results"}});
        assert!(matches!(job_status(&failed).unwrap(), PollStatus::Failed(ProviderError::Failed(m)) if m.contains("empty results")));
        let video = json!({"status": "completed", "result": {"output_format": "webm", "mime_type": "video/webm", "fps": 16, "frame_count": 33, "b64_json": "GkXfow=="}});
        match job_status(&video).unwrap() {
            PollStatus::Done(out) => assert!(matches!(&out[0], Output::Bytes { mime, .. } if mime == "video/webm")),
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn server_errors() {
        assert!(matches!(server_error(503, r#"{"error":"No model is loaded","code":"model_not_loaded"}"#), ProviderError::NotSupported(_)));
        assert!(server_error(503, r#"{"error":"loading","code":"model_loading"}"#).is_retryable());
        assert!(matches!(server_error(429, r#"{"error":"queue full"}"#), ProviderError::RateLimited { .. }));
        assert!(matches!(server_error(400, r#"{"error":"invalid width"}"#), ProviderError::InvalidInput(m) if m.contains("invalid width")));
    }
}
