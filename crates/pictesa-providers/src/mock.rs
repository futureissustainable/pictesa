//! A free fake provider for development and tests (enabled with `PICTESA_MOCK=1`).
//! It behaves like a real queued API: submit, a few seconds of progress, then media.

use std::time::{SystemTime, UNIX_EPOCH};

use async_trait::async_trait;
use pictesa_core::{
    common, CredentialField, Ctx, GenerationRequest, InputRole, InputSlot, MediaKind, ModelSpec,
    Output, PollStatus, Pricing, Provider, ProviderError, ProviderInfo, ProviderKind,
    ProviderResult, RemoteHandle, Submission, Task,
};
use serde_json::json;

pub struct Mock;

/// Simulated run time.
const RUN_MS: u128 = 4000;

fn now_ms() -> u128 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis())
        .unwrap_or(0)
}

/// Gradient pixels (RGB, row by row) whose colours depend on the prompt, so outputs differ.
fn gradient(prompt: &str, w: u32, h: u32) -> Vec<u8> {
    let seed = prompt
        .bytes()
        .fold(7u32, |a, b| a.wrapping_mul(31).wrapping_add(b as u32));
    let (r0, g0, b0) = (
        (seed & 0xff) as f32,
        ((seed >> 8) & 0xff) as f32,
        ((seed >> 16) & 0xff) as f32,
    );
    let mut out = Vec::with_capacity((w * h * 3) as usize);
    for y in 0..h {
        for x in 0..w {
            let t = x as f32 / w as f32;
            let u = y as f32 / h as f32;
            out.push((r0 * (1.0 - t) + 242.0 * t) as u8);
            out.push((g0 * (1.0 - u) + 105.0 * u) as u8);
            out.push((b0 * 0.5 + 61.0 * t) as u8);
        }
    }
    out
}

/// A small PNG gradient.
fn png(prompt: &str, w: u32, h: u32) -> Vec<u8> {
    encode_png(w, h, &gradient(prompt, w, h), false)
}

/// A cut-out like a background remover returns: an opaque gradient ellipse (the "subject") on a
/// fully transparent background.
fn cutout(prompt: &str, w: u32, h: u32) -> Vec<u8> {
    let rgb = gradient(prompt, w, h);
    let mut rgba = Vec::with_capacity((w * h * 4) as usize);
    for (i, px) in rgb.chunks(3).enumerate() {
        let (x, y) = (i as u32 % w, i as u32 / w);
        let dx = (x as f32 + 0.5) / w as f32 - 0.5;
        let dy = (y as f32 + 0.5) / h as f32 - 0.5;
        // Fully opaque inside, a soft edge over the last few percent of the radius.
        let alpha = ((1.0 - (dx * dx + dy * dy).sqrt() / 0.36) * 25.0).clamp(0.0, 1.0);
        rgba.extend_from_slice(px);
        rgba.push((alpha * 255.0) as u8);
    }
    encode_png(w, h, &rgba, true)
}

/// Encodes RGB (or RGBA with `alpha`) pixels, row by row, as a PNG.
fn encode_png(w: u32, h: u32, pixels: &[u8], alpha: bool) -> Vec<u8> {
    use image::ImageEncoder;
    let mut out = Vec::new();
    let color = if alpha { image::ExtendedColorType::Rgba8 } else { image::ExtendedColorType::Rgb8 };
    match image::codecs::png::PngEncoder::new(&mut out).write_image(pixels, w, h, color) {
        Ok(()) => out,
        Err(_) => Vec::new(),
    }
}

/// Edits keep the shape of their source image, like real edit models; other runs follow the ratio.
fn size_for(req: &GenerationRequest) -> (u32, u32) {
    if let Some((w, h)) = req.input(InputRole::Reference).and_then(|i| Some((i.width?, i.height?))).filter(|(w, h)| *w > 0 && *h > 0) {
        let scale = 768.0 / w.max(h) as f32;
        return (((w as f32 * scale) as u32).max(16), ((h as f32 * scale) as u32).max(16));
    }
    let ratio = req.str("aspect_ratio").unwrap_or("1:1");
    crate::util::ratio_to_size(ratio, 768).unwrap_or((768, 768))
}

