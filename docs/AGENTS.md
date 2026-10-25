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

Tools: `list_collections`, `add_to_collection`, `list_models`, `get_model`, `list_presets`, `generate`, `get_job`, `wait_for_job`, `cancel_job`, `list_assets`, `get_asset`, `import_asset`.
`generate` waits for the result by default and returns local file paths, so an agent can open the image right away. Pass `include_preview: true` to also receive small JPEG previews for vision models.

## 2. CLI

```bash
pictesa models --output video --featured           # what can I use?
pictesa model fal:kling/v4/image-to-video          # its inputs and parameters
pictesa generate "a timber cabin in fog at dawn" -m fal:google/nano-banana-2.1 -p aspect_ratio=16:9 -n 4 -o ./out
pictesa generate "slow push in" -m fal:kling/v4/image-to-video -i start_frame=./out/cabin.png -p duration=5 --json
```

`--json` prints machine-readable results; the exit code is non-zero when a generation fails. When a server is running, CLI jobs run there and appear live in the studio.

## 3. HTTP API

See [API.md](API.md). Events stream from `GET /api/events` (server-sent events), so agents can follow progress without polling.

## Costs

Generations on paid providers cost money. Every model lists its published price (`pricing`), and every job records an estimate (`cost_usd`). Local models (ComfyUI, stable-diffusion.cpp) are free to run.

## File outputs into collections

- `list_collections {}` returns your collections as `{ collections: [{ id, name, created_at, asset_count }] }`.
- `add_to_collection { collection_id, asset_ids: ["..."] }` adds 1 to 100 of your assets to a collection and returns `{ ok: true }`. Adding the same asset twice is safe. Assets may belong to several collections.

Create or rename collections in the studio's Collections section or through the HTTP API. Call `list_collections`, generate or import assets, then call `add_to_collection` with their library IDs. Hosted agents can only list and change their own collections and assets.

Settings > Spend shows recent costs and optional daily and monthly budgets. All generation paths, including MCP, CLI, comparisons and recipe steps, check the caps before queuing. A refusal names the cap and remaining USD. Local runs count as zero; known wallet charges include markup; direct-key runs use estimates. See [API.md](API.md) for spend, tags, filters and ZIP routes.
