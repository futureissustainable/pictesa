//! Pictesa HTTP server: the JSON API from docs/API.md, server-sent events, files with range
//! support, the MCP endpoint and the embedded web UI, all on one port.

pub mod mcp;
mod web;

use std::{convert::Infallible, net::SocketAddr, path::PathBuf, time::Duration};

use axum::{
    body::Body,
    extract::{DefaultBodyLimit, FromRequestParts, Multipart, Path, Query, Request, State},
    http::{header, request::Parts, HeaderMap, HeaderValue, StatusCode},
    middleware::{self, Next},
    response::{
        sse::{Event as SseEvent, KeepAlive, Sse},
        IntoResponse, Response,
    },
    routing::{delete, get, post, put},
    Json, Router,
};
use futures::Stream;
use pictesa_core::MediaKind;
use pictesa_engine::{AssetFilter, CreateGeneration, Engine, EngineError, JobFilter, Mode, ModelFilter, User, LOCAL_USER};
use serde::Deserialize;
use serde_json::{json, Map, Value};
use tower::ServiceExt;
use tower_http::{
    compression::{
        predicate::{DefaultPredicate, NotForContentType, Predicate},
        CompressionLayer,
    },
    services::ServeFile,
};

pub const SESSION_COOKIE: &str = "pictesa_session";
pub const TOKEN_COOKIE: &str = "pictesa_token";
/// Largest upload accepted in one request (streamed to disk, never held in memory).
pub const MAX_UPLOAD: usize = 4 * 1024 * 1024 * 1024 - 1;
/// Largest JSON body on every other route.
pub const MAX_JSON: usize = 2 * 1024 * 1024;

#[derive(Clone, Debug, Default)]
pub struct ServerOptions {
    pub addr: Option<SocketAddr>,
    /// Local mode: require this bearer token for API and MCP calls.
    pub token: Option<String>,
    /// Serve the UI from this folder instead of the embedded copy (development).
    pub web_dir: Option<PathBuf>,
    /// Mark cookies Secure (hosted mode behind HTTPS).
    pub secure_cookies: bool,
    /// Local mode: Host and Origin values accepted (blocks DNS rebinding and cross-site requests).
    /// Empty means "derive from the listen address".
    pub allowed_hosts: Vec<String>,
}

impl ServerOptions {
    /// Hosts a local server answers to: the loopback names on its port plus any configured extras.
    pub fn local_hosts(addr: SocketAddr, extra: &[String]) -> Vec<String> {
        let port = addr.port();
        let mut hosts = vec![format!("localhost:{port}"), format!("127.0.0.1:{port}"), format!("[::1]:{port}")];
        if !addr.ip().is_loopback() && !addr.ip().is_unspecified() {
            hosts.push(addr.to_string());
        }
        hosts.extend(extra.iter().map(|h| h.trim().to_lowercase()).filter(|h| !h.is_empty()));
        hosts
    }
}

#[derive(Clone)]
pub struct AppState {
    pub engine: Engine,
    pub opts: ServerOptions,
}

/// The authenticated caller of a request.
#[derive(Clone, Debug)]
pub struct Caller {
    pub user_id: String,
    pub user: Option<User>,
}

pub struct ApiError(EngineError);

impl From<EngineError> for ApiError {
    fn from(e: EngineError) -> Self {
        Self(e)
    }
}

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        let (status, kind, field) = match &self.0 {
            EngineError::NotFound(_) => (StatusCode::NOT_FOUND, "not_found", None),
            EngineError::Validation { field, .. } => (StatusCode::BAD_REQUEST, "validation", field.clone()),
            EngineError::Unauthorized(_) => (StatusCode::UNAUTHORIZED, "auth", None),
            EngineError::Forbidden(_) => (StatusCode::FORBIDDEN, "forbidden", None),
            EngineError::Conflict(_) => (StatusCode::CONFLICT, "conflict", None),
            EngineError::TooMany(_) => (StatusCode::TOO_MANY_REQUESTS, "rate_limited", None),
            EngineError::Internal(e) => {
                tracing::error!("internal error: {e:#}");
                (StatusCode::INTERNAL_SERVER_ERROR, "internal", None)
            }
        };
        let message = match &self.0 {
            EngineError::Internal(_) => "Something went wrong on the server. Check the Pictesa log.".to_string(),
            other => other.to_string(),
        };
        let mut body = json!({ "error": message, "kind": kind });
        if let Some(f) = field {
            body["field"] = json!(f);
        }
        (status, Json(body)).into_response()
    }
}

type ApiResult<T> = Result<T, ApiError>;

fn bad(msg: &str, field: Option<&str>) -> ApiError {
    ApiError(EngineError::invalid(msg, field))
}

impl FromRequestParts<AppState> for Caller {
    type Rejection = ApiError;
    async fn from_request_parts(parts: &mut Parts, _state: &AppState) -> Result<Self, Self::Rejection> {
        parts.extensions.get::<Caller>().cloned().ok_or_else(|| ApiError(EngineError::Unauthorized("Sign in to continue.".into())))
    }
}

