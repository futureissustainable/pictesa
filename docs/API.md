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

`GET /api/models?output=image|video|audio|lora&task=text_to_video&provider=fal&q=kling&configured=true&sort=arena`
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
  "id": "01J...", "kind": "image"|"video"|"audio"|"lora", "mime": "image/png",
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
Tools: `train_lora`, `get_training`, `list_loras`, `list_collections`, `add_to_collection`, `list_models`, `list_presets`, `generate`, `get_job`, `wait_for_job`, `cancel_job`, `list_assets`, `get_asset`, `import_asset`.

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

### Personal prompts and templates

All prompt routes use the caller's user ID. History contains distinct nonempty job prompts, newest use first; the model and name come from that latest use. Search is a case-insensitive literal substring, including punctuation. `limit` defaults to 50 and is capped at 100.

| Method | Path | Body | Answer |
|---|---|---|---|
| GET | `/api/prompts/history?q=cabin&limit=50` | | `{ history: [{ prompt, model, model_name, last_used_at }] }` |
| GET | `/api/prompts?q=cabin&limit=100` | | `{ prompts: [SavedPrompt] }` |
| POST | `/api/prompts` | `{ name, prompt, negative_prompt?, model?, params? }` | 201 SavedPrompt |
| PATCH | `/api/prompts/{id}` | `{ name }` | SavedPrompt |
| DELETE | `/api/prompts/{id}` | | 204 |
| POST | `/api/prompts/{id}/render` | `{ values: { subject: "cabin", place: "forest" } }` | rendered `{ name, prompt, negative_prompt, model, params }` |

`SavedPrompt` adds `id`, `placeholders: [string]`, `created_at` and `updated_at` to the saved content. Names have 1 to 100 characters; prompt and negative prompt each allow 20,000. Params are optional, with up to 64 scalar values and 64 KB. Saving settings does not queue a generation or require a currently configured model. Other users cannot read, render, rename or delete the prompt.

Templates use `{identifier}` where the identifier starts with an ASCII letter or underscore and continues with letters, numbers or underscores. The same identifier uses one value everywhere, including the negative prompt. Paired `{{` and `}}` produce literal braces. Other braces, including JSON, stay literal. Missing or blank values fail with `field: values.<identifier>`. Rendering is single-pass: braces inside a supplied value are never evaluated. Rendered text is capped at 20,000 characters. The studio asks for these values inline before inserting the prompt and optional model/settings into the composer.

### Batch runs and variations

`POST /api/generations/batch` uses the normal generation fields, plus:

```json
{
  "model": "mock:image",
  "prompts": ["a cabin at dawn", "a cabin at dusk"],
  "seeds": { "start": 0, "count": 2 },
  "grid": [{ "key": "aspect_ratio", "values": ["1:1", "16:9"] }],
  "dry_run": true
}
```

A dry run returns 200 `{ run_count: 8, total_estimated_cost_usd: 0, max_runs: 64 }` after validating every run and checking the current budget/wallet, without queuing or reserving money. Unknown estimates are `null`, never zero. Set `dry_run: false` (the default) to queue: 201 `{ batch_id, jobs }`. It revalidates the request and current funds, then commits every job, spend reservation and wallet charge in one transaction. Failed validation or insufficient funds queues nothing. Hosted estimates include wallet markup. A dry run is an estimate, not a reservation.

Instead of `prompts`, send `prompt_list` with `prompt_format: "lines"` (default) or `"csv"` (first column, no header). CSV supports quoted commas, newlines and doubled quotes, UTF-8 BOM and CRLF. Empty rows are skipped. Lines preserve commas. Do not send both list forms. Without a list, `prompt` is used once. Each run requests one output; `count` must be omitted or 1. Compare is not combined with batch runs.

Runs expand in prompt order, then ascending seeds, then grid axes in request order (last axis changes fastest). Limit: `MAX_BATCH_RUNS = 64` after multiplication, and up to 3 distinct model parameters in the grid, with nonempty value lists. A seed sweep requires a declared seed parameter, takes 1 to 64 seeds and stays within 0 to 9007199254740991. A grid cannot also sweep that seed key. Each value passes the normal model validation, with the same defaults, inputs, presets, characters, provider readiness, paid-call switch, budgets and wallet rules. `Asset.batch_id` lets clients group generated results in the feed.

`POST /api/generations/{id}/variations` uses a succeeded job's model, prompt, negative prompt, inputs, preset and settings. Models with a seed parameter get 4 distinct new seeds in one batch. Models without one use the normal `count: 4` path. Answer: 201 `{ batch_id, jobs }`. All normal cost and budget checks apply.

## Video tools

When `ffmpeg` and `ffprobe` are executable on PATH, the built-in `local` provider is configured automatically. It is free (`kind: "local"`, `pricing: {"unit":"local"}`), needs no key and runs one job at a time. `PICTESA_FFMPEG_DIR`, when set, also supports a bundled installation. Local tools use the same generation, recipe, library and event APIs as every other model. Jobs record all input asset IDs, their roles and parameters. No video-specific endpoint is needed.

