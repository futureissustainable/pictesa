# Using Pictesa from agents

Pictesa exposes the same capabilities to agents three ways. All of them share the library, so anything an agent makes shows up in the studio, and anything you make can be handed to an agent by id.

## 1. MCP (recommended)

Local, over stdio (no server needed):

```bash
# Claude Code
claude mcp add pictesa -- pictesa mcp

# Codex (~/.codex/config.toml)
[mcp_servers.pictesa]
command = "pictesa"
args = ["mcp"]
# `codex exec` cannot ask before a tool runs, so it refuses Pictesa's tools unless they are
# pre-approved. Set a budget cap in Pictesa first when paid models are configured.
default_tools_approval_mode = "approve"
```

```json
// Cursor, Windsurf, Zed and other MCP clients
{ "mcpServers": { "pictesa": { "command": "pictesa", "args": ["mcp"] } } }
```

Remote or shared, over Streamable HTTP: point the client at `http://127.0.0.1:4747/mcp` (add `Authorization: Bearer <token>` when the server requires one; hosted users create tokens in Settings > Agents).

`generate` waits for its result, and a local video can take a few minutes. Give the client a long tool timeout so it waits instead of giving up and submitting again; for Claude Code, `MCP_TOOL_TIMEOUT=1200000 claude`. If a call does time out, the job keeps running: call `wait_for_job` with its id.

Tools: `list_characters`, `list_recipes`, `run_recipe`, `import_workflow`, `train_lora`, `get_training`, `list_loras`, `list_collections`, `add_to_collection`, `list_models`, `get_model`, `list_presets`, `generate`, `get_job`, `wait_for_job`, `cancel_job`, `list_assets`, `get_asset`, `import_asset`, `list_boards`, `create_board`, `get_board`, `add_to_board`, `run_board_step`.
`list_models` lists set-up models first and, among them, free local models (ComfyUI, stable-diffusion.cpp) before paid ones. Each model carries its LMArena rank per task when people have rated it; pass `sort: "arena"` with a `task` to get the best rated first, or `sort: "newest"`.
`generate` waits for the result by default and returns local file paths, so an agent can open the image right away. Pass `include_preview: true` to also receive small JPEG previews for vision models.

### Apps (recipes)

The studio's Apps are recipes, so agents run the same tools: `list_recipes` returns each app with its fields, input files, steps, the model each step would use and, for a step without one, `needs` (what the user should set up). `run_recipe { id, values, inputs, models?, character? }` runs one and waits:

- `values` fills the fields. A `multi` field takes a list, for example `{"angles": ["view from behind", "low angle looking up"]}`.
- `inputs` maps each input key to files as `{path|url|asset_id}`, for example `{"photo": [{"asset_id": "01J..."}]}`.
- `character` (id or name from `list_characters`) applies a character or brand kit to every step.
- The answer has `run`, `steps` (every job) and `results`: the final files. A step that runs once per item returns a set, such as the nine shots of `photo/shots` or one clip per line of `ads/hook-variants`.

| App | id | Asks for |
|---|---|---|
| Shots | `photo/shots` | `photo`. Returns nine different shots. |
| Angles | `photo/angles` | `photo`, `angles` (several). One image per angle. |
| Zooms | `photo/zooms` | `photo`. Returns close, medium and wide. |
| Relight | `photo/relight` | `photo`, `direction`, `mood`, optional `light` |
| Face swap | `photo/face-swap` | `face` and `target` |
| Product placement | `product/placement` | `product`, and a `scene` photo or a `where` description |
| Color grade | `photo/color-grade` | `photo`, `look` (a `grade/*` preset) |
| UGC ad | `ads/ugc` | `product`, optional `presenter` photos, `line`, `setting` |
| Hook variants | `ads/hook-variants` | `frame`, `hooks` (one per line) |
| Explainer | `video/explainer` | `scenes` (one per line), `style`, `aspect_ratio` |

### Skill packs

Skill packs are MCP prompts: ready-made workflows that say which Pictesa tools to call, in which order, with which arguments. Call `prompts/list` to see them and `prompts/get { name, arguments }` to get the steps for your task. Arguments are text; a missing, unknown or invalid argument is refused with error -32602 and a message that names it.

| Prompt | What it makes | Arguments (required in bold) |
|---|---|---|
| `ugc_ad` | A phone video of a person showing a product (runs `ads/ugc`) | **product**, **product_image**, audience, presenter, brand, length (5 or 10) |
| `hook_variants` | Several openings for one video, one clip each (runs `ads/hook-variants`) | **video**, **frame**, count (2 to 8), length, brand |
| `explainer` | Script, scenes, images and clips in order (runs `video/explainer`) | **topic**, audience, scenes (3 to 12), shape (16:9, 9:16, 1:1), style, brand |
| `faceless_short` | A vertical short with no presenter, plus a voiceover when a speech model is set up | **topic**, length (15 to 60 seconds), style, brand |
| `brand_kit` | Anything, with a brand kit on every step | **brand**, **brief** |

