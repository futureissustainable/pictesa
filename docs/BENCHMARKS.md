# Benchmarks

Measured on 24 and 25.10.2026 on an 8-vCPU Linux VM without a GPU. Each section names its method so it can be repeated.

## Pictesa's own overhead

`scripts/bench.sh target/release/pictesa` (free mock models, empty data folder):

| | |
|---|---|
| Binary, with the web UI inside | 18 MB |
| Cold start to the first API answer | 17 ms |
| Memory after start | 15 MB |
| `GET /api/models` | 2.7 ms average |
| Queue overhead on a job whose model takes 4,000 ms | 92 ms (4,092 ms end to end) |
| Web UI code | 436 KB, 129 KB gzipped |

## The studio page against Higgsfield's video page

`node scripts/pageload.mjs http://127.0.0.1:4747/ 36 https://higgsfield.ai/create/video`: headless Chromium at 1440×900, cache off, network emulation per profile, median of 3 cold loads. Pictesa ran locally, so it gets the measured 36 ms round trip to Higgsfield added on top of each profile; both sides pay the same latency.

| Connection | First paint, Pictesa | First paint, Higgsfield | Page loaded, Pictesa | Page loaded, Higgsfield |
|---|---|---|---|---|
| 100/20 Mbps, +10 ms | 280 ms | 500 ms | 190 ms | 2,090 ms |
| 20/5 Mbps, +40 ms | 380 ms | 512 ms | 284 ms | 2,210 ms |
| 9/1.5 Mbps, +100 ms | 636 ms | 1,240 ms | 446 ms | 3,852 ms |

The studio transfers 308 KB in 41 requests; Higgsfield's video page (logged out) 7.8 MB in 745.

## Real local models, CPU only

These ran through the full path (studio or CLI, server, provider) on the same VM without a GPU, so they show that the integrations work, not how fast a GPU is.

| Backend | Model | Result |
|---|---|---|
| stable-diffusion.cpp `sd-server` | SDXS (1 step) | two 512×512 images in 20 s |
| stable-diffusion.cpp `sd-server` | Wan 2.1 T2V 1.3B (Q4_K_M GGUF) | a 336×192 video, 9 frames, in 263 s |
| ComfyUI 0.42.1 | SD 1.5, as an imported workflow | 8 live sampler previews in the studio while it ran, 400×400 image in 72 s |
| ComfyUI 0.42.1 | Real-ESRGAN template, with an uploaded image | 2× upscale to 800×800 in 29 s |
| Automatic1111/Forge API (`a1111` provider, served by `sd-server`) | SDXS | 512×512 image in 20 s |
| OpenAI-compatible API (`openai_compatible` provider, served by `sd-server`) | SDXS | 512×512 image in 9 s |

The features added on 25.10.2026 (batch runs, variations, inpainting with a painted mask, the Zooms app, a board step, and the ffmpeg trim and frame tools on the Wan 2.1 clip above) also ran end to end on SDXS and ffmpeg through the API and MCP, 8 of 8 checks passing.

Cloud providers were checked with real keys without spending: the key checks and live catalogs of fal, OpenAI, Google and OpenRouter (1,469 models, 613 of 617 fal image and 544 of 545 fal video models priced) on 25.10.2026, with paid runs switched off.
