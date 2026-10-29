mod common;

use common::{start, Reply};
use serde_json::json;

#[tokio::test]
async fn the_stand_in_answers_for_a_real_host_name() {
    let fake = start(|call| Reply::json(json!({ "path": call.path, "host": call.host }))).await;
    let ctx = fake.ctx(&["api.example-provider.com"], &[("api_key", "test-key")]);
    let v: serde_json::Value = ctx.http.post("https://api.example-provider.com/v1/run?x=1").bearer_auth("test-key").json(&json!({ "a": 1 })).send().await.unwrap().json().await.unwrap();
    assert_eq!(v["path"], "/v1/run?x=1");
    assert_eq!(v["host"], "api.example-provider.com");
    let call = &fake.calls()[0];
    assert_eq!(call.header("authorization"), Some("Bearer test-key"));
    assert_eq!(call.json()["a"], 1);
}
