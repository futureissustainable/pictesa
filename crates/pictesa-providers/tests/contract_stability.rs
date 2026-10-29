//! Stability AI (Stable Image v2beta): the adapter against the documented API, replayed offline.
//! Docs: https://docs.stability.ai/llms.txt (Markdown and OpenAPI of each reference page).
mod common;
mod form_parts;

use common::{start, Call, Fake, Reply, PNG_1X1};
use form_parts::{fields, files, parts};
use pictesa_core::{Ctx, GenerationRequest, InputRole, MediaInput, MediaKind, ModelSpec, Output, PollStatus, Provider, ProviderError, Submission};
use pictesa_providers::stability::Stability;
use serde_json::{json, Value};

const GENERATION: &str = "a6dc6c6e20acda010fe14d71f180658f2896ed9b4ec25aa99a6ff06c796987c4";

fn ctx(fake: &Fake) -> Ctx {
    fake.ctx(&["api.stability.ai"], &[("api_key", "test-key")])
}

fn model(id: &str) -> ModelSpec {
    Stability.models().into_iter().find(|m| m.remote_id() == id).expect("model in catalog")
}

fn request(model: &str, params: Value, inputs: Vec<MediaInput>) -> GenerationRequest {
    GenerationRequest {
        job_id: "contract".into(),
        model: model.into(),
        prompt: "A lighthouse on a cliff at sunrise".into(),
        negative_prompt: None,
        params: params.as_object().cloned().unwrap_or_default(),
        inputs,
        num_outputs: 1,
    }
}


/// A file with these bytes, named by their hash so reruns reuse it; written by rename so a test
/// running in parallel never reads it half-written.
fn file(bytes: &[u8]) -> std::path::PathBuf {
    let path = std::env::temp_dir().join(format!("pictesa-contract-{:08x}.png", crc32fast::hash(bytes)));
    if std::fs::read(&path).ok().as_deref() != Some(bytes) {
        let tmp = path.with_extension(format!("{}.tmp", ulid::Ulid::new()));
        std::fs::write(&tmp, bytes).unwrap();
        std::fs::rename(&tmp, &path).unwrap();
    }
    path
}
fn input(role: InputRole, bytes: &[u8]) -> MediaInput {
    MediaInput { role, kind: MediaKind::Image, mime: "image/png".into(), path: file(bytes), url: None, width: None, height: None }
}

// Raw image answer with `finish-reason` and `seed` headers, "Responses" in
// https://docs.stability.ai/api-reference.md
fn image(mime: &str, finish: &str) -> Reply {
    Reply::bytes(mime, PNG_1X1).header("finish-reason", finish).header("seed", "343940597")
}

async fn submit(fake: &Fake, id: &str, params: Value, inputs: Vec<MediaInput>) -> Result<Submission, ProviderError> {
    Stability.submit(&ctx(fake), &model(id), &request(id, params, inputs)).await
}

fn field<'a>(sent: &'a std::collections::BTreeMap<String, String>, key: &str) -> Option<&'a str> {
    sent.get(key).map(String::as_str)
}

fn image_parts(call: &Call, name: &str) -> usize {
    files(&parts(call), name).len()
}

#[tokio::test]
async fn check_reads_the_balance_with_a_bearer_key() {
    // https://docs.stability.ai/api-reference/user/account-balance.md
    let fake = start(|_| Reply::json(json!({ "credits": 0.6336833840314097 }))).await;
    Stability.check(&ctx(&fake)).await.expect("check");
    let call = &fake.calls()[0];
    assert_eq!((call.method.as_str(), call.path.as_str()), ("GET", "/v1/user/balance"));
    assert_eq!(call.header("authorization"), Some("Bearer test-key"));
}

