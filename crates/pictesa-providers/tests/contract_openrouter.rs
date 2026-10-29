//! OpenRouter contract: OpenRouter's documented answers replayed through the real adapter, checking
//! what it sends. openrouter.ai resolves to the local stand-in; no key or network is used.
//! The adapter generates images synchronously through chat completions, so there is no poll or cancel.

mod common;

use base64::Engine as _;
use common::{start, Fake, Reply, PNG_1X1};
use pictesa_core::{Ctx, GenerationRequest, InputRole, MediaInput, MediaKind, Output, Provider, ProviderError, Submission};
use pictesa_providers::openrouter::OpenRouter;
use serde_json::{json, Value};

fn ctx(fake: &Fake) -> Ctx {
    fake.ctx(&["openrouter.ai"], &[("api_key", "test-key")])
}

fn request(prompt: &str, params: Value, inputs: Vec<MediaInput>) -> GenerationRequest {
    GenerationRequest {
        job_id: "job-1".into(),
        model: "google/gemini-2.5-flash-image".into(),
        prompt: prompt.into(),
        negative_prompt: None,
        params: params.as_object().cloned().unwrap_or_default(),
        inputs,
        num_outputs: 1,
    }
}

fn model() -> pictesa_core::ModelSpec {
    OpenRouter.models().into_iter().find(|m| m.id == "openrouter:google/gemini-2.5-flash-image").unwrap()
}

#[tokio::test]
async fn check_reads_the_current_key_with_a_bearer_token() {
    // https://openrouter.ai/docs/api/api-reference/api-keys/get-current-api-key (200 example, trimmed).
    let fake = start(|_| Reply::json(json!({ "data": { "label": "sk-or-v1-0e6...1c96", "limit": null, "limit_remaining": null, "usage": 0.5 } }))).await;
    OpenRouter.check(&ctx(&fake)).await.unwrap();
    let call = &fake.calls()[0];
    assert_eq!((call.method.as_str(), call.path.as_str()), ("GET", "/api/v1/key"));
    assert_eq!(call.header("authorization"), Some("Bearer test-key"));
}

#[tokio::test]
async fn discover_keeps_models_that_output_images() {
    // https://openrouter.ai/docs/api/api-reference/models/list-all-models-and-their-properties (200 example),
    // plus one entry of the same shape whose output_modalities include image.
    let fake = start(|_| {
        Reply::json(json!({ "data": [
            { "id": "openai/gpt-4", "canonical_slug": "openai/gpt-4", "name": "GPT-4", "created": 1692901234, "description": "GPT-4 is a large multimodal model.",
              "architecture": { "input_modalities": ["text"], "output_modalities": ["text"], "modality": "text->text", "tokenizer": "GPT" } },
            { "id": "google/gemini-2.5-flash-image", "canonical_slug": "google/gemini-2.5-flash-image", "name": "Google: Nano Banana", "created": 1759881600, "description": "Image generation and editing.",
              "architecture": { "input_modalities": ["image", "text"], "output_modalities": ["image", "text"], "modality": "text+image->text+image", "tokenizer": "Gemini" } }
        ] }))
    })
    .await;
    let specs = OpenRouter.discover(&ctx(&fake)).await.unwrap();
    assert_eq!(specs.iter().map(|s| s.id.as_str()).collect::<Vec<_>>(), ["openrouter:google/gemini-2.5-flash-image"]);
    assert_eq!(specs[0].output, MediaKind::Image);
    let call = &fake.calls()[0];
    assert_eq!((call.path.split('?').next().unwrap(), call.header("authorization")), ("/api/v1/models", Some("Bearer test-key")));
}

/// Chat completion with `modalities` and `image_config`, a reference image as an `image_url` part,
/// and the image returned as a data URL in `message.images`
/// (https://openrouter.ai/docs/api/api-reference/chat/create-a-chat-completion, `ChatAssistantImages`).
#[tokio::test]
async fn image_generation_sends_the_chat_request_and_reads_the_image() {
    let png = base64::engine::general_purpose::STANDARD.encode(PNG_1X1);
    let answer = json!({
        "id": "gen-1750000000-abc", "object": "chat.completion", "created": 1750000000, "model": "google/gemini-2.5-flash-image",
        "choices": [{ "index": 0, "finish_reason": "stop", "message": { "role": "assistant", "content": "Here is your image.",
            "images": [{ "type": "image_url", "image_url": { "url": format!("data:image/png;base64,{png}") } }] } }],
        "usage": { "prompt_tokens": 10, "completion_tokens": 1290, "total_tokens": 1300, "cost": 0.0387 }
    });
    let fake = start(move |_| Reply::json(answer.clone())).await;
    let ctx = ctx(&fake);
    let reference = MediaInput {
        role: InputRole::Reference,
        kind: MediaKind::Image,
        mime: "image/png".into(),
        path: "/nonexistent".into(),
        url: Some("https://example.com/photo.jpg".into()),
        width: None,
        height: None,
    };
    let req = request("make it a watercolor", json!({ "aspect_ratio": "16:9", "quality": "high" }), vec![reference]);
    let Submission::Done(out) = OpenRouter.submit(&ctx, &model(), &req).await.unwrap() else { panic!("synchronous") };

    let call = &fake.calls()[0];
    assert_eq!((call.method.as_str(), call.path.as_str()), ("POST", "/api/v1/chat/completions"));
    assert_eq!(call.header("authorization"), Some("Bearer test-key"));
    assert_eq!(
        call.json(),
        json!({
            "model": "google/gemini-2.5-flash-image",
            "messages": [{ "role": "user", "content": [
                { "type": "text", "text": "make it a watercolor" },
                { "type": "image_url", "image_url": { "url": "https://example.com/photo.jpg" } }
            ] }],
            "modalities": ["image", "text"],
            "image_config": { "aspect_ratio": "16:9", "quality": "high" }
        })
    );
    assert!(matches!(&out[..], [Output::Bytes { data, mime }] if data == PNG_1X1 && mime == "image/png"));
    // `usage.cost` is the charge in credits (USD).
    assert_eq!(ctx.reported_cost(), Some(0.0387));
}

