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

Tools: `list_collections`, `add_to_collection`, `list_models`, `get_model`, `list_presets`, `generate`, `get_job`, `wait_for_job`, `cancel_job`, `list_assets`, `get_asset`, `import_asset`, `list_boards`, `create_board`, `get_board`, `add_to_board`, `run_board_step`.
`list_models` returns each model's LMArena rank per task when people have rated it; pass `sort: "arena"` with a `task` to get the best rated first, or `sort: "newest"`.
`generate` waits for the result by default and returns local file paths, so an agent can open the image right away. Pass `include_preview: true` to also receive small JPEG previews for vision models.

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
