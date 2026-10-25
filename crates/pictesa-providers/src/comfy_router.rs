//! Comfy Router: Comfy's gateway to hosted image, video and audio models behind one API key.
//!
//! - Catalog: `GET /v2/models` (paged, needs a key). Without a key, [`SNAPSHOT`] lists the media
//!   models of the October 2026 catalog so they show before setup.
//! - Schemas: `GET /v2/models/{provider}/{model}/openapi.json`; the same documents are published
//!   keyless at `https://docs.comfy.org/router-schemas/{provider}/{model}.json`. The document root
//!   carries `x-comfy-router-input-modalities` and `x-comfy-router-output-modalities`.
//! - Runs: queued delivery. `POST /v2/models/{id}/requests` with an `Idempotency-Key`, then
//!   `GET .../requests/{request_id}/status` (honouring `Retry-After`), `GET .../requests/{request_id}`
//!   for the native result, `PUT .../cancel`. Models that cannot be queued run on the synchronous route.
//!
//! Each model keeps its partner's native body. [`read_schema`] turns the published input schema into
//! Pictesa params and inputs and records where each value goes in the body (`meta.bindings`).

use std::{
    collections::HashMap,
    sync::{Mutex, OnceLock},
    time::{Duration, Instant},
};

use async_trait::async_trait;
use base64::Engine as _;
use pictesa_core::{
    keys, CredentialField, Ctx, EnumOption, GenerationRequest, InputRole, InputSlot, MediaInput,
    MediaKind, ModelSpec, Output, ParamKind, ParamSpec, PollStatus, Provider, ProviderError,
    ProviderInfo, ProviderKind, ProviderResult, RemoteHandle, Submission, Task,
};
use serde::{Deserialize, Serialize};
use serde_json::{json, Map, Value};
use sha2::{Digest, Sha256};

use crate::util::{net_error, API_TIMEOUT};

pub struct ComfyRouter;

const ID: &str = "comfy_router";
const API: &str = "https://api.comfy.org";
const DOCS_SCHEMAS: &str = "https://docs.comfy.org/router-schemas";
/// The synchronous route can hold the connection for Router's 10-minute deadline.
const SYNC_TIMEOUT: Duration = Duration::from_secs(660);

/// Media models of the Router catalog (docs snapshot, October 2026): id, name, output
/// (`i`mage, `v`ideo, `a`udio) and accepted input kinds (`t`ext, `i`mage, `v`ideo, `a`udio).
pub(crate) const SNAPSHOT: &[(&str, &str, char, &str)] = &[
    ("beeble/switchx", "Beeble SwitchX", 'v', "itv"),
    ("bfl/erase-v1", "Erase V1", 'i', "i"),
    ("bfl/flux-2-max", "FLUX 2 Max", 'i', "it"),
    ("bfl/flux-2-pro", "FLUX 2 Pro", 'i', "it"),
    ("bfl/flux-3-image", "FLUX 3 Image", 'i', "it"),
    ("bfl/flux-3-video", "FLUX 3 Video", 'v', "it"),
    ("bfl/flux-kontext-max", "FLUX.1 Kontext Max", 'i', "it"),
    ("bfl/flux-kontext-pro", "FLUX.1 Kontext", 'i', "it"),
    ("bfl/flux-pro-1.0-expand", "FLUX Pro 1.0 Expand", 'i', "it"),
    ("bfl/flux-pro-1.0-fill", "FLUX Pro 1.0 Fill", 'i', "it"),
    (
        "bfl/flux-pro-1.1-ultra",
        "Flux 1.1 Pro Ultra Image",
        'i',
        "it",
    ),
    ("bfl/flux-pro-1.1", "FLUX 1.1 Pro", 'i', "it"),
    ("bfl/video-edit-v1", "Video Edit V1", 'v', "tv"),
    ("bfl/video-upscale-v1", "FLUX Video Upscale", 'v', "v"),
    ("bfl/vto-v1", "VTO V1", 'i', "i"),
    ("bria/fibo", "Bria Fibo", 'i', "it"),
    (
        "bria/image-edit-add-object-by-text",
        "Bria Image Edit Add Object By Text",
        'i',
        "it",
    ),
    (
        "bria/image-edit-erase-by-text",
        "Bria Image Edit Erase By Text",
        'i',
        "it",
    ),
    (
        "bria/image-edit-erase-foreground",
        "Bria Image Edit Erase Foreground",
        'i',
        "i",
    ),
    ("bria/image-edit-erase", "Bria Image Edit Erase", 'i', "i"),
    (
        "bria/image-edit-expand",
        "Bria Image Edit Expand",
        'i',
        "it",
    ),
    (
        "bria/image-edit-gen-fill",
        "Bria Image Edit Gen Fill",
        'i',
        "it",
    ),
    (
        "bria/image-edit-increase-resolution",
        "Bria Image Edit Increase Resolution",
        'i',
        "i",
    ),
    (
        "bria/image-edit-product-holding",
        "Bria Image Edit Product Holding",
        'i',
        "it",
    ),
    (
        "bria/image-edit-product-virtual-tryon",
        "Bria Image Edit Product Virtual Tryon",
        'i',
        "it",
    ),
    (
        "bria/image-edit-relight",
        "Bria Image Edit Relight",
        'i',
        "it",
    ),
    (
        "bria/image-edit-remove-background-guided",
        "Bria Image Edit Remove Background Guided",
        'i',
        "it",
    ),
    (
        "bria/image-edit-remove-background",
        "Bria Image Edit Remove Background",
        'i',
        "i",
    ),
    (
        "bria/image-edit-replace-background",
        "Bria Image Edit Replace Background",
        'i',
        "it",
    ),
    (
        "bria/image-edit-replace-object-by-text",
        "Bria Image Edit Replace Object By Text",
        'i',
        "it",
    ),
    (
        "bria/image-edit-reseason",
        "Bria Image Edit Reseason",
        'i',
        "it",
    ),
    (
        "bria/image-edit-restore",
        "Bria Image Edit Restore",
        'i',
        "i",
    ),
    ("bria/image", "Bria Image", 'i', "it"),
    ("bria/video-edit-erase", "Bria Video Edit Erase", 'v', "v"),
    (
        "bria/video-edit-green-screen",
        "Bria Video Edit Green Screen",
        'v',
        "v",
    ),
    (
        "bria/video-edit-remove-background",
        "Bria Video Edit Remove Background",
        'v',
        "v",
    ),
    (
        "bria/video-edit-replace-background",
        "Bria Video Edit Replace Background",
        'v',
        "tv",
    ),
    (
        "byteplus/dreamina-seedance-2-0-260128",
        "Dreamina Seedance 2.0 260128",
        'v',
        "aitv",
    ),
    (
        "byteplus/dreamina-seedance-2-0-fast-260128",
        "Dreamina Seedance 2.0 Fast 260128",
        'v',
        "aitv",
    ),
    (
        "byteplus/dreamina-seedance-2-0-mini",
        "Dreamina Seedance 2.0 Mini",
        'v',
        "aitv",
    ),
    (
        "byteplus/dreamina-seedance-2-5-260628",
        "Dreamina Seedance 2.5 260628",
        'v',
        "aitv",
    ),
    (
        "byteplus/seed-audio-1.0-multilingual",
        "Seed Audio 1.0 Multilingual",
        'a',
        "t",
    ),
    ("byteplus/seed-audio-1.0", "Seed Audio 1.0", 'a', "t"),
    (
        "byteplus/seedance-1-0-pro-250528",
        "Seedance 1.0 Pro 250528",
        'v',
        "it",
    ),
    (
        "byteplus/seedance-1-0-pro-fast-251015",
        "Seedance 1.0 Pro Fast 251015",
        'v',
        "it",
    ),
    (
        "byteplus/seedance-1-5-pro-251215",
        "Seedance 1.5 Pro 251215",
        'v',
        "it",
    ),
    (
        "byteplus/seedream-4-0-250828",
        "Seedream 4.0 250828",
        'i',
        "it",
    ),
    (
        "byteplus/seedream-4-5-251128",
        "Seedream 4.5 251128",
        'i',
        "it",
    ),
    (
        "byteplus/seedream-5-0-260128",
        "Seedream 5.0 260128",
        'i',
        "it",
    ),
    (
        "byteplus/seedream-5-0-flash-260915",
        "Seedream 5.0 Flash 260915",
        'i',
        "it",
    ),
    (
        "byteplus/seedream-5-0-pro-260628",
        "Seedream 5.0 Pro 260628",
        'i',
        "it",
    ),
    (
        "elevenlabs/eleven_multilingual_v2",
        "ElevenLabs Eleven Multilingual V2",
        'a',
        "t",
    ),
    (
        "elevenlabs/eleven_sfx_v2",
        "ElevenLabs Eleven Sfx V2",
        'a',
        "t",
    ),
    ("elevenlabs/eleven_v3", "ElevenLabs Eleven V3", 'a', "t"),
    ("elevenlabs/eleven_v4", "ElevenLabs Eleven V4", 'a', "t"),
    (
        "elevenlabs/eleven_v4_turbo",
        "ElevenLabs Eleven V4 Turbo",
        'a',
        "t",
    ),
    ("fal/crystal-upscaler", "Crystal Upscaler", 'i', "i"),
    (
        "fal/crystal-video-upscaler",
        "Crystal Video Upscaler",
        'v',
        "v",
    ),
    ("fal/flux-3-max", "FLUX 3 Max", 'v', "it"),
    ("fal/h3-max-turbo", "H3 Max Turbo", 'v', "it"),
    ("fal/h3-max", "H3 Max", 'v', "aitv"),
    ("fal/patina", "Patina", 'i', "i"),
    (
        "freepik/ai-image-upscaler-precision-v2",
        "Freepik AI Image Upscaler Precision V2",
        'i',
        "i",
    ),
    (
        "freepik/ai-skin-enhancer-creative",
        "Freepik AI Skin Enhancer Creative",
        'i',
        "i",
    ),
    (
        "freepik/ai-skin-enhancer-faithful",
        "Freepik AI Skin Enhancer Faithful",
        'i',
        "i",
    ),
    (
        "freepik/ai-skin-enhancer-flexible",
        "Freepik AI Skin Enhancer Flexible",
        'i',
        "i",
    ),
    ("heygen/starfish", "HeyGen Starfish", 'a', "t"),
    (
        "higgsfield/higgsfield-genjutsu-motion-transfer-v1.0",
        "Higgsfield Genjutsu Motion Transfer V1.0",
        'v',
        "itv",
    ),
    (
        "higgsfield/higgsfield-kling-3-4k",
        "Higgsfield Kling 3.4k",
        'v',
        "t",
    ),
    (
        "higgsfield/higgsfield-kling-3-pro",
        "Higgsfield Kling 3 Pro",
        'v',
        "t",
    ),
    ("ideogram/ideogram-4-5", "Ideogram 4.5", 'i', "it"),
    ("ideogram/ideogram-v3", "Ideogram V3", 'i', "t"),
    ("ideogram/ideogram-v4", "Ideogram 4.0", 'i', "t"),
    ("ideogram/p-image-ideogram", "P Image Ideogram", 'i', "t"),
    ("kling/kling-3.0-turbo", "Kling 3.0 Turbo", 'v', "t"),
    ("kling/kling-image-o1", "Kling Image O1", 'i', "it"),
    ("kling/kling-v2-5-turbo", "Kling V2.5 Turbo", 'v', "t"),
    ("kling/kling-v2-6", "Kling V2.6", 'v', "t"),
    ("kling/kling-v3-omni", "Kling V3 Omni", 'v', "itv"),
    ("kling/kling-v3", "Kling V3", 'v', "t"),
    ("kling/kling-video-o1", "Kling Video O1", 'v', "itv"),
    (
        "kling/videos-avatar-image2video",
        "Kling Videos Avatar Image 2video",
        'v',
        "ai",
    ),
    ("kling/videos-lip-sync", "Kling Videos Lip Sync", 'v', "av"),
    (
        "kling/videos-video-extend",
        "Kling Videos Video Extend",
        'v',
        "tv",
    ),
    ("krea/krea-2-large", "Krea 2 Large", 'i', "t"),
    ("krea/krea-2-medium-turbo", "Krea 2 Medium Turbo", 'i', "t"),
    ("krea/krea-2-medium", "Krea 2 Medium", 'i', "t"),
    ("krea/krea-2", "Krea 2", 'i', "t"),
    ("krea/krea-3", "Krea 3", 'i', "it"),
    ("ltx/ltx-2-5-fast", "LTX 2.5 Fast", 'v', "t"),
    ("ltx/ltx-2-5-pro", "LTX 2.5 Pro", 'v', "t"),
    ("luma_2/ray-3.2", "Luma Ray 3.2", 'v', "itv"),
    ("luma_2/uni-1-max", "Luma Uni 1 Max", 'i', "it"),
    ("luma_2/uni-1", "Luma Uni 1", 'i', "it"),
    ("luma/photon-1", "Photon 1", 'i', "t"),
    ("luma/photon-flash-1", "Photon Flash 1", 'i', "t"),
    ("luma/ray-2", "Ray 2", 'v', "it"),
    ("luma/ray-flash-2", "Ray Flash 2", 'v', "it"),
    ("minimax/minimax-h3", "MiniMax H3", 'v', "it"),
    (
        "moonvalley/image-to-video",
        "Moonvalley Image To Video",
        'v',
        "it",
    ),
    (
        "moonvalley/text-to-video",
        "Moonvalley Text To Video",
        'v',
        "t",
    ),
    (
        "moonvalley/video-to-video",
        "Moonvalley Video To Video",
        'v',
        "tv",
    ),
    ("openai/gpt-image-1.5", "GPT Image 1.5", 'i', "it"),
    ("openai/gpt-image-1", "GPT Image 1", 'i', "it"),
    (
        "openai/gpt-image-2.5-flare",
        "GPT Image 2.5 Flare",
        'i',
        "it",
    ),
    (
        "openai/gpt-image-2.5-sunburst",
        "GPT Image 2.5 Sunburst",
        'i',
        "it",
    ),
    ("openai/gpt-image-2", "GPT Image 2", 'i', "it"),
    (
        "pika/pika-2.5-image-to-video",
        "Pika 2.5 Image To Video",
        'v',
        "it",
    ),
    (
        "pika/pika-2.5-text-to-video",
        "Pika 2.5 Text To Video",
        'v',
        "t",
    ),
    ("pruna/p-video-2", "Pruna P Video 2", 'v', "ait"),
    ("qwen/qwen-image-3.0-pro", "Qwen Image 3.0 Pro", 'i', "it"),
    ("qwen/qwen-image-3.0", "Qwen Image 3.0", 'i', "it"),
    ("recraft/recraftv2", "Recraft V2", 'i', "t"),
    ("recraft/recraftv3", "Recraft V3", 'i', "t"),
    ("recraft/recraftv4", "Recraft V4", 'i', "t"),
    ("recraft/recraftv4_1", "Recraft V4.1", 'i', "t"),
    ("recraft/recraftv4_1_pro", "Recraft V4.1 Pro", 'i', "t"),
    (
        "recraft/recraftv4_1_pro_vector",
        "Recraft V4.1 Pro Vector",
        'i',
        "t",
    ),
    (
        "recraft/recraftv4_1_utility",
        "Recraft V4.1 Utility",
        'i',
        "t",
    ),
    (
        "recraft/recraftv4_1_utility_pro",
        "Recraft V4.1 Utility Pro",
        'i',
        "t",
    ),
    (
        "recraft/recraftv4_1_utility_pro_vector",
        "Recraft V4.1 Utility Pro Vector",
        'i',
        "t",
    ),
    (
        "recraft/recraftv4_1_utility_vector",
        "Recraft V4.1 Utility Vector",
        'i',
        "t",
    ),
    (
        "recraft/recraftv4_1_vector",
        "Recraft V4.1 Vector",
        'i',
        "t",
    ),
    ("recraft/recraftv4_pro", "Recraft V4 Pro", 'i', "t"),
    ("recraft/recraftv4_styles", "Recraft V4 Styles", 'i', "t"),
    (
        "recraft/recraftv4_styles_pro",
        "Recraft V4 Styles Pro",
        'i',
        "t",
    ),
    (
        "recraft/recraftv4_styles_pro_vector",
        "Recraft V4 Styles Pro Vector",
        'i',
        "t",
    ),
    (
        "recraft/recraftv4_styles_vector",
        "Recraft V4 Styles Vector",
        'i',
        "t",
    ),
    ("runway/aleph2", "Runway Aleph 2", 'v', "itv"),
    ("runway/gen4_image", "Runway Gen 4 Image", 'i', "it"),
    ("runway/gen4_turbo", "Runway Gen 4 Turbo", 'v', "it"),
    ("synclabs/sync-3", "Sync Labs Sync 3", 'v', "aitv"),
    ("veed/clean-audio-video", "VEED Clean Audio Video", 'v', "v"),
    ("veed/clean-audio", "VEED Clean Audio", 'a', "a"),
    ("veed/fabric-1.0", "VEED Fabric 1.0", 'v', "ai"),
    ("veed/lipsync-2.0", "VEED Lipsync 2.0", 'v', "av"),
    (
        "veed/video-background-removal-fast",
        "VEED Video Background Removal Fast",
        'v',
        "v",
    ),
    (
        "veed/video-background-removal-green-screen",
        "VEED Video Background Removal Green Screen",
        'v',
        "v",
    ),
    (
        "veed/video-background-removal",
        "VEED Video Background Removal",
        'v',
        "v",
    ),
    (
        "veo/veo-3.1-fast-generate-001",
        "Veo 3.1 Fast Generate 001",
        'v',
        "itv",
    ),
    (
        "veo/veo-3.1-generate-001",
        "Veo 3.1 Generate 001",
        'v',
        "itv",
    ),
    (
        "veo/veo-3.1-lite-generate-001",
        "Veo 3.1 Lite Generate 001",
        'v',
        "itv",
    ),
    (
        "vertexai/gemini-2.5-flash-image",
        "Gemini 2.5 Flash Image",
        'i',
        "it",
    ),
    ("vertexai/gemini-3-pro-image", "Nano Banana Pro", 'i', "it"),
    (
        "vertexai/gemini-3.1-flash-image",
        "Nano Banana 2",
        'i',
        "itv",
    ),
    (
        "vertexai/gemini-3.1-flash-lite-image",
        "Nano Banana 2 Lite",
        'i',
        "itv",
    ),
    (
        "vertexai/gemini-nano-banana-2.1",
        "Gemini Nano Banana 2.1",
        'i',
        "itv",
    ),
    ("wan/happyhorse-1.0-i2v", "HappyHorse 1.0 I2V", 'v', "it"),
    ("wan/happyhorse-1.0-r2v", "HappyHorse 1.0 R2V", 'v', "it"),
    ("wan/happyhorse-1.0-t2v", "HappyHorse 1.0 T2V", 'v', "t"),
    (
        "wan/happyhorse-1.0-video-edit",
        "HappyHorse 1.0 Video Edit",
        'v',
        "itv",
    ),
    ("wan/happyhorse-1.1-i2v", "HappyHorse 1.1 I2V", 'v', "it"),
    ("wan/happyhorse-1.1-r2v", "HappyHorse 1.1 R2V", 'v', "it"),
    ("wan/happyhorse-1.1-t2v", "HappyHorse 1.1 T2V", 'v', "t"),
    ("wan/wan2.5-i2i-preview", "Wan 2.5 I2I Preview", 'i', "it"),
    ("wan/wan2.5-i2v-preview", "Wan 2.5 I2V Preview", 'v', "ait"),
    ("wan/wan2.5-t2i-preview", "Wan 2.5 T2I Preview", 'i', "t"),
    ("wan/wan2.5-t2v-preview", "Wan 2.5 T2V Preview", 'v', "at"),
    ("wan/wan2.6-i2v", "Wan 2.6 I2V", 'v', "ait"),
    ("wan/wan2.6-r2v", "Wan 2.6 R2V", 'v', "itv"),
    ("wan/wan2.6-t2v", "Wan 2.6 T2V", 'v', "at"),
    ("wan/wan2.7-i2v", "Wan 2.7 I2V", 'v', "aitv"),
    ("wan/wan2.7-r2v", "Wan 2.7 R2V", 'v', "itv"),
    ("wan/wan2.7-t2v", "Wan 2.7 T2V", 'v', "t"),
    ("wan/wan2.7-videoedit", "Wan 2.7 Video Edit", 'v', "itv"),
    ("wan/wan3.0-video-prime", "Wan 3.0 Video Prime", 'v', "aitv"),
    ("wan/wan3.0-video", "Wan 3.0 Video", 'v', "aitv"),
    ("wavespeed/flashvsr", "WaveSpeed Flashvsr", 'v', "v"),
    ("wavespeed/seedvr2", "WaveSpeed Seedvr 2", 'i', "i"),
    (
        "wavespeed/ultimate-image-upscaler",
        "WaveSpeed Ultimate Image Upscaler",
        'i',
        "i",
    ),
    (
        "xai/grok-imagine-image-2.0",
        "Grok Imagine Image 2.0",
        'i',
        "t",
    ),
    (
        "xai/grok-imagine-image-pro",
        "Grok Imagine Image Pro",
        'i',
        "t",
    ),
    (
        "xai/grok-imagine-image-quality",
        "Grok Imagine Image Quality",
        'i',
        "t",
    ),
    ("xai/grok-imagine-image", "Grok Imagine Image", 'i', "t"),
    (
        "xai/grok-imagine-video-1.5-lite",
        "Grok Imagine Video 1.5 Lite",
        'v',
        "it",
    ),
    (
        "xai/grok-imagine-video-1.5-preview",
        "Grok Imagine Video 1.5 Preview",
        'v',
        "it",
    ),
    (
        "xai/grok-imagine-video-1.5",
        "Grok Imagine Video 1.5",
        'v',
        "it",
    ),
    ("xai/grok-imagine-video", "Grok Imagine Video", 'v', "it"),
];