fn cookie(headers: &HeaderMap, name: &str) -> Option<String> {
    headers.get_all(header::COOKIE).iter().filter_map(|v| v.to_str().ok()).flat_map(|v| v.split(';')).find_map(|kv| {
        let (k, v) = kv.trim().split_once('=')?;
        (k == name).then(|| v.to_string())
    })
}

fn bearer(headers: &HeaderMap) -> Option<String> {
    headers.get(header::AUTHORIZATION)?.to_str().ok()?.strip_prefix("Bearer ").map(|s| s.trim().to_string())
}

/// Constant-time comparison for tokens.
fn same(a: &str, b: &str) -> bool {
    a.len() == b.len() && a.bytes().zip(b.bytes()).fold(0u8, |acc, (x, y)| acc | (x ^ y)) == 0
}

/// Local mode: only answer to known host names, and refuse writes from other sites.
fn foreign_request(state: &AppState, headers: &HeaderMap, method: &axum::http::Method) -> Option<&'static str> {
    // With an access token the token itself (and its SameSite=Strict cookie) stops foreign sites.
    if state.engine.mode() != Mode::Local || state.opts.allowed_hosts.is_empty() || state.opts.token.is_some() {
        return None;
    }
    let allowed = &state.opts.allowed_hosts;
    let host = headers.get(header::HOST).and_then(|v| v.to_str().ok()).map(str::to_lowercase);
    if let Some(h) = &host {
        if !allowed.iter().any(|a| a == h) {
            return Some("unknown host");
        }
    }
    if !matches!(*method, axum::http::Method::GET | axum::http::Method::HEAD | axum::http::Method::OPTIONS) {
        if let Some(origin) = headers.get(header::ORIGIN).and_then(|v| v.to_str().ok()) {
            let origin_host = origin.split("://").nth(1).unwrap_or("").trim_end_matches('/').to_lowercase();
            if !allowed.iter().any(|a| *a == origin_host) {
                return Some("cross-site request");
            }
        }
    }
    None
}

/// Works out who is calling and stores a [`Caller`] in the request extensions.
async fn authenticate(State(state): State<AppState>, mut req: Request, next: Next) -> Response {
    let headers = req.headers().clone();
    if let Some(reason) = foreign_request(&state, &headers, req.method()) {
        tracing::warn!("refused {} {}: {reason}", req.method(), req.uri().path());
        return (StatusCode::FORBIDDEN, Json(json!({ "error": format!("Refused: {reason}. Open Pictesa at http://127.0.0.1 or set PICTESA_ALLOWED_HOSTS."), "kind": "forbidden" }))).into_response();
    }
    let caller = match state.engine.mode() {
        Mode::Local => {
            let ok = match &state.opts.token {
                None => true,
                Some(t) => bearer(&headers).or_else(|| cookie(&headers, TOKEN_COOKIE)).is_some_and(|given| same(&given, t)),
            };
            ok.then(|| Caller { user_id: LOCAL_USER.to_string(), user: None })
        }
        Mode::Hosted => {
            let mut found = None;
            if let Some(token) = bearer(&headers) {
                found = state.engine.token_user(&token).await.ok().flatten();
            } else if let Some(session) = cookie(&headers, SESSION_COOKIE) {
                found = state.engine.session_user(&session).await.ok().flatten();
            }
            found.map(|u| Caller { user_id: u.id.clone(), user: Some(u) })
        }
    };
    if let Some(c) = caller {
        req.extensions_mut().insert(c);
    }
    next.run(req).await
}

