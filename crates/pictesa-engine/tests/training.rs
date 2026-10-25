//! Real queue and subprocess, isolated local data, no credentials or paid calls.
use pictesa_core::lora::TrainingOptions;
use pictesa_engine::{
    training::TrainRequest, AssetFilter, Config, CreateGeneration, Engine, JobStatus, LOCAL_USER,
};
use serde_json::{json, Value};
use std::{io::Cursor, time::Duration};

async fn setup() -> (Engine, std::path::PathBuf, Vec<String>) {
    std::env::set_var("PICTESA_MOCK", "1");
    std::env::set_var("PICTESA_DENY_PAID", "1");
    let root = std::env::temp_dir().join(format!(
        "pictesa-training-test-{}",
        pictesa_engine::new_id()
    ));
    let e = Engine::open(Config::local(&root)).await.unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let script = root.join("fake-ai-toolkit");
        std::fs::write(&script, include_str!("fixtures/fake-ai-toolkit.py")).unwrap();
        std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o700)).unwrap();
        e.set_credentials(
            LOCAL_USER,
            "ai-toolkit",
            json!({"path":script}).as_object().unwrap(),
        )
        .await
        .unwrap();
    }
    let mut ids = vec![];
    for i in 0..3 {
        let img = image::RgbImage::from_pixel(256, 256, image::Rgb([i * 60, 30, 100]));
        let mut bytes = Cursor::new(Vec::new());
        img.write_to(&mut bytes, image::ImageFormat::Png).unwrap();
        ids.push(
            e.upload(LOCAL_USER, bytes.get_ref(), Some("source.png"), None)
                .await
                .unwrap()
                .id,
        );
    }
    e.start_scoped();
    (e, root, ids)
}
fn train(ids: Vec<String>) -> TrainRequest {
    TrainRequest {
        trainer: "ai-toolkit:flux1".into(),
        asset_ids: ids,
        options: TrainingOptions {
            trigger_word: "maya_person".into(),
            ..Default::default()
        },
    }
}

#[tokio::test]
#[cfg(unix)]
async fn local_training_progress_asset_and_generation_use() {
    let (e, root, ids) = setup().await;
    let listed = e
        .models(
            LOCAL_USER,
            &pictesa_engine::ModelFilter {
                provider: Some("mock".into()),
                ..Default::default()
            },
        )
        .await
        .unwrap();
    let mock = listed.iter().find(|m| m.spec.id == "mock:image").unwrap();
    assert_eq!(
        mock.spec.meta["supports_loras"], true,
        "composer needs the capability in catalog responses"
    );
    assert_eq!(mock.spec.meta["lora_family"], "flux1");
    let trainers = e
        .models(
            LOCAL_USER,
            &pictesa_engine::ModelFilter {
                output: Some("lora".into()),
                provider: Some("ai-toolkit".into()),
                ..Default::default()
            },
        )
        .await
        .unwrap();
    assert!(
        trainers
            .iter()
            .all(|m| !m.spec.complete && m.spec.meta["lora_family"].is_string()),
        "training UI loads full trainer controls"
    );
    let character = e
        .create_character(
            LOCAL_USER,
            pictesa_engine::characters::CharacterInput {
                name: Some("Maya".into()),
                asset_ids: Some(ids.clone()),
                ..Default::default()
            },
        )
        .await
        .unwrap();
    let mut request = train(vec![]);
    request.options.character = Some(character.id.clone());
    assert_eq!(
        e.estimate_training(LOCAL_USER, request.clone())
            .await
            .unwrap()
            .total_estimated_cost_usd,
        Some(0.0)
    );
    let job = e.train(LOCAL_USER, request).await.unwrap().jobs.remove(0);
    let mut events = e.subscribe();
    let progressed = tokio::time::timeout(Duration::from_secs(15), async {
        loop {
            let event = events.recv().await.unwrap();
            if event.kind == "job"
                && event.data["id"] == job.id
                && event.data["progress"]
                    .as_f64()
                    .is_some_and(|p| p > 0.0 && p < 0.99)
            {
                break;
            }
        }
    })
    .await;
    assert!(
        progressed.is_ok(),
        "local stdout progress must reach job events"
    );
    let other = Engine::open(Config::local(&root)).await.unwrap();
    other.start_scoped();
    let queued = other
        .train(LOCAL_USER, train(ids.clone()))
        .await
        .unwrap()
        .jobs
        .remove(0);
    tokio::time::sleep(Duration::from_millis(250)).await;
    assert_eq!(
        other.job(LOCAL_USER, &queued.id).await.unwrap().status,
        JobStatus::Queued,
        "another engine must not train on the same GPU at the same time"
    );
    other.cancel(LOCAL_USER, &queued.id).await.unwrap();
    let done = e
        .wait_job(LOCAL_USER, &job.id, Duration::from_secs(20))
        .await
        .unwrap();
    assert_eq!(done.status, JobStatus::Succeeded, "{:?}", done.error);
    assert_eq!(done.cost_usd, Some(0.0));
    let loras = e
        .loras(LOCAL_USER, Some("flux1"), Some(&character.id))
        .await
        .unwrap();
    assert_eq!(loras.len(), 1);
    let lora = &loras[0];
    assert_eq!(lora.trigger_word, "maya_person");
    assert_eq!(lora.asset.kind, pictesa_core::MediaKind::Lora);
    let (file, mime, _) = e
        .asset_file(Some(LOCAL_USER), &lora.asset.id)
        .await
        .unwrap();
    assert!(file.is_file());
    assert_eq!(mime, "application/x-safetensors");
    let (grid, _) = e.assets(LOCAL_USER, &AssetFilter::default()).await.unwrap();
    assert_eq!(grid.len(), 3, "weights stay out of the image library");
    assert!(e
        .loras("another-user", None, None)
        .await
        .unwrap()
        .is_empty());
    let req = CreateGeneration {
        model: "mock:image".into(),
        prompt: "at a lake".into(),
        params: json!({"loras":[{"asset_id":lora.asset.id,"strength":0.75}]})
            .as_object()
            .unwrap()
            .clone(),
        ..Default::default()
    };
    let generated = e
        .create_generation(LOCAL_USER, req)
        .await
        .unwrap()
        .jobs
        .remove(0);
    let generated = e
        .wait_job(LOCAL_USER, &generated.id, Duration::from_secs(20))
        .await
        .unwrap();
    assert_eq!(
        generated.status,
        JobStatus::Succeeded,
        "{:?}",
        generated.error
    );
    assert_eq!(generated.prompt, "maya_person at a lake");
    let (remote,): (String,) = sqlx::query_as("SELECT remote FROM jobs WHERE id=?")
        .bind(&generated.id)
        .fetch_one(e.db())
        .await
        .unwrap();
    let remote: Value = serde_json::from_str(&remote).unwrap();
    assert_eq!(remote["data"]["loras"][0]["strength"], 0.75);
    assert_eq!(
        remote["data"]["loras"][0]["path"],
        file.to_string_lossy().as_ref()
    );
    let mut wrong = CreateGeneration {
        model: "mock:video".into(),
        prompt: "test".into(),
        params: json!({"loras":[{"asset_id":lora.asset.id,"strength":1}]})
            .as_object()
            .unwrap()
            .clone(),
        ..Default::default()
    };
    assert!(e
        .create_generation(LOCAL_USER, wrong.clone())
        .await
        .is_err());
    wrong.model = "mock:image".into();
    wrong.params["loras"] = json!([{"asset_id":lora.asset.id,"strength":3}]);
    assert!(e.create_generation(LOCAL_USER, wrong).await.is_err());
    assert!(std::fs::read_dir(root.join("tmp"))
        .unwrap()
        .next()
        .is_none());
}

