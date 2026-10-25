//! Developer probe for one provider, without the engine.
//!
//!   cargo run -p pictesa-providers --example probe -- <provider> [model_id] [--check] [--submit "prompt"]
//!
//! Credentials come from the provider's environment variables (see its `info().fields`).
//! Listing and resolving models is free on every provider; `--submit` runs a real (paid) generation.

use std::collections::HashMap;

use pictesa_core::{Ctx, GenerationRequest, PollStatus, Submission};

#[tokio::main]
async fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let pid = args.first().expect("usage: probe <provider> [model] [--check] [--submit prompt]");
    let provider = pictesa_providers::registry().into_iter().find(|p| p.info().id == *pid).expect("unknown provider");
    let info = provider.info();
    let mut creds = HashMap::new();
    for f in &info.fields {
        if let Some(v) = f.env.iter().find_map(|e| std::env::var(e).ok()).or_else(|| f.default.clone()) {
            creds.insert(f.key.clone(), v);
        }
    }
    let ctx = Ctx::new(reqwest::Client::builder().user_agent("pictesa-probe").build().unwrap(), creds);
    if args.iter().any(|a| a == "--check") {
        println!("check: {:?}", provider.check(&ctx).await);
    }
    let mut models = provider.models();
    match provider.discover(&ctx).await {
        Ok(found) => {
            println!("static {} + discovered {}", models.len(), found.len());
            models.extend(found);
        }
        Err(e) => println!("discover failed: {e}"),
    }
    let featured: Vec<&str> = models.iter().filter(|m| m.tags.iter().any(|t| t == "featured")).map(|m| m.id.as_str()).collect();
    println!("featured ({}): {}", featured.len(), featured.join(", "));
    let Some(model_id) = args.get(1).filter(|a| !a.starts_with("--")) else { return };
    let full = format!("{pid}:{model_id}");
    let spec = models.iter().find(|m| m.id == full).cloned().unwrap_or_else(|| panic!("{full} not in catalog"));
    let spec = provider.resolve(&ctx, &spec).await.expect("resolve failed");
    println!("{}", serde_json::to_string_pretty(&spec).unwrap());
    if let Some(i) = args.iter().position(|a| a == "--submit") {
        let req = GenerationRequest {
            job_id: "probe".into(),
            model: spec.remote_id().into(),
            prompt: args.get(i + 1).cloned().unwrap_or_default(),
            negative_prompt: None,
            params: Default::default(),
            inputs: vec![],
            num_outputs: 1,
        };
        let mut handle = match provider.submit(&ctx, &spec, &req).await.expect("submit failed") {
            Submission::Done(out) => return println!("done: {out:?}"),
            Submission::Pending(h) => h,
        };
        for attempt in 0..600 {
            tokio::time::sleep(provider.poll_interval(attempt)).await;
            match provider.poll(&ctx, &handle).await {
                Ok(PollStatus::Done(out)) => return println!("done: {out:?}"),
                Ok(PollStatus::Failed(e)) => return println!("failed: {e}"),
                Ok(s) => println!("{s:?}"),
                Err(e) => println!("poll error: {e}"),
            }
            handle = handle.clone();
        }
    }
}
