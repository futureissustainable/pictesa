//! Ideogram 3.0 (v1 API): the adapter against the documented endpoints, replayed offline.
//! Docs: https://developer.ideogram.ai/v1/llms.txt (Markdown of each reference page).
mod common;
mod form_parts;

use common::{start, Fake, Reply, PNG_1X1};
use form_parts::{fields, files, parts};
use pictesa_core::{Ctx, GenerationRequest, InputRole, MediaInput, MediaKind, ModelSpec, Output, Provider, ProviderError, Submission};
use pictesa_providers::ideogram::Ideogram;
use serde_json::{json, Value};

fn ctx(fake: &Fake) -> Ctx {
    fake.ctx(&["api.ideogram.ai"], &[("api_key", "test-key")])
}

fn model() -> ModelSpec {
    Ideogram.models().remove(0)
}

fn request(params: Value, inputs: Vec<MediaInput>, num_outputs: u32) -> GenerationRequest {
    GenerationRequest {
        job_id: "contract".into(),
        model: "ideogram-v3".into(),
        prompt: "A photo of a cat sleeping on a couch.".into(),
        negative_prompt: None,
        params: params.as_object().cloned().unwrap_or_default(),
        inputs,
        num_outputs,
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
    let path = file(bytes);
    MediaInput { role, kind: MediaKind::Image, mime: "image/png".into(), path, url: None, width: None, height: None }
}

// ImageGenerationObjectV3 items, response example of
// https://developer.ideogram.ai/v1/api-reference/generate-images/generate-v3
fn image(n: u32, safe: bool) -> Value {
    json!({
        "prompt": "A photo of a cat sleeping on a couch.",
        "resolution": "1024x1024",
        "is_image_safe": safe,
        "seed": 12345,
        "url": if safe { json!(format!("https://ideogram.ai/api/images/ephemeral/xtdZiqPwRxqY1Y7NExFmz{n}.png?exp=1743867804&sig=e13e1267")) } else { Value::Null },
        "style_type": "GENERAL"
    })
}

fn generated(images: Vec<Value>) -> Reply {
    Reply::json(json!({ "created": "2026-10-29T12:00:00Z", "data": images }))
}

async fn submit(fake: &Fake, req: &GenerationRequest) -> Result<Submission, ProviderError> {
    Ideogram.submit(&ctx(fake), &model(), req).await
}

#[tokio::test]
async fn check_lists_datasets_with_the_api_key_header() {
    // https://developer.ideogram.ai/v1/api-reference/custom-model-training/list-datasets
    let fake = start(|_| Reply::json(json!({ "datasets": [{ "dataset_id": "dataset_id", "name": "name", "user_id": "user_id", "creation_time": "2024-01-15T09:30:00Z" }] }))).await;
    Ideogram.check(&ctx(&fake)).await.expect("check");
    let call = &fake.calls()[0];
    assert_eq!((call.method.as_str(), call.path.as_str()), ("GET", "/datasets"));
    assert_eq!(call.header("api-key"), Some("test-key"));
}

#[tokio::test]
async fn generate_sends_documented_fields_and_returns_every_image() {
    // https://developer.ideogram.ai/v1/api-reference/generate-images/generate-v3
    let fake = start(|_| generated(vec![image(1, true), image(2, true)])).await;
    let req = request(json!({ "aspect_ratio": "16:9", "quality": "TURBO", "style": "REALISTIC", "seed": 42 }), vec![], 2);
    let Submission::Done(outputs) = submit(&fake, &req).await.expect("submit") else { panic!("synchronous") };

    let call = &fake.calls()[0];
    assert_eq!((call.method.as_str(), call.path.as_str()), ("POST", "/v1/ideogram-v3/generate"));
    assert_eq!(call.header("api-key"), Some("test-key"));
    let sent = fields(call);
    let field = |k: &str| sent.get(k).map(String::as_str);
    assert_eq!(field("prompt"), Some("A photo of a cat sleeping on a couch."));
    assert_eq!(field("aspect_ratio"), Some("16x9"), "Ideogram writes ratios as WxH");
    assert_eq!((field("rendering_speed"), field("style_type")), (Some("TURBO"), Some("REALISTIC")));
    assert_eq!((field("num_images"), field("seed")), (Some("2"), Some("42")));
    assert_eq!(outputs.len(), 2);
    assert!(outputs.iter().all(|o| matches!(o, Output::Url { url, .. } if url.starts_with("https://ideogram.ai/api/images/ephemeral/"))));
}

#[tokio::test]
async fn remix_uploads_the_image_and_style_references_as_files() {
    // Body (multipart/form-data): `image` file, `image_weight`, `style_reference_images` files,
    // https://developer.ideogram.ai/v1/api-reference/edit-images/remix-v3
    let fake = start(|_| generated(vec![image(1, true)])).await;
    let inputs = vec![input(InputRole::StartFrame, PNG_1X1), input(InputRole::Reference, PNG_1X1), input(InputRole::Reference, PNG_1X1)];
    submit(&fake, &request(json!({ "reference_type": "style", "strength": 70 }), inputs, 1)).await.expect("submit");

    let call = &fake.calls()[0];
    assert_eq!(call.path, "/v1/ideogram-v3/remix");
    let form = parts(call);
    let image = files(&form, "image");
    assert_eq!(image.len(), 1);
    assert_eq!((image[0].data.as_slice(), image[0].content_type.as_deref()), (PNG_1X1, Some("image/png")));
    assert_eq!(files(&form, "style_reference_images").len(), 2);
    assert!(files(&form, "character_reference_images").is_empty());
    assert_eq!(fields(call).get("image_weight").map(String::as_str), Some("70"));
}

#[tokio::test]
async fn edit_sends_the_mask_black_where_pictesa_paints_white() {
    // `mask`: "Black regions in the mask should match up with the regions of the image that you
    // would like to edit", https://developer.ideogram.ai/v1/api-reference/legacy-endpoints/edit-v3.
    // Pictesa masks are white where to change (InputRole::Mask).
    let mut mask = image::GrayImage::new(2, 1);
    mask.put_pixel(0, 0, image::Luma([255])); // change
    mask.put_pixel(1, 0, image::Luma([0])); // keep
    let mut png = std::io::Cursor::new(Vec::new());
    mask.write_to(&mut png, image::ImageFormat::Png).unwrap();

    let fake = start(|_| generated(vec![image(1, true)])).await;
    submit(&fake, &request(json!({}), vec![input(InputRole::StartFrame, PNG_1X1), input(InputRole::Mask, png.get_ref())], 1)).await.expect("submit");

    let call = &fake.calls()[0];
    assert_eq!(call.path, "/v1/ideogram-v3/edit");
    let form = parts(call);
    assert_eq!(files(&form, "image").len(), 1);
    let sent = files(&form, "mask");
    assert_eq!(sent.len(), 1, "one mask file");
    let sent = image::load_from_memory(&sent[0].data).expect("mask is an image").to_luma8();
    assert_eq!((sent.get_pixel(0, 0).0[0], sent.get_pixel(1, 0).0[0]), (0, 255), "painted area is black for Ideogram, the rest white");
}

#[tokio::test]
async fn images_that_fail_the_safety_check_are_dropped_and_none_left_is_a_policy_refusal() {
    // `is_image_safe`: "If false, the url field will be empty."
    // https://developer.ideogram.ai/v1/api-reference/generate-images/generate-v3 (ImageGenerationObjectV3)
    let fake = start(|_| generated(vec![image(1, true), image(2, false)])).await;
    let Submission::Done(outputs) = submit(&fake, &request(json!({}), vec![], 2)).await.expect("the safe image is delivered") else { panic!("synchronous") };
    assert_eq!(outputs.len(), 1);

    let fake = start(|_| generated(vec![image(1, false)])).await;
    let err = submit(&fake, &request(json!({}), vec![], 1)).await.expect_err("nothing to deliver");
    assert_eq!(err.kind(), "content_policy");
}

#[tokio::test]
async fn documented_errors_map_to_their_meaning() {
    // Errors sections of the generate, remix and edit pages
    // (https://developer.ideogram.ai/v1/api-reference/generate-images/generate-v3). Bodies are typed
    // `any` except 422's `error` string; they carry the documented description.
    let cases = [
        (400, json!({ "detail": "Invalid input provided." }), "invalid_input"),
        (401, json!({ "detail": "Not authorized to generate an image." }), "auth"),
        (403, json!({ "detail": "Not authorized to generate an image." }), "auth"),
        (422, json!({ "error": "Prompt failed the safety check." }), "content_policy"),
        (429, json!({ "detail": "Too many requests." }), "rate_limited"),
        // 5xx is not documented; the shared mapping retries it.
        (500, json!({ "detail": "Internal server error" }), "transient"),
    ];
    let mut wrong = Vec::new();
    for (status, body, want) in cases {
        let fake = start(move |_| Reply::status(status, body.clone())).await;
        let err = submit(&fake, &request(json!({}), vec![], 1)).await.expect_err("error status");
        if err.kind() != want {
            wrong.push(format!("HTTP {status}: {}, want {want}", err.kind()));
        }
    }
    assert!(wrong.is_empty(), "{wrong:#?}");
}
