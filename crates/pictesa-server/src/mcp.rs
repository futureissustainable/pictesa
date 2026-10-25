//! Model Context Protocol server: the same tools over Streamable HTTP (`POST /mcp`) and stdio
//! (`pictesa mcp`). Agents can list models, generate, wait for jobs and read the library.

use std::time::Duration;

use base64::Engine as _;
use pictesa_core::{InputRole, MediaKind};
use pictesa_engine::{
    boards::{Board, Op, RunStatus},
    packs::Pack,
    Asset, AssetFilter, CreateGeneration, Engine, EngineError, Job, JobInput, ModelFilter,
};
use serde_json::{json, Map, Value};

pub struct McpContext {
    pub engine: Engine,
    pub user_id: String,
    /// Local mode: tools may read and return file system paths.
    pub local_files: bool,
}

const PROTOCOLS: &[&str] = &["2025-06-18", "2025-03-26", "2024-11-05"];

const INSTRUCTIONS: &str = "Pictesa generates images and videos with many providers and local models. \
Typical flow: list_models (filter by output image or video), optionally get_model for its parameters, then generate \
(it waits for the result by default and returns file paths). Pass input images or videos to generate as {role, path|url|asset_id}; \
roles are reference, start_frame, end_frame, video, audio, mask. Presets (list_presets) add camera moves or looks. \
Boards (list_boards, get_board, add_to_board, run_board_step) are shared canvases of files, notes and chained steps; people see your changes live. \
Recipes (list_recipes, run_recipe) are the studio's one-click apps, such as shots, angles, face swap, product placement and \
color grade; a step that runs once per item returns a set in results. Skill packs (prompts/list, prompts/get) are step-by-step \
workflows for a UGC ad, hook variants, an explainer, a faceless short and a brand kit.";

pub async fn handle(ctx: &McpContext, msg: Value) -> Option<Value> {
    let method = msg.get("method").and_then(Value::as_str).unwrap_or("");
    let params = msg.get("params").cloned().unwrap_or(Value::Null);
    // Notifications (no id, or a null id) never get an answer.
    let id = msg.get("id").cloned().filter(|v| !v.is_null())?;
    let result = match method {
        "initialize" => {
            let asked = params.get("protocolVersion").and_then(Value::as_str).unwrap_or(PROTOCOLS[0]);
            let version = if PROTOCOLS.contains(&asked) { asked } else { PROTOCOLS[0] };
            Ok(json!({
                "protocolVersion": version,
                "capabilities": { "tools": { "listChanged": false }, "prompts": { "listChanged": false } },
                "serverInfo": { "name": "pictesa", "title": "Pictesa", "version": env!("CARGO_PKG_VERSION") },
                "instructions": INSTRUCTIONS,
            }))
        }
        "ping" => Ok(json!({})),
        "tools/list" => Ok(json!({ "tools": tools() })),
        "tools/call" => {
            let name = params.get("name").and_then(Value::as_str).unwrap_or("");
            let args = params.get("arguments").cloned().unwrap_or(json!({}));
            Ok(match call(ctx, name, &args).await {
                Ok(content) => json!({ "content": content, "isError": false }),
                Err(e) => json!({ "content": [{ "type": "text", "text": e }], "isError": true }),
            })
        }
        "resources/list" => Ok(json!({ "resources": [] })),
        "prompts/list" => Ok(json!({ "prompts": ctx.engine.packs().iter().map(prompt_json).collect::<Vec<_>>() })),
        "prompts/get" => get_prompt(ctx, &params).await,
        _ => Err((-32601, format!("Method not found: {method}"))),
    };
    Some(match result {
        Ok(r) => json!({ "jsonrpc": "2.0", "id": id, "result": r }),
        Err((code, message)) => json!({ "jsonrpc": "2.0", "id": id, "error": { "code": code, "message": message } }),
    })
}

/// A skill pack as an MCP prompt.
fn prompt_json(p: &Pack) -> Value {
    json!({
        "name": p.name, "title": p.title, "description": p.description,
        "arguments": p.arguments.iter().map(|a| json!({ "name": a.name, "description": a.description, "required": a.required })).collect::<Vec<_>>(),
    })
}

/// A skill pack's instructions for the given arguments. Unknown packs and missing or invalid
/// arguments are invalid params (-32602), as the protocol asks.
async fn get_prompt(ctx: &McpContext, params: &Value) -> Result<Value, (i64, String)> {
    let name = params.get("name").and_then(Value::as_str).ok_or((-32602, "name is required".to_string()))?;
    let args = match params.get("arguments") {
        None | Some(Value::Null) => Map::new(),
        Some(Value::Object(args)) => args.clone(),
        Some(_) => return Err((-32602, "arguments must be an object of text values".into())),
    };
    match ctx.engine.pack_prompt(&ctx.user_id, name, &args).await {
        Ok((pack, text)) => Ok(json!({
            "description": pack.description,
            "messages": [{ "role": "user", "content": { "type": "text", "text": text } }],
        })),
        Err(e @ (EngineError::Validation { .. } | EngineError::NotFound(_))) => Err((-32602, e.to_string())),
        Err(e) => Err((-32603, e.to_string())),
    }
}