#[tokio::test]
async fn core_generation_returns_the_image_bytes_it_was_sent() {
    // https://docs.stability.ai/api-reference/generate/stable-image-core.md
    let fake = start(|_| image("image/webp", "SUCCESS")).await;
    let Submission::Done(outputs) = submit(&fake, "core", json!({ "aspect_ratio": "16:9", "output_format": "webp", "seed": 7 }), vec![]).await.expect("submit") else { panic!("synchronous") };
    let call = &fake.calls()[0];
    assert_eq!((call.method.as_str(), call.path.as_str()), ("POST", "/v2beta/stable-image/generate/core"));
    assert_eq!(call.header("authorization"), Some("Bearer test-key"));
    assert_eq!(call.header("accept"), Some("image/*"));
    let sent = fields(call);
    assert_eq!(field(&sent, "prompt"), Some("A lighthouse on a cliff at sunrise"));
    assert_eq!((field(&sent, "aspect_ratio"), field(&sent, "output_format"), field(&sent, "seed")), (Some("16:9"), Some("webp"), Some("7")));
    assert!(matches!(&outputs[..], [Output::Bytes { data, mime }] if data == PNG_1X1 && mime == "image/webp"));
}

#[tokio::test]
async fn image_to_image_sends_the_fields_each_endpoint_requires() {
    let fake = start(|_| image("image/png", "SUCCESS")).await;
    // Ultra: "The `strength` parameter is required when `image` is provided."
    // https://docs.stability.ai/api-reference/generate/stable-image-ultra.md
    submit(&fake, "ultra", json!({ "aspect_ratio": "1:1", "output_format": "png" }), vec![input(InputRole::Reference, PNG_1X1)]).await.expect("ultra");
    // SD 3.5 image-to-image needs `mode: image-to-image`, `image` and `strength`; `aspect_ratio` "is only
    // valid for text-to-image requests". https://docs.stability.ai/api-reference/generate/stable-diffusion-35.md
    submit(&fake, "sd3.5-large", json!({ "aspect_ratio": "1:1", "output_format": "png", "strength": 0.6 }), vec![input(InputRole::Reference, PNG_1X1)]).await.expect("sd3.5 i2i");
    submit(&fake, "sd3.5-large", json!({ "aspect_ratio": "16:9", "output_format": "png" }), vec![]).await.expect("sd3.5 t2i");

    let calls = fake.calls();
    let (ultra, i2i, t2i) = (&calls[0], &calls[1], &calls[2]);
    assert_eq!(ultra.path, "/v2beta/stable-image/generate/ultra");
    assert_eq!(image_parts(ultra, "image"), 1);
    assert!(field(&fields(ultra), "strength").and_then(|s| s.parse::<f64>().ok()).is_some_and(|s| (0.0..=1.0).contains(&s)), "Ultra needs strength with an image");

    assert_eq!(i2i.path, "/v2beta/stable-image/generate/sd3");
    let sent = fields(i2i);
    assert_eq!((field(&sent, "mode"), field(&sent, "model"), field(&sent, "strength")), (Some("image-to-image"), Some("sd3.5-large"), Some("0.6")));
    assert_eq!(image_parts(i2i, "image"), 1);
    assert_eq!(field(&sent, "aspect_ratio"), None);

    let sent = fields(t2i);
    assert_ne!(field(&sent, "mode"), Some("image-to-image"));
    assert_eq!((field(&sent, "aspect_ratio"), field(&sent, "model")), (Some("16:9"), Some("sd3.5-large")));
    assert_eq!(image_parts(t2i, "image"), 0);
}

