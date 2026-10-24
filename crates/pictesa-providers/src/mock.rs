//! A free fake provider for development and tests (enabled with `PICTESA_MOCK=1`).
//! It behaves like a real queued API: submit, a few seconds of progress, then media.

use std::time::{SystemTime, UNIX_EPOCH};

use async_trait::async_trait;
use pictesa_core::{
    common, CredentialField, Ctx, GenerationRequest, InputRole, InputSlot, MediaKind, ModelSpec, Output, PollStatus, Pricing,
    Provider, ProviderError, ProviderInfo, ProviderKind, ProviderResult, RemoteHandle, Submission, Task,
};
use serde_json::json;

pub struct Mock;

/// Simulated run time.
const RUN_MS: u128 = 4000;

fn now_ms() -> u128 {
    SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_millis()).unwrap_or(0)
}

/// A small PNG gradient whose colours depend on the prompt, so outputs differ.
fn png(prompt: &str, w: u32, h: u32) -> Vec<u8> {
    let seed = prompt.bytes().fold(7u32, |a, b| a.wrapping_mul(31).wrapping_add(b as u32));
    let (r0, g0, b0) = ((seed & 0xff) as f32, ((seed >> 8) & 0xff) as f32, ((seed >> 16) & 0xff) as f32);
    let mut raw = Vec::with_capacity(((w * 3 + 1) * h) as usize);
    for y in 0..h {
        raw.push(0);
        for x in 0..w {
            let t = x as f32 / w as f32;
            let u = y as f32 / h as f32;
            raw.push((r0 * (1.0 - t) + 242.0 * t) as u8);
            raw.push((g0 * (1.0 - u) + 105.0 * u) as u8);
            raw.push((b0 * 0.5 + 61.0 * t) as u8);
        }
    }
    encode_png(w, h, &raw)
}

fn encode_png(w: u32, h: u32, raw: &[u8]) -> Vec<u8> {
    fn crc(data: &[u8]) -> u32 {
        let mut c = 0xffff_ffffu32;
        for &b in data {
            c ^= b as u32;
            for _ in 0..8 {
                c = if c & 1 != 0 { 0xedb8_8320 ^ (c >> 1) } else { c >> 1 };
            }
        }
        !c
    }
    fn chunk(out: &mut Vec<u8>, kind: &[u8], data: &[u8]) {
        out.extend_from_slice(&(data.len() as u32).to_be_bytes());
        let mut body = kind.to_vec();
        body.extend_from_slice(data);
        out.extend_from_slice(&body);
        out.extend_from_slice(&crc(&body).to_be_bytes());
    }
    // Stored (uncompressed) deflate blocks keep this dependency-free.
    let mut z = vec![0x78, 0x01];
    let mut adler = (1u32, 0u32);
    for (i, block) in raw.chunks(65_535).enumerate() {
        let last = (i + 1) * 65_535 >= raw.len();
        z.push(last as u8);
        z.extend_from_slice(&(block.len() as u16).to_le_bytes());
        z.extend_from_slice(&(!(block.len() as u16)).to_le_bytes());
        z.extend_from_slice(block);
        for &b in block {
            adler.0 = (adler.0 + b as u32) % 65_521;
            adler.1 = (adler.1 + adler.0) % 65_521;
        }
    }
    z.extend_from_slice(&((adler.1 << 16) | adler.0).to_be_bytes());
    let mut out = vec![0x89, b'P', b'N', b'G', 0x0d, 0x0a, 0x1a, 0x0a];
    let mut ihdr = Vec::new();
    ihdr.extend_from_slice(&w.to_be_bytes());
    ihdr.extend_from_slice(&h.to_be_bytes());
    ihdr.extend_from_slice(&[8, 2, 0, 0, 0]);
    chunk(&mut out, b"IHDR", &ihdr);
    chunk(&mut out, b"IDAT", &z);
    chunk(&mut out, b"IEND", &[]);
    out
}

fn size_for(req: &GenerationRequest) -> (u32, u32) {
    let ratio = req.str("aspect_ratio").unwrap_or("1:1");
    crate::util::ratio_to_size(ratio, 768).unwrap_or((768, 768))
}

