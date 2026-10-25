# Pictesa

**The open-source studio for AI images and video.** Every major model, your own keys or your own GPU, one fast binary, built for people and for agents.

Pictesa is an open alternative to closed creative platforms like Higgsfield: the same models and the same one-prompt workflow, without credits that hide the real price, without a single company between you and the model, and with every recipe and preset open to read, change and share.

---

## What you get

- **Every model in one place.** About 1,250 image and video models through fal, Replicate and Comfy Router, plus direct APIs: OpenAI, Google, OpenRouter, xAI, ByteDance (BytePlus), Black Forest Labs, Runway, Luma, Kling, MiniMax, Alibaba, Stability AI, Ideogram and Recraft. New gateway models appear without an update: Pictesa reads their catalogs and builds each model's controls from its published schema. Each model shows its LMArena rank when people have rated it, and the picker sorts by rank, price or release date, with your pinned and recent models on top.
- **Any ComfyUI workflow becomes an app.** Drop in a workflow exported with *Export (API)*: Pictesa reads the graph, finds the prompt, seed, steps, guidance, size, video length and image inputs, and turns it into a model with proper controls in the studio, a CLI command and an MCP tool for agents. No glue code. Built-in templates cover LTX 2.5, FLUX 3 Klein, Wan 2.2, Qwen Image and upscalers, and stable-diffusion.cpp, Forge and Automatic1111 work too. See [docs/WORKFLOWS.md](docs/WORKFLOWS.md).
- **Live previews.** Local runs show their image as it renders: ComfyUI's sampler previews stream into the job card and the feed, so a run that goes wrong can be stopped early.
- **Real prices.** You pay providers directly, at their price. Models show the provider's published price (per image, second or megapixel; fal's whole catalog is priced this way), token-billed models say so, and every run records its cost estimate. No credits, no markup.
- **Apps.** One-click tools with a picture and one line each: *Shots* (nine different shots of a scene from one photo), *Angles*, *Zooms*, *Relight*, *Face swap*, *Product placement*, *Color grade* and *Upscale*, plus pipelines such as *UGC ad*, *Hook variants*, *Explainer*, *Cinema shot*, *Character shot*, *Product hero* and *Animate a photo*. Each app is a recipe: plain JSON you can fork, where each step uses the first set-up model that fits, cloud or local, and says what to set up when none does. A step can run once per item and return a set.
- **Edit any image.** Paint over a part and describe what goes there, expand the frame to a new shape, relight it or remove the background, with the models you have set up. Results land in the library next to the image they came from, ready to edit again.
- **Edit video.** Trim, join up to ten clips, grab a frame, smooth to twice the frame rate or add music with the free built-in ffmpeg provider. Extend, upscale and create sound with compatible cloud models you have set up. Open a video and choose **Edit video**, or pick it from **Edit**. Results stay in the library with their inputs, ready to keep editing.
- **Characters.** Save a person, product or style as a named set of reference images and attach it to any run on any model that takes references, or train a LoRA for compatible models.
- **Compare.** Run one prompt on up to eight models side by side, with parameters adapted to each.
- **Boards.** A canvas for moodboards and chained steps: add files, notes and steps, connect them and run. Results land next to their step, and a step's results can feed the next one. Share a board in hosted mode and edit it live with others and with agents.
- **Presets.** 76 camera moves and looks (dolly in, orbit, crane, FPV drone, bullet time, film stocks, light) as editable JSON.
- **Built for agents.** An MCP server (stdio and HTTP), a CLI with `--json`, and a REST API with live events. Agents and people share one library, so an image you make can go straight to an agent and back. See [docs/AGENTS.md](docs/AGENTS.md).
- **One fast binary.** Rust (Tokio, Axum, SQLite) with the web UI embedded. The studio needs no Python, Docker or Node at runtime; optional ai-toolkit training uses its Python installation. Files are stored once by content hash, with thumbnails, range-streamed video and full-text prompt search. It starts in 17 ms, and the studio page loads in 41 requests and 308 KB where Higgsfield's video page needs 745 and 7.8 MB ([docs/BENCHMARKS.md](docs/BENCHMARKS.md)).
- **Characters and brand kits.** Save a person, product or style as a named set of reference images and attach it to any run on any model that takes references. A brand kit adds brand colors and words to every step it is used in.
- **Compare.** Run one prompt on up to eight models side by side, with parameters adapted to each.
- **Presets.** 87 camera moves, looks and color grades (dolly in, orbit, crane, FPV drone, bullet time, film stocks, teal and orange, bleach bypass) as editable JSON.
- **Built for agents.** An MCP server (stdio and HTTP), a CLI with `--json`, and a REST API with live events. Skill packs give agents ready-made workflows: a UGC ad, hook variants, an explainer, a faceless short and a brand kit. Agents and people share one library, so an image you make can go straight to an agent and back. See [docs/AGENTS.md](docs/AGENTS.md).
- **One fast binary.** Rust (Tokio, Axum, SQLite) with the web UI embedded. No Python, Docker or Node at runtime. Files are stored once by content hash, with thumbnails, range-streamed video and full-text prompt search. It starts in 17 ms, and the studio page loads in 41 requests and 308 KB where Higgsfield's video page needs 745 and 7.8 MB ([docs/BENCHMARKS.md](docs/BENCHMARKS.md)).
- **Yours or hosted.** Run it for yourself, or host it for others the way n8n works: accounts, API tokens, and optional prepaid wallets with Stripe top-ups so people can run on your provider keys.