#[tokio::test]
#[cfg(unix)]
async fn training_validation_cancel_and_retry() {
    let (e, root, ids) = setup().await;
    for bad in [
        train(ids[..2].to_vec()),
        train(vec![ids[0].clone(); 3]),
        TrainRequest {
            options: TrainingOptions {
                family: "wan21".into(),
                ..train(ids.clone()).options
            },
            ..train(ids.clone())
        },
    ] {
        assert!(e.train(LOCAL_USER, bad).await.is_err());
    }
    let (count,): (i64,) = sqlx::query_as("SELECT count(*) FROM jobs")
        .fetch_one(e.db())
        .await
        .unwrap();
    assert_eq!(count, 0, "invalid training queues and reserves nothing");
    let mut req = train(ids);
    req.options.trigger_word = "cancel_me".into();
    let job = e.train(LOCAL_USER, req).await.unwrap().jobs.remove(0);
    tokio::time::timeout(Duration::from_secs(15), async {
        loop {
            let j = e.job(LOCAL_USER, &job.id).await.unwrap();
            if j.progress.is_some() {
                break;
            }
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
    })
    .await
    .unwrap();
    let temp_root = root.join("tmp");
    // The fixture writes a PID only under its own generated config's directory.
    let dirs = std::fs::read_dir(&temp_root)
        .unwrap()
        .flatten()
        .map(|e| e.path())
        .filter(|p| p.join("pid").is_file())
        .collect::<Vec<_>>();
    let pids = dirs
        .iter()
        .flat_map(|p| {
            ["pid", "worker-pid"]
                .into_iter()
                .filter_map(|name| std::fs::read_to_string(p.join(name)).ok())
        })
        .collect::<Vec<_>>();
    e.cancel(LOCAL_USER, &job.id).await.unwrap();
    tokio::time::timeout(Duration::from_secs(10), async {
        loop {
            if dirs.iter().all(|p| !p.exists())
                && pids
                    .iter()
                    .all(|pid| !std::path::Path::new(&format!("/proc/{}", pid.trim())).exists())
            {
                break;
            }
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
    })
    .await
    .unwrap();
    assert_eq!(
        e.job(LOCAL_USER, &job.id).await.unwrap().status,
        JobStatus::Canceled
    );
    assert!(e.loras(LOCAL_USER, None, None).await.unwrap().is_empty());
    #[cfg(target_os = "linux")]
    for pid in pids {
        assert!(
            !std::path::Path::new(&format!("/proc/{}", pid.trim())).exists(),
            "canceled child still exists"
        );
    }
    let retried = e.retry(LOCAL_USER, &job.id).await.unwrap().jobs.remove(0);
    let done = e
        .wait_job(LOCAL_USER, &retried.id, Duration::from_secs(30))
        .await
        .unwrap();
    assert_eq!(done.status, JobStatus::Succeeded, "{:?}", done.error);
    assert_eq!(e.loras(LOCAL_USER, None, None).await.unwrap().len(), 1);
}
