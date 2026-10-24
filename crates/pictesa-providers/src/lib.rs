//! Provider adapters. Each module implements [`pictesa_core::Provider`] for one backend.
//!
//! To add a provider: create a module, implement the trait (see `fal.rs` for a gateway with
//! live discovery and `util.rs` for shared helpers), then list it in [`registry`].
//! Probe one without the engine: `cargo run -p pictesa-providers --example probe -- <id> [model]`.

use std::sync::Arc;

use pictesa_core::Provider;

pub mod util;

// Gateways
pub mod fal;
// BATCH-A-MODULES (openai, google, openrouter, xai, byteplus, bfl, replicate)
// BATCH-B-MODULES (runway, luma, kling, minimax, alibaba, stability, ideogram, recraft)
// LOCAL-MODULES (comfy_router, comfyui, sdcpp)

/// Every built-in provider, in the order the Settings page shows them.
pub fn registry() -> Vec<Arc<dyn Provider>> {
    vec![
        Arc::new(fal::Fal),
        // BATCH-A-REGISTRY
        // BATCH-B-REGISTRY
        // LOCAL-REGISTRY
    ]
}
