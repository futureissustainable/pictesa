//! Provider adapters. Each module implements [`pictesa_core::Provider`] for one backend.
//!
//! To add a provider: create a module, implement the trait (see `fal.rs` for a gateway with
//! live discovery and `util.rs` for shared helpers), then list it in [`registry`].
//! Probe one without the engine: `cargo run -p pictesa-providers --example probe -- <id> [model]`.

use std::sync::Arc;

use pictesa_core::Provider;

pub mod mock;
pub mod util;

// Gateways
pub mod fal;
pub mod openai;
pub mod google;
pub mod openrouter;
pub mod xai;
pub mod byteplus;
pub mod bfl;
pub mod replicate;
// BATCH-A-MODULES (openai, google, openrouter, xai, byteplus, bfl, replicate)
pub mod runway;
pub mod luma;
pub mod kling;
pub mod minimax;
pub mod alibaba;
pub mod stability;
pub mod ideogram;
pub mod recraft;
// BATCH-B-MODULES (runway, luma, kling, minimax, alibaba, stability, ideogram, recraft)
// LOCAL-MODULES (comfy_router, comfyui, sdcpp)

/// Every built-in provider, in the order the Settings page shows them.
pub fn registry() -> Vec<Arc<dyn Provider>> {
    let mut list: Vec<Arc<dyn Provider>> = vec![
        Arc::new(fal::Fal),
        Arc::new(openai::OpenAI),
        Arc::new(google::Google),
        Arc::new(openrouter::OpenRouter),
        Arc::new(xai::Xai),
        Arc::new(byteplus::BytePlus),
        Arc::new(bfl::Bfl),
        Arc::new(replicate::Replicate),
        // BATCH-A-REGISTRY
        Arc::new(runway::Runway),
        Arc::new(luma::Luma),
        Arc::new(kling::Kling),
        Arc::new(minimax::Minimax),
        Arc::new(alibaba::Alibaba),
        Arc::new(stability::Stability),
        Arc::new(ideogram::Ideogram),
        Arc::new(recraft::Recraft),
        // BATCH-B-REGISTRY
        // LOCAL-REGISTRY
    ];
    if std::env::var("PICTESA_MOCK").is_ok_and(|v| v == "1") {
        list.insert(0, Arc::new(mock::Mock));
    }
    list
}
