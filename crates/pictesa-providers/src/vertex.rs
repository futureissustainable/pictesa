use std::{
    collections::HashMap,
    sync::{Mutex, OnceLock},
    time::{Duration, SystemTime, UNIX_EPOCH},
};

use async_trait::async_trait;
use base64::Engine;
use jsonwebtoken::{Algorithm, EncodingKey, Header};
use pictesa_core::{
    common, keys, CredentialField, Ctx, GenerationRequest, InputRole, InputSlot, MediaKind,
    ModelSpec, Output, ParamSpec, PollStatus, Pricing, Provider, ProviderError, ProviderInfo,
    ProviderKind, ProviderResult, RemoteHandle, Submission, Task,
};
use serde::Serialize;
use serde_json::{json, Value};
use sha2::{Digest, Sha256};

use crate::util::{failure, get_json, send_json};

pub struct Vertex;
const TOKEN_URL: &str = "https://oauth2.googleapis.com/token";
const SCOPE: &str = "https://www.googleapis.com/auth/cloud-platform";
struct CachedToken {
    token: String,
    valid_until: u64,
}
static TOKENS: OnceLock<Mutex<HashMap<[u8; 32], CachedToken>>> = OnceLock::new();
fn tokens() -> &'static Mutex<HashMap<[u8; 32], CachedToken>> {
    TOKENS.get_or_init(|| Mutex::new(HashMap::new()))
}

#[derive(Serialize)]
struct Claims<'a> {
    iss: &'a str,
    scope: &'a str,
    aud: &'a str,
    iat: u64,
    exp: u64,
}

fn now() -> ProviderResult<u64> {
    Ok(SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_err(|_| ProviderError::Failed("system clock is invalid".into()))?
        .as_secs())
}

fn assertion(account: &Value, time: u64) -> ProviderResult<String> {
    let email = account
        .get("client_email")
        .and_then(Value::as_str)
        .ok_or_else(|| ProviderError::Auth("service account has no client_email".into()))?;
    let pem = account
        .get("private_key")
        .and_then(Value::as_str)
        .ok_or_else(|| ProviderError::Auth("service account has no private_key".into()))?;
    let key = EncodingKey::from_rsa_pem(pem.as_bytes())
        .map_err(|_| ProviderError::Auth("invalid service account private key".into()))?;
    jsonwebtoken::encode(
        &Header::new(Algorithm::RS256),
        &Claims {
            iss: email,
            scope: SCOPE,
            aud: TOKEN_URL,
            iat: time,
            exp: time + 3600,
        },
        &key,
    )
    .map_err(|_| ProviderError::Auth("could not sign service account assertion".into()))
}

async fn access_token(ctx: &Ctx) -> ProviderResult<String> {
    let raw = ctx.require("service_account_json")?;
    let fingerprint: [u8; 32] = Sha256::digest(raw.as_bytes()).into();
    let time = now()?;
    if let Some(c) = tokens()
        .lock()
        .map_err(|_| ProviderError::Failed("token cache lock poisoned".into()))?
        .get(&fingerprint)
    {
        if c.valid_until > time + 60 {
            return Ok(c.token.clone());
        }
    }
    let account: Value = serde_json::from_str(raw)
        .map_err(|_| ProviderError::Auth("service account JSON is invalid".into()))?;
    let jwt = assertion(&account, time)?;
    let v = get_json(ctx.http.post(TOKEN_URL).form(&[
        ("grant_type", "urn:ietf:params:oauth:grant-type:jwt-bearer"),
        ("assertion", jwt.as_str()),
    ]))
    .await
    .map_err(token_error)?;
    let token = v
        .get("access_token")
        .and_then(Value::as_str)
        .ok_or_else(|| ProviderError::Auth("OAuth response has no access token".into()))?
        .to_string();
    let valid_until = time + v.get("expires_in").and_then(Value::as_u64).unwrap_or(3600);
    tokens()
        .lock()
        .map_err(|_| ProviderError::Failed("token cache lock poisoned".into()))?
        .insert(
            fingerprint,
            CachedToken {
                token: token.clone(),
                valid_until,
            },
        );
    Ok(token)
}

