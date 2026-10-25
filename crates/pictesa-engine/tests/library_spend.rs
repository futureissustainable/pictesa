//! Owner isolation, bounded downloads and budget transactions on the free mock provider.
use image::{DynamicImage, ImageFormat};
use pictesa_engine::{spend::BudgetInput, AssetFilter, Config, CreateGeneration, Engine, EngineError, Mode, LOCAL_USER};
use std::{io::Cursor, time::Duration};

async fn engine(mode: Mode) -> Engine {
    std::env::set_var("PICTESA_MOCK", "1");
    std::env::set_var("PICTESA_MOCK_KEY", "fake-test-key");
    std::env::set_var("PICTESA_BILLING", "1");
    let mut cfg = Config::local(std::env::temp_dir().join(format!("pictesa-library-spend-{}", pictesa_engine::new_id())));
    cfg.mode = mode;
    Engine::open(cfg).await.unwrap()
}
fn request(count: u32) -> CreateGeneration {
    CreateGeneration { model: "mock:image".into(), prompt: "budget test".into(), count: Some(count), ..Default::default() }
}
/// A different small PNG on every call: the library keeps one copy of identical uploads.
fn png() -> Vec<u8> {
    static NEXT: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);
    let width = 2 + NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    let mut bytes = Cursor::new(Vec::new());
    DynamicImage::new_rgb8(width, 2).write_to(&mut bytes, ImageFormat::Png).unwrap();
    bytes.into_inner()
}

#[tokio::test]
async fn collections_tags_and_bulk_changes_are_owned_and_atomic() {
    let e = engine(Mode::Hosted).await;
    let a = e.signup("a@example.com", "correct horse battery", None).await.unwrap();
    let b = e.signup("b@example.com", "correct horse battery", None).await.unwrap();
    let asset = e.upload(&a.id, &png(), Some("original.png"), None).await.unwrap();
    let other = e.upload(&b.id, &png(), None, None).await.unwrap();
    let collection = e.create_collection(&a.id, "First").await.unwrap();
    let second = e.create_collection(&a.id, "Second").await.unwrap();
    let ids = vec![asset.id.clone()];
    e.collection_assets(&a.id, &collection.id, &ids, false).await.unwrap();
    e.collection_assets(&a.id, &second.id, &ids, false).await.unwrap();
    e.tag_assets(&a.id, &ids, "Editorial", false).await.unwrap();
    assert!(e.collections(&b.id).await.unwrap().is_empty());
    assert!(e.tags(&b.id).await.unwrap().is_empty());
    assert!(e.asset_tags(&b.id, &asset.id).await.is_err());
    assert!(e.rename_collection(&b.id, &collection.id, "stolen").await.is_err());
    assert!(e.delete_collection(&b.id, &collection.id).await.is_err());
    assert!(e.collection_assets(&b.id, &collection.id, &[other.id.clone()], false).await.is_err());
    assert!(e.collection_assets(&b.id, &collection.id, &ids, true).await.is_err());
    let mixed = vec![asset.id.clone(), other.id.clone()];
    assert!(e.tag_assets(&a.id, &mixed, "leak", false).await.is_err());
    assert!(e.bulk_assets(&a.id, &mixed, "favorite").await.is_err());
    assert!(!e.asset(&a.id, &asset.id).await.unwrap().0.favorite);
    assert_eq!(e.tags(&a.id).await.unwrap(), vec!["Editorial"]);
    let filter = AssetFilter { collection: Some(collection.id.clone()), tag: Some("editorial".into()), ..Default::default() };
    assert_eq!(e.assets(&a.id, &filter).await.unwrap().0.len(), 1);
    assert!(e.assets(&b.id, &filter).await.unwrap().0.is_empty());
    e.rename_collection(&a.id, &collection.id, "Renamed").await.unwrap();
    e.collection_assets(&a.id, &collection.id, &ids, true).await.unwrap();
    assert!(e.assets(&a.id, &filter).await.unwrap().0.is_empty());
    e.delete_collection(&a.id, &collection.id).await.unwrap();
    assert_eq!(e.collections(&a.id).await.unwrap()[0].asset_count, 1);
    e.tag_assets(&a.id, &ids, "EDITORIAL", true).await.unwrap();
    assert!(e.tags(&a.id).await.unwrap().is_empty());
    assert!(e.asset(&a.id, &asset.id).await.is_ok());
}