#[tokio::test]
async fn edits_send_the_image_mask_and_search_prompt() {
    let fake = start(|_| image("image/png", "SUCCESS")).await;
    let mask = input(InputRole::Mask, b"\x89PNG mask bytes");
    submit(&fake, "inpaint", json!({ "output_format": "png" }), vec![input(InputRole::Reference, PNG_1X1), mask]).await.expect("inpaint");
    submit(&fake, "search-and-replace", json!({ "search_prompt": "the boat", "output_format": "png" }), vec![input(InputRole::Reference, PNG_1X1)]).await.expect("search and replace");

    let calls = fake.calls();
    // `mask`: "Completely black pixels represent no inpainting strength while completely white pixels
    // represent maximum strength", the same polarity as Pictesa masks, so it goes as is.
    // https://docs.stability.ai/api-reference/edit/inpaint.md
    assert_eq!(calls[0].path, "/v2beta/stable-image/edit/inpaint");
    let form = parts(&calls[0]);
    assert_eq!(files(&form, "image").len(), 1);
    assert_eq!(files(&form, "mask").iter().map(|p| p.data.as_slice()).collect::<Vec<_>>(), vec![&b"\x89PNG mask bytes"[..]]);
    assert_eq!(field(&fields(&calls[0]), "prompt"), Some("A lighthouse on a cliff at sunrise"));
    // https://docs.stability.ai/api-reference/edit/search-and-replace.md
    assert_eq!(calls[1].path, "/v2beta/stable-image/edit/search-and-replace");
    assert_eq!(field(&fields(&calls[1]), "search_prompt"), Some("the boat"));
}

#[tokio::test]
async fn fast_and_conservative_upscales_answer_with_the_image() {
    // Both are synchronous: the `accept` header picks raw bytes or base64 JSON of the image.
    // https://docs.stability.ai/api-reference/upscale/fast.md, https://docs.stability.ai/api-reference/upscale/conservative.md
    let fake = start(|_| image("image/png", "SUCCESS")).await;
    for id in ["fast", "conservative"] {
        let done = submit(&fake, id, json!({ "output_format": "png" }), vec![input(InputRole::Reference, PNG_1X1)]).await.unwrap_or_else(|e| panic!("{id}: {e}"));
        assert!(matches!(done, Submission::Done(ref o) if matches!(&o[..], [Output::Bytes { data, .. }] if data == PNG_1X1)), "{id}: {done:?}");
    }
    let calls = fake.calls();
    assert_eq!(calls[0].path, "/v2beta/stable-image/upscale/fast");
    assert_eq!(field(&fields(&calls[0]), "prompt"), None, "fast upscale takes no prompt");
    assert_eq!(calls[1].path, "/v2beta/stable-image/upscale/conservative");
    assert_eq!(field(&fields(&calls[1]), "prompt"), Some("A lighthouse on a cliff at sunrise"), "conservative requires a prompt");
}

#[tokio::test]
async fn creative_upscale_is_polled_at_the_results_endpoint() {
    // Submit answers `{ "id" }`; poll `GET /v2beta/results/{id}`: 202 in progress, 200 the image.
    // https://docs.stability.ai/api-reference/upscale/creative-upscale-async.md,
    // https://docs.stability.ai/api-reference/results/fetch-async-generation-result.md
    let polls = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let seen = polls.clone();
    let fake = start(move |call| {
        if call.method == "POST" {
            Reply::json(json!({ "id": GENERATION }))
        } else if seen.fetch_add(1, std::sync::atomic::Ordering::SeqCst) == 0 {
            Reply { status: 202, ..Reply::json(json!({ "id": GENERATION, "status": "in-progress" })) }
        } else {
            image("image/png", "SUCCESS")
        }
    })
    .await;
    let Submission::Pending(handle) = submit(&fake, "creative", json!({ "output_format": "png" }), vec![input(InputRole::Reference, PNG_1X1)]).await.expect("submit") else { panic!("async") };
    assert_eq!(fake.calls()[0].path, "/v2beta/stable-image/upscale/creative");

    assert!(matches!(Stability.poll(&ctx(&fake), &handle).await.expect("poll"), PollStatus::Running { .. }));
    let PollStatus::Done(outputs) = Stability.poll(&ctx(&fake), &handle).await.expect("poll") else { panic!("200 is done") };
    assert!(matches!(&outputs[..], [Output::Bytes { data, mime }] if data == PNG_1X1 && mime == "image/png"));
    for call in &fake.calls()[1..] {
        assert_eq!((call.method.as_str(), call.path.as_str()), ("GET", format!("/v2beta/results/{GENERATION}").as_str()));
        assert_eq!(call.header("authorization"), Some("Bearer test-key"));
        assert_eq!(call.header("accept"), Some("image/*"));
    }
    // "Rate-limiting or other errors may occur if you poll more than once every 10 seconds."
    assert!(Stability.poll_interval(0) >= std::time::Duration::from_secs(10));
}

