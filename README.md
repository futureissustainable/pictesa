# Pictesa

**The open-source studio for AI images and video.** Every major model, your own keys or your own GPU, one fast binary, built for people and for agents.

Pictesa is an open alternative to closed creative platforms like Higgsfield: the same models and the same one-prompt workflow, without credits that hide the real price, without a single company between you and the model, and with every recipe and preset open to read, change and share.

---

## What you get

- **Every model in one place.** About 1,250 image and video models through fal, Replicate and Comfy Router, plus direct APIs: OpenAI, Google, OpenRouter, xAI, ByteDance (BytePlus), Black Forest Labs, Runway, Luma, Kling, MiniMax, Alibaba, Stability AI, Ideogram and Recraft. New gateway models appear without an update: Pictesa reads their catalogs and builds each model's controls from its published schema. Each model shows its LMArena rank when people have rated it, and the picker sorts by rank, price or release date, with your pinned and recent models on top.
- **Any ComfyUI workflow becomes an app.** Drop in a workflow exported with *Export (API)*: Pictesa reads the graph, finds the prompt, seed, steps, guidance, size, video length and image inputs, and turns it into a model with proper controls in the studio, a CLI command and an MCP tool for agents. No glue code. Built-in templates cover LTX 2.5, FLUX 3 Klein, Wan 2.2, Qwen Image and upscalers, and stable-diffusion.cpp, Forge and Automatic1111 work too. See [docs/WORKFLOWS.md](docs/WORKFLOWS.md).
- **Live previews.** Local runs show their image as it renders: ComfyUI's sampler previews stream into the job card and the feed, so a run that goes wrong can be stopped early.
- **Real prices.** You pay providers directly, at their price. Models show the provider's published price (per image, second or megapixel; fal's whole catalog is priced this way), token-billed models say so, and every run records its cost estimate. No credits, no markup.
- **Recipes.** Open multi-step pipelines: *Cinema shot* (compose a still with your lens and light, then animate it with a camera move), *Character shot*, *Product hero*, *Animate a photo*, *New angle* and *Upscale*. Each step uses the first model you have set up, cloud or local, and recipes are plain JSON you can fork.
- **Characters.** Save a person, product or style as a named set of reference images and attach it to any run on any model that takes references.
- **Compare.** Run one prompt on up to eight models side by side, with parameters adapted to each.
- **Presets.** 76 camera moves and looks (dolly in, orbit, crane, FPV drone, bullet time, film stocks, light) as editable JSON.
- **Built for agents.** An MCP server (stdio and HTTP), a CLI with `--json`, and a REST API with live events. Agents and people share one library, so an image you make can go straight to an agent and back. See [docs/AGENTS.md](docs/AGENTS.md).
- **One fast binary.** Rust (Tokio, Axum, SQLite) with the web UI embedded. No Python, Docker or Node at runtime. Files are stored once by content hash, with thumbnails, range-streamed video and full-text prompt search. It starts in 17 ms, and the studio page loads in 41 requests and 308 KB where Higgsfield's video page needs 745 and 7.8 MB ([docs/BENCHMARKS.md](docs/BENCHMARKS.md)).
- **Yours or hosted.** Run it for yourself, or host it for others the way n8n works: accounts, API tokens, and optional prepaid wallets with Stripe top-ups so people can run on your provider keys.

## Quick start

1. Download the binary for your system from the releases page, or build it (below).
2. Run `pictesa`. The studio opens at http://127.0.0.1:4747.
3. Open **Settings > Providers**, paste a key (fal or OpenRouter cover the most models), or point Pictesa at your ComfyUI.
4. Write a prompt, pick a model, generate.

Try it without any key: `PICTESA_MOCK=1 pictesa` adds two free fake models that behave like real ones.

### Build from source

```bash
pnpm --dir web install && pnpm --dir web build   # the web UI, embedded at compile time
cargo build --release                            # target/release/pictesa
```

Needs Rust 1.85+ and Node 22+ to build. `ffmpeg` on the PATH enables video thumbnails and durations.

## Command line

```bash
pictesa models --output video --featured
pictesa models --task text_to_image --sort arena    # best rated first, by LMArena rank
pictesa model fal:kling/v4/image-to-video
pictesa generate "a timber cabin on a misty lake at dawn" -m fal:google/nano-banana-2.1 -p aspect_ratio=16:9 -n 4 -o ./out
pictesa generate "slow push in" -m fal:kling/v4/image-to-video -i start_frame=./out/cabin.png -p duration=5
pictesa recipe run cinema/shot -v scene="a lighthouse in a storm" -v camera=camera/crane-up
pictesa workflow import my-flow.json --name "My flow"   # any ComfyUI workflow becomes a model
pictesa keys set fal api_key        # reads the key from stdin; stored encrypted
```

When the studio is running, CLI jobs run there and appear live in the UI.

## Agents

```bash
claude mcp add pictesa -- pictesa mcp          # Claude Code
```

Tools: `list_models`, `get_model`, `list_presets`, `list_recipes`, `list_characters`, `generate`, `run_recipe`, `get_job`, `wait_for_job`, `cancel_job`, `list_assets`, `get_asset`, `import_asset`, `import_workflow`. `generate` waits for the result and returns file paths. Setup for Codex, Cursor and HTTP clients is in [docs/AGENTS.md](docs/AGENTS.md); the HTTP API is in [docs/API.md](docs/API.md).

## Hosting Pictesa for others

```bash
pictesa serve --hosted --host 0.0.0.0 --public-url https://studio.example.com
```

Hosted mode adds sign-up, sessions and personal API tokens. Users can bring their own keys (stored encrypted per user). To let them run on your keys instead, set `PICTESA_BILLING=1`, `PICTESA_MARKUP` (default 1.25), `STRIPE_SECRET_KEY` and `STRIPE_WEBHOOK_SECRET`: runs are charged from a prepaid wallet at provider price times the markup, failed runs are refunded automatically, and `pictesa billing grant` adds allowances. Your environment keys are never used for free in hosted mode.

## How it fits together

```
 browser UI ─┐
 CLI ────────┼── HTTP API + events ── engine ── job queue ── providers ── fal, OpenAI, Google, Runway, ...
 MCP agents ─┘        (axum)          (tokio)    (SQLite)       │        ComfyUI, stable-diffusion.cpp
                                         │                      │
                                  library: files by hash, thumbnails, prompt search
```

- `crates/pictesa-core`: model specs, the provider contract, schema reading.
- `crates/pictesa-providers`: one module per provider.
- `crates/pictesa-engine`: storage, queue with retries and restart recovery, files, catalog, recipes, accounts, billing.
- `crates/pictesa-server`: HTTP API, events, files, MCP, embedded UI.
- `crates/pictesa-cli`: the `pictesa` binary.
- `web/`: the studio UI (React, TypeScript, Tailwind).

Adding a provider is one file: see [docs/PROVIDER-GUIDE.md](docs/PROVIDER-GUIDE.md).

## Privacy

Pictesa has no telemetry. Keys are encrypted at rest with a per-install key and never sent back to the browser. Your files stay in your data folder (`pictesa doctor` shows where); prompts and inputs go only to the provider that runs them.

## License

[AGPL-3.0](LICENSE). You can use, change and self-host Pictesa freely. If you offer a modified version to others over a network, you share your changes under the same license.
