//! Vertex AI contract: the documented OAuth service-account exchange, Imagen `:predict` and Veo
//! `:predictLongRunning` / `:fetchPredictOperation` answers replayed through the adapter, checking
//! what it sends. Docs read 29.10.2026 (Vertex AI is now "Gemini Enterprise Agent Platform"; the
//! REST hosts and paths are unchanged).
mod common;

use std::sync::atomic::{AtomicUsize, Ordering};

use base64::{engine::general_purpose::{STANDARD, URL_SAFE_NO_PAD}, Engine as _};
use common::{start, Call, Fake, Reply, PNG_1X1};
use pictesa_core::{
    Ctx, GenerationRequest, InputRole, MediaInput, MediaKind, ModelSpec, Output, PollStatus,
    Provider, ProviderError, RemoteHandle, Submission,
};
use pictesa_providers::vertex::Vertex;
use serde_json::{json, Value};

const TOKEN_HOST: &str = "oauth2.googleapis.com";
const API_HOST: &str = "us-central1-aiplatform.googleapis.com";
const MODELS: &str = "/v1/projects/test-project/locations/us-central1/publishers/google/models";

/// A throwaway RSA key generated for these tests only (`openssl genpkey`); it belongs to no account.
const TEST_KEY: &str = "-----BEGIN PRIVATE KEY-----\nMIIEvgIBADANBgkqhkiG9w0BAQEFAASCBKgwggSkAgEAAoIBAQC0a3r10dWB7V0f\nWIufy/pEhOT4vxedvMGKi1VOjk14lCgebNvm4hUHhtm1rI67DAYt60Gu3pj0i6aQ\nmuMmv3L2p2ItNyRSuwMaHVpfX0aL5CeyuH8QANnRA1R53Ywdpu1OJIvzOLJyEtNf\nNDmiSmc3JoUuWwFxTYJqFajQ8+BOshGgsQTPfcsXzJsrf8lf2HlU8FzQSrFl5QXG\nQx1QW+urY/wc59UBr8qJeUmbNn8sourgZtgelib/t2jQDq/2LYdI1PF4XJI7kmsV\nWCdwWuProHTMBN6n+DT8SeV8bnEU0T43C3jULSLXA6UqVaWAiRjwI83aMFzPO2+s\nZ+eYktTfAgMBAAECggEATe0gal1r9RbjQl85y1bMbVzjEwdieWhmReNhgLEZCAwh\n+eY14tbik2Is1XnHGkYawymaMKAHe5R36HxlwT4utxdb4cCv6SXKNLTZ7FiX/WMZ\nvGlQxEZBzjO8gHhvBP2tALPrWK2z8W5tKnFBtXWieCEko/19tYTvX8ekJ0A2sXYh\nTzMzM3omi2+AV8Aa6e5FKWyn0mpIB0koxInXQqmCGbX32zY7szah7D3DNW3AhsmT\nWAyjbIXX8RzEjucqKF7yE4I0R5kCFFi/RoWXlcdNODALTJd5zUhtRkqoC0vqcdcM\nws03iEZhIhQq3mIpV6zUBa6J9bFK0jeGeiPERzZCJQKBgQDcMtmpPRVQ0n4zsrW0\nqFi11N7CaMtM6Co74gh0OvyfDZQ+bF5pKESGQavi4WJn60SNtc2qZFoB0GFrSS+H\n3ND0lk22EF7XYecFX14yFKj20JH0hIg6z/S2SygvqFtu6TJT4KXOwtEhfdlisC+I\n66e5pbfDqtcvfP2bB/D9d0kvFQKBgQDRwPMfjPzXiOUnXLaTjCw8/4xSUc88nJOY\nujE6xDb6uOpR/TpY25BdB7dEUkIw8a6ndL/GBkzoiq1OqGLgGJeKGyrthbTnESe4\n3d/kn6pIviDsbdNpi3SPieVA6OqLSDMMYUJN6TSY9JXp6ilMLSVP++AkwPPDexuL\nKgNSO+0RIwKBgQC1P0XZ/+VjuV8XvqnCIi+iu6z/6LIug4r/73V19nepk8vJ9flg\nJT0CJVwOS+ZsE0NCQbsf3aoYhCUr4fm1oxR7vpwImED4mCVITs+B+W96eQMqeUyI\n3FNZGVPipnlcOaarU+Ym4kSxhJ1CljMKSK6uKLGx5RVqiT9WNJswhtc+YQKBgHRo\n9YWJJbopC6Sw29M/dD3WRfZZ2P32mCOnEjNfOnQjtEgUlhFCqfg3lAqi7IFlYF0n\neldYn85fSJfi9B0TCOdo9IYmMqQa7US4Ps4Pnt4IRxVwTmjBMMuI6GcB3Um3pD02\nrSm4R6yr8ZeEka7pT0/4pgk/3QmchUjS5Ez7XTKpAoGBANOOsZfYDJJ3RX4vsvOP\nAUCDj+90j2FIR54XY1q7UkvvPpo3c7VxxLsA19eHTa4FxVLGnKGCNsbc5cVAx0zk\n6VVZqd9k9obsycEA3iCs1MpXh0g95oiyiFEMwqH1FDSQdSr5Zz+LmUbGBCPVQwrZ\nTeTcfV7nVJHdYvxpqqNprHXx\n-----END PRIVATE KEY-----";