fn tools() -> Value {
    let input_item = json!({
        "type": "object",
        "properties": {
            "role": { "type": "string", "enum": ["reference", "start_frame", "end_frame", "video", "audio", "mask"] },
            "path": { "type": "string", "description": "Local file path (local mode only)" },
            "url": { "type": "string", "description": "http(s) URL to import" },
            "asset_id": { "type": "string", "description": "An asset already in the Pictesa library" }
        },
        "required": ["role"]
    });
    let prompt_tools = json!([
        {
            "name": "list_prompt_history", "description": "Search your distinct past prompts, newest first, with last model and use time.",
            "inputSchema": { "type": "object", "properties": { "q": { "type": "string" }, "limit": { "type": "integer", "minimum": 1, "maximum": 100 } } },
            "annotations": { "readOnlyHint": true }
        },
        {
            "name": "list_saved_prompts", "description": "Search your saved prompts and templates, including settings and placeholder names.",
            "inputSchema": { "type": "object", "properties": { "q": { "type": "string" }, "limit": { "type": "integer", "minimum": 1, "maximum": 100 } } },
            "annotations": { "readOnlyHint": true }
        },
        {
            "name": "save_prompt", "description": "Save a personal prompt or {placeholder} template with optional model settings.",
            "inputSchema": { "type": "object", "properties": {
                "name": { "type": "string" }, "prompt": { "type": "string" }, "negative_prompt": { "type": "string" },
                "model": { "type": "string" }, "params": { "type": "object" }
            }, "required": ["name", "prompt"] }
        },
        {
            "name": "generate_batch", "description": "Expand prompts x seeds x parameter grid, up to 64 runs in one atomic batch. Call with dry_run:true first for count and total estimated USD. Queuing can cost money. Returns job ids for wait_for_job.",
            "inputSchema": { "type": "object", "properties": {
                "model": { "type": "string" }, "prompt": { "type": "string" }, "negative_prompt": { "type": "string" },
                "params": { "type": "object" }, "inputs": { "type": "array", "items": input_item },
                "preset": { "type": "string" }, "character": { "type": "string" },
                "prompts": { "type": "array", "items": { "type": "string" }, "maxItems": 64 },
                "prompt_list": { "type": "string" }, "prompt_format": { "type": "string", "enum": ["lines", "csv"], "default": "lines" },
                "seeds": { "type": "object", "properties": { "start": { "type": "integer", "minimum": 0, "maximum": 9007199254740991u64 }, "count": { "type": "integer", "minimum": 1, "maximum": 64 } }, "required": ["start", "count"] },
                "grid": { "type": "array", "maxItems": 3, "items": { "type": "object", "properties": { "key": { "type": "string" }, "values": { "type": "array", "minItems": 1, "items": {} } }, "required": ["key", "values"] } },
                "dry_run": { "type": "boolean", "default": false }
            }, "required": ["model"] },
            "annotations": { "readOnlyHint": false, "openWorldHint": true }
        }
    ]);
    let mut tools = json!([
        {"name":"train_lora","description":"Queue LoRA training from a character or library images. fal training costs money; local ai-toolkit is free. Discover trainers with list_models output:lora. Returns normal job IDs; cancel_job and wait_for_job also work. Use dry_run:true to validate and estimate first.",
         "inputSchema":{"type":"object","properties":{"trainer":{"type":"string"},"character":{"type":"string"},"asset_ids":{"type":"array","items":{"type":"string"}},"trigger_word":{"type":"string"},"family":{"type":"string","enum":["flux1","qwen_image","qwen_image_2512","wan21"]},"steps":{"type":"integer","default":1000},"rank":{"type":"integer","default":16},"learning_rate":{"type":"number","default":0.0001},"dry_run":{"type":"boolean","default":false}},"required":["trainer","trigger_word","family"]}},
        {"name":"get_training","description":"Read a training job's status, progress and saved weights.","inputSchema":{"type":"object","properties":{"id":{"type":"string"}},"required":["id"]},"annotations":{"readOnlyHint":true}},
        {"name":"list_loras","description":"List your trained LoRAs with trigger word, character and base model family.","inputSchema":{"type":"object","properties":{"family":{"type":"string"},"character":{"type":"string"}}},"annotations":{"readOnlyHint":true}},
        {
            "name": "list_collections", "description": "List your named library collections.",
            "inputSchema": { "type": "object", "properties": {} },
            "annotations": { "readOnlyHint": true }
        },
        {
            "name": "add_to_collection", "description": "Add library assets to one of your collections.",
            "inputSchema": { "type": "object", "properties": {
                "collection_id": { "type": "string" },
                "asset_ids": { "type": "array", "items": { "type": "string" }, "minItems": 1, "maxItems": 100 }
            }, "required": ["collection_id", "asset_ids"] }
        },
        {
            "name": "list_models",
            "title": "List models",
            "description": "List generation and training models. By default only models whose provider is set up. Each model carries its LMArena rank per task when rated (human-preference leaderboards, updated daily).",
            "inputSchema": { "type": "object", "properties": {
                "output": { "type": "string", "enum": ["image", "video", "audio", "lora"] },
                "task": { "type": "string", "enum": ["text_to_image", "image_edit", "text_to_video", "image_to_video", "video_to_video", "video_to_audio", "upscale", "lip_sync", "text_to_audio", "training"] },
                "q": { "type": "string", "description": "Search in name, id and maker" },
                "provider": { "type": "string" },
                "configured_only": { "type": "boolean", "default": true },
                "featured_only": { "type": "boolean", "default": false },
                "sort": { "type": "string", "enum": ["recommended", "arena", "newest", "name"], "description": "arena: best rated by people first (LMArena, per task); newest: latest release first" },
                "limit": { "type": "integer", "default": 40, "maximum": 300 }
            } },
            "annotations": { "readOnlyHint": true }
        },
        {
            "name": "get_model",
            "title": "Model details",
            "description": "Full parameters and input slots of one model, by id (provider:model).",
            "inputSchema": { "type": "object", "properties": { "id": { "type": "string" } }, "required": ["id"] },
            "annotations": { "readOnlyHint": true }
        },
        {
            "name": "list_presets",
            "title": "List presets",
            "description": "Camera moves and looks that can be passed to generate as preset.",
            "inputSchema": { "type": "object", "properties": { "output": { "type": "string", "enum": ["image", "video"] } } },
            "annotations": { "readOnlyHint": true }
        },
        {
            "name": "generate",
            "title": "Generate",
            "description": "Generate images or video with a model. Waits for the result by default and returns local file paths and library ids. This can cost money on paid providers.",
            "inputSchema": { "type": "object", "properties": {
                "model": { "type": "string", "description": "Model id from list_models, e.g. fal:fal-ai/flux/dev" },
                "prompt": { "type": "string" },
                "negative_prompt": { "type": "string" },
                "params": { "type": "object", "description": "Model parameters by key (see get_model), e.g. {\"aspect_ratio\":\"16:9\",\"duration\":5}" },
                "inputs": { "type": "array", "items": input_item },
                "count": { "type": "integer", "minimum": 1, "maximum": 16, "default": 1 },
                "preset": { "type": "string", "description": "Preset id from list_presets" },
                "compare": { "type": "array", "items": { "type": "string" }, "maxItems": 7, "description": "Also run the same request on these model ids, for a side-by-side comparison" },
                "character": { "type": "string", "description": "Character id from list_characters: attaches its reference images" },
                "wait": { "type": "boolean", "default": true },
                "timeout_seconds": { "type": "integer", "default": 900, "maximum": 3600 },
                "include_preview": { "type": "boolean", "default": false, "description": "Attach small JPEG previews of the results" }
            }, "required": ["model"] },
            "annotations": { "readOnlyHint": false, "openWorldHint": true }
        },
        {
            "name": "import_workflow",
            "title": "Import a ComfyUI workflow",
            "description": "Turn a ComfyUI workflow (API format, exported with Workflow > Export (API)) into a Pictesa model. Pictesa finds its prompt, seed, steps, size and file inputs. Returns the new model id for generate.",
            "inputSchema": { "type": "object", "properties": {
                "name": { "type": "string" },
                "path": { "type": "string", "description": "Path to the workflow JSON (local mode)" },
                "workflow": { "type": "object", "description": "The workflow JSON itself" }
            }, "required": ["name"] }
        },
        {
            "name": "list_characters",
            "title": "List characters",
            "description": "Saved characters (named sets of reference images for a person, product or style) and brand kits (kind brand: references plus brand colors and words). generate and run_recipe attach one with `character`.",
            "inputSchema": { "type": "object", "properties": {} },
            "annotations": { "readOnlyHint": true }
        },
        {
            "name": "list_recipes",
            "title": "List recipes",
            "description": "The studio's apps as recipes: shots, angles, zooms, relight, face swap, product placement, color grade, upscale, UGC ad, hook variants, explainer and more. Each lists its fields, input files, steps, the model each step would use, and what to set up when a step has none.",
            "inputSchema": { "type": "object", "properties": {} },
            "annotations": { "readOnlyHint": true }
        },
        {
            "name": "run_recipe",
            "title": "Run a recipe",
            "description": "Run a recipe and wait for it. values fills its fields (a multi field takes a list); inputs maps each recipe input key to files as {path|url|asset_id}. results holds the final files: a set when the last step runs once per item. This can cost money on paid providers.",
            "inputSchema": { "type": "object", "properties": {
                "id": { "type": "string" },
                "values": { "type": "object" },
                "character": { "type": "string", "description": "A character or brand kit (id or name from list_characters) applied to every step" },
                "inputs": { "type": "object", "additionalProperties": { "type": "array", "items": { "type": "object", "properties": { "path": { "type": "string" }, "url": { "type": "string" }, "asset_id": { "type": "string" } } } } },
                "models": { "type": "object", "description": "Optional model id per step id" },
                "timeout_seconds": { "type": "integer", "default": 1800, "maximum": 7200 }
            }, "required": ["id"] },
            "annotations": { "readOnlyHint": false, "openWorldHint": true }
        },
        {
            "name": "get_job",
            "title": "Generation status",
            "description": "Status, progress and outputs of a generation.",
            "inputSchema": { "type": "object", "properties": { "id": { "type": "string" } }, "required": ["id"] },
            "annotations": { "readOnlyHint": true }
        },
        {
            "name": "wait_for_job",
            "title": "Wait for generation",
            "description": "Wait until a generation finishes (or the timeout passes) and return it.",
            "inputSchema": { "type": "object", "properties": { "id": { "type": "string" }, "timeout_seconds": { "type": "integer", "default": 600, "maximum": 3600 } }, "required": ["id"] },
            "annotations": { "readOnlyHint": true }
        },
        {
            "name": "cancel_job",
            "title": "Cancel generation",
            "description": "Cancel a queued or running generation.",
            "inputSchema": { "type": "object", "properties": { "id": { "type": "string" } }, "required": ["id"] }
        },
        {
            "name": "list_assets",
            "title": "Search the library",
            "description": "Recent images and videos in the library, newest first. q searches prompts.",
            "inputSchema": { "type": "object", "properties": {
                "kind": { "type": "string", "enum": ["image", "video", "audio", "lora"] },
                "q": { "type": "string" },
                "favorite": { "type": "boolean" },
                "limit": { "type": "integer", "default": 20, "maximum": 200 }
            } },
            "annotations": { "readOnlyHint": true }
        },
        {
            "name": "get_asset",
            "title": "Library item",
            "description": "One library item with its file path and the generation that made it.",
            "inputSchema": { "type": "object", "properties": { "id": { "type": "string" } }, "required": ["id"] },
            "annotations": { "readOnlyHint": true }
        },
        {
            "name": "import_asset",
            "title": "Add to library",
            "description": "Save an image or video from another tool with its prompt, model, settings and origin. Local paths work only in local mode. Reimports fill missing metadata without replacing existing values.",
            "inputSchema": { "type": "object", "properties": {
                "path": { "type": "string" }, "url": { "type": "string" },
                "prompt": { "type": "string" }, "negative_prompt": { "type": "string" },
                "origin": { "type": "string" }, "model": { "type": "string" },
                "model_label": { "type": "string" }, "params": { "type": "object" },
                "source_url": { "type": "string" }
            } }
        }
    ]);
    tools.as_array_mut().unwrap().extend(prompt_tools.as_array().unwrap().iter().cloned());
    tools.as_array_mut().unwrap().extend(board_tools());
    tools
}