File arguments (`product_image`, `frame`) take a library asset id, an https URL or, in local mode, a path. `presenter` and `brand` take a character or brand kit by id or name; the returned steps already contain the ids and file inputs to pass on. Packs are JSON like recipes (`packs/packs.json`; your own go in the `packs/` folder of the data folder). The CLI prints the same steps: `pictesa pack` lists them and `pictesa pack ugc_ad -a product="..." -a product_image=./set.png` prints one.

### Brand kits

A brand kit is a character of kind `brand`: reference images plus brand colors and words. Pass its id as `character` to `generate` or `run_recipe`. Its images join every step whose model takes image references, and this line joins every image and video prompt, also on models without image references: `Brand look: <words>. Brand colors: <colors>. Keep the look of the brand reference images.` The line is stored with the prompt, so the library shows what was asked. `list_characters` returns `kind`, `colors`, `words` and `image_ids`.

## 2. CLI

```bash
pictesa models --output video --featured           # what can I use?
pictesa models --task text_to_video --sort arena   # best rated first (#n is the LMArena rank)
pictesa model fal:kling/v4/image-to-video          # its inputs and parameters
pictesa generate "a timber cabin in fog at dawn" -m fal:google/nano-banana-2.1 -p aspect_ratio=16:9 -n 4 -o ./out
pictesa generate "slow push in" -m fal:kling/v4/image-to-video -i start_frame=./out/cabin.png -p duration=5 --json
```

`--json` prints machine-readable results; the exit code is non-zero when a generation fails. When a server is running, CLI jobs run there and appear live in the studio.

## 3. HTTP API

See [API.md](API.md). Events stream from `GET /api/events` (server-sent events), so agents can follow progress without polling.

## Costs

Generations on paid providers cost money. Every model lists its published price (`pricing`), and every job records an estimate (`cost_usd`). Local models (ComfyUI, stable-diffusion.cpp) are free to run.

## Save results from other tools

Save images made in ChatGPT, Gemini API scripts or another studio with their original prompt and settings:

```bash
pictesa import result.png --prompt-file prompt.txt --origin gemini-api --model google:gemini-3-pro-image --label "Nano Banana Pro" -p resolution=2K --json
```

Pass multiple files to apply the same metadata to each. Use `--prompt` instead of `--prompt-file` for short text. Optional flags: `--negative TEXT`, `--source-url URL`. The CLI uses the running server when reachable, else the local engine. JSON output is `{ "assets": [Asset] }`.

MCP `import_asset` accepts `path` (local mode) or `url`, plus `prompt`, `negative_prompt`, `origin`, `model`, `model_label`, `params` (object) and `source_url`. It returns the saved asset with its metadata. The library stores duplicate content once per user and only fills missing metadata on reimport. A catalog model ID enables **Reuse settings**; an unknown ID or a display label still records how the file was made. Imported prompts are searchable with `list_assets`.

For nonurgent OpenAI or Gemini image generation, pass `params: { "speed": "batch" }` or CLI `-p speed=batch`. Batch costs half the standard price and may take up to 24 hours. Use `get_job` or `wait_for_job` again when the wait times out; do not resubmit accepted jobs.

## File outputs into collections

- `list_collections {}` returns your collections as `{ collections: [{ id, name, created_at, asset_count }] }`.
- `add_to_collection { collection_id, asset_ids: ["..."] }` adds 1 to 100 of your assets to a collection and returns `{ ok: true }`. Adding the same asset twice is safe. Assets may belong to several collections.

Create or rename collections in the studio's Collections section or through the HTTP API. Call `list_collections`, generate or import assets, then call `add_to_collection` with their library IDs. Hosted agents can only list and change their own collections and assets.

Settings > Spend shows recent costs and optional daily and monthly budgets. All generation paths, including MCP, CLI, comparisons and recipe steps, check the caps before queuing. A refusal names the cap and remaining USD. Local runs count as zero; known wallet charges include markup; direct-key runs use estimates. See [API.md](API.md) for spend, tags, filters and ZIP routes.

## Prompt memory and batch runs

- `list_prompt_history { q?, limit? }` returns distinct personal job prompts with the latest model and use time, newest first (up to 100).
- `list_saved_prompts { q?, limit? }` returns saved prompts, optional model/settings and template placeholder names.
- `save_prompt { name, prompt, negative_prompt?, model?, params? }` saves a personal prompt. `{identifier}` marks a template field; `{{` and `}}` escape braces. To render a template, call `POST /api/prompts/{id}/render { values: {...} }`, then use the returned content for generation.
- `generate_batch` accepts normal generation fields plus `prompts: [string]` or `prompt_list` with `prompt_format: "lines"|"csv"`, `seeds: { start, count }`, `grid: [{ key, values: [...] }]` and `dry_run`. Call with `dry_run: true` to check count, estimated total USD and available funds before queuing. Set false to commit. Returns batch/job IDs; use `wait_for_job` for each ID.

Batches expand prompts × seeds × grid, with up to 3 grid axes and 64 runs total. Each run requests one output. Order: prompts, ascending seeds, then grid (last axis fastest). Every run uses ordinary model validation. A batch that exceeds a budget or wallet queues nothing. Estimates can be null for unpriced models and can change before commit. CSV uses the first column without a header; lines keep punctuation intact. Prompt memory is private to the authenticated caller.