pub fn router(engine: Engine, opts: ServerOptions) -> Router {
    let state = AppState { engine, opts };
    let api = Router::new()
        .route("/health", get(health))
        .route("/auth/me", get(me))
        .route("/auth/signup", post(signup))
        .route("/auth/login", post(login))
        .route("/auth/logout", post(logout))
        .route("/auth/token", post(token_login))
        .route("/tokens", get(list_tokens).post(create_token))
        .route("/tokens/{id}", delete(delete_token))
        .route("/providers", get(providers))
        .route("/providers/{id}", put(set_provider).delete(clear_provider))
        .route("/providers/{id}/check", post(check_provider))
        .route("/models", get(models))
        .route("/models/refresh", post(refresh_models))
        .route("/models/{*id}", get(model))
        .route("/presets", get(presets))
        .route("/prompt/enhance", post(enhance_prompt))
        .route("/generations", get(list_jobs).post(create_job))
        .route("/generations/{id}", get(get_job).delete(delete_job))
        .route("/generations/{id}/cancel", post(cancel_job))
        .route("/generations/{id}/retry", post(retry_job))
        .route("/generations/{id}/preview", get(job_preview))
        .route("/assets", get(list_assets).post(upload_assets).layer(DefaultBodyLimit::max(MAX_UPLOAD)))
        .route("/assets/import", post(import_asset))
        .route("/assets/{id}", get(get_asset).patch(patch_asset).delete(delete_asset))
        .route("/assets/{id}/restore", post(restore_asset))
        .route("/events", get(events))
        .route("/workflows/import", post(import_workflow))
        .route("/characters", get(characters).post(create_character))
        .route("/characters/{id}", get(character).patch(update_character).delete(delete_character))
        .route("/recipes", get(recipes))
        .route("/recipes/runs", get(recipe_runs))
        .route("/recipes/runs/{id}", get(recipe_run))
        .route("/recipes/runs/{id}/cancel", post(cancel_recipe_run))
        .route("/recipes/run/{*id}", post(run_recipe))
        .route("/billing", get(billing))
        .route("/billing/checkout", post(checkout))
        .route("/billing/webhook", post(stripe_webhook))
        .fallback(|| async { ApiError(EngineError::NotFound("No such API route.".into())) });

    Router::new()
        .nest("/api", api)
        .route("/files/{id}", get(file))
        .route("/thumbs/{id}", get(thumb))
        .route("/mcp", post(mcp_http).get(|| async { StatusCode::METHOD_NOT_ALLOWED }))
        .fallback(web::serve)
        .layer(middleware::from_fn_with_state(state.clone(), authenticate))
        .layer(DefaultBodyLimit::max(MAX_JSON))
        .layer(CompressionLayer::new().compress_when(DefaultPredicate::new().and(NotForContentType::const_new("video/")).and(NotForContentType::const_new("audio/"))))
        .with_state(state)
}

/// Cancelled when the server starts to stop. Endless responses (the event stream) end on it, or
/// graceful shutdown would wait for them forever.
static STOPPING: std::sync::LazyLock<tokio_util::sync::CancellationToken> = std::sync::LazyLock::new(Default::default);

pub async fn serve(engine: Engine, opts: ServerOptions) -> anyhow::Result<()> {
    let addr = opts.addr.unwrap_or_else(|| SocketAddr::from(([127, 0, 0, 1], 4747)));
    let listener = tokio::net::TcpListener::bind(addr).await.map_err(|e| anyhow::anyhow!("cannot listen on {addr}: {e}"))?;
    tracing::info!("Pictesa is running at http://{addr}");
    let server = axum::serve(listener, router(engine, opts).into_make_service_with_connect_info::<SocketAddr>()).with_graceful_shutdown(async {
        shutdown().await;
        STOPPING.cancel();
    });
    // Open downloads and uploads get a few seconds to finish, then the process stops anyway.
    let deadline = async {
        STOPPING.cancelled().await;
        tokio::time::sleep(Duration::from_secs(10)).await;
    };
    tokio::select! {
        done = server => done?,
        _ = deadline => tracing::warn!("Stopped with requests still open."),
    }
    Ok(())
}

async fn shutdown() {
    let ctrl_c = async {
        let _ = tokio::signal::ctrl_c().await;
    };
    #[cfg(unix)]
    let term = async {
        if let Ok(mut s) = tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate()) {
            s.recv().await;
        }
    };
    #[cfg(not(unix))]
    let term = std::future::pending::<()>();
    tokio::select! { _ = ctrl_c => {}, _ = term => {} }
}

// ---------- health and auth ----------

async fn health(State(s): State<AppState>) -> Json<Value> {
    Json(json!({
        "ok": true,
        "version": env!("CARGO_PKG_VERSION"),
        "mode": mode_name(s.engine.mode()),
        "ffmpeg": s.engine.has_ffmpeg(),
        "data_dir": (s.engine.mode() == Mode::Local).then(|| s.engine.data_dir().display().to_string()),
    }))
}

fn mode_name(m: Mode) -> &'static str {
    match m {
        Mode::Local => "local",
        Mode::Hosted => "hosted",
    }
}

async fn me(State(s): State<AppState>, req: Request) -> Json<Value> {
    let caller = req.extensions().get::<Caller>().cloned();
    let needs_token = s.engine.mode() == Mode::Local && s.opts.token.is_some() && caller.is_none();
    Json(json!({
        "mode": mode_name(s.engine.mode()),
        "user": caller.and_then(|c| c.user),
        "needs_token": needs_token,
    }))
}

#[derive(Deserialize)]
struct Credentials {
    email: String,
    password: String,
    name: Option<String>,
}

fn session_cookie(s: &AppState, value: &str, max_age: i64) -> HeaderValue {
    let secure = if s.opts.secure_cookies { "; Secure" } else { "" };
    HeaderValue::from_str(&format!("{SESSION_COOKIE}={value}; Path=/; HttpOnly; SameSite=Lax; Max-Age={max_age}{secure}")).unwrap()
}