/// Boards: read them, add to them and run their steps.
fn board_tools() -> Vec<Value> {
    vec![
        json!({
            "name": "list_boards",
            "title": "List boards",
            "description": "Boards you own or that were shared with you: canvases of library files, notes and steps that chain generations.",
            "inputSchema": { "type": "object", "properties": {} },
            "annotations": { "readOnlyHint": true }
        }),
        json!({
            "name": "create_board",
            "title": "Create a board",
            "description": "Create an empty board for references, notes and chained generation steps.",
            "inputSchema": { "type": "object", "properties": { "name": { "type": "string" } }, "required": ["name"] }
        }),
        json!({
            "name": "get_board",
            "title": "Read a board",
            "description": "A board's nodes (asset, note, step), edges, the latest run of each step and its files (with local paths in local mode).",
            "inputSchema": { "type": "object", "properties": { "id": { "type": "string" } }, "required": ["id"] },
            "annotations": { "readOnlyHint": true }
        }),
        json!({
            "name": "add_to_board",
            "title": "Add to a board",
            "description": "Add nodes and edges to a board. People who have it open see them at once. A node is {kind: \"asset\", asset_id | url | path}, {kind: \"note\", text} or {kind: \"step\", model, prompt, params, count}. Give new nodes an id to connect them in the same call. An edge from a file or a step into a step feeds it: role picks the model's slot, otherwise the first free one. Nodes without x and y go in a row below the board's content.",
            "inputSchema": { "type": "object", "properties": {
                "board_id": { "type": "string" },
                "nodes": { "type": "array", "maxItems": 100, "items": { "type": "object", "properties": {
                    "id": { "type": "string", "description": "Optional: 1 to 64 letters, digits, - or _" },
                    "kind": { "type": "string", "enum": ["asset", "note", "step"] },
                    "x": { "type": "number" }, "y": { "type": "number" }, "w": { "type": "number" },
                    "asset_id": { "type": "string" }, "url": { "type": "string" }, "path": { "type": "string", "description": "Local file path (local mode only)" },
                    "text": { "type": "string" },
                    "model": { "type": "string", "description": "Model id from list_models" },
                    "prompt": { "type": "string" }, "negative_prompt": { "type": "string" },
                    "params": { "type": "object" },
                    "count": { "type": "integer", "minimum": 1, "maximum": 8 }
                }, "required": ["kind"] } },
                "edges": { "type": "array", "maxItems": 100, "items": { "type": "object", "properties": {
                    "from": { "type": "string" }, "to": { "type": "string" },
                    "role": { "type": "string", "enum": ["reference", "start_frame", "end_frame", "video", "audio", "mask"] }
                }, "required": ["from", "to"] } }
            }, "required": ["board_id"] }
        }),
        json!({
            "name": "run_board_step",
            "title": "Run a board step",
            "description": "Run a step node as an ordinary generation; its results land on the board, linked to the step. downstream also runs every step that takes its results, in order. Waits by default and returns the runs and the new files. This can cost money on paid providers.",
            "inputSchema": { "type": "object", "properties": {
                "board_id": { "type": "string" },
                "node_id": { "type": "string", "description": "A step node id from get_board" },
                "downstream": { "type": "boolean", "default": false },
                "wait": { "type": "boolean", "default": true },
                "timeout_seconds": { "type": "integer", "default": 900, "maximum": 3600 }
            }, "required": ["board_id", "node_id"] },
            "annotations": { "readOnlyHint": false, "openWorldHint": true }
        }),
    ]
}

