# Pictesa HTTP API (v1)

One Rust binary serves the web UI, this JSON API, server-sent events, files and an MCP endpoint.
Default address: `http://127.0.0.1:4747`. All bodies are JSON unless noted. Times are Unix milliseconds.

## Auth

- **Local mode** (default): no login. When `PICTESA_TOKEN` is set, API and MCP calls need `Authorization: Bearer <token>`; the bundled UI gets a cookie automatically from the same origin.
- **Hosted mode** (`pictesa serve --hosted`): users sign up and log in; the session is an HttpOnly cookie. Agents use personal API tokens as `Authorization: Bearer pct_...`.

| Method | Path | Body | Answer |
|---|---|---|---|
| GET | `/api/auth/me` | | `{ "mode": "local"\|"hosted", "user": User\|null }` |
| POST | `/api/auth/signup` | `{ email, password, name? }` | `User` (hosted only) |
| POST | `/api/auth/login` | `{ email, password }` | `User` |
| POST | `/api/auth/logout` | | 204 |
| GET | `/api/tokens` | | `{ tokens: [{ id, name, created_at, last_used_at }] }` |
| POST | `/api/tokens` | `{ name }` | `{ id, name, token }` (token shown once) |
| DELETE | `/api/tokens/{id}` | | 204 |

`User`: `{ id, email, name, role: "admin"|"user", created_at }`

## Health

`GET /api/health` → `{ ok: true, version, mode, ffmpeg: bool, data_dir }`

## Providers

| Method | Path | Body | Answer |
|---|---|---|---|
| GET | `/api/providers` | | `{ providers: [ProviderState] }` |
| PUT | `/api/providers/{id}` | `{ values: { api_key?, base_url?, ... } }` | `ProviderState` |
| DELETE | `/api/providers/{id}` | | 204 (forget stored values) |
| POST | `/api/providers/{id}/check` | | `{ ok: bool, error?: string }` |

`ProviderState`:
```json
{
  "id": "fal", "name": "fal", "kind": "gateway",
  "fields": [{ "key": "api_key", "label": "API key", "env": ["FAL_KEY"], "secret": true, "optional": false }],
  "signup_url": "https://fal.ai/dashboard/keys", "docs_url": "...",
  "configured": true,
  "values": { "api_key": { "set": true, "source": "stored"|"env"|"default" }, "base_url": { "set": true, "value": "http://127.0.0.1:8188", "source": "default" } },
  "model_count": 42
}
```
Secret values are never returned, only `set` and `source`.

## Models

`GET /api/models?output=image|video|audio&task=text_to_video&provider=fal&q=kling&configured=true&sort=arena`
→ `{ models: [ModelSpec & { configured: bool, arena?: [ArenaScore], released?: "YYYY-MM-DD" }] }`

`sort`: `arena` (best rated first, in the arena of `task` when given), `newest` (by `released`) or `name`. Without it, set-up and featured models come first.

`arena` holds the model's standing on LMArena's human-preference leaderboards (`text_to_image`, `image_edit`, `text_to_video`, `image_to_video`), matched by name. The server downloads them once a day from the public Hugging Face dataset `lmarena-ai/leaderboard-dataset`; `PICTESA_LEADERBOARDS=off` turns that off.
```json
{ "arena": "text_to_video", "name": "kling-2.6-pro", "rank": 29, "of": 48, "rating": 1183.4, "votes": 73362, "published": "2026-09-22" }
```
`name` is the model's name on the leaderboard, so a client can show what was rated.

`GET /api/models/prefs` → `{ pinned: [model id], recent: [model id] }`: the caller's pinned models in pin order and the models of their latest generations, newest first. `PUT /api/models/prefs` with `{ "pinned": [model id] }` replaces the pins (up to 100) and answers the same shape.