async fn video(prompt: &str, seconds: u32, w: u32, h: u32) -> ProviderResult<Vec<u8>> {
    let dir = std::env::temp_dir().join(format!("pictesa-mock-{}", ulid::Ulid::new()));
    tokio::fs::create_dir_all(&dir)
        .await
        .map_err(|e| ProviderError::Failed(e.to_string()))?;
    let out = dir.join("out.mp4");
    let hue = prompt.len() * 37 % 360;
    let status = tokio::process::Command::new("ffmpeg")
        .args([
            "-v",
            "error",
            "-y",
            "-f",
            "lavfi",
            "-i",
            &format!("testsrc2=size={w}x{h}:rate=24:duration={seconds}"),
            "-vf",
            &format!("hue=h={hue}"),
            "-pix_fmt",
            "yuv420p",
            "-c:v",
            "libx264",
            "-preset",
            "ultrafast",
        ])
        .arg(&out)
        .status()
        .await
        .map_err(|e| {
            ProviderError::NotSupported(format!("the mock video model needs ffmpeg: {e}"))
        })?;
    if !status.success() {
        return Err(ProviderError::Failed(
            "ffmpeg could not render the mock video".into(),
        ));
    }
    let bytes = tokio::fs::read(&out)
        .await
        .map_err(|e| ProviderError::Failed(e.to_string()))?;
    let _ = tokio::fs::remove_dir_all(&dir).await;
    Ok(bytes)
}

#[async_trait]
impl Provider for Mock {
    fn info(&self) -> ProviderInfo {
        ProviderInfo {
            id: "mock".into(),
            name: "Mock (development)".into(),
            kind: ProviderKind::Local,
            // An optional fake key lets tests exercise hosted billing (platform keys are charged).
            fields: vec![CredentialField {
                optional: true,
                ..CredentialField::api_key(&["PICTESA_MOCK_KEY"])
            }],
            signup_url: None,
            docs_url: None,
            max_concurrency: 4,
        }
    }

    fn models(&self) -> Vec<ModelSpec> {
        vec![
            ModelSpec::new("mock", "image", "Mock image", MediaKind::Image, &[Task::TextToImage, Task::ImageEdit])
                .input(InputSlot::new(InputRole::Reference, MediaKind::Image, 4))
                .input(InputSlot::new(InputRole::Mask, MediaKind::Image, 1))
                .param(common::aspect_ratio(common::IMAGE_RATIOS, "1:1"))
                .param(pictesa_core::ParamSpec::seed())
                .batch()
                .max_outputs(4)
                .pricing(Pricing::PerImage { usd: 0.01 })
                .describe("Free fake model for trying Pictesa without keys. Returns a gradient. The price is fake and only used to test billing.")
                .tag("featured"),
            ModelSpec::new("mock", "remove-background", "Mock background remover", MediaKind::Image, &[Task::ImageEdit])
                .input(InputSlot { label: Some("Image".into()), ..InputSlot::new(InputRole::Reference, MediaKind::Image, 1).required() })
                .no_prompt()
                .pricing(Pricing::PerImage { usd: 0.01 })
                .describe("Free fake background remover. Returns a transparent PNG with a gradient shape as the subject."),
            ModelSpec::new("mock", "video", "Mock video", MediaKind::Video, &[Task::TextToVideo, Task::ImageToVideo])
                .input(InputSlot::new(InputRole::StartFrame, MediaKind::Image, 1))
                .input(InputSlot::new(InputRole::EndFrame, MediaKind::Image, 1))
                .param(common::duration(&[2, 4, 6], 2))
                .param(common::aspect_ratio(common::VIDEO_RATIOS, "16:9"))
                .pricing(Pricing::PerVideo { usd: 0.05 })
                .describe("Free fake video model (needs ffmpeg). Returns a test pattern clip.")
                .tag("featured"),
        ]
    }

    async fn check(&self, _ctx: &Ctx) -> ProviderResult<()> {
        Ok(())
    }