fn hosted_only(s: &AppState) -> ApiResult<()> {
    if s.engine.mode() != Mode::Hosted {
        return Err(bad("Accounts are only used in hosted mode.", None));
    }
    Ok(())
}

/// The client's address: the TCP peer, or, when the peer is a trusted proxy
/// (`PICTESA_TRUSTED_PROXIES`, comma separated), the last address that proxy appended.
fn client_ip(peer: std::net::SocketAddr, headers: &HeaderMap) -> String {
    let trusted: Vec<String> = std::env::var("PICTESA_TRUSTED_PROXIES").unwrap_or_default().split(',').map(|s| s.trim().to_string()).filter(|s| !s.is_empty()).collect();
    let peer_ip = peer.ip().to_string();
    if !trusted.contains(&peer_ip) {
        return peer_ip;
    }
    let chain: Vec<String> = headers
        .get_all("x-forwarded-for")
        .iter()
        .filter_map(|v| v.to_str().ok())
        .flat_map(|v| v.split(','))
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .collect();
    // Walk from the proxy backwards; the first address that is not a trusted proxy is the client.
    chain.into_iter().rev().find(|ip| !trusted.contains(ip)).unwrap_or(peer_ip)
}

/// Few sign-in attempts per address and per account each minute (Argon2 is deliberately slow).
fn throttle(peer: std::net::SocketAddr, headers: &HeaderMap, email: &str) -> ApiResult<()> {
    use std::{collections::HashMap, sync::Mutex, time::Instant};
    static SEEN: std::sync::OnceLock<Mutex<HashMap<String, (Instant, u32)>>> = std::sync::OnceLock::new();
    let mut seen = SEEN.get_or_init(|| Mutex::new(HashMap::new())).lock().unwrap();
    if seen.len() > 50_000 {
        seen.retain(|_, (t, _)| t.elapsed().as_secs() < 60);
    }
    for (key, limit) in [(format!("ip:{}", client_ip(peer, headers)), 20), (format!("acct:{}", email.trim().to_lowercase()), 10)] {
        let entry = seen.entry(key).or_insert((Instant::now(), 0));
        if entry.0.elapsed().as_secs() >= 60 {
            *entry = (Instant::now(), 0);
        }
        entry.1 += 1;
        if entry.1 > limit {
            return Err(ApiError(EngineError::TooMany("Too many attempts. Wait a minute and try again.".into())));
        }
    }
    Ok(())
}

async fn signup(State(s): State<AppState>, axum::extract::ConnectInfo(peer): axum::extract::ConnectInfo<SocketAddr>, headers: HeaderMap, Json(b): Json<Credentials>) -> ApiResult<Response> {
    hosted_only(&s)?;
    throttle(peer, &headers, &b.email)?;
    let user = s.engine.signup(&b.email, &b.password, b.name.as_deref()).await?;
    let session = s.engine.create_session(&user.id).await?;
    let mut resp = Json(json!(user)).into_response();
    resp.headers_mut().insert(header::SET_COOKIE, session_cookie(&s, &session, pictesa_engine::auth::SESSION_DAYS * 86_400));
    Ok(resp)
}

async fn login(State(s): State<AppState>, axum::extract::ConnectInfo(peer): axum::extract::ConnectInfo<SocketAddr>, headers: HeaderMap, Json(b): Json<Credentials>) -> ApiResult<Response> {
    hosted_only(&s)?;
    throttle(peer, &headers, &b.email)?;
    let user = s.engine.login(&b.email, &b.password).await?;
    let session = s.engine.create_session(&user.id).await?;
    let mut resp = Json(json!(user)).into_response();
    resp.headers_mut().insert(header::SET_COOKIE, session_cookie(&s, &session, pictesa_engine::auth::SESSION_DAYS * 86_400));
    Ok(resp)
}

async fn logout(State(s): State<AppState>, headers: HeaderMap) -> ApiResult<Response> {
    if let Some(session) = cookie(&headers, SESSION_COOKIE) {
        s.engine.end_session(&session).await?;
    }
    let mut resp = StatusCode::NO_CONTENT.into_response();
    resp.headers_mut().insert(header::SET_COOKIE, session_cookie(&s, "", 0));
    Ok(resp)
}

#[derive(Deserialize)]
struct TokenLogin {
    token: String,
}

/// Local mode with an access token: the UI trades the token for a cookie.
async fn token_login(State(s): State<AppState>, Json(b): Json<TokenLogin>) -> ApiResult<Response> {
    let Some(expected) = &s.opts.token else { return Ok(StatusCode::NO_CONTENT.into_response()) };
    if !same(b.token.trim(), expected) {
        return Err(ApiError(EngineError::Unauthorized("That access token is wrong.".into())));
    }
    let secure = if s.opts.secure_cookies { "; Secure" } else { "" };
    let mut resp = StatusCode::NO_CONTENT.into_response();
    resp.headers_mut().insert(
        header::SET_COOKIE,
        HeaderValue::from_str(&format!("{TOKEN_COOKIE}={}; Path=/; HttpOnly; SameSite=Strict; Max-Age=31536000{secure}", expected)).unwrap(),
    );
    Ok(resp)
}

