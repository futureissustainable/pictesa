//! Skill packs over the MCP HTTP endpoint: prompts/list, prompts/get with arguments, and refusals.
use axum::{
    body::{to_bytes, Body},
    http::Request,
    Router,
};
use base64::Engine as _;
use pictesa_engine::{characters::CharacterInput, Config, Engine, LOCAL_USER};
use pictesa_server::{router, ServerOptions};
use serde_json::{json, Value};
use tower::ServiceExt;

fn png() -> Vec<u8> {
    base64::engine::general_purpose::STANDARD.decode("iVBORw0KGgoAAAANSUhEUgAAAAQAAAAECAIAAAAmkwkpAAAAEElEQVR4nGP4lGkLRwzEcQCiohmBzIRWUgAAAABJRU5ErkJggg==").unwrap()
}

async fn rpc(app: &Router, method: &str, params: Value) -> Value {
    let body = json!({ "jsonrpc": "2.0", "id": 7, "method": method, "params": params });
    let request = Request::post("/mcp").header("content-type", "application/json").body(Body::from(body.to_string())).unwrap();
    let response = app.clone().oneshot(request).await.unwrap();
    serde_json::from_slice(&to_bytes(response.into_body(), 1 << 20).await.unwrap()).unwrap()
}

fn text(v: &Value) -> &str {
    v["result"]["messages"][0]["content"]["text"].as_str().unwrap_or_else(|| panic!("no prompt text in {v}"))
}

#[tokio::test]
async fn skill_packs_are_mcp_prompts_with_checked_arguments() {
    let dir = std::env::temp_dir().join(format!("pictesa-prompts-{}", pictesa_engine::new_id()));
    let e = Engine::open(Config::local(&dir)).await.unwrap();
    let logo = e.upload(LOCAL_USER, &png(), Some("logo.png"), None).await.unwrap();
    let kit = CharacterInput { name: Some("Oak and Co".into()), asset_ids: Some(vec![logo.id.clone()]), kind: Some("brand".into()), colors: Some(vec!["#0B6E4F".into()]), words: Some("calm, natural".into()), ..Default::default() };
    let kit = e.create_character(LOCAL_USER, kit).await.unwrap();
    e.create_character(LOCAL_USER, CharacterInput { name: Some("Maya".into()), asset_ids: Some(vec![logo.id.clone()]), ..Default::default() }).await.unwrap();
    let app = router(e.clone(), ServerOptions::default());

    let init = rpc(&app, "initialize", json!({ "protocolVersion": "2025-06-18" })).await;
    assert!(init["result"]["capabilities"]["prompts"].is_object(), "{init}");

    let list = rpc(&app, "prompts/list", json!({})).await;
    let prompts = list["result"]["prompts"].as_array().unwrap();
    let names: Vec<&str> = prompts.iter().map(|p| p["name"].as_str().unwrap()).collect();
    assert_eq!(names, ["ugc_ad", "hook_variants", "explainer", "faceless_short", "brand_kit"]);
    let explainer = prompts.iter().find(|p| p["name"] == "explainer").unwrap();
    assert!(explainer["arguments"].as_array().unwrap().iter().any(|a| a["name"] == "topic" && a["required"] == true));

    // Arguments fill the steps: files become run inputs, characters and brand kits resolve by name.
    let ugc = rpc(&app, "prompts/get", json!({ "name": "ugc_ad", "arguments": {
        "product": "A ceramic pour-over coffee set", "product_image": "https://example.com/set.png",
        "brand": "oak and co", "presenter": "Maya", "length": "10"
    } })).await;
    assert_eq!(ugc["result"]["messages"][0]["role"], "user");
    let steps = text(&ugc);
    assert!(steps.contains("A ceramic pour-over coffee set"));
    assert!(steps.contains(r#""product": [{"url":"https://example.com/set.png"}]"#), "{steps}");
    assert!(steps.contains(&format!(r#""presenter": [{{"asset_id":"{}"}}]"#, logo.id)), "{steps}");
    assert!(steps.contains(&format!("character \"{}\"", kit.id)), "{steps}");
    assert!(steps.contains(r#""duration": "10""#), "{steps}");
    assert!(steps.contains("\n1. Call list_recipes"), "steps are numbered: {steps}");

    let brand = rpc(&app, "prompts/get", json!({ "name": "brand_kit", "arguments": { "brand": kit.id, "brief": "three product shots" } })).await;
    assert!(text(&brand).contains("Brand look: calm, natural. Brand color: #0b6e4f."), "{brand}");

    // Refusals are invalid params (-32602) that name the problem.
    for (params, needle) in [
        (json!({ "name": "explainer" }), "topic"),
        (json!({ "name": "explainer", "arguments": { "topic": "tea", "scenes": "many" } }), "whole number"),
        (json!({ "name": "explainer", "arguments": { "topic": "tea", "scenes": "40" } }), "from 3 to 12"),
        (json!({ "name": "explainer", "arguments": { "topic": "tea", "shape": "4:3" } }), "shape"),
        (json!({ "name": "explainer", "arguments": { "topic": "tea", "tone": "fun" } }), "no argument tone"),
        (json!({ "name": "brand_kit", "arguments": { "brand": "Maya", "brief": "x" } }), "not a brand kit"),
        (json!({ "name": "brand_kit", "arguments": { "brand": "Nobody", "brief": "x" } }), "list_characters"),
        (json!({ "name": "explainer", "arguments": ["tea"] }), "object"),
        (json!({ "name": "nope" }), "prompts/list"),
    ] {
        let answer = rpc(&app, "prompts/get", params.clone()).await;
        assert_eq!(answer["error"]["code"], -32602, "{params} -> {answer}");
        let message = answer["error"]["message"].as_str().unwrap_or_default();
        assert!(message.contains(needle), "{params} -> {message}");
    }
}
