# Higgsfield teardown for Pictesa

Research date: 24.10.2026. Scope: image and video generation first; editing and studio apps second.

## Method and sources

| Source | What it gave | Confidence |
|---|---|---|
| Higgsfield MCP (`models_explore`, `get_presets`, `get_preset_instructions`, `get_workflow_instructions`, `show_plans_and_credits`), read-only calls only | Live model catalog (59 video, 47 image, 8 audio, 18 3D entries), parameter schemas, preset catalogs, recipe internals, top-up prices | High (live account data) |
| Public API docs (docs.higgsfield.ai, Mintlify, `llms.txt`, 84 workflow pages, `openapi.json`) | Endpoint paths, JSON schemas, reference limits, request lifecycle, billing, limits | High |
| Public web pages rendered in a fresh, logged-out headless browser (no login, no generation) | Create-page layouts, model picker, preset picker, pricing page, apps list, camera presets | High for what is visible logged out; progress states not observed directly |
| Public JSON the web app loads anonymously: `fnf-api-gw.higgsfield.ai/fnf/job-sets/costs`, `/fnf/subscriptions/v2/plans`, `pricing.higgsfield.ai/v2/pricing-config` | Machine-readable credit cost table (62 job types), plan prices | High |
| Higgsfield help center articles (credits, failures, Soul ID, Cinema Studio, Canvas, Lipsync, MCP) | Credit rules, queue rules, statuses, feature workflows | High |
| Third-party reviews (labelled where used) | User complaints, refund terms, controversies | Low to medium |

No proprietary images, preset thumbnails or master prompts are copied here. Preset descriptions are paraphrased.

## Key takeaways for the build

1. Higgsfield is an aggregator with a thin proprietary layer. Most quality comes from third-party models (Seedance, Kling, Veo, Nano Banana, GPT Image, Wan, MiniMax, FLUX, Grok, Seedream). Its own models (Soul family, DoP, Speak) are a minority of usage.
2. Seedance 2.5 is the default video model and the base of several "own" products. Cinema Studio 4.0 has the same mode set and the same per-second credit price as Seedance 2.5; Ad Multiplier states it is powered by Seedance 2.5. Genjutsu Lite states Wan 3.0 (motion) and Kling 4.0 (object swap).
3. A "preset" is a stored recipe: a locked model, locked parameters, a hidden master prompt, typed input slots, and sometimes a 2-step chain (image preprocessing, then video). Users only see a few exposed choices (resolution, aspect ratio). This is the main product moat and the easiest thing to open-source better.
4. Reference semantics differ per model and are a large hidden complexity: Seedance 2.5 accepts up to 30 images, 10 videos and 10 audio clips; Seedance 2.0 accepts 9/3/3; MiniMax H3 9/3/3; Wan 3.0 10/5/5; Kling Omni 4 images plus 1 video; Soul accepts 1 image. A normalized role model (start frame, end frame, image reference, video reference, audio reference, mask) is required.
5. Pricing is opaque by design: plan credits cost USD 0.033 to 0.070 each depending on plan, subscription credits expire every cycle, top-ups expire after 90 days, and per-second prices depend on resolution, audio and reference use. The same clip costs about 2.5x to 6x more on the Starter plan than at the public API headline price (API tier not stated).
6. "Unlimited" plans run on a slower shared queue and only on the website. MCP, CLI, Canvas, Supercomputer and API always charge credits. Paid credit jobs get a priority queue.
7. Agents are a first-class channel: a hosted MCP server (`mcp.higgsfield.ai/mcp`), CLI, ChatGPT plugin, Claude connector, bundled agent workflows (SKILL.md files) and an async REST API with idempotency keys and webhooks. Pictesa must match this on day one.
8. The public API omits the flagship third-party image models (Nano Banana, GPT Image, Seedream, FLUX) and Veo. Bring-your-own-key access to those providers is a real gap Pictesa can fill.
9. The web UX has three different composer layouts, about 25 top-level navigation entries, promo banners and upsell modals on load. A single fast composer is an easy win.
10. API outputs are deleted after 7 days by default; web assets live in a cloud library. A local-first library with full provenance (prompt, params, seed, cost, provider request id) is a clear differentiator.

---

## 1. Model catalog

Legend for aspect ratios (AR):

- `S5` = 1:1, 4:3, 3:4, 16:9, 9:16
- `S7` = S5 + 3:2, 2:3
- `W8` = S7 + 21:9
- `T10` = 1:1, 3:2, 2:3, 4:3, 3:4, 4:5, 5:4, 9:16, 16:9, 21:9
- `V3` = 16:9, 9:16, 1:1
- `V5` = V3 + 4:3, 3:4
- `V7` = auto, 21:9, 16:9, 4:3, 1:1, 3:4, 9:16

Credit costs come from the public cost table and the pricing page on 24.10.2026. "cr" = Higgsfield credits. One credit costs about USD 0.033 (Ultra annual) to USD 0.070 (Starter). Where the table shows a promo price, the promo price is given.

"Refs" counts are documented maximums. "n/d" means the count is not documented in the MCP schema or API docs.

### 1.1 Image models