async fn list_tokens(State(s): State<AppState>, c: Caller) -> ApiResult<Json<Value>> {
    Ok(Json(json!({ "tokens": s.engine.tokens(&c.user_id).await? })))
}

#[derive(Deserialize)]
struct NewTokenBody {
    name: String,
}

async fn create_token(State(s): State<AppState>, c: Caller, Json(b): Json<NewTokenBody>) -> ApiResult<Json<Value>> {
    hosted_only(&s)?;
    Ok(Json(json!(s.engine.create_token(&c.user_id, &b.name).await?)))
}

async fn delete_token(State(s): State<AppState>, c: Caller, Path(id): Path<String>) -> ApiResult<StatusCode> {
    s.engine.delete_token(&c.user_id, &id).await?;
    Ok(StatusCode::NO_CONTENT)
}

// ---------- providers and models ----------

async fn providers(State(s): State<AppState>, c: Caller) -> ApiResult<Json<Value>> {
    Ok(Json(json!({ "providers": s.engine.providers(&c.user_id).await? })))
}

#[derive(Deserialize)]
struct ProviderValues {
    values: Map<String, Value>,
}

async fn set_provider(State(s): State<AppState>, c: Caller, Path(id): Path<String>, Json(b): Json<ProviderValues>) -> ApiResult<Json<Value>> {
    Ok(Json(json!(s.engine.set_credentials(&c.user_id, &id, &b.values).await?)))
}

async fn clear_provider(State(s): State<AppState>, c: Caller, Path(id): Path<String>) -> ApiResult<StatusCode> {
    s.engine.clear_credentials(&c.user_id, &id).await?;
    Ok(StatusCode::NO_CONTENT)
}

async fn check_provider(State(s): State<AppState>, c: Caller, Path(id): Path<String>) -> ApiResult<Json<Value>> {
    Ok(Json(match s.engine.check_provider(&c.user_id, &id).await? {
        Ok(()) => json!({ "ok": true }),
        Err(e) => json!({ "ok": false, "error": e }),
    }))
}

async fn models(State(s): State<AppState>, c: Caller, Query(f): Query<ModelFilter>) -> ApiResult<Json<Value>> {
    Ok(Json(json!({ "models": s.engine.models(&c.user_id, &f).await? })))
}

async fn model(State(s): State<AppState>, c: Caller, Path(id): Path<String>) -> ApiResult<Json<Value>> {
    Ok(Json(json!(s.engine.model(&c.user_id, &id).await?)))
}

async fn refresh_models(State(s): State<AppState>, c: Caller) -> ApiResult<Json<Value>> {
    if s.engine.mode() == Mode::Hosted && c.user.as_ref().is_none_or(|u| u.role != "admin") {
        return Err(ApiError(EngineError::Forbidden("Only the administrator can refresh catalogs.".into())));
    }
    Ok(Json(json!({ "models": s.engine.refresh_models(&c.user_id).await })))
}

#[derive(Deserialize)]
struct PresetQuery {
    output: Option<String>,
}

async fn presets(State(s): State<AppState>, _c: Caller, Query(q): Query<PresetQuery>) -> Json<Value> {
    let output: Option<MediaKind> = q.output.and_then(|o| serde_json::from_value(json!(o)).ok());
    Json(json!({ "presets": s.engine.presets(output) }))
}

#[derive(Deserialize)]
struct EnhanceBody {
    prompt: String,
    output: Option<MediaKind>,
}

async fn enhance_prompt(State(s): State<AppState>, c: Caller, Json(b): Json<EnhanceBody>) -> ApiResult<Json<Value>> {
    let prompt = s.engine.enhance_prompt(&c.user_id, &b.prompt, b.output.unwrap_or(MediaKind::Image)).await?;
    Ok(Json(json!({ "prompt": prompt })))
}

// ---------- generations ----------

async fn create_job(State(s): State<AppState>, c: Caller, Json(b): Json<CreateGeneration>) -> ApiResult<Response> {
    let created = s.engine.create_generation(&c.user_id, b).await?;
    Ok((StatusCode::CREATED, Json(json!(created))).into_response())
}

async fn list_jobs(State(s): State<AppState>, c: Caller, Query(f): Query<JobFilter>) -> ApiResult<Json<Value>> {
    let (jobs, next) = s.engine.jobs(&c.user_id, &f).await?;
    Ok(Json(json!({ "jobs": jobs, "next": next })))
}

async fn get_job(State(s): State<AppState>, c: Caller, Path(id): Path<String>) -> ApiResult<Json<Value>> {
    Ok(Json(json!(s.engine.job(&c.user_id, &id).await?)))
}