/// The token endpoint answers a deleted, disabled or foreign key with HTTP 400 `invalid_grant`
/// (https://developers.google.com/identity/protocols/oauth2/service-account#error-codes): a
/// credential problem for the user to fix, not bad generation input.
fn token_error(e: ProviderError) -> ProviderError {
    match e {
        ProviderError::InvalidInput(m) => ProviderError::Auth(m),
        e => e,
    }
}

fn project_location(ctx: &Ctx) -> ProviderResult<(&str, &str)> {
    let project = ctx.require("project_id")?;
    let location = ctx.get("location").unwrap_or("us-central1");
    let valid =
        |s: &str| !s.is_empty() && s.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-');
    if !valid(project) || !valid(location) {
        return Err(ProviderError::InvalidInput(
            "invalid Vertex project or location".into(),
        ));
    }
    Ok((project, location))
}
fn endpoint(ctx: &Ctx, model: &str) -> ProviderResult<String> {
    let (project, location) = project_location(ctx)?;
    Ok(format!("https://{location}-aiplatform.googleapis.com/v1/projects/{project}/locations/{location}/publishers/google/models/{model}"))
}

fn image_spec(id: &str) -> ModelSpec {
    let mut m = ModelSpec::new("vertex", id, id, MediaKind::Image, &[Task::TextToImage])
        .param(common::aspect_ratio(
            &["1:1", "3:4", "4:3", "9:16", "16:9"],
            "1:1",
        ))
        .param(ParamSpec::seed())
        .max_outputs(4)
        .pricing(Pricing::Variable {
            note: "Vertex Imagen pricing".into(),
        })
        .vendor("Google");
    if !id.contains("fast") {
        m.params.push(common::resolution(&["1K", "2K"], "1K"));
    }
    m.tags
        .push("deprecated by Google; availability depends on project".into());
    m
}
fn video_spec(id: &str) -> ModelSpec {
    ModelSpec::new(
        "vertex",
        id,
        id,
        MediaKind::Video,
        &[Task::TextToVideo, Task::ImageToVideo],
    )
    .input(InputSlot::new(InputRole::StartFrame, MediaKind::Image, 1))
    .input(InputSlot::new(InputRole::EndFrame, MediaKind::Image, 1))
    .param(common::aspect_ratio(&["16:9", "9:16"], "16:9"))
    .param(common::duration(&[4, 6, 8], 8))
    .param(common::resolution(
        if id == "veo-3.1-generate-001" {
            &["720p", "1080p", "4k"]
        } else {
            &["720p", "1080p"]
        },
        "720p",
    ))
    .param(common::generate_audio(true))
    .param(ParamSpec::seed())
    .max_outputs(4)
    .pricing(Pricing::Variable {
        note: "Vertex Veo per-second pricing".into(),
    })
    .vendor("Google")
}

async fn request_body(model: &ModelSpec, req: &GenerationRequest) -> ProviderResult<Value> {
    if model.output == MediaKind::Image {
        if !req.inputs.is_empty() {
            return Err(ProviderError::NotSupported(
                "Imagen text generation does not accept reference images".into(),
            ));
        }
        let mut params = json!({"sampleCount":req.num_outputs});
        req.copy_params(
            params.as_object_mut().unwrap(),
            &[
                (keys::ASPECT_RATIO, "aspectRatio"),
                (keys::RESOLUTION, "sampleImageSize"),
                (keys::SEED, "seed"),
            ],
        );
        if req.u64(keys::SEED).is_some() {
            params["addWatermark"] = json!(false);
        }
        Ok(json!({"instances":[{"prompt":req.prompt}],"parameters":params}))
    } else {
        let mut instance = json!({"prompt":req.prompt});
        if let Some(input) = req.input(InputRole::StartFrame) {
            instance["image"] =
                json!({"bytesBase64Encoded":input.base64().await?,"mimeType":input.mime});
        }
        if let Some(input) = req.input(InputRole::EndFrame) {
            instance["lastFrame"] =
                json!({"bytesBase64Encoded":input.base64().await?,"mimeType":input.mime});
        }
        let mut params = json!({"sampleCount":req.num_outputs});
        req.copy_params(
            params.as_object_mut().unwrap(),
            &[
                (keys::ASPECT_RATIO, "aspectRatio"),
                (keys::DURATION, "durationSeconds"),
                (keys::RESOLUTION, "resolution"),
                (keys::GENERATE_AUDIO, "generateAudio"),
                (keys::SEED, "seed"),
            ],
        );
        if let Some(v) = &req.negative_prompt {
            params["negativePrompt"] = json!(v);
        }
        Ok(json!({"instances":[instance],"parameters":params}))
    }
}