fn text(v: &Value) -> Vec<Value> {
    vec![json!({ "type": "text", "text": serde_json::to_string_pretty(v).unwrap_or_default() })]
}

fn err(e: EngineError) -> String {
    e.to_string()
}

fn arg_str<'a>(args: &'a Value, key: &str) -> Option<&'a str> {
    args.get(key).and_then(Value::as_str).filter(|s| !s.is_empty())
}

async fn asset_json(ctx: &McpContext, a: &Asset) -> Value {
    let mut v = json!({
        "id": a.id, "kind": a.kind, "mime": a.mime, "width": a.width, "height": a.height,
        "duration_ms": a.duration_ms, "size": a.size, "model": a.model, "prompt": a.prompt, "url": a.url,
        "negative_prompt": a.negative_prompt, "origin": a.origin, "model_label": a.model_label,
        "params": a.params, "source_url": a.source_url,
    });
    if ctx.local_files {
        if let Ok((path, _, _)) = ctx.engine.asset_file(Some(&ctx.user_id), &a.id).await {
            v["path"] = json!(path.display().to_string());
        }
    }
    v
}

async fn job_json(ctx: &McpContext, j: &Job) -> Value {
    let mut outputs = Vec::new();
    for a in &j.outputs {
        outputs.push(asset_json(ctx, a).await);
    }
    json!({
        "id": j.id, "status": j.status, "model": j.model, "prompt": j.prompt, "progress": j.progress,
        "message": j.message, "error": j.error, "cost_usd": j.cost_usd, "outputs": outputs,
    })
}

