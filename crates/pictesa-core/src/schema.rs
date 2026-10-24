//! Turns a provider's JSON Schema for model inputs into Pictesa parameters and input slots.
//!
//! Gateways (fal, Replicate, Comfy Router) publish one input schema per model. Converting it
//! here lets Pictesa offer every model they host without hand-written catalog entries.

use serde_json::{json, Map, Value};

use crate::{keys, EnumOption, InputRole, InputSlot, MediaKind, ParamKind, ParamSpec};

/// Result of reading an input schema.
#[derive(Debug, Default, Clone)]
pub struct SchemaModel {
    pub params: Vec<ParamSpec>,
    pub inputs: Vec<InputSlot>,
    pub has_prompt: bool,
    pub has_negative_prompt: bool,
    /// Largest output count the schema allows, and the field that sets it.
    pub max_outputs: u32,
    /// Bindings stored in `ModelSpec::meta` so the provider can rebuild a request:
    /// `{"prompt": "prompt", "count": "num_images", "inputs": [{"role":"reference","field":"image_urls","array":true}]}`
    pub bindings: Value,
}

const COUNT_FIELDS: &[&str] = &["num_images", "num_outputs", "n", "number_of_images", "batch_size", "num_videos"];
const SKIP_FIELDS: &[&str] = &["sync_mode", "enable_safety_checker", "safety_tolerance", "webhook", "webhook_url", "callback_url", "disable_safety_checker", "safety_filter_level"];

/// Reads `schema` (an object schema with `properties`, optionally `required` and `x-*` hints).
/// `$ref`s are resolved against `defs` (the schema's components or definitions map).
pub fn read_input_schema(schema: &Value, defs: Option<&Value>, output: MediaKind) -> SchemaModel {
    let schema = resolve(schema, defs);
    let props = match schema.get("properties").and_then(Value::as_object) {
        Some(p) => p,
        None => return SchemaModel { max_outputs: 1, ..Default::default() },
    };
    let required: Vec<&str> = schema
        .get("required")
        .and_then(Value::as_array)
        .map(|a| a.iter().filter_map(Value::as_str).collect())
        .unwrap_or_default();
    let order = ordered_keys(&schema, props);

    let mut out = SchemaModel { max_outputs: 1, ..Default::default() };
    let mut bindings = Map::new();
    let mut input_bindings = Vec::new();

    for name in order {
        let Some(raw) = props.get(&name) else { continue };
        let prop = resolve(raw, defs);
        let lname = name.to_ascii_lowercase();
        let is_required = required.contains(&name.as_str());

        if lname == "prompt" || lname == "text" && !out.has_prompt {
            out.has_prompt = true;
            bindings.insert("prompt".into(), json!(name));
            continue;
        }
        if lname == "negative_prompt" {
            out.has_negative_prompt = true;
            bindings.insert("negative_prompt".into(), json!(name));
            continue;
        }
        if SKIP_FIELDS.contains(&lname.as_str()) {
            continue;
        }
        if COUNT_FIELDS.contains(&lname.as_str()) {
            let max = prop.get("maximum").and_then(Value::as_u64).unwrap_or(4).clamp(1, 16) as u32;
            out.max_outputs = max;
            bindings.insert("count".into(), json!(name));
            continue;
        }
        if let Some((role, kind, array, max)) = media_field(&lname, &prop, output) {
            let mut slot = InputSlot::new(role, kind, max);
            if is_required {
                slot.min = 1;
            }
            slot.label = Some(title(&name, &prop));
            out.inputs.push(slot);
            input_bindings.push(json!({"role": role, "field": name, "array": array}));
            continue;
        }
        if let Some(mut param) = param_for(&name, &prop) {
            if lname == "seed" {
                param.key = keys::SEED.into();
                bindings.insert("seed".into(), json!(name));
            }
            out.params.push(param);
        }
    }
    bindings.insert("inputs".into(), Value::Array(input_bindings));
    out.bindings = Value::Object(bindings);
    out
}