/// Credentials built from the adapter's own credential fields. Each test gets its own service
/// account email, so the adapter's process-wide token cache never crosses tests.
fn ctx(fake: &Fake) -> Ctx {
    static N: AtomicUsize = AtomicUsize::new(0);
    let email = format!("pictesa-test-{}@test-project.iam.gserviceaccount.com", N.fetch_add(1, Ordering::SeqCst));
    // https://docs.cloud.google.com/iam/docs/keys-create-delete: the key file's type,
    // project_id, private_key_id, private_key and client_email fields.
    let account = json!({
        "type": "service_account", "project_id": "test-project", "private_key_id": "test-key-id",
        "private_key": TEST_KEY, "client_email": email, "token_uri": "https://oauth2.googleapis.com/token"
    })
    .to_string();
    let fields = Vertex.info().fields;
    let value = |key: &str| match key {
        "service_account_json" => account.clone(),
        "project_id" => "test-project".to_string(),
        other => fields.iter().find(|f| f.key == other).and_then(|f| f.default.clone()).unwrap_or_default(),
    };
    let creds: Vec<(String, String)> = fields.iter().map(|f| (f.key.clone(), value(&f.key))).collect();
    let creds: Vec<(&str, &str)> = creds.iter().map(|(k, v)| (k.as_str(), v.as_str())).collect();
    fake.ctx(&[TOKEN_HOST, API_HOST], &creds)
}

// https://developers.google.com/identity/protocols/oauth2/service-account#httprest
// ("the response that you'll receive": access_token, token_type, expires_in).
fn token() -> Reply {
    Reply::json(json!({ "access_token": "ya29.test-token", "scope": "https://www.googleapis.com/auth/cloud-platform", "token_type": "Bearer", "expires_in": 3599 }))
}

fn model(id: &str) -> ModelSpec {
    Vertex.models().into_iter().find(|m| m.remote_id() == id).unwrap()
}

fn request(model: &str, params: Value, inputs: Vec<MediaInput>, n: u32) -> GenerationRequest {
    GenerationRequest {
        job_id: "job-1".into(),
        model: model.into(),
        prompt: "a lone cowboy rides across an open plain at sunset".into(),
        negative_prompt: None,
        params: serde_json::from_value(params).unwrap(),
        inputs,
        num_outputs: n,
    }
}

/// An input file the way the engine hands it over: a local path, no public URL.
fn input(role: InputRole) -> MediaInput {
    let path = std::env::temp_dir().join(format!("pictesa-contract-{:08x}.png", crc32fast::hash(PNG_1X1)));
    let part = path.with_extension(format!("{}.part", ulid::Ulid::new()));
    std::fs::write(&part, PNG_1X1).unwrap();
    std::fs::rename(&part, &path).unwrap();
    MediaInput { role, kind: MediaKind::Image, mime: "image/png".into(), path, url: None, width: None, height: None }
}

fn token_calls(fake: &Fake) -> Vec<Call> {
    fake.calls().into_iter().filter(|c| c.host == TOKEN_HOST).collect()
}

fn api_calls(fake: &Fake) -> Vec<Call> {
    fake.calls().into_iter().filter(|c| c.host == API_HOST).collect()
}