| Model | Required inputs | Parameters |
|---|---|---|
| `local:trim` | One `video` | `start` (default 0), `end` (default 1), in seconds; `0 <= start < end <= clip duration` |
| `local:join` | 2 to 10 `video` inputs, in playback order | None. Re-encodes to the first clip's size and frame rate, fits each image without cropping, pads with black, and uses stereo 48 kHz audio. Silent clips get silence. |
| `local:frame` | One `video` | `time` in seconds (default 0), before the end of the clip; outputs a PNG image |
| `local:smooth` | One `video` | None. Uses ffmpeg `minterpolate` to produce 2× the input frame rate; slow on CPU. |
| `local:add-audio` | One `video`, one `audio` | `mode`: `replace` (default) or `mix`; `volume`: 0 to 4 (default 1); `fade_in`, `fade_out`: seconds (default 0), each no longer than the video |

Added audio is padded with silence or cut to the video's length. Volume and fades affect only the added track. Mix keeps existing sound, or behaves like replace on a silent source. Video outputs use MP4/H.264/AAC; odd dimensions round down to an even pixel count for H.264. Join uses the first input's display dimensions and frame rate.

Limits: inputs need a known duration of at most 600 seconds each; joined duration must total at most 600 seconds. Videos must be at most 3840 pixels per side, 3840 × 2160 pixels total and 120 fps (60 fps for smoothing). Each command uses at most two threads per codec or filter pool, a ten-minute runtime and a 128 MiB output limit. Commands run directly without a shell, disallow network and playlist protocols, and remove temporary files on completion, failure or cancellation. Supported containers include MP4/MOV, WebM/Matroska, AVI, MPEG/TS, WAV, MP3, Ogg, FLAC and AAC. Invalid files and unsupported settings fail the job with a readable error.

```json
{
  "model": "local:trim",
  "prompt": "",
  "inputs": [{"asset_id":"YOUR_VIDEO_ID","role":"video"}],
  "params": {"start":1.5,"end":4},
  "count":1
}
```

The video editor's cloud tools use configured catalog models and their published parameter schemas: **Extend** selects video extension endpoints; **Upscale** selects video upscalers such as Topaz or SeedVR; **Create sound** selects video-to-audio or synchronized sound models. Models may return audio alone or a video with sound. Their prices and limits come from the provider. See [fal MMAudio](https://fal.ai/models/fal-ai/mmaudio-v2/api) and [Topaz](https://fal.ai/topaz). Catalog task `video_to_audio` is available as a model-list filter and through MCP `list_models`; existing generation tools handle it.

## Character training and LoRAs

Training uses the normal job queue, progress events, retry, cancellation, budgets and wallet reservations. No provider request starts during validation or estimation. Local ai-toolkit training is free and available only in local mode; fal training is paid. `PICTESA_DENY_PAID=1` blocks fal training too.

| Method | Path | Body / query | Answer |
|---|---|---|---|
| GET | `/api/models?output=lora` | optional `configured=true` | Trainer models from the active fal catalog and local ai-toolkit |
| POST | `/api/training/estimate` | Training request below | `{ run_count: 1, total_estimated_cost_usd, max_runs }`, without reserving funds |
| POST | `/api/training` | Training request below | 201 `{ batch_id, jobs }` |
| GET | `/api/training` | | Latest 50 training jobs |
| GET | `/api/training/{id}` | | Normal `Job` with `output: "lora"`, progress and weights asset |
| GET | `/api/loras` | optional `family`, `character` | `{ loras: [{ asset, character_id, family, trigger_word }] }` |

```json
{
  "trainer": "ai-toolkit:flux1",
  "character": "CHARACTER_ID",
  "trigger_word": "maya_person",
  "family": "flux1",
  "steps": 1000,
  "rank": 16,
  "learning_rate": 0.0001
}
```

Instead of `character`, send `asset_ids: ["IMAGE_ID", "..."]` to choose library images. Inputs must be 3 to 100 distinct, owned, nontrashed PNG, JPEG or WebP images, at least 256 px per side, no more than 8192 px per side, 24 megapixels or 32 MiB each, and no more than 512 MiB in total. Image count, dimensions, ownership, settings and published trainer limits are checked before money is reserved. Steps: 100 to 10,000; rank: 4 to 128; learning rate: 0.000001 to 0.001. The trigger contains 1 to 64 ASCII letters, numbers or underscores. Defaults are 1000 steps, rank 16, learning rate 0.0001.