async fn previews(ctx: &McpContext, jobs: &[Job]) -> Vec<Value> {
    let mut out = Vec::new();
    for a in jobs.iter().flat_map(|j| j.outputs.iter()).take(8) {
        if let Ok(path) = ctx.engine.asset_thumb(Some(&ctx.user_id), &a.id).await {
            if let Ok(bytes) = tokio::fs::read(&path).await {
                out.push(json!({ "type": "image", "mimeType": "image/jpeg", "data": base64::engine::general_purpose::STANDARD.encode(bytes) }));
            }
        }
    }
    out
}

async fn resolve_input(ctx: &McpContext, item: &Value) -> Result<JobInput, String> {
    let role: InputRole = serde_json::from_value(item.get("role").cloned().unwrap_or(Value::Null)).map_err(|_| "each input needs a role".to_string())?;
    if let Some(id) = arg_str(item, "asset_id") {
        return Ok(JobInput { asset_id: id.to_string(), role });
    }
    if let Some(url) = arg_str(item, "url") {
        let a = ctx.engine.import_url(&ctx.user_id, url).await.map_err(err)?;
        return Ok(JobInput { asset_id: a.id, role });
    }
    if let Some(path) = arg_str(item, "path") {
        let a = import_path(ctx, path, &pictesa_engine::ImportMetadata::default()).await?;
        return Ok(JobInput { asset_id: a.id, role });
    }
    Err("each input needs asset_id, url or path".into())
}

async fn import_path(ctx: &McpContext, path: &str, metadata: &pictesa_engine::ImportMetadata) -> Result<Asset, String> {
    if !ctx.local_files {
        return Err("file paths only work in local mode; pass a url instead".into());
    }
    let bytes = tokio::fs::read(path).await.map_err(|e| format!("cannot read {path}: {e}"))?;
    let name = std::path::Path::new(path).file_name().map(|n| n.to_string_lossy().into_owned());
    ctx.engine.upload_with_metadata(&ctx.user_id, &bytes, name.as_deref(), None, metadata).await.map_err(err)
}

