//! Kling contract: the documented answers of api-singapore.klingai.com replayed through the adapter,
//! and what the adapter sends checked against the published API.
//! Docs: https://kling.ai/document-api/llms.txt (read 29.10.2026). The adapter uses Kling's legacy
//! design (`model_name` in the body, access key + secret key); of that design the docs still publish
//! the Image 2.1 endpoints, so the video routes are not covered here.

mod common;

use std::time::{SystemTime, UNIX_EPOCH};

use common::{start, Reply, PNG_1X1};
use pictesa_core::{GenerationRequest, InputRole, MediaInput, MediaKind, ModelSpec, Output, PollStatus, Provider, Submission};
use pictesa_providers::kling::Kling;
use serde_json::{json, Value};

const HOST: &str = "api-singapore.klingai.com";

/// Test values for every credential field the adapter declares.
fn creds() -> Vec<(String, String)> {
    let fields = Kling.info().fields;
    assert_eq!(fields.iter().map(|f| f.key.as_str()).collect::<Vec<_>>(), ["access_key", "secret_key"]);
    fields.iter().map(|f| (f.key.clone(), format!("test-{}", f.key))).collect()
}

fn ctx(fake: &common::Fake) -> pictesa_core::Ctx {
    let creds = creds();
    let pairs: Vec<(&str, &str)> = creds.iter().map(|(k, v)| (k.as_str(), v.as_str())).collect();
    fake.ctx(&[HOST], &pairs)
}

fn spec(id: &str) -> ModelSpec {
    Kling.models().into_iter().find(|m| m.remote_id() == id).unwrap()
}

/// A request as the engine hands it over: params checked and filled with the model's defaults.
fn request(model: &ModelSpec, params: Value, inputs: Vec<MediaInput>, n: u32) -> GenerationRequest {
    let mut p = params.as_object().cloned().unwrap_or_default();
    for spec in &model.params {
        if let Some(d) = &spec.default {
            p.entry(spec.key.clone()).or_insert(d.clone());
        }
    }
    GenerationRequest { job_id: "job-1".into(), model: model.remote_id().into(), prompt: "Generate a Pixar-style puppy".into(), negative_prompt: None, params: p, inputs, num_outputs: n }
}

/// The Image 2.1 create answer. https://kling.ai/document-api/api/image/2-1/image-generation.md ("Create Task", response example)
fn created() -> Value {
    json!({ "code": 0, "message": "string", "request_id": "string", "data": { "task_id": "893605946402811985", "task_status": "submitted", "task_info": { "external_task_id": "string" }, "created_at": 1722769557708u64, "updated_at": 1722769557708u64 } })
}

/// A single-task answer. Same page, "Query Task (Single)" response example.
fn task(status: &str, extra: Value) -> Value {
    let mut d = json!({ "task_id": "893605946402811985", "task_status": status, "task_info": { "external_task_id": "string" }, "created_at": 1722769557708u64, "updated_at": 1722769557708u64 });
    d.as_object_mut().unwrap().extend(extra.as_object().cloned().unwrap_or_default());
    json!({ "code": 0, "message": "string", "request_id": "string", "data": d })
}

#[tokio::test]
async fn check_signs_a_kling_jwt_from_the_access_and_secret_keys() {
    // https://kling.ai/document-api/api/get-started/authentication.md: JWT (RFC 7519), header
    // {"alg":"HS256","typ":"JWT"}, payload iss = access key, exp = now + 1800, nbf = now - 5, signed
    // with the secret key; sent as `Authorization: Bearer <token>`.
    // https://kling.ai/document-api/api/image/2-1/image-generation.md ("Query Task (List)": GET
    // /v1/images/generations?pageNum=&pageSize=).
    let fake = start(|_| Reply::json(json!({ "code": 0, "message": "string", "request_id": "string", "data": [] }))).await;
    Kling.check(&ctx(&fake)).await.unwrap();
    let call = &fake.calls()[0];
    assert_eq!((call.method.as_str(), call.path.as_str()), ("GET", "/v1/images/generations?pageNum=1&pageSize=1"));
    let token = call.header("authorization").and_then(|h| h.strip_prefix("Bearer ")).expect("a bearer token");

    let header = jsonwebtoken::decode_header(token).unwrap();
    assert_eq!((header.alg, header.typ.as_deref()), (jsonwebtoken::Algorithm::HS256, Some("JWT")));
    let mut validation = jsonwebtoken::Validation::new(jsonwebtoken::Algorithm::HS256);
    validation.set_issuer(&["test-access_key"]);
    validation.validate_nbf = true;
    let claims = jsonwebtoken::decode::<Value>(token, &jsonwebtoken::DecodingKey::from_secret(b"test-secret_key"), &validation).expect("signed with the secret key").claims;
    let now = SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_secs() as i64;
    let (exp, nbf) = (claims["exp"].as_i64().unwrap(), claims["nbf"].as_i64().unwrap());
    assert!((exp - (now + 1800)).abs() <= 60, "exp {exp} is not now + 1800 ({now})");
    assert!((nbf - (now - 5)).abs() <= 60, "nbf {nbf} is not now - 5 ({now})");
}

