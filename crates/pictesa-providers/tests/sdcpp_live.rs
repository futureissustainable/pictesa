//! End-to-end checks against a running sd-server (stable-diffusion.cpp). Ignored by default:
//!
//!   SDCPP_URL=http://127.0.0.1:1234 cargo test -p pictesa-providers --test sdcpp_live -- --ignored --nocapture
//!
//! Start sd-server with any image model (a one-step SDXS checkpoint runs on CPU) and, for the
//! upscale check, `--hires-upscalers-dir` holding an ESRGAN model. Nothing is billed.

use std::{collections::HashMap, path::PathBuf, time::Duration};

use pictesa_core::{Ctx, GenerationRequest, InputRole, MediaInput, MediaKind, Output, PollStatus, Provider, ProviderError, Submission};
use pictesa_providers::sdcpp::SdCpp;
use serde_json::{json, Value};

fn ctx() -> Ctx {
    let mut creds = HashMap::new();
    if let Ok(v) = std::env::var("SDCPP_URL") {
        creds.insert("base_url".to_string(), v);
    } else {
        creds.insert("base_url".to_string(), "http://127.0.0.1:1234".to_string());
    }
    Ctx::new(reqwest::Client::new(), creds)
}

fn request(model: &str, prompt: &str, params: Value, inputs: Vec<MediaInput>, n: u32) -> GenerationRequest {
    GenerationRequest {
        job_id: "live".into(),
        model: model.into(),
        prompt: prompt.into(),
        negative_prompt: None,
        params: params.as_object().cloned().unwrap_or_default(),
        inputs,
        num_outputs: n,
    }
}

fn png_size(b: &[u8]) -> (u32, u32) {
    assert!(b.starts_with(b"\x89PNG\r\n\x1a\n"), "not a PNG");
    (u32::from_be_bytes(b[16..20].try_into().unwrap()), u32::from_be_bytes(b[20..24].try_into().unwrap()))
}

#[tokio::test]
#[ignore]
async fn generate_upscale_and_cancel() {
    let (p, ctx) = (SdCpp, ctx());
    p.check(&ctx).await.expect("check");
    let models = p.discover(&ctx).await.expect("discover");
    println!("models: {:?}", models.iter().map(|m| &m.id).collect::<Vec<_>>());
    let image = models.iter().find(|m| m.output == MediaKind::Image && !m.id.contains(":upscale-")).expect("an image model is loaded");

    // Text to image, two at once, landscape.
    let req = request(image.remote_id(), "a lighthouse on a cliff at sunset", json!({ "aspect_ratio": "16:9", "seed": 42 }), vec![], 2);
    let Submission::Pending(handle) = p.submit(&ctx, image, &req).await.expect("submit") else { panic!("sd-server queues jobs") };
    let mut seen = Vec::new();
    let out = loop {
        tokio::time::sleep(Duration::from_millis(300)).await;
        match p.poll(&ctx, &handle).await.expect("poll") {
            PollStatus::Done(out) => break out,
            PollStatus::Failed(e) => panic!("failed: {e}"),
            s => {
                let s = format!("{s:?}");
                if seen.last() != Some(&s) {
                    seen.push(s);
                }
            }
        }
    };
    assert_eq!(out.len(), 2);
    for o in &out {
        let Output::Bytes { data, mime } = o else { panic!("expected bytes") };
        assert_eq!(mime, "image/png");
        let (w, h) = png_size(data);
        assert!(w > h, "landscape, got {w}x{h}");
    }
    println!("generated 2 images; statuses {seen:?}");

    // ESRGAN upscale answers synchronously.
    if let Some(up) = models.iter().find(|m| m.id.contains(":upscale-")) {
        let input = MediaInput {
            role: InputRole::Reference,
            kind: MediaKind::Image,
            mime: "image/png".into(),
            path: PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/comfyui_input_64x48.png"),
            url: None,
            width: None,
            height: None,
        };
        let Submission::Done(out) = p.submit(&ctx, up, &request(up.remote_id(), "", json!({}), vec![input], 1)).await.expect("upscale") else {
            panic!("upscale is synchronous")
        };
        let Output::Bytes { data, .. } = &out[0] else { panic!() };
        assert_eq!(png_size(data), (256, 192));
        println!("upscaled 64x48 to 256x192");
    }

    // A queued job can be cancelled.
    let a = request(image.remote_id(), "a red bicycle", json!({}), vec![], 1);
    let b = request(image.remote_id(), "a blue bicycle", json!({}), vec![], 1);
    let Submission::Pending(h1) = p.submit(&ctx, image, &a).await.unwrap() else { panic!() };
    let Submission::Pending(h2) = p.submit(&ctx, image, &b).await.unwrap() else { panic!() };
    println!("second job before cancel: {:?}", p.poll(&ctx, &h2).await.unwrap());
    p.cancel(&ctx, &h2).await.unwrap();
    let mut after = p.poll(&ctx, &h2).await.unwrap();
    for _ in 0..50 {
        if matches!(after, PollStatus::Failed(_) | PollStatus::Done(_)) {
            break;
        }
        tokio::time::sleep(Duration::from_millis(200)).await;
        after = p.poll(&ctx, &h2).await.unwrap();
    }
    assert!(matches!(&after, PollStatus::Failed(ProviderError::Failed(m)) if m == "cancelled"), "{after:?}");
    loop {
        tokio::time::sleep(Duration::from_millis(300)).await;
        match p.poll(&ctx, &h1).await.unwrap() {
            PollStatus::Done(_) => break,
            PollStatus::Failed(e) => panic!("first job failed: {e}"),
            _ => {}
        }
    }
    println!("cancel ok");
}
