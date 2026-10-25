//! Never starts a dispatcher. Recorded specs and fake keys exercise reservations without network.
use pictesa_core::{
    lora::TrainingOptions, InputRole, InputSlot, MediaKind, ModelSpec, Pricing, Task,
};
use pictesa_engine::{spend::BudgetInput, training::TrainRequest, Config, Engine, Mode};
use serde_json::json;
use std::io::Cursor;

#[tokio::test]
async fn cloud_training_obeys_budget_wallet_and_estimate_without_submission() {
    std::env::set_var("PICTESA_DENY_PAID", "0");
    std::env::set_var("PICTESA_BILLING", "1");
    std::env::set_var("FAL_KEY", "fixture-not-a-real-key");
    let root = std::env::temp_dir().join(format!(
        "pictesa-training-billing-{}",
        pictesa_engine::new_id()
    ));
    let mut cfg = Config::local(&root);
    cfg.mode = Mode::Hosted;
    let initial = Engine::open(cfg.clone()).await.unwrap();
    let user = initial
        .signup("training@example.com", "long fixture password", None)
        .await
        .unwrap();
    let mut ids = Vec::new();
    for i in 0..3 {
        let img = image::RgbImage::from_pixel(256, 256, image::Rgb([i * 60, 30, 100]));
        let mut bytes = Cursor::new(Vec::new());
        img.write_to(&mut bytes, image::ImageFormat::Png).unwrap();
        ids.push(
            initial
                .upload(&user.id, bytes.get_ref(), None, None)
                .await
                .unwrap()
                .id,
        );
    }
    let mut model = ModelSpec::new(
        "fal",
        "fixture-trainer",
        "Recorded FLUX trainer",
        MediaKind::Lora,
        &[Task::Training],
    );
    model.prompt = false;
    model.pricing = Some(Pricing::PerStep { usd: 0.001 });
    model.inputs = vec![InputSlot {
        min: 3,
        ..InputSlot::new(InputRole::Reference, MediaKind::Image, 100)
    }];
    model.meta = json!({"lora_family":"flux1","training_zip_field":"images_data_url","lora_output":"diffusers_lora_file","training_schema":{"required":["images_data_url"],"properties":{"images_data_url":{"type":"string"},"steps":{"type":"integer","minimum":100,"maximum":10000},"trigger_word":{"type":"string"}}}});
    sqlx::query("INSERT INTO model_cache(provider,scope,data,fetched_at) VALUES('fal',?,?,?)")
        .bind(&model.id)
        .bind(serde_json::to_string(&model).unwrap())
        .bind(pictesa_core::now_ms())
        .execute(initial.db())
        .await
        .unwrap();
    let e = Engine::open(cfg).await.unwrap(); // No start: no provider requests can leave this test.
    let req = TrainRequest {
        trainer: model.id.clone(),
        asset_ids: ids,
        options: TrainingOptions {
            trigger_word: "maya".into(),
            ..Default::default()
        },
    };
    assert!(
        e.train(&user.id, req.clone()).await.is_err(),
        "no wallet funds"
    );
    e.grant("training@example.com", 5.0, "fixture")
        .await
        .unwrap();
    e.set_budgets(
        &user.id,
        BudgetInput {
            daily_usd: Some(1.0),
            monthly_usd: None,
        },
    )
    .await
    .unwrap();
    assert!(
        e.estimate_training(&user.id, req.clone()).await.is_err(),
        "markup costs 1.25, above 1.00 cap"
    );
    e.set_budgets(
        &user.id,
        BudgetInput {
            daily_usd: Some(1.25),
            monthly_usd: None,
        },
    )
    .await
    .unwrap();
    let before = e.billing_view(&user.id).await.unwrap().balance_usd;
    let estimate = e.estimate_training(&user.id, req.clone()).await.unwrap();
    assert_eq!(estimate.total_estimated_cost_usd, Some(1.25));
    assert_eq!(
        e.billing_view(&user.id).await.unwrap().balance_usd,
        before,
        "estimate reserves nothing"
    );
    let job = e.train(&user.id, req.clone()).await.unwrap().jobs.remove(0);
    assert_eq!(job.cost_usd, Some(1.25));
    assert_eq!(
        e.billing_view(&user.id).await.unwrap().balance_usd,
        before - 1.25
    );
    assert!(
        e.train(&user.id, req.clone()).await.is_err(),
        "queued training reserves budget"
    );
    e.cancel(&user.id, &job.id).await.unwrap();
    assert_eq!(
        e.billing_view(&user.id).await.unwrap().balance_usd,
        before,
        "unsent cancel refunds once"
    );
    e.cancel(&user.id, &job.id).await.unwrap();
    assert_eq!(e.billing_view(&user.id).await.unwrap().balance_usd, before);
    let (sent,): (i64,) = sqlx::query_as("SELECT sum(submitting) FROM jobs")
        .fetch_one(e.db())
        .await
        .unwrap();
    assert_eq!(sent, 0);
}