async fn video(prompt: &str, seconds: u32) -> ProviderResult<Vec<u8>> {
    let dir = std::env::temp_dir().join(format!("pictesa-mock-{}", ulid::Ulid::new()));
    tokio::fs::create_dir_all(&dir).await.map_err(|e| ProviderError::Failed(e.to_string()))?;
    let out = dir.join("out.mp4");
    let hue = prompt.len() * 37 % 360;
    let status = tokio::process::Command::new("ffmpeg")
        .args(["-v", "error", "-y", "-f", "lavfi", "-i", &format!("testsrc2=size=640x360:rate=24:duration={seconds}"), "-vf", &format!("hue=h={hue}"), "-pix_fmt", "yuv420p", "-c:v", "libx264", "-preset", "ultrafast"])
        .arg(&out)
        .status()
        .await
        .map_err(|e| ProviderError::NotSupported(format!("the mock video model needs ffmpeg: {e}")))?;
    if !status.success() {
        return Err(ProviderError::Failed("ffmpeg could not render the mock video".into()));
    }
    let bytes = tokio::fs::read(&out).await.map_err(|e| ProviderError::Failed(e.to_string()))?;
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
            fields: vec![CredentialField { optional: true, ..CredentialField::base_url("local", &[]) }],
            signup_url: None,
            docs_url: None,
            max_concurrency: 4,
        }
    }

    fn models(&self) -> Vec<ModelSpec> {
        vec![
            ModelSpec::new("mock", "image", "Mock image", MediaKind::Image, &[Task::TextToImage, Task::ImageEdit])
                .input(InputSlot::new(InputRole::Reference, MediaKind::Image, 4))
                .param(common::aspect_ratio(common::IMAGE_RATIOS, "1:1"))
                .param(pictesa_core::ParamSpec::seed())
                .max_outputs(4)
                .pricing(Pricing::Local)
                .describe("Free fake model for trying Pictesa without keys. Returns a gradient.")
                .tag("featured"),
            ModelSpec::new("mock", "video", "Mock video", MediaKind::Video, &[Task::TextToVideo, Task::ImageToVideo])
                .input(InputSlot::new(InputRole::StartFrame, MediaKind::Image, 1))
                .input(InputSlot::new(InputRole::EndFrame, MediaKind::Image, 1))
                .param(common::duration(&[2, 4, 6], 2))
                .param(common::aspect_ratio(common::VIDEO_RATIOS, "16:9"))
                .pricing(Pricing::Local)
                .describe("Free fake video model (needs ffmpeg). Returns a test pattern clip.")
                .tag("featured"),
        ]
    }

    async fn check(&self, _ctx: &Ctx) -> ProviderResult<()> {
        Ok(())
    }

    async fn submit(&self, _ctx: &Ctx, model: &ModelSpec, req: &GenerationRequest) -> ProviderResult<Submission> {
        if req.prompt.to_lowercase().contains("fail please") {
            return Err(ProviderError::ContentPolicy("the mock refuses prompts containing 'fail please'".into()));
        }
        let (w, h) = size_for(req);
        Ok(Submission::Pending(RemoteHandle::with(
            format!("mock-{}", ulid::Ulid::new()),
            json!({
                "started": now_ms().to_string(), "model": model.remote_id(), "prompt": req.prompt, "w": w, "h": h,
                "n": req.num_outputs, "seconds": req.i64("duration").unwrap_or(2),
            }),
        )))
    }

    async fn poll(&self, _ctx: &Ctx, handle: &RemoteHandle) -> ProviderResult<PollStatus> {
        let started: u128 = handle.data_str("started").and_then(|s| s.parse().ok()).unwrap_or(0);
        let elapsed = now_ms().saturating_sub(started);
        if elapsed < 800 {
            return Ok(PollStatus::Queued { position: Some(0) });
        }
        if elapsed < RUN_MS {
            return Ok(PollStatus::Running { progress: Some(elapsed as f32 / RUN_MS as f32), message: Some("Painting".into()) });
        }
        let prompt = handle.data_str("prompt").unwrap_or("");
        if handle.data_str("model") == Some("video") {
            let secs = handle.data.get("seconds").and_then(|v| v.as_u64()).unwrap_or(2) as u32;
            return Ok(PollStatus::Done(vec![Output::Bytes { data: video(prompt, secs).await?, mime: "video/mp4".into() }]));
        }
        let (w, h) = (
            handle.data.get("w").and_then(|v| v.as_u64()).unwrap_or(768) as u32,
            handle.data.get("h").and_then(|v| v.as_u64()).unwrap_or(768) as u32,
        );
        let n = handle.data.get("n").and_then(|v| v.as_u64()).unwrap_or(1).max(1);
        Ok(PollStatus::Done((0..n).map(|i| Output::Bytes { data: png(&format!("{prompt}{i}"), w, h), mime: "image/png".into() }).collect()))
    }

    fn poll_interval(&self, _attempt: u32) -> std::time::Duration {
        std::time::Duration::from_millis(400)
    }
}