#[tokio::test]
async fn check_exchanges_a_signed_jwt_for_a_token_and_calls_the_regional_host_with_it() {
    // https://developers.google.com/identity/protocols/oauth2/service-account#httprest: POST
    // https://oauth2.googleapis.com/token, URL-encoded grant_type=urn:ietf:params:oauth:grant-type:jwt-bearer
    // and assertion=<JWT>; header {"alg":"RS256","typ":"JWT"}; claims iss, scope, aud
    // (always https://oauth2.googleapis.com/token), exp at most 1 hour after iat.
    let fake = start(|call| match call.host.as_str() {
        TOKEN_HOST => token(),
        // google.cloud.location.Location (name, locationId); see the report for this endpoint.
        _ => Reply::json(json!({ "name": "projects/test-project/locations/us-central1", "locationId": "us-central1" })),
    })
    .await;
    Vertex.check(&ctx(&fake)).await.unwrap();

    let exchange = &token_calls(&fake)[0];
    assert_eq!((exchange.method.as_str(), exchange.path.as_str()), ("POST", "/token"));
    assert!(exchange.header("content-type").unwrap().starts_with("application/x-www-form-urlencoded"));
    let form: std::collections::HashMap<String, String> = url::form_urlencoded::parse(&exchange.body).into_owned().collect();
    assert_eq!(form["grant_type"], "urn:ietf:params:oauth:grant-type:jwt-bearer");
    let parts: Vec<&str> = form["assertion"].split('.').collect();
    assert_eq!(parts.len(), 3, "a signed JWT");
    let header: Value = serde_json::from_slice(&URL_SAFE_NO_PAD.decode(parts[0]).unwrap()).unwrap();
    assert_eq!(header["alg"], "RS256");
    let claims: Value = serde_json::from_slice(&URL_SAFE_NO_PAD.decode(parts[1]).unwrap()).unwrap();
    assert!(claims["iss"].as_str().unwrap().ends_with("@test-project.iam.gserviceaccount.com"));
    assert_eq!(claims["aud"], "https://oauth2.googleapis.com/token");
    assert!(claims["scope"].as_str().unwrap().split(' ').any(|s| s == "https://www.googleapis.com/auth/cloud-platform"));
    let lifetime = claims["exp"].as_u64().unwrap() - claims["iat"].as_u64().unwrap();
    assert!(lifetime > 0 && lifetime <= 3600, "{lifetime}");

    let api = &api_calls(&fake)[0];
    assert_eq!(api.header("authorization"), Some("Bearer ya29.test-token"));
    assert_eq!(api.path, "/v1/projects/test-project/locations/us-central1");
}

#[tokio::test]
async fn imagen_predict_sends_documented_parameters_and_reads_predictions() {
    // https://docs.cloud.google.com/vertex-ai/generative-ai/docs/model-reference/imagen-api: POST
    // .../publishers/google/models/MODEL_VERSION:predict; parameters sampleCount, aspectRatio,
    // sampleImageSize ("1K"|"2K"), seed (only with addWatermark false); sample response with
    // predictions[{bytesBase64Encoded, mimeType}] for sampleCount 2.
    let fake = start(|call| match call.host.as_str() {
        TOKEN_HOST => token(),
        _ => Reply::json(json!({ "predictions": [
            { "bytesBase64Encoded": STANDARD.encode(PNG_1X1), "mimeType": "image/png" },
            { "mimeType": "image/png", "bytesBase64Encoded": STANDARD.encode(PNG_1X1) }
        ]})),
    })
    .await;
    let req = request("imagen-4.0-generate-001", json!({ "aspect_ratio": "16:9", "resolution": "2K", "seed": 42 }), vec![], 2);
    let Submission::Done(out) = Vertex.submit(&ctx(&fake), &model("imagen-4.0-generate-001"), &req).await.unwrap() else {
        panic!("predict answers synchronously")
    };
    assert_eq!(out.len(), 2);
    assert!(out.iter().all(|o| matches!(o, Output::Bytes { data, mime } if data == PNG_1X1 && mime == "image/png")));
    let call = &api_calls(&fake)[0];
    assert_eq!((call.method.as_str(), call.path.as_str()), ("POST", format!("{MODELS}/imagen-4.0-generate-001:predict").as_str()));
    assert_eq!(call.header("authorization"), Some("Bearer ya29.test-token"));
    let body = call.json();
    assert_eq!(body["instances"], json!([{ "prompt": "a lone cowboy rides across an open plain at sunset" }]));
    let p = &body["parameters"];
    assert_eq!((p["sampleCount"].as_u64(), p["aspectRatio"].as_str(), p["sampleImageSize"].as_str()), (Some(2), Some("16:9"), Some("2K")));
    assert_eq!((p["seed"].as_u64(), p["addWatermark"].as_bool()), (Some(42), Some(false)), "a seed needs the watermark off");
}