async fn cancel_job(State(s): State<AppState>, c: Caller, Path(id): Path<String>) -> ApiResult<Json<Value>> {
    Ok(Json(json!(s.engine.cancel(&c.user_id, &id).await?)))
}

async fn retry_job(State(s): State<AppState>, c: Caller, Path(id): Path<String>) -> ApiResult<Response> {
    Ok((StatusCode::CREATED, Json(json!(s.engine.retry(&c.user_id, &id).await?))).into_response())
}

async fn job_preview(State(s): State<AppState>, c: Caller, Path(id): Path<String>) -> ApiResult<Response> {
    let (bytes, mime) = s.engine.job_preview(&c.user_id, &id).await?;
    let mut resp = Response::new(Body::from(bytes));
    let h = resp.headers_mut();
    h.insert(header::CONTENT_TYPE, HeaderValue::from_str(&mime).unwrap_or(HeaderValue::from_static("image/jpeg")));
    h.insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
    h.insert("x-content-type-options", HeaderValue::from_static("nosniff"));
    Ok(resp)
}

async fn delete_job(State(s): State<AppState>, c: Caller, Path(id): Path<String>) -> ApiResult<StatusCode> {
    s.engine.delete_job(&c.user_id, &id).await?;
    Ok(StatusCode::NO_CONTENT)
}

// ---------- assets ----------

async fn list_assets(State(s): State<AppState>, c: Caller, Query(f): Query<AssetFilter>) -> ApiResult<Json<Value>> {
    let (assets, next) = s.engine.assets(&c.user_id, &f).await?;
    Ok(Json(json!({ "assets": assets, "next": next })))
}

/// Stores every file it can; files that fail are reported in `errors` instead of failing the batch.
async fn upload_assets(State(s): State<AppState>, c: Caller, mut form: Multipart) -> ApiResult<Json<Value>> {
    let mut out = Vec::new();
    let mut errors = Vec::new();
    while let Some(field) = form.next_field().await.map_err(|e| bad(&format!("Upload failed: {e}"), Some("file")))? {
        let name = field.file_name().map(String::from);
        let stream = field.map(|chunk| chunk.map_err(|e| std::io::Error::other(e.to_string())));
        match s.engine.upload_stream(&c.user_id, Box::pin(stream), name.as_deref()).await {
            Ok(asset) => out.push(asset),
            Err(EngineError::Validation { message, .. }) => errors.push(json!({ "name": name, "error": message })),
            Err(e) => return Err(e.into()),
        }
    }
    if out.is_empty() {
        let first = errors.first().and_then(|e| e["error"].as_str()).unwrap_or("Choose at least one file.").to_string();
        return Err(bad(&first, Some("file")));
    }
    Ok(Json(json!({ "assets": out, "errors": errors })))
}

#[derive(Deserialize)]
struct ImportBody {
    url: String,
}

async fn import_asset(State(s): State<AppState>, c: Caller, Json(b): Json<ImportBody>) -> ApiResult<Json<Value>> {
    Ok(Json(json!({ "assets": [s.engine.import_url(&c.user_id, &b.url).await?] })))
}

async fn get_asset(State(s): State<AppState>, c: Caller, Path(id): Path<String>) -> ApiResult<Json<Value>> {
    let (asset, job) = s.engine.asset(&c.user_id, &id).await?;
    let mut v = json!(asset);
    v["job"] = json!(job);
    Ok(Json(v))
}

#[derive(Deserialize)]
struct AssetPatch {
    favorite: Option<bool>,
}

async fn patch_asset(State(s): State<AppState>, c: Caller, Path(id): Path<String>, Json(b): Json<AssetPatch>) -> ApiResult<Json<Value>> {
    match b.favorite {
        Some(f) => Ok(Json(json!(s.engine.set_favorite(&c.user_id, &id, f).await?))),
        None => Ok(Json(json!(s.engine.asset(&c.user_id, &id).await?.0))),
    }
}

#[derive(Deserialize)]
struct DeleteQuery {
    permanent: Option<bool>,
}

async fn delete_asset(State(s): State<AppState>, c: Caller, Path(id): Path<String>, Query(q): Query<DeleteQuery>) -> ApiResult<StatusCode> {
    s.engine.trash(&c.user_id, &id, q.permanent.unwrap_or(false)).await?;
    Ok(StatusCode::NO_CONTENT)
}

async fn restore_asset(State(s): State<AppState>, c: Caller, Path(id): Path<String>) -> ApiResult<Json<Value>> {
    Ok(Json(json!(s.engine.restore(&c.user_id, &id).await?)))
}

// ---------- ComfyUI workflow import ----------

#[derive(Deserialize)]
struct ImportWorkflowBody {
    name: String,
    workflow: Value,
}

async fn import_workflow(State(s): State<AppState>, c: Caller, Json(b): Json<ImportWorkflowBody>) -> ApiResult<Response> {
    Ok((StatusCode::CREATED, Json(json!(s.engine.import_comfy_workflow(&c.user_id, &b.name, b.workflow).await?))).into_response())
}

