//! Personal prompt isolation and atomic, priced batches on the mock provider only.
use std::collections::HashMap;
use pictesa_engine::{batches::{BatchRequest, GridAxis, SeedSweep}, prompts::{PromptFilter, SavePrompt}, spend::BudgetInput, Config, CreateGeneration, Engine, EngineError, JobFilter, Mode, LOCAL_USER};
use serde_json::json;

async fn engine(mode: Mode) -> Engine {
    std::env::set_var("PICTESA_MOCK", "1");
    std::env::set_var("PICTESA_MOCK_KEY", "fake-test-key");
    std::env::set_var("PICTESA_BILLING", "1");
    let mut cfg = Config::local(std::env::temp_dir().join(format!("pictesa-prompts-batches-{}", pictesa_engine::new_id())));
    cfg.mode = mode;
    Engine::open(cfg).await.unwrap()
}
fn request(prompt: &str) -> CreateGeneration { CreateGeneration { model: "mock:image".into(), prompt: prompt.into(), ..Default::default() } }
fn batch() -> BatchRequest { BatchRequest { generation: request("batch"), ..Default::default() } }
fn field(error: EngineError, expected: &str) { assert!(matches!(error, EngineError::Validation { field, .. } if field.as_deref() == Some(expected))); }

#[tokio::test]
async fn history_is_distinct_searchable_latest_model_and_private() {
    let e = engine(Mode::Local).await;
    let first = e.create_generation(LOCAL_USER, request("a birch cabin")).await.unwrap();
    let oak = e.create_generation(LOCAL_USER, request("an oak cabin")).await.unwrap();
    let mut latest = request("a birch cabin"); latest.model = "mock:video".into();
    let last = e.create_generation(LOCAL_USER, latest).await.unwrap();
    // Millisecond ties use the job id as the stable newest-first tie-breaker.
    sqlx::query("UPDATE jobs SET created_at = 100 WHERE id = ?").bind(&first.jobs[0].id).execute(e.db()).await.unwrap();
    sqlx::query("UPDATE jobs SET created_at = 300 WHERE id = ?").bind(&last.jobs[0].id).execute(e.db()).await.unwrap();
    sqlx::query("UPDATE jobs SET created_at = 200 WHERE id = ?").bind(&oak.jobs[0].id).execute(e.db()).await.unwrap();
    let h = e.prompt_history(LOCAL_USER, &PromptFilter { q: Some("BIRCH".into()), ..Default::default() }).await.unwrap();
    assert_eq!(h.len(), 1); assert_eq!(h[0].model, "mock:video"); assert_eq!(h[0].last_used_at, 300);
    assert!(e.prompt_history("other-user", &PromptFilter::default()).await.unwrap().is_empty());
    let all = e.prompt_history(LOCAL_USER, &PromptFilter::default()).await.unwrap();
    assert_eq!(all.len(), 2); assert_eq!(all[0].prompt, "a birch cabin");
}

#[tokio::test]
async fn saved_templates_keep_settings_and_enforce_owner_on_every_operation() {
    let e = engine(Mode::Local).await;
    let saved = e.save_prompt(LOCAL_USER, SavePrompt { name: "Cabin".into(), prompt: "{subject} at {place}".into(), negative_prompt: Some("no {avoid}".into()), model: Some("mock:image".into()), params: Some(serde_json::from_value(json!({"aspect_ratio":"16:9"})).unwrap()) }).await.unwrap();
    assert_eq!(saved.placeholders, ["subject", "place", "avoid"]);
    assert!(e.saved_prompts("other-user", &PromptFilter::default()).await.unwrap().is_empty());
    assert!(e.saved_prompt("other-user", &saved.id).await.is_err());
    assert!(e.use_prompt("other-user", &saved.id, HashMap::new()).await.is_err());
    assert!(e.rename_prompt("other-user", &saved.id, "stolen").await.is_err());
    assert!(e.delete_prompt("other-user", &saved.id).await.is_err());
    let used = e.use_prompt(LOCAL_USER, &saved.id, HashMap::from([("subject".into(), "a cabin".into()), ("place".into(), "dawn".into()), ("avoid".into(), "fog".into())])).await.unwrap();
    assert_eq!(used.prompt, "a cabin at dawn"); assert_eq!(used.negative_prompt.as_deref(), Some("no fog")); assert_eq!(used.params.unwrap()["aspect_ratio"], "16:9");
    e.rename_prompt(LOCAL_USER, &saved.id, "Timber").await.unwrap();
    assert_eq!(e.saved_prompts(LOCAL_USER, &PromptFilter { q: Some("TIMBER".into()), ..Default::default() }).await.unwrap().len(), 1);
    e.delete_prompt(LOCAL_USER, &saved.id).await.unwrap();
    assert!(e.saved_prompt(LOCAL_USER, &saved.id).await.is_err());
    assert!(e.save_prompt(LOCAL_USER, SavePrompt { name: " ".into(), prompt: "x".into(), ..Default::default() }).await.is_err());
}