async fn call(ctx: &McpContext, name: &str, args: &Value) -> Result<Vec<Value>, String> {
    let e = &ctx.engine;
    let user = ctx.user_id.as_str();
    match name {
        "list_collections" => Ok(text(&json!({ "collections": e.collections(user).await.map_err(err)? }))),
        "add_to_collection" => {
            let collection = arg_str(args, "collection_id").ok_or("collection_id is required")?;
            let ids: Vec<String> = serde_json::from_value(args.get("asset_ids").cloned().unwrap_or(Value::Null)).map_err(|_| "asset_ids must be an array of ids")?;
            e.collection_assets(user, collection, &ids, false).await.map_err(err)?;
            Ok(text(&json!({ "ok": true })))
        },
        "list_models" => {
            let filter = ModelFilter {
                output: arg_str(args, "output").map(String::from),
                task: arg_str(args, "task").map(String::from),
                provider: arg_str(args, "provider").map(String::from),
                q: arg_str(args, "q").map(String::from),
                configured: Some(args.get("configured_only").and_then(Value::as_bool).unwrap_or(true)),
                featured: args.get("featured_only").and_then(Value::as_bool),
                sort: arg_str(args, "sort").map(String::from),
            };
            let limit = args.get("limit").and_then(Value::as_u64).unwrap_or(40).min(300) as usize;
            let models = e.models(user, &filter).await.map_err(err)?;
            if models.is_empty() && filter.configured == Some(true) {
                return Ok(text(&json!({
                    "models": [],
                    "hint": "No matching models on a configured provider. The user can add an API key in Pictesa Settings > Providers, or run a local ComfyUI. Call again with configured_only=false to see everything."
                })));
            }
            let list: Vec<Value> = models
                .iter()
                .take(limit)
                .map(|m| {
                    let s = &m.spec;
                    json!({
                        "id": s.id, "name": s.name, "vendor": s.vendor, "output": s.output, "tasks": s.tasks,
                        "inputs": s.inputs.iter().map(|i| json!({"role": i.role, "max": i.max, "required": i.min > 0})).collect::<Vec<_>>(),
                        "pricing": s.pricing, "open_weights": s.open_weights, "configured": m.configured,
                        "featured": s.tags.iter().any(|t| t == "featured"),
                        "arena": m.arena.iter().map(|a| json!({"arena": a.arena, "rank": a.rank, "of": a.of, "rating": a.rating.round()})).collect::<Vec<_>>(),
                        "released": m.released,
                    })
                })
                .collect();
            Ok(text(&json!({ "total": models.len(), "models": list })))
        }
        "get_model" => {
            let id = arg_str(args, "id").ok_or("id is required")?;
            Ok(text(&json!(e.model(user, id).await.map_err(err)?)))
        }
        "list_presets" => {
            let output: Option<MediaKind> = arg_str(args, "output").and_then(|o| serde_json::from_value(json!(o)).ok());
            let list: Vec<Value> = e.presets(output).iter().map(|p| json!({"id": p.id, "name": p.name, "output": p.output, "description": p.description})).collect();
            Ok(text(&json!({ "presets": list })))
        }
        "list_prompt_history" | "list_saved_prompts" => {
            let filter = serde_json::from_value(args.clone()).map_err(|e| e.to_string())?;
            if name == "list_prompt_history" { Ok(text(&json!({ "history": e.prompt_history(user, &filter).await.map_err(err)? }))) }
            else { Ok(text(&json!({ "prompts": e.saved_prompts(user, &filter).await.map_err(err)? }))) }
        }
        "save_prompt" => {
            let input = serde_json::from_value(args.clone()).map_err(|e| e.to_string())?;
            Ok(text(&json!(e.save_prompt(user, input).await.map_err(err)?)))
        }
        "train_lora" => {
            let mut body=args.clone();
            let dry=match body.as_object_mut().and_then(|b|b.remove("dry_run")) {
                None=>false, Some(Value::Bool(value))=>value, _=>return Err("dry_run must be a boolean.".into()),
            };
            let req=serde_json::from_value(body).map_err(|e|e.to_string())?;
            if dry {Ok(text(&json!(e.estimate_training(user,req).await.map_err(err)?)))} else {Ok(text(&json!(e.train(user,req).await.map_err(err)?)))}
        }
        "get_training" => {
            let job=e.job(user,arg_str(args,"id").ok_or("id is required")?).await.map_err(err)?;
            if job.output!=MediaKind::Lora {return Err("Training job not found.".into());} Ok(text(&json!(job)))
        }
        "list_loras" => Ok(text(&json!({"loras":e.loras(user,arg_str(args,"family"),arg_str(args,"character")).await.map_err(err)?}))),
        "generate_batch" => {
            let mut body = args.as_object().cloned().ok_or("arguments must be an object")?;
            let mut inputs = Vec::new();
            for item in args.get("inputs").and_then(Value::as_array).into_iter().flatten() { inputs.push(resolve_input(ctx, item).await?); }
            body.insert("inputs".into(), json!(inputs));
            let req: pictesa_engine::batches::BatchRequest = serde_json::from_value(Value::Object(body)).map_err(|e| e.to_string())?;
            if req.dry_run { Ok(text(&json!(e.estimate_batch(user, req).await.map_err(err)?))) }
            else { Ok(text(&json!(e.create_batch(user, req).await.map_err(err)?))) }
        }
        "generate" => {
            let model = arg_str(args, "model").ok_or("model is required (call list_models)")?;
            let mut inputs = Vec::new();
            for item in args.get("inputs").and_then(Value::as_array).into_iter().flatten() {
                inputs.push(resolve_input(ctx, item).await?);
            }
            let req = CreateGeneration {
                model: model.to_string(),
                prompt: arg_str(args, "prompt").unwrap_or("").to_string(),
                negative_prompt: arg_str(args, "negative_prompt").map(String::from),
                params: args.get("params").and_then(Value::as_object).cloned().unwrap_or_else(Map::new),
                inputs,
                count: args.get("count").and_then(Value::as_u64).map(|c| c as u32),
                preset: arg_str(args, "preset").map(String::from),
                compare: args.get("compare").and_then(Value::as_array).map(|a| a.iter().filter_map(|v| v.as_str().map(String::from)).collect()).unwrap_or_default(),
                character: arg_str(args, "character").map(String::from),
            };
            let created = e.create_generation(user, req).await.map_err(err)?;
            if args.get("wait").and_then(Value::as_bool) == Some(false) {
                let mut jobs = Vec::new();
                for j in &created.jobs {
                    jobs.push(job_json(ctx, j).await);
                }
                return Ok(text(&json!({ "batch_id": created.batch_id, "jobs": jobs, "note": "Call wait_for_job with each id." })));
            }
            let timeout = Duration::from_secs(args.get("timeout_seconds").and_then(Value::as_u64).unwrap_or(900).clamp(5, 3600));
            let mut done = Vec::new();
            for j in &created.jobs {
                done.push(e.wait_job(user, &j.id, timeout).await.map_err(err)?);
            }
            let mut jobs = Vec::new();
            for j in &done {
                jobs.push(job_json(ctx, j).await);
            }
            let mut content = text(&json!({ "batch_id": created.batch_id, "jobs": jobs }));
            if args.get("include_preview").and_then(Value::as_bool) == Some(true) {
                content.extend(previews(ctx, &done).await);
            }
            Ok(content)
        }
        "import_workflow" => {
            let name = arg_str(args, "name").ok_or("name is required")?;
            let workflow = match (args.get("workflow").filter(|w| w.is_object()), arg_str(args, "path")) {
                (Some(w), _) => w.clone(),
                (None, Some(path)) if ctx.local_files => {
                    let text = tokio::fs::read_to_string(path).await.map_err(|e| format!("cannot read {path}: {e}"))?;
                    serde_json::from_str(&text).map_err(|e| format!("{path} is not JSON: {e}"))?
                }
                (None, Some(_)) => return Err("file paths only work in local mode; pass workflow instead".into()),
                (None, None) => return Err("pass workflow or path".into()),
            };
            Ok(text(&json!(e.import_comfy_workflow(user, name, workflow).await.map_err(err)?)))
        }
        "list_characters" => {
            let list: Vec<Value> = e
                .characters(user)
                .await
                .map_err(err)?
                .iter()
                .map(|c| json!({
                    "id": c.id, "name": c.name, "kind": c.kind, "description": c.description, "images": c.assets.len(),
                    "image_ids": c.assets.iter().map(|a| a.id.as_str()).collect::<Vec<_>>(), "colors": c.colors, "words": c.words,
                }))
                .collect();
            Ok(text(&json!({ "characters": list })))
        }
        "list_recipes" => {
            let list: Vec<Value> = e
                .recipes(user)
                .await
                .map_err(err)?
                .iter()
                .map(|r| json!({
                    "id": r.recipe.id, "name": r.recipe.name, "description": r.recipe.description, "output": r.recipe.output,
                    "fields": r.recipe.fields, "inputs": r.recipe.inputs, "runnable": r.runnable, "character": r.recipe.character,
                    "steps": r.recipe.steps.iter().zip(&r.step_models).map(|(s, m)| {
                        let mut step = json!({"id": s.id, "label": s.label, "output": s.output, "model": m});
                        if !s.each.is_empty() {
                            step["each"] = json!(s.each);
                        }
                        if m.is_none() {
                            step["needs"] = json!(s.needs);
                        }
                        step
                    }).collect::<Vec<_>>(),
                }))
                .collect();
            Ok(text(&json!({ "recipes": list })))
        }
        "run_recipe" => {
            let id = arg_str(args, "id").ok_or("id is required (call list_recipes)")?;
            let mut req = pictesa_engine::recipes::RunRecipe {
                values: args.get("values").and_then(Value::as_object).cloned().unwrap_or_default(),
                models: args.get("models").and_then(Value::as_object).map(|m| m.iter().filter_map(|(k, v)| v.as_str().map(|s| (k.clone(), s.to_string()))).collect()).unwrap_or_default(),
                character: arg_str(args, "character").map(String::from),
                ..Default::default()
            };
            for (key, items) in args.get("inputs").and_then(Value::as_object).into_iter().flatten() {
                let mut ids = Vec::new();
                for item in items.as_array().into_iter().flatten() {
                    let Some(obj) = item.as_object() else {
                        return Err(format!("inputs.{key} items must be objects like {{\"path\": ...}}, {{\"url\": ...}} or {{\"asset_id\": ...}}"));
                    };
                    let mut tagged = obj.clone();
                    tagged.insert("role".into(), json!("reference"));
                    ids.push(resolve_input(ctx, &Value::Object(tagged)).await?.asset_id);
                }
                req.inputs.insert(key.clone(), ids);
            }
            let run = e.run_recipe(user, id, req).await.map_err(err)?;
            let timeout = Duration::from_secs(args.get("timeout_seconds").and_then(Value::as_u64).unwrap_or(1800).clamp(10, 7200));
            let deadline = tokio::time::Instant::now() + timeout;
            let mut current = run;
            while current.status == "running" && tokio::time::Instant::now() < deadline {
                tokio::time::sleep(Duration::from_secs(2)).await;
                current = e.recipe_run(user, &current.id).await.map_err(err)?;
            }
            let (mut jobs, mut results) = (Vec::new(), Vec::new());
            for j in &current.jobs {
                if let Ok(job) = e.job(user, j).await {
                    if current.results.contains(j) {
                        for a in &job.outputs {
                            results.push(asset_json(ctx, a).await);
                        }
                    }
                    jobs.push(job_json(ctx, &job).await);
                }
            }
            Ok(text(&json!({ "run": current, "results": results, "steps": jobs })))
        }
        "get_job" => {
            let id = arg_str(args, "id").ok_or("id is required")?;
            let j = e.job(user, id).await.map_err(err)?;
            Ok(text(&job_json(ctx, &j).await))
        }
        "wait_for_job" => {
            let id = arg_str(args, "id").ok_or("id is required")?;
            let timeout = Duration::from_secs(args.get("timeout_seconds").and_then(Value::as_u64).unwrap_or(600).clamp(1, 3600));
            let j = e.wait_job(user, id, timeout).await.map_err(err)?;
            Ok(text(&job_json(ctx, &j).await))
        }
        "cancel_job" => {
            let id = arg_str(args, "id").ok_or("id is required")?;
            let j = e.cancel(user, id).await.map_err(err)?;
            Ok(text(&job_json(ctx, &j).await))
        }
        "list_assets" => {
            let f = AssetFilter {
                kind: arg_str(args, "kind").map(String::from),
                q: arg_str(args, "q").map(String::from),
                favorite: args.get("favorite").and_then(Value::as_bool),
                limit: Some(args.get("limit").and_then(Value::as_u64).unwrap_or(20).min(200) as u32),
                ..Default::default()
            };
            let (assets, next) = e.assets(user, &f).await.map_err(err)?;
            let mut list = Vec::new();
            for a in &assets {
                list.push(asset_json(ctx, a).await);
            }
            Ok(text(&json!({ "assets": list, "next": next })))
        }
        "get_asset" => {
            let id = arg_str(args, "id").ok_or("id is required")?;
            let (a, job) = e.asset(user, id).await.map_err(err)?;
            let mut v = asset_json(ctx, &a).await;
            if let Some(j) = job {
                v["job"] = json!({ "id": j.id, "model": j.model, "prompt": j.prompt, "params": j.params, "inputs": j.inputs });
            }
            Ok(text(&v))
        }
        "list_boards" => Ok(text(&json!({ "boards": e.boards(user).await.map_err(err)? }))),
        "create_board" => {
            let name = arg_str(args, "name").ok_or("name is required")?;
            let board = e.create_board(user, name).await.map_err(err)?;
            Ok(text(&json!({ "id": board.id, "name": board.name, "version": board.version })))
        }
        "get_board" => {
            let id = arg_str(args, "id").ok_or("id is required (call list_boards)")?;
            let board = e.board(user, id).await.map_err(err)?;
            Ok(text(&board_json(ctx, &board).await))
        }
        "add_to_board" => add_to_board(ctx, args).await,
        "run_board_step" => run_board_step(ctx, args).await,
        "import_asset" => {
            let metadata: pictesa_engine::ImportMetadata = serde_json::from_value(args.clone()).map_err(|e| format!("invalid import metadata: {e}"))?;
            let a = if let Some(url) = arg_str(args, "url") {
                e.import_url_with_metadata(user, url, &metadata).await.map_err(err)?
            } else if let Some(path) = arg_str(args, "path") {
                import_path(ctx, path, &metadata).await?
            } else {
                return Err("pass path or url".into());
            };
            Ok(text(&asset_json(ctx, &a).await))
        }
        other => Err(format!("unknown tool {other}")),
    }
}