| Model (MCP id) | Upstream vendor | Text-to-image | Edit / references (max) | Resolutions | AR | Notable controls | Cost |
|---|---|---|---|---|---|---|---|
| Soul 2.0 (`soul_2`) | Higgsfield | Yes | 1 image; trained Soul ID; curated style (`style_id`, 33 styles) cannot be combined with an image reference | 1.5k, 2k | S7 | API: batch 1 or 4, seed, prompt enhance, style strength (accepted, no effect) | 0.12 cr |
| Soul Cinema (`soul_cinematic`) | Higgsfield | Yes | 1 image; Soul Cinema character | 1.5k, 2k | W8 | Cinematic stills | Free-gen pools on paid plans |
| Soul (v1, API `soul/standard`) | Higgsfield | Yes | 1 image that guides prompt enhancement; trained character | 720p, 1080p (API) | S7 | Style presets with strength | 0.25 cr |
| Soul Cast (`soul_cast`) | Higgsfield | Text only | None | n/d | 16:9 (Cinema Studio variant: 11 ARs) | Compute budget 10-500; builds consistent cinematic actors | n/d |
| Soul Location (`soul_location`) | Higgsfield | Text only | None | n/d | W8 + 9:21 | Environments for Cinema Studio | n/d |
| Cinema Studio Image 2.5 (`cinematic_studio_2_5`) | Higgsfield | Yes | Yes, n/d | 1k, 2k, 4k | T10 | Older variant requires camera body, lens, focal length and optional aperture preset ids | n/d |
| Nano Banana (`nano_banana`) | Google | Yes | Yes, n/d | n/d | T10 | Budget tier | 1 cr |
| Nano Banana Pro (`nano_banana_pro`) | Google | Yes | Yes, n/d | 1k, 2k, 4k | T10 | Text and diagrams | 2 cr (4K: 4 cr) |
| Nano Banana 2 (`nano_banana_2`) | Google | Yes | Yes + mask inpaint | 1k, 2k, 4k | auto + T10 | `is_inpaint` | 2 cr |
| Nano Banana 2.1 (`nano_banana_2_1`) | Google | Yes | Images + video references + mask | 1k, 2k, 4k | auto + T10 + 4:1, 1:4, 8:1, 1:8 | Thinking level, web and image search grounding, seed | 1.5 / 2 / 3 cr |
| Nano Banana 2 Lite (`nano_banana_2_lite`) | Google | Yes | Yes + mask | 1k | auto + T10 | Thinking MINIMAL or HIGH | n/d |
| GPT Image 2 (`gpt_image_2`) | OpenAI | Yes | Yes, n/d | 1k, 2k, 4k | W8 | Quality low/medium/high | n/d |
| GPT Image 2.5 (`gpt_image_2_5`) | OpenAI | Yes | Yes, n/d | 1k, 2k, 4k | auto + 14 ARs | Variants Flare (fast) and Sunburst (quality); quality low to max; transparent background | 0.25 cr (low, 1k) to 15 cr (max, 4k) |
| OpenAI Hazel (`openai_hazel`) | OpenAI | Yes | Yes, n/d | n/d | 1:1, 3:2, 2:3, auto | Editing, text, logos, infographics | n/d |
| GPT Image / GPT Image 1.5 (web legacy) | OpenAI | Yes | Yes | n/d | n/d | | 1 cr |
| Seedream 4.5 (`seedream_v4_5`) | ByteDance | Yes | Yes, n/d | basic up to 4K, high about 6K | W8 | | n/d |
| Seedream 5.0 Lite / Flash / Pro | ByteDance | Yes | Yes, n/d; Pro adds inpaint and background removal | Flash and Pro: 1k, 1.5k, 2k | W8 (Flash adds auto) | Visual reasoning, instruction edits | Flash 0.5 cr; Pro 1.25-2.5 cr |
| 3D Jutsu Angles (`3d_jutsu_angles`) | ByteDance (Seedream 5.0 Pro) | Edit | Yes | 1k-2k | W8 | New camera viewpoints of a supplied scene | 1.25-2.5 cr |
| FLUX.2 (`flux_2`) | Black Forest Labs | Yes | Yes, n/d | 1k, 2k | S5 | Variants pro, flex, max | 1 / 3 / 4 cr |
| FLUX 3 Image (`flux_3_image`) | Black Forest Labs | Yes | Up to 10 images | 768sq, 1k, 1.5k, 2k, 4k | auto + 14 ARs | Batch 1-4 | n/d |
| Flux Kontext (`flux_kontext`) | Black Forest Labs | Edit-first | Yes, n/d | n/d | S5 | Style transfer, typography edits | 1.5 cr |
| Kling O1 Image (`kling_omni_image`) | Kling | Yes | Yes, n/d | 1k, 2k | auto + W8 | | n/d |
| Grok Image / Grok Image 2.0 | xAI | Yes | 2.0: up to 10 images | 1k, 2k | auto + 9 ARs | Quality std/quality or low/medium | API: USD 0.04 |
| Ideogram 4.5 (`ideogram_4_5`) | Ideogram | Yes | Up to 5 images + mask | 1k, 2k | 24 ARs incl. 3:1, 1:3 | Magic prompt, seed, batch 1-4, quality very_low to high | 0.25-6.25 cr |
| Ideogram Vector (`ideogram_vector`) | Ideogram | Yes (SVG) | Vectorize 1 image | n/d | 18 ARs | Generate or vectorize | 3-10 cr |
| Ideogram 4.0 (API only) | Ideogram | Yes | 1 image with weight 1-100 | n/d | 23 ARs | Rendering speed TURBO/DEFAULT/QUALITY | API: USD 0.03 |
| Recraft V4.1 (`recraft_v4_1`) | Recraft | Text only | None | 1k, 2k | 9 ARs | Model types standard, vector, utility, utility_vector; palette up to 10 colours; background colour | 1.25-10 cr |
| Recraft V4 Styles | Recraft | Text only | Saved personal style id | 1k, 2k | 14 ARs | Style match precise/flexible | n/d |
| Qwen Image 3 (API only) | Alibaba | Yes | Edit: 1-3 images | 1k, 2k | 10 ARs | Thinking, prompt extend | API: USD 0.04 |
| Z-Image / Z-Image Turbo | Alibaba Tongyi-MAI | Yes | None | 1k, 2k (API) | S5 / 10 ARs | Very fast, budget | API: USD 0.015 |
| Image Auto (`image_auto`) | Higgsfield router | Yes | Yes | n/d | S5 | Picks a model from prompt intent | n/d |
| Marketing Studio Image | Higgsfield on GPT Image 2.5 (API exposes Flare and Sunburst variants) | Yes | Up to 16 images | 1k, 2k, 4k | auto + T10 | Product presets, prompt enhance with product + person reference | API: USD 0.0107 |
| DTC Ads (`ms_image`) | Higgsfield | Yes | Up to 14 images, up to 4 saved products | 1k, 2k, 4k | 15 ARs | Required ad style, brand kit, batch 1-20 | n/d |
| Marketing Studio Product Shot | Higgsfield | Yes | Product, character, model images | n/d | S7 | Types: product shots, product shots with people, poster, ads, marketplace | n/d |
| Ads Studio (API) | Higgsfield on GPT Image 2.5 Flare | Brief to images | Up to 10 images | fixed 2k, high quality | 14 ARs | Brief to 1-8 ad concepts with copy | n/d |
| AutoSprite (`autosprite`) | Higgsfield | Image to sprite sheet | 1 character image | 32-512 px frames | n/a | 2-64 frames, idle/walk/run/attack/jump and isometric directions, optional sound | n/d |

Image utilities (editing, not generation): FLUX.2 Pro Outpaint (per-side pixel expansion, negative values crop), Outpaint (expand to a target AR), Topaz image upscale (Standard V2, Low Resolution V2, CGI, High Fidelity V2, Text Refine) and Topaz generative upscale (Standard MAX, Redefine, Recovery, Recovery V2), ByteDance image upscale (2k, 4k), Image Decompose (split an image into layers), background remover.

Web-only legacy image tools still on the pricing page: Reve (0.25 cr), Wan 2.2 image (0.5 cr), Multi Reference (1.5 cr), Face Swap and Character Swap (about 2 cr), Soul Inpaint and Product Placement (0.25 cr), Banana Placement (1 cr), Higgsfield Popcorn storyboard (1.5 cr per image, up to 8 scenes).

### 1.2 Video models

