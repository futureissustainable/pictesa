//! Leonardo contract: the documented answers of cloud.leonardo.ai replayed through the adapter, and
//! what the adapter sends checked against the published API.
//! Docs: https://docs.leonardo.ai/v1.0/llms.txt (v1 REST reference and model guides) and
//! https://docs.leonardo.ai/llms.txt (v2 models, errors, deprecations), read 29.10.2026.

mod common;

use common::{start, Reply};
use pictesa_core::{GenerationRequest, MediaKind, ModelSpec, Output, PollStatus, Provider, ProviderError, Submission};
use pictesa_providers::leonardo::Leonardo;
use serde_json::{json, Value};

const HOST: &str = "cloud.leonardo.ai";

fn spec(id: &str) -> ModelSpec {
    Leonardo.models().into_iter().find(|m| m.remote_id() == id).unwrap()
}

/// A request as the engine hands it over: params checked and filled with the model's defaults.
fn request(model: &ModelSpec, params: Value, negative: Option<&str>, n: u32) -> GenerationRequest {
    let mut p = params.as_object().cloned().unwrap_or_default();
    for spec in &model.params {
        if let Some(d) = &spec.default {
            p.entry(spec.key.clone()).or_insert(d.clone());
        }
    }
    GenerationRequest { job_id: "job-1".into(), model: model.remote_id().into(), prompt: "An oil painting of a cat".into(), negative_prompt: negative.map(String::from), params: p, inputs: vec![], num_outputs: n }
}

/// Answer of POST /generations. https://docs.leonardo.ai/v1.0/reference/creategeneration.md (example)
fn image_job() -> Value {
    json!({ "sdGenerationJob": { "generationId": "a1b2c3d4-e5f6-7890-abcd-ef1234567890", "apiCreditCost": null, "cost": { "amount": "0.0147", "unit": "DOLLARS" } } })
}

/// Answer of POST /generations-text-to-video. https://docs.leonardo.ai/v1.0/reference/createtexttovideogeneration.md (example)
fn video_job() -> Value {
    json!({ "motionVideoGenerationJob": { "generationId": "j9k0l1m2-n3o4-p5q6-rstu-vw8901234567", "apiCreditCost": null, "cost": { "amount": "0.0147", "unit": "DOLLARS" } } })
}

/// A `generations_by_pk` answer. Fields from https://docs.leonardo.ai/v1.0/reference/getgenerationbyid.md
/// (`job_status`: PENDING, COMPLETE, FAILED); the image entry is the one in
/// https://docs.leonardo.ai/docs/guide-to-handling-not-safe-for-work-image-generation-nsfw.md.
fn generation(status: &str, images: Value) -> Value {
    json!({ "generations_by_pk": { "id": "a1b2c3d4-e5f6-7890-abcd-ef1234567890", "status": status, "prompt": "An oil painting of a cat", "imageHeight": 512, "imageWidth": 512, "public": false, "generated_images": images, "generation_elements": [] } })
}

#[tokio::test]
async fn check_lists_models_with_the_key_as_a_bearer_token() {
    // https://docs.leonardo.ai/reference/getmodels.md (GET /api/rest/v2/models)
    let fake = start(|_| Reply::json(json!({ "productionApiAvailableModels": [] }))).await;
    Leonardo.check(&fake.ctx(&[HOST], &[("api_key", "test-key")])).await.unwrap();
    let call = &fake.calls()[0];
    assert_eq!((call.method.as_str(), call.path.as_str()), ("GET", "/api/rest/v2/models"));
    assert_eq!(call.header("authorization"), Some("Bearer test-key"));
}

