//! Boards over HTTP and MCP: status codes for each role, the conflict answer, files shared through
//! a board, and the agent tools on the free mock provider.

use axum::{
    body::{to_bytes, Body},
    http::{Request, StatusCode},
    Router,
};
use pictesa_engine::{Config, Engine, Mode};
use pictesa_server::{mcp, router, ServerOptions};
use serde_json::{json, Value};
use tower::ServiceExt;

async fn call(app: &Router, token: &str, method: &str, path: &str, body: Value) -> (StatusCode, Value) {
    let request = Request::builder()
        .method(method)
        .uri(path)
        .header("Authorization", format!("Bearer {token}"))
        .header("Content-Type", "application/json")
        .body(Body::from(body.to_string()))
        .unwrap();
    let response = app.clone().oneshot(request).await.unwrap();
    let status = response.status();
    let bytes = to_bytes(response.into_body(), 4 * 1024 * 1024).await.unwrap();
    (status, serde_json::from_slice(&bytes).unwrap_or(Value::Null))
}

async fn tool(ctx: &mcp::McpContext, name: &str, arguments: Value) -> Value {
    let answer = mcp::handle(ctx, json!({ "jsonrpc": "2.0", "id": 1, "method": "tools/call", "params": { "name": name, "arguments": arguments } })).await.unwrap();
    let result = &answer["result"];
    assert_eq!(result["isError"], false, "{name}: {}", result["content"][0]["text"]);
    serde_json::from_str(result["content"][0]["text"].as_str().unwrap()).unwrap()
}

/// A 2 by 2 PNG, so this test needs no image encoder.
fn png() -> Vec<u8> {
    use base64::Engine as _;
    base64::engine::general_purpose::STANDARD.decode("iVBORw0KGgoAAAANSUhEUgAAAAIAAAACCAIAAAD91JpzAAAAC0lEQVR4nGNgQAYAAA4AAamRc7EAAAAASUVORK5CYII=").unwrap()
}