/// Newest flagship of each family, tagged `featured` in the picker.
const FEATURED: &[&str] = &[
    "byteplus/dreamina-seedance-2-5-260628",
    "bfl/flux-3-image",
    "bfl/flux-3-video",
    "vertexai/gemini-3.1-flash-image",
    "vertexai/gemini-3-pro-image",
    "veo/veo-3.1-generate-001",
    "kling/kling-v3",
    "kling/kling-v3-omni",
    "ltx/ltx-2-5-pro",
    "wan/wan3.0-video",
    "openai/gpt-image-2.5-sunburst",
    "xai/grok-imagine-video-1.5",
    "xai/grok-imagine-image-2.0",
    "qwen/qwen-image-3.0-pro",
    "byteplus/seedream-5-0-pro-260628",
    "ideogram/ideogram-4-5",
    "minimax/minimax-h3",
    "luma_2/ray-3.2",
    "runway/aleph2",
    "recraft/recraftv4_1_pro",
    "krea/krea-3",
];

fn vendor_for(id: &str) -> Option<&'static str> {
    let (provider, model) = id.split_once('/').unwrap_or(("", id));
    let by_model: &[(&str, &str)] = &[
        ("nano-banana", "Google"),
        ("gpt-image", "OpenAI"),
        ("seedance", "ByteDance"),
        ("seedvr", "ByteDance"),
        ("flux", "Black Forest Labs"),
        ("kling", "Kuaishou"),
        ("wan-3", "Alibaba"),
        ("crystal", "Clarity AI"),
    ];
    if let Some((_, v)) = by_model.iter().find(|(k, _)| model.contains(k)) {
        return Some(v);
    }
    Some(match provider {
        "bfl" => "Black Forest Labs",
        "byteplus" => "ByteDance",
        "kling" => "Kuaishou",
        "ltx" => "Lightricks",
        "wan" | "qwen" => "Alibaba",
        "veo" | "vertexai" => "Google",
        "openai" => "OpenAI",
        "xai" => "xAI",
        "runway" => "Runway",
        "luma" | "luma_2" => "Luma",
        "minimax" => "MiniMax",
        "recraft" => "Recraft",
        "ideogram" => "Ideogram",
        "krea" => "Krea",
        "pika" => "Pika",
        "moonvalley" => "Moonvalley",
        "bria" => "Bria",
        "elevenlabs" => "ElevenLabs",
        "higgsfield" => "Higgsfield",
        "beeble" => "Beeble",
        "freepik" => "Freepik",
        "heygen" => "HeyGen",
        "pruna" => "Pruna",
        "synclabs" => "Sync Labs",
        "veed" => "VEED",
        _ => return None,
    })
}

fn kind_from(c: char) -> MediaKind {
    match c {
        'v' => MediaKind::Video,
        'a' => MediaKind::Audio,
        _ => MediaKind::Image,
    }
}

/// Tasks from the output kind, the accepted input kinds and the model name.
fn tasks_for(id: &str, output: MediaKind, inputs: &[MediaKind], text: bool) -> Vec<Task> {
    let lower = id.to_ascii_lowercase();
    let upscale = [
        "upscal",
        "increase-resolution",
        "seedvr",
        "flashvsr",
        "crystal",
        "precision",
    ]
    .iter()
    .any(|k| lower.contains(k));
    let mut tasks = Vec::new();
    match output {
        MediaKind::Image if upscale => tasks.push(Task::Upscale),
        MediaKind::Image => {
            if text {
                tasks.push(Task::TextToImage);
            }
            if inputs.contains(&MediaKind::Image) {
                tasks.push(Task::ImageEdit);
            }
        }
        MediaKind::Video if upscale => tasks.push(Task::Upscale),
        MediaKind::Video
            if lower.contains("lip")
                || (inputs.contains(&MediaKind::Audio) && lower.contains("avatar")) =>
        {
            tasks.push(Task::LipSync)
        }
        MediaKind::Video => {
            if text {
                tasks.push(Task::TextToVideo);
            }
            if inputs.contains(&MediaKind::Image) {
                tasks.push(Task::ImageToVideo);
            }
            if inputs.contains(&MediaKind::Video) {
                tasks.push(Task::VideoToVideo);
            }
        }
        MediaKind::Audio if text => tasks.push(Task::TextToAudio),
        MediaKind::Lora => tasks.push(Task::Training),
        MediaKind::Audio => {}
    }
    if tasks.is_empty() {
        tasks.push(match output {
            MediaKind::Image => Task::ImageEdit,
            MediaKind::Video => Task::VideoToVideo,
            MediaKind::Audio => Task::TextToAudio,
            MediaKind::Lora => Task::Training,
        });
    }
    tasks
}

fn base_spec(
    id: &str,
    name: &str,
    output: MediaKind,
    inputs: &[MediaKind],
    text: bool,
) -> ModelSpec {
    let mut s = ModelSpec::new(ID, id, name, output, &tasks_for(id, output, inputs, text));
    s.vendor = vendor_for(id).map(String::from);
    s.complete = false;
    if FEATURED.contains(&id) {
        s.tags.push("featured".into());
    }
    s
}

fn snapshot_specs() -> Vec<ModelSpec> {
    SNAPSHOT
        .iter()
        .map(|(id, name, out, ins)| {
            let kinds: Vec<MediaKind> = ins.chars().filter(|c| *c != 't').map(kind_from).collect();
            base_spec(id, name, kind_from(*out), &kinds, ins.contains('t'))
        })
        .collect()
}

fn modality(s: &str) -> Option<MediaKind> {
    match s {
        "image" => Some(MediaKind::Image),
        "video" => Some(MediaKind::Video),
        "audio" => Some(MediaKind::Audio),
        _ => None,
    }
}

fn strings(v: Option<&Value>) -> Vec<&str> {
    v.and_then(Value::as_array)
        .map(|a| a.iter().filter_map(Value::as_str).collect())
        .unwrap_or_default()
}

/// The output kind a schema document declares, when it is a media model Pictesa can show.
fn output_of(doc: &Value) -> Option<MediaKind> {
    let out = strings(doc.get("x-comfy-router-output-modalities"));
    let inp = strings(doc.get("x-comfy-router-input-modalities"));
    if out.contains(&"text") && out.contains(&"audio") && inp.contains(&"audio") {
        return None; // omni chat models
    }
    if out.contains(&"video") {
        Some(MediaKind::Video)
    } else if out.contains(&"image") {
        Some(MediaKind::Image)
    } else if out.contains(&"audio") {
        Some(MediaKind::Audio)
    } else {
        None
    }
}

// ---------------------------------------------------------------------------------------------
// Reading a model's input schema

/// A JSON location inside the request body: object keys and array indices.
type Path = Vec<Value>;

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "snake_case")]
pub(crate) enum Dialect {
    /// Fields at the top level or one container deep (`input`, `parameters`, `instances[0]`).
    #[default]
    Flat,
    /// BytePlus and MiniMax: a `content` array of text and media items with roles.
    Content,
    /// Gemini: `contents[0].parts` with text and inline data.
    Gemini,
    /// DashScope image models: `input.messages[0].content` with text and image items.
    Messages,
}