// ---------- characters ----------

async fn characters(State(s): State<AppState>, c: Caller) -> ApiResult<Json<Value>> {
    Ok(Json(json!({ "characters": s.engine.characters(&c.user_id).await? })))
}

async fn character(State(s): State<AppState>, c: Caller, Path(id): Path<String>) -> ApiResult<Json<Value>> {
    Ok(Json(json!(s.engine.character(&c.user_id, &id).await?)))
}

async fn create_character(State(s): State<AppState>, c: Caller, Json(b): Json<pictesa_engine::characters::CharacterInput>) -> ApiResult<Response> {
    Ok((StatusCode::CREATED, Json(json!(s.engine.create_character(&c.user_id, b).await?))).into_response())
}

async fn update_character(State(s): State<AppState>, c: Caller, Path(id): Path<String>, Json(b): Json<pictesa_engine::characters::CharacterInput>) -> ApiResult<Json<Value>> {
    Ok(Json(json!(s.engine.update_character(&c.user_id, &id, b).await?)))
}

async fn delete_character(State(s): State<AppState>, c: Caller, Path(id): Path<String>) -> ApiResult<StatusCode> {
    s.engine.delete_character(&c.user_id, &id).await?;
    Ok(StatusCode::NO_CONTENT)
}

// ---------- recipes ----------

async fn recipes(State(s): State<AppState>, c: Caller) -> ApiResult<Json<Value>> {
    Ok(Json(json!({ "recipes": s.engine.recipes(&c.user_id).await? })))
}

async fn run_recipe(State(s): State<AppState>, c: Caller, Path(id): Path<String>, Json(b): Json<pictesa_engine::recipes::RunRecipe>) -> ApiResult<Response> {
    Ok((StatusCode::CREATED, Json(json!(s.engine.run_recipe(&c.user_id, &id, b).await?))).into_response())
}

#[derive(Deserialize)]
struct LimitQuery {
    limit: Option<u32>,
}

async fn recipe_runs(State(s): State<AppState>, c: Caller, Query(q): Query<LimitQuery>) -> ApiResult<Json<Value>> {
    Ok(Json(json!({ "runs": s.engine.recipe_runs(&c.user_id, q.limit.unwrap_or(30)).await? })))
}

async fn recipe_run(State(s): State<AppState>, c: Caller, Path(id): Path<String>) -> ApiResult<Json<Value>> {
    Ok(Json(json!(s.engine.recipe_run(&c.user_id, &id).await?)))
}

async fn cancel_recipe_run(State(s): State<AppState>, c: Caller, Path(id): Path<String>) -> ApiResult<Json<Value>> {
    Ok(Json(json!(s.engine.cancel_recipe_run(&c.user_id, &id).await?)))
}

// ---------- billing ----------

async fn billing(State(s): State<AppState>, c: Caller) -> ApiResult<Json<Value>> {
    Ok(Json(json!(s.engine.billing_view(&c.user_id).await?)))
}

#[derive(Deserialize)]
struct CheckoutBody {
    amount_cents: i64,
    request_id: String,
}

async fn checkout(State(s): State<AppState>, c: Caller, Json(b): Json<CheckoutBody>) -> ApiResult<Json<Value>> {
    let email = c.user.as_ref().and_then(|u| u.email.clone());
    let url = s.engine.create_checkout(&c.user_id, email.as_deref(), &b.request_id, b.amount_cents).await?;
    Ok(Json(json!({ "url": url })))
}

/// Stripe calls this without a session; the signature authenticates it.
async fn stripe_webhook(State(s): State<AppState>, headers: HeaderMap, body: axum::body::Bytes) -> ApiResult<Json<Value>> {
    if body.len() > 1_000_000 {
        return Err(bad("Event too large.", None));
    }
    let sig = headers.get("stripe-signature").and_then(|v| v.to_str().ok()).unwrap_or("");
    s.engine.apply_stripe_event(&body, sig).await?;
    Ok(Json(json!({ "ok": true })))
}

// ---------- events ----------

async fn events(State(s): State<AppState>, c: Caller, headers: HeaderMap) -> Sse<impl Stream<Item = Result<SseEvent, Infallible>>> {
    static NEXT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(1);
    let rx = s.engine.subscribe();
    let user = c.user_id.clone();
    // A reconnecting browser sends Last-Event-ID; events are not replayed, so ask it to refetch.
    let reconnect = headers.contains_key("last-event-id");
    let first = futures::stream::iter(reconnect.then(|| Ok(SseEvent::default().event("resync").data("{}"))));
    let stream = tokio_stream::wrappers::BroadcastStream::new(rx).filter_map(move |item| {
        let user = user.clone();
        async move {
            match item {
                Ok(ev) if ev.user_id == user => {
                    let id = NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                    Some(Ok(SseEvent::default().id(id.to_string()).event(ev.kind).data(ev.data.to_string())))
                }
                Ok(_) => None,
                // A slow client missed events; tell it to refetch.
                Err(_) => Some(Ok(SseEvent::default().event("resync").data("{}"))),
            }
        }
    });
    Sse::new(first.chain(stream).take_until(STOPPING.cancelled())).keep_alive(KeepAlive::new().interval(Duration::from_secs(20)).event(SseEvent::default().event("ping").data("{}")))
}