`ModelSpec` (see `crates/pictesa-core/src/model.rs`):
```json
{
  "id": "fal:fal-ai/kling-video/v3/pro/image-to-video", "provider": "fal", "name": "Kling 3 Pro", "vendor": "Kuaishou",
  "output": "video", "tasks": ["image_to_video"],
  "inputs": [{ "role": "start_frame", "kind": "image", "min": 1, "max": 1, "label": "Start frame" }],
  "params": [
    { "key": "duration", "label": "Duration", "type": "enum", "options": [{ "value": 5, "label": "5s" }], "default": 5, "advanced": false },
    { "key": "seed", "label": "Seed", "type": "seed", "advanced": true }
  ],
  "prompt": true, "negative_prompt": true, "max_outputs": 1,
  "pricing": { "unit": "per_second", "usd": 0.112 },
  "open_weights": false, "description": "...", "tags": ["featured"]
}
```
Param `type`: `enum` (`options`), `int` (`min`,`max`,`step`), `float` (`min`,`max`,`step`), `bool`, `text` (`multiline`,`max_len`), `seed`.
Input `role`: `reference`, `start_frame`, `end_frame`, `video`, `audio`, `mask`. `kind`: `image`, `video`, `audio`.

`POST /api/models/refresh` re-runs live discovery (gateway catalogs, local ComfyUI workflows) → `{ models: n }`. `?provider=fal` refreshes only that provider.

## Presets

