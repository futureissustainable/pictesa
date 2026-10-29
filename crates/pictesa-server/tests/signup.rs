use axum::{
    body::{to_bytes, Body},
    extract::ConnectInfo,
    http::{Request, StatusCode},
};
use pictesa_engine::{Config, Engine, Mode};
use pictesa_server::{router, ServerOptions};
use serde_json::{json, Value};
use tower::ServiceExt;

#[tokio::test]
async fn closed_signup_refuses_new_accounts_and_says_so() {
    let mut cfg = Config::local(std::env::temp_dir().join(format!("pictesa-server-signup-{}", pictesa_engine::new_id())));
    cfg.mode = Mode::Hosted;
    let e = Engine::open(cfg).await.unwrap();
    let app = router(e, ServerOptions { closed_signup: true, ..Default::default() });
    let peer = ConnectInfo(std::net::SocketAddr::from(([203, 0, 113, 7], 5000)));

    let signup = Request::post("/api/auth/signup")
        .header("Content-Type", "application/json")
        .extension(peer)
        .body(Body::from(json!({"email":"stranger@example.com","password":"correct horse battery"}).to_string()))
        .unwrap();
    assert_eq!(app.clone().oneshot(signup).await.unwrap().status(), StatusCode::FORBIDDEN);

    let me = app.oneshot(Request::get("/api/auth/me").extension(peer).body(Body::empty()).unwrap()).await.unwrap();
    let me: Value = serde_json::from_slice(&to_bytes(me.into_body(), 1 << 16).await.unwrap()).unwrap();
    assert_eq!(me["signup"], false);
}