use futures::StreamExt;

// ---------- files ----------

#[derive(Deserialize)]
struct FileQuery {
    download: Option<String>,
    /// Signed key and expiry that let a provider fetch an input file without a session (hosted mode).
    k: Option<String>,
    e: Option<i64>,
}

async fn file(State(s): State<AppState>, caller: Option<Caller>, Path(id): Path<String>, Query(q): Query<FileQuery>, req: Request) -> Response {
    let user = match (&caller, &q.k) {
        (Some(c), _) => Some(c.user_id.clone()),
        (None, Some(k)) if s.engine.file_key_ok(&id, k, q.e.unwrap_or(0)) => None,
        _ => return ApiError(EngineError::Unauthorized("Sign in to view this file.".into())).into_response(),
    };
    let (path, mime, name) = match s.engine.asset_file(user.as_deref(), &id).await {
        Ok(v) => v,
        Err(e) => return ApiError(e).into_response(),
    };
    let mut resp = match ServeFile::new_with_mime(&path, &mime.parse().unwrap_or(mime_guess::mime::APPLICATION_OCTET_STREAM)).oneshot(req).await {
        Ok(r) => r.map(Body::new),
        Err(e) => return (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()).into_response(),
    };
    let h = resp.headers_mut();
    h.insert(header::CACHE_CONTROL, HeaderValue::from_static("private, max-age=31536000, immutable"));
    h.insert("x-content-type-options", HeaderValue::from_static("nosniff"));
    // Files are media only; even if one were misread, it cannot run script on this origin.
    h.insert("content-security-policy", HeaderValue::from_static("sandbox; default-src 'none'; img-src 'self'; media-src 'self'"));
    if q.download.is_some() {
        if let Ok(v) = HeaderValue::from_str(&format!("attachment; filename=\"{name}\"")) {
            h.insert(header::CONTENT_DISPOSITION, v);
        }
    }
    resp
}

async fn thumb(State(s): State<AppState>, caller: Option<Caller>, Path(id): Path<String>, req: Request) -> Response {
    let Some(c) = caller else { return ApiError(EngineError::Unauthorized("Sign in to view this file.".into())).into_response() };
    let path = match s.engine.asset_thumb(Some(&c.user_id), &id).await {
        Ok(p) => p,
        Err(e) => return ApiError(e).into_response(),
    };
    let mut resp = match ServeFile::new_with_mime(&path, &mime_guess::mime::IMAGE_JPEG).oneshot(req).await {
        Ok(r) => r.map(Body::new),
        Err(e) => return (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()).into_response(),
    };
    resp.headers_mut().insert(header::CACHE_CONTROL, HeaderValue::from_static("private, max-age=31536000, immutable"));
    resp
}

// ---------- MCP ----------

async fn mcp_http(State(s): State<AppState>, caller: Option<Caller>, raw: axum::body::Bytes) -> Response {
    let Some(c) = caller else {
        return (StatusCode::UNAUTHORIZED, Json(json!({ "jsonrpc": "2.0", "id": null, "error": { "code": -32001, "message": "Unauthorized: send Authorization: Bearer <token>" } })))
            .into_response();
    };
    let ctx = mcp::McpContext { engine: s.engine.clone(), user_id: c.user_id, local_files: s.engine.mode() == Mode::Local };
    let body: Value = match serde_json::from_slice(&raw) {
        Ok(v) => v,
        Err(e) => return Json(json!({ "jsonrpc": "2.0", "id": null, "error": { "code": -32700, "message": format!("Parse error: {e}") } })).into_response(),
    };
    match body {
        Value::Array(batch) => {
            let mut out = Vec::new();
            for msg in batch {
                if let Some(r) = mcp::handle(&ctx, msg).await {
                    out.push(r);
                }
            }
            if out.is_empty() {
                StatusCode::ACCEPTED.into_response()
            } else {
                Json(Value::Array(out)).into_response()
            }
        }
        msg => match mcp::handle(&ctx, msg).await {
            Some(r) => Json(r).into_response(),
            None => StatusCode::ACCEPTED.into_response(),
        },
    }
}

impl axum::extract::OptionalFromRequestParts<AppState> for Caller {
    type Rejection = Infallible;
    async fn from_request_parts(parts: &mut Parts, _state: &AppState) -> Result<Option<Self>, Self::Rejection> {
        Ok(parts.extensions.get::<Caller>().cloned())
    }
}