#[tokio::test]
async fn a_content_filtered_result_is_a_policy_refusal_not_an_image() {
    // `finish-reason: CONTENT_FILTERED`: "the output violated the content moderation policy and was
    // blurred", https://docs.stability.ai/api-reference.md ("Responses")
    let fake = start(|_| image("image/png", "CONTENT_FILTERED")).await;
    let err = submit(&fake, "core", json!({}), vec![]).await.expect_err("blurred output");
    assert_eq!(err.kind(), "content_policy");
}

#[tokio::test]
async fn documented_errors_map_to_their_meaning() {
    // Status table and Error bodies: https://docs.stability.ai/api-reference.md ("Errors") and the
    // response components of https://docs.stability.ai/api-reference/generate/stable-image-ultra.md.
    // 401 example from https://docs.stability.ai/api-reference/user/account-balance.md; the 402 body
    // is not documented and carries the documented meaning.
    let cases = [
        (400, json!({ "id": "a1b2c3d4e5f6a1b2c3d4e5f6a1b2c3d4", "name": "bad_request", "errors": ["prompt: is required"] }), "invalid_input"),
        (401, json!({ "id": "9160aa70-222f-4a36-9eb7-475e2668362a", "name": "unauthorized", "message": "missing authorization header" }), "auth"),
        (402, json!({ "id": "a1b2c3d4e5f6a1b2c3d4e5f6a1b2c3d4", "name": "payment_required", "errors": ["Your account doesn't have enough credits."] }), "insufficient_credits"),
        (403, json!({ "id": "ed14db44362126aab3cbd25cca51ffe3", "name": "content_moderation", "errors": ["Your request was flagged by our content moderation system, as a result your request was denied and you were not charged."] }), "content_policy"),
        (413, json!({ "id": "4212a4b66fbe1cedca4bf2133d35dca5", "name": "payload_too_large", "errors": ["body: payloads cannot be larger than 10MiB in size"] }), "invalid_input"),
        (422, json!({ "id": "ff54b236a3acdde1522cb1ba641c43ed", "name": "public_figure", "errors": ["Our system detected the likeness of a public figure in your image. To comply with our guidelines, this request cannot be processed. Please upload a different image."] }), "content_policy"),
        (422, json!({ "id": "ff54b236a3acdde1522cb1ba641c43ed", "name": "invalid_language", "errors": ["English is the only supported language for this service."] }), "invalid_input"),
        (429, json!({ "id": "rate_limit_exceeded", "name": "rate_limit_exceeded", "errors": ["You have exceeded the rate limit of 150 requests within a 10 second period, and have been timed out for 60 seconds."] }), "rate_limited"),
        (500, json!({ "id": "2a1b2d4eafe2bc6ab4cd4d5c6133f513", "name": "internal_error", "errors": ["An unexpected server error has occurred, please try again later."] }), "transient"),
    ];
    let mut wrong = Vec::new();
    for (status, body, want) in cases {
        let name = body["name"].as_str().unwrap().to_string();
        let fake = start(move |_| Reply::status(status, body.clone())).await;
        let err = submit(&fake, "ultra", json!({}), vec![]).await.expect_err("error status");
        if err.kind() != want {
            wrong.push(format!("HTTP {status} {name}: {}, want {want}", err.kind()));
        }
    }
    assert!(wrong.is_empty(), "{wrong:#?}");
}
