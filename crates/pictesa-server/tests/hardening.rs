//! What a hosted user or a local caller without the token can learn about the operator.
use axum::{
    body::{to_bytes, Body},
    http::{Request, StatusCode},
    Router,
};
use pictesa_engine::{Config, Engine, Mode};
use pictesa_server::{router, ServerOptions};
use serde_json::{json, Value};
use tower::ServiceExt;

async fn hosted() -> (Engine, Router, String, String) {
    let mut cfg = Config::local(std::env::temp_dir().join(format!("pictesa-server-hardening-{}", pictesa_engine::new_id())));
    cfg.mode = Mode::Hosted;
    let e = Engine::open(cfg).await.unwrap();
    let admin = e.signup("admin@example.com", "correct horse battery", None).await.unwrap();
    let user = e.signup("user@example.com", "correct horse battery", None).await.unwrap();
    let admin_token = e.create_token(&admin.id, "test").await.unwrap().token;
    let user_token = e.create_token(&user.id, "test").await.unwrap().token;
    let app = router(e.clone(), ServerOptions::default());
    (e, app, admin_token, user_token)
}

async fn call(app: &Router, token: Option<&str>, method: &str, path: &str, body: Value) -> (StatusCode, Value) {
    let mut request = Request::builder().method(method).uri(path).header("Content-Type", "application/json");
    if let Some(token) = token {
        request = request.header("Authorization", format!("Bearer {token}"));
    }
    let response = app.clone().oneshot(request.body(Body::from(body.to_string())).unwrap()).await.unwrap();
    let status = response.status();
    let bytes = to_bytes(response.into_body(), 1 << 20).await.unwrap();
    (status, serde_json::from_slice(&bytes).unwrap_or(Value::Null))
}

#[tokio::test]
async fn hosted_users_see_no_operator_details() {
    // The operator's ComfyUI is down: the catalog marks its templates unavailable, naming its address.
    std::env::set_var("COMFYUI_URL", "http://127.0.0.1:1");
    let (e, app, admin, user) = hosted().await;
    let admin_id = e.token_user(&admin).await.unwrap().unwrap().id;
    e.refresh_models_for(&admin_id, "comfyui").await.unwrap();
    let (status, model) = call(&app, Some(&user), "GET", "/api/models/comfyui:flux3-klein-t2i", json!({})).await;
    assert_eq!(status, StatusCode::OK);
    let meta = model["meta"].as_object().unwrap();
    for key in ["dir", "rename"] {
        assert!(!meta.contains_key(key), "{key} in {meta:?}");
    }
    assert!(meta.contains_key("unavailable"), "{meta:?}");
    assert!(!model["meta"].to_string().contains("127.0.0.1"), "{meta:?}");
    assert_eq!(call(&app, Some(&user), "GET", "/api/downloads", json!({})).await.0, StatusCode::FORBIDDEN);
    assert_eq!(call(&app, Some(&admin), "GET", "/api/downloads", json!({})).await.0, StatusCode::OK);
}

#[tokio::test]
async fn the_ai_toolkit_program_is_not_set_through_the_api() {
    let e = Engine::open(Config::local(std::env::temp_dir().join(format!("pictesa-server-toolkit-{}", pictesa_engine::new_id())))).await.unwrap();
    let app = router(e, ServerOptions { token: Some("local-token".into()), ..Default::default() });
    let (status, body) = call(&app, Some("local-token"), "PUT", "/api/providers/ai-toolkit", json!({ "values": { "path": "/tmp/anything" } })).await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{body}");
    // Without the token, health does not name the data folder.
    let (_, health) = call(&app, None, "GET", "/api/health", json!({})).await;
    assert!(health["data_dir"].is_null(), "{health}");
}