#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "snake_case")]
pub(crate) enum Encoding {
    /// A public URL, else a data URI.
    #[default]
    Url,
    /// Bare base64.
    Base64,
    /// `{"bytesBase64Encoded": .., "mimeType": ..}` (Vertex / Veo).
    Vertex,
    /// `{<key>: url}` plus constant fields.
    Object,
    /// An item of a BytePlus/MiniMax `content` array.
    Content,
    /// A `{"type": .., "url": ..}` item of a DashScope `media` array.
    Media,
    /// A Gemini `inlineData` part.
    Gemini,
    /// A DashScope message item `{"image": ..}`.
    Message,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub(crate) struct InputBind {
    pub role: InputRole,
    pub kind: MediaKind,
    pub path: Path,
    #[serde(default)]
    pub enc: Encoding,
    /// Sends a list even for one file.
    #[serde(default)]
    pub array: bool,
    /// Extra single-value fields named `input_image_2`, `input_image_3`, ... after the first.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub siblings: Vec<Path>,
    /// Property holding the URL for [`Encoding::Object`].
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub key: Option<String>,
    /// Constant fields added to each item (`{"type": "image_url"}`).
    #[serde(default, skip_serializing_if = "Map::is_empty")]
    pub extra: Map<String, Value>,
    /// Role or type tag of [`Encoding::Content`] and [`Encoding::Media`] items.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tag: Option<String>,
    /// Adds to a list other roles also fill (FLUX 3 Video `keyframes`: start, then end).
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub append: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub(crate) struct ParamBind {
    pub key: String,
    pub path: Path,
    /// The schema takes numbers as strings (`"5"`): Pictesa shows numbers and sends text.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub stringify: bool,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
pub(crate) struct Bindings {
    #[serde(default)]
    pub dialect: Dialect,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub prompt: Option<Path>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub negative: Option<Path>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub count: Option<Path>,
    #[serde(default)]
    pub params: Vec<ParamBind>,
    #[serde(default)]
    pub inputs: Vec<InputBind>,
    /// Constants always sent.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub fixed: Vec<(Path, Value)>,
    /// A `mode` field set from the inputs: `i2v` with images, `v2v` with a video, else `t2v`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mode: Option<Path>,
}

#[derive(Debug, Clone, Default)]
pub(crate) struct ReadModel {
    pub params: Vec<ParamSpec>,
    pub inputs: Vec<InputSlot>,
    pub max_outputs: u32,
    pub bindings: Bindings,
    /// Required top-level fields Pictesa cannot fill; such a model is listed as unavailable.
    pub unmapped: Vec<String>,
}

fn resolve(v: &Value, defs: Option<&Value>) -> Value {
    let mut cur = v.clone();
    for _ in 0..10 {
        let Some(r) = cur.get("$ref").and_then(Value::as_str) else {
            break;
        };
        match defs.and_then(|d| d.get(r.rsplit('/').next().unwrap_or(r))) {
            Some(t) => cur = t.clone(),
            None => break,
        }
    }
    if let Some(parts) = cur.get("allOf").and_then(Value::as_array).cloned() {
        let mut merged = cur.clone();
        if let Some(m) = merged.as_object_mut() {
            m.remove("allOf");
        }
        for p in parts {
            let p = resolve(&p, defs);
            if let (Some(m), Some(po)) = (merged.as_object_mut(), p.as_object()) {
                for (k, val) in po {
                    if k == "properties" {
                        let props = m.entry("properties").or_insert_with(|| json!({}));
                        if let (Some(dst), Some(src)) = (props.as_object_mut(), val.as_object()) {
                            for (pk, pv) in src {
                                dst.entry(pk.clone()).or_insert(pv.clone());
                            }
                        }
                    } else if k == "required" {
                        let req = m.entry("required").or_insert_with(|| json!([]));
                        if let (Some(dst), Some(src)) = (req.as_array_mut(), val.as_array()) {
                            dst.extend(src.iter().cloned());
                        }
                    } else {
                        m.entry(k.clone()).or_insert(val.clone());
                    }
                }
            }
        }
        cur = merged;
    }
    cur
}

/// The non-null variants of an `anyOf`/`oneOf`, or the schema itself.
fn variants(v: &Value, defs: Option<&Value>) -> Vec<Value> {
    match v
        .get("anyOf")
        .or_else(|| v.get("oneOf"))
        .and_then(Value::as_array)
    {
        Some(list) => list
            .iter()
            .map(|x| {
                let mut r = resolve(x, defs);
                // Keep the parent's description on each variant.
                if let (Some(m), Some(d)) = (r.as_object_mut(), v.get("description")) {
                    m.entry("description").or_insert(d.clone());
                }
                r
            })
            .filter(|x| x.get("type").and_then(Value::as_str) != Some("null"))
            .collect(),
        None => vec![v.clone()],
    }
}

fn ty(v: &Value) -> &str {
    match v.get("type") {
        Some(Value::String(s)) => s,
        Some(Value::Array(a)) => a
            .iter()
            .filter_map(Value::as_str)
            .find(|t| *t != "null")
            .unwrap_or(""),
        _ if v.get("properties").is_some() => "object",
        _ => "",
    }
}

const PROMPT_NAMES: &[&str] = &[
    "prompt",
    "promptText",
    "positivePrompt",
    "prompt_text",
    "text_prompt",
    "instruction",
    "text",
];
const NEGATIVE_NAMES: &[&str] = &["negative_prompt", "negativePrompt"];
const COUNT_NAMES: &[&str] = &[
    "n",
    "num_images",
    "numberResults",
    "sampleCount",
    "num_outputs",
    "number_of_images",
    "num_videos",
    "batch_size",
];
const SKIP: &[&str] = &[
    "model",
    "model_id",
    "model_name",
    "callback_url",
    "callbackUrl",
    "webhook_url",
    "webhookUrl",
    "webhook_secret",
    "external_task_id",
    "taskUUID",
    "taskType",
    "deliveryMethod",
    "includeCost",
    "response_format",
    "stream",
    "partial_images",
    "sync",
    "sync_mode",
    "idempotency_key",
    "execution_expires_after",
    "user",
    "enable_base64_output",
    "storageUri",
    "uploadImagesToStorage",
    "safetySettings",
    "tools",
    "systemInstruction",
    "previous_interaction_id",
    "contentModeration",
    "enable_safety_checker",
    "prompt_content_moderation",
    "visual_input_content_moderation",
    "visual_output_content_moderation",
    "outputFileName",
    "projectId",
    "service_tier",
    "draft_task",
    "return_last_frame",
    "draft",
    "draft_cache",
    "template",
    "multi_prompt",
    "voice_list",
    "element_list",
    "pronunciation_dictionary_locators",
    "next_request_ids",
    "previous_request_ids",
    "next_text",
    "previous_text",
    "structured_prompt",
    "json_prompt",
    "color_palette",
    "style_codes",
    "moodboards",
    "styles",
    "camera_control",
    "watermark_info",
    "output_compression",
    "segments",
    "dubParams",
    "source",
    "safety_tolerance",
];
/// Containers whose fields are listed as if they were top-level.
const CONTAINERS: &[&str] = &[
    "input",
    "parameters",
    "instances",
    "inference_params",
    "settings",
    "options",
];

/// Schema field name to the shared Pictesa key, when one fits.
fn shared_key(name: &str, t: &str) -> Option<&'static str> {
    Some(match name {
        "aspect_ratio" | "aspectRatio" | "ratio" => keys::ASPECT_RATIO,
        "resolution" => keys::RESOLUTION,
        "seed" | "noise_seed" | "random_seed" => keys::SEED,
        "duration" | "durationSeconds" | "duration_s" | "duration_seconds" => keys::DURATION,
        "fps" | "frame_rate" | "framesPerSecond" => keys::FPS,
        "guidance_scale" | "cfg_scale" | "guidance" | "cfg" => keys::GUIDANCE,
        "steps" | "num_inference_steps" | "steps_num" | "sample_steps" => keys::STEPS,
        "output_format" | "outputFormat" => keys::OUTPUT_FORMAT,
        "generate_audio" | "generateAudio" | "audio" if t == "boolean" => keys::GENERATE_AUDIO,
        "enhance_prompt" | "enhancePrompt" | "prompt_extend" | "prompt_upsampling"
            if t == "boolean" =>
        {
            keys::ENHANCE_PROMPT
        }
        "width" => keys::WIDTH,
        "height" => keys::HEIGHT,
        "quality" => keys::QUALITY,
        "style" => keys::STYLE,
        "loop" if t == "boolean" => keys::LOOP,
        "scale_factor" | "upscale_factor" => keys::SCALE,
        "strength" => keys::STRENGTH,
        _ => return None,
    })
}

const MAIN_KEYS: &[&str] = &[
    keys::ASPECT_RATIO,
    keys::RESOLUTION,
    keys::DURATION,
    keys::GENERATE_AUDIO,
    keys::QUALITY,
    keys::STYLE,
    keys::SCALE,
];

fn title(name: &str) -> String {
    let mut out = String::new();
    let mut prev_lower = false;
    for c in name.chars() {
        if c == '_' || c == '-' {
            out.push(' ');
            prev_lower = false;
            continue;
        }
        if c.is_ascii_uppercase() && prev_lower {
            out.push(' ');
        }
        prev_lower = c.is_ascii_lowercase();
        out.push(if out.is_empty() {
            c.to_ascii_uppercase()
        } else {
            c.to_ascii_lowercase()
        });
    }
    out
}

/// A Pictesa parameter for one scalar field.
fn param_for(name: &str, schema: &Value) -> Option<(ParamSpec, bool)> {
    let t = ty(schema);
    let key = shared_key(name, t).unwrap_or(name);
    let label = match key {
        k if k == keys::ASPECT_RATIO => "Aspect ratio".to_string(),
        k if k == keys::DURATION => "Duration".to_string(),
        k if k == keys::GENERATE_AUDIO => "Sound".to_string(),
        _ => title(name),
    };
    let help = schema
        .get("description")
        .and_then(Value::as_str)
        .map(|d| pictesa_core::error::truncate(d.trim(), 220));
    let default = schema.get("default").cloned().filter(|d| !d.is_null());
    let mut stringify = false;
    let kind = if let Some(values) = schema.get("enum").and_then(Value::as_array) {
        let numeric = key == keys::DURATION
            && values
                .iter()
                .all(|v| v.as_str().is_some_and(|s| s.parse::<f64>().is_ok()));
        stringify = numeric;
        let options: Vec<EnumOption> = values
            .iter()
            .filter(|v| !v.is_null())
            .map(|v| {
                let value = if numeric {
                    v.as_str()
                        .and_then(|s| s.parse::<i64>().ok())
                        .map(Value::from)
                        .unwrap_or(v.clone())
                } else {
                    v.clone()
                };
                let label = match (&value, key) {
                    (Value::Number(n), k) if k == keys::DURATION => format!("{n}s"),
                    (Value::String(s), _) => s.clone(),
                    (other, _) => other.to_string(),
                };
                EnumOption { value, label }
            })
            .collect();
        if options.is_empty() {
            return None;
        }
        ParamKind::Enum { options }
    } else {
        match t {
            "integer" if key == keys::SEED => ParamKind::Seed,
            "integer" => ParamKind::Int {
                min: schema.get("minimum").and_then(Value::as_i64).unwrap_or(0),
                max: schema
                    .get("maximum")
                    .and_then(Value::as_i64)
                    .unwrap_or(i32::MAX as i64),
                step: 1,
            },
            "number" => ParamKind::Float {
                min: schema.get("minimum").and_then(Value::as_f64).unwrap_or(0.0),
                max: schema
                    .get("maximum")
                    .and_then(Value::as_f64)
                    .unwrap_or(1_000_000.0),
                step: 0.01,
            },
            "boolean" => ParamKind::Bool,
            "string" => ParamKind::Text {
                multiline: false,
                max_len: schema
                    .get("maxLength")
                    .and_then(Value::as_u64)
                    .map(|n| n as u32),
            },
            _ => return None,
        }
    };
    let default = match (&default, stringify) {
        (Some(Value::String(s)), true) => s.parse::<i64>().ok().map(Value::from),
        _ => default,
    };
    let advanced = !MAIN_KEYS.contains(&key) && !matches!(name, "size" | "mode");
    Some((
        ParamSpec {
            key: key.to_string(),
            label,
            kind,
            default,
            advanced,
            help,
        },
        stringify,
    ))
}

/// Field names that hold media, and the kind they hold.
fn media_kind(name: &str) -> Option<MediaKind> {
    let n = name.to_ascii_lowercase();
    let has = |w: &[&str]| w.iter().any(|x| n.contains(x));
    if has(&["video", "clip"]) {
        Some(MediaKind::Video)
    } else if has(&["audio", "sound_file", "speech", "voice_url"]) {
        Some(MediaKind::Audio)
    } else if has(&[
        "image",
        "img",
        "frame",
        "mask",
        "photo",
        "garment",
        "person",
        "picture",
        "source_uri",
        "reference_uri",
    ]) {
        Some(MediaKind::Image)
    } else {
        None
    }
}

fn role_for(name: &str, kind: MediaKind, output: MediaKind) -> InputRole {
    let n = name.to_ascii_lowercase();
    let has = |w: &[&str]| w.iter().any(|x| n.contains(x));
    match kind {
        MediaKind::Video => InputRole::Video,
        MediaKind::Audio => InputRole::Audio,
        MediaKind::Image if has(&["mask"]) => InputRole::Mask,
        MediaKind::Image if has(&["last", "end_", "tail", "lastframe"]) => InputRole::EndFrame,
        MediaKind::Image
            if output == MediaKind::Video
                && !has(&[
                    "reference",
                    "ref",
                    "style",
                    "subject",
                    "element",
                    "_urls",
                    "images",
                    "list",
                ])
                && has(&["first", "start", "img_url", "image", "frame", "init"]) =>
        {
            InputRole::StartFrame
        }
        MediaKind::Image | MediaKind::Lora => InputRole::Reference,
    }
}

fn wants_base64(schema: &Value) -> bool {
    let d = schema
        .get("description")
        .and_then(Value::as_str)
        .unwrap_or("")
        .to_ascii_lowercase();
    schema.get("format").and_then(Value::as_str) == Some("byte")
        || (d.contains("base64") && !d.contains("url") && !d.contains("uri"))
}

fn numbered_sibling(name: &str) -> Option<(&str, u32)> {
    let (base, n) = name.rsplit_once('_')?;
    let n: u32 = n.parse().ok()?;
    (n >= 2 && media_kind(base).is_some()).then_some((base, n))
}

struct Reader<'a> {
    defs: Option<&'a Value>,
    output: MediaKind,
    accepts: Vec<MediaKind>,
    out: ReadModel,
    slots: Vec<(InputSlot, InputBind)>,
}

impl Reader<'_> {
    fn add_input(
        &mut self,
        role: InputRole,
        kind: MediaKind,
        max: u32,
        required: bool,
        label: String,
        bind: InputBind,
    ) {
        if !self.accepts.contains(&kind) {
            return;
        }
        // One slot per role: a second field with the same role becomes a sibling.
        if let Some((slot, b)) = self.slots.iter_mut().find(|(s, _)| s.role == role) {
            if b.enc == bind.enc && !bind.array && !b.array {
                b.siblings.push(bind.path);
                slot.max += 1;
            }
            return;
        }
        let mut slot = InputSlot::new(role, kind, max.max(1));
        slot.min = u32::from(required);
        slot.label = Some(label);
        self.slots.push((slot, bind));
    }