#[tokio::test]
async fn veo_runs_as_a_long_running_operation_polled_with_fetch_predict_operation() {
    // https://docs.cloud.google.com/vertex-ai/generative-ai/docs/model-reference/veo-video-generation:
    // instances[{prompt, image, lastFrame}] with {bytesBase64Encoded, mimeType}; parameters
    // aspectRatio, durationSeconds (integer 4|6|8 on Veo 3), resolution, generateAudio (required on
    // Veo 3), sampleCount, seed, negativePrompt; the answer is {"name": <operation>}; polling posts
    // {operationName} to :fetchPredictOperation; without storageUri the videos carry
    // bytesBase64Encoded.
    let operation = "projects/test-project/locations/us-central1/publishers/google/models/veo-3.1-generate-001/operations/a1b07c8e-7b5a-4aba-bb34-3e1ccb8afcc8";
    let polls = AtomicUsize::new(0);
    let fake = start(move |call| match (call.host.as_str(), call.path.as_str()) {
        (TOKEN_HOST, _) => token(),
        (_, p) if p.ends_with(":predictLongRunning") => Reply::json(json!({ "name": operation })),
        (_, p) if p.ends_with(":fetchPredictOperation") && polls.fetch_add(1, Ordering::SeqCst) == 0 => Reply::json(json!({ "name": operation, "done": false })),
        _ => Reply::json(json!({
            "name": operation,
            "done": true,
            "response": {
                "@type": "type.googleapis.com/cloud.ai.large_models.vision.GenerateVideoResponse",
                "raiMediaFilteredCount": 0,
                "videos": [{ "bytesBase64Encoded": STANDARD.encode(b"mp4 bytes"), "mimeType": "video/mp4" }]
            }
        })),
    })
    .await;
    let ctx = ctx(&fake);
    let mut req = request(
        "veo-3.1-generate-001",
        json!({ "aspect_ratio": "9:16", "duration": 8, "resolution": "1080p", "generate_audio": true, "seed": 7 }),
        vec![input(InputRole::StartFrame), input(InputRole::EndFrame)],
        1,
    );
    req.negative_prompt = Some("overhead lighting".into());
    let Submission::Pending(handle) = Vertex.submit(&ctx, &model("veo-3.1-generate-001"), &req).await.unwrap() else {
        panic!("Veo is long-running")
    };
    let submit = &api_calls(&fake)[0];
    assert_eq!(submit.path, format!("{MODELS}/veo-3.1-generate-001:predictLongRunning"));
    let body = submit.json();
    let frame = json!({ "bytesBase64Encoded": STANDARD.encode(PNG_1X1), "mimeType": "image/png" });
    let instance = &body["instances"][0];
    assert_eq!((&instance["image"], &instance["lastFrame"]), (&frame, &frame));
    let p = &body["parameters"];
    assert!(p["durationSeconds"].is_u64(), "durationSeconds is an integer: {}", p["durationSeconds"]);
    assert_eq!(p["durationSeconds"], 8);
    assert_eq!((p["aspectRatio"].as_str(), p["resolution"].as_str(), p["generateAudio"].as_bool()), (Some("9:16"), Some("1080p"), Some(true)));
    assert_eq!((p["sampleCount"].as_u64(), p["seed"].as_u64(), p["negativePrompt"].as_str()), (Some(1), Some(7), Some("overhead lighting")));
    assert!(p.get("storageUri").is_none(), "without storageUri the bytes come back inline");

    let handle: RemoteHandle = serde_json::from_str(&serde_json::to_string(&handle).unwrap()).unwrap();
    assert!(matches!(Vertex.poll(&ctx, &handle).await.unwrap(), PollStatus::Running { .. }));
    let PollStatus::Done(out) = Vertex.poll(&ctx, &handle).await.unwrap() else { panic!("finished operation") };
    assert!(matches!(&out[..], [Output::Bytes { data, mime }] if data == b"mp4 bytes" && mime == "video/mp4"));
    let fetch = &api_calls(&fake)[1];
    assert_eq!((fetch.method.as_str(), fetch.path.as_str()), ("POST", format!("{MODELS}/veo-3.1-generate-001:fetchPredictOperation").as_str()));
    assert_eq!(fetch.json(), json!({ "operationName": operation }));
    // "Access tokens can be reused during the duration window specified by the expires_in value."
    assert_eq!(token_calls(&fake).len(), 1, "one token for submit and both polls");
}