async fn board_json(ctx: &McpContext, board: &Board) -> Value {
    let mut assets = Map::new();
    for (id, asset) in &board.assets {
        assets.insert(id.clone(), asset_json(ctx, asset).await);
    }
    json!({
        "id": board.id, "name": board.name, "role": board.role, "version": board.version,
        "nodes": board.nodes, "edges": board.edges, "runs": board.runs, "assets": assets,
    })
}

async fn add_to_board(ctx: &McpContext, args: &Value) -> Result<Vec<Value>, String> {
    let e = &ctx.engine;
    let board_id = arg_str(args, "board_id").ok_or("board_id is required (call list_boards)")?;
    let current = e.board(&ctx.user_id, board_id).await.map_err(err)?;
    // Nodes without a position go in a row below everything on the board.
    let mut x = current.nodes.iter().map(|n| n.x).fold(f64::INFINITY, f64::min);
    if !x.is_finite() {
        x = 0.0;
    }
    let y = current.nodes.iter().map(|n| n.y + 420.0).fold(0.0, f64::max);
    let mut ops = Vec::new();
    for item in args.get("nodes").and_then(Value::as_array).into_iter().flatten() {
        let kind = arg_str(item, "kind").ok_or("each node needs kind: asset, note or step")?;
        let mut node = Map::new();
        node.insert("id".into(), json!(arg_str(item, "id").map(String::from).unwrap_or_else(pictesa_engine::new_id)));
        node.insert("kind".into(), json!(kind));
        for key in ["x", "y", "w", "text", "model", "prompt", "negative_prompt", "params", "count"] {
            if let Some(value) = item.get(key).filter(|v| !v.is_null()) {
                node.insert(key.into(), value.clone());
            }
        }
        if kind == "asset" {
            let asset = if let Some(id) = arg_str(item, "asset_id") {
                id.to_string()
            } else if let Some(url) = arg_str(item, "url") {
                e.import_url(&ctx.user_id, url).await.map_err(err)?.id
            } else if let Some(path) = arg_str(item, "path") {
                import_path(ctx, path, &pictesa_engine::ImportMetadata::default()).await?.id
            } else {
                return Err("asset nodes need asset_id, url or path".into());
            };
            node.insert("asset_id".into(), json!(asset));
        }
        if !node.contains_key("x") || !node.contains_key("y") {
            node.insert("x".into(), json!(x));
            node.insert("y".into(), json!(y));
            x += 340.0;
        }
        ops.push(json!({ "op": "add_node", "node": node }));
    }
    for item in args.get("edges").and_then(Value::as_array).into_iter().flatten() {
        let (Some(from), Some(to)) = (arg_str(item, "from"), arg_str(item, "to")) else {
            return Err("each edge needs from and to (node ids)".into());
        };
        let role = item.get("role").cloned().unwrap_or(Value::Null);
        ops.push(json!({ "op": "add_edge", "edge": { "id": pictesa_engine::new_id(), "from": from, "to": to, "role": role } }));
    }
    if ops.is_empty() {
        return Err("pass nodes or edges".into());
    }
    let ops: Vec<Op> = serde_json::from_value(Value::Array(ops)).map_err(|e| format!("a node or edge is not valid: {e}"))?;
    match e.apply_board_ops(&ctx.user_id, board_id, ops).await.map_err(err)? {
        Ok(changes) => Ok(text(&json!({ "version": changes.version, "nodes": changes.nodes, "edges": changes.edges }))),
        Err(stale) => Err(format!("{} Read the board again with get_board.", stale.message)),
    }
}