    async fn submit(
        &self,
        _ctx: &Ctx,
        model: &ModelSpec,
        req: &GenerationRequest,
    ) -> ProviderResult<Submission> {
        if req.prompt.to_lowercase().contains("busy please") {
            return Err(ProviderError::RateLimited { retry_after_ms: Some(3000) });
        }
        if req.prompt.to_lowercase().contains("fail please") {
            return Err(ProviderError::ContentPolicy(
                "the mock refuses prompts containing 'fail please'".into(),
            ));
        }
        let (w, h) = size_for(req);
        Ok(Submission::Pending(RemoteHandle::with(
            format!("mock-{}", ulid::Ulid::new()),
            json!({
                "started": now_ms().to_string(), "model": model.remote_id(), "prompt": req.prompt, "w": w, "h": h,
                "n": req.num_outputs, "seconds": req.i64("duration").unwrap_or(2), "batch": model.is_batch(&req.params),
            }),
        )))
    }

    async fn poll(&self, _ctx: &Ctx, handle: &RemoteHandle) -> ProviderResult<PollStatus> {
        let started: u128 = handle
            .data_str("started")
            .and_then(|s| s.parse().ok())
            .unwrap_or(0);
        let elapsed = now_ms().saturating_sub(started);
        if elapsed < 800 {
            return Ok(PollStatus::Queued { position: Some(0) });
        }
        if elapsed < RUN_MS {
            return Ok(PollStatus::Running {
                progress: Some(elapsed as f32 / RUN_MS as f32),
                message: Some("Painting".into()),
            });
        }
        let prompt = handle.data_str("prompt").unwrap_or("");
        if handle.data_str("model") == Some("video") {
            let secs = handle
                .data
                .get("seconds")
                .and_then(|v| v.as_u64())
                .unwrap_or(2) as u32;
            // Half-size even dimensions keep the fake clip cheap to render.
            let w = (handle.data.get("w").and_then(|v| v.as_u64()).unwrap_or(768) as u32 / 2) & !1;
            let h = (handle.data.get("h").and_then(|v| v.as_u64()).unwrap_or(432) as u32 / 2) & !1;
            return Ok(PollStatus::Done(vec![Output::Bytes {
                data: video(prompt, secs, w.max(64), h.max(64)).await?,
                mime: "video/mp4".into(),
            }]));
        }
        let (w, h) = (
            handle.data.get("w").and_then(|v| v.as_u64()).unwrap_or(768) as u32,
            handle.data.get("h").and_then(|v| v.as_u64()).unwrap_or(768) as u32,
        );
        if handle.data_str("model") == Some("remove-background") {
            return Ok(PollStatus::Done(vec![Output::Bytes { data: cutout(prompt, w, h), mime: "image/png".into() }]));
        }
        let n = handle
            .data
            .get("n")
            .and_then(|v| v.as_u64())
            .unwrap_or(1)
            .max(1);
        Ok(PollStatus::Done(
            (0..n)
                .map(|i| Output::Bytes {
                    data: png(&format!("{prompt}{i}"), w, h),
                    mime: "image/png".into(),
                })
                .collect(),
        ))
    }

    /// A coarse picture that sharpens as the fake run progresses, like a sampler preview.
    fn preview(&self, handle: &RemoteHandle) -> Option<(Vec<u8>, String)> {
        if handle.data_str("model") == Some("video") {
            return None;
        }
        let started: u128 = handle.data_str("started").and_then(|s| s.parse().ok())?;
        let progress = (now_ms().saturating_sub(started) as f32 / RUN_MS as f32).clamp(0.0, 1.0);
        let (w, h) = (
            handle.data.get("w").and_then(|v| v.as_u64()).unwrap_or(768) as u32 / 4,
            handle.data.get("h").and_then(|v| v.as_u64()).unwrap_or(768) as u32 / 4,
        );
        // Changes a few times per run, so each poll can deliver a new frame.
        let stage = (progress * 5.0) as u32;
        Some((png(&format!("{}{stage}", handle.data_str("prompt").unwrap_or("")), w, h), "image/png".into()))
    }

    fn poll_interval(&self, _attempt: u32) -> std::time::Duration {
        std::time::Duration::from_millis(400)
    }

    fn poll_interval_for(&self, _handle: &RemoteHandle, attempt: u32) -> std::time::Duration {
        self.poll_interval(attempt)
    }
}