#[tokio::test]
async fn image_models_send_the_documented_v1_generation_fields() {
    // https://docs.leonardo.ai/v1.0/reference/creategeneration.md (request schema) and the v1 model
    // guides (lucid-origin, lucid-realism, phoenix, flux-dev .md), which list `contrast` as *Required*
    // with values 3, 3.5 or 4, and give each model's UUID.
    let documented = [
        "alchemy", "contrastRatio", "contrast", "controlnets", "elements", "userElements", "expandedDomain", "fantasyAvatar", "guidance_scale", "height", "highContrast", "highResolution",
        "imagePrompts", "imagePromptWeight", "init_generation_image_id", "init_image_id", "init_strength", "modelId", "negative_prompt", "num_images", "num_inference_steps", "photoReal",
        "photoRealVersion", "photoRealStrength", "presetStyle", "prompt", "promptMagic", "promptMagicStrength", "promptMagicVersion", "public", "scheduler", "sd_version", "seed", "tiling",
        "transparency", "ultra", "unzoom", "unzoomAmount", "upscaleRatio", "width", "controlNet", "controlNetType", "weighting", "canvasRequest", "canvasRequestType", "canvasInitId",
        "canvasMaskId", "enhancePrompt", "enhancePromptInstruction",
    ];
    for uuid in ["7b592283-e8a7-4c5a-9ba6-d18c31f258b9", "05ce0082-2d80-4a2d-8653-4d1c85e2418e", "de7d3faf-762f-48e0-b3b7-9d0ac3a3fcf3", "b2614463-296c-462a-9586-aafdb8f00e36"] {
        let model = spec(uuid);
        let req = request(&model, json!({ "width": 768, "height": 1024, "steps": 30, "guidance": 7.0, "seed": 42 }), Some("blur"), 2);
        let fake = start(|_| Reply::json(image_job())).await;
        let Submission::Pending(handle) = Leonardo.submit(&fake.ctx(&[HOST], &[("api_key", "test-key")]), &model, &req).await.unwrap() else { panic!() };
        assert_eq!(handle.id, "a1b2c3d4-e5f6-7890-abcd-ef1234567890");
        let call = &fake.calls()[0];
        assert_eq!((call.method.as_str(), call.path.as_str()), ("POST", "/api/rest/v1/generations"));
        assert_eq!(call.header("authorization"), Some("Bearer test-key"));
        let body = call.json();
        for key in body.as_object().unwrap().keys() {
            assert!(documented.contains(&key.as_str()), "{uuid} sends undocumented `{key}`: {body}");
        }
        assert_eq!((body["modelId"].as_str(), body["prompt"].as_str(), body["num_images"].as_u64()), (Some(uuid), Some("An oil painting of a cat"), Some(2)));
        assert_eq!((body["width"].as_u64(), body["height"].as_u64(), body["num_inference_steps"].as_u64(), body["seed"].as_u64()), (Some(768), Some(1024), Some(30), Some(42)));
        assert_eq!((body["guidance_scale"].as_f64(), body["negative_prompt"].as_str(), body["public"].as_bool()), (Some(7.0), Some("blur"), Some(false)));
        assert!(body["contrast"].as_f64().is_some_and(|c| [3.0, 3.5, 4.0].contains(&c)), "{uuid}: contrast is required: {body}");
    }
}

#[tokio::test]
async fn video_models_send_a_documented_live_text_to_video_model() {
    // https://docs.leonardo.ai/v1.0/reference/createtexttovideogeneration.md: `model` one of MOTION2,
    // VEO3, MOTION2FAST, VEO3FAST, KLING2_1, KLING2_5; resolution RESOLUTION_480 | _720 | _1080;
    // isPublic, seed, negativePrompt, duration. https://docs.leonardo.ai/docs/deprecations-changes.md:
    // "Veo 3 and Veo 3 Fast will no longer be available via the Leonardo API after June 29, 2026."
    let live = ["MOTION2", "MOTION2FAST", "KLING2_1", "KLING2_5"];
    let documented = ["prompt", "resolution", "model", "frameInterpolation", "isPublic", "seed", "negativePrompt", "promptEnhance", "promptEnhanceInstruction", "styleIds", "height", "width", "duration", "elements"];
    for model in Leonardo.models().into_iter().filter(|m| m.output == MediaKind::Video) {
        let req = request(&model, json!({ "seed": 5 }), Some("blurry"), 1);
        let fake = start(|_| Reply::json(video_job())).await;
        let Submission::Pending(handle) = Leonardo.submit(&fake.ctx(&[HOST], &[("api_key", "test-key")]), &model, &req).await.unwrap() else { panic!() };
        assert_eq!(handle.id, "j9k0l1m2-n3o4-p5q6-rstu-vw8901234567");
        let call = &fake.calls()[0];
        assert_eq!((call.method.as_str(), call.path.as_str()), ("POST", "/api/rest/v1/generations-text-to-video"));
        let body = call.json();
        assert!(live.contains(&body["model"].as_str().unwrap()), "{} is not a live Leonardo video model", body["model"]);
        for key in body.as_object().unwrap().keys() {
            assert!(documented.contains(&key.as_str()), "undocumented `{key}`: {body}");
        }
        assert_eq!((body["resolution"].as_str(), body["isPublic"].as_bool(), body["negativePrompt"].as_str(), body["seed"].as_u64()), (Some("RESOLUTION_720"), Some(false), Some("blurry"), Some(5)));
    }
}

