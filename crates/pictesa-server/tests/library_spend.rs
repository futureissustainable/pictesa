use axum::{
    body::{to_bytes, Body},
    http::{Request, StatusCode},
    Router,
};
use base64::Engine as _;
use pictesa_engine::{Config, Engine, Mode};
use pictesa_server::{router, ServerOptions};
use serde_json::{json, Value};
use tower::ServiceExt;

async fn call(app: &Router, token: &str, method: &str, path: &str, body: Value) -> (StatusCode, Vec<u8>) {
    let request = Request::builder()
        .method(method)
        .uri(path)
        .header("Authorization", format!("Bearer {token}"))
        .header("Content-Type", "application/json")
        .body(Body::from(body.to_string()))
        .unwrap();
    let response = app.clone().oneshot(request).await.unwrap();
    (response.status(), to_bytes(response.into_body(), 4 * 1024 * 1024).await.unwrap().to_vec())
}

#[tokio::test]
async fn hosted_http_and_mcp_enforce_collection_tag_zip_and_spend_ownership() {
    let mut cfg = Config::local(std::env::temp_dir().join(format!("pictesa-server-library-{}", pictesa_engine::new_id())));
    cfg.mode = Mode::Hosted;
    let e = Engine::open(cfg).await.unwrap();
    let a = e.signup("http-a@example.com", "correct horse battery", None).await.unwrap();
    let b = e.signup("http-b@example.com", "correct horse battery", None).await.unwrap();
    let a_token = e.create_token(&a.id, "test").await.unwrap().token;
    let b_token = e.create_token(&b.id, "test").await.unwrap().token;
    let bytes = base64::engine::general_purpose::STANDARD
        .decode("iVBORw0KGgoAAAANSUhEUgAAAAIAAAACCAIAAAD91JpzAAAAC0lEQVR4nGNgQAYAAA4AAamRc7EAAAAASUVORK5CYII=")
        .unwrap();
    // An independently encoded PNG fixture, read through the normal upload path.
    let asset = e.upload(&a.id, &bytes, Some("private.png"), None).await.unwrap();
    let collection = e.create_collection(&a.id, "Private").await.unwrap();
    e.tag_assets(&a.id, &[asset.id.clone()], "Private tag", false).await.unwrap();
    let app = router(e.clone(), ServerOptions::default());
    let membership = format!("/api/collections/{}/assets", collection.id);
    for (method, path, body) in [
        ("PATCH", format!("/api/collections/{}", collection.id), json!({"name":"stolen"})),
        ("DELETE", format!("/api/collections/{}", collection.id), json!({})),
        ("POST", membership.clone(), json!({"asset_ids":[asset.id]})),
        ("DELETE", membership.clone(), json!({"asset_ids":[asset.id]})),
        ("POST", "/api/tags".into(), json!({"asset_ids":[asset.id],"tag":"stolen"})),
        ("POST", "/api/assets/download".into(), json!({"asset_ids":[asset.id]})),
        ("POST", "/api/assets/bulk".into(), json!({"asset_ids":[asset.id],"action":"trash"})),
        ("GET", format!("/api/assets/{}/tags", asset.id), json!({})),
    ] {
        assert_eq!(call(&app, &b_token, method, &path, body).await.0, StatusCode::NOT_FOUND, "{method} {path}");
    }
    for (path, key) in [("/api/collections", "collections"), ("/api/tags", "tags")] {
        let (status, body) = call(&app, &b_token, "GET", path, json!({})).await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(serde_json::from_slice::<Value>(&body).unwrap()[key], json!([]));
    }
    assert_eq!(call(&app, &a_token, "POST", &membership, json!({"asset_ids":[asset.id]})).await.0, StatusCode::NO_CONTENT);
    let query = format!("/api/assets?collection={}&tag=Private%20tag", collection.id);
    let (_, body) = call(&app, &b_token, "GET", &query, json!({})).await;
    assert_eq!(serde_json::from_slice::<Value>(&body).unwrap()["assets"], json!([]));
    let (status, zip) = call(&app, &a_token, "POST", "/api/assets/download", json!({"asset_ids":[asset.id]})).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(&zip[..4], b"PK\x03\x04");
    assert!(zip.windows(bytes.len()).any(|data| data == bytes));
    assert_eq!(&zip[zip.len() - 22..zip.len() - 18], b"PK\x05\x06");
    assert_eq!(call(&app, &a_token, "POST", "/api/assets/download", json!({"asset_ids":vec![asset.id.clone();101]})).await.0, StatusCode::BAD_REQUEST);
    let own_ctx = pictesa_server::mcp::McpContext { engine: e.clone(), user_id: a.id.clone(), local_files: false };
    for (name, arguments) in [("list_collections", json!({})), ("add_to_collection", json!({"collection_id":collection.id,"asset_ids":[asset.id]}))] {
        let result = pictesa_server::mcp::handle(&own_ctx, json!({"jsonrpc":"2.0","id":1,"method":"tools/call","params":{"name":name,"arguments":arguments}}))
            .await
            .unwrap();
        assert_eq!(result["result"]["isError"], false);
    }
    let ctx = pictesa_server::mcp::McpContext { engine: e, user_id: b.id, local_files: false };
    let result = pictesa_server::mcp::handle(&ctx, json!({"jsonrpc":"2.0","id":1,"method":"tools/call","params":{"name":"add_to_collection","arguments":{"collection_id":collection.id,"asset_ids":[asset.id]}}})).await.unwrap();
    assert_eq!(result["result"]["isError"], true);
    let (_, body) = call(&app, &a_token, "PUT", "/api/spend/budgets", json!({"daily_usd":2,"monthly_usd":10})).await;
    assert_eq!(serde_json::from_slice::<Value>(&body).unwrap()["daily"]["cap_usd"], 2.0);
    let (_, body) = call(&app, &b_token, "GET", "/api/spend", json!({})).await;
    let spend: Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(spend["daily"]["cap_usd"], Value::Null);
    assert_eq!(spend["today_usd"], 0.0);
}