`GET /api/presets?output=video` → `{ presets: [Preset] }`
```json
{ "id": "camera/dolly-in", "name": "Dolly in", "category": "camera", "output": "video",
  "prompt_suffix": "slow dolly-in toward the subject, steady motion", "params": { "camera_motion": "dolly_in" },
  "description": "Camera glides forward toward the subject." }
```
A generation with `preset` appends `prompt_suffix` to the prompt and merges `params` (the user's explicit params win). Model-specific params only apply when the model has that key.

## Generations (jobs)

`POST /api/generations`
```json
{
  "model": "fal:fal-ai/flux-pro/v1.1-ultra",
  "prompt": "a cabin in fog at dawn",
  "negative_prompt": null,
  "params": { "aspect_ratio": "16:9", "seed": 42 },
  "inputs": [{ "asset_id": "01J...", "role": "reference" }],
  "count": 4,
  "preset": "camera/dolly-in"
}
```
→ `201 { "batch_id": "01J...", "jobs": [Job] }`. `count` above the model's `max_outputs` becomes several jobs. Validation errors → `400 { "error": "...", "field": "params.duration" }`.

| Method | Path | Answer |
|---|---|---|
| GET | `/api/generations?status=running&output=video&limit=50&before=<id>` | `{ jobs: [Job], next: id\|null }` (newest first) |
| GET | `/api/generations/{id}` | `Job` |
| POST | `/api/generations/{id}/cancel` | `Job` |
| POST | `/api/generations/{id}/retry` | `{ batch_id, jobs: [Job] }` (same inputs) |
| DELETE | `/api/generations/{id}` | 204 (assets stay in the library) |

`Job`:
```json
{
  "id": "01J...", "batch_id": "01J...", "model": "fal:...", "provider": "fal", "model_name": "FLUX 1.1 Pro Ultra",
  "output": "image", "status": "queued"|"running"|"succeeded"|"failed"|"canceled",
  "prompt": "...", "negative_prompt": null, "params": {}, "inputs": [{ "asset_id": "...", "role": "reference" }],
  "preset": null, "count": 1,
  "progress": 0.42, "message": "In queue (position 3)",
  "error": { "kind": "content_policy", "message": "..." },
  "cost_usd": 0.06, "attempts": 1,
  "created_at": 0, "started_at": 0, "finished_at": 0,
  "outputs": [Asset]
}
```
`progress` is null when the provider reports none (the UI then shows elapsed time, never an invented percentage).

## Assets (library)

| Method | Path | Body | Answer |
|---|---|---|---|
| POST | `/api/assets` | multipart `file` (repeatable), optional `metadata` JSON or metadata text fields | `{ assets: [Asset] }` |
| POST | `/api/assets/import` | `{ url }` | `{ assets: [Asset] }` |
| GET | `/api/assets?kind=image&favorite=true&q=fog&model=fal:...&source=generated&trash=false&limit=60&before=<id>` | | `{ assets: [Asset], next }` |
| GET | `/api/assets/{id}` | | `Asset & { job: Job\|null }` |
| PATCH | `/api/assets/{id}` | `{ favorite?: bool }` | `Asset` |
| DELETE | `/api/assets/{id}?permanent=false` | | 204 (moves to trash; `permanent=true` deletes the file when unused) |
| POST | `/api/assets/{id}/restore` | | `Asset` |

`Asset`:
```json
{
  "id": "01J...", "kind": "image"|"video"|"audio", "mime": "image/png",
  "width": 1024, "height": 1024, "duration_ms": null, "size": 812345,
  "url": "/files/01J...", "thumb_url": "/thumbs/01J..." ,
  "source": "generated"|"upload"|"import", "job_id": "01J...", "model": "fal:...", "prompt": "...",
  "favorite": false, "created_at": 0, "deleted_at": null
}
```
`q` searches prompts (SQLite FTS5). `source` also takes `edit` (see Edit helpers); without it, edit helpers are left out of the list.

### Save results from other tools

Both upload and URL import accept `prompt`, `negative_prompt`, `origin` (free text), `model` (optional catalog ID), `model_label`, `params` (a JSON object), and `source_url`. These fields are returned on `Asset`, including in events and MCP. No generation job is created. `q` finds imported prompts too. A matching catalog `model` enables **Reuse settings** in the viewer.

For multipart upload, send one `metadata` JSON object or individual text fields (`params` must contain JSON). Fields may precede or follow the files; the same metadata applies to every file. Duplicate content returns the existing active asset for this user and only fills missing fields and parameter keys. Existing values stay unchanged. Other users' assets are separate. Text limits: prompt and negative prompt 20,000 characters each; origin 200; model and label 500; source URL 4,000. Params allow 64 keys and 64 KB.

```bash
curl -F 'file=@result.png' \
  -F 'metadata={"prompt":"a cabin in fog","origin":"gemini-api","model":"google:gemini-3-pro-image","model_label":"Nano Banana Pro","params":{"resolution":"2K"}}' \
  http://127.0.0.1:4747/api/assets
```

### Batch speed

Models opt in with `meta.batch: true` and a `speed` parameter: `standard` (default) or `batch`. OpenAI and Google image models expose **Speed** with help text: Batch is half price and can take up to 24 hours. Set `params.speed` to `batch` on a generation. Fixed cost estimates and hosted wallet charges use half the standard price. Variable/token prices remain unknown and require the user's own key, as in Standard mode.

Batch handles persist in the job database. The queue polls every 60 seconds and resumes accepted jobs after a restart. Cancellation uses the provider's batch cancellation route. The queue allows 25 hours including finalization. Cancellation after submission may still incur provider charges. The mock image model supports Batch for free testing and polls faster.

Provider contracts: [OpenAI Batch API](https://developers.openai.com/api/docs/guides/batch), [Gemini REST Batch API](https://ai.google.dev/api/batch-api).

## Files

- `GET /files/{asset_id}` streams the original with `Range` support (video scrubbing) and long cache headers.
- `GET /thumbs/{asset_id}` returns a JPEG thumbnail (longest edge 512 px; video poster frame). 404 when none exists yet.
- `GET /files/{asset_id}?download=1` sets `Content-Disposition: attachment`.

## Events

`GET /api/events` is a server-sent event stream for the current user:
- `event: job` with a `Job` as data, on every status or progress change.
- `event: asset` with an `Asset`, when one is created, changed or trashed.
- `event: ping` every 20 s.

## MCP

`POST /mcp` speaks the Model Context Protocol (Streamable HTTP, JSON responses). `pictesa mcp` speaks it over stdio.
Tools: `list_collections`, `add_to_collection`, `list_models`, `list_presets`, `generate`, `get_job`, `wait_for_job`, `cancel_job`, `list_assets`, `get_asset`, `import_asset`, `list_boards`, `create_board`, `get_board`, `add_to_board`, `run_board_step`.

## Errors

Non-2xx answers carry `{ "error": "human readable message", "kind"?: "auth"|"validation"|"not_found"|..., "field"?: "..." }`.

## Additions (v1.1)

### Model details are loaded on demand
`GET /api/models` lists discovered gateway models with `"complete": false` (no params or inputs yet, to keep the list light). When a model is selected, call `GET /api/models/{id}` (the id contains slashes; send it as is, e.g. `/api/models/fal:kling/v4/image-to-video`). It returns the full spec and caches it. Models also carry `"billed": true` in hosted mode when runs are paid from the wallet; their `pricing` then already includes the markup.

### Local access token
`GET /api/auth/me` returns `"needs_token": true` when the local server requires a token and the browser has none. Show a single "Access token" field and `POST /api/auth/token { token }`; on 204 the cookie is set, reload `/api/auth/me`.

### Recipes (multi-step pipelines)
| Method | Path | Body | Answer |
|---|---|---|---|
| GET | `/api/recipes` | | `{ recipes: [RecipeView] }` |
| POST | `/api/recipes/run/{id}` | `{ values: {field: value}, inputs: {input_key: [asset_id]}, models?: {step_id: model_id} }` | `201 RecipeRun` |
| GET | `/api/recipes/runs?limit=30` | | `{ runs: [RecipeRun] }` |
| GET | `/api/recipes/runs/{id}` | | `RecipeRun` |
| POST | `/api/recipes/runs/{id}/cancel` | | `RecipeRun` |

`RecipeView`: `{ id, name, description, output, fields: [{ key, label, type: "text"|"enum"|"preset", options, category, default, required, multiline, help }], inputs: [{ key, label, kind, max, required }], steps: [{ id, label, output, models: [ids], prompt, params, preset, inputs }], step_models: [model id or null per step], runnable }`.
A `preset` field offers presets of its `category` (GET /api/presets). Recipe ids contain a slash (`cinema/shot`).
`RecipeRun`: `{ id, recipe_id, recipe_name, status: "running"|"succeeded"|"failed"|"canceled", step, steps, jobs: [job ids], error, created_at, updated_at }`. Its jobs are ordinary generations (they appear in `/api/generations` and fire `job` events); runs also fire `event: recipe`.

### Billing (hosted mode)
| Method | Path | Body | Answer |
|---|---|---|---|
| GET | `/api/billing` | | `{ enabled, topups_available, balance_usd, debt_usd, locked, markup, topups_usd: [10, 25, 50], ledger: [{ id, amount_usd, reason, job_id, created_at }] }` |
| POST | `/api/billing/checkout` | `{ amount_cents, request_id }` | `{ url }` (redirect the browser to Stripe) |
| POST | `/api/billing/webhook` | Stripe event | `{ ok }` (Stripe only) |

Show the balance and top-up only when `enabled` is true. A generation the wallet cannot cover fails validation with `field: "billing"`.

### Compare models
`POST /api/generations` accepts `"compare": ["model id", ...]` (up to 7 extra models). The same prompt, inputs and params run on every model in one batch; params are adapted to each model (unknown ones dropped, values snapped to the nearest allowed option) and inputs a model does not take are left out. Show the batch side by side.

### Prompt help
`POST /api/prompt/enhance { prompt, output: "image"|"video" }` → `{ prompt }`. Rewrites a short idea into a detailed prompt with a free OpenRouter text model (the person's OpenRouter key; `PICTESA_ENHANCE_MODEL` picks another). Errors come back as validation errors with a readable message (no key, models busy).

### Notes for clients (v1.2)
- `ModelSpec.prompt` means the model *accepts* a prompt. A prompt is required only for text-to-image, text-to-video and text-to-audio runs without input files.
- `Job.prompt` is the person's own text. When `preset` is set, its words are added only when the request is sent, so reusing a job's settings never doubles them.
- `Asset.name` holds the original file name of uploads and imports.
- Recipe `enum` field `options` are plain strings. Validation errors use `field` values `values.<key>`, `inputs.<key>`, `models.<step>` or `models`.
- Stripe returns to `/settings/billing?checkout=success` or `?checkout=canceled`.
- Uploads stream to disk (up to 4 GB per file); every other request body is limited to 2 MB. Files must be real images, videos or audio (recognised from their content); SVG and other active formats are refused.
- The CLI takes library items as inputs with `-i role=asset:<id>`.
- `POST /api/assets` with several files stores every valid one and lists the others in `errors: [{ name, error }]`; it answers 400 only when no file could be stored.

### Characters
Named sets of reference images (a person, a product, a style) that any run can attach.

| Method | Path | Body | Answer |
|---|---|---|---|
| GET | `/api/characters` | | `{ characters: [Character] }` |
| POST | `/api/characters` | `{ name, description?, asset_ids: [1..14 image ids] }` | `201 Character` |
| GET | `/api/characters/{id}` | | `Character` |
| PATCH | `/api/characters/{id}` | any of `{ name, description, asset_ids }` | `Character` |
| DELETE | `/api/characters/{id}` | | 204 |

`Character`: `{ id, name, description, assets: [Asset], created_at, updated_at }`. `POST /api/generations` takes `"character": "<id>"`: its images are added as `reference` inputs, up to what the model accepts (a model without image references refuses it). Changes fire `event: character`.
- Events carry an `id`. After a reconnect (the browser sends `Last-Event-ID`), the stream starts with `event: resync`: refetch jobs, runs and lists, since missed events are not replayed.

### Import a ComfyUI workflow
`POST /api/workflows/import { name, workflow }` turns a ComfyUI workflow in API format into a model (hosted mode: administrators only). Pictesa reads the graph for the prompt and negative prompt (by following the samplers' conditioning), seed, steps, guidance, output size, batch size, video length and frame rate, and the image, video and audio loaders, writes a template into the ComfyUI workflows folder and refreshes the catalog. Answer: `201 { model: "comfyui:<id>", manifest, notes: [] }`. The editor format (with `nodes` and `links`) is refused with a hint to use Workflow > Export (API). Also: `pictesa workflow import file.json --name ...` and the MCP tool `import_workflow`.

### Live previews
Backends that stream in-progress images (ComfyUI) produce `event: preview` with `{ job_id, version, url }` while a job runs. Pictesa asks ComfyUI for sampler previews on every prompt, so a stock ComfyUI needs no `--preview-method` flag. `GET /api/generations/{id}/preview` returns the latest preview image (JPEG or PNG, not cached); it answers 404 when there is none. Previews disappear when the job ends.

### Edit helpers
The studio's Edit view (inpaint, expand, relight, remove background) runs ordinary generations. Two routes prepare the files a mask-taking model needs. Both store PNGs with `source: "edit"` and `made_from` set to the image they were made from; library lists leave them out unless `source=edit` is asked for.

| Method | Path | Body | Answer |
|---|---|---|---|
| POST | `/api/assets/{id}/mask` | the painted mask as an image file (`Content-Type: image/png`, up to 32 MB) | `201 Asset` |
| POST | `/api/assets/{id}/expand` | `{ left, top, right, bottom }` (pixels to add per side) | `201 { image: Asset, mask: Asset }` |

- A mask marks paint with opaque pixels (a transparent canvas export) or with light pixels (a black and white image). It is stored black and white at the image's own size, white = change. A mask with no paint is refused with `field: "mask"`.
- `expand` repeats the edge pixels into the new area and returns a mask that is white over the new area, reaching a few pixels into the old edge so the seam blends. The result is at most 8192 px per side and 24 megapixels.
- Then run any model with a `mask` input: `inputs: [{ asset_id: <image or expanded image>, role: "reference" }, { asset_id: <mask>, role: "mask" }]`. Relight and background removal need no helper: send the image itself.

### Collections, tags and bulk library actions

All routes use the authenticated user's library. Collections can contain the same asset in several groups. Deleting a collection keeps its assets. Changes emit a user-scoped `library` event so clients can reload memberships and autocomplete. Names contain 1 to 100 characters; tags contain 1 to 60 characters and match without case sensitivity.

| Method | Path | Body | Answer |
|---|---|---|---|
| GET | `/api/collections` | | `{ collections: [{ id, name, created_at, asset_count }] }` |
| POST | `/api/collections` | `{ name }` | 201 collection |
| PATCH | `/api/collections/{id}` | `{ name }` | 204 |
| DELETE | `/api/collections/{id}` | | 204 |
| POST | `/api/collections/{id}/assets` | `{ asset_ids: [...] }` | 204 (idempotent add) |
| DELETE | `/api/collections/{id}/assets` | `{ asset_ids: [...] }` | 204 (remove membership) |
| GET | `/api/tags` | | `{ tags: [string] }` for autocomplete |
| GET | `/api/assets/{id}/tags` | | `{ tags: [string] }` |
| POST | `/api/tags` | `{ asset_ids: [...], tag, remove?: false }` | 204 (add or remove a tag) |
| POST | `/api/assets/bulk` | `{ asset_ids: [...], action: "favorite"\|"unfavorite"\|"trash" }` | 204 |
| POST | `/api/assets/download` | `{ asset_ids: [...] }` | streamed `application/zip` attachment |

Bulk operations accept 1 to 100 IDs and validate every owner before changing anything. ZIP downloads accept up to 100 assets and 1 GiB of original media, stored uncompressed. An empty selection, a trashed asset or an exceeded limit gives a 400 with a readable error before streaming starts; another user's or a missing asset gives 404. Filenames use the original name or prompt, sanitized and suffixed with the asset ID. Duplicate IDs are downloaded once.

`GET /api/assets` also takes `collection=<id>`, `tag=<label>`, `provider=<id>`, `cost_min=<USD>` and `cost_max=<USD>`. Filters combine with existing parameters and pagination. Cost endpoints are inclusive, finite and nonnegative. Cost is the run's known charge or estimate, so assets from the same run have the same cost. Unknown costs and assets without runs do not match a cost range. Provider filtering uses the recorded run provider. Filters appear in the library URL.

### Spend and budgets

| Method | Path | Body | Answer |
|---|---|---|---|
| GET | `/api/spend` | | spend view below |
| PUT | `/api/spend/budgets` | `{ daily_usd: number\|null, monthly_usd: number\|null }` | updated spend view |

The spend view contains `today_usd`, `seven_days_usd`, `thirty_days_usd`, `days: [{ name: "YYYY-MM-DD", usd }]` (30 calendar days including today), `models` and `providers` (30-day groups with `{ name, usd }`), `daily` and `monthly` (`{ cap_usd, spent_usd, remaining_usd }`), `unknown_runs`, `estimated_runs` and `timezone: "UTC"`. The 7-day and 30-day totals include today. Calendar caps reset at midnight UTC or the start of the calendar month UTC.

Spend is isolated per hosted user and uses the single global local user otherwise. Queued runs reserve their estimated cost. Wallet-funded runs record the actual wallet charge including markup. Direct-key runs keep the estimate because provider responses currently do not report final charges. Local backends count as zero. Confirmed unbilled failures and cancellations release spend, including wallet refunds; uncertain upstream submissions keep their reservation. Deleting a job does not erase spend. Unpriced runs are counted in `unknown_runs` and excluded from totals.

Caps are optional, from $0 to $1,000,000; null removes a cap and zero blocks paid runs. Both caps are replaced by PUT. A paid batch that exceeds either remaining cap is rejected atomically before queuing or provider submission, with `field: "budget"` and the cap name, amount and remainder. The exact boundary is allowed. A paid model without a cost estimate is refused while a cap is active. Amounts are stored in integer micro-dollars.

### Boards

A board is a canvas of library files, notes and steps that people and agents edit together: moodboards, storyboards and chained generations. It is one document (nodes, edges and the latest run of each step) with a version number.

| Method | Path | Body | Answer |
|---|---|---|---|
| GET | `/api/boards` | | `{ boards: [BoardSummary] }`: own and shared boards, newest change first |
| POST | `/api/boards` | `{ name }` | `201 Board` |
| GET | `/api/boards/{id}` | | `Board` |
| PATCH | `/api/boards/{id}` | `{ name }` | `Changes` (owner and editors) |
| DELETE | `/api/boards/{id}` | | 204 (owner) |
| POST | `/api/boards/{id}/ops` | `{ ops: [Op] }` | `Changes`, or `409 { error, kind: "conflict", current: Changes }` |
| POST | `/api/boards/{id}/steps/{node}/run` | `{ downstream?: bool }` (optional) | `201 { steps: [node id], run: StepRun }` |
| POST | `/api/boards/{id}/steps/{node}/cancel` | | `StepRun` |
| POST | `/api/boards/{id}/members` | `{ email, role: "editor"\|"viewer" }` | `{ members: [Member] }` (hosted mode, owner; also changes a role) |
| DELETE | `/api/boards/{id}/members/{user}` | | 204 (the owner removes anyone; a member can leave) |
| POST | `/api/boards/{id}/presence` | `{ client, leave?: bool }` | `{ viewers: [Person] }` |
| POST | `/api/boards/{id}/copy` | `{ asset_ids }` | `{ assets: [Asset] }`: the caller's own copies of files on the board |

`Board`: `{ id, name, role: "owner"|"editor"|"viewer", owner: Person, version, created_at, updated_at, nodes: [Node], edges: [Edge], runs: { <node id>: StepRun }, assets: { <asset id>: Asset }, members: [Member] }`. `Person`: `{ id, name, email }`. `Member`: a Person with `{ role, added_at }`. `BoardSummary`: `{ id, name, role, owner, version, created_at, updated_at, node_count, member_count, covers: [Asset] }`.

```json
{ "id": "paint", "kind": "step", "x": 340, "y": 0, "w": 300, "rev": 7,
  "model": "mock:image", "prompt": "the cabin at golden hour", "params": { "aspect_ratio": "4:5" }, "count": 2 }
```
A `Node` has `id`, `kind` (`asset`, `note` or `step`), `x`, `y`, `w` (0 on a new node means the kind's default) and `rev`. Asset nodes add `asset_id`, notes add `text`, steps add `model`, `prompt`, `negative_prompt`, `params` and `count` (1 to 8 results per run). An `Edge` is `{ id, from, to, role, rev }`. An edge into a step is an input: from an asset node it feeds that file, from another step it feeds the results of that step's last run that are still on the board. `role` picks the model's slot (`reference`, `start_frame`, `end_frame`, `video`, `audio`, `mask`); null takes the first free slot that takes the file, never the mask. More connected files than a slot takes are refused, except extra results of a step, which are left out. Steps cannot feed each other in a loop. Ids are 1 to 64 letters, digits, dashes or underscores.

`StepRun`: `{ status: "waiting"|"starting"|"running"|"succeeded"|"failed"|"canceled", by: <user id>, jobs: [job id], outputs: [asset id], after: [node id], error, updated_at, rev }`.

| Op | Fields |
|---|---|
| `add_node` | `node` |
| `move_node` | `id`, `base`, `x`, `y` |
| `update_node` | `id`, `base` and any of `w`, `text` (notes), `model`, `prompt`, `negative_prompt`, `params`, `count` (steps) |
| `delete_node` | `id`, `base` (its edges go with it) |
| `add_edge` | `edge: { id, from, to, role? }` |
| `update_edge` | `id`, `base`, `role` |
| `delete_edge` | `id`, `base` |
| `rename` | `name` |

**Versions.** Every applied request adds 1 to the board's `version`, and every node and edge it touches gets that version as its `rev`. An op on an existing item names `base`, the item's `rev` the caller saw. A request applies whole or not at all. When an item it names changed or was removed since, the answer is 409 and `current` holds the current state of every item the request named (removed ones in `removed_nodes` and `removed_edges`): show it and drop the change. Edits to different items never conflict. Limits: 200 ops per request, 500 nodes and 1,000 edges per board, notes up to 10,000 characters, prompts up to 20,000.

`Changes` (the answer to a change, and the `board` event): `{ board_id, version, name?, nodes, edges, runs, removed_nodes, removed_edges, assets, by? }` with the current state of each touched item and the files of new asset nodes. Clients merge items by `rev`, so the order of arrival does not matter.

**Runs.** Running a step queues an ordinary generation as the caller, with the same validation, budgets, wallet and queue as `POST /api/generations`. The answer comes once the first generation is queued, so a missing input or a budget refusal comes back at once (400; the step is marked failed). Results land on the board as asset nodes right of the step, linked from it. With `downstream`, every step reached through edges between steps waits (`waiting`, with `after` naming the steps it needs) and starts when they succeed; a failed or canceled step settles the ones after it. A step that is still running cannot start again (409). The server's board watcher moves runs on, so runs started by another process (an agent over stdio MCP) still land.

**Events.**
- `event: board` with `Changes`, to the owner and every member, for every change, including results landing and runs moving on. `{ board_id, version, reload: true }` means another process changed the board: load it again. The server looks for such changes every 2 seconds.
- `event: boards` with `{ board_id, deleted?, removed? }`: a board was created, renamed, shared or deleted, or someone left.
- `event: board_presence` with `{ board_id, viewers: [Person] }`: who has the board open. Clients check in with `POST presence` every 20 seconds; a check-in expires after 45 seconds.

**Sharing and files.** Hosted mode: the owner shares by email. Editors change the board and run its steps, paid from their own account and budget; viewers only look; anyone else gets 404 for the board, its routes and its files. Members can open the files shown on the board (`/files/{id}`, `/thumbs/{id}`) while they are on it and not in their owner's trash. When a step uses another member's file, the runner first gets their own copy in their library (stored once, `origin: "board"`); `POST /copy` does the same, for example to use board images as references in Create. Local mode has one user, and agents use the same routes.
