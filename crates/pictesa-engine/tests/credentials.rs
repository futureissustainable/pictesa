use pictesa_engine::{Config, Engine, LOCAL_USER};
use serde_json::json;

#[tokio::test]
async fn url_fields_take_web_addresses_only() {
    let root = std::env::temp_dir().join(format!("pictesa-credentials-{}", pictesa_engine::new_id()));
    let e = Engine::open(Config::local(&root)).await.unwrap();
    for bad in ["not a url", "javascript:alert(1)", "file:///etc/passwd", "127.0.0.1:8188"] {
        let err = e.set_credentials(LOCAL_USER, "comfyui", json!({ "base_url": bad }).as_object().unwrap()).await.unwrap_err();
        assert!(err.to_string().contains("http:// or https://"), "{bad}: {err}");
    }
    e.set_credentials(LOCAL_USER, "comfyui", json!({ "base_url": "http://127.0.0.1:8188" }).as_object().unwrap()).await.unwrap();
}