fn ordered_keys(schema: &Value, props: &Map<String, Value>) -> Vec<String> {
    // fal publishes `x-fal-order-properties`; otherwise keep the map order (serde_json preserves it with
    // the `preserve_order` feature, else alphabetical, which is still stable).
    if let Some(order) = schema.get("x-fal-order-properties").and_then(Value::as_array) {
        let mut keys: Vec<String> = order.iter().filter_map(Value::as_str).map(String::from).collect();
        for k in props.keys() {
            if !keys.contains(k) {
                keys.push(k.clone());
            }
        }
        return keys;
    }
    props.keys().cloned().collect()
}

fn resolve<'a>(v: &'a Value, defs: Option<&'a Value>) -> Value {
    let mut current = v.clone();
    for _ in 0..8 {
        let Some(r) = current.get("$ref").and_then(Value::as_str) else { break };
        let name = r.rsplit('/').next().unwrap_or(r);
        match defs.and_then(|d| d.get(name)) {
            Some(target) => current = target.clone(),
            None => break,
        }
    }
    // `anyOf: [{...}, {"type":"null"}]` is how Pydantic marks optional fields.
    if let Some(any) = current.get("anyOf").or_else(|| current.get("oneOf")).and_then(Value::as_array) {
        if let Some(first) = any.iter().find(|s| s.get("type").and_then(Value::as_str) != Some("null")) {
            let mut merged = resolve(first, defs);
            if let (Some(m), Some(orig)) = (merged.as_object_mut(), current.as_object()) {
                for (k, val) in orig {
                    if k != "anyOf" && k != "oneOf" {
                        m.entry(k.clone()).or_insert(val.clone());
                    }
                }
            }
            return merged;
        }
    }
    current
}

/// Detects file inputs: URL strings or arrays of them, named like images, videos, audio or masks.
fn media_field(lname: &str, prop: &Value, output: MediaKind) -> Option<(InputRole, MediaKind, bool, u32)> {
    let ty = prop.get("type").and_then(Value::as_str).unwrap_or("");
    let is_array = ty == "array";
    let item_format = prop.pointer("/items/format").and_then(Value::as_str);
    let format = prop.get("format").and_then(Value::as_str).or(item_format);
    let modality = prop.get("x-comfy-router-input-modalities").or_else(|| prop.get("x-modality"));
    let mentions = |words: &[&str]| words.iter().any(|w| lname.contains(w));

    let kind = if let Some(m) = modality.and_then(|m| m.as_array().and_then(|a| a.first()).or(Some(m))).and_then(Value::as_str) {
        match m {
            "image" => MediaKind::Image,
            "video" => MediaKind::Video,
            "audio" => MediaKind::Audio,
            _ => return None,
        }
    } else if mentions(&["video"]) {
        MediaKind::Video
    } else if mentions(&["audio", "voice", "speech"]) {
        MediaKind::Audio
    } else if mentions(&["image", "frame", "mask", "photo", "reference", "subject", "face", "init_"]) {
        MediaKind::Image
    } else {
        return None;
    };
    let urlish = format == Some("uri") || lname.ends_with("_url") || lname.ends_with("_urls") || lname.ends_with("image") || lname.ends_with("images") || modality.is_some() || lname.contains("frame");
    if !urlish || !(ty == "string" || is_array || ty.is_empty()) {
        return None;
    }
    let role = if mentions(&["mask"]) {
        InputRole::Mask
    } else if kind == MediaKind::Video {
        InputRole::Video
    } else if kind == MediaKind::Audio {
        InputRole::Audio
    } else if mentions(&["end_image", "last_frame", "end_frame", "tail_image", "image_tail", "last_image"]) {
        InputRole::EndFrame
    } else if output == MediaKind::Video && !is_array && mentions(&["start", "first_frame", "image_url", "input_image", "image"]) && !mentions(&["reference"]) {
        InputRole::StartFrame
    } else {
        InputRole::Reference
    };
    let max = if is_array { prop.get("maxItems").and_then(Value::as_u64).unwrap_or(4).clamp(1, 16) as u32 } else { 1 };
    Some((role, kind, is_array, max))
}

fn title(name: &str, prop: &Value) -> String {
    if let Some(t) = prop.get("title").and_then(Value::as_str) {
        return t.to_string();
    }
    let words: Vec<String> = name
        .split(['_', '-'])
        .filter(|w| !w.is_empty())
        .map(|w| w.to_string())
        .collect();
    let mut s = words.join(" ");
    if let Some(first) = s.get_mut(0..1) {
        first.make_ascii_uppercase();
    }
    s
}