#[tokio::test]
async fn expansion_order_seed_zero_and_dry_run_match_committed_batch() {
    let e = engine(Mode::Local).await;
    let mut b = batch(); b.prompts = vec!["first".into(), "second".into()]; b.seeds = Some(SeedSweep { start: 0, count: 2 });
    b.grid = vec![GridAxis { key: "aspect_ratio".into(), values: vec![json!("1:1"), json!("16:9")] }, GridAxis { key: "speed".into(), values: vec![json!("standard"), json!("batch")] }];
    let estimate = e.estimate_batch(LOCAL_USER, b.clone()).await.unwrap();
    assert_eq!(estimate.run_count, 16); assert_eq!(estimate.total_estimated_cost_usd, Some(0.0));
    assert!(e.jobs(LOCAL_USER, &JobFilter::default()).await.unwrap().0.is_empty());
    let created = e.create_batch(LOCAL_USER, b).await.unwrap();
    let actual: Vec<_> = created.jobs.iter().map(|j| (j.prompt.as_str(), j.params["seed"].as_u64().unwrap(), j.params["aspect_ratio"].as_str().unwrap(), j.params["speed"].as_str().unwrap())).collect();
    assert_eq!(&actual[..5], &[("first",0,"1:1","standard"), ("first",0,"1:1","batch"), ("first",0,"16:9","standard"), ("first",0,"16:9","batch"), ("first",1,"1:1","standard")]);
    assert_eq!(actual[8], ("second",0,"1:1","standard"));
    assert!(created.jobs.iter().all(|j| j.batch_id == created.batch_id && j.count == 1));
}

#[tokio::test]
async fn batch_limits_and_invalid_values_refuse_every_run() {
    let e = engine(Mode::Local).await;
    let mut b = batch(); b.seeds = Some(SeedSweep { start: 1, count: 64 });
    assert_eq!(e.estimate_batch(LOCAL_USER, b.clone()).await.unwrap().run_count, 64);
    b.prompts = vec!["a".into(), "b".into()]; field(e.create_batch(LOCAL_USER, b.clone()).await.unwrap_err(), "batch");
    b.prompts.clear(); b.seeds = Some(SeedSweep { start: u64::MAX, count: 2 }); field(e.estimate_batch(LOCAL_USER, b.clone()).await.unwrap_err(), "seeds");
    b.seeds = Some(SeedSweep { start: 0, count: 0 }); field(e.estimate_batch(LOCAL_USER, b.clone()).await.unwrap_err(), "seeds");
    b.seeds = None; b.grid = vec![GridAxis { key: "aspect_ratio".into(), values: vec![json!("1:1"), json!("invalid")] }];
    field(e.create_batch(LOCAL_USER, b.clone()).await.unwrap_err(), "params.aspect_ratio");
    b.grid = vec![GridAxis { key: "seed".into(), values: vec![json!(1)] }; 4]; field(e.create_batch(LOCAL_USER, b.clone()).await.unwrap_err(), "batch");
    b.grid.truncate(2); field(e.create_batch(LOCAL_USER, b.clone()).await.unwrap_err(), "grid");
    b.grid.clear(); b.generation.model = "mock:remove-background".into(); b.seeds = Some(SeedSweep { start: 1, count: 2 });
    field(e.estimate_batch(LOCAL_USER, b).await.unwrap_err(), "seeds");
    assert!(e.jobs(LOCAL_USER, &JobFilter::default()).await.unwrap().0.is_empty());
}