    /// Classifies one field. Returns false when it is not media.
    fn media(&mut self, name: &str, schema: &Value, path: &Path, required: bool) -> bool {
        let vars = variants(schema, self.defs);
        // Veo-style inline image objects (their `oneOf` only lists required-field alternatives).
        if schema.pointer("/properties/bytesBase64Encoded").is_some()
            || vars
                .iter()
                .any(|v| v.pointer("/properties/bytesBase64Encoded").is_some())
        {
            let kind = media_kind(name).unwrap_or(MediaKind::Image);
            let role = role_for(name, kind, self.output);
            let bind = InputBind {
                role,
                kind,
                path: path.clone(),
                enc: Encoding::Vertex,
                array: false,
                siblings: vec![],
                key: None,
                extra: Map::new(),
                tag: None,
                append: false,
            };
            self.add_input(role, kind, 1, required, title(name), bind);
            return true;
        }
        let Some(kind) = media_kind(name) else {
            return false;
        };
        let string_var = vars.iter().find(|v| ty(v) == "string");
        let array_var = vars.iter().find(|v| ty(v) == "array");
        let object_var = vars.iter().find(|v| ty(v) == "object");
        let role = role_for(name, kind, self.output);
        let single = matches!(
            role,
            InputRole::StartFrame | InputRole::EndFrame | InputRole::Mask
        );
        let array_var = if single && string_var.is_some() {
            None
        } else {
            array_var
        };
        if let Some(a) = array_var {
            let items = resolve(a.get("items").unwrap_or(&Value::Null), self.defs);
            let max = a
                .get("maxItems")
                .and_then(Value::as_u64)
                .unwrap_or(4)
                .clamp(1, 16) as u32;
            let (enc, key) = match ty(&items) {
                "string" | "" => (
                    if wants_base64(&items) && wants_base64(a) {
                        Encoding::Base64
                    } else {
                        Encoding::Url
                    },
                    None,
                ),
                "object" => match url_key(&items) {
                    Some(k) => (Encoding::Object, Some(k)),
                    None => return false,
                },
                _ => return false,
            };
            let extra = if enc == Encoding::Object {
                constant_fields(&items, self.defs)
            } else {
                Map::new()
            };
            let bind = InputBind {
                role,
                kind,
                path: path.clone(),
                enc,
                array: true,
                siblings: vec![],
                key,
                extra,
                tag: None,
                append: false,
            };
            self.add_input(role, kind, max, required, title(name), bind);
            return true;
        }
        if let Some(s) = string_var {
            let enc = if wants_base64(s) {
                Encoding::Base64
            } else {
                Encoding::Url
            };
            let bind = InputBind {
                role,
                kind,
                path: path.clone(),
                enc,
                array: false,
                siblings: vec![],
                key: None,
                extra: Map::new(),
                tag: None,
                append: false,
            };
            self.add_input(role, kind, 1, required, title(name), bind);
            return true;
        }
        if let Some(o) = object_var {
            if let Some(k) = url_key(o) {
                let extra = constant_fields(o, self.defs);
                let bind = InputBind {
                    role,
                    kind,
                    path: path.clone(),
                    enc: Encoding::Object,
                    array: false,
                    siblings: vec![],
                    key: Some(k),
                    extra,
                    tag: None,
                    append: false,
                };
                self.add_input(role, kind, 1, required, title(name), bind);
                return true;
            }
        }
        false
    }

    fn field(&mut self, name: &str, schema: &Value, path: Path, required: bool) {
        let schema = resolve(schema, self.defs);
        if SKIP.contains(&name) {
            return;
        }
        let is_string = variants(&schema, self.defs)
            .iter()
            .any(|v| ty(v) == "string");
        let desc = schema
            .get("description")
            .and_then(Value::as_str)
            .unwrap_or("");
        if name == "mode" && is_string && desc.contains("t2v") && desc.contains("i2v") {
            self.out.bindings.mode = Some(path);
            return;
        }
        if name == "keyframes" && is_string {
            // One image starts the video, a second one ends it.
            for role in [InputRole::StartFrame, InputRole::EndFrame] {
                let bind = InputBind {
                    role,
                    kind: MediaKind::Image,
                    path: path.clone(),
                    enc: Encoding::Url,
                    array: true,
                    siblings: vec![],
                    key: None,
                    extra: Map::new(),
                    tag: None,
                    append: true,
                };
                let label = if role == InputRole::StartFrame {
                    "First frame"
                } else {
                    "Last frame"
                };
                self.add_input(role, MediaKind::Image, 1, false, label.into(), bind);
            }
            return;
        }
        let b = &mut self.out.bindings;
        if b.prompt.is_none() && PROMPT_NAMES.contains(&name) && is_string {
            b.prompt = Some(path);
            return;
        }
        if NEGATIVE_NAMES.contains(&name) {
            b.negative = Some(path);
            return;
        }
        if COUNT_NAMES.contains(&name) && ty(&schema) == "integer" {
            self.out.max_outputs = schema
                .get("maximum")
                .and_then(Value::as_u64)
                .unwrap_or(4)
                .clamp(1, 8) as u32;
            self.out.bindings.count = Some(path);
            return;
        }
        if let Some((base, _)) = numbered_sibling(name) {
            // `input_image_2` joins the slot of `input_image`.
            if let Some((slot, b)) = self
                .slots
                .iter_mut()
                .find(|(_, b)| b.path.last().and_then(Value::as_str) == Some(base))
            {
                b.siblings.push(path);
                slot.max += 1;
            }
            return;
        }
        if self.media(name, &schema, &path, required) {
            return;
        }
        let scalar = variants(&schema, self.defs).into_iter().find(|v| {
            matches!(ty(v), "string" | "integer" | "number" | "boolean") || v.get("enum").is_some()
        });
        if let Some(s) = scalar {
            let merged = merge_desc(&schema, s);
            if let Some((spec, stringify)) = param_for(name, &merged) {
                if self.out.params.iter().any(|p| p.key == spec.key) {
                    return;
                }
                self.out.bindings.params.push(ParamBind {
                    key: spec.key.clone(),
                    path,
                    stringify,
                });
                self.out.params.push(spec);
            }
        }
    }
}

fn merge_desc(parent: &Value, mut v: Value) -> Value {
    if let Some(m) = v.as_object_mut() {
        for k in ["description", "default", "enum"] {
            if let Some(x) = parent.get(k) {
                m.entry(k).or_insert(x.clone());
            }
        }
    }
    v
}

/// The string property of an item object that takes the file URL.
fn url_key(obj: &Value) -> Option<String> {
    let props = obj.get("properties")?.as_object()?;
    ["url", "image_url", "uri", "image", "file_url"]
        .iter()
        .find(|k| {
            props
                .get(**k)
                .is_some_and(|p| matches!(ty(p), "string" | ""))
        })
        .map(|k| k.to_string())
}

/// Single-value enums of an item schema (`{"type": "image_url"}`), sent with every item.
fn constant_fields(obj: &Value, defs: Option<&Value>) -> Map<String, Value> {
    let mut out = Map::new();
    for (k, v) in obj
        .get("properties")
        .and_then(Value::as_object)
        .into_iter()
        .flatten()
    {
        let v = resolve(v, defs);
        if let Some(e) = v
            .get("enum")
            .and_then(Value::as_array)
            .filter(|e| e.len() == 1)
        {
            out.insert(k.clone(), e[0].clone());
        } else if let Some(c) = v.get("const") {
            out.insert(k.clone(), c.clone());
        }
    }
    out
}

/// The request body schema of a Router model document.
fn request_schema(doc: &Value) -> Option<&Value> {
    doc.get("paths")?
        .as_object()?
        .values()
        .find_map(|p| p.pointer("/post/requestBody/content/application~1json/schema"))
}

/// Reads one model document into params, inputs and body bindings.
pub(crate) fn read_schema(doc: &Value, id: &str, output: MediaKind) -> ReadModel {
    let defs = doc.pointer("/components/schemas");
    let accepts: Vec<MediaKind> = strings(doc.get("x-comfy-router-input-modalities"))
        .into_iter()
        .filter_map(modality)
        .collect();
    let mut r = Reader {
        defs,
        output,
        accepts,
        out: ReadModel {
            max_outputs: 1,
            ..Default::default()
        },
        slots: Vec::new(),
    };
    let root = request_schema(doc)
        .map(|s| resolve(s, defs))
        .unwrap_or(Value::Null);
    let props = root
        .get("properties")
        .and_then(Value::as_object)
        .cloned()
        .unwrap_or_default();
    let required = strings(root.get("required"))
        .into_iter()
        .map(String::from)
        .collect::<Vec<_>>();
    if props.is_empty() {
        // Unauthored schema: Router forwards any object. Nearly every such model takes `prompt`.
        r.out.bindings.prompt = Some(vec![json!("prompt")]);
        return r.out;
    }
    let item_props = |name: &str| -> Option<Map<String, Value>> {
        let a = resolve(props.get(name)?, defs);
        let items = resolve(a.get("items")?, defs);
        items.get("properties").and_then(Value::as_object).cloned()
    };
    let mut skip_top: Vec<&str> = Vec::new();
    if let Some(content) = item_props("content").filter(|p| p.contains_key("type")) {
        r.out.bindings.dialect = Dialect::Content;
        r.out.bindings.prompt = Some(vec![json!("content")]);
        skip_top.push("content");
        let roles: Vec<String> = content
            .get("role")
            .map(|v| resolve(v, defs))
            .map(|v| {
                strings(v.get("enum"))
                    .into_iter()
                    .map(String::from)
                    .collect()
            })
            .unwrap_or_default();
        let described = content
            .get("role")
            .and_then(|v| v.get("description"))
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_string();
        let has_role = |name: &str| roles.iter().any(|x| x == name) || described.contains(name);
        for (role, kind, tag, max, label) in [
            (
                InputRole::StartFrame,
                MediaKind::Image,
                "first_frame",
                1,
                "First frame",
            ),
            (
                InputRole::EndFrame,
                MediaKind::Image,
                "last_frame",
                1,
                "Last frame",
            ),
            (
                InputRole::Reference,
                MediaKind::Image,
                "reference_image",
                9,
                "Reference images",
            ),
            (
                InputRole::Video,
                MediaKind::Video,
                "reference_video",
                3,
                "Reference videos",
            ),
            (
                InputRole::Audio,
                MediaKind::Audio,
                "reference_audio",
                3,
                "Reference audio",
            ),
        ] {
            if has_role(tag) {
                let bind = InputBind {
                    role,
                    kind,
                    path: vec![json!("content")],
                    enc: Encoding::Content,
                    array: true,
                    siblings: vec![],
                    key: None,
                    extra: Map::new(),
                    tag: Some(tag.into()),
                    append: false,
                };
                r.add_input(role, kind, max, false, label.into(), bind);
            }
        }
    } else if props.contains_key("contents") {
        r.out.bindings.dialect = Dialect::Gemini;
        r.out.bindings.prompt = Some(vec![json!("contents")]);
        skip_top.extend(["contents", "generationConfig"]);
        let bind = InputBind {
            role: InputRole::Reference,
            kind: MediaKind::Image,
            path: vec![json!("contents")],
            enc: Encoding::Gemini,
            array: true,
            siblings: vec![],
            key: None,
            extra: Map::new(),
            tag: None,
            append: false,
        };
        r.add_input(
            InputRole::Reference,
            MediaKind::Image,
            14,
            false,
            "Reference images".into(),
            bind,
        );
        let mut ratio = ParamSpec::choices(
            keys::ASPECT_RATIO,
            "Aspect ratio",
            &[
                "1:1", "2:3", "3:2", "3:4", "4:3", "4:5", "5:4", "9:16", "16:9", "21:9",
            ],
        );
        ratio.help = Some("Leave unset to let the model choose.".into());
        r.out.bindings.params.push(ParamBind {
            key: keys::ASPECT_RATIO.into(),
            path: vec![
                json!("generationConfig"),
                json!("imageConfig"),
                json!("aspectRatio"),
            ],
            stringify: false,
        });
        r.out.params.push(ratio);
        r.out.bindings.params.push(ParamBind {
            key: keys::SEED.into(),
            path: vec![json!("generationConfig"), json!("seed")],
            stringify: false,
        });
        r.out.params.push(ParamSpec::seed());
        r.out.bindings.fixed.push((
            vec![json!("generationConfig"), json!("responseModalities")],
            json!(["TEXT", "IMAGE"]),
        ));
    } else if resolve(props.get("input").unwrap_or(&Value::Null), defs)
        .pointer("/properties/messages")
        .is_some()
    {
        r.out.bindings.dialect = Dialect::Messages;
        r.out.bindings.prompt = Some(vec![json!("input"), json!("messages")]);
        let bind = InputBind {
            role: InputRole::Reference,
            kind: MediaKind::Image,
            path: vec![json!("input"), json!("messages")],
            enc: Encoding::Message,
            array: true,
            siblings: vec![],
            key: None,
            extra: Map::new(),
            tag: None,
            append: false,
        };
        r.add_input(
            InputRole::Reference,
            MediaKind::Image,
            3,
            false,
            "Images to edit".into(),
            bind,
        );
        skip_top.push("input");
    }
    let mut fields: Vec<(String, Value, Path, bool)> = Vec::new();
    for (name, schema) in &props {
        if skip_top.contains(&name.as_str()) || SKIP.contains(&name.as_str()) {
            continue;
        }
        let resolved = resolve(schema, defs);
        let container = CONTAINERS.contains(&name.as_str());
        if container && ty(&resolved) == "object" && resolved.get("properties").is_some() {
            let inner_req = strings(resolved.get("required"))
                .into_iter()
                .map(String::from)
                .collect::<Vec<_>>();
            for (k, v) in resolved["properties"].as_object().into_iter().flatten() {
                fields.push((
                    k.clone(),
                    v.clone(),
                    vec![json!(name), json!(k)],
                    inner_req.contains(k),
                ));
            }
        } else if name == "instances" && ty(&resolved) == "array" {
            let items = resolve(resolved.get("items").unwrap_or(&Value::Null), defs);
            let inner_req = strings(items.get("required"))
                .into_iter()
                .map(String::from)
                .collect::<Vec<_>>();
            for (k, v) in items
                .get("properties")
                .and_then(Value::as_object)
                .into_iter()
                .flatten()
            {
                fields.push((
                    k.clone(),
                    v.clone(),
                    vec![json!(name), json!(0), json!(k)],
                    inner_req.contains(k),
                ));
            }
        } else {
            fields.push((
                name.clone(),
                schema.clone(),
                vec![json!(name)],
                required.contains(name),
            ));
        }
    }
    // Prompt-like names first, so `prompt` wins over `text`; then the main inputs before their siblings.
    fields.sort_by_key(|(n, _, _, _)| {
        let p = PROMPT_NAMES
            .iter()
            .position(|x| x == n)
            .unwrap_or(PROMPT_NAMES.len());
        (p, numbered_sibling(n).map_or(0, |(_, i)| i))
    });
    // DashScope models that take their files in `input.media` claim those roles before the older
    // `img_url`-style fields the shared schema also lists.
    if let Some((_, _, path, _)) = fields
        .iter()
        .find(|(n, _, p, _)| n == "media" && p.len() == 2)
    {
        media_array(&mut r, id, path);
    }
    for (name, schema, path, req) in fields {
        if name == "media" {
            continue;
        }
        r.field(&name, &schema, path, req);
    }
    let (slots, binds): (Vec<_>, Vec<_>) = r.slots.into_iter().unzip();
    r.out.inputs = slots;
    r.out.bindings.inputs = binds;
    let b = &r.out.bindings;
    let mut heads: Vec<&Value> = b
        .params
        .iter()
        .filter_map(|p| p.path.first())
        .chain(b.inputs.iter().filter_map(|i| i.path.first()))
        .collect();
    heads.extend(
        [&b.prompt, &b.negative, &b.count, &b.mode]
            .into_iter()
            .flatten()
            .filter_map(|p| p.first()),
    );
    heads.extend(b.fixed.iter().filter_map(|(p, _)| p.first()));
    r.out.unmapped = required
        .into_iter()
        .filter(|n| {
            !matches!(n.as_str(), "model" | "model_id" | "model_name")
                && !heads.iter().any(|h| h.as_str() == Some(n.as_str()))
        })
        .collect();
    r.out
}

