//! End-to-end checks against a running ComfyUI server. Ignored by default because they need one:
//!
//!   COMFYUI_URL=http://127.0.0.1:8188 cargo test -p pictesa-providers --test comfyui_live -- --ignored --nocapture
//!
//! `esrgan_upscale` needs `RealESRGAN_x4plus.safetensors` in `models/upscale_models` (67 MB).
//! `text_to_image` runs the template named by `COMFYUI_LIVE_T2I` (for example a one-step SD 1.5-class
//! checkpoint kept in `PICTESA_WORKFLOWS_DIR`) and skips itself when that variable is unset.
//! Both use only the local server: nothing is billed.

use std::{collections::HashMap, path::PathBuf, time::Duration};

use pictesa_core::{Ctx, GenerationRequest, InputRole, MediaInput, MediaKind, ModelSpec, Output, PollStatus, Provider, ProviderError, Submission};
use pictesa_providers::comfyui::ComfyUi;
use serde_json::{json, Value};

fn ctx() -> Ctx {
    let mut creds = HashMap::new();
    for (key, env) in [("base_url", "COMFYUI_URL"), ("api_key", "COMFYUI_API_KEY"), ("workflows_dir", "PICTESA_WORKFLOWS_DIR")] {
        if let Ok(v) = std::env::var(env) {
            creds.insert(key.to_string(), v);
        }
    }
    Ctx { http: reqwest::Client::builder().user_agent("pictesa-test").build().unwrap(), creds }
}

fn request(model: &ModelSpec, prompt: &str, params: Value, inputs: Vec<MediaInput>, n: u32) -> GenerationRequest {
    GenerationRequest {
        job_id: "live".into(),
        model: model.remote_id().into(),
        prompt: prompt.into(),
        negative_prompt: None,
        params: params.as_object().cloned().unwrap_or_default(),
        inputs,
        num_outputs: n,
    }
}

async fn find(p: &ComfyUi, ctx: &Ctx, id: &str) -> ModelSpec {
    let found = p.discover(ctx).await.expect("discover");
    let spec = found.into_iter().find(|m| m.id == id).unwrap_or_else(|| panic!("{id} not discovered"));
    println!("{id}: tags {:?}, missing models {}", spec.tags, spec.meta["missing_models"]);
    spec
}

/// Submits and polls to the end. Returns the outputs and every status seen on the way.
async fn run(p: &ComfyUi, ctx: &Ctx, spec: &ModelSpec, req: &GenerationRequest) -> (Vec<Output>, Vec<String>) {
    let handle = match p.submit(ctx, spec, req).await.expect("submit") {
        Submission::Pending(h) => h,
        Submission::Done(_) => panic!("ComfyUI should queue"),
    };
    let mut seen = Vec::new();
    for attempt in 0..1200 {
        tokio::time::sleep(p.poll_interval(attempt).min(Duration::from_millis(400))).await;
        match p.poll(ctx, &handle).await.expect("poll") {
            PollStatus::Done(out) => return (out, seen),
            PollStatus::Failed(e) => panic!("run failed: {e}"),
            other => {
                let s = format!("{other:?}");
                if seen.last() != Some(&s) {
                    println!("  {s}");
                    seen.push(s);
                }
            }
        }
    }
    panic!("timed out")
}

async fn fetch(ctx: &Ctx, out: &Output) -> Vec<u8> {
    match out {
        Output::Url { url, headers } => {
            let mut rb = ctx.http.get(url);
            for (k, v) in headers {
                rb = rb.header(k, v);
            }
            let resp = rb.send().await.expect("download");
            assert!(resp.status().is_success(), "download {url}: {}", resp.status());
            resp.bytes().await.unwrap().to_vec()
        }
        Output::Bytes { data, .. } => data.clone(),
    }
}

fn png_size(b: &[u8]) -> (u32, u32) {
    assert!(b.starts_with(b"\x89PNG\r\n\x1a\n"), "not a PNG");
    (u32::from_be_bytes(b[16..20].try_into().unwrap()), u32::from_be_bytes(b[20..24].try_into().unwrap()))
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore]
async fn esrgan_upscale() {
    let (p, ctx) = (ComfyUi, ctx());
    p.check(&ctx).await.expect("check");
    let spec = find(&p, &ctx, "comfyui:esrgan-upscale").await;
    assert!(!spec.tags.iter().any(|t| t.starts_with("needs-")), "server lacks the upscaler: {:?}", spec.meta);
    let input = MediaInput {
        role: InputRole::Reference,
        kind: MediaKind::Image,
        mime: "image/png".into(),
        path: PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/comfyui_input_64x48.png"),
        url: None,
        width: None,
        height: None,
    };
    for (scale, expect) in [(4, (256, 192)), (2, (128, 96))] {
        let req = request(&spec, "", json!({ "scale": scale }), vec![input.clone()], 1);
        let (out, seen) = run(&p, &ctx, &spec, &req).await;
        assert_eq!(out.len(), 1);
        let bytes = fetch(&ctx, &out[0]).await;
        assert_eq!(png_size(&bytes), expect, "{scale}x");
        println!("esrgan {scale}x ok: {} bytes, statuses {seen:?}", bytes.len());
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore]
async fn text_to_image() {
    let Ok(template) = std::env::var("COMFYUI_LIVE_T2I") else {
        println!("COMFYUI_LIVE_T2I not set; skipped");
        return;
    };
    let (p, ctx) = (ComfyUi, ctx());
    let spec = find(&p, &ctx, &format!("comfyui:{template}")).await;
    assert!(!spec.tags.iter().any(|t| t.starts_with("needs-")), "server lacks files: {:?}", spec.meta);

    // Two images in one run, landscape.
    let req = request(&spec, "a lighthouse on a cliff at sunset, oil painting", json!({ "aspect_ratio": "16:9", "seed": 42, "steps": 4 }), vec![], 2);
    let (out, seen) = run(&p, &ctx, &spec, &req).await;
    assert_eq!(out.len(), 2, "batch of two");
    for o in &out {
        let (w, h) = png_size(&fetch(&ctx, o).await);
        assert!(w > h, "landscape, got {w}x{h}");
    }
    println!("text to image ok: statuses {seen:?}");

    // A queued job can be cancelled while another one runs.
    let first = request(&spec, "a red bicycle", json!({ "seed": 1 }), vec![], 1);
    let second = request(&spec, "a blue bicycle", json!({ "seed": 2 }), vec![], 1);
    let Submission::Pending(h1) = p.submit(&ctx, &spec, &first).await.unwrap() else { panic!() };
    let Submission::Pending(h2) = p.submit(&ctx, &spec, &second).await.unwrap() else { panic!() };
    let before = p.poll(&ctx, &h2).await.unwrap();
    println!("second job before cancel: {before:?}");
    p.cancel(&ctx, &h2).await.unwrap();
    let after = p.poll(&ctx, &h2).await.unwrap();
    assert!(matches!(&after, PollStatus::Failed(ProviderError::Failed(m)) if m == "cancelled"), "cancelled job reports cancelled, got {after:?}");
    loop {
        tokio::time::sleep(Duration::from_millis(400)).await;
        match p.poll(&ctx, &h1).await.unwrap() {
            PollStatus::Done(out) => {
                assert_eq!(out.len(), 1);
                break;
            }
            PollStatus::Failed(e) => panic!("first job failed: {e}"),
            _ => {}
        }
    }
    println!("cancel ok: {after:?}");
}