#[tokio::test]
async fn zip_validates_owners_count_size_and_trash_before_streaming() {
    let e = engine(Mode::Hosted).await;
    let a = e.signup("zip-a@example.com", "correct horse battery", None).await.unwrap();
    let b = e.signup("zip-b@example.com", "correct horse battery", None).await.unwrap();
    let asset = e.upload(&a.id, &png(), Some("../../Original photo.png"), None).await.unwrap();
    let ids = vec![asset.id.clone()];
    assert!(e.zip_entries(&b.id, &ids).await.is_err());
    assert!(e.zip_entries(&a.id, &[]).await.is_err());
    assert!(e.zip_entries(&a.id, &vec![asset.id.clone(); 101]).await.is_err());
    let files = e.zip_entries(&a.id, &ids).await.unwrap();
    assert_eq!(files.len(), 1);
    assert!(files[0].name.starts_with("Original photo"));
    assert!(!files[0].name.contains('/'));
    e.trash(&a.id, &asset.id, false).await.unwrap();
    assert!(e.zip_entries(&a.id, &ids).await.is_err());
    e.restore(&a.id, &asset.id).await.unwrap();
    let second = e.upload(&a.id, &png(), Some("second.png"), None).await.unwrap();
    assert!(e.zip_entries(&a.id, &vec![asset.id.clone(); 100]).await.is_ok(), "exact count limit is allowed");
    let path = e.asset_file(Some(&a.id), &asset.id).await.unwrap().0;
    let file = tokio::fs::OpenOptions::new().write(true).open(path).await.unwrap();
    file.set_len(pictesa_engine::library::MAX_ZIP_BYTES).await.unwrap();
    assert!(e.zip_entries(&a.id, &ids).await.is_ok(), "exact size limit is allowed");
    file.set_len(pictesa_engine::library::MAX_ZIP_BYTES + 1).await.unwrap();
    assert!(matches!(e.zip_entries(&a.id, &ids).await, Err(EngineError::Validation { .. })));
    file.set_len(pictesa_engine::library::MAX_ZIP_BYTES / 2 + 1).await.unwrap();
    assert!(e.zip_entries(&a.id, &[asset.id.clone()]).await.is_ok());
    let second_path = e.asset_file(Some(&a.id), &second.id).await.unwrap().0;
    let second_file = tokio::fs::OpenOptions::new().write(true).open(second_path).await.unwrap();
    second_file.set_len(pictesa_engine::library::MAX_ZIP_BYTES / 2 + 1).await.unwrap();
    assert!(e.zip_entries(&a.id, &[asset.id, second.id]).await.is_err(), "the cap applies to the archive total");
}

#[tokio::test]
async fn budgets_allow_exact_boundary_serialize_parallel_runs_and_keep_deleted_spend() {
    let e = engine(Mode::Hosted).await;
    let user = e.signup("budget@example.com", "correct horse battery", None).await.unwrap();
    e.grant("budget@example.com", 1.0, "test").await.unwrap();
    e.set_budgets(&user.id, BudgetInput { daily_usd: Some(0.025), monthly_usd: Some(0.025) }).await.unwrap();
    let (a, b) = tokio::join!(e.create_generation(&user.id, request(2)), e.create_generation(&user.id, request(2)));
    let (accepted, rejected) = match (a, b) {
        (Ok(accepted), Err(rejected)) | (Err(rejected), Ok(accepted)) => (accepted, rejected),
        other => panic!("exactly one batch must fit: {other:?}"),
    };
    assert!(matches!(rejected, EngineError::Validation { field, .. } if field.as_deref() == Some("budget")));
    let denied = e.create_generation(&user.id, request(1)).await.unwrap_err();
    assert!(matches!(denied, EngineError::Validation { field, .. } if field.as_deref() == Some("budget")));
    let view = e.spend_view(&user.id).await.unwrap();
    assert_eq!(view.today_usd, 0.025);
    assert_eq!(view.daily.remaining_usd, Some(0.0));
    // A run already submitted upstream keeps its cost when history is deleted.
    sqlx::query("UPDATE jobs SET status = 'succeeded' WHERE id = ?").bind(&accepted.jobs[0].id).execute(e.db()).await.unwrap();
    e.delete_job(&user.id, &accepted.jobs[0].id).await.unwrap();
    assert_eq!(e.spend_view(&user.id).await.unwrap().today_usd, 0.025);
    e.set_budgets(&user.id, BudgetInput { daily_usd: None, monthly_usd: Some(0.025) }).await.unwrap();
    assert!(e.create_generation(&user.id, request(1)).await.unwrap_err().to_string().contains("monthly"));
}