async fn run_board_step(ctx: &McpContext, args: &Value) -> Result<Vec<Value>, String> {
    let e = &ctx.engine;
    let board_id = arg_str(args, "board_id").ok_or("board_id is required (call list_boards)")?;
    let node_id = arg_str(args, "node_id").ok_or("node_id is required (a step node id from get_board)")?;
    let downstream = args.get("downstream").and_then(Value::as_bool).unwrap_or(false);
    let started = e.run_board_step(&ctx.user_id, board_id, node_id, downstream).await.map_err(err)?;
    if args.get("wait").and_then(Value::as_bool) == Some(false) {
        return Ok(text(&json!({ "steps": started.steps, "run": started.run, "note": "Call get_board to follow the runs." })));
    }
    let timeout = Duration::from_secs(args.get("timeout_seconds").and_then(Value::as_u64).unwrap_or(900).clamp(5, 3600));
    let deadline = tokio::time::Instant::now() + timeout;
    loop {
        // Moves the runs forward even when no Pictesa server runs next to this process.
        e.advance_board(board_id).await.map_err(err)?;
        let board = e.board(&ctx.user_id, board_id).await.map_err(err)?;
        let done = started.steps.iter().all(|s| board.runs.get(s).is_none_or(|r| !r.status.is_active()));
        if done || tokio::time::Instant::now() >= deadline {
            let mut runs = Map::new();
            let mut outputs = Vec::new();
            for step in &started.steps {
                let Some(run) = board.runs.get(step) else { continue };
                runs.insert(step.clone(), json!(run));
                if run.status == RunStatus::Succeeded {
                    for asset in run.outputs.iter().filter_map(|a| board.assets.get(a)) {
                        outputs.push(asset_json(ctx, asset).await);
                    }
                }
            }
            return Ok(text(&json!({ "done": done, "runs": runs, "outputs": outputs })));
        }
        tokio::time::sleep(Duration::from_secs(1)).await;
    }
}