| Model (MCP id) | Vendor | T2V | I2V start | End frame | Refs img / vid / audio (max) | Edit / extend | Duration | Resolution | AR | Native audio | Credits per second |
|---|---|---|---|---|---|---|---|---|---|---|---|
| Seedance 2.5 (`seedance_2_5`) | ByteDance | Yes | Yes | Yes | 30 / 10 / 10, max 50 items, 30 s per media type | Edit and extend (forward or backward); Draft mode at 480p | 4-30 s | 480p, 720p, 1080p (cost table also lists 4K) | V7 | Yes (default on) | 3 / 7 / 16 / 26-27; edit 3.5 / 7.5 / 20 / 34 |
| Seedance 2.0 (`seedance_2_0`) | ByteDance | Yes | Yes | Yes | 9 / 3 / 3 (audio alone is not enough) | No | 4-15 s | 480p-4K | V7 | Yes | 3 / 4.5 / 12 / 22 |
| Seedance 2.0 Fast | ByteDance | Yes | Yes | Yes | As 2.0 | No | 4-15 s | 480p, 720p | V7 | Yes | about 12 cr per 5 s at 720p; marked "not available in US" |
| Seedance 2.0 Mini (`seedance_2_0_mini`) | ByteDance | Yes | Yes | Yes | Yes, n/d | No | 4-15 s | 480p, 720p | V7 | Yes | 0.5 / 1 (promo) |
| Seedance 1.5 Pro (`seedance1_5`) | ByteDance | Yes | Yes | Yes | No | No | 4, 8, 12 s | 480p-1080p | V7 | Yes | about 1 / 3 / 7 cr per 5 s; being retired |
| Seedance Pro, Pro Fast (1.0, web legacy) | ByteDance | Yes | Yes | n/d | No | No | 5-10 s | to 1080p | n/d | No | about 4-18 cr per 5 s |
| Kling 4.0 (`kling4_0`) | Kling | Yes | Ordered keyframes | Yes | Images + videos, Kling elements | Video edit | 3-30 s | 720p, 1080p | 21:9, 16:9, 9:16, 1:1, auto | n/d | 1.5 / 2-2.5 (promo; list 2.75-5) |
| Kling 3.0 (`kling3_0`) | Kling | Yes | Yes | Yes | Kling elements; multi-shot with 1-6 shot prompts | No | 3-15 s | std, pro, 4K | V3 | On/off | std 1.5 off / 2 on; pro 1.75 / 2.5; 4K 6 |
| Kling 3.0 Turbo (`kling3_0_turbo`) | Kling | Yes | Yes | No | No | No | 3-15 s | 720p, 1080p | V3 | No | 1.5 / 2 |
| Kling 3.0 Omni, image reference (`kling_o3_image_reference`) | Kling | Yes | Optional | Optional | Up to 7 images (MCP); elements | No | 3-15 s | std, pro, 4K | V3 | On/off | std 1-1.25; pro 1.5-2; 4K 6 |
| Kling 3.0 Omni, frames (`kling_o3_flf`) | Kling | Yes | Yes | Yes | No | No | 3-15 s | std, pro, 4K | V3 | On/off | std 1-1.25; pro 1.25-1.8; 4K 6 |
| Kling 3.0 Omni edit / video reference (API `kling-video/o3/...`) | Kling | n/a | n/a | n/a | 1 video + up to 4 images + 4 elements | Edit (source 3-15.5 s, up to 200 MB) | ref mode 3-10 s | std, pro, 4K | V3 | n/d | API: USD 0.042/s headline |
| Kling O1 / Omni 1 (web, API `kling-video/omni/...`) | Kling | Yes | Yes | Yes | Images, 1 video | Edit (3-10 s source) | 3-10 s | 720p, 1080p | V3 | n/d | about 7-10 cr per 5 s |
| Kling 2.6 (`kling2_6`) | Kling | Yes | Yes | No | No | No | 5, 10 s | n/d | V3 | On/off | about 5 (off) / 10 (on) cr per 5 s |
| Kling 2.5 Turbo (web, API) | Kling | Yes | Yes | No | No | No | 5, 10 s | 720p, 1080p | n/d | No | about 4-6 cr per 5 s |
| Kling 2.1 / 2.1 Master (web legacy) | Kling | Yes | Yes | n/d | No | No | 5 s | to 1080p | n/d | No | about 5-25 cr per 5 s |
| Kling 3.0 / 2.6 Motion Control | Kling | n/a | Character image | n/a | 1 driving video (3-30 s) | Recast | follows source | std, pro | follows input | Keeps source sound | 1.5 / 2.5 |
| Google Veo 3.1 (`veo3_1`) | Google | Yes | Yes | No (MCP) | No | No | 4, 6, 8 s | 720p, 1080p | 16:9, 9:16 | Yes | Fast about 4/s; full about 10/s |
| Google Veo 3.1 Lite (`veo3_1_lite`) | Google | Yes | Yes | Yes | No | No | 4, 6, 8 s | to 1080p | 16:9, 9:16, auto | Toggle | n/d |
| Google Veo 3 (`veo3`) | Google | Yes | Yes | No | No | No | 8 s | 720p | 16:9, 9:16 | Yes | Fast about 22, full about 58 cr per 8 s |
| Gemini Omni Flash (`gemini_omni`) | Google | Yes | n/a | n/a | Images + videos | Video to video | 4-10 s | 720p | 16:9, 9:16 | Yes | 3 |
| Gemini Omni Flash 1.1 (+ Extend) | Google | Yes | Yes | Yes | Images + videos | Edit, extend | 3-10 s | 360p-4K | 16:9, 9:16 | Yes | n/d |
| MiniMax H3 (`minimax_h3`) | MiniMax | Yes | Yes | Yes | 9 / 3 / 3 (audio needs an image or video) | No | 5-15 s | fixed 2K | auto, adaptive + 7 | Audio reference only | API: USD 0.065/s headline |
| MiniMax H3 Max (`minimax_h3_max`) | MiniMax | Yes | Yes | Yes | As H3 | No | 5-15 s | 480p, 768p | V7 | n/d | n/d |
| MiniMax Hailuo 2.3 / 2.3 Fast / 02 / 02 Fast | MiniMax | Yes | Yes | Yes | No | No | 6, 10 s | 512p, 768p, 1080p | from input | No | about 3-10 cr per 6 s |
| Wan 3.0 / Wan 3.0 Prime (`wan3_0`, `wan3_0_prime`) | Alibaba | Yes | Yes | Yes | 10 / 5 / 5, plus one document or web page reference (enables thinking) | No | 2-30 s | 480p, 720p, 1080p | 16:9, 4:3, 1:1, 3:4, 9:16, adaptive | Yes | 3.0: 1 / 1.75 / 3.5; Prime: 1.5 / 3 / 6 |
| Wan 2.7 (`wan2_7`) | Alibaba | Yes | Yes | Yes | Up to 5 images and videos combined; driving audio | No | 2-15 s (reference mode 2-10 s) | 720p, 1080p | V5 | Yes | n/d |
| Wan 2.6 (`wan2_6`) | Alibaba (open weights) | Yes | Yes | No | 1-3 reference videos; driving audio | No | 5, 10, 15 s | 480p-1080p | V3 / V5 | Yes | about 1-2 |
| Wan 2.5, 2.5 Fast, 2.2, 2.2 Fast (web legacy) | Alibaba | Yes | Yes | n/d | No | No | 5-10 s | to 1080p | n/d | 2.5 yes | about 1-2.6 |
| Grok Imagine Video 1.5 (`grok_video_v15`) | xAI | Yes | Yes | No | Up to 7 images + 1 audio (reference mode max 720p) | No | 1-15 s | 480p, 720p, 1080p | auto + 7 | Yes | 2.5 / 4.5 / 8 |
| Grok Imagine Video 1.5 Lite | xAI | Yes | Yes | No | No | No | 1-15 s | to 1080p (upscaled) | auto + 7 | Yes | 1 / 1 / 4 |
| Grok Video, Grok Video Edit (web) | xAI | Yes | Yes | No | No | Edit | 1-15 s | n/d | V3 | Yes | about 1.4-2; charged at start, no refund on failure |
| FLUX 3 Video (`flux_3_video`) | Black Forest Labs | Yes | Yes | Yes | Images + videos (storyboard, continuation) | Continuation; separate prompt edit model (first 15 s, 1 cr/s) | 5-20 s | 720p, 1080p, 2K, 4K | auto, 21:9, 2:1, 16:9, 4:3, 1:1, 3:4, 9:16 | Yes | 6 / 8 / 10 / 13 |
| HappyHorse 1.0 / 1.1 (`happy_horse_video`) | Alibaba | Yes | Yes | No | 1-9 images (no video refs) | No | 2-15 s | 720p, 1080p | V5 | n/d | 2.5 / 4.5 (promo) |
| LTX-2.5 Fast / Pro (API only, not in web picker) | Lightricks | Yes | Yes | Yes | No | No | 6, 8, 10 s | Fast 720p-4K at 24-50 fps; Pro 720p, 1080p | 16:9, 9:16 | Yes | API: USD 0.09 / 0.12 per s; has a camera_movement enum |
| PixVerse V6 (API only) | PixVerse | Yes | Yes | Yes | No | No | 1-15 s | 360p-1080p | V5 | Yes | API: USD 0.0978/s |
| Higgsfield DoP Lite / Standard / Turbo | Higgsfield | No | Yes (required) | No | Motion preset instead of references | No | 3 or 5 s | 720p | from input | No | about 1 / 2.3 / 1.7 (3 / 7 / 5 cr per 3 s) |
| Cinema Studio Video 4.0 (`cinematic_studio_video_4_0`) | Higgsfield, likely on Seedance 2.5 (same modes and same price) | Yes | Yes | Yes | 30 / 10 / 10, up to 50 | Edit and extension | 4-30 s | 480p-1080p (4K in cost table) | V7 | Yes | 3 / 7 / 16 / 26-27 |
| Cinema Studio Video 3.5 | Higgsfield, likely Seedance 2.0 class | Yes | Yes | Yes | Images, videos, audio | No | 4-15 s | 480p-1080p | V7 | Toggle | 3.5 / 5 / 13 / 24 |
| Cinema Studio Video 3.0 (`cinematic_studio_3_0`) | Higgsfield | Yes | Yes | Yes | Up to 9 references (help center) | No | 4-15 s | 480p-4K | V7 | Toggle | 3.5 / 5 / 13 / 24 |
| Cinema Studio Video v1 / v2 | Higgsfield | Yes | Yes | Yes | No | No | 5, 10 s / 3-12 s | n/d | V5 | Yes | n/d |
| Marketing Studio Video (v1, v2) | Higgsfield | Product brief | Product, avatar, location photos | n/a | Product links, ad references, app screenshots | No | 12-15 s (v1), 4-30 s (v2) | 480p-1080p | V7 | Yes | 3.5 / 5 / 13 |

Video utilities and post-processing (editing, not generation): Topaz video upscale (1080p, 2160p, enhancement, frame interpolation), ByteDance video upscale (1080p, 2K, 4K, 24-60 fps, content presets common/aigc/short_series/ugc/old_film), FLUX video upscale (creativity 0 or 1, 0.5-8 cr/s), Topaz Hyperion 2.5 (SDR to HDR, ProRes/H.265/EXR), FPS Boost (ByteDance or Topaz, 16-120 fps), Video Deflicker, SAM 3 video background removal (0.225 cr/s), Depth Anything Video (depth maps, 1.5 cr/s).

Sora 2 (OpenAI): marketing pages, a preset page and an upscale page still exist, but Sora is absent from the live video model picker, the MCP catalog and the API catalog on 24.10.2026. Treat it as unavailable. Not verified for every account.

### 1.3 Audio and 3D (context only)