#[tokio::test]
async fn refunds_release_caps_and_hosted_spend_is_isolated() {
    let e = engine(Mode::Hosted).await;
    let a = e.signup("refund@example.com", "correct horse battery", None).await.unwrap();
    let b = e.signup("isolated@example.com", "correct horse battery", None).await.unwrap();
    e.grant("refund@example.com", 1.0, "test").await.unwrap();
    e.set_budgets(&a.id, BudgetInput { daily_usd: Some(0.0125), monthly_usd: None }).await.unwrap();
    let run = e.create_generation(&a.id, request(1)).await.unwrap();
    assert_eq!(e.spend_view(&a.id).await.unwrap().today_usd, 0.0125);
    e.cancel(&a.id, &run.jobs[0].id).await.unwrap();
    assert_eq!(e.spend_view(&a.id).await.unwrap().today_usd, 0.0);
    assert_eq!(e.billing_view(&a.id).await.unwrap().balance_usd, 1.0);
    assert!(e.create_generation(&a.id, request(1)).await.is_ok());
    let other = e.spend_view(&b.id).await.unwrap();
    assert_eq!(other.today_usd, 0.0);
    assert!(other.models.is_empty());
    assert_eq!(other.daily.cap_usd, None);
}

#[tokio::test]
async fn local_runs_remain_zero_with_zero_caps_and_paid_filter_queries_use_cost() {
    let e = engine(Mode::Local).await;
    e.set_budgets(LOCAL_USER, BudgetInput { daily_usd: Some(0.0), monthly_usd: Some(0.0) }).await.unwrap();
    let created = e.create_generation(LOCAL_USER, request(1)).await.unwrap();
    assert_eq!(created.jobs[0].cost_usd, Some(0.0));
    e.start_scoped();
    let job = e.wait_job(LOCAL_USER, &created.jobs[0].id, Duration::from_secs(30)).await.unwrap();
    assert_eq!(job.status, pictesa_engine::JobStatus::Succeeded);
    assert_eq!(e.spend_view(LOCAL_USER).await.unwrap().today_usd, 0.0);
    // Cost comes from the recorded run, independently of whether its job history exists.
    sqlx::query("UPDATE spend_runs SET actual_micros = 500000, provider = 'example' WHERE job_id = ?").bind(&job.id).execute(e.db()).await.unwrap();
    let filter =
        AssetFilter { model: Some("mock:image".into()), provider: Some("example".into()), cost_min: Some(0.5), cost_max: Some(0.5), ..Default::default() };
    assert_eq!(e.assets(LOCAL_USER, &filter).await.unwrap().0.len(), 1);
    let none = AssetFilter { cost_min: Some(0.500001), cost_max: None, ..filter.clone() };
    assert!(e.assets(LOCAL_USER, &none).await.unwrap().0.is_empty());
    assert!(e.assets(LOCAL_USER, &AssetFilter { cost_min: Some(-1.0), ..Default::default() }).await.is_err());
    assert!(e.assets(LOCAL_USER, &AssetFilter { cost_min: Some(1.0), cost_max: Some(0.0), ..Default::default() }).await.is_err());
}

#[tokio::test]
async fn spend_uses_calendar_windows_instead_of_a_rolling_month() {
    let e = engine(Mode::Local).await;
    let (today, month): (i64, i64) =
        sqlx::query_as("SELECT CAST(strftime('%s','now','start of day') AS INTEGER)*1000, CAST(strftime('%s','now','start of month') AS INTEGER)*1000")
            .fetch_one(e.db())
            .await
            .unwrap();
    let day = 86_400_000;
    for (i, at) in [today, today - 6 * day, today - 7 * day, today - 29 * day, today - 30 * day, month - 1].into_iter().enumerate() {
        sqlx::query("INSERT INTO spend_runs (job_id,user_id,model,provider,estimated_micros,created_at) VALUES (?, 'local', 'test:model', 'test', 1000000, ?)")
            .bind(i.to_string())
            .bind(at)
            .execute(e.db())
            .await
            .unwrap();
    }
    let view = e.spend_view(LOCAL_USER).await.unwrap();
    let timestamps = [today, today - 6 * day, today - 7 * day, today - 29 * day, today - 30 * day, month - 1];
    assert_eq!(view.today_usd, 1.0);
    assert_eq!(view.seven_days_usd, timestamps.iter().filter(|at| **at >= today - 6 * day).count() as f64);
    assert_eq!(view.thirty_days_usd, timestamps.iter().filter(|at| **at >= today - 29 * day).count() as f64);
    assert_eq!(view.monthly.spent_usd, timestamps.iter().filter(|at| **at >= month).count() as f64);
    assert_eq!(view.days.len(), 30);
    assert_eq!(view.days.iter().map(|day| day.usd).sum::<f64>(), view.thirty_days_usd);
}