/// DashScope `input.media`: typed items whose accepted types depend on the model.
fn media_array(r: &mut Reader<'_>, id: &str, path: &Path) {
    let model = id.rsplit('/').next().unwrap_or(id);
    if !(model.starts_with("wan2.7")
        || model.starts_with("wan3")
        || model.starts_with("happyhorse"))
    {
        return; // wan2.5 and wan2.6 use img_url, audio_url and reference_video_urls
    }
    let mut add = |role: InputRole, kind: MediaKind, tag: &str, max: u32, label: &str| {
        let bind = InputBind {
            role,
            kind,
            path: path.clone(),
            enc: Encoding::Media,
            array: true,
            siblings: vec![],
            key: None,
            extra: Map::new(),
            tag: Some(tag.into()),
            append: false,
        };
        r.add_input(role, kind, max, false, label.into(), bind);
    };
    if model.contains("i2v") {
        add(
            InputRole::StartFrame,
            MediaKind::Image,
            "first_frame",
            1,
            "First frame",
        );
        if model.starts_with("wan2.7") {
            add(
                InputRole::EndFrame,
                MediaKind::Image,
                "last_frame",
                1,
                "Last frame",
            );
            add(
                InputRole::Audio,
                MediaKind::Audio,
                "driving_audio",
                1,
                "Driving audio",
            );
        }
    } else if model.contains("r2v") {
        add(
            InputRole::Reference,
            MediaKind::Image,
            "reference_image",
            if model.starts_with("happyhorse") {
                9
            } else {
                5
            },
            "Reference images",
        );
        if model.starts_with("wan2.7") {
            add(
                InputRole::Video,
                MediaKind::Video,
                "reference_video",
                5,
                "Reference videos",
            );
        }
    } else if model.contains("edit") {
        add(
            InputRole::Video,
            MediaKind::Video,
            "video",
            1,
            "Video to edit",
        );
        add(
            InputRole::Reference,
            MediaKind::Image,
            "reference_image",
            4,
            "Reference images",
        );
    } else if model.starts_with("wan3") {
        add(
            InputRole::StartFrame,
            MediaKind::Image,
            "first_frame",
            1,
            "First frame",
        );
        add(
            InputRole::EndFrame,
            MediaKind::Image,
            "last_frame",
            1,
            "Last frame",
        );
        add(
            InputRole::Reference,
            MediaKind::Image,
            "reference_image",
            10,
            "Reference images",
        );
        add(
            InputRole::Video,
            MediaKind::Video,
            "reference_video",
            5,
            "Reference videos",
        );
        add(
            InputRole::Audio,
            MediaKind::Audio,
            "reference_audio",
            5,
            "Reference audio",
        );
    }
}

// ---------------------------------------------------------------------------------------------
// Building a request body

fn set_path(root: &mut Value, path: &[Value], v: Value) {
    let Some((last, parents)) = path.split_last() else {
        return;
    };
    let mut cur = root;
    for (i, seg) in parents.iter().enumerate() {
        let next_is_index = path.get(i + 1).is_some_and(Value::is_u64);
        cur = match seg {
            Value::String(k) => {
                if !cur.is_object() {
                    *cur = json!({});
                }
                cur.as_object_mut()
                    .unwrap()
                    .entry(k.clone())
                    .or_insert_with(|| if next_is_index { json!([]) } else { json!({}) })
            }
            Value::Number(n) => {
                let i = n.as_u64().unwrap_or(0) as usize;
                if !cur.is_array() {
                    *cur = json!([]);
                }
                let a = cur.as_array_mut().unwrap();
                while a.len() <= i {
                    a.push(if next_is_index { json!([]) } else { json!({}) });
                }
                &mut a[i]
            }
            _ => return,
        };
    }
    match last {
        Value::String(k) => {
            if !cur.is_object() {
                *cur = json!({});
            }
            cur.as_object_mut().unwrap().insert(k.clone(), v);
        }
        Value::Number(n) => {
            let i = n.as_u64().unwrap_or(0) as usize;
            if !cur.is_array() {
                *cur = json!([]);
            }
            let a = cur.as_array_mut().unwrap();
            while a.len() <= i {
                a.push(Value::Null);
            }
            a[i] = v;
        }
        _ => {}
    }
}

fn push_path(root: &mut Value, path: &[Value], v: Value) {
    let mut existing = get_path(root, path).cloned().unwrap_or_else(|| json!([]));
    if let Some(a) = existing.as_array_mut() {
        a.push(v);
    }
    set_path(root, path, existing);
}

fn get_path<'a>(root: &'a Value, path: &[Value]) -> Option<&'a Value> {
    path.iter().try_fold(root, |cur, seg| match seg {
        Value::String(k) => cur.get(k),
        Value::Number(n) => cur.get(n.as_u64()? as usize),
        _ => None,
    })
}

/// One input file, ready to place: a URL (public or data URI), its base64 and MIME type.
#[derive(Debug, Clone)]
pub(crate) struct Encoded {
    pub url: String,
    pub base64: String,
    pub mime: String,
}

async fn encode(input: &MediaInput) -> ProviderResult<Encoded> {
    let base64 = input.base64().await?;
    let url = match &input.url {
        Some(u) => u.clone(),
        None => format!("data:{};base64,{base64}", input.mime),
    };
    Ok(Encoded {
        url,
        base64,
        mime: input.mime.clone(),
    })
}

fn item_for(b: &InputBind, e: &Encoded) -> Value {
    match b.enc {
        Encoding::Url => json!(e.url),
        Encoding::Base64 => json!(e.base64),
        Encoding::Vertex => json!({ "bytesBase64Encoded": e.base64, "mimeType": e.mime }),
        Encoding::Object => {
            let mut m = b.extra.clone();
            m.insert(b.key.clone().unwrap_or_else(|| "url".into()), json!(e.url));
            Value::Object(m)
        }
        Encoding::Content => {
            let field = match b.kind {
                MediaKind::Image => "image_url",
                MediaKind::Video => "video_url",
                MediaKind::Audio => "audio_url",
                MediaKind::Lora => "file_url",
            };
            json!({ "type": field, field: { "url": e.url }, "role": b.tag })
        }
        Encoding::Media => json!({ "type": b.tag, "url": e.url }),
        Encoding::Gemini => json!({ "inlineData": { "mimeType": e.mime, "data": e.base64 } }),
        Encoding::Message => json!({ "image": e.url }),
    }
}

/// Builds the native request body. `files` holds the encoded inputs per role, in request order.
pub(crate) fn build_body(
    b: &Bindings,
    req: &GenerationRequest,
    files: &HashMap<InputRole, Vec<Encoded>>,
) -> ProviderResult<Value> {
    let mut body = json!({});
    let prompt = req.prompt.trim();
    match b.dialect {
        Dialect::Content => {
            if !prompt.is_empty() {
                push_path(
                    &mut body,
                    &[json!("content")],
                    json!({ "type": "text", "text": prompt }),
                );
            }
        }
        Dialect::Gemini => {
            set_path(
                &mut body,
                &[json!("contents"), json!(0), json!("role")],
                json!("user"),
            );
            push_path(
                &mut body,
                &[json!("contents"), json!(0), json!("parts")],
                json!({ "text": prompt }),
            );
        }
        Dialect::Messages => {
            set_path(
                &mut body,
                &[json!("input"), json!("messages"), json!(0), json!("role")],
                json!("user"),
            );
        }
        Dialect::Flat => {
            if let (Some(p), false) = (&b.prompt, prompt.is_empty()) {
                set_path(&mut body, p, json!(prompt));
            }
        }
    }
    if let (Some(p), Some(neg)) = (
        &b.negative,
        req.negative_prompt
            .as_deref()
            .filter(|n| !n.trim().is_empty()),
    ) {
        set_path(&mut body, p, json!(neg));
    }
    if let Some(p) = &b.count {
        set_path(&mut body, p, json!(req.num_outputs.max(1)));
    }
    for pb in &b.params {
        let Some(v) = req.params.get(&pb.key) else {
            continue;
        };
        let v = match (pb.stringify, v) {
            (true, Value::Number(n)) => json!(n.to_string()),
            _ => v.clone(),
        };
        set_path(&mut body, &pb.path, v);
    }
    for (path, v) in &b.fixed {
        set_path(&mut body, path, v.clone());
    }
    for ib in &b.inputs {
        let list = files.get(&ib.role).map(Vec::as_slice).unwrap_or_default();
        if list.is_empty() {
            continue;
        }
        match ib.enc {
            Encoding::Content | Encoding::Media => list
                .iter()
                .for_each(|e| push_path(&mut body, &ib.path, item_for(ib, e))),
            Encoding::Gemini => {
                let parts = [json!("contents"), json!(0), json!("parts")];
                list.iter()
                    .for_each(|e| push_path(&mut body, &parts, item_for(ib, e)));
            }
            Encoding::Message => {
                let content = [
                    json!("input"),
                    json!("messages"),
                    json!(0),
                    json!("content"),
                ];
                list.iter()
                    .for_each(|e| push_path(&mut body, &content, item_for(ib, e)));
            }
            _ if ib.append => list
                .iter()
                .for_each(|e| push_path(&mut body, &ib.path, item_for(ib, e))),
            _ if ib.array => set_path(
                &mut body,
                &ib.path,
                Value::Array(list.iter().map(|e| item_for(ib, e)).collect()),
            ),
            _ => {
                set_path(&mut body, &ib.path, item_for(ib, &list[0]));
                for (path, e) in ib.siblings.iter().zip(&list[1..]) {
                    set_path(&mut body, path, item_for(ib, e));
                }
            }
        }
    }
    if let Some(path) = &b.mode {
        let has = |r: InputRole| files.get(&r).is_some_and(|l| !l.is_empty());
        let mode = if has(InputRole::Video) {
            "v2v"
        } else if has(InputRole::StartFrame)
            || has(InputRole::EndFrame)
            || has(InputRole::Reference)
        {
            "i2v"
        } else {
            "t2v"
        };
        set_path(&mut body, path, json!(mode));
    }
    if b.dialect == Dialect::Messages && !prompt.is_empty() {
        push_path(
            &mut body,
            &[
                json!("input"),
                json!("messages"),
                json!(0),
                json!("content"),
            ],
            json!({ "text": prompt }),
        );
    }
    Ok(body)
}

/// A UUID derived from the job, the model and the body, so a retried submit of the same job reuses
/// its key (Router then answers with the original request instead of running it twice).
pub(crate) fn idempotency_key(job: &str, model: &str, body: &[u8]) -> String {
    let mut h = Sha256::new();
    h.update(b"pictesa-comfy-router\0");
    h.update(job.as_bytes());
    h.update(b"\0");
    h.update(model.as_bytes());
    h.update(b"\0");
    h.update(body);
    let d = h.finalize();
    let mut b = [0u8; 16];
    b.copy_from_slice(&d[..16]);
    b[6] = (b[6] & 0x0f) | 0x80; // version 8 (custom)
    b[8] = (b[8] & 0x3f) | 0x80; // RFC 9562 variant
    let hex: String = b.iter().map(|x| format!("{x:02x}")).collect();
    format!(
        "{}-{}-{}-{}-{}",
        &hex[0..8],
        &hex[8..12],
        &hex[12..16],
        &hex[16..20],
        &hex[20..32]
    )
}

// ---------------------------------------------------------------------------------------------
// Results and errors

fn mime_from_magic(b: &[u8]) -> Option<&'static str> {
    if b.starts_with(b"\x89PNG") {
        Some("image/png")
    } else if b.starts_with(&[0xFF, 0xD8, 0xFF]) {
        Some("image/jpeg")
    } else if b.len() > 12 && &b[0..4] == b"RIFF" && &b[8..12] == b"WEBP" {
        Some("image/webp")
    } else if b.len() > 12 && &b[4..8] == b"ftyp" {
        Some("video/mp4")
    } else if b.starts_with(&[0x1A, 0x45, 0xDF, 0xA3]) {
        Some("video/webm")
    } else if b.starts_with(b"ID3") || b.starts_with(&[0xFF, 0xFB]) {
        Some("audio/mpeg")
    } else if b.len() > 12 && &b[0..4] == b"RIFF" && &b[8..12] == b"WAVE" {
        Some("audio/wav")
    } else {
        None
    }
}

fn url_kind(url: &str) -> Option<MediaKind> {
    let lower = url.split('?').next().unwrap_or(url).to_ascii_lowercase();
    let ext = lower.rsplit('.').next().unwrap_or("");
    match ext {
        "png" | "jpg" | "jpeg" | "webp" | "gif" | "avif" | "svg" => Some(MediaKind::Image),
        "mp4" | "webm" | "mov" | "mkv" | "m4v" => Some(MediaKind::Video),
        "mp3" | "wav" | "ogg" | "m4a" | "flac" | "aac" => Some(MediaKind::Audio),
        _ => None,
    }
}