#[tokio::test]
async fn every_documented_job_status_maps_to_a_poll_status() {
    // https://docs.leonardo.ai/v1.0/reference/getgenerationbyid.md (GET /generations/{id})
    let image = spec("7b592283-e8a7-4c5a-9ba6-d18c31f258b9");
    let video = spec("MOTION2");
    let cat = json!({ "url": "https://cdn.leonardo.ai/users/ef8b8386-94f7-48d1-b10e-e87fd4dee6e6/generations/88b381ea-7baf-457d-a5b4-8068cb6bac21/Leonardo_Creative_An_oil_painting_of_a_cat_0.jpg", "nsfw": false, "id": "5b710f5f-22de-4d27-8b5c-4eadc98bfc85", "likeCount": 0, "generated_image_variation_generics": [] });
    let clip = json!({ "url": "https://cdn.leonardo.ai/users/x/generations/y/frame.jpg", "motionMP4URL": "https://cdn.leonardo.ai/users/x/generations/y/clip.mp4", "nsfw": false, "id": "5b710f5f-22de-4d27-8b5c-4eadc98bfc86", "likeCount": 0, "generated_image_variation_generics": [] });
    let cases = [
        (&image, image_job(), generation("PENDING", json!([]))),
        (&image, image_job(), generation("COMPLETE", json!([cat]))),
        (&video, video_job(), generation("COMPLETE", json!([clip]))),
        (&image, image_job(), generation("FAILED", json!([]))),
    ];
    let mut seen = Vec::new();
    for (model, job, answer) in cases {
        let fake = start(move |call| if call.method == "POST" { Reply::json(job.clone()) } else { Reply::json(answer.clone()) }).await;
        let ctx = fake.ctx(&[HOST], &[("api_key", "test-key")]);
        let Submission::Pending(handle) = Leonardo.submit(&ctx, model, &request(model, json!({}), None, 1)).await.unwrap() else { panic!() };
        seen.push(Leonardo.poll(&ctx, &handle).await.unwrap());
        let poll = &fake.calls()[1];
        assert_eq!((poll.method.as_str(), poll.path.as_str()), ("GET", format!("/api/rest/v1/generations/{}", handle.id).as_str()));
        assert_eq!(poll.header("authorization"), Some("Bearer test-key"));
    }
    let url = |s: &PollStatus| match s {
        PollStatus::Done(o) => match o.as_slice() {
            [Output::Url { url, .. }] => url.clone(),
            other => panic!("{other:?}"),
        },
        other => panic!("not done: {other:?}"),
    };
    assert!(matches!(seen[0], PollStatus::Queued { .. }), "{:?}", seen[0]);
    assert!(url(&seen[1]).ends_with("Leonardo_Creative_An_oil_painting_of_a_cat_0.jpg"));
    assert_eq!(url(&seen[2]), "https://cdn.leonardo.ai/users/x/generations/y/clip.mp4");
    assert!(matches!(seen[3], PollStatus::Failed(_)), "{:?}", seen[3]);
}

#[tokio::test]
async fn moderation_refusals_read_as_content_policy() {
    // https://docs.leonardo.ai/docs/api-error-messages.md ("403 Forbidden: content moderation filter",
    // samples 1 and 2) and https://docs.leonardo.ai/docs/guide-to-handling-not-safe-for-work-image-generation-nsfw.md
    // ("Any prompts flagged as NSFW will return a 400 Bad Request error").
    let model = spec("7b592283-e8a7-4c5a-9ba6-d18c31f258b9");
    let req = request(&model, json!({}), None, 1);
    for (status, body) in [
        (403u16, json!({ "error": "Your prompt appears to contain inappropriate content, please be mindful of our Terms of Service. You may modify your prompt and try again.", "path": "$", "code": "unexpected" })),
        (403, json!({ "error": "content moderation filter: naked", "path": "$", "code": "unexpected" })),
        (400, json!({ "error": "content moderation filter: nude", "path": "$", "code": "unexpected" })),
    ] {
        let fake = start(move |_| Reply::status(status, body.clone())).await;
        let e = Leonardo.submit(&fake.ctx(&[HOST], &[("api_key", "test-key")]), &model, &req).await.unwrap_err();
        assert!(matches!(e, ProviderError::ContentPolicy(_)), "HTTP {status}: {e:?}");
    }
}