#[tokio::test]
async fn over_budget_and_wallet_batches_leave_no_jobs_or_charges() {
    let e = engine(Mode::Hosted).await;
    let user = e.signup("batch@example.com", "correct horse battery", None).await.unwrap();
    e.grant("batch@example.com", 1.0, "test").await.unwrap();
    let mut b = batch(); b.seeds = Some(SeedSweep { start: 10, count: 4 });
    e.set_budgets(&user.id, BudgetInput { daily_usd: Some(0.025), monthly_usd: None }).await.unwrap();
    field(e.estimate_batch(&user.id, b.clone()).await.unwrap_err(), "budget");
    field(e.create_batch(&user.id, b.clone()).await.unwrap_err(), "budget");
    assert!(e.jobs(&user.id, &JobFilter::default()).await.unwrap().0.is_empty());
    let balance: i64 = sqlx::query_scalar("SELECT balance_micros FROM wallets WHERE user_id = ?").bind(&user.id).fetch_one(e.db()).await.unwrap(); assert_eq!(balance, 1_000_000);
    e.set_budgets(&user.id, BudgetInput { daily_usd: Some(0.05), monthly_usd: None }).await.unwrap();
    let estimate = e.estimate_batch(&user.id, b.clone()).await.unwrap(); assert_eq!(estimate.total_estimated_cost_usd, Some(0.05));
    assert_eq!(e.spend_view(&user.id).await.unwrap().today_usd, 0.0);
    e.create_batch(&user.id, b.clone()).await.unwrap(); assert_eq!(e.spend_view(&user.id).await.unwrap().today_usd, 0.05);
    e.set_budgets(&user.id, BudgetInput::default()).await.unwrap();
    sqlx::query("UPDATE wallets SET balance_micros = 0 WHERE user_id = ?").bind(&user.id).execute(e.db()).await.unwrap();
    field(e.create_batch(&user.id, b).await.unwrap_err(), "billing");
    assert_eq!(e.jobs(&user.id, &JobFilter::default()).await.unwrap().0.len(), 4);
}

#[tokio::test]
async fn variations_preserve_settings_and_generate_four_fresh_seeds() {
    let e = engine(Mode::Local).await;
    let mut req = request("a cabin"); req.params.insert("seed".into(), json!(42)); req.params.insert("aspect_ratio".into(), json!("16:9")); req.negative_prompt = Some("fog".into());
    let original = e.create_generation(LOCAL_USER, req).await.unwrap().jobs.remove(0);
    assert!(e.variations(LOCAL_USER, &original.id).await.is_err());
    sqlx::query("UPDATE jobs SET status = 'succeeded' WHERE id = ?").bind(&original.id).execute(e.db()).await.unwrap();
    assert!(e.variations("other-user", &original.id).await.is_err());
    let variants = e.variations(LOCAL_USER, &original.id).await.unwrap();
    assert_eq!(variants.jobs.len(), 4);
    let seeds: std::collections::HashSet<_> = variants.jobs.iter().map(|j| j.params["seed"].as_u64().unwrap()).collect(); assert_eq!(seeds.len(), 4); assert!(!seeds.contains(&42));
    assert!(variants.jobs.iter().all(|j| j.model == original.model && j.prompt == original.prompt && j.negative_prompt == original.negative_prompt && j.params["aspect_ratio"] == "16:9"));
}

#[tokio::test]
async fn variations_without_seed_use_four_normal_outputs() {
    let e = engine(Mode::Local).await;
    let mut original = request("slow camera move"); original.model = "mock:video".into(); original.params.insert("duration".into(), json!(4));
    let job = e.create_generation(LOCAL_USER, original).await.unwrap().jobs.remove(0);
    sqlx::query("UPDATE jobs SET status = 'succeeded' WHERE id = ?").bind(&job.id).execute(e.db()).await.unwrap();
    let variations = e.variations(LOCAL_USER, &job.id).await.unwrap();
    assert_eq!(variations.jobs.iter().map(|j| j.count).sum::<u32>(), 4);
    assert!(variations.jobs.iter().all(|j| j.model == "mock:video" && j.prompt == "slow camera move" && j.params["duration"] == 4 && !j.params.contains_key("seed")));
}