/// Finds result media in a native Router answer: URLs (`result.sample`, `images[].url`,
/// `content.video_url`, `gcsUri`, ...) and inline base64 (`b64_json`, `bytesBase64Encoded`,
/// Gemini `inlineData`). Echoed inputs, thumbnails and last-frame stills are skipped.
pub(crate) fn collect_outputs(v: &Value, want: MediaKind) -> Vec<Output> {
    fn walk(
        v: &Value,
        key: &str,
        want: MediaKind,
        urls: &mut Vec<String>,
        bytes: &mut Vec<(Vec<u8>, String)>,
    ) {
        let lk = key.to_ascii_lowercase();
        if [
            "input",
            "inputs",
            "request",
            "prompt",
            "references",
            "reference_images",
            "thumbnail",
            "poster",
            "cover",
            "last_frame",
            "lastframe",
            "preview",
        ]
        .iter()
        .any(|s| {
            lk == *s
                || lk.contains("last_frame")
                || lk.contains("thumbnail")
                || lk.ends_with("_input")
        }) {
            return;
        }
        match v {
            Value::String(s) if s.starts_with("http://") || s.starts_with("https://") => {
                let keyed = key.is_empty()
                    || [
                        "url", "uri", "sample", "video", "image", "audio", "file", "src", "output",
                        "result", "link", "download",
                    ]
                    .iter()
                    .any(|w| lk.contains(w));
                let kind_ok = url_kind(s).is_none_or(|k| k == want);
                if keyed && kind_ok && !urls.contains(s) {
                    urls.push(s.clone());
                }
            }
            Value::String(s)
                if s.starts_with("data:")
                    && ["url", "image", "video", "audio", "output", "data"]
                        .iter()
                        .any(|w| lk.contains(w)) =>
            {
                if let Some((meta, data)) = s.split_once(',') {
                    let mime = meta
                        .trim_start_matches("data:")
                        .split(';')
                        .next()
                        .unwrap_or("")
                        .to_string();
                    if MediaKind::from_mime(&mime) == Some(want) {
                        if let Ok(b) = base64::engine::general_purpose::STANDARD.decode(data) {
                            bytes.push((b, mime));
                        }
                    }
                }
            }
            Value::Array(items) => items.iter().for_each(|i| walk(i, key, want, urls, bytes)),
            Value::Object(map) => {
                let inline = map
                    .get("inlineData")
                    .or_else(|| map.get("inline_data"))
                    .and_then(|d| {
                        Some((
                            d.get("data")?.as_str()?,
                            d.get("mimeType")
                                .or_else(|| d.get("mime_type"))
                                .and_then(Value::as_str),
                        ))
                    });
                let b64 = map
                    .get("b64_json")
                    .or_else(|| map.get("bytesBase64Encoded"))
                    .or_else(|| map.get("base64"))
                    .and_then(Value::as_str);
                let mime_hint = map
                    .get("mimeType")
                    .or_else(|| map.get("mime_type"))
                    .or_else(|| map.get("content_type"))
                    .and_then(Value::as_str);
                if let Some((data, mime)) = inline
                    .map(|(d, m)| (d, m.or(mime_hint)))
                    .or(b64.map(|d| (d, mime_hint)))
                {
                    if let Ok(b) = base64::engine::general_purpose::STANDARD.decode(data.trim()) {
                        let fallback = match want {
                            MediaKind::Image => "image/png",
                            MediaKind::Video => "video/mp4",
                            MediaKind::Audio => "audio/mpeg",
                            MediaKind::Lora => "application/x-safetensors",
                        };
                        let mime = mime
                            .map(String::from)
                            .or_else(|| mime_from_magic(&b).map(String::from))
                            .unwrap_or_else(|| fallback.into());
                        if MediaKind::from_mime(&mime).is_none_or(|k| k == want) {
                            bytes.push((b, mime));
                        }
                    }
                }
                for (k, child) in map {
                    if matches!(
                        k.as_str(),
                        "inlineData" | "inline_data" | "b64_json" | "bytesBase64Encoded" | "base64"
                    ) {
                        continue;
                    }
                    walk(child, k, want, urls, bytes);
                }
            }
            _ => {}
        }
    }
    let (mut urls, mut bytes) = (Vec::new(), Vec::new());
    walk(v, "", want, &mut urls, &mut bytes);
    let mut out: Vec<Output> = urls.into_iter().map(Output::url).collect();
    out.extend(
        bytes
            .into_iter()
            .map(|(data, mime)| Output::Bytes { data, mime }),
    );
    out
}

fn header_str<'a>(h: &'a reqwest::header::HeaderMap, name: &str) -> Option<&'a str> {
    h.get(name).and_then(|v| v.to_str().ok())
}

fn retry_after_ms(h: &reqwest::header::HeaderMap) -> Option<u64> {
    header_str(h, "retry-after")
        .and_then(|s| s.trim().parse::<f64>().ok())
        .map(|s| (s * 1000.0) as u64)
}

/// Maps a Router failure (`X-Comfy-Error-Type` and the body's `detail`) to a Pictesa error.
pub(crate) fn router_error(
    status: u16,
    error_type: Option<&str>,
    body: &str,
    retry_after: Option<u64>,
) -> ProviderError {
    let v: Value = serde_json::from_str(body).unwrap_or(Value::Null);
    let error_type = error_type
        .or_else(|| v.get("error_type").and_then(Value::as_str))
        .unwrap_or("");
    let mut detail = match v.get("detail") {
        Some(Value::String(s)) => s.clone(),
        Some(Value::Array(items)) => items
            .iter()
            .map(|e| {
                let loc: Vec<String> = e
                    .get("loc")
                    .and_then(Value::as_array)
                    .into_iter()
                    .flatten()
                    .filter(|l| l.as_str() != Some("body"))
                    .map(|l| {
                        l.as_str()
                            .map(String::from)
                            .unwrap_or_else(|| l.to_string())
                    })
                    .collect();
                let msg = e.get("msg").and_then(Value::as_str).unwrap_or("invalid");
                if loc.is_empty() {
                    msg.to_string()
                } else {
                    format!("{}: {msg}", loc.join("."))
                }
            })
            .collect::<Vec<_>>()
            .join("; "),
        _ => pictesa_core::error::truncate(body, 300),
    };
    if let Some(up) = v.get("upstream_detail").and_then(Value::as_str) {
        detail = format!("{detail} ({up})");
    }
    if detail.trim().is_empty() {
        detail = format!("HTTP {status}");
    }
    let detail = pictesa_core::error::truncate(&detail, 600);
    match error_type {
        "insufficient_credits" => ProviderError::InsufficientCredits(format!(
            "{detail}. Add credits at https://platform.comfy.org"
        )),
        "unauthorized" | "forbidden" => ProviderError::Auth(detail),
        "rate_limited" | "concurrency_limit_exceeded" | "queue_backlog_full" => {
            ProviderError::RateLimited {
                retry_after_ms: retry_after.or(Some(5_000)),
            }
        }
        "content_policy_violation" => ProviderError::ContentPolicy(detail),
        "invalid_input" => ProviderError::InvalidInput(detail),
        "model_not_found" | "not_enabled" => ProviderError::NotSupported(detail),
        "provider_timeout"
        | "deadline_exceeded"
        | "service_unavailable"
        | "internal_error"
        | "client_disconnected" => ProviderError::Transient(detail),
        "provider_error" => ProviderError::Failed(detail),
        "cancelled" => ProviderError::Failed("cancelled".into()),
        "queue_timeout" => ProviderError::Failed(format!(
            "the request waited too long in Comfy Router's queue and was dropped: {detail}"
        )),
        _ => ProviderError::from_http(status, &detail, retry_after),
    }
}

async fn send(
    rb: reqwest::RequestBuilder,
) -> ProviderResult<(u16, reqwest::header::HeaderMap, bytes::Bytes)> {
    let resp = rb.send().await.map_err(net_error)?;
    let status = resp.status().as_u16();
    let headers = resp.headers().clone();
    let body = resp.bytes().await.map_err(net_error)?;
    Ok((status, headers, body))
}

fn ensure_ok(
    status: u16,
    headers: &reqwest::header::HeaderMap,
    body: &[u8],
) -> ProviderResult<Value> {
    let text = String::from_utf8_lossy(body);
    if !(200..300).contains(&status) {
        return Err(router_error(
            status,
            header_str(headers, "x-comfy-error-type"),
            &text,
            retry_after_ms(headers),
        ));
    }
    if text.trim().is_empty() {
        return Ok(Value::Null);
    }
    serde_json::from_str(&text).map_err(|e| {
        ProviderError::Failed(format!(
            "unexpected Comfy Router answer ({e}): {}",
            pictesa_core::error::truncate(&text, 300)
        ))
    })
}

/// A finished result: JSON with media inside, or the media bytes themselves.
fn result_outputs(
    headers: &reqwest::header::HeaderMap,
    body: &[u8],
    want: MediaKind,
) -> PollStatus {
    let ctype = header_str(headers, "content-type")
        .unwrap_or("")
        .split(';')
        .next()
        .unwrap_or("")
        .trim()
        .to_string();
    if MediaKind::from_mime(&ctype).is_some() {
        return PollStatus::Done(vec![Output::Bytes {
            data: body.to_vec(),
            mime: ctype,
        }]);
    }
    let v: Value = serde_json::from_slice(body).unwrap_or(Value::Null);
    let out = collect_outputs(&v, want);
    if out.is_empty() {
        return PollStatus::Failed(ProviderError::Failed(format!(
            "Comfy Router finished without {}: {}",
            want.as_str(),
            pictesa_core::error::truncate(&v.to_string(), 300)
        )));
    }
    PollStatus::Done(out)
}

// ---------------------------------------------------------------------------------------------
// Provider

fn key(ctx: &Ctx) -> Option<&str> {
    ctx.get("api_key")
}

fn base(ctx: &Ctx) -> String {
    ctx.base_url(API)
}

fn authed(ctx: &Ctx, rb: reqwest::RequestBuilder) -> reqwest::RequestBuilder {
    match key(ctx) {
        Some(k) => rb.header("X-API-Key", k),
        None => rb,
    }
}

/// Request id to (do not ask again before, queue position, running message).
type PollCache = HashMap<String, (Instant, Option<u32>, Option<String>)>;

/// Status answers are cached until their `Retry-After` passes, so fast engine polls cost nothing.
fn poll_cache() -> &'static Mutex<PollCache> {
    static C: OnceLock<Mutex<PollCache>> = OnceLock::new();
    C.get_or_init(Default::default)
}

impl ComfyRouter {
    async fn schema_doc(&self, ctx: &Ctx, id: &str) -> ProviderResult<Value> {
        if key(ctx).is_some() {
            let (status, headers, body) = send(
                authed(
                    ctx,
                    ctx.http
                        .get(format!("{}/v2/models/{id}/openapi.json", base(ctx))),
                )
                .timeout(API_TIMEOUT),
            )
            .await?;
            match ensure_ok(status, &headers, &body) {
                Ok(v) => return Ok(v),
                Err(ProviderError::Auth(_)) | Err(ProviderError::NotSupported(_)) => {}
                Err(e) => return Err(e),
            }
        }
        // The docs site publishes the same documents without a key.
        let (status, headers, body) = send(
            ctx.http
                .get(format!("{DOCS_SCHEMAS}/{id}.json"))
                .timeout(API_TIMEOUT),
        )
        .await?;
        if status == 404 {
            return Err(ProviderError::NotSupported(format!(
                "Comfy Router publishes no schema for {id}"
            )));
        }
        ensure_ok(status, &headers, &body)
    }

    async fn catalog(&self, ctx: &Ctx) -> ProviderResult<Vec<Value>> {
        let mut out = Vec::new();
        let mut cursor: Option<String> = None;
        for _ in 0..50 {
            let mut url = url::Url::parse(&format!("{}/v2/models", base(ctx)))
                .map_err(|e| ProviderError::Failed(e.to_string()))?;
            url.query_pairs_mut().append_pair("limit", "100");
            if let Some(c) = &cursor {
                url.query_pairs_mut().append_pair("cursor", c);
            }
            let (status, headers, body) =
                send(authed(ctx, ctx.http.get(url.as_str())).timeout(API_TIMEOUT)).await?;
            let page = ensure_ok(status, &headers, &body)?;
            out.extend(
                page.get("data")
                    .and_then(Value::as_array)
                    .cloned()
                    .unwrap_or_default(),
            );
            if page.get("has_more").and_then(Value::as_bool) != Some(true) {
                break;
            }
            cursor = page
                .get("next_cursor")
                .and_then(Value::as_str)
                .map(String::from);
            if cursor.is_none() {
                break;
            }
        }
        Ok(out)
    }

    async fn run_sync(
        &self,
        ctx: &Ctx,
        url: &str,
        idem: &str,
        body: &Value,
        want: MediaKind,
    ) -> ProviderResult<Submission> {
        let rb = authed(ctx, ctx.http.post(url))
            .header("Idempotency-Key", idem)
            .timeout(SYNC_TIMEOUT)
            .json(body);
        let (status, headers, bytes) = send(rb).await?;
        if !(200..300).contains(&status) {
            let text = String::from_utf8_lossy(&bytes);
            return Err(router_error(
                status,
                header_str(&headers, "x-comfy-error-type"),
                &text,
                retry_after_ms(&headers),
            ));
        }
        match result_outputs(&headers, &bytes, want) {
            PollStatus::Done(out) => Ok(Submission::Done(out)),
            PollStatus::Failed(e) => Err(e),
            _ => Err(ProviderError::Failed(
                "Comfy Router returned no result".into(),
            )),
        }
    }
}

#[async_trait]
impl Provider for ComfyRouter {
    fn info(&self) -> ProviderInfo {
        let mut url = CredentialField::base_url(API, &["COMFY_ROUTER_URL"]);
        url.label = "API URL".into();
        ProviderInfo {
            id: ID.into(),
            name: "Comfy Router".into(),
            kind: ProviderKind::Gateway,
            fields: vec![
                CredentialField {
                    placeholder: Some("comfyui-...".into()),
                    ..CredentialField::api_key(&["COMFY_API_KEY"])
                },
                url,
            ],
            signup_url: Some(
                "https://platform.comfy.org/profile/api-keys?onboarding=router".into(),
            ),
            docs_url: Some("https://docs.comfy.org/development/comfy-router/quickstart".into()),
            max_concurrency: 8,
        }
    }

    /// The catalog comes from `discover()`, which falls back to [`SNAPSHOT`] without a key.
    fn models(&self) -> Vec<ModelSpec> {
        Vec::new()
    }

    async fn discover(&self, ctx: &Ctx) -> ProviderResult<Vec<ModelSpec>> {
        if key(ctx).is_none() {
            return Ok(snapshot_specs());
        }
        let known: HashMap<&str, ModelSpec> =
            SNAPSHOT.iter().map(|r| r.0).zip(snapshot_specs()).collect();
        let mut out = Vec::new();
        let mut unknown_fetched = 0;
        for entry in self.catalog(ctx).await? {
            let Some(id) = entry.get("id").and_then(Value::as_str) else {
                continue;
            };
            let mut spec = if let Some(s) = known.get(id) {
                s.clone()
            } else {
                // A model newer than the snapshot: its schema says what it makes.
                if unknown_fetched >= 30 {
                    continue;
                }
                unknown_fetched += 1;
                let Ok(doc) = self.schema_doc(ctx, id).await else {
                    continue;
                };
                let Some(output) = output_of(&doc) else {
                    continue;
                };
                let inputs: Vec<MediaKind> = strings(doc.get("x-comfy-router-input-modalities"))
                    .into_iter()
                    .filter_map(modality)
                    .collect();
                let text = strings(doc.get("x-comfy-router-input-modalities")).contains(&"text");
                let name = entry
                    .get("model")
                    .and_then(Value::as_str)
                    .map(title)
                    .unwrap_or_else(|| id.to_string());
                let mut s = base_spec(id, &name, output, &inputs, text);
                s.tags.push("new".into());
                s
            };
            spec.meta = json!({
                "billing": entry.get("billing"),
                "schema_etag": entry.get("schema_etag"),
            });
            out.push(spec);
        }
        Ok(out)
    }

