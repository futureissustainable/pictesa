//! Model Context Protocol server: the same tools over Streamable HTTP (`POST /mcp`) and stdio
//! (`pictesa mcp`). Agents can list models, generate, wait for jobs and read the library.

use std::time::Duration;

use base64::Engine as _;
use pictesa_core::{InputRole, MediaKind};
use pictesa_engine::{Asset, AssetFilter, CreateGeneration, Engine, EngineError, Job, JobInput, ModelFilter};
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
roles are reference, start_frame, end_frame, video, audio, mask. Presets (list_presets) add camera moves or looks.";

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
                "capabilities": { "tools": { "listChanged": false } },
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
        "prompts/list" => Ok(json!({ "prompts": [] })),
        _ => Err((-32601, format!("Method not found: {method}"))),
    };
    Some(match result {
        Ok(r) => json!({ "jsonrpc": "2.0", "id": id, "result": r }),
        Err((code, message)) => json!({ "jsonrpc": "2.0", "id": id, "error": { "code": code, "message": message } }),
    })
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
    json!([
        {
            "name": "list_models",
            "title": "List models",
            "description": "List image and video models. By default only models whose provider is set up. Each model carries its LMArena rank per task when rated (human-preference leaderboards, updated daily).",
            "inputSchema": { "type": "object", "properties": {
                "output": { "type": "string", "enum": ["image", "video", "audio"] },
                "task": { "type": "string", "enum": ["text_to_image", "image_edit", "text_to_video", "image_to_video", "video_to_video", "upscale", "lip_sync", "text_to_audio"] },
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
            "description": "Saved characters (named sets of reference images for a person, product or style) that generate can attach with `character`.",
            "inputSchema": { "type": "object", "properties": {} },
            "annotations": { "readOnlyHint": true }
        },
        {
            "name": "list_recipes",
            "title": "List recipes",
            "description": "Multi-step pipelines (cinema shot, character shot, product hero, animate a photo, new angle, upscale) with their fields and which model each step would use.",
            "inputSchema": { "type": "object", "properties": {} },
            "annotations": { "readOnlyHint": true }
        },
        {
            "name": "run_recipe",
            "title": "Run a recipe",
            "description": "Run a recipe and wait for it. values fills its fields; inputs maps each recipe input key to files as {path|url|asset_id}. This can cost money on paid providers.",
            "inputSchema": { "type": "object", "properties": {
                "id": { "type": "string" },
                "values": { "type": "object" },
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
                "kind": { "type": "string", "enum": ["image", "video", "audio"] },
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
            "description": "Add a local file (local mode) or a URL to the library, to use as an input.",
            "inputSchema": { "type": "object", "properties": { "path": { "type": "string" }, "url": { "type": "string" } } }
        }
    ])
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
        let a = import_path(ctx, path).await?;
        return Ok(JobInput { asset_id: a.id, role });
    }
    Err("each input needs asset_id, url or path".into())
}

async fn import_path(ctx: &McpContext, path: &str) -> Result<Asset, String> {
    if !ctx.local_files {
        return Err("file paths only work in local mode; pass a url instead".into());
    }
    let bytes = tokio::fs::read(path).await.map_err(|e| format!("cannot read {path}: {e}"))?;
    let name = std::path::Path::new(path).file_name().map(|n| n.to_string_lossy().into_owned());
    ctx.engine.upload(&ctx.user_id, &bytes, name.as_deref(), None).await.map_err(err)
}

async fn call(ctx: &McpContext, name: &str, args: &Value) -> Result<Vec<Value>, String> {
    let e = &ctx.engine;
    let user = ctx.user_id.as_str();
    match name {
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
                .map(|c| json!({ "id": c.id, "name": c.name, "description": c.description, "images": c.assets.len() }))
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
                    "fields": r.recipe.fields, "inputs": r.recipe.inputs, "runnable": r.runnable,
                    "steps": r.recipe.steps.iter().zip(&r.step_models).map(|(s, m)| json!({"id": s.id, "label": s.label, "model": m})).collect::<Vec<_>>(),
                }))
                .collect();
            Ok(text(&json!({ "recipes": list })))
        }
        "run_recipe" => {
            let id = arg_str(args, "id").ok_or("id is required (call list_recipes)")?;
            let mut req = pictesa_engine::recipes::RunRecipe {
                values: args.get("values").and_then(Value::as_object).cloned().unwrap_or_default(),
                models: args.get("models").and_then(Value::as_object).map(|m| m.iter().filter_map(|(k, v)| v.as_str().map(|s| (k.clone(), s.to_string()))).collect()).unwrap_or_default(),
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
            let mut jobs = Vec::new();
            for j in &current.jobs {
                if let Ok(job) = e.job(user, j).await {
                    jobs.push(job_json(ctx, &job).await);
                }
            }
            Ok(text(&json!({ "run": current, "steps": jobs })))
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
        "import_asset" => {
            let a = if let Some(url) = arg_str(args, "url") {
                e.import_url(user, url).await.map_err(err)?
            } else if let Some(path) = arg_str(args, "path") {
                import_path(ctx, path).await?
            } else {
                return Err("pass path or url".into());
            };
            Ok(text(&asset_json(ctx, &a).await))
        }
        other => Err(format!("unknown tool {other}")),
    }
}
