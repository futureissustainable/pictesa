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
```

```json
// Cursor, Windsurf, Zed and other MCP clients
{ "mcpServers": { "pictesa": { "command": "pictesa", "args": ["mcp"] } } }
```

Remote or shared, over Streamable HTTP: point the client at `http://127.0.0.1:4747/mcp` (add `Authorization: Bearer <token>` when the server requires one; hosted users create tokens in Settings > Agents).

Tools: `list_collections`, `add_to_collection`, `list_models`, `get_model`, `list_presets`, `list_characters`, `list_recipes`, `run_recipe`, `generate`, `get_job`, `wait_for_job`, `cancel_job`, `list_assets`, `get_asset`, `import_asset`, `import_workflow`.
Prompts (skill packs, see below): `ugc_ad`, `hook_variants`, `explainer`, `faceless_short`, `brand_kit`.
`list_models` returns each model's LMArena rank per task when people have rated it; pass `sort: "arena"` with a `task` to get the best rated first, or `sort: "newest"`.
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