    async fn resolve(&self, ctx: &Ctx, model: &ModelSpec) -> ProviderResult<ModelSpec> {
        let id = model.remote_id();
        let doc = self.schema_doc(ctx, id).await?;
        let output = output_of(&doc).unwrap_or(model.output);
        let read = read_schema(&doc, id, output);
        let mut spec = model.clone();
        spec.output = output;
        spec.params = read.params;
        spec.inputs = read.inputs;
        spec.prompt = read.bindings.prompt.is_some();
        spec.negative_prompt = read.bindings.negative.is_some();
        spec.max_outputs = read.max_outputs.max(1);
        let accepted: Vec<MediaKind> = spec.inputs.iter().map(|s| s.kind).collect();
        spec.tasks = tasks_for(id, output, &accepted, spec.prompt);
        if spec.inputs.iter().any(|s| s.role == InputRole::StartFrame)
            && !spec.tasks.contains(&Task::ImageToVideo)
            && output == MediaKind::Video
        {
            spec.tasks.push(Task::ImageToVideo);
        }
        if doc.get("x-comfy-input-schema-authored") == Some(&Value::Bool(false))
            && !spec.tags.iter().any(|t| t == "unvalidated")
        {
            spec.tags.push("unvalidated".into());
        }
        let mut meta = spec.meta.as_object().cloned().unwrap_or_default();
        let unusable = if !read.unmapped.is_empty() {
            Some(format!(
                "its request needs fields Pictesa does not fill yet: {}",
                read.unmapped.join(", ")
            ))
        } else if !spec.prompt && spec.inputs.is_empty() {
            Some("Pictesa cannot map this model's request format yet".to_string())
        } else {
            None
        };
        if let Some(reason) = unusable {
            if !spec.tags.iter().any(|t| t == "unavailable") {
                spec.tags.push("unavailable".into());
            }
            meta.insert("unavailable".into(), json!(reason));
        }
        meta.insert("bindings".into(), serde_json::to_value(&read.bindings)?);
        spec.meta = Value::Object(meta);
        spec.complete = true;
        Ok(spec)
    }

    async fn check(&self, ctx: &Ctx) -> ProviderResult<()> {
        if key(ctx).is_none() {
            return Err(ProviderError::Auth(
                "missing api_key; create one at https://platform.comfy.org/profile/api-keys".into(),
            ));
        }
        let (status, headers, body) = send(
            authed(
                ctx,
                ctx.http.get(format!("{}/v2/models?limit=1", base(ctx))),
            )
            .timeout(API_TIMEOUT),
        )
        .await?;
        ensure_ok(status, &headers, &body).map(|_| ())
    }

    async fn submit(
        &self,
        ctx: &Ctx,
        model: &ModelSpec,
        req: &GenerationRequest,
    ) -> ProviderResult<Submission> {
        if key(ctx).is_none() {
            return Err(ProviderError::Auth(
                "missing api_key; add a Comfy API key in Settings".into(),
            ));
        }
        let id = model.remote_id();
        let bindings: Bindings = match model.meta.get("bindings") {
            Some(b) => serde_json::from_value(b.clone())?,
            None => self
                .resolve(ctx, model)
                .await
                .ok()
                .and_then(|m| serde_json::from_value(m.meta.get("bindings")?.clone()).ok())
                .unwrap_or_default(),
        };
        let mut files: HashMap<InputRole, Vec<Encoded>> = HashMap::new();
        for input in &req.inputs {
            files
                .entry(input.role)
                .or_default()
                .push(encode(input).await?);
        }
        let body = build_body(&bindings, req, &files)?;
        let raw = serde_json::to_vec(&body)?;
        let idem = idempotency_key(&req.job_id, id, &raw);
        let url = format!("{}/v2/models/{id}", base(ctx));
        let rb = authed(ctx, ctx.http.post(format!("{url}/requests")))
            .header("Idempotency-Key", &idem)
            .header(reqwest::header::CONTENT_TYPE, "application/json")
            .timeout(API_TIMEOUT)
            .body(raw);
        let (status, headers, bytes) = send(rb).await?;
        if status == 403 && header_str(&headers, "x-comfy-error-type") == Some("not_enabled") {
            // Raw-bytes models (some audio) and legacy keys cannot use the queue.
            return self.run_sync(ctx, &url, &idem, &body, model.output).await;
        }
        let answer = ensure_ok(status, &headers, &bytes)?;
        let request_id = answer
            .get("request_id")
            .and_then(Value::as_str)
            .ok_or_else(|| {
                ProviderError::Failed(format!("Comfy Router returned no request id: {answer}"))
            })?;
        if let Some(ms) = retry_after_ms(&headers) {
            if let Ok(mut c) = poll_cache().lock() {
                let position = answer
                    .get("queue_position")
                    .and_then(Value::as_u64)
                    .map(|p| p as u32);
                c.insert(
                    request_id.to_string(),
                    (Instant::now() + Duration::from_millis(ms), position, None),
                );
            }
        }
        let estimate = answer
            .get("estimate")
            .filter(|e| e.get("source").and_then(Value::as_str) == Some("exact"))
            .and_then(|e| e.get("amount"))
            .cloned();
        Ok(Submission::Pending(RemoteHandle::with(
            request_id,
            json!({
                "model": id,
                "status_url": answer.get("status_url"),
                "response_url": answer.get("response_url"),
                "cancel_url": answer.get("cancel_url"),
                "output": model.output,
                "estimate_usd": estimate,
            }),
        )))
    }

    async fn poll(&self, ctx: &Ctx, handle: &RemoteHandle) -> ProviderResult<PollStatus> {
        let id = &handle.id;
        let model = handle.data_str("model").unwrap_or_default();
        let base_url = format!("{}/v2/models/{model}/requests/{id}", base(ctx));
        if let Ok(c) = poll_cache().lock() {
            if let Some((until, position, message)) = c.get(id) {
                if Instant::now() < *until {
                    return Ok(match position {
                        Some(p) if message.is_none() => PollStatus::Queued { position: Some(*p) },
                        _ => PollStatus::Running {
                            progress: None,
                            message: message.clone(),
                        },
                    });
                }
            }
        }
        let status_url = handle
            .data_str("status_url")
            .map(String::from)
            .unwrap_or_else(|| format!("{base_url}/status"));
        let (code, headers, body) =
            send(authed(ctx, ctx.http.get(&status_url)).timeout(API_TIMEOUT)).await?;
        if code == 410 {
            return Ok(PollStatus::Failed(ProviderError::Failed("Comfy Router no longer keeps this result (results expire 24 hours after they finish)".into())));
        }
        let status = ensure_ok(code, &headers, &body)?;
        let state = status.get("status").and_then(Value::as_str).unwrap_or("");
        let position = status
            .get("queue_position")
            .and_then(Value::as_u64)
            .map(|p| p as u32);
        let estimate = header_str(&headers, "x-comfy-estimated-duration-seconds")
            .and_then(|s| s.parse::<u64>().ok());
        let message = estimate.map(|s| format!("usually takes about {s} s"));
        let wait = retry_after_ms(&headers);
        let remember = |position: Option<u32>, message: Option<String>| {
            if let (Some(ms), Ok(mut c)) = (wait, poll_cache().lock()) {
                c.retain(|_, (until, _, _)| until.elapsed() < Duration::from_secs(3600));
                c.insert(
                    id.to_string(),
                    (
                        Instant::now() + Duration::from_millis(ms.min(60_000)),
                        position,
                        message,
                    ),
                );
            }
        };
        match state {
            "IN_QUEUE" => {
                remember(position, None);
                Ok(PollStatus::Queued { position })
            }
            "IN_PROGRESS" => {
                remember(None, message.clone());
                Ok(PollStatus::Running {
                    progress: None,
                    message,
                })
            }
            "COMPLETED" => {
                if let Ok(mut c) = poll_cache().lock() {
                    c.remove(id);
                }
                let response_url = handle
                    .data_str("response_url")
                    .map(String::from)
                    .unwrap_or(base_url);
                let (code, headers, body) =
                    send(authed(ctx, ctx.http.get(&response_url)).timeout(API_TIMEOUT)).await?;
                if code == 202 {
                    return Ok(PollStatus::Running {
                        progress: None,
                        message: None,
                    });
                }
                let want = handle
                    .data
                    .get("output")
                    .and_then(|o| serde_json::from_value::<MediaKind>(o.clone()).ok())
                    .unwrap_or(MediaKind::Image);
                if let Some(error_type) = status.get("error_type").and_then(Value::as_str) {
                    let text = String::from_utf8_lossy(&body);
                    return Ok(PollStatus::Failed(router_error(
                        code,
                        Some(error_type),
                        &text,
                        None,
                    )));
                }
                if !(200..300).contains(&code) {
                    let text = String::from_utf8_lossy(&body);
                    return Ok(PollStatus::Failed(router_error(
                        code,
                        header_str(&headers, "x-comfy-error-type"),
                        &text,
                        None,
                    )));
                }
                Ok(result_outputs(&headers, &body, want))
            }
            other => Ok(PollStatus::Running {
                progress: None,
                message: Some(other.to_ascii_lowercase()),
            }),
        }
    }

    async fn cancel(&self, ctx: &Ctx, handle: &RemoteHandle) -> ProviderResult<()> {
        let url = match handle.data_str("cancel_url") {
            Some(u) => u.to_string(),
            None => format!(
                "{}/v2/models/{}/requests/{}/cancel",
                base(ctx),
                handle.data_str("model").unwrap_or_default(),
                handle.id
            ),
        };
        let _ = authed(ctx, ctx.http.put(url))
            .timeout(API_TIMEOUT)
            .send()
            .await;
        Ok(())
    }