#[tokio::test(flavor = "multi_thread")]
async fn boards_over_http_and_mcp_follow_roles_and_report_conflicts() {
    std::env::set_var("PICTESA_MOCK", "1");
    let mut cfg = Config::local(std::env::temp_dir().join(format!("pictesa-server-boards-{}", pictesa_engine::new_id())));
    cfg.mode = Mode::Hosted;
    let e = Engine::open(cfg).await.unwrap();
    e.start();
    let owner = e.signup("ana@example.com", "correct horse battery", Some("Ana")).await.unwrap();
    let viewer = e.signup("bo@example.com", "correct horse battery", None).await.unwrap();
    let stranger = e.signup("cy@example.com", "correct horse battery", None).await.unwrap();
    let owner_token = e.create_token(&owner.id, "test").await.unwrap().token;
    let viewer_token = e.create_token(&viewer.id, "test").await.unwrap().token;
    let stranger_token = e.create_token(&stranger.id, "test").await.unwrap().token;
    let photo = e.upload(&owner.id, &png(), Some("photo.png"), None).await.unwrap();
    let app = router(e.clone(), ServerOptions::default());

    let (status, board) = call(&app, &owner_token, "POST", "/api/boards", json!({ "name": "Moodboard" })).await;
    assert_eq!(status, StatusCode::CREATED);
    let id = board["id"].as_str().unwrap().to_string();
    let ops = format!("/api/boards/{id}/ops");
    let (status, changes) = call(&app, &owner_token, "POST", &ops, json!({ "ops": [
        { "op": "add_node", "node": { "id": "photo", "kind": "asset", "asset_id": photo.id, "x": 0, "y": 0 } },
        { "op": "add_node", "node": { "id": "note", "kind": "note", "text": "warm light", "x": 300, "y": 0 } },
    ] }))
    .await;
    assert_eq!(status, StatusCode::OK, "{changes}");
    assert_eq!(changes["version"], 1);
    assert_eq!(changes["assets"][&photo.id]["id"], photo.id.as_str());

    // A stale write gets 409 and the current state of what it named; a bad one gets 400.
    let (status, stale) = call(&app, &owner_token, "POST", &ops, json!({ "ops": [{ "op": "update_node", "id": "note", "base": 0, "text": "old view" }] })).await;
    assert_eq!(status, StatusCode::CONFLICT);
    assert_eq!(stale["kind"], "conflict");
    assert_eq!(stale["current"]["nodes"][0]["text"], "warm light");
    let (status, invalid) = call(&app, &owner_token, "POST", &ops, json!({ "ops": [{ "op": "paint_everything" }] })).await;
    assert_eq!((status, invalid["field"].as_str()), (StatusCode::BAD_REQUEST, Some("ops")));

    // Sharing: only the owner shares; the viewer looks, and sees the files on the board.
    let members = format!("/api/boards/{id}/members");
    assert_eq!(call(&app, &owner_token, "POST", &members, json!({ "email": "bo@example.com", "role": "viewer" })).await.0, StatusCode::OK);
    assert_eq!(call(&app, &viewer_token, "POST", &members, json!({ "email": "cy@example.com", "role": "editor" })).await.0, StatusCode::FORBIDDEN);
    let (status, seen) = call(&app, &viewer_token, "GET", &format!("/api/boards/{id}"), Value::Null).await;
    assert_eq!((status, seen["role"].as_str()), (StatusCode::OK, Some("viewer")));
    assert_eq!(call(&app, &viewer_token, "POST", &ops, json!({ "ops": [{ "op": "move_node", "id": "note", "base": 1, "x": 1, "y": 1 }] })).await.0, StatusCode::FORBIDDEN);
    assert_eq!(call(&app, &viewer_token, "GET", &format!("/files/{}", photo.id), Value::Null).await.0, StatusCode::OK);
    let (status, viewers) = call(&app, &viewer_token, "POST", &format!("/api/boards/{id}/presence"), json!({ "client": "tab-1" })).await;
    assert_eq!((status, viewers["viewers"][0]["id"].as_str()), (StatusCode::OK, Some(viewer.id.as_str())));

    // Strangers: the board and its files do not exist for them.
    for (method, path) in [("GET", format!("/api/boards/{id}")), ("POST", ops.clone()), ("GET", format!("/files/{}", photo.id)), ("POST", format!("/api/boards/{id}/copy"))] {
        let body = json!({ "ops": [{ "op": "rename", "name": "mine" }], "asset_ids": [photo.id] });
        assert_eq!(call(&app, &stranger_token, method, &path, body).await.0, StatusCode::NOT_FOUND, "{method} {path}");
    }
    let (_, listed) = call(&app, &stranger_token, "GET", "/api/boards", Value::Null).await;
    assert_eq!(listed["boards"], json!([]));

    // Agents: add a step fed by the photo, run it and get the result back.
    let ctx = mcp::McpContext { engine: e.clone(), user_id: owner.id.clone(), local_files: false };
    let boards = tool(&ctx, "list_boards", json!({})).await;
    assert_eq!(boards["boards"][0]["id"], id.as_str());
    let added = tool(&ctx, "add_to_board", json!({
        "board_id": id,
        "nodes": [{ "id": "paint", "kind": "step", "model": "mock:image", "prompt": "a cabin in warm light" }],
        "edges": [{ "from": "photo", "to": "paint", "role": "reference" }],
    }))
    .await;
    assert_eq!(added["nodes"][0]["id"], "paint");
    let board = tool(&ctx, "get_board", json!({ "id": id })).await;
    assert_eq!(board["edges"][0]["role"], "reference");
    let ran = tool(&ctx, "run_board_step", json!({ "board_id": id, "node_id": "paint", "timeout_seconds": 60 })).await;
    assert_eq!(ran["done"], true, "{ran}");
    assert_eq!(ran["runs"]["paint"]["status"], "succeeded");
    let result = ran["outputs"][0]["id"].as_str().unwrap();
    assert_eq!(call(&app, &viewer_token, "GET", &format!("/files/{result}"), Value::Null).await.0, StatusCode::OK, "members see results");
    assert_eq!(call(&app, &stranger_token, "GET", &format!("/files/{result}"), Value::Null).await.0, StatusCode::NOT_FOUND);
    let viewer_ctx = mcp::McpContext { engine: e.clone(), user_id: viewer.id.clone(), local_files: false };
    let refused = mcp::handle(&viewer_ctx, json!({ "jsonrpc": "2.0", "id": 2, "method": "tools/call", "params": { "name": "run_board_step", "arguments": { "board_id": id, "node_id": "paint" } } })).await.unwrap();
    assert_eq!(refused["result"]["isError"], true);
}