#[tokio::test]
async fn a_failed_or_filtered_veo_operation_ends_the_job() {
    // Operation.error (google.rpc.Status) and raiMediaFilteredCount / raiMediaFilteredReasons
    // (https://docs.cloud.google.com/vertex-ai/generative-ai/docs/model-reference/veo-video-generation#response-body;
    // reason text from https://docs.cloud.google.com/vertex-ai/generative-ai/docs/video/responsible-ai-and-usage-guidelines).
    let operation = "projects/test-project/locations/us-central1/publishers/google/models/veo-3.1-generate-001/operations/op-1";
    let reason = "Veo could not generate videos because the input image violates Vertex AI's usage guidelines. If you think this was an error, send feedback. Support codes: 15236754";
    let cases = [
        (json!({ "name": operation, "done": true, "error": { "code": 3, "message": "The prompt could not be submitted." } }), false),
        (json!({ "name": operation, "done": true, "response": {
            "@type": "type.googleapis.com/cloud.ai.large_models.vision.GenerateVideoResponse",
            "raiMediaFilteredCount": 1, "raiMediaFilteredReasons": [reason]
        }}), true),
    ];
    for (answer, filtered) in cases {
        let reply = answer.clone();
        let fake = start(move |call| if call.host == TOKEN_HOST { token() } else { Reply::json(reply.clone()) }).await;
        let handle = RemoteHandle::with(operation, json!({ "model": "veo-3.1-generate-001" }));
        let PollStatus::Failed(e) = Vertex.poll(&ctx(&fake), &handle).await.unwrap() else { panic!("{answer}") };
        assert_eq!(matches!(e, ProviderError::ContentPolicy(_)), filtered, "{e:?}");
        if filtered {
            assert!(e.to_string().contains("15236754"), "the support code reaches the user: {e}");
        }
    }
}

#[tokio::test]
async fn token_and_api_errors_map_to_auth_rate_limits_and_transient_failures() {
    // Token endpoint: https://developers.google.com/identity/protocols/oauth2/service-account#error-codes
    // (`invalid_grant` "Invalid JWT Signature." = deleted, disabled or foreign key; OAuth error
    // answers are HTTP 400, RFC 6749 section 5.2). API: https://docs.cloud.google.com/vertex-ai/generative-ai/docs/model-reference/api-errors
    // (401 UNAUTHENTICATED, 403 PERMISSION_DENIED, 429 RESOURCE_EXHAUSTED, 503 UNAVAILABLE).
    let bad_grant = Reply::status(400, json!({ "error": "invalid_grant", "error_description": "Invalid JWT Signature." }));
    let api = |status: u16, name: &str| Reply::status(status, json!({ "error": { "code": status, "message": format!("{name} from the stand-in"), "status": name } }));
    let cases: Vec<(Option<Reply>, Option<Reply>, fn(&ProviderError) -> bool)> = vec![
        (Some(bad_grant), None, |e| matches!(e, ProviderError::Auth(_))),
        (None, Some(api(401, "UNAUTHENTICATED")), |e| matches!(e, ProviderError::Auth(_))),
        (None, Some(api(403, "PERMISSION_DENIED")), |e| matches!(e, ProviderError::Auth(_))),
        (None, Some(api(429, "RESOURCE_EXHAUSTED")), |e| matches!(e, ProviderError::RateLimited { .. })),
        (None, Some(api(503, "UNAVAILABLE")), |e| matches!(e, ProviderError::Transient(_))),
    ];
    for (token_reply, api_reply, expected) in cases {
        let (token_reply, api_reply) = (std::sync::Mutex::new(token_reply), std::sync::Mutex::new(api_reply));
        let fake = start(move |call| {
            let slot = if call.host == TOKEN_HOST { &token_reply } else { &api_reply };
            slot.lock().unwrap().take().unwrap_or_else(|| if call.host == TOKEN_HOST { token() } else { Reply::json(json!({})) })
        })
        .await;
        let req = request("imagen-4.0-generate-001", json!({}), vec![], 1);
        let e = Vertex.submit(&ctx(&fake), &model("imagen-4.0-generate-001"), &req).await.unwrap_err();
        assert!(expected(&e), "{e:?}");
    }
}