## Quick start

1. Download the binary for your system from the [releases page](https://github.com/futureissustainable/pictesa/releases/latest) (Windows, macOS, or Linux with glibc 2.34 or newer, such as Ubuntu 22.04, Debian 12 and RHEL 9), or build it (below).
2. Run `pictesa`. The studio opens at http://127.0.0.1:4747.
3. Open **Settings > Providers**, paste a key (fal or OpenRouter cover the most models), or point Pictesa at your ComfyUI.
4. Write a prompt, pick a model, generate.

Try it without any key: `PICTESA_MOCK=1 pictesa` adds free fake models (image, video and a background remover) that behave like real ones.

Or run the Docker image, which includes ffmpeg:

```bash
docker run -p 4747:4747 -v pictesa:/data -e PICTESA_TOKEN=change-me ghcr.io/futureissustainable/pictesa
```

### Build from source

```bash
pnpm --dir web install && pnpm --dir web build   # the web UI, embedded at compile time
cargo build --release                            # target/release/pictesa
```

Needs Rust 1.85+ and Node 22+ to build. `ffmpeg` and `ffprobe` on PATH enable video thumbnails, durations and free local video tools.

## Command line

```bash
pictesa models --output video --featured
pictesa models --task text_to_image --sort arena    # best rated first, by LMArena rank
pictesa model fal:kling/v4/image-to-video
pictesa generate "a timber cabin on a misty lake at dawn" -m fal:google/nano-banana-2.1 -p aspect_ratio=16:9 -n 4 -o ./out
pictesa generate "slow push in" -m fal:kling/v4/image-to-video -i start_frame=./out/cabin.png -p duration=5
pictesa generate "" -m local:trim -i video=./clip.mp4 -p start=1 -p end=4
pictesa generate "" -m local:join -i video=./a.mp4 -i video=./b.mp4
pictesa generate "" -m local:frame -i video=./clip.mp4 -p time=2 -o ./frames
pictesa generate "" -m local:smooth -i video=./clip.mp4
pictesa generate "" -m local:add-audio -i video=./clip.mp4 -i audio=./music.wav -p mode=mix -p volume=0.5 -p fade_in=1 -p fade_out=1
pictesa recipe run cinema/shot -v scene="a lighthouse in a storm" -v camera=camera/crane-up
pictesa recipe run photo/shots -i photo=./cabin.png -o ./shots       # nine shots of one scene
pictesa recipe run video/explainer -v scenes="$(cat scenes.txt)" --character "Oak and Co"
pictesa pack ugc_ad -a product="a ceramic pour-over set" -a product_image=./set.png   # a skill pack's steps
pictesa import result.png --prompt-file prompt.txt --origin gemini-api --model google:gemini-3-pro-image --label "Nano Banana Pro" -p resolution=2K --json
pictesa workflow import my-flow.json --name "My flow"   # any ComfyUI workflow becomes a model
pictesa keys set fal api_key        # reads the key from stdin; stored encrypted
```

`pictesa recipe list` shows every app and whether it can run; `recipe run` takes `-v key=value` (a JSON list for a field with several choices), `-i input_key=path` and `--character <id or name>` for a character or brand kit on every step. `pictesa pack` lists the skill packs, and `pictesa pack <name> -a key=value` prints one as steps an agent can follow.

When the studio is running, CLI jobs and imports run there and appear live in the UI. Otherwise imports use the local library. `pictesa import <file>...` accepts `--prompt TEXT` or `--prompt-file FILE`, `--negative`, `--model`, `--label`, `--origin`, repeatable `-p key=value`, `--source-url`, and `--json`. Reimporting the same file fills missing metadata without replacing existing values.

OpenAI and Gemini image models offer **Speed: Batch** (`-p speed=batch`): half price, with a turnaround of up to 24 hours. Accepted jobs resume after restarts and can be canceled in the studio.

Local video inputs are limited to ten minutes, 4K and 120 fps, with a 128 MiB output and ten-minute processing limit. Smooth frames accepts up to 60 fps, uses CPU interpolation and can be slow. Join normalizes size, frame rate and audio; Add audio pads or cuts the track to the video. All models, parameters and limits are in [docs/API.md](docs/API.md#video-tools). Recipes, MCP and CLI use the same models and queue.

## Agents

```bash
claude mcp add pictesa -- pictesa mcp          # Claude Code
```

Tools: `train_lora`, `get_training`, `list_loras`, `list_models`, `get_model`, `list_presets`, `list_recipes`, `list_characters`, `generate`, `run_recipe`, `get_job`, `wait_for_job`, `cancel_job`, `list_assets`, `get_asset`, `import_asset`, `import_workflow`, `list_boards`, `create_board`, `get_board`, `add_to_board`, `run_board_step`. `generate` waits for the result and returns file paths. Prompts (skill packs): `ugc_ad`, `hook_variants`, `explainer`, `faceless_short`, `brand_kit`. Setup for Codex, Cursor and HTTP clients is in [docs/AGENTS.md](docs/AGENTS.md); the HTTP API is in [docs/API.md](docs/API.md).

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

### Prompt memory, templates and batch runs

Use **Prompt history** beside the composer to search past prompts, or type at least three characters for up to three suggestions. Arrow keys select a suggestion, Enter inserts it and Escape dismisses it. **More settings > Save prompt** stores a name, prompt, negative prompt and optional model/settings. **Prompts** searches, inserts, renames and deletes saved prompts. Templates such as `a {material} cabin at {place}` ask for each value inline before insertion.

**More settings > Batch** combines prompt lines (or CSV first column), a seed sweep and up to three parameter grids. The studio validates every combination and shows the run count and estimated total before **Run batch**. A batch has at most 64 runs, all committed together under one batch ID. Results group in the feed. **Variations** on a finished card or in the viewer makes four new results with the same prompt and settings.

```bash
pictesa prompts history -q cabin
pictesa prompts list
pictesa prompts save "Cabin" "a {material} cabin at dawn" -m mock:image -p aspect_ratio=16:9
pictesa generate -m mock:image --prompts-file prompts.txt --seeds 42:4 --grid aspect_ratio=1:1,16:9 --dry-run --json
pictesa generate -m mock:image --prompts-file prompts.csv --no-wait --json
```

`.csv` files use the first column, without a header; other files use prompt lines. `--seeds START:COUNT` sweeps consecutive seeds. Repeat `--grid key=a,b` for more axes, or use a JSON array for values containing commas. Remove `--dry-run` to queue. All normal model validation, budgets and wallet checks apply. MCP tools: `list_prompt_history`, `list_saved_prompts`, `save_prompt`, `generate_batch`. See [API.md](docs/API.md) and [AGENTS.md](docs/AGENTS.md) for formats and template rules.

### Character training

Open **Characters > Train LoRA**, or **LoRAs > Train a LoRA**, to train from a saved character or selected library images. Choose a trigger word, compatible base family and a Quick, Balanced or Detailed preset. The studio validates the dataset and checks estimated cost before **Start training**. Training has live progress, cancel and retry. Weights appear on the character and in **LoRAs**, with a download link. Compatible models show a LoRA picker and strength slider in Create; the trigger word is added when missing.

fal trainers come from its active catalog and published schemas. Local training runs [ostris/ai-toolkit](https://github.com/ostris/ai-toolkit) when `PICTESA_AI_TOOLKIT_PATH` points to its installation or an executable wrapper. Install its Python dependencies and GPU prerequisites yourself. Pictesa writes the config and captions, launches the process and saves the final `.safetensors` file. Local training is free and limited to one run at a time; fal training is paid and follows the existing budgets and wallet checks. No GPU training is bundled with Pictesa.

```bash
pictesa train --trainer ai-toolkit:flux1 --character CHARACTER_ID --trigger-word maya_person --family flux1 --dry-run --json
pictesa train --trainer ai-toolkit:flux1 --character CHARACTER_ID --trigger-word maya_person --family flux1 --steps 1000 --rank 16 --learning-rate 0.0001 --json
pictesa train --trainer fal:fal-ai/qwen-image-2512-trainer --asset IMAGE_A --asset IMAGE_B --asset IMAGE_C --trigger-word my_style --family qwen_image_2512 --no-wait --json
pictesa generate "at a lake at dawn" -m fal:fal-ai/flux-lora -p 'loras=[{"asset_id":"LORA_ASSET_ID","strength":0.8}]' --json
```

`train` uses the running server when available, otherwise runs the local engine and waits. `--no-wait` requires a running server. `--dry-run` validates and estimates without queuing. Trainer defaults: 1000 steps, rank 16 and learning rate 0.0001; fal controls fixed by the provider keep their own defaults. The API and MCP also support training, status and LoRA listing. Shared local ComfyUI and stable-diffusion.cpp directories enable generation with saved weights; see [setup, supported families and limits](docs/API.md#character-training-and-loras).