#[tokio::test]
async fn image_generation_sends_the_documented_legacy_fields() {
    // https://kling.ai/document-api/api/image/2-1/image-generation.md (POST /v1/images/generations):
    // model_name kling-v2-1; n 1-9; resolution 1k | 2k; image is a URL or raw Base64 "do NOT add any
    // prefix like data:image/png;base64,".
    let documented = ["model_name", "prompt", "negative_prompt", "image", "image_reference", "image_fidelity", "human_fidelity", "element_list", "resolution", "n", "aspect_ratio", "watermark_info", "callback_url", "external_task_id"];
    let model = spec("kling-v2-1");
    let dir = std::env::temp_dir().join(format!("pictesa-contract-kling-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("dog.png"), PNG_1X1).unwrap();
    let hosted = MediaInput { role: InputRole::Reference, kind: MediaKind::Image, mime: "image/png".into(), path: "unused.png".into(), url: Some("https://p1-kling.klingai.com/kcdn/cdn-kcdn112452/kling-qa-test/dog.png".into()), width: None, height: None };
    let local = MediaInput { url: None, path: dir.join("dog.png"), ..hosted.clone() };
    for (input, image) in [(hosted, "https://p1-kling.klingai.com/kcdn/cdn-kcdn112452/kling-qa-test/dog.png".to_string()), (local, base64::Engine::encode(&base64::engine::general_purpose::STANDARD, PNG_1X1))] {
        let req = request(&model, json!({ "aspect_ratio": "16:9", "resolution": "2k" }), vec![input], 2);
        let fake = start(|_| Reply::json(created())).await;
        let Submission::Pending(handle) = Kling.submit(&ctx(&fake), &model, &req).await.unwrap() else { panic!() };
        assert_eq!(handle.id, "893605946402811985");
        let call = &fake.calls()[0];
        assert_eq!((call.method.as_str(), call.path.as_str()), ("POST", "/v1/images/generations"));
        assert!(call.header("authorization").is_some_and(|h| h.starts_with("Bearer ")));
        let body = call.json();
        for key in body.as_object().unwrap().keys() {
            assert!(documented.contains(&key.as_str()), "undocumented `{key}`: {body}");
        }
        assert_eq!(
            body,
            json!({ "model_name": "kling-v2-1", "prompt": "Generate a Pixar-style puppy", "aspect_ratio": "16:9", "resolution": "2k", "n": 2, "image": image })
        );
    }
}

#[tokio::test]
async fn every_documented_image_task_status_maps_to_a_poll_status() {
    // https://kling.ai/document-api/api/image/2-1/image-generation.md ("Query Task (Single)": GET
    // /v1/images/generations/{id}; task_status submitted, processing, succeed, failed;
    // task_result.images[].url; task_status_msg holds the failure reason).
    let model = spec("kling-v2-1");
    let req = request(&model, json!({}), vec![], 1);
    let cases = [
        task("submitted", json!({})),
        task("processing", json!({})),
        task("succeed", json!({ "task_result": { "images": [{ "index": 0, "url": "https://h1.inkwai.com/bs2/upload-ylab-stunt/1fa0ac67d8ce6cd55b50d68b967b3a59.png", "watermark_url": "string" }] } })),
        task("failed", json!({ "task_status_msg": "string" })),
    ];
    let mut seen = Vec::new();
    for answer in cases {
        let fake = start(move |call| if call.method == "POST" { Reply::json(created()) } else { Reply::json(answer.clone()) }).await;
        let ctx = ctx(&fake);
        let Submission::Pending(handle) = Kling.submit(&ctx, &model, &req).await.unwrap() else { panic!() };
        seen.push(Kling.poll(&ctx, &handle).await.unwrap());
        let poll = &fake.calls()[1];
        assert_eq!((poll.method.as_str(), poll.path.as_str()), ("GET", "/v1/images/generations/893605946402811985"));
        assert!(poll.header("authorization").is_some_and(|h| h.starts_with("Bearer ")));
    }
    assert!(matches!(seen[0], PollStatus::Queued { .. }), "{:?}", seen[0]);
    assert!(matches!(seen[1], PollStatus::Running { .. }), "{:?}", seen[1]);
    assert!(matches!(&seen[2], PollStatus::Done(o) if matches!(o.as_slice(), [Output::Url { url, .. }] if url.ends_with("1fa0ac67d8ce6cd55b50d68b967b3a59.png"))), "{:?}", seen[2]);
    assert!(matches!(seen[3], PollStatus::Failed(_)), "{:?}", seen[3]);
}

#[tokio::test]
async fn service_codes_decide_the_error_kind() {
    // https://kling.ai/document-api/api/get-started/error-codes.md: each error has an HTTP status and a
    // service code in the body's `code`; messages are the table's "Explanation of Service Code".
    // 1101/1102 (no balance, no resource pack) come as 429 and 1301 (content security) as 400, so the
    // status alone misreads them as a rate limit and a bad parameter.
    let model = spec("kling-v2-1");
    let req = request(&model, json!({}), vec![], 1);
    let cases = [
        (401u16, 1002, "Authorization is invalid", "auth"),
        (401, 1004, "Authorization has expired", "auth"),
        (429, 1101, "Account in arrears (postpaid scenario)", "insufficient_credits"),
        (429, 1102, "Resource pack exhausted or expired (prepaid scenario)", "insufficient_credits"),
        (400, 1301, "Trigger the platform's content security policy", "content_policy"),
        (429, 1302, "Too many requests; rate limit exceeded", "rate_limited"),
        (400, 1201, "Invalid parameters, such as an incorrect key or invalid value", "invalid_input"),
        (500, 5000, "Server internal error", "transient"),
        (503, 5001, "Server temporarily unavailable, usually due to maintenance", "transient"),
    ];
    for (status, code, message, kind) in cases {
        let fake = start(move |_| Reply::status(status, json!({ "code": code, "message": message, "request_id": "string" }))).await;
        let e = Kling.submit(&ctx(&fake), &model, &req).await.unwrap_err();
        assert_eq!(e.kind(), kind, "HTTP {status} code {code}: {e:?}");
    }
}