- Audio: Seed Audio 1.0, Qwen Audio 3.0 TTS Flash, Eleven v4 and v4 Turbo (dialogue with up to 10 voices), Text to Speech V2 (engines ElevenLabs, MiniMax, Seed Speech, Vibe Voice, Cozy Voice), voice cloning from a sample, voice change, dubbing into 18 languages, Sync Lipsync 3.
- 3D: Meshy 5/6/7 (image or text to 3D, rigging, 697 animation clips, remesh, retexture), Tripo H3.1 and P2, Hunyuan3D v3/v3.1, SAM 3 3D objects and body.

### 1.4 Catalog size by channel

| Channel | Image | Video | Notes |
|---|---|---|---|
| MCP `models_explore` | 47 entries | 59 entries | Includes utilities and app models |
| Web model picker | about 35 image models | 11 video families, about 45 models | Shows badges NEW, TOP, PREMIUM, EXCLUSIVE and notes such as "being retired" |
| Public API catalog | 17 entries (14 families) | 67 entries (22 families) | Seedance, Kling, Wan, HappyHorse, MiniMax, LTX, PixVerse, Grok, Genjutsu, Cinema Studio, Soul, Recraft, Qwen, Ideogram, Z-Image, Ads/Marketing Studio |

---

## 2. Preset taxonomy

Higgsfield uses the word "preset" for six different things. Pictesa should model them as one concept with different templates.

| Preset family | Count (24.10.2026) | Bound to | What the user supplies | What the preset fixes |
|---|---|---|---|---|
| Camera Controls (DoP motion presets) | 65 names on /camera-controls (site claims 50+, video page claims 250+ incl. VFX) | Higgsfield DoP models | 1 image + scene prompt | Camera logic or effect; duration 3 or 5 s |
| Viral Hub effects ("chain" presets) | 87 in Effects | A hidden chain of image and video jobs | 1-3 photos (character, optional location, optional product), optional prompt | Models, prompts, durations; exposes resolution and AR only |
| Marketing Studio templates | 231 motion, 418 product shot, plus UGC, ads, posters, marketplace, TikTok | Marketing Studio models | Product, optional avatar, brief | Style, layout, motion type |
| Genjutsu presets | Large community feed (Higgsfield, Trending, New tabs); 20 Styles | Genjutsu models | 1-8 reference images (motion and swap) or own video (styles) | Driving video, operation, model |
| Katana presets | Community catalog | Claude-driven edit workflow | Reference edit video + subject media | Edit structure, pacing, transitions |
| Agent recipes (MCP slash commands) | 63 (45 image, 18 video) | Fixed model per recipe | Product image | Model, AR, resolution, quality, duration, master prompt, optional preprocessing step |
| Style presets | Soul 2.0: 33; Genjutsu Styles: 20; personal Recraft styles; Cinema Studio palettes: 50 | Soul, Genjutsu restyle, Recraft, Cinema Studio | Prompt or video | Look, grade, rendering style |

### 2.1 Camera and motion presets (DoP Camera Controls)

Help-center grouping: Effects, Basic Camera Control (lower intensity), Epic Camera Control (higher intensity), Catch the Pulse (curated theme), Mix (combine several presets in one clip), New and Trending feeds. A "Top Choice" badge marks curated presets. Third-party API resellers describe a motion strength value of about 0.3-1.0 and quality tiers lite, turbo and preview; Higgsfield's own help center only documents duration (3 or 5 s), seed, steps and a prompt Enhance toggle.

Descriptions below are in our own words.

| Group | Preset | What it does |
|---|---|---|
| Neutral | General | No fixed move; motion follows the prompt |
| Neutral | Static | Locked-off camera, only the subject moves |
| Push and pull | Dolly In / Dolly Out | Smooth physical move toward or away from the subject |
| Push and pull | Super Dolly In / Super Dolly Out | Long, exaggerated travel toward or away from the subject |
| Push and pull | Double Dolly | Camera and subject travel together so the subject seems to glide |
| Lateral | Dolly Left / Dolly Right | Sideways slide on a track |
| Lateral | Pan Left / Pan Right | Camera turns horizontally from a fixed point |
| Lateral | Whip Pan | Very fast pan with blur, often a transition |
| Vertical | Tilt Up / Tilt Down | Camera turns vertically from a fixed point |
| Vertical | Crane Up / Crane Down | Camera rises or descends on a crane |
| Vertical | Crane Over The Head | Camera rises and passes over the subject to look down |
| Vertical | Jib Up / Jib Down | Short vertical arc on a jib arm |
| Vertical | Overhead | Straight top-down view |
| Orbit | Arc Left / Arc Right | Curved path partway around the subject |
| Orbit | 360 Orbit | Full circle around the subject |
| Orbit | Lazy Susan | Slow turntable-like rotation around the subject |
| Orbit | 3D Rotation | The scene or subject turns as one object in 3D space |
| Orbit | Bullet Time | Action freezes while the camera sweeps around it |
| Zoom | Zoom In / Zoom Out | Standard optical zoom |
| Zoom | Rapid Zoom In / Rapid Zoom Out | Fast zoom, smoother than a crash zoom |
| Zoom | Crash Zoom In / Crash Zoom Out | Abrupt, very fast zoom for impact |
| Zoom | Dolly Zoom In / Dolly Zoom Out | Dolly and zoom in opposite directions so the background stretches (vertigo effect) |
| Zoom | YoYo Zoom | Zoom in and back out in a bouncing rhythm |
| Zoom | Eating Zoom | Tight push toward the mouth during eating, food-ad style |
| Body and rig | Snorricam | Camera fixed to the subject's body; subject stays still in frame while the world moves |
| Body and rig | Head Tracking | Camera locks onto the head and follows it |
| Body and rig | Robo Arm | Precise, fast robotic arm move around the subject |
| Body and rig | Handheld | Natural hand-held shake |
| Body and rig | Wiggle | Small rhythmic jitter |
| Body and rig | BTS | Candid behind-the-scenes look, as if a crew member films |
| Body and rig | Hero Cam | Low, slow move that makes the subject look powerful |
| Body and rig | Glam | Slow flattering beauty move with a soft fashion look |
| Aerial | FPV Drone | Fast, agile first-person drone flight |
| Aerial | Aerial Pullback | Camera rises and pulls back to reveal the area |
| Vehicle | Car Grip | Camera mounted on the car body |
| Vehicle | Car Chasing | Chase-vehicle view following a car |
| Vehicle | Buckle Up | Fast forward rush with strong shake, like hard acceleration |
| Vehicle | Road Rush | Low, fast travel along the road surface |
| Transition | Eyes In | Camera dives into the eye and through it into a new scene |
| Transition | Mouth In | Camera dives into an open mouth |
| Transition | Through Object In / Through Object Out | Camera passes through a gap or object into or out of the scene |
| Transition | Flying Cam Transition | Camera flies through space to link two scenes |
| Transition | Object POV | Shot from the viewpoint of an object in the scene |
| Optical | Focus Change | Rack focus between foreground and background |
| Optical | Fisheye | Ultra-wide, curved lens look |
| Optical | Low Shutter | Motion-blur trails from a slow shutter |
| Optical | Dutch Angle | Tilted horizon for unease |
| Optical | Incline | The whole world tilts as if gravity shifted |
| Time | Hyperlapse | Moving time-lapse over a long camera path |
| Time | Timelapse Human / Timelapse Landscape / Timelapse Glam | Accelerated time for people, scenery or fashion |

Structured camera vocabularies in other products (useful as Pictesa enums):

- Cinema Studio 4.0 `camera_movement` (33): snorricam, robot-arm, tilt-up, rack-focus, tilt-down, pov, pan-left, crane-up, pan-right, crane-down, side-tracking, pedestal-up, pedestal-down, handheld, tracking, drone-orbit, dolly-zoom, aerial-pullback, static-shot, bullet-time, whip-pan, slow-zoom-in, arc-left, slow-zoom-out, arc-right, truck-right, dolly-in, truck-left, dolly-out, slider-right, crush-zoom, slider-left, helicopter-shot.
- Cinema Studio 4.0 optics and look: camera body (modern, 35mm film, 8mm film, DV camcorder), lens (clean sharp, anamorphic, vintage anamorphic, warm vintage, halation vintage), aperture (f/1.4 wide open, f/4, f/11 deep focus), pacing (chaotic, dynamic, calm, single shot), genre (epic, drama, noir, comedy, horror, action), era (1960s, 1980s, 1990s, 2000s, 2020s), light (silhouette, practicals, window, overhead fall, contre-jour, soft cross), 50 named colour palettes. Omitted fields mean "the director chooses".
- Cinema Studio 3.5: 9 camera move sets (classic static, silent machine, one take, epic scale, intimate observer, impossible camera, documentary snap, raw chaos, dreamy flow), 6 light schemes, 8 grades (naturalistic clean, bleached warm, hyper neon, teal-orange epic, sodium decay, cold steel, bleach bypass, classic B/W), multi-shot with per-shot prompts.
- Cinema Studio 2.x / 3.0 speed ramps: auto, linear, slow-mo, speed-up, impact, custom (3.0 adds flash in, flash out, bullet time, ramp up).
- LTX-2.5 (API) `camera_movement`: dolly in, dolly out, dolly left, dolly right, jib up, jib down, static, focus shift.