#[tokio::test]
async fn upgrading_existing_history_preserves_charges_refunds_and_local_zero() {
    use sqlx::{
        migrate::Migrator,
        sqlite::{SqliteConnectOptions, SqlitePoolOptions},
    };
    use std::borrow::Cow;
    let dir = std::env::temp_dir().join(format!("pictesa-upgrade-{}", pictesa_engine::new_id()));
    std::fs::create_dir_all(&dir).unwrap();
    let db = SqlitePoolOptions::new().connect_with(SqliteConnectOptions::new().filename(dir.join("pictesa.db")).create_if_missing(true)).await.unwrap();
    let all = sqlx::migrate!("../../migrations");
    let old = Migrator { migrations: Cow::Owned(all.iter().filter(|m| m.version < 9).cloned().collect()), ..Migrator::DEFAULT };
    old.run(&db).await.unwrap();
    for (id, provider, cost, charged) in [("paid", "fal", 0.5, 500_000i64), ("refunded", "fal", 0.25, 250_000), ("local", "comfyui", 10.0, 0)] {
        sqlx::query("INSERT INTO jobs (id,user_id,batch_id,model,provider,model_name,output,status,cost_usd,charged_micros,created_at,updated_at) VALUES (?, 'local', 'old', 'old:model', ?, 'Old model', 'image', 'succeeded', ?, ?, ?, ?)")
            .bind(id).bind(provider).bind(cost).bind(charged).bind(pictesa_core::now_ms()).bind(pictesa_core::now_ms()).execute(&db).await.unwrap();
    }
    sqlx::query("INSERT INTO ledger (id,user_id,amount_micros,reason,job_id,created_at) VALUES ('refund:refunded','local',250000,'Refund','refunded',0)")
        .execute(&db)
        .await
        .unwrap();
    db.close().await;
    let e = Engine::open(Config::local(dir)).await.unwrap();
    let view = e.spend_view(LOCAL_USER).await.unwrap();
    assert_eq!(view.today_usd, 0.5);
    assert_eq!(view.providers.iter().find(|group| group.name == "comfyui").unwrap().usd, 0.0);
    assert!(e.collections(LOCAL_USER).await.unwrap().is_empty());
}

#[tokio::test]
async fn an_unpriced_paid_model_cannot_bypass_active_caps() {
    use pictesa_core::{MediaKind, ModelSpec, Pricing, Task};
    let e = engine(Mode::Hosted).await;
    let user = e.signup("unpriced@example.com", "correct horse battery", None).await.unwrap();
    let spec = ModelSpec::new("openai_compatible", "unpriced", "Unpriced model", MediaKind::Image, &[Task::TextToImage])
        .pricing(Pricing::Variable { note: "Token billed".into() });
    sqlx::query("INSERT INTO model_cache (provider,scope,data,fetched_at) VALUES ('openai_compatible', ?, ?, ?)")
        .bind(&spec.id)
        .bind(serde_json::to_string(&spec).unwrap())
        .bind(pictesa_core::now_ms())
        .execute(e.db())
        .await
        .unwrap();
    // Fake credentials only. No dispatcher is started and no provider call is made.
    for (field, value) in [("base_url", "https://example.invalid/v1"), ("api_key", "fake-test-key")] {
        sqlx::query("INSERT INTO credentials (user_id,provider,field,value,encrypted,updated_at) VALUES (?, 'openai_compatible', ?, ?, 0, 0)")
            .bind(&user.id)
            .bind(field)
            .bind(value)
            .execute(e.db())
            .await
            .unwrap();
    }
    let e = Engine::open(e.config().clone()).await.unwrap();
    e.set_budgets(&user.id, BudgetInput { daily_usd: Some(0.0), monthly_usd: None }).await.unwrap();
    let req = CreateGeneration { model: spec.id, prompt: "Unpriced".into(), ..Default::default() };
    assert!(e.create_generation(&user.id, req.clone()).await.unwrap_err().to_string().contains("no cost estimate"));
    assert!(e.jobs(&user.id, &Default::default()).await.unwrap().0.is_empty());
    e.set_budgets(&user.id, BudgetInput::default()).await.unwrap();
    assert!(e.create_generation(&user.id, req).await.is_ok());
    assert_eq!(e.spend_view(&user.id).await.unwrap().unknown_runs, 1);
}