Supported families are `flux1`, `qwen_image`, `qwen_image_2512` and `wan21`. Local training currently supports the first, second and fourth. Families are matched conservatively: FLUX.2, FLUX Kontext, Qwen editing and Wan 2.2 do not inherit incompatible LoRAs. fal trainers use their published input and output schemas, including each ZIP field, steps field, trigger or caption field, exposed learning rate and weights output. Trainers without a published rank or learning-rate control keep their provider defaults; changing those unsupported settings is rejected. The API's default rank and learning-rate values act as placeholders for provider-fixed values, not a claim about those values. Steps outside the published schema's range are rejected. The Qwen Image 2512 trainer receives the trigger as `default_caption`, and every ZIP includes matching trigger captions.

fal examples: `fal:fal-ai/flux-lora-fast-training`, `fal:fal-ai/qwen-image-2512-trainer`, `fal:fal-ai/wan-trainer/t2v`. Availability comes from the active catalog. The older `qwen-image-trainer` and bare `wan-trainer` endpoints are deprecated upstream; their recorded schemas remain contract test fixtures, not recommended endpoints. ZIP datasets stream to fal storage before submission. Only the schema's LoRA output is saved, never the trainer's config or debug archive. Published prices per training run or step produce estimates; compute-time or unknown pricing remains `null`. Existing budgets reject unknown-cost paid runs while a cap is active; hosted wallet runs require a known estimate. Cancellation after fal accepts a job can still cost money, as with ordinary paid generations.

Configure **Settings > Providers > Local training (ai-toolkit)** with its installation folder, or set `PICTESA_AI_TOOLKIT_PATH`. The folder needs `run.py` and its dependencies; Pictesa uses `venv/bin/python` (`venv/Scripts/python.exe` on Windows) when present, else `python3`. An executable wrapper is also accepted and receives the generated config path as its sole argument. Pictesa does not install ai-toolkit, download model weights in advance or manage GPU drivers. Configs are generated in YAML 1.2's JSON subset, with escaped paths, dataset captions, the requested steps/rank/rate, batch size 1, gradient checkpointing, disabled samples and `push_to_hub: false`. stdout and stderr progress (`250/1000`, including carriage-return tqdm lines) becomes live job progress. One local training job runs at a time; cancellation kills its subprocess. A stopped local training process cannot resume its GPU state after a Pictesa restart: retry starts a fresh job. Temporary datasets and outputs are removed after completion, failure or cancellation.

Weights have asset kind `lora`, MIME `application/x-safetensors`, a downloadable `/files/{id}` URL and no thumbnail. Default asset lists and live image grids exclude them; `/api/assets?kind=lora` lists them explicitly. Character links and trigger metadata are private to their owner. Deleting a character keeps the weights but clears their character link. Trashing weights removes them from the picker; deleting weights permanently removes their LoRA metadata.

To generate, send up to four compatible LoRAs in normal generation params:

```json
{
  "model": "fal:fal-ai/flux-lora",
  "prompt": "at a lake at dawn",
  "params": { "loras": [{ "asset_id": "LORA_ASSET_ID", "strength": 0.8 }] }
}
```

Strength is 0 to 2. The engine checks the owner, asset state and family, then adds each missing trigger word once. Clients send asset IDs, never filesystem paths or arbitrary URLs. fal models expose the picker only when their published schema accepts a `loras` array, mapped to `{ path: <uploaded or signed URL>, scale: <strength> }`.

For local generation, set `PICTESA_COMFYUI_LORA_DIR` or `PICTESA_SDCPP_LORA_DIR`, or the provider's **Shared LoRA directory** setting. This must be the directory the corresponding local server actually reads, accessible on Pictesa's filesystem. Weights are copied under content-hash filenames; these copies remain in that directory after library deletion. Without a shared directory, local LoRA use is unavailable. Shared-directory writes are restricted to local mode. ComfyUI workflows with a known family, a model link and the required LoRA loader node get a `LoraLoader` (or `LoraLoaderModelOnly`) inserted into their model/CLIP chain. Workflows with an existing LoRA chain keep their explicit routing and do not expose this picker. stable-diffusion.cpp must advertise the `lora` capability and receives structured `lora: [{ path, multiplier, is_high_noise: false }]` entries. No prompt tags are used. Remote local servers without a shared filesystem need manual weight installation and workflow configuration.

The studio has **Characters > Train LoRA**, a **LoRAs** list with progress, cancel/retry and downloads, and a compatible-model composer picker with strength. Attaching a character offers its matching trained LoRA; using it is an explicit choice.

Sources: [fal trainer schema](https://fal.ai/models/fal-ai/flux-lora-fast-training/api), [Qwen Image 2512 trainer](https://fal.ai/models/fal-ai/qwen-image-2512-trainer/api), [Wan T2V trainer](https://fal.ai/models/fal-ai/wan-trainer/t2v/api), [ai-toolkit example configs](https://github.com/ostris/ai-toolkit/tree/main/config/examples), [stable-diffusion.cpp LoRA API](https://github.com/leejet/stable-diffusion.cpp/blob/master/examples/server/api.md).