fn media(v: &Value, kind: MediaKind) -> ProviderResult<Vec<Output>> {
    let key = if kind == MediaKind::Video {
        "videos"
    } else {
        "predictions"
    };
    let mut out = Vec::new();
    for item in v.get(key).and_then(Value::as_array).into_iter().flatten() {
        if let Some(encoded) = item.get("bytesBase64Encoded").and_then(Value::as_str) {
            out.push(Output::Bytes {
                data: base64::engine::general_purpose::STANDARD
                    .decode(encoded)
                    .map_err(|e| ProviderError::Failed(format!("invalid media base64: {e}")))?,
                mime: item
                    .get("mimeType")
                    .and_then(Value::as_str)
                    .unwrap_or(if kind == MediaKind::Video {
                        "video/mp4"
                    } else {
                        "image/png"
                    })
                    .into(),
            });
        } else if item.get("gcsUri").is_some() {
            return Err(ProviderError::NotSupported(
                "Vertex returned a GCS URI; omit storageUri to receive inline media".into(),
            ));
        }
    }
    if !out.is_empty() {
        return Ok(out);
    }
    // Veo leaves out the videos its responsible-AI filters removed, counts them and names the
    // reasons (https://docs.cloud.google.com/vertex-ai/generative-ai/docs/model-reference/veo-video-generation).
    if v.get("raiMediaFilteredCount").and_then(Value::as_u64).is_some_and(|n| n > 0) {
        let reasons: Vec<&str> = v
            .get("raiMediaFilteredReasons")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .filter_map(Value::as_str)
            .collect();
        return Err(ProviderError::ContentPolicy(format!(
            "Vertex filtered the output: {}",
            reasons.join("; ")
        )));
    }
    Err(ProviderError::Failed(format!("Vertex returned no {key}")))
}

fn operation(v: &Value) -> ProviderResult<PollStatus> {
    if !v.get("done").and_then(Value::as_bool).unwrap_or(false) {
        return Ok(PollStatus::Running {
            progress: None,
            message: None,
        });
    }
    if let Some(error) = v.get("error") {
        return Ok(PollStatus::Failed(failure(
            error
                .get("message")
                .and_then(Value::as_str)
                .unwrap_or("Vertex operation failed"),
        )));
    }
    match media(&v["response"], MediaKind::Video) {
        Err(e @ ProviderError::ContentPolicy(_)) => Ok(PollStatus::Failed(e)),
        out => Ok(PollStatus::Done(out?)),
    }
}

