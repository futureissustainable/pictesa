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