fn param_for(name: &str, prop: &Value) -> Option<ParamSpec> {
    let label = title(name, prop);
    let ty = prop.get("type").and_then(Value::as_str).unwrap_or("");
    let default = prop.get("default").cloned().filter(|d| !d.is_null());
    let help = prop.get("description").and_then(Value::as_str).map(|d| crate::error::truncate(d, 220));
    let kind = if let Some(values) = prop.get("enum").and_then(Value::as_array) {
        let options = values
            .iter()
            .filter(|v| !v.is_null())
            .map(|v| EnumOption { value: v.clone(), label: v.as_str().map(String::from).unwrap_or_else(|| v.to_string()) })
            .collect::<Vec<_>>();
        if options.is_empty() {
            return None;
        }
        ParamKind::Enum { options }
    } else {
        match ty {
            "integer" if name.eq_ignore_ascii_case("seed") => ParamKind::Seed,
            "integer" => ParamKind::Int {
                min: prop.get("minimum").and_then(Value::as_i64).unwrap_or(0),
                max: prop.get("maximum").and_then(Value::as_i64).unwrap_or(i32::MAX as i64),
                step: 1,
            },
            "number" => ParamKind::Float {
                min: prop.get("minimum").and_then(Value::as_f64).unwrap_or(0.0),
                max: prop.get("maximum").and_then(Value::as_f64).unwrap_or(1_000_000.0),
                step: 0.01,
            },
            "boolean" => ParamKind::Bool,
            "string" => ParamKind::Text { multiline: false, max_len: prop.get("maxLength").and_then(Value::as_u64).map(|n| n as u32) },
            _ => return None,
        }
    };
    let advanced = !matches!(name, "aspect_ratio" | "resolution" | "duration" | "image_size" | "quality" | "size" | "generate_audio" | "style");
    Some(ParamSpec { key: name.to_string(), label, kind, default, advanced, help })
}

/// Rebuilds a provider request body from a Pictesa request using the stored bindings.
/// Media inputs are filled by the caller (they may need uploading); this handles text and params.
pub fn body_from_bindings(bindings: &Value, req: &crate::GenerationRequest) -> Map<String, Value> {
    let mut body = Map::new();
    // Plain params first; bound fields (prompt, count, seed, inputs) are written after and always win.
    let bound: Vec<&str> = ["prompt", "negative_prompt", "count"].iter().filter_map(|k| bindings.get(*k).and_then(Value::as_str)).collect();
    let input_fields: Vec<String> = input_bindings(bindings).into_iter().map(|(_, f, _)| f).collect();
    let seed_field = bindings.get("seed").and_then(Value::as_str);
    for (k, v) in &req.params {
        if k == keys::SEED || bound.contains(&k.as_str()) || input_fields.contains(k) {
            continue;
        }
        body.insert(k.clone(), v.clone());
    }
    if let Some(v) = req.params.get(keys::SEED) {
        body.insert(seed_field.unwrap_or("seed").into(), v.clone());
    }
    if let Some(field) = bindings.get("prompt").and_then(Value::as_str) {
        if !req.prompt.is_empty() {
            body.insert(field.into(), json!(req.prompt));
        }
    }
    if let (Some(field), Some(neg)) = (bindings.get("negative_prompt").and_then(Value::as_str), &req.negative_prompt) {
        if !neg.is_empty() {
            body.insert(field.into(), json!(neg));
        }
    }
    if let Some(field) = bindings.get("count").and_then(Value::as_str) {
        body.insert(field.into(), json!(req.num_outputs));
    }
    body
}

/// Input bindings stored by [`read_input_schema`]: `(role, field, is_array)`.
pub fn input_bindings(bindings: &Value) -> Vec<(InputRole, String, bool)> {
    bindings
        .get("inputs")
        .and_then(Value::as_array)
        .map(|a| {
            a.iter()
                .filter_map(|b| {
                    let role: InputRole = serde_json::from_value(b.get("role")?.clone()).ok()?;
                    Some((role, b.get("field")?.as_str()?.to_string(), b.get("array").and_then(Value::as_bool).unwrap_or(false)))
                })
                .collect()
        })
        .unwrap_or_default()
}
