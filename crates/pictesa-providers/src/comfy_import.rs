//! Turns any ComfyUI workflow (API format) into a Pictesa template: finds the prompt, seed, steps,
//! guidance, output size, frame count and file loaders by reading the graph, and writes a
//! `manifest.json` next to the workflow. The result is a model in the picker, a CLI command and an
//! MCP tool, without writing any glue.

use std::{
    collections::{BTreeSet, HashMap, VecDeque},
    path::{Path, PathBuf},
};

use pictesa_core::Ctx;
use serde_json::{json, Map, Value};

type Graph = Map<String, Value>;

/// What a saving node writes, judged from its class name (SaveImage, SaveImageAdvanced, SaveVideo,
/// SaveAnimatedWEBP, VHS_VideoCombine, SaveAudioMP3, ...).
fn saver_kind(class: &str) -> Option<&'static str> {
    let c = class.to_ascii_lowercase();
    if c == "vhs_videocombine" {
        return Some("video");
    }
    if !c.contains("save") || c.contains("preview") || c.contains("latent") || c.contains("model") {
        return None;
    }
    if c.contains("video") || c.contains("webm") || c.contains("animated") || c.contains("gif") {
        Some("video")
    } else if c.contains("audio") {
        Some("audio")
    } else if c.contains("image") {
        Some("image")
    } else {
        None
    }
}
/// Text inputs that hold a prompt, in the order they are preferred.
const TEXT_INPUTS: &[&str] = &["text", "prompt", "t5xxl", "clip_l", "text_g", "text_l"];

fn class(node: &Value) -> &str {
    node.get("class_type").and_then(Value::as_str).unwrap_or("")
}

fn inputs(node: &Value) -> Option<&Map<String, Value>> {
    node.get("inputs").and_then(Value::as_object)
}

/// The source node id when an input is a link (`["12", 0]`).
fn link(v: &Value) -> Option<String> {
    let a = v.as_array()?;
    if a.len() != 2 {
        return None;
    }
    match &a[0] {
        Value::String(s) => Some(s.clone()),
        Value::Number(n) => Some(n.to_string()),
        _ => None,
    }
}

fn bind(node: &str, input: &str) -> Value {
    json!([node, input])
}

/// Output slot of a link (`["12", 1]` is slot 1).
fn slot(v: &Value) -> u64 {
    v.as_array().and_then(|a| a.get(1)).and_then(Value::as_u64).unwrap_or(0)
}

/// Walks backwards from a link through the graph and returns the prompt text inputs it reaches.
/// Nodes that carry both conditionings (LTXVConditioning, WanImageToVideo, ...) output positive on
/// slot 0 and negative on slot 1, so only the matching input is followed through them.
fn trace_text(graph: &Graph, start: &Value) -> Vec<(String, String)> {
    let mut found = Vec::new();
    let mut seen = BTreeSet::new();
    let Some(first) = link(start) else { return found };
    let mut queue = VecDeque::from([(first, slot(start), 0)]);
    while let Some((id, out_slot, depth)) = queue.pop_front() {
        if depth > 8 || !seen.insert((id.clone(), out_slot)) {
            continue;
        }
        let Some(node) = graph.get(&id) else { continue };
        let Some(ins) = inputs(node) else { continue };
        let texts: Vec<&str> = TEXT_INPUTS.iter().copied().filter(|k| ins.get(*k).is_some_and(Value::is_string)).collect();
        if !texts.is_empty() {
            for t in texts {
                found.push((id.clone(), t.to_string()));
            }
            continue;
        }
        let both = ins.get("positive").is_some_and(|v| link(v).is_some()) && ins.get("negative").is_some_and(|v| link(v).is_some());
        for (name, v) in ins {
            let Some(src) = link(v) else { continue };
            if both && ((name == "positive" && out_slot != 0) || (name == "negative" && out_slot != 1)) {
                continue;
            }
            queue.push_back((src, slot(v), depth + 1));
        }
    }
    found
}