CLI equivalents:

```bash
pictesa prompts history -q cabin
pictesa prompts list -q portrait
pictesa prompts save "Cabin template" "a {material} cabin at {place}" -m mock:image -p aspect_ratio=16:9
pictesa generate -m mock:image --prompts-file prompts.txt --seeds 42:4 --grid aspect_ratio=1:1,16:9 --dry-run --json
pictesa generate -m mock:image --prompts-file prompts.csv --no-wait --json
```

A `.csv` file uses CSV first-column parsing; other files use one prompt per line. Repeated `--grid key=a,b` adds axes; `--grid 'key=["a,b","c"]'` keeps commas inside values. `--seeds START:COUNT` starts a consecutive seed sweep. `--dry-run` queues nothing. Commands use the running server when reachable, else the local engine; prompt commands always print JSON.

## Local video tools

`list_models` includes the free `local:trim`, `local:join`, `local:frame`, `local:smooth` and `local:add-audio` models when ffmpeg/ffprobe are installed. Call `get_model` for inputs and ranges, then use `generate` or a recipe as usual; no new tool is required. Send 2 to 10 ordered `video` inputs for join. Frame returns an image. Smooth doubles the frame rate and is slow on CPU. Cloud catalog models cover extension, video upscaling and sound; `list_models` also accepts task `video_to_audio`. See [video tool parameters and limits](API.md#video-tools).

## Work on boards

Boards are shared canvases of files, notes and steps that chain generations. People who have a board open see an agent's changes at once; over stdio MCP the running studio picks them up within about two seconds.

- `list_boards {}` returns the boards you own or that were shared with you.
- `create_board { name }` returns `{ id, name, version }`.
- `get_board { id }` returns the nodes, edges, the latest run of each step and the files on the board (`assets`, with local paths in local mode).
- `add_to_board { board_id, nodes, edges }` adds items. A node is `{ kind: "asset", asset_id | url | path }`, `{ kind: "note", text }` or `{ kind: "step", model, prompt, params, count }`, with optional `id`, `x`, `y` and `w`. Give new nodes an `id` to connect them in the same call. An edge `{ from, to, role? }` from a file or a step into a step feeds it; `role` picks the model's slot (`reference`, `start_frame`, `end_frame`, `video`, `audio`, `mask`), otherwise the first free one. Nodes without a position go in a row below the board's content.
- `run_board_step { board_id, node_id, downstream?, wait?, timeout_seconds? }` runs a step as an ordinary generation, so budgets and the wallet apply and paid providers cost money. `downstream: true` also runs every step that takes its results, in order. It waits by default and returns the runs and the new files; the results land on the board, linked to their step.

```json
{ "board_id": "01J...", "nodes": [
    { "id": "still", "kind": "step", "model": "fal:fal-ai/flux/dev", "prompt": "a lighthouse in a storm" },
    { "id": "clip", "kind": "step", "model": "fal:kling/v4/image-to-video", "prompt": "slow push in" } ],
  "edges": [ { "from": "still", "to": "clip", "role": "start_frame" } ] }
```
Then `run_board_step { "board_id": "01J...", "node_id": "still", "downstream": true }` makes the still, animates it and puts both on the board. Hosted agents act as their user: they see that user's boards and the boards shared with them, and as a viewer they cannot change or run anything.

## Train and use a character LoRA

- `list_models { output: "lora" }` discovers configured local and active fal trainers; `get_model` reads the trainer's full schema and price. Local ai-toolkit is free. fal training costs money.
- `train_lora { trainer, character?, asset_ids?, trigger_word, family, steps?: 1000, rank?: 16, learning_rate?: 0.0001, dry_run?: false }` validates and queues a normal training job. Send a character or picked library image IDs. Use `dry_run: true` first for validation, estimated cost and budget/wallet checks without reserving funds. Unknown cost stays null. At least 3 distinct images of 256 px per side are required. Read the [full limits and provider setup](API.md#character-training-and-loras).
- `get_training { id }` returns the job, progress and weights asset. `wait_for_job`, `cancel_job` and the normal retry API also apply. Do not resubmit an accepted fal training job after a timeout.
- `list_loras { family?, character? }` lists your weights assets, character links, family and trigger words. They are private and excluded from ordinary image lists.
- `generate { model, prompt, params: { loras: [{ asset_id, strength: 0.8 }] } }` uses compatible weights. The engine adds a missing trigger and refuses wrong-family, trashed or unowned assets. Model metadata `supports_loras` and `lora_family` describe compatibility. fal receives its published `loras` shape; local servers require a configured shared LoRA directory.

Families: `flux1`, `qwen_image`, `qwen_image_2512`, `wan21`. Local training supports `flux1`, `qwen_image`, `wan21`, configured with `PICTESA_AI_TOOLKIT_PATH` or Settings > Providers. Local subprocess training and shared-directory writes are available only in local mode. MCP training returns immediately with job IDs so agents can follow progress, cancel or return later.
