//! Import transport contracts. All files and HTTP sources are local fixtures.
use axum::{
    body::{to_bytes, Body},
    http::{Request, StatusCode},
    routing::get,
    Router,
};
use base64::Engine as _;
use pictesa_engine::{Config, Engine, LOCAL_USER};
use pictesa_server::{
    mcp::{self, McpContext},
    router, ServerOptions,
};
use serde_json::{json, Value};
use tower::ServiceExt;

fn png() -> Vec<u8> {
    base64::engine::general_purpose::STANDARD.decode("iVBORw0KGgoAAAANSUhEUgAAAAQAAAAECAIAAAAmkwkpAAAAEElEQVR4nGP4lGkLRwzEcQCiohmBzIRWUgAAAABJRU5ErkJggg==").unwrap()
}

async fn json_body(response: axum::response::Response) -> Value {
    assert_eq!(response.status(), StatusCode::OK);
    serde_json::from_slice(&to_bytes(response.into_body(), 1024 * 1024).await.unwrap()).unwrap()
}

#[tokio::test]
async fn multipart_url_and_mcp_preserve_import_metadata() {
    let dir = std::env::temp_dir().join(format!(
        "pictesa-import-transport-{}",
        pictesa_core::now_ms()
    ));
    let e = Engine::open(Config::local(&dir)).await.unwrap();
    let app = router(e.clone(), ServerOptions::default());
    // Metadata after the file must work too; the file is streamed, not buffered by the handler.
    let mut body = b"--test\r\nContent-Disposition: form-data; name=\"file\"; filename=\"outside.png\"\r\nContent-Type: image/png\r\n\r\n".to_vec();
    body.extend(png());
    body.extend(b"\r\n--test\r\nContent-Disposition: form-data; name=\"metadata\"\r\n\r\n");
    body.extend(json!({"prompt":"a cedar cabin","negative_prompt":"fog","model":"openai:gpt-image-2.5-sunburst","model_label":"Outside image","origin":"gpt-image","params":{"quality":"high"}}).to_string().as_bytes());
    body.extend(b"\r\n--test--\r\n");
    let result = json_body(
        app.clone()
            .oneshot(
                Request::post("/api/assets")
                    .header("content-type", "multipart/form-data; boundary=test")
                    .body(Body::from(body))
                    .unwrap(),
            )
            .await
            .unwrap(),
    )
    .await;
    let asset = &result["assets"][0];
    assert_eq!(asset["prompt"], "a cedar cabin");
    assert_eq!(asset["origin"], "gpt-image");
    assert_eq!(asset["params"], json!({"quality":"high"}));
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("http://{}/outside.png", listener.local_addr().unwrap());
    let source = tokio::spawn(async move {
        axum::serve(
            listener,
            Router::new().route(
                "/outside.png",
                get(|| async { ([("content-type", "image/png")], png()) }),
            ),
        )
        .await
        .unwrap();
    });
    let result = json_body(app.clone().oneshot(Request::post("/api/assets/import").header("content-type", "application/json")
        .body(Body::from(json!({"url":url,"prompt":"ignored replacement","origin":"ignored","source_url":"https://example.com/original","params":{"seed":42}}).to_string())).unwrap()).await.unwrap()).await;
    source.abort();
    assert_eq!(result["assets"][0]["id"], asset["id"]);
    assert_eq!(result["assets"][0]["prompt"], "a cedar cabin");
    assert_eq!(
        result["assets"][0]["source_url"],
        "https://example.com/original"
    );
    let path = dir.join("source.png");
    tokio::fs::write(&path, png()).await.unwrap();
    let ctx = McpContext {
        engine: e.clone(),
        user_id: LOCAL_USER.into(),
        local_files: true,
    };
    let result = mcp::handle(&ctx, json!({"jsonrpc":"2.0","id":1,"method":"tools/call","params":{"name":"import_asset","arguments":{
        "path":path,"negative_prompt":"different","params":{"seed":9,"aspect_ratio":"16:9"}
    }}})).await.unwrap();
    assert_eq!(result["result"]["isError"], false, "{result}");
    let saved: Value =
        serde_json::from_str(result["result"]["content"][0]["text"].as_str().unwrap()).unwrap();
    assert_eq!(saved["id"], asset["id"]);
    assert_eq!(saved["negative_prompt"], "fog");
    assert_eq!(
        saved["params"],
        json!({"quality":"high","seed":42,"aspect_ratio":"16:9"})
    );
    assert_eq!(saved["model_label"], "Outside image");
    let found = json_body(
        app.oneshot(
            Request::get("/api/assets?q=cedar")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap(),
    )
    .await;
    assert_eq!(found["assets"].as_array().unwrap().len(), 1);
}