/// Nodes that (directly or through a few hops) consume `start`, with the input name they use.
fn consumers(graph: &Graph, start: &str) -> Vec<(String, String)> {
    let mut out = Vec::new();
    let mut frontier = vec![start.to_string()];
    for _ in 0..3 {
        let mut next = Vec::new();
        for (id, node) in graph {
            for (name, v) in inputs(node).into_iter().flatten() {
                if link(v).is_some_and(|src| frontier.contains(&src)) {
                    out.push((id.clone(), name.clone()));
                    next.push(id.clone());
                }
            }
        }
        frontier = next;
    }
    out
}

fn model_dir(input: &str) -> &'static str {
    match input {
        "ckpt_name" => "checkpoints",
        "unet_name" => "diffusion_models",
        "vae_name" => "vae",
        "lora_name" => "loras",
        "control_net_name" => "controlnet",
        "model_name" => "upscale_models",
        i if i.starts_with("clip_name") || i.contains("text_encoder") => "text_encoders",
        _ => "models",
    }
}

fn closest_ratio(w: f64, h: f64) -> &'static str {
    let ratios = [("1:1", 1.0), ("16:9", 16.0 / 9.0), ("9:16", 9.0 / 16.0), ("4:3", 4.0 / 3.0), ("3:4", 0.75), ("3:2", 1.5), ("2:3", 2.0 / 3.0), ("21:9", 21.0 / 9.0)];
    let r = w / h.max(1.0);
    ratios.iter().min_by(|a, b| (a.1 - r).abs().partial_cmp(&(b.1 - r).abs()).unwrap()).map(|x| x.0).unwrap_or("1:1")
}