### 2.2 Effects and viral presets

Viral Hub chain presets fall into these categories (sample names from the 87 live presets):

| Category | Examples | Typical use |
|---|---|---|
| Body transformations | Vanish, Melting, Burning man, Monster dab | Hook shots, surreal reveals |
| Scale illusions | Street colossus, Lacewalker, Selfception | Giant or tiny subject, recursive zoom |
| Time effects | Act natural, Frozen in motion, Bullet time, Stop world | Subject or world frozen while the other moves |
| World warps | Architecture wave, World morphing, Incline, Lidar transition, Cutout | City bends, gravity shifts, layered cutouts |
| Clones | Clones, Infinite clones, Studio slide | Multiplied subject, fashion layering |
| Camera transitions | Eyes in, Earth zoom, High flip, Wild ride, Flying cam | Scene changes in one move |
| Art recreations (split screen) | Pearl earring, Fallen angel, Cyclope, Argus | User inserted into a famous painting, original below |
| Mixed-media looks | Comic, Particles, LSD, Canvas, Palette, Fragments, Tracking, Windows, Ink Riot, Cold vision | Graphic overlays and stylized treatments |
| Cinematic scene templates | Fairytale castle, Mighty fighter, Knight's diary, Superstar, 2000's paparazzi, Moonwalk, Blue depth, Pigeons, Agamemnon | Ready-made scenes starring the user |
| Product reveals | Floating fall, Smash and grab, Boarding pass | Lifestyle ads with product close-ups |

Other effect galleries:

