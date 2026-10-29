//! Recraft V4.1: the adapter against the documented API, replayed offline.
//! Docs: https://www.recraft.ai/docs/api-reference/agents.md (the whole API on one page).
mod common;
mod form_parts;

use common::{start, Fake, Reply};
use form_parts::fields;
use pictesa_core::{Ctx, GenerationRequest, ModelSpec, Output, Provider, Submission};
use pictesa_providers::recraft::Recraft;
use serde_json::{json, Value};

fn ctx(fake: &Fake) -> Ctx {
    fake.ctx(&["external.api.recraft.ai"], &[("api_key", "test-key")])
}

fn model(id: &str) -> ModelSpec {
    Recraft.models().into_iter().find(|m| m.remote_id() == id).expect("model in catalog")
}

fn request(model: &str, params: Value, num_outputs: u32) -> GenerationRequest {
    GenerationRequest {
        job_id: "contract".into(),
        model: model.into(),
        prompt: "two race cars on a track".into(),
        negative_prompt: None,
        params: params.as_object().cloned().unwrap_or_default(),
        inputs: vec![],
        num_outputs,
    }
}

// "Response of generation", https://www.recraft.ai/docs/api-reference/agents.md
fn generated(urls: &[&str]) -> Reply {
    let data: Vec<Value> = urls.iter().enumerate().map(|(i, u)| json!({ "image_id": format!("0e6a1b8c-5b1f-4a8e-9f3a-2c7d4e8b9a1{i}"), "url": u })).collect();
    Reply::json(json!({ "created": 1759140000, "credits": 35 * urls.len(), "data": data }))
}

fn text(status: u16, body: &str) -> Reply {
    Reply { status, content_type: "text/plain".into(), body: body.as_bytes().to_vec(), headers: vec![] }
}

#[tokio::test]
async fn check_reads_the_account_with_a_bearer_token() {
    // "GET /v1/users/me returns {"id", "email", "name", "credits"}", https://www.recraft.ai/docs/api-reference/agents.md
    let fake = start(|_| Reply::json(json!({ "id": "5f0b8d1e-0000-4000-8000-000000000001", "email": "dev@example.com", "name": "Dev", "credits": 12000 }))).await;
    Recraft.check(&ctx(&fake)).await.expect("check");
    let call = &fake.calls()[0];
    assert_eq!((call.method.as_str(), call.path.as_str()), ("GET", "/v1/users/me"));
    assert_eq!(call.header("authorization"), Some("Bearer test-key"));
}

#[tokio::test]
async fn raster_generation_sends_only_fields_v4_1_supports() {
    let fake = start(|_| generated(&["https://img.recraft.ai/a.webp", "https://img.recraft.ai/b.webp"])).await;
    let style_id = "229b2a75-05e4-4580-85f9-b47ee521a00d";
    let req = request("recraftv4_1", json!({ "aspect_ratio": "16:9", "seed": 7, "output_format": "png", "style": style_id }), 2);
    let Submission::Done(outputs) = Recraft.submit(&ctx(&fake), &model("recraftv4_1"), &req).await.expect("submit") else { panic!("synchronous") };

    let call = &fake.calls()[0];
    assert_eq!((call.method.as_str(), call.path.as_str()), ("POST", "/v1/images/generations"));
    assert_eq!(call.header("authorization"), Some("Bearer test-key"));
    let sent = fields(call);
    let field = |k: &str| sent.get(k).map(String::as_str);
    // "Generate image" fields, https://www.recraft.ai/docs/api-reference/agents.md
    assert_eq!((field("model"), field("prompt")), (Some("recraftv4_1"), Some("two race cars on a track")));
    assert_eq!((field("n"), field("size"), field("random_seed"), field("image_format")), (Some("2"), Some("16:9"), Some("7"), Some("png")));
    // V4.1 takes a custom style as `style_id`; curated `style` names are V2/V3 only ("Not supported
    // by V4.1: negative_prompt, style (curated styles) ...", https://www.recraft.ai/docs/api-reference/models/recraft-v4-1.md).
    assert_eq!(field("style_id"), Some(style_id));
    assert_eq!(field("style"), None);
    assert_eq!(outputs.len(), 2);
    assert!(matches!(&outputs[0], Output::Url { url, .. } if url == "https://img.recraft.ai/a.webp"));
}

#[tokio::test]
async fn vector_models_use_the_vector_endpoint() {
    // "Variants that enforce the output type: POST /v1/images/generations/raster and
    // POST /v1/images/generations/vector", https://www.recraft.ai/docs/api-reference/agents.md
    let fake = start(|_| generated(&["https://img.recraft.ai/logo.svg"])).await;
    let Submission::Done(outputs) = Recraft.submit(&ctx(&fake), &model("recraftv4_1_vector"), &request("recraftv4_1_vector", json!({ "aspect_ratio": "1:1" }), 1)).await.expect("submit") else { panic!("synchronous") };
    let call = &fake.calls()[0];
    assert_eq!(call.path, "/v1/images/generations/vector");
    assert_eq!(fields(call).get("model").map(String::as_str), Some("recraftv4_1_vector"));
    assert!(matches!(&outputs[..], [Output::Url { url, .. }] if url.ends_with("logo.svg")));
}

#[tokio::test]
async fn documented_errors_map_to_their_meaning() {
    // Known errors are plain text; 5xx "are usually temporary",
    // https://www.recraft.ai/docs/api-reference/error-handling.md
    let cases = [(401, "request unauthorized", "auth"), (404, "404 page not found", "invalid_input"), (500, "internal server error", "transient")];
    let mut wrong = Vec::new();
    for (status, body, want) in cases {
        let fake = start(move |_| text(status, body)).await;
        let err = Recraft.submit(&ctx(&fake), &model("recraftv4_1"), &request("recraftv4_1", json!({}), 1)).await.expect_err("error status");
        if err.kind() != want {
            wrong.push(format!("HTTP {status}: {}, want {want}", err.kind()));
        }
    }
    assert!(wrong.is_empty(), "{wrong:#?}");
}