/// Reads a workflow and proposes its manifest. Returns the manifest and notes for the person.
pub fn infer_manifest(name: &str, graph: &Graph) -> Result<(Value, Vec<String>), String> {
    if graph.contains_key("nodes") && graph.contains_key("links") {
        return Err("This is the ComfyUI editor format. In ComfyUI use Workflow > Export (API) and import that file.".into());
    }
    if graph.is_empty() || !graph.values().all(|n| n.get("class_type").is_some()) {
        return Err("This is not a ComfyUI API workflow: every node needs a class_type.".into());
    }
    let mut notes = Vec::new();

    // What it saves.
    let pick = |kind: &str| -> Vec<String> { graph.iter().filter(|(_, n)| saver_kind(class(n)) == Some(kind)).map(|(id, _)| id.clone()).collect() };
    let (output, outputs) = if !pick("video").is_empty() {
        ("video", pick("video"))
    } else if !pick("image").is_empty() {
        ("image", pick("image"))
    } else if !pick("audio").is_empty() {
        ("audio", pick("audio"))
    } else {
        return Err("The workflow saves nothing. Add a Save Image, Save Video or Save Audio node.".into());
    };

    // Prompts: whatever feeds the samplers' positive and negative conditioning. A plain
    // `conditioning` input (guiders, conditioning tweaks) counts as positive unless its node
    // flows into a `negative` input, as in SD3's zeroed and time-limited negative branch.
    let mut positive = Vec::new();
    let mut negative = Vec::new();
    for (id, node) in graph {
        for (name, v) in inputs(node).into_iter().flatten() {
            if link(v).is_none() {
                continue;
            }
            match name.as_str() {
                "positive" => positive.extend(trace_text(graph, v)),
                "conditioning" if !consumers(graph, id).iter().any(|(_, input)| input == "negative") => positive.extend(trace_text(graph, v)),
                "negative" => negative.extend(trace_text(graph, v)),
                _ => {}
            }
        }
    }
    // A text node that reaches both (negative = zeroed positive) is the prompt.
    negative.retain(|b| !positive.contains(b));
    let dedup = |v: &mut Vec<(String, String)>| {
        let mut seen = BTreeSet::new();
        v.retain(|b| seen.insert(b.clone()));
    };
    dedup(&mut positive);
    dedup(&mut negative);
    if positive.is_empty() {
        // No sampler wiring found: fall back to the first text encoder, if any.
        if let Some((id, n)) = graph.iter().find(|(_, n)| class(n).contains("TextEncode") && inputs(n).is_some_and(|i| i.get("text").is_some_and(Value::is_string))) {
            let _ = n;
            positive.push((id.clone(), "text".into()));
            notes.push(format!("Used node {id} as the prompt; check it in the workflow."));
        } else {
            notes.push("No prompt found, so the model takes no prompt.".into());
        }
    }

    // Settings found on the nodes.
    let mut seeds = Vec::new();
    let mut steps: Vec<(Value, i64)> = Vec::new();
    let mut cfgs: Vec<(Value, f64)> = Vec::new();
    let mut guidance: Vec<(Value, f64)> = Vec::new();
    let mut widths = Vec::new();
    let mut heights = Vec::new();
    let mut size_now: Option<(f64, f64)> = None;
    let mut batch = Vec::new();
    let mut frames: Vec<(String, String, i64)> = Vec::new();
    let mut fps: Option<f64> = None;
    let mut models: BTreeSet<(String, &'static str)> = BTreeSet::new();
    // A value either sits on the node, or comes from a Primitive node (then bind that node's `value`).
    let scalar = |v: &Value| -> Option<(Option<String>, Value)> {
        match link(v) {
            None => Some((None, v.clone())),
            Some(src) => {
                let node = graph.get(&src)?;
                if !class(node).starts_with("Primitive") {
                    return None;
                }
                let value = inputs(node)?.get("value")?.clone();
                (link(&value).is_none()).then_some((Some(src), value))
            }
        }
    };
    for (id, node) in graph {
        let c = class(node);
        if c.starts_with("Primitive") {
            continue;
        }
        let Some(ins) = inputs(node) else { continue };
        let latent = c.contains("Latent") || c.contains("ToVideo") || c.starts_with("Empty");
        for (name, raw) in ins {
            let Some((via, v)) = scalar(raw) else { continue };
            let at = |input: &str| match &via {
                Some(src) => bind(src, "value"),
                None => bind(id, input),
            };
            match name.as_str() {
                "seed" | "noise_seed" if v.is_i64() || v.is_u64() => seeds.push(at(name)),
                "steps" if v.is_i64() => steps.push((at(name), v.as_i64().unwrap_or(20).clamp(1, 1000))),
                "cfg" if v.is_number() => cfgs.push((at(name), v.as_f64().unwrap_or(4.0))),
                "guidance" if v.is_number() => guidance.push((at(name), v.as_f64().unwrap_or(3.5))),
                "width" if v.is_i64() && latent => {
                    widths.push(at(name));
                    let h = ins.get("height").and_then(|h| scalar(h)).and_then(|(_, h)| h.as_f64()).unwrap_or(0.0);
                    size_now.get_or_insert((v.as_f64().unwrap_or(1024.0), h));
                }
                "height" if v.is_i64() && latent => heights.push(at(name)),
                "batch_size" if v.is_i64() => batch.push(at(name)),
                "length" | "num_frames" | "video_length" | "frames" if v.is_i64() && output == "video" => {
                    let (n, i) = match &via {
                        Some(src) => (src.clone(), "value".to_string()),
                        None => (id.clone(), name.clone()),
                    };
                    frames.push((n, i, v.as_i64().unwrap_or(81).clamp(1, 100_000)));
                }
                "fps" | "frame_rate" if v.is_number() => fps = fps.or(v.as_f64().filter(|f| *f > 0.0 && *f <= 240.0)),
                n if n.ends_with("_name") || n.starts_with("clip_name") => {
                    if let Some(file) = v.as_str().filter(|f| [".safetensors", ".gguf", ".ckpt", ".pt", ".pth", ".bin"].iter().any(|e| f.ends_with(e))) {
                        models.insert((file.to_string(), model_dir(n)));
                    }
                }
                _ => {}
            }
        }
    }

    let uniq = |v: &mut Vec<Value>| {
        let mut seen = BTreeSet::new();
        v.retain(|b| seen.insert(b.to_string()));
    };
    uniq(&mut seeds);
    uniq(&mut widths);
    uniq(&mut heights);
    uniq(&mut batch);
    frames.sort();
    frames.dedup();

    // Files the person attaches, by what the loader feeds.
    let mut groups: HashMap<&'static str, (Vec<String>, &'static str, &'static str)> = HashMap::new();
    for (id, node) in graph {
        let (kind, input, default_role) = match class(node) {
            "LoadImage" => ("image", "image", "reference"),
            "LoadImageMask" => ("image", "image", "mask"),
            "LoadVideo" => ("video", "file", "video"),
            "VHS_LoadVideo" => ("video", "video", "video"),
            "LoadAudio" => ("audio", "audio", "audio"),
            _ => continue,
        };
        let mut role = default_role;
        if kind == "image" && default_role == "reference" {
            let used: Vec<String> = consumers(graph, id).into_iter().map(|(_, n)| n.to_lowercase()).collect();
            if used.iter().any(|n| n.contains("mask")) {
                role = "mask";
            } else if used.iter().any(|n| n.contains("end")) {
                role = "end_frame";
            } else if output == "video" && used.iter().any(|n| n.contains("start") || n == "image") {
                role = "start_frame";
            }
        }
        let entry = groups.entry(role).or_insert((Vec::new(), kind, input));
        entry.0.push(id.clone());
    }
    let mut inputs_def = Vec::new();
    for role in ["start_frame", "end_frame", "reference", "mask", "video", "audio"] {
        if let Some((mut nodes, kind, input)) = groups.remove(role) {
            nodes.sort_by_key(|n| n.parse::<u64>().unwrap_or(u64::MAX));
            let min = nodes.len();
            inputs_def.push(json!({ "role": role, "kind": kind, "nodes": nodes, "input": input, "min": min }));
        }
    }

    // Tasks from what goes in and what comes out.
    let has = |r: &str| inputs_def.iter().any(|i| i["role"] == r);
    let tasks: Vec<&str> = match output {
        "video" if has("video") && positive.is_empty() => vec!["upscale"],
        "video" if has("video") => vec!["video_to_video"],
        "video" if has("start_frame") || has("reference") => vec!["image_to_video"],
        "video" => vec!["text_to_video"],
        "image" if has("reference") || has("mask") => {
            if positive.is_empty() {
                vec!["upscale"]
            } else {
                vec!["image_edit"]
            }
        }
        "image" => vec!["text_to_image"],
        _ => vec!["text_to_audio"],
    };

    let mut params = Vec::new();
    if let Some((w, h)) = size_now.filter(|(w, h)| *w > 0.0 && *h > 0.0) {
        let ratios = ["1:1", "16:9", "9:16", "4:3", "3:4", "3:2", "2:3", "21:9"];
        let mut opts: Vec<Value> = ratios.iter().map(|r| json!({ "value": r, "label": r })).collect();
        if !inputs_def.is_empty() {
            opts.insert(0, json!({ "value": "auto", "label": "Match input" }));
        }
        params.push(json!({ "key": "aspect_ratio", "label": "Aspect ratio", "type": "enum", "options": opts, "default": if inputs_def.is_empty() { closest_ratio(w, h) } else { "auto" } }));
    }
    if let Some((id, name, len)) = frames.first() {
        let fps_v = fps.unwrap_or(24.0);
        let (step, offset) = if (len - 1) % 8 == 0 { (8, 1) } else if (len - 1) % 4 == 0 { (4, 1) } else { (1, 0) };
        let secs = ((*len as f64) / fps_v).round().clamp(1.0, 3600.0) as i64;
        let binds: Vec<Value> = frames.iter().map(|(i, n, _)| json!({ "node": i, "input": n, "frames": { "fps": fps_v, "step": step, "offset": offset } })).collect();
        let _ = (id, name);
        params.push(json!({ "key": "duration", "label": "Duration", "type": "int", "min": 1, "max": (secs * 3).clamp(4, 20), "default": secs, "bind": binds }));
    }
    if !seeds.is_empty() {
        params.push(json!({ "key": "seed", "label": "Seed", "type": "seed", "advanced": true, "bind": seeds }));
    }
    if let Some((_, now)) = steps.first() {
        let binds: Vec<Value> = steps.iter().map(|(b, _)| b.clone()).collect();
        params.push(json!({ "key": "steps", "label": "Steps", "type": "int", "min": 1, "max": (now * 3).max(50), "default": now, "advanced": true, "bind": binds }));
    }
    if let Some((_, now)) = guidance.first() {
        let binds: Vec<Value> = guidance.iter().map(|(b, _)| b.clone()).collect();
        params.push(json!({ "key": "guidance", "label": "Guidance", "type": "float", "min": 0.0, "max": 30.0, "step": 0.1, "default": now, "advanced": true, "bind": binds }));
    }
    if let Some((_, now)) = cfgs.first() {
        let key = if guidance.is_empty() { "guidance" } else { "cfg" };
        let binds: Vec<Value> = cfgs.iter().map(|(b, _)| b.clone()).collect();
        params.push(json!({ "key": key, "label": if key == "cfg" { "CFG" } else { "Guidance" }, "type": "float", "min": 0.0, "max": 30.0, "step": 0.1, "default": now, "advanced": true, "bind": binds }));
    }

    let mut manifest = json!({
        "schema": 1,
        "name": name,
        "description": format!("Imported ComfyUI workflow ({} nodes).", graph.len()),
        "output": output,
        "tasks": tasks,
        "open_weights": true,
        "prompt": positive.iter().map(|(n, i)| bind(n, i)).collect::<Vec<_>>(),
        "negative_prompt": negative.iter().map(|(n, i)| bind(n, i)).collect::<Vec<_>>(),
        "inputs": inputs_def,
        "params": params,
        "outputs": outputs,
        "requires": { "models": models.iter().map(|(f, d)| json!({ "name": f, "directory": d })).collect::<Vec<_>>() },
        "tags": ["imported"],
    });
    if let Some((w, h)) = size_now.filter(|(w, h)| *w > 0.0 && *h > 0.0) {
        let mp = ((w * h) / 1_048_576.0 * 20.0).round() / 20.0;
        let mut size = json!({ "width": widths, "height": heights, "megapixels": mp.max(0.05), "multiple": if output == "video" { 32 } else { 16 } });
        if has("start_frame") {
            size["match_input"] = json!("start_frame");
        } else if has("reference") {
            size["match_input"] = json!("reference");
        }
        manifest["size"] = size;
    }
    if !batch.is_empty() && output == "image" {
        manifest["batch"] = json!(batch);
        manifest["max_outputs"] = json!(4);
    }
    Ok((manifest, notes))
}

/// Where imported templates go for this ComfyUI configuration.
pub fn import_dir(ctx: &Ctx) -> Option<PathBuf> {
    crate::comfyui::workflows_dir_for(ctx)
}

/// Lowercase id from a name: letters, digits and dashes.
pub fn slug(name: &str) -> String {
    let mut s: String = name.to_lowercase().chars().map(|c| if c.is_ascii_alphanumeric() { c } else { '-' }).collect();
    while s.contains("--") {
        s = s.replace("--", "-");
    }
    let s = s.trim_matches('-').chars().take(48).collect::<String>();
    if s.is_empty() {
        "workflow".into()
    } else {
        s
    }
}

/// Writes the workflow and its inferred manifest as a template, after checking that the
/// ComfyUI provider accepts it. Returns the template id and the notes.
pub fn import(dir: &Path, name: &str, workflow: &Value) -> Result<(String, Value, Vec<String>), String> {
    let graph = workflow.as_object().ok_or("The workflow must be a JSON object.")?;
    let (manifest, notes) = infer_manifest(name, graph)?;
    let mut id = slug(name);
    let mut n = 2;
    // A folder with a built-in's id replaces that built-in, which an import must never do.
    while dir.join(&id).exists() || crate::comfyui::is_builtin(&id) {
        id = format!("{}-{n}", slug(name));
        n += 1;
    }
    let manifest_text = serde_json::to_string_pretty(&manifest).map_err(|e| e.to_string())?;
    let workflow_text = serde_json::to_string_pretty(workflow).map_err(|e| e.to_string())?;
    crate::comfyui::check_template(&id, &manifest_text, &workflow_text)?;
    let target = dir.join(&id);
    std::fs::create_dir_all(&target).map_err(|e| format!("cannot create {}: {e}", target.display()))?;
    std::fs::write(target.join("workflow.json"), workflow_text).map_err(|e| e.to_string())?;
    std::fs::write(target.join("manifest.json"), manifest_text).map_err(|e| e.to_string())?;
    Ok((id, manifest, notes))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sdxl_like() -> Graph {
        serde_json::from_value(json!({
            "3": { "class_type": "KSampler", "inputs": { "seed": 7, "steps": 20, "cfg": 7.0, "sampler_name": "euler", "scheduler": "normal", "denoise": 1.0,
                   "model": ["4", 0], "positive": ["6", 0], "negative": ["7", 0], "latent_image": ["5", 0] } },
            "4": { "class_type": "CheckpointLoaderSimple", "inputs": { "ckpt_name": "sd_xl_base_1.0.safetensors" } },
            "5": { "class_type": "EmptyLatentImage", "inputs": { "width": 1344, "height": 768, "batch_size": 1 } },
            "6": { "class_type": "CLIPTextEncode", "inputs": { "text": "a cabin", "clip": ["4", 1] } },
            "7": { "class_type": "CLIPTextEncode", "inputs": { "text": "blurry", "clip": ["4", 1] } },
            "8": { "class_type": "VAEDecode", "inputs": { "samples": ["3", 0], "vae": ["4", 2] } },
            "9": { "class_type": "SaveImage", "inputs": { "filename_prefix": "x", "images": ["8", 0] } }
        }))
        .unwrap()
    }

    #[test]
    fn reads_a_text_to_image_graph() {
        let (m, _) = infer_manifest("SDXL", &sdxl_like()).unwrap();
        assert_eq!(m["output"], "image");
        assert_eq!(m["tasks"], json!(["text_to_image"]));
        assert_eq!(m["prompt"], json!([["6", "text"]]));
        assert_eq!(m["negative_prompt"], json!([["7", "text"]]));
        assert_eq!(m["outputs"], json!(["9"]));
        assert_eq!(m["size"]["width"], json!([["5", "width"]]));
        assert_eq!(m["batch"], json!([["5", "batch_size"]]));
        let keys: Vec<&str> = m["params"].as_array().unwrap().iter().map(|p| p["key"].as_str().unwrap()).collect();
        assert_eq!(keys, ["aspect_ratio", "seed", "steps", "guidance"]);
        assert_eq!(m["params"][0]["default"], "16:9");
        assert_eq!(m["requires"]["models"][0]["directory"], "checkpoints");
    }

    #[test]
    fn finds_start_frames_and_frame_counts_in_video_graphs() {
        let g: Graph = serde_json::from_value(json!({
            "1": { "class_type": "LoadImage", "inputs": { "image": "in.png" } },
            "2": { "class_type": "WanImageToVideo", "inputs": { "width": 832, "height": 480, "length": 81, "batch_size": 1, "start_image": ["1", 0], "positive": ["3", 0], "negative": ["4", 0] } },
            "3": { "class_type": "CLIPTextEncode", "inputs": { "text": "a boat" } },
            "4": { "class_type": "CLIPTextEncode", "inputs": { "text": "static" } },
            "5": { "class_type": "CreateVideo", "inputs": { "fps": 16, "images": ["2", 0] } },
            "6": { "class_type": "SaveVideo", "inputs": { "video": ["5", 0] } }
        }))
        .unwrap();
        let (m, _) = infer_manifest("Wan", &g).unwrap();
        assert_eq!(m["output"], "video");
        assert_eq!(m["tasks"], json!(["image_to_video"]));
        assert_eq!(m["inputs"][0]["role"], "start_frame");
        let dur = m["params"].as_array().unwrap().iter().find(|p| p["key"] == "duration").unwrap();
        assert_eq!(dur["default"], 5);
        assert_eq!(dur["bind"][0]["frames"]["step"], 8);
        assert_eq!(m["size"]["match_input"], "start_frame");
    }

    #[test]
    fn a_zeroed_negative_branch_stays_negative() {
        // SD3 style: the negative encoder reaches KSampler.negative through ZeroOut, a timestep range and a combine.
        let g: Graph = serde_json::from_value(json!({
            "1": { "class_type": "CheckpointLoaderSimple", "inputs": { "ckpt_name": "sd3.safetensors" } },
            "2": { "class_type": "CLIPTextEncode", "inputs": { "text": "a fox", "clip": ["1", 1] } },
            "3": { "class_type": "CLIPTextEncode", "inputs": { "text": "blurry", "clip": ["1", 1] } },
            "4": { "class_type": "ConditioningZeroOut", "inputs": { "conditioning": ["3", 0] } },
            "5": { "class_type": "ConditioningSetTimestepRange", "inputs": { "conditioning": ["3", 0], "start": 0.0, "end": 0.1 } },
            "6": { "class_type": "ConditioningCombine", "inputs": { "conditioning_1": ["4", 0], "conditioning_2": ["5", 0] } },
            "7": { "class_type": "EmptySD3LatentImage", "inputs": { "width": 1024, "height": 1024, "batch_size": 1 } },
            "8": { "class_type": "KSampler", "inputs": { "model": ["1", 0], "positive": ["2", 0], "negative": ["6", 0], "latent_image": ["7", 0], "seed": 1, "steps": 28, "cfg": 4.5 } },
            "9": { "class_type": "VAEDecode", "inputs": { "samples": ["8", 0], "vae": ["1", 2] } },
            "10": { "class_type": "SaveImage", "inputs": { "images": ["9", 0] } }
        }))
        .unwrap();
        let (m, _) = infer_manifest("SD3", &g).unwrap();
        assert_eq!(m["prompt"], json!([["2", "text"]]));
        assert_eq!(m["negative_prompt"], json!([["3", "text"]]));
    }

    #[test]
    fn extreme_numbers_do_not_overflow() {
        let g: Graph = serde_json::from_value(json!({
            "1": { "class_type": "EmptyLatentVideo", "inputs": { "width": 832, "height": 480, "length": i64::MIN } },
            "2": { "class_type": "KSampler", "inputs": { "latent_image": ["1", 0], "steps": i64::MAX } },
            "3": { "class_type": "CreateVideo", "inputs": { "fps": 0, "images": ["2", 0] } },
            "4": { "class_type": "SaveVideo", "inputs": { "video": ["3", 0] } }
        }))
        .unwrap();
        let (m, _) = infer_manifest("Odd", &g).unwrap();
        let param = |key: &str| m["params"].as_array().unwrap().iter().find(|p| p["key"] == key).unwrap().clone();
        assert_eq!(param("duration")["default"], 1, "one frame at the 24 fps fallback");
        assert_eq!(param("steps")["default"], 1000);
    }

    #[test]
    fn an_import_never_takes_a_built_in_id() {
        let dir = std::env::temp_dir().join(format!("pictesa-import-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let workflow = serde_json::to_value(sdxl_like()).unwrap();
        let (id, _, _) = import(&dir, "Wan22 i2v", &workflow).unwrap();
        assert_eq!(id, "wan22-i2v-2");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn rejects_the_editor_format() {
        let g: Graph = serde_json::from_value(json!({ "nodes": [], "links": [] })).unwrap();
        assert!(infer_manifest("x", &g).unwrap_err().contains("Export (API)"));
    }
}