- Marketing Studio motion types: hypermotion, mixed media, 2D motion, SaaS motion. Product-shot types: product only, product with people.
- Genjutsu Styles (restyle the user's own video, motion kept): Hand-Painted Cel Animation, Knitted Yarn World, Expressive 2D Animation, Rounded Toy CGI, Felt Puppet Stop Motion, Grotesque Street CGI, Warm Painterly 3D Animation, Graphic Cel, Expressive Gouache Fashion, Sculptural 3D Fashion, Chunky Textured Low-Poly, Cel-Shaded CG Anime, Painterly Cinematic 3D, Outline-free Cel, Painterly Sculpted Animation, Neon Graphic Cinematic 3D, Textured Gouache Character Illustration, Comic 3D, Retro Game 3D, Nighttime Anime Pixel Art.
- Apps gallery (one-click templates): Professional (Virality Predictor, Expand image, Angles 2.0, Shots, Transitions), Enhance and Style (Skin Enhancer, AI Stylist, Relight, Outfit Swap, Style Snap), Face and Identity (Face Swap, Headshot Generator, Character Swap 2.0, Recast, Video Face Swap), Video Editing (ClipCut, Urban Cuts, Video Background Remover, Breakdown, Japanese Show), Ads and Products (Click to Ad, Billboard Ad, Bullet Time Scene, Truck Ad, Bullet Time White), Games and Characters (Game Dump, Nano Strike, Nano Theft, Simlife, Plushies), Extras (AI Meme Generator, Background Remover, Micro-Beasts, Signboard, Paint App), Trending Templates (meme and trend formats).
- Agent recipes: image recipes such as hero shot, flatlay, in hand, before/after, benefits, specs, unboxing, reel cover, swipe opener, three reasons, water splash, ice capsule; video recipes such as product spin, half turn, push in, pull back, crane reveal, macro glide, label trace, light sweep, liquid wrap, paint wave, topdown dive, whip pan, impact, shadow motion, sunrise pass, texture track, detail scan, floating roll.

### 2.3 Style presets

- Soul 2.0 styles (33): General, Warm ambient, Y2K studio, Swag era, Theatrical light, Y2K street, Flash editorial, Old smartphone, Street photography, Retro BW, Asian nostalgia, Surreal solarization, Subtle flash, Digital camera, Siren, Mystique city, Candy pop, Double exposure, 2000s band, Frutiger aero, Drain, Extraterrestrial, Editorial street style, Nature light, New Indie, Underwater, 80s horror, Disposable camera, Neutral pastel film, Warm vivid film, BW film, Warm contrast film, Muted cool film.
- Personal styles: Recraft V4 Styles (user-trained), Shorts Studio style presets (user or library), explainer video style presets.
- Cinema Studio colour palettes (50 named grades) and Cinema Studio 2.5 post grading (temperature, contrast, saturation, sharpness, grain, highlights, exposure).

### 2.4 How presets combine with models

Observed mechanics, from MCP schemas and recipe instructions:

1. **Recipe preset = locked model + locked params + hidden prompt + slot rules.** Example image recipe: GPT Image 2.5 Sunburst, 3:4, 2k, high quality, product image in the `image` role, a long hidden master prompt. Users may override only AR and output count; the model is locked.
2. **Video recipe = small DAG.** Example "product spin": step 1 builds a clean start frame with Nano Banana Pro from the product photo (hidden preprocessing prompt); step 2 runs Seedance 2.0 at 1080p, 6 s, silent, high bitrate, with the step-1 image as `start_image`. The intermediate image is not shown as a deliverable.
3. **Viral chain preset = graph with typed inputs.** The execution schema exposes `media` slots (for example `input_image`, 1 item, accepts an upload or a previous generation via `image_job`), `slot_selections` (preset-provided media), `slot_values` (text slots), `choice_values` keyed per graph node (for example `<node-uuid>.resolution` 480p/720p and `<node-uuid>.aspect_ratio`), and `config`. Everything else is fixed.
4. **DoP motion preset = motion blueprint for one model.** The preset defines the camera; the prompt describes the scene. Mix stacks several presets in one clip.
5. **Preset picker is per model.** On the video page the preset gallery has a tab per model family (Seedance 2.5, Genjutsu, Katana, FLUX.3 Video, Kling 4.0, Kling 3.0, Higgsfield DoP, Veo 3.1, Wan, Grok and others). "General" means manual control with no preset.
6. **Genjutsu preset = driving video + operation.** Motion transfer (`hf_mult_motion_control`) or object swap (`hf_mult_replace_object`); 1-8 reference images; 480p-1080p; output length follows the source (4-30 s). The user's request can override the example's operation.
7. **Style presets are parameters, not prompts.** Soul `style_id` combines with a trained `soul_id` but not with image references. Genjutsu Style is `preset_id` + user video (4-30 s) + up to 5 references + optional prompt.
8. **Cinema Studio controls are typed enums.** Leaving a field empty lets the "director" pick; the API rejects the literal value "auto".
9. **Model recommendation interception.** The MCP `generate_video` can return a `preset_recommendation` instead of submitting; the agent must retry with `declined_preset_id` to keep its own choice.

---

## 3. Apps and features map

Type: **G** = core image or video generation, **E** = editing or post, **W** = workflow, studio or agent layer.

| Feature | Type | What it does | Inputs | Outputs |
|---|---|---|---|---|
| Image (create) | G | Prompt or reference image generation across about 30 models | Prompt, references, @elements, sketch (Draw), AR, resolution, batch 1-4 | Images |
| Video (create) | G | Text, image, frame and reference video generation | Prompt, references (image, video, audio), start/end frames, preset, duration, AR, quality, bitrate, audio toggle | Video |
| Soul 2.0 / Soul Cinema | G | House photo models for fashion, UGC and cinematic stills | Prompt, 1 reference, style, character | Images |
| Soul ID (Characters) | W | Trains a reusable identity in a few minutes; help center asks for 20-80 photos, MCP guidance says 5-20, API accepts 1-100 | Photos of one consenting person | Character usable in Soul models and as an Element in Seedance |
| Elements (@ mentions) | W | Saved characters, locations, props and voices referenced by @name in prompts | Uploads, past generations, Soul characters, Kling elements | Reusable references |
| Soul Cast / Soul Location | G | Text-built actors and locations for Cinema Studio | Prompt, compute budget | Character and location stills |
| AI Influencer | G + W | Builds a character sheet (human and animal tiers with trait options), then animates it via Genjutsu | Traits or preset, seed | Character sheet, videos |
| Photodump / Fashion Factory | G | Many scenes of the same character in one click | Character | Image sets |
| Popcorn (storyboard) | G + W | Up to 8 consistent scenes from one brief; export prompt to video | Optional reference, per-scene prompts | 8 keyframes |
| Cinema Studio (2.5 to 4.0) | G + W | Film-style video with camera, lens, genre, era, light, palette; projects; AI Director chat (3.5); up to 50 references (4.0) | Prompt, references, Elements, settings | Video shots, stills |
| Camera Controls (DoP) | G | Image to video with a named camera preset; Mix combines presets | 1 image, prompt, preset | 3-5 s clip |
| Effects (Viral Hub) | G | One-click trend templates | 1-3 photos, optional prompt | Short video |
| Genjutsu | G + E | Motion transfer, object swap, restyle of a source video; Lite tier on Wan 3.0 and Kling 4.0 | Driving or source video (4-30 s), 1-8 references | Video |
| Katana | W | Recreates a reference edit with the user's subjects, run through Claude ("Claude Motion") | Reference video, characters, instructions | Edited video |
| Draw to Video | G | Draws motion or changes on a frame and animates it | Video or frame, sketch, references, enhancer toggle | Video (3.5 cr per frame + per-second rate) |
| Motion Control (Kling) | E | Recasts a character into a driving clip | Character image, motion video | Video |
| Edit Video | E | Prompt edits on existing footage (Seedance 2.5 Edit, Kling Omni edit, FLUX 3 Video Edit, Grok edit) | Video, prompt, references | Video |
| Extend Video | E | Continues a clip forward or backward | Video, prompt | Longer video |
| Reframe | E | Changes a video's AR by generating new edges | 1 video (up to 60 s), optional start image, 1-2 refs | Video |
| Outpaint / Expand image | E | Extends an image beyond its border | Image, target AR or per-side pixels | Image |
| Upscale | E | Image and video upscale (Topaz, ByteDance, FLUX), FPS boost, SDR to HDR | Image or video | Higher resolution media |
| Edit (Layers) | E | Relight, inpaint, layer decomposition, edit text, effects | Image | Edited image or layers |
| Relight | E | Moves light position, colour, brightness (image app and Cinema Studio video relight) | Image or video | Relit media |
| Face Swap / Character Swap / Recast / Video Face Swap | E | Identity replacement in images or videos | Source + face or character | Media |
| Background remover (image, video) | E | Matte extraction (SAM 3 for video) | Media | Cut-out media |
| Lipsync Studio / Speak | G | Talking clips from an image or video plus speech | Image or video, text or audio, scene template | Talking video (Speak 2.0, Wan 2.5 Speak, Kling Avatars 2.0, Kling Lipsync, Infinite Talk, Sync Lipsync 3, Veo) |
| Audio | G | TTS, voice change, translate (18 languages), voice cloning | Text, audio, sample | Audio |
| Marketing Studio | W | Product ads: UGC, product shots, motion, ads, posters, marketplace, TikTok; Product Link, Ad Reference, App video | Product (or URL), avatar, brief, brand kit | Images and videos |
| Ads Studio | W | Brand kit scraped from a website, products imported, static ads at scale, influencer ads, Ad Multiplier | URL, products, brief | Ad images and videos |
| Ad Multiplier | E + W | Edits or recreates a 4-30 s ad for another audience, language, cast or product; can regenerate only hook or CTA | Source ad, images, prompt | Ad variants |
| Content Factory | W | Trending "What If" styles to ready-to-post series | Style, topic | Series of posts |
| Shorts Studio | W | Restyles a short toward a chosen look | Video, style preset | Short |
| Virality Predictor | W | Predicts hook strength and retention risk | Video | Score and analysis |
| Canvas | W | Node board chaining models, parallel runs, templates, real-time team editing; credits spent only when a node runs | Nodes, references | Any media |
| Supercomputer | W | Agent (GPT-6 Astra) running multi-step creative jobs, scheduled jobs, 5 GB storage | Instructions | Projects |
| MCP / CLI / ChatGPT plugin | W | Generation from Claude, ChatGPT, Claude Code and other agents | Natural language, tool calls | Same as web, always credit-billed |
| API | W | Async REST for developers, USD balance | JSON | Media URLs |
| Plugins | W | After Effects and Blender integrations, Chrome reference extension | Host app media | Media |
| 3D | G | Image or text to GLB, rigging, 3D scene builder, location angles | Images, text | GLB, renders |
| Websites and games builder | W | Builds sites and browser games with Higgsfield SDK | Brief | Deployed site |
| Community, Contests, Originals, Academy | W | Social gallery with prompts and assets, contests, original series, courses | n/a | n/a |

---

## 4. Create-page UX flow

### 4.1 Global shell

- Top navigation with about 25 entries (Explore, Image, Video, Audio, MCP, API, Genjutsu, Content Factory, AI Influencer, Academy, Katana, ChatGPT Plugin, Ads Studio, Effects, Cinema Studio, Contests, Marketing Studio, Supercomputer, Edit, Community, Plugins, Canvas, Originals, Pricing with a "30% OFF" badge, Enterprise). A promo bar sits above it.
- Credit balance sits in the top bar for signed-in users. Generations land in the tool's History and in a global Assets library.
- Front end: appears to be TanStack Start (React), based on `/tanstack/` asset paths and `/_serverFn/` calls; Clerk for auth; Cloudflare; a separate localisation service; internal gateway `fnf-api-gw.higgsfield.ai` (internal job types are called "job sets").

### 4.2 Image page (`/ai/image?model=<id>`)

- Empty state: centered hero, Academy hint, "View" toggle for grid density at the top right.
- Bottom-docked composer: prompt box, `+` (upload references), `@` (Elements), model chip, AR chip, resolution chip, batch stepper (1/4), Draw (sketch input), and a large Generate button that shows the credit cost (2 for Nano Banana Pro at 1K).
- Model picker: searchable popover. "Featured models" with a one-line description and badges (NEW, PREMIUM), then "All models". The model id lives in the URL, so every model state is deep-linkable.

### 4.3 Video page (`/ai/video?model=<id>`)

- Left control panel, right canvas. Tabs: Create Video, Edit Video, Motion Control.
- Preset card at the top ("GENERAL" + model name) with Change. Change opens a full-width preset gallery with one tab per model family and a search box.
- Segmented control: References or Extend Video. A single dropzone accepts image, video or audio references.
- Prompt box with an @Elements button and an audio On/Off toggle. Prompt hint suggests edit-style instructions.
- Model row opens a searchable picker. Featured rows show max resolution and duration range chips (for example "1080p, 4s-30s") and an audio icon. "All models" lists 11 families with one-line descriptions; each family expands to its versions with PREMIUM, NEW and EXCLUSIVE badges and notes such as "not available in US" or "being retired".
- Settings: Duration (chips 4, 10, 15, 20, 30 for Seedance 2.5), Aspect Ratio, Quality (resolution), Bitrate.
- Generate button with live cost (80 credits = Seedance 2.5, 1080p, 5 s, at 16 cr/s).
- Right side: History and How it works tabs.

### 4.4 Studio and app pages

- Effects: preset hero card with Change, typed upload slots (Character, Location optional, Products optional), prompt toggle, Resolution, AR, "Use free gens" toggle with remaining count, "Try in ChatGPT", Generate ("1 FREE LEFT").
- Cinema Studio: own app shell (Home, My elements, My favorites, Community, Projects). Composer chips: References 0/50, Film setup, Camera, Color palette, Lighting (all "Auto"), Image or Video switch, prompt with @ for characters and locations, model, 1080p, 16:9, 5 s, audio, batch, Generate (80). An upsell modal opens on load.
- Marketing Studio: tabs UGC, Product shot, Motion, Ads, Posters, Marketplace, TikTok; Image or Video; shot type (Closeup); AVATAR and PRODUCT slots; Ad Reference and Product Link entry points; template gallery below.
- Lipsync Studio: image upload, "Audio text" with Generate Audio, model (default Wan 2.5 Fast), duration, resolution, Generate (9).

### 4.5 Progress states, errors and history

From the help center (states shown on each generation tile):

| State | Meaning | User action |
|---|---|---|
| Queued | Waiting for a slot | Hover, Cancel (instant refund) |
| Processing | Started on the backend | Cancel may not apply |
| Generating | Running | No cancel |
| "Something went wrong" | Temporary error or skipped captcha | Hard refresh, complete captcha, retry |
| "You've hit your limit of generations at the same time" | Concurrency limit | Cancel a queued job or wait |
| "Rejected due to copyright restrictions" / "IP detected" | Copyright or real-face check | Rework prompt or reference |
| Flagged as NSFW | Content filter | Rephrase |
| Failed, credits refunded | Automatic refund, usually within minutes | Retry (Grok is charged at start) |

Credit jobs run in a priority queue. Unlimited and free generations share a slower pool. A captcha can appear before generation.

### 4.6 Credit display

- Cost on the Generate button before confirming; the total multiplies with batch size.
- Balance in the top bar; Manage Account, Usage lists every deduction and refund with model, timestamp and source (web, MCP, CLI).
- No breakdown on the button (per second, audio, references, resolution); the user only sees a total.

### 4.7 Friction points

1. Three composer layouts (left panel for video, bottom bar for image, chip bar for Cinema Studio and Marketing Studio) for the same task.
2. Navigation overload; several entry points for the same function (Edit, Layers and `/edit?model=` all land on Layers; Photodump redirects into an image-page modal).
3. Promo bars, discount badges, upsell modals on load and an agent tool that leads with "turn on auto-refill".
4. Model naming confusion: "Higgsfield Lite/Standard/Turbo" means DoP; Kling O1, Omni 1, 3.0 Omni and O3 overlap; Grok Imagine 2.0 is an image model while Grok Imagine 1.5 is video.
5. Per-model reference semantics are inconsistent (in Canvas, Kling treats a connected image as a start frame; Seedance treats it as a reference and needs the role written in the prompt).
6. Hidden costs: unlimited does not apply on MCP, CLI, Canvas or Supercomputer; batch multiplies cost; subscription credits reset each cycle.
7. Unlimited mode is slower by design, and third-party reports say the toggle can reset on mobile so users spend credits by accident (unverified).
8. Region and plan gating appear only inside the picker (Seedance 2.0 Fast not in US, std mode needs Plus).
9. Captchas and content checks can fail a job late; failures need a manual retry.
10. Output retention: API outputs expire after 7 days unless copied.

---

## 5. Pricing, credits and API

### 5.1 Plans (USD, 24.10.2026)

| Plan | Monthly price | Annual price (per month) | Credits per month | Effective USD per credit | Concurrency (video / image) |
|---|---|---|---|---|---|
| Free | 0 | 0 | 0 (limited use, watermark) | n/a | 1 / 1 |
| Starter | 19 | 19 | 270 | 0.070 | 2 / 4 |
| Plus | 59 | 47 | 1,200 | 0.049 (0.039 annual) | 6 / 8 per comparison table; plan card says "unlimited paid parallel generations" |
| Ultra | 129 | 99 | 3,000 | 0.043 (0.033 annual) | 8 / 8 |
| Ultra 6,000 | 220 | 193.50 | 6,000 | 0.037 (0.032 annual) | as Ultra (assumed) |
| Ultra 9,000 | 310 | 270 | 9,000 | 0.034 (0.030 annual) | as Ultra (assumed) |
| Team | 69 per seat | 65 per seat | 1,000 per seat, pooled | 0.069 | Shared workspace, 2-15 seats |
| Scale | 169 per seat | 150 per seat | 2,500 per seat, pooled (up to 37,500) | 0.068 (0.060 annual) | Dedicated capacity, SLA, SSO, admin spend control, priority queue |

Other credit sources:

- Top-ups (one-time, expire after 90 days): 500 cr = 29.41, 1,000 = 55.56, 2,000 = 105.26, 4,000 = 210.53 USD (shown as 31-38% off a list price).
- Auto-refill: 20 credits per USD when the balance drops below a threshold; credits expire after 90 days.
- Concurrency Boost packs: +4, +8, +12 or +16 parallel jobs with a separate Boost credit balance (90 days).
- Unlimited models: per-plan lists with 7-day windows for flagships (Nano Banana 2 and Pro at 2K, Kling 3.0 at 720p/5 s) and 365-day windows for older models (Seedream 4.5/5.0 Lite, FLUX.2 Pro 1K, Nano Banana, Kling O1 Image, GPT Image); an Unlimited Models Marketplace sells single-model unlimited access; business plans sell 30-day unlimited Seedance 2.5. Web only, slower queue, "dynamic speed adjustment" at peak.
- Free generation pools: for example 5,000 (monthly) or 10,000 (annual) Soul 2.0 and Soul Cinema generations on Ultra; 10 Genjutsu Lite, 10-30 AI Influencer, 10 Ads Studio, 3 Genjutsu generations.

Credit rules: subscription credits are spent first and reset each cycle (no rollover); other credits are spent by soonest expiry. Failed and NSFW jobs are refunded automatically. Paid plans have no visible watermark; invisible AI-content markers may still be embedded. Third-party summaries of the terms say refunds are possible within 7 days of a first purchase only if no credits were used (unverified here).

### 5.2 Representative per-clip costs

| Job | Credits | USD at 0.033-0.070 per credit | Public API USD (headline, tier not stated) |
|---|---|---|---|
| Seedance 2.5, 1080p, 5 s | 80 | 2.64-5.60 | 0.144 per s (30% promo) |
| Seedance 2.5, 720p, 5 s | 35 | 1.16-2.45 | same |
| Seedance 2.0, 720p, 5 s | 22.5 | 0.74-1.58 | 0.0985 per s |
| Kling 3.0 std, 5 s, no audio | 7.5 | 0.25-0.53 | 0.042 per s |
| Kling 4.0, 1080p, 5 s | 10-12.5 | 0.33-0.88 | 0.047 per s |
| Veo 3.1 Fast, 4 s | about 16 | 0.53-1.12 | not on API |
| Wan 3.0, 720p, 5 s | 8.75 | 0.29-0.61 | 0.02 per s |
| Genjutsu, 720p, 10 s | 70 | 2.31-4.90 | 0.159 per s (Lite 0.025 per s) |
| Nano Banana Pro image | 2 | 0.07-0.14 | not on API |
| GPT Image 2.5, high, 2k | 2.75 | 0.09-0.19 | not on API |
| Soul 2.0 image | 0.12 | 0.004-0.008 | 0.0032 |

The public cost table (`GET fnf-api-gw.higgsfield.ai/fnf/job-sets/costs`, anonymous, versioned by hash) prices by `job_set_type` with fields such as `cost_per_second`, `original_cost_per_second` (pre-promo), `resolution`, `with_video` / `without_video`, audio on/off, `video_edit`, `frame_credits`, and formula types for audio (`character_blocks`, `input_duration`, `duration_blocks`).

### 5.3 Public API (platform API)

- Hosts: docs at docs.higgsfield.ai (Mintlify, `llms.txt`, per-model `llms.txt`, supplementary `openapi.json`), keys and Playground at console.higgsfield.ai, requests to `https://api.higgsfield.ai`.
- Auth: `Authorization: Key <KEY_ID>:<KEY_SECRET>`.
- Endpoints are model and workflow specific, shaped `/<vendor>/<model>/<version>/<workflow>`, for example:
  - `POST /bytedance/seedance-2.5/reference-to-video`, `/text-to-video`, `/image-to-video`, `/video-edit`, `/video-extend`
  - `POST /kling-video/v3.0/pro/image-to-video`, `/kling-video/o3/first-last-frame`
  - `POST /higgsfield/genjutsu/motion-transfer/v1.0`, `/object-swap/v1.0`, `/restyle/v1.0` (restyle presets: `GET /models/higgsfield/genjutsu/restyle/v1.0/presets`)
  - `POST /higgsfield/cinema-studio/4.0`, `POST /higgsfield-ai/soul/v2/standard`, `POST /v1/custom-references` (Soul ID training, 1-100 images)
  - `POST /marketing-studio/image`, `/marketing-studio/image/flare`, `/sunburst` (presets: `GET /marketing-studio/image/presets`)
- Bodies use flat JSON with public URLs: `prompt`, `image_url`, `end_image_url` (names vary: `last_image_url`, `first_frame_url`), `image_urls[]`, `video_url`, `video_urls[]`, `audio_urls[]`, `duration`, `resolution`, `aspect_ratio`, `generate_audio` (Kling uses `sound: on|off`), `seed`, model-specific extras. Prompt tokens such as `<<<image_1>>>` bind references in Cinema Studio 4.0.
- Lifecycle: submit returns `{status: "queued", request_id, status_url, cancel_url, expires_after}`. Status values: `queued`, `in_progress`, `completed`, `failed`, `nsfw`, `canceled`. Completed payloads: `images[]`, `video`, `audio` / `audios[]`, plus artifacts such as zip, mov, jsx, fbx, ply.
- Reliability: `Idempotency-Key` header; polling with backoff; signed webhooks with secret rotation; cancel only while queued (`POST /requests/{id}/cancel`, 202); `DELETE /requests/{id}` removes files and parameters.
- Uploads: `POST /files/generate-upload-url` with a content type returns a presigned PUT URL (1 hour) and a `public_url`; JPEG, PNG, WebP, GIF, WAV, MP4.
- Billing: prepaid USD balance (funds expire after 1 year); `POST /estimate/<endpoint>` returns `{usd}`; failed and NSFW jobs are not charged; canceled queued jobs are refunded.
- Limits: concurrency per account and model; extra requests queue; the queue is a multiple of concurrency; overflow returns `429 account_queue_full` with `Retry-After`; `503 admission_control_unavailable` is retryable; no remaining-capacity headers yet.
- Retention: 7 days by default; `Hf-Expires-After` shortens it (minimum 1 hour); enterprise adds organisations, roles, SSO, training opt-out, capacity requests, SLAs.
- SDKs: Python `higgsfield_client.subscribe(endpoint, arguments=...)`; TypeScript `@higgsfield/client/v2` with `higgsfield.subscribe(endpoint, {input, withPolling})`.
- Gaps: no Veo, Nano Banana, GPT Image, Seedream or FLUX endpoints; LTX-2.5 and PixVerse V6 are API-only.

### 5.4 MCP and agent channel

- Hosted MCP at `https://mcp.higgsfield.ai/mcp`, also a CLI, ChatGPT plugin and Telegram entry. Uses the account plan; no API key; always charges credits.
- Tool shape: `generate_image` / `generate_video` / `generate_audio` with `params {model, prompt, medias:[{role, value: media_id|job_id}], aspect_ratio, duration, count 1-4, get_cost, use_unlim, folder_id}`; batch tools for 2-12 independent jobs; `jobs_wait` (15 s polls); `media_upload` (presigned PUT), `media_import_url`; `models_explore` (list, search, get, recommend); preset tools; bundled workflows delivered as SKILL.md files (UGC video, faceless video, product photoshoot, thumbnails, Katana, Ad Multiplier, brand assets, website builder).
- Outputs of one job can be passed as inputs to the next by job id, which makes chaining cheap for agents.

---

## 6. Do better: concrete gaps Pictesa can beat

| # | Gap at Higgsfield | Pictesa answer |
|---|---|---|
| 1 | Credit layer hides real cost; credit value varies 2x by plan; credits expire | Show provider USD per job before and after the run, with the per-second and per-option breakdown. No credits, no expiry. Export a cost ledger |
| 2 | No bring-your-own-key | BYOK per provider (fal, Replicate, Google, OpenAI, BFL, BytePlus/ModelArk, Kling, MiniMax, xAI, Alibaba DashScope, Runway, Luma, Higgsfield API itself). Route each model to the cheapest or fastest configured provider |
| 3 | API lacks flagship image models and Veo | Unified catalog across all configured providers, so one request schema reaches Nano Banana, GPT Image, Seedream, FLUX, Veo and Seedance |
| 4 | Slower shared queue for "unlimited"; queue priority sold as a feature | No artificial queue. Parallel fan-out limited only by provider limits and a user-set budget. Show live ETA from measured provider latency |
| 5 | Agents pay more than web users (no unlimited on MCP/CLI) and MCP replies contain sales copy | Same price and features on web, CLI, REST and MCP. Neutral tool output. Idempotency keys, webhooks, dry-run cost and JSON Schema per model in every channel |
| 6 | Reference roles differ per model and are undocumented in the UI | One normalized role model (start frame, end frame, image ref, video ref, audio ref, mask, element) with per-model capability flags and limits from the provider schema; validate before submit |
| 7 | Presets are closed recipes with hidden prompts | Presets as open, versioned files in `presets/` (model + params + prompt template + typed slots + optional DAG). Show and fork the prompt. Run a preset on any model that meets its capability needs |
| 8 | Camera presets locked to DoP | Camera vocabulary as an enum that maps to native controls where they exist (LTX `camera_movement`, Kling camera control, Cinema-style enums) and to tested prompt fragments elsewhere. Support Mix by combining fragments |
| 9 | No local option | Local backends via ComfyUI workflows (`workflows/comfy`) for open models (Wan 2.x, LTX-2, FLUX dev, Z-Image, Qwen-Image, HunyuanVideo); auto-detect GPU; same job API |
| 10 | Batch limited to 1-4 variants or 12 agent jobs | Matrix runs: prompts x models x seeds x settings, CSV import, side-by-side compare, pick-best, then upscale only the winners |
| 11 | Draft-then-final exists only for Seedance 2.5 | Generic draft pipeline: cheap low-resolution or turbo pass first, then the same seed and settings at final quality or through an upscaler |
| 12 | Cloud-only library; API files deleted after 7 days | Local-first asset library with full provenance (prompt, params, seed, model, provider request id, cost, parent assets), search and tags, optional C2PA. Nothing expires |
| 13 | Three composer layouts and 25-item navigation | One composer for image and video: prompt, references with roles, model chip, settings chips, cost on the button. Keyboard first. No promo surfaces |
| 14 | Failures need manual retry; Grok charges on start | Automatic retry and provider fallback with the same idempotency key; show the exact provider error; never double-charge |
| 15 | Region and plan gating appear late in the picker | Capability and availability resolved per configured key before the user writes a prompt |
| 16 | Character consistency locked to Soul ID (trained photo set, usable only in Soul models and as a Seedance Element) | Portable characters: reference sets usable by any multi-reference model, plus optional local LoRA training for open models |
| 17 | Price table is public but undocumented | Publish a machine-readable price list for every provider and model, refreshed automatically, and use it for estimates |
| 18 | Content and IP checks run server-side with opaque messages | Provider moderation errors passed through verbatim; local runs follow the user's own policy |

Suggested first build order for image and video generation: normalized model schema and capability flags, BYOK providers with live cost, the single composer, the local library with provenance, open preset files with slots, matrix batches, then MCP and CLI parity.

---

## Sources

- Higgsfield MCP connector, read-only tool calls on 24.10.2026: `models_explore` (image, video, audio, 3D), `get_presets` (viral, marketing_studio motion and product-shot, genjutsu trending and styles, soul), `get_preset_instructions` (catalog, /genjutsu, /hero-shot, /product-spin), `get_workflow_instructions` (catalog), `show_plans_and_credits` (top-up data).
- API docs: https://docs.higgsfield.ai/docs/llms.txt, /docs/models/video-generation.md, /docs/models/image-generation.md, 84 workflow pages under /docs/models/, /docs/concepts/requests.md, /docs/concepts/file-uploads.md, /docs/concepts/rate-limits.md, /docs/concepts/billing-and-retention.md, /docs/openapi.json.
- Web pages (logged out): https://higgsfield.ai/, /pricing, /ai/video, /ai/image, /generate, /camera-controls, /effects/use, /apps, /lipsync-studio, /upscale, /layers, /character, /genjutsu, /katana, /marketing-studio, /ads-studio, /canvas-intro, /storyboard-generator, /mcp, /higgsfield-api, /sora-2.
- Public JSON: https://fnf-api-gw.higgsfield.ai/fnf/job-sets/costs, https://fnf-api-gw.higgsfield.ai/fnf/subscriptions/v2/plans?plan_set_key=ps_a3, https://pricing.higgsfield.ai/v2/pricing-config.
- Help center: https://higgsfield.ai/creator-hub/help-center (credits, what uses my credits, generation stuck or failed, first generation, watermark, concurrency boost, Soul ID, Cinema Studio, Canvas, lipsync and voiceover, MCP, unlimited models, DoP).
- Third-party (low confidence, used only for labelled claims): https://weprompt.it.com/blog/higgsfield-ai/, https://techjournal.org/higgsfield-review, https://www.capcut.com/resource/higgsfield-ai-image-to-video, https://www.segmind.com/models/higgsfield-image2video, https://www.aibase.com/news/17049.