    fn poll_interval(&self, attempt: u32) -> Duration {
        Duration::from_millis((1_000 + 500 * attempt as u64).min(5_000))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Recorded schema documents (docs.comfy.org/router-schemas snapshots, trimmed to the request).
    fn doc(id: &str) -> Value {
        let file = format!(
            "{}/tests/fixtures/comfy_router/{}.json",
            env!("CARGO_MANIFEST_DIR"),
            id.replace('/', "__")
        );
        serde_json::from_str(
            &std::fs::read_to_string(&file).unwrap_or_else(|e| panic!("{file}: {e}")),
        )
        .unwrap()
    }

    fn read(id: &str) -> ReadModel {
        let d = doc(id);
        read_schema(&d, id, output_of(&d).unwrap())
    }

    fn req(prompt: &str, params: Value) -> GenerationRequest {
        GenerationRequest {
            job_id: "01JOB".into(),
            model: "x".into(),
            prompt: prompt.into(),
            negative_prompt: None,
            params: params.as_object().cloned().unwrap_or_default(),
            inputs: vec![],
            num_outputs: 1,
        }
    }

    fn file(n: u8) -> Encoded {
        Encoded {
            url: format!("data:image/png;base64,QU{n}="),
            base64: format!("QU{n}="),
            mime: "image/png".into(),
        }
    }

    fn param_path<'a>(r: &'a ReadModel, key: &str) -> &'a Path {
        &r.bindings
            .params
            .iter()
            .find(|p| p.key == key)
            .unwrap_or_else(|| panic!("no {key} in {:?}", r.bindings.params))
            .path
    }

    #[test]
    fn snapshot_lists_media_models_with_tasks() {
        let specs = snapshot_specs();
        assert_eq!(specs.len(), SNAPSHOT.len());
        let mut ids: Vec<&str> = SNAPSHOT.iter().map(|r| r.0).collect();
        ids.sort_unstable();
        ids.dedup();
        assert_eq!(ids.len(), SNAPSHOT.len(), "duplicate ids");
        for f in FEATURED {
            assert!(ids.contains(f), "featured {f} is not in the snapshot");
        }
        let get = |id: &str| specs.iter().find(|s| s.remote_id() == id).unwrap();
        assert_eq!(
            get("bfl/flux-3-image").tasks,
            vec![Task::TextToImage, Task::ImageEdit]
        );
        let seedance = get("byteplus/dreamina-seedance-2-5-260628");
        assert_eq!(seedance.output, MediaKind::Video);
        assert_eq!(
            seedance.tasks,
            vec![Task::TextToVideo, Task::ImageToVideo, Task::VideoToVideo]
        );
        assert_eq!(seedance.vendor.as_deref(), Some("ByteDance"));
        assert!(seedance.tags.contains(&"featured".to_string()));
        assert_eq!(get("fal/crystal-upscaler").tasks, vec![Task::Upscale]);
        assert_eq!(get("kling/videos-lip-sync").tasks, vec![Task::LipSync]);
        assert!(specs.iter().all(|s| !s.complete && s.provider == ID));
        assert!(
            !ids.iter()
                .any(|i| i.starts_with("anthropic/") || i.starts_with("meshy/")),
            "text and 3D models are not listed"
        );
    }

    #[test]
    fn flat_schema_with_base64_numbered_inputs() {
        let r = read("bfl/flux-2-pro");
        assert_eq!(r.bindings.dialect, Dialect::Flat);
        assert_eq!(r.bindings.prompt, Some(vec![json!("prompt")]));
        assert_eq!(r.inputs.len(), 1);
        assert_eq!(
            (r.inputs[0].role, r.inputs[0].max),
            (InputRole::Reference, 9)
        );
        let b = &r.bindings.inputs[0];
        assert_eq!(b.enc, Encoding::Base64);
        assert_eq!(b.siblings.len(), 8);
        let keys: Vec<&str> = r.params.iter().map(|p| p.key.as_str()).collect();
        for k in ["width", "height", "seed", "output_format"] {
            assert!(keys.contains(&k), "{k} in {keys:?}");
        }
        assert!(!keys.contains(&"safety_tolerance"));
        assert!(matches!(
            r.params.iter().find(|p| p.key == "seed").unwrap().kind,
            ParamKind::Seed
        ));

        let files = HashMap::from([(InputRole::Reference, vec![file(1), file(2)])]);
        let body = build_body(
            &r.bindings,
            &req("a cabin", json!({"width": 1024, "seed": 3})),
            &files,
        )
        .unwrap();
        assert_eq!(
            body,
            json!({"prompt": "a cabin", "width": 1024, "seed": 3, "input_image": "QU1=", "input_image_2": "QU2="})
        );
    }

    #[test]
    fn content_items_for_seedance() {
        let r = read("byteplus/dreamina-seedance-2-5-260628");
        assert_eq!(r.bindings.dialect, Dialect::Content);
        let roles: Vec<InputRole> = r.inputs.iter().map(|s| s.role).collect();
        assert_eq!(
            roles,
            vec![
                InputRole::StartFrame,
                InputRole::EndFrame,
                InputRole::Reference,
                InputRole::Video,
                InputRole::Audio
            ]
        );
        let ratio = r
            .params
            .iter()
            .find(|p| p.key == keys::ASPECT_RATIO)
            .expect("ratio becomes aspect_ratio");
        assert!(ratio.validate(&json!("adaptive")).is_ok());
        assert_eq!(param_path(&r, keys::ASPECT_RATIO), &vec![json!("ratio")]);
        for k in [
            keys::RESOLUTION,
            keys::DURATION,
            keys::GENERATE_AUDIO,
            keys::SEED,
        ] {
            param_path(&r, k);
        }
        let keys_seen: Vec<&str> = r.params.iter().map(|p| p.key.as_str()).collect();
        assert!(
            !keys_seen.contains(&"callback_url") && !keys_seen.contains(&"draft"),
            "{keys_seen:?}"
        );

        let files = HashMap::from([(InputRole::StartFrame, vec![file(1)])]);
        let body = build_body(
            &r.bindings,
            &req(
                "the fox runs",
                json!({"aspect_ratio": "16:9", "duration": 5}),
            ),
            &files,
        )
        .unwrap();
        assert_eq!(
            body,
            json!({
                "content": [
                    {"type": "text", "text": "the fox runs"},
                    {"type": "image_url", "image_url": {"url": "data:image/png;base64,QU1="}, "role": "first_frame"}
                ],
                "ratio": "16:9",
                "duration": 5
            })
        );
    }

    #[test]
    fn dashscope_containers_and_media() {
        let r = read("wan/wan2.7-i2v");
        assert_eq!(
            r.bindings.prompt,
            Some(vec![json!("input"), json!("prompt")])
        );
        assert_eq!(
            r.bindings.negative,
            Some(vec![json!("input"), json!("negative_prompt")])
        );
        assert_eq!(
            param_path(&r, keys::DURATION),
            &vec![json!("parameters"), json!("duration")]
        );
        assert_eq!(
            param_path(&r, keys::GENERATE_AUDIO),
            &vec![json!("parameters"), json!("audio")]
        );
        let start = r
            .bindings
            .inputs
            .iter()
            .find(|b| b.role == InputRole::StartFrame)
            .unwrap();
        assert_eq!(
            (start.enc, start.tag.as_deref()),
            (Encoding::Media, Some("first_frame"))
        );
        let files = HashMap::from([(InputRole::StartFrame, vec![file(1)])]);
        let mut rq = req("leaf falls", json!({"resolution": "720P"}));
        rq.negative_prompt = Some("blur".into());
        let body = build_body(&r.bindings, &rq, &files).unwrap();
        assert_eq!(
            body,
            json!({
                "input": {"prompt": "leaf falls", "negative_prompt": "blur", "media": [{"type": "first_frame", "url": "data:image/png;base64,QU1="}]},
                "parameters": {"resolution": "720P"}
            })
        );

        // wan2.6 takes its first frame in img_url, not in media.
        let r = read("wan/wan2.6-i2v");
        let start = r
            .bindings
            .inputs
            .iter()
            .find(|b| b.role == InputRole::StartFrame)
            .unwrap();
        assert_eq!(
            (start.enc, start.path.clone()),
            (Encoding::Url, vec![json!("input"), json!("img_url")])
        );
        assert!(r.inputs.iter().any(|s| s.role == InputRole::Audio));
    }

    #[test]
    fn vertex_instances_for_veo() {
        let r = read("veo/veo-3.1-generate-001");
        assert_eq!(
            r.bindings.prompt,
            Some(vec![json!("instances"), json!(0), json!("prompt")])
        );
        assert_eq!(
            param_path(&r, keys::DURATION),
            &vec![json!("parameters"), json!("durationSeconds")]
        );
        assert_eq!(
            param_path(&r, keys::ASPECT_RATIO),
            &vec![json!("parameters"), json!("aspectRatio")]
        );
        assert_eq!(
            r.bindings.count,
            Some(vec![json!("parameters"), json!("sampleCount")])
        );
        let start = r
            .bindings
            .inputs
            .iter()
            .find(|b| b.role == InputRole::StartFrame)
            .unwrap();
        assert_eq!(start.enc, Encoding::Vertex);
        assert!(
            r.inputs.iter().any(|s| s.role == InputRole::EndFrame),
            "lastFrame"
        );
        let files = HashMap::from([(InputRole::StartFrame, vec![file(1)])]);
        let body = build_body(&r.bindings, &req("waves", json!({"duration": 8})), &files).unwrap();
        assert_eq!(body["instances"][0]["prompt"], "waves");
        assert_eq!(
            body["instances"][0]["image"],
            json!({"bytesBase64Encoded": "QU1=", "mimeType": "image/png"})
        );
        assert_eq!(body["parameters"]["durationSeconds"], 8);
        assert_eq!(body["parameters"]["sampleCount"], 1);
    }

    #[test]
    fn gemini_contents_for_nano_banana() {
        let r = read("vertexai/gemini-3.1-flash-image");
        assert_eq!(r.bindings.dialect, Dialect::Gemini);
        assert_eq!(r.inputs[0].role, InputRole::Reference);
        let files = HashMap::from([(InputRole::Reference, vec![file(1)])]);
        let body = build_body(
            &r.bindings,
            &req("make it night", json!({"aspect_ratio": "16:9"})),
            &files,
        )
        .unwrap();
        assert_eq!(
            body,
            json!({
                "contents": [{"role": "user", "parts": [{"text": "make it night"}, {"inlineData": {"mimeType": "image/png", "data": "QU1="}}]}],
                "generationConfig": {"imageConfig": {"aspectRatio": "16:9"}, "responseModalities": ["TEXT", "IMAGE"]}
            })
        );
    }

    #[test]
    fn dashscope_messages_for_qwen_image() {
        let r = read("qwen/qwen-image-3.0");
        assert_eq!(r.bindings.dialect, Dialect::Messages);
        assert_eq!(
            r.bindings.count,
            Some(vec![json!("parameters"), json!("n")])
        );
        assert_eq!(r.max_outputs, 6);
        assert!(r.params.iter().any(|p| p.key == keys::ENHANCE_PROMPT));
        let files = HashMap::from([(InputRole::Reference, vec![file(1)])]);
        let body = build_body(&r.bindings, &req("add a hat", json!({})), &files).unwrap();
        assert_eq!(
            body["input"]["messages"][0],
            json!({"role": "user", "content": [{"image": "data:image/png;base64,QU1="}, {"text": "add a hat"}]})
        );
        assert_eq!(body["parameters"]["n"], 1);
    }

    #[test]
    fn other_shapes() {
        // Unauthored schemas forward any object: send the prompt.
        let r = read("fal/fal-nano-banana-2");
        assert_eq!(r.bindings.prompt, Some(vec![json!("prompt")]));
        assert!(r.params.is_empty());
        // An upscaler takes no prompt.
        let r = read("fal/crystal-upscaler");
        assert!(r.bindings.prompt.is_none());
        assert_eq!(r.inputs[0].role, InputRole::Reference);
        assert!(r.params.iter().any(|p| p.key == keys::SCALE));
        // An image object with a constant type field.
        let r = read("xai/grok-imagine-video-1.5");
        let start = r
            .bindings
            .inputs
            .iter()
            .find(|b| b.role == InputRole::StartFrame)
            .unwrap();
        assert_eq!(start.enc, Encoding::Object);
        let files = HashMap::from([(InputRole::StartFrame, vec![file(1)])]);
        let body = build_body(&r.bindings, &req("go", json!({})), &files).unwrap();
        assert_eq!(
            body["image"],
            json!({"type": "image_url", "url": "data:image/png;base64,QU1="})
        );
        // Runway: promptText, and a single first frame as a plain string.
        let r = read("runway/gen4_turbo");
        assert_eq!(r.bindings.prompt, Some(vec![json!("promptText")]));
        let start = r
            .bindings
            .inputs
            .iter()
            .find(|b| b.role == InputRole::StartFrame)
            .unwrap();
        assert_eq!((start.enc, start.array), (Encoding::Url, false));
        // Kling's string durations show as numbers and go out as strings.
        let r = read("kling/kling-v3");
        let d = r.params.iter().find(|p| p.key == keys::DURATION).unwrap();
        assert!(d.validate(&json!(5)).is_ok());
        let body = build_body(
            &r.bindings,
            &req("a dog", json!({"duration": 5})),
            &HashMap::new(),
        )
        .unwrap();
        assert_eq!(body["duration"], "5");
        // LTX 2.5 on Router is text to video with its own fps and resolution choices.
        let r = read("ltx/ltx-2-5-pro");
        assert!(r.inputs.is_empty());
        assert!(r.params.iter().any(|p| p.key == keys::FPS));
    }

    #[test]
    fn keyframes_mode_and_unmapped_fields() {
        // FLUX 3 Video: images go to `keyframes` (start, then end) and `mode` follows the inputs.
        let r = read("bfl/flux-3-video");
        assert!(r.unmapped.is_empty(), "{:?}", r.unmapped);
        assert!(!r.params.iter().any(|p| p.key == "mode"));
        let files = HashMap::from([
            (InputRole::StartFrame, vec![file(1)]),
            (InputRole::EndFrame, vec![file(2)]),
        ]);
        let body = build_body(&r.bindings, &req("dolly in", json!({})), &files).unwrap();
        assert_eq!(
            body["keyframes"],
            json!(["data:image/png;base64,QU1=", "data:image/png;base64,QU2="])
        );
        assert_eq!(body["mode"], "i2v");
        let body = build_body(&r.bindings, &req("dolly in", json!({})), &HashMap::new()).unwrap();
        assert_eq!(body["mode"], "t2v");
        // Sync Labs needs an `input` list Pictesa does not build: listed as unavailable.
        let r = read("synclabs/sync-3");
        assert_eq!(r.unmapped, vec!["input".to_string()]);
    }

    #[test]
    fn idempotency_key_is_a_stable_uuid() {
        let a = idempotency_key("job1", "bfl/flux-2-pro", b"{}");
        assert_eq!(a, idempotency_key("job1", "bfl/flux-2-pro", b"{}"));
        assert_ne!(a, idempotency_key("job2", "bfl/flux-2-pro", b"{}"));
        assert_ne!(
            a,
            idempotency_key("job1", "bfl/flux-2-pro", b"{\"prompt\":\"x\"}")
        );
        let parts: Vec<&str> = a.split('-').collect();
        assert_eq!(
            parts.iter().map(|p| p.len()).collect::<Vec<_>>(),
            vec![8, 4, 4, 4, 12]
        );
        assert!(parts[2].starts_with('8'), "version 8");
        assert!(
            matches!(&parts[3][..1], "8" | "9" | "a" | "b"),
            "RFC 9562 variant"
        );
    }

    #[test]
    fn collects_outputs_from_recorded_results() {
        let all: Value =
            serde_json::from_str(include_str!("../tests/fixtures/comfy_router/outputs.json"))
                .unwrap();
        let urls = |id: &str, want| -> Vec<String> {
            collect_outputs(&all[id], want)
                .into_iter()
                .map(|o| match o {
                    Output::Url { url, .. } => url,
                    Output::Bytes { mime, .. } => format!("bytes:{mime}"),
                    Output::File { mime, .. } => format!("file:{mime}"),
                })
                .collect()
        };
        assert_eq!(
            urls("bfl/flux-2-pro", MediaKind::Image),
            vec!["https://example.invalid/bfl/flux-pro-1.1/sample.png"]
        );
        assert_eq!(
            urls("byteplus/dreamina-seedance-2-5-260628", MediaKind::Video),
            vec!["https://example.invalid/byteplus/seedance-1-0-pro-250528/generated.mp4"]
        );
        assert_eq!(
            urls("veo/veo-3.1-generate-001", MediaKind::Video),
            vec![
                "https://storage.googleapis.com/EXAMPLE_BUCKET/veo/USER_ID/REQUEST_ID/sample_0.mp4"
            ]
        );
        assert_eq!(
            urls("vertexai/gemini-3.1-flash-image", MediaKind::Image),
            vec!["bytes:image/png"]
        );
        assert_eq!(
            urls("openai/gpt-image-2", MediaKind::Image),
            vec!["bytes:image/png"]
        );
        assert_eq!(
            urls("xai/grok-imagine-video-1.5", MediaKind::Video),
            vec!["https://example.invalid/xai/grok-imagine-video/generated.mp4"]
        );
        assert_eq!(
            urls("ltx/ltx-2-5-pro", MediaKind::Video),
            vec!["https://example.invalid/ltx/generated.mp4"]
        );
        assert_eq!(
            urls("kling/kling-v3", MediaKind::Video),
            vec!["https://example.invalid/kling/kling-v2-5-turbo/generated.mp4"]
        );
        assert_eq!(urls("wan/wan2.7-i2v", MediaKind::Video).len(), 1);
    }

    #[test]
    fn router_errors_map_to_pictesa_errors() {
        let e = router_error(
            402,
            Some("insufficient_credits"),
            r#"{"detail":"Your workspace has no credits.","error_type":"insufficient_credits"}"#,
            None,
        );
        assert!(matches!(e, ProviderError::InsufficientCredits(m) if m.contains("no credits")));
        let e = router_error(
            422,
            Some("invalid_input"),
            r#"{"detail":[{"loc":["body","prompt"],"msg":"Field required","type":"missing"}]}"#,
            None,
        );
        assert!(matches!(e, ProviderError::InvalidInput(m) if m == "prompt: Field required"));
        let e = router_error(
            429,
            Some("rate_limited"),
            r#"{"detail":"slow down","error_type":"rate_limited"}"#,
            Some(7000),
        );
        assert!(matches!(
            e,
            ProviderError::RateLimited {
                retry_after_ms: Some(7000)
            }
        ));
        let e = router_error(409, Some("concurrency_limit_exceeded"), "{}", None);
        assert!(e.is_retryable());
        let e = router_error(
            200,
            Some("content_policy_violation"),
            r#"{"detail":"refused","refusal_subject":"input_text"}"#,
            None,
        );
        assert!(matches!(e, ProviderError::ContentPolicy(_)));
        let e = router_error(
            404,
            Some("model_not_found"),
            r#"{"detail":"bfl/flux-9 was retired on 2026-09-01; use bfl/flux-3-image"}"#,
            None,
        );
        assert!(matches!(e, ProviderError::NotSupported(m) if m.contains("retired")));
        let e = router_error(504, Some("provider_timeout"), "upstream timed out", None);
        assert!(e.is_retryable());
        // An unknown bucket falls back to the HTTP status.
        assert!(router_error(503, Some("brand_new_bucket"), "", None).is_retryable());
        assert!(matches!(
            router_error(
                400,
                None,
                r#"{"detail":"bad cursor","error_type":"invalid_input"}"#,
                None
            ),
            ProviderError::InvalidInput(_)
        ));
    }

    #[test]
    fn set_path_builds_nested_objects_and_arrays() {
        let mut v = json!({});
        set_path(
            &mut v,
            &[json!("instances"), json!(0), json!("prompt")],
            json!("x"),
        );
        push_path(&mut v, &[json!("a"), json!("list")], json!(1));
        push_path(&mut v, &[json!("a"), json!("list")], json!(2));
        assert_eq!(
            v,
            json!({"instances": [{"prompt": "x"}], "a": {"list": [1, 2]}})
        );
    }
}
