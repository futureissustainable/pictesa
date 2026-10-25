//! A charge the provider reports becomes the run's actual cost. OpenRouter is a local stub here.
use std::time::Duration;

use axum::{
    routing::{get, post},
    Json, Router,
};
use pictesa_engine::{Config, CreateGeneration, Engine, JobStatus, LOCAL_USER};
use serde_json::{json, Map};

const PNG: &str = "iVBORw0KGgoAAAANSUhEUgAAAAQAAAAECAIAAAAmkwkpAAAAEElEQVR4nGP4lGkLRwzEcQCiohmBzIRWUgAAAABJRU5ErkJggg==";

#[tokio::test(flavor = "multi_thread")]
async fn openrouter_charge_becomes_the_actual_cost() {
    std::env::set_var("PICTESA_LEADERBOARDS", "off");
    std::env::remove_var("PICTESA_DENY_PAID");
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let base = format!("http://{}", listener.local_addr().unwrap());
    let stub = Router::new()
        .route("/key", get(|| async { Json(json!({ "data": {} })) }))
        .route(
            "/models",
            get(|| async {
                Json(json!({ "data": [{ "id": "google/gemini-2.5-flash-image", "name": "Nano Banana", "architecture": { "output_modalities": ["image"] } }] }))
            }),
        )
        .route(
            "/chat/completions",
            post(|| async {
                Json(json!({
                    "choices": [{ "message": { "images": [{ "image_url": { "url": format!("data:image/png;base64,{PNG}") } }] } }],
                    "usage": { "prompt_tokens": 12, "completion_tokens": 1290, "cost": 0.0387 }
                }))
            }),
        );
    let server = tokio::spawn(async move { axum::serve(listener, stub).await.unwrap() });

    let dir = std::env::temp_dir().join(format!("pictesa-actual-cost-{}", pictesa_core::now_ms()));
    let e = Engine::open(Config::local(&dir)).await.unwrap();
    e.start();
    let values: Map<String, serde_json::Value> = [("api_key", json!("test-key")), ("base_url", json!(base))].into_iter().map(|(k, v)| (k.into(), v)).collect();
    e.set_credentials(LOCAL_USER, "openrouter", &values).await.unwrap();
    let created = e
        .create_generation(LOCAL_USER, CreateGeneration { model: "openrouter:google/gemini-2.5-flash-image".into(), prompt: "a cabin".into(), ..Default::default() })
        .await
        .unwrap();
    assert_eq!(created.jobs[0].cost_usd, None, "OpenRouter publishes no price before the run");
    let job = e.wait_job(LOCAL_USER, &created.jobs[0].id, Duration::from_secs(30)).await.unwrap();
    server.abort();
    assert_eq!(job.status, JobStatus::Succeeded, "{:?}", job.error);
    assert_eq!(job.cost_usd, Some(0.0387));
    let spend = e.spend_view(LOCAL_USER).await.unwrap();
    assert!((spend.today_usd - 0.0387).abs() < 1e-6, "spend counts the charge, got {}", spend.today_usd);
}
