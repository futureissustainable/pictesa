//! MCP request parsing must preserve batch fields and refuse malformed arguments without panic.
use pictesa_engine::{Config, Engine, JobFilter, LOCAL_USER};
use pictesa_server::mcp::{handle, McpContext};
use serde_json::{json, Value};

async fn call(ctx: &McpContext, name: &str, args: Value) -> Value {
    handle(ctx, json!({"jsonrpc":"2.0","id":1,"method":"tools/call","params":{"name":name,"arguments":args}})).await.unwrap()
}
fn payload(response: &Value) -> Value {
    assert_ne!(response["result"]["isError"], true, "{response}");
    serde_json::from_str(response["result"]["content"][0]["text"].as_str().unwrap()).unwrap()
}

#[tokio::test]
async fn mcp_batch_dry_run_and_commit_share_expansion_and_handle_invalid_arguments() {
    std::env::set_var("PICTESA_MOCK", "1");
    let e = Engine::open(Config::local(std::env::temp_dir().join(format!("pictesa-mcp-batch-{}", pictesa_engine::new_id())))).await.unwrap();
    let ctx = McpContext { engine: e.clone(), user_id: LOCAL_USER.into(), local_files: true };
    for args in [json!(null), json!([]), json!(42)] {
        let response = call(&ctx, "generate_batch", args).await;
        assert_eq!(response["result"]["isError"], true, "{response}");
    }
    let mut args = json!({"model":"mock:image","prompts":["first","second"],"seeds":{"start":0,"count":2},"dry_run":true});
    assert_eq!(payload(&call(&ctx, "generate_batch", args.clone()).await)["run_count"], 4);
    assert!(e.jobs(LOCAL_USER, &JobFilter::default()).await.unwrap().0.is_empty());
    args["dry_run"] = json!(false);
    let created = payload(&call(&ctx, "generate_batch", args).await);
    assert_eq!(created["jobs"].as_array().unwrap().len(), 4);
    assert!(created["jobs"].as_array().unwrap().iter().all(|j| j["batch_id"] == created["batch_id"]));
    let saved = payload(&call(&ctx, "save_prompt", json!({"name":"Template","prompt":"a {subject}"})).await);
    assert_eq!(saved["placeholders"], json!(["subject"]));
    assert_eq!(payload(&call(&ctx, "list_saved_prompts", json!({"q":"Template"})).await)["prompts"][0]["id"], saved["id"]);
    assert_eq!(payload(&call(&ctx, "list_prompt_history", json!({})).await)["history"].as_array().unwrap().len(), 2);
}
