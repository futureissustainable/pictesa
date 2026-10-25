//! An invalid dry-run argument must never turn an estimate into a queued training run.
use pictesa_engine::{Config, CreateGeneration, Engine, JobFilter, LOCAL_USER};
use pictesa_server::mcp::{self, McpContext};
use serde_json::json;
use std::time::Duration;

#[tokio::test]
async fn mcp_rejects_non_boolean_training_dry_run_without_queuing() {
    std::env::set_var("PICTESA_MOCK", "1");
    std::env::set_var("PICTESA_DENY_PAID", "1");
    let root =
        std::env::temp_dir().join(format!("pictesa-mcp-training-{}", pictesa_engine::new_id()));
    let engine = Engine::open(Config::local(root)).await.unwrap();
    engine.start_scoped();
    let created = engine
        .create_generation(
            LOCAL_USER,
            CreateGeneration {
                model: "mock:image".into(),
                prompt: "dataset".into(),
                count: Some(3),
                ..Default::default()
            },
        )
        .await
        .unwrap();
    let job = engine
        .wait_job(LOCAL_USER, &created.jobs[0].id, Duration::from_secs(20))
        .await
        .unwrap();
    assert_eq!(job.outputs.len(), 3);
    engine
        .set_credentials(
            LOCAL_USER,
            "ai-toolkit",
            json!({"path":"not-executed-fixture-wrapper"})
                .as_object()
                .unwrap(),
        )
        .await
        .unwrap();
    let context = McpContext {
        engine: engine.clone(),
        user_id: LOCAL_USER.into(),
        local_files: true,
    };
    let before = engine
        .jobs(LOCAL_USER, &JobFilter::default())
        .await
        .unwrap()
        .0
        .len();
    let result=mcp::handle(&context,json!({"jsonrpc":"2.0","id":1,"method":"tools/call","params":{"name":"train_lora","arguments":{
        "trainer":"ai-toolkit:flux1","asset_ids":job.outputs.iter().map(|a|&a.id).collect::<Vec<_>>(),"trigger_word":"maya","family":"flux1","dry_run":"true"
    }}})).await.unwrap();
    assert_eq!(result["result"]["isError"], true);
    assert!(result["result"]["content"][0]["text"]
        .as_str()
        .unwrap()
        .contains("dry_run must be a boolean"));
    let after = engine
        .jobs(LOCAL_USER, &JobFilter::default())
        .await
        .unwrap()
        .0
        .len();
    assert_eq!(after, before, "malformed estimates must queue nothing");
}
