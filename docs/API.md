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

`GET /api/models?output=image|video|audio&task=text_to_video&provider=fal&q=kling&configured=true`
→ `{ models: [ModelSpec & { configured: bool }] }`

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

`POST /api/models/refresh` re-runs live discovery (gateway catalogs, local ComfyUI workflows) → `{ models: n }`.

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
| POST | `/api/assets` | multipart `file` (repeatable) | `{ assets: [Asset] }` |
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
`q` searches prompts (SQLite FTS5).

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
Tools: `list_models`, `list_presets`, `generate`, `get_job`, `wait_for_job`, `cancel_job`, `list_assets`, `get_asset`, `import_asset`.

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