/// Error bodies from https://openrouter.ai/docs/api_reference/errors-and-debugging and
/// https://openrouter.ai/docs/api_reference/limits, each with the status and headers it is sent with.
#[tokio::test]
async fn errors_map_to_their_kinds() {
    let fake = start(|call| {
        let case = call.json()["messages"][0]["content"][0]["text"].as_str().unwrap_or("").to_string();
        let error = |code: u16, body: Value| Reply::status(code, body);
        match case.as_str() {
            "401" => error(401, json!({ "error": { "code": 401, "message": "Missing Authentication header" } })),
            "402" => error(402, json!({ "error": { "code": 402, "message": "Insufficient credits. Add more using https://openrouter.ai/credits" } })),
            "402-in-flight" => error(402, json!({ "error": { "code": 402,
                "message": "This request would exceed your available credits given your current in-flight requests. Retry after in-flight requests settle, or add credits.",
                "metadata": { "reason": "in_flight_budget_exhausted", "limit_source": "openrouter_in_flight_budget",
                    "remedy_hint": "Retry after your in-flight requests settle (see the Retry-After header). Adding credits at https://openrouter.ai/settings/credits raises your in-flight budget, up to a capped ceiling." } } }))
                .header("retry-after", "2"),
            "403-budget" => error(403, json!({ "error": { "code": 403,
                "message": "Budget limit exceeded (weekly limit). Contact your org admin. $52.00 of $50.00 used; resets 2026-10-12T00:00:00.000Z.",
                "metadata": { "limit_source": "openrouter_guardrail_budget", "remedy_hint": "Wait for the budget window to reset (see metadata.resets_at), or ask your org admin to raise the guardrail budget.",
                    "scope": "workspace_default_member", "limit_usd": 50, "spend_usd": 52, "resets_at": "2026-10-12T00:00:00.000Z" } } })),
            "403-moderation" => error(403, json!({ "error": { "code": 403, "message": "Input flagged by moderation",
                "metadata": { "reasons": ["violence"], "flagged_input": "a violent scene", "provider_name": "Google", "model_slug": "google/gemini-2.5-flash-image" } } })),
            "403-permissions" => error(403, json!({ "error": { "code": 403, "message": "Only management keys can perform this operation" } })),
            "429" => error(429, json!({ "error": { "code": 429, "message": "Rate limit exceeded" } })).header("retry-after", "60"),
            "502" => error(502, json!({ "error": { "code": 502, "message": "Provider returned error" } })),
            // A provider failure after OpenRouter accepted the request: HTTP 200 with an error body.
            "200-rate-limited" => Reply::json(json!({ "error": { "code": 429, "message": "Rate limit exceeded", "metadata": { "error_type": "rate_limit_exceeded", "provider_code": "rate_limited" } } })),
            _ => Reply::json(json!({ "error": { "code": 403, "message": "Generation blocked by the provider's safety filter",
                "metadata": { "error_type": "content_policy_violation", "provider_name": "Google AI Studio" } } })),
        }
    })
    .await;
    let ctx = ctx(&fake);
    for (case, expected) in [
        ("401", "auth"),
        ("402", "insufficient_credits"),
        ("402-in-flight", "rate_limited"),
        ("403-budget", "insufficient_credits"),
        ("403-moderation", "content_policy"),
        ("403-permissions", "auth"),
        ("429", "rate_limited"),
        ("502", "transient"),
        ("200-rate-limited", "rate_limited"),
        ("200-content-policy", "content_policy"),
    ] {
        let err = OpenRouter.submit(&ctx, &model(), &request(case, json!({}), vec![])).await.unwrap_err();
        assert_eq!(err.kind(), expected, "{case}: {err:?}");
        if case == "429" {
            assert!(matches!(err, ProviderError::RateLimited { retry_after_ms: Some(60_000) }));
        }
        if case == "402-in-flight" {
            assert!(matches!(err, ProviderError::RateLimited { retry_after_ms: Some(2_000) }));
        }
    }
}