#[async_trait]
impl Provider for Vertex {
    fn info(&self) -> ProviderInfo {
        ProviderInfo {id:"vertex".into(),name:"Google Vertex AI".into(),kind:ProviderKind::Cloud,
            fields:vec![
                CredentialField {key:"service_account_json".into(),label:"Service account JSON".into(),env:vec!["GOOGLE_APPLICATION_CREDENTIALS_JSON".into()],secret:true,optional:false,default:None,placeholder:None},
                CredentialField {key:"project_id".into(),label:"Project ID".into(),env:vec!["GOOGLE_CLOUD_PROJECT".into()],secret:false,optional:false,default:None,placeholder:None},
                CredentialField {key:"location".into(),label:"Location".into(),env:vec!["GOOGLE_CLOUD_LOCATION".into()],secret:false,optional:true,default:Some("us-central1".into()),placeholder:None},
            ],signup_url:Some("https://console.cloud.google.com/vertex-ai".into()),
            docs_url:Some("https://docs.cloud.google.com/vertex-ai/generative-ai/docs/video/generate-videos-from-text".into()),max_concurrency:2}
    }
    fn models(&self) -> Vec<ModelSpec> {
        [
            "veo-3.1-generate-001",
            "veo-3.1-fast-generate-001",
            "veo-3.1-lite-generate-001",
        ]
        .into_iter()
        .map(video_spec)
        .chain(
            [
                "imagen-4.0-generate-001",
                "imagen-4.0-fast-generate-001",
                "imagen-4.0-ultra-generate-001",
            ]
            .into_iter()
            .map(image_spec),
        )
        .collect()
    }
    async fn check(&self, ctx: &Ctx) -> ProviderResult<()> {
        let (project, location) = project_location(ctx)?;
        let url=format!("https://{location}-aiplatform.googleapis.com/v1/projects/{project}/locations/{location}");
        get_json(ctx.http.get(url).bearer_auth(access_token(ctx).await?)).await?;
        Ok(())
    }
    async fn submit(
        &self,
        ctx: &Ctx,
        model: &ModelSpec,
        req: &GenerationRequest,
    ) -> ProviderResult<Submission> {
        let path = endpoint(ctx, model.remote_id())?;
        let suffix = if model.output == MediaKind::Video {
            ":predictLongRunning"
        } else {
            ":predict"
        };
        let v = send_json(
            ctx.http
                .post(format!("{path}{suffix}"))
                .bearer_auth(access_token(ctx).await?),
            &request_body(model, req).await?,
        )
        .await?;
        if model.output == MediaKind::Image {
            return Ok(Submission::Done(media(&v, MediaKind::Image)?));
        }
        let name = v
            .get("name")
            .and_then(Value::as_str)
            .ok_or_else(|| ProviderError::Failed("Vertex returned no operation name".into()))?;
        Ok(Submission::Pending(RemoteHandle::with(
            name,
            json!({"model":model.remote_id()}),
        )))
    }
    async fn poll(&self, ctx: &Ctx, handle: &RemoteHandle) -> ProviderResult<PollStatus> {
        let model = handle
            .data_str("model")
            .ok_or_else(|| ProviderError::Failed("Vertex handle has no model".into()))?;
        let v = send_json(
            ctx.http
                .post(format!("{}:fetchPredictOperation", endpoint(ctx, model)?))
                .bearer_auth(access_token(ctx).await?),
            &json!({"operationName":handle.id}),
        )
        .await?;
        operation(&v)
    }
    fn poll_interval(&self, _attempt: u32) -> Duration {
        Duration::from_secs(10)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[tokio::test]
    async fn maps_veo_and_imagen_payloads() {
        let mut req:GenerationRequest=serde_json::from_value(json!({"job_id":"x","model":"veo-3.1-generate-001","prompt":"sea","inputs":[],"params":{"duration":8,"aspect_ratio":"9:16","generate_audio":true},"num_outputs":1})).unwrap();
        let video = request_body(&video_spec("veo-3.1-generate-001"), &req)
            .await
            .unwrap();
        assert_eq!(video["parameters"]["durationSeconds"], 8);
        assert_eq!(video["parameters"]["generateAudio"], true);
        req.params.insert(keys::SEED.into(), json!(10));
        let image = request_body(&image_spec("imagen-4.0-generate-001"), &req)
            .await
            .unwrap();
        assert_eq!(image["parameters"]["addWatermark"], false);
    }
    #[test]
    fn parses_completed_operation() {
        let v = json!({"done":true,"response":{"videos":[{"bytesBase64Encoded":"YQ==","mimeType":"video/mp4"}]}});
        assert!(matches!(operation(&v).unwrap(), PollStatus::Done(_)));
    }
}
