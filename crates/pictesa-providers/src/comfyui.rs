//! ComfyUI: any ComfyUI server (local, LAN, behind a reverse proxy, or Comfy Cloud).
//!
//! Every workflow template becomes a model. A template is a folder holding `workflow.json`
//! (ComfyUI API format) and `manifest.json` (what Pictesa shows and where each value goes);
//! docs/WORKFLOWS.md documents the format. Built-in templates live in `workflows/comfy/` and are
//! compiled into the binary; users add or override templates in their workflows folder.
//!
//! - Discovery: `GET /api/object_info` tags templates whose node classes (`needs-nodes`) or model
//!   files (`needs-models`) the server lacks. A model file kept in a subfolder is found and used.
//! - Runs: inputs go through `POST /api/upload/image` (images and videos), the patched graph through
//!   `POST /api/prompt` with this process's `client_id`.
//! - Status: `GET /api/jobs/{id}` (ComfyUI 0.4+ and Comfy Cloud), else `/api/history/{id}` plus
//!   `/api/queue`. Step progress comes from the `/ws` event stream when it is reachable.
//! - Outputs: `GET /api/view?filename=&subfolder=&type=`; Comfy Cloud answers with a signed redirect.

use std::{
    collections::{BTreeMap, HashMap},
    path::{Path, PathBuf},
    sync::OnceLock,
};

use async_trait::async_trait;
use pictesa_core::{
    json_response, keys, CredentialField, Ctx, GenerationRequest, InputRole, InputSlot, MediaInput,
    MediaKind, ModelSpec, Output, ParamSpec, PollStatus, Pricing, Provider, ProviderError,
    ProviderInfo, ProviderKind, ProviderResult, RemoteHandle, Submission, Task,
};
use serde::Deserialize;
use serde_json::{json, Map, Value};
use sha2::{Digest, Sha256};

use crate::util::{failure, net_error, API_TIMEOUT};

pub struct ComfyUi;

const ID: &str = "comfyui";
const DEFAULT_URL: &str = "http://127.0.0.1:8188";
/// Uploads can be large videos.
const UPLOAD_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(600);
/// Pixels in one "megapixel" as ComfyUI's ResolutionSelector counts it (1.0 = 1024 x 1024).
const MEGAPIXEL: f64 = 1024.0 * 1024.0;

/// Built-in templates, compiled in from `workflows/comfy/<id>/`. Add new ones here.
macro_rules! builtin {
    ($($id:literal),* $(,)?) => {
        &[$((
            $id,
            include_str!(concat!("../../../workflows/comfy/", $id, "/manifest.json")),
            include_str!(concat!("../../../workflows/comfy/", $id, "/workflow.json")),
        )),*]
    };
}

const BUILTIN: &[(&str, &str, &str)] = builtin![
    "flux3-klein-t2i",
    "flux3-klein-edit",
    "qwen-image-21",
    "qwen-image-21-turbo",
    "qwen-image-2512",
    "ltx25-t2v",
    "ltx25-i2v",
    "wan22-i2v",
    "wan22-5b",
    "esrgan-upscale",
    "esrgan-video-upscale",
];

// ---------------------------------------------------------------------------------------------
// Template format

type Graph = Map<String, Value>;

/// `manifest.json`. Unknown fields are ignored so newer templates still load.
#[derive(Debug, Clone, Deserialize)]
pub(crate) struct Manifest {
    #[serde(default = "one")]
    pub schema: u32,
    pub name: String,
    #[serde(default)]
    pub description: Option<String>,
    #[serde(default)]
    pub vendor: Option<String>,
    pub output: MediaKind,
    pub tasks: Vec<Task>,
    #[serde(default)]
    pub open_weights: bool,
    #[serde(default)]
    pub license: Option<String>,
    /// Where the prompt text goes. Empty means the template takes no prompt.
    #[serde(default)]
    pub prompt: Vec<Bind>,
    /// Where a negative prompt goes. The template's own text stays when the user gives none.
    #[serde(default)]
    pub negative_prompt: Vec<Bind>,
    #[serde(default)]
    pub inputs: Vec<InputDef>,
    #[serde(default)]
    pub params: Vec<ParamDef>,
    #[serde(default)]
    pub size: Option<SizeDef>,
    /// Batch-size inputs; when set, one run can return up to `max_outputs` results.
    #[serde(default)]
    pub batch: Vec<Bind>,
    #[serde(default = "one")]
    pub max_outputs: u32,
    /// Saving nodes whose files are the results. Empty means every saved file of the right kind.
    #[serde(default)]
    pub outputs: Vec<String>,
    #[serde(default)]
    pub requires: Requires,
    #[serde(default)]
    pub sources: Vec<String>,
    #[serde(default)]
    pub tags: Vec<String>,
}

fn one() -> u32 {
    1
}

/// A graph location: `["node_id", "input_name"]`, or an object with an optional value transform.
#[derive(Debug, Clone, Deserialize)]
#[serde(untagged)]
pub(crate) enum Bind {
    At(String, String),
    With {
        node: String,
        input: String,
        /// Seconds to frame count, for nodes that take `length` in frames.
        #[serde(default)]
        frames: Option<Frames>,
        /// Translates a parameter value (as text) to the value the node needs.
        #[serde(default)]
        map: Option<Map<String, Value>>,
    },
}

impl Bind {
    fn node(&self) -> &str {
        match self {
            Self::At(n, _) | Self::With { node: n, .. } => n,
        }
    }
    fn input(&self) -> &str {
        match self {
            Self::At(_, i) | Self::With { input: i, .. } => i,
        }
    }
}

#[derive(Debug, Clone, Deserialize)]
pub(crate) struct Frames {
    /// Frames per second when no `fps_param` value is given.
    pub fps: f64,
    /// Parameter key holding the frame rate (usually `fps`).
    #[serde(default)]
    pub fps_param: Option<String>,
    /// The frame count is a multiple of `step` plus `offset` (Wan and LTX use 4n+1 or 8n+1).
    #[serde(default = "one")]
    pub step: u32,
    #[serde(default)]
    pub offset: u32,
}

#[derive(Debug, Clone, Deserialize)]
pub(crate) struct InputDef {
    pub role: InputRole,
    pub kind: MediaKind,
    /// One loader node per accepted file, in order. Loaders without a file are removed from the graph.
    pub nodes: Vec<String>,
    /// The loader input that takes the uploaded file name (`image` for LoadImage, `file` for LoadVideo).
    #[serde(default)]
    pub input: Option<String>,
    #[serde(default)]
    pub min: u32,
    #[serde(default)]
    pub label: Option<String>,
}

impl InputDef {
    fn input_name(&self) -> &str {
        self.input.as_deref().unwrap_or(match self.kind {
            MediaKind::Image => "image",
            MediaKind::Video | MediaKind::Lora => "file",
            MediaKind::Audio => "audio",
        })
    }
}

/// A Pictesa parameter (same fields as [`ParamSpec`]) plus the graph inputs it sets.
#[derive(Debug, Clone, Deserialize)]
pub(crate) struct ParamDef {
    #[serde(flatten)]
    pub spec: ParamSpec,
    #[serde(default)]
    pub bind: Vec<Bind>,
}

/// Output size, computed from the shared `aspect_ratio`, `resolution`, `width` and `height` params.
#[derive(Debug, Clone, Deserialize)]
pub(crate) struct SizeDef {
    pub width: Vec<Bind>,
    pub height: Vec<Bind>,
    /// Output area when no `resolution` preset applies (1.0 = 1024 x 1024 pixels).
    #[serde(default = "default_megapixels")]
    pub megapixels: f64,
    /// `resolution` values to the short side in pixels, e.g. `{"720p": 720}`.
    #[serde(default)]
    pub resolutions: BTreeMap<String, f64>,
    #[serde(default = "sixteen")]
    pub multiple: u32,
    /// Take the aspect ratio from this input when `aspect_ratio` is `auto` or missing.
    #[serde(default)]
    pub match_input: Option<InputRole>,
}

fn default_megapixels() -> f64 {
    1.0
}
fn sixteen() -> u32 {
    16
}

#[derive(Debug, Clone, Default, Deserialize)]
pub(crate) struct Requires {
    #[serde(default)]
    pub nodes: Vec<String>,
    #[serde(default)]
    pub models: Vec<ModelFile>,
}

#[derive(Debug, Clone, Deserialize)]
pub(crate) struct ModelFile {
    pub name: String,
    #[serde(default)]
    pub directory: Option<String>,
    #[serde(default)]
    pub url: Option<String>,
}

#[derive(Debug, Clone)]
pub(crate) struct Template {
    pub id: String,
    pub manifest: Manifest,
    pub workflow: Graph,
    /// The folder of a user template; `None` for built-ins.
    pub dir: Option<PathBuf>,
}

/// Parses and checks one template: the graph must be API format and every bound node must exist.
pub(crate) fn parse_template(
    id: &str,
    manifest: &str,
    workflow: &str,
    dir: Option<PathBuf>,
) -> Result<Template, String> {
    if id.is_empty()
        || !id
            .chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || matches!(c, '-' | '_' | '.'))
    {
        return Err(format!(
            "template id {id:?} must use lowercase letters, digits, '-', '_' or '.'"
        ));
    }
    let manifest: Manifest =
        serde_json::from_str(manifest).map_err(|e| format!("manifest.json: {e}"))?;
    if manifest.schema != 1 {
        return Err(format!(
            "manifest schema {} is not supported (expected 1)",
            manifest.schema
        ));
    }
    let workflow: Value =
        serde_json::from_str(workflow).map_err(|e| format!("workflow.json: {e}"))?;
    if workflow.get("nodes").is_some() && workflow.get("links").is_some() {
        return Err(
            "workflow.json is in UI format; export it with Workflow > Export (API) in ComfyUI"
                .into(),
        );
    }
    let Value::Object(workflow) = workflow else {
        return Err("workflow.json must be a JSON object".into());
    };
    if let Some((k, _)) = workflow
        .iter()
        .find(|(_, n)| n.get("class_type").and_then(Value::as_str).is_none())
    {
        return Err(format!("workflow.json node {k} has no class_type"));
    }
    let mut missing = Vec::new();
    let mut check = |node: &str, what: &str| {
        if !workflow.contains_key(node) {
            missing.push(format!("{what} -> node {node}"));
        }
    };
    for b in manifest
        .prompt
        .iter()
        .chain(&manifest.negative_prompt)
        .chain(&manifest.batch)
    {
        check(b.node(), b.input());
    }
    for d in &manifest.inputs {
        d.nodes.iter().for_each(|n| check(n, "input"));
    }
    for p in &manifest.params {
        p.bind.iter().for_each(|b| check(b.node(), &p.spec.key));
    }
    if let Some(s) = &manifest.size {
        s.width
            .iter()
            .chain(&s.height)
            .for_each(|b| check(b.node(), "size"));
    }
    manifest.outputs.iter().for_each(|n| check(n, "output"));
    if !missing.is_empty() {
        return Err(format!(
            "manifest binds nodes the workflow does not have: {}",
            missing.join(", ")
        ));
    }
    Ok(Template {
        id: id.to_string(),
        manifest,
        workflow,
        dir,
    })
}

/// Whether an id belongs to a template shipped with Pictesa.
pub(crate) fn is_builtin(id: &str) -> bool {
    BUILTIN.iter().any(|(builtin, _, _)| *builtin == id)
}

fn builtins() -> &'static [Template] {
    static CELL: OnceLock<Vec<Template>> = OnceLock::new();
    CELL.get_or_init(|| {
        BUILTIN
            .iter()
            .filter_map(|(id, m, w)| match parse_template(id, m, w, None) {
                Ok(t) => Some(t),
                Err(e) => {
                    tracing::error!("built-in ComfyUI template {id} is invalid: {e}");
                    None
                }
            })
            .collect()
    })
}

/// Where this configuration reads user templates from (and where imports are written).
pub fn workflows_dir_for(ctx: &Ctx) -> Option<PathBuf> {
    user_dir(ctx)
}

/// Checks a manifest and workflow the same way loading them would.
pub(crate) fn check_template(id: &str, manifest: &str, workflow: &str) -> Result<(), String> {
    parse_template(id, manifest, workflow, None).map(|_| ())
}

/// The user's workflows folder: the `workflows_dir` field (env `PICTESA_WORKFLOWS_DIR`), else
/// `<Pictesa data dir>/workflows/comfy`, resolved the same way the engine finds its data dir.
fn user_dir(ctx: &Ctx) -> Option<PathBuf> {
    if let Some(d) = ctx.get("workflows_dir") {
        return Some(PathBuf::from(d));
    }
    let data = std::env::var_os("PICTESA_DATA_DIR")
        .map(PathBuf::from)
        .or_else(|| {
            let home = std::env::var_os("HOME")
                .or_else(|| std::env::var_os("USERPROFILE"))
                .map(PathBuf::from);
            if cfg!(windows) {
                std::env::var_os("APPDATA").map(PathBuf::from)
            } else if cfg!(target_os = "macos") {
                home.map(|h| h.join("Library/Application Support"))
            } else {
                std::env::var_os("XDG_DATA_HOME")
                    .map(PathBuf::from)
                    .or_else(|| home.map(|h| h.join(".local/share")))
            }
            .map(|d| d.join("pictesa"))
        })?;
    Some(data.join("workflows").join("comfy"))
}

fn load_dir(dir: &Path) -> Option<Template> {
    let id = dir.file_name()?.to_string_lossy().to_ascii_lowercase();
    let manifest = std::fs::read_to_string(dir.join("manifest.json")).ok()?;
    let workflow = match std::fs::read_to_string(dir.join("workflow.json")) {
        Ok(w) => w,
        Err(e) => {
            tracing::warn!(
                "ComfyUI template {}: cannot read workflow.json: {e}",
                dir.display()
            );
            return None;
        }
    };
    match parse_template(&id, &manifest, &workflow, Some(dir.to_path_buf())) {
        Ok(t) => Some(t),
        Err(e) => {
            tracing::warn!("ComfyUI template {} skipped: {e}", dir.display());
            None
        }
    }
}

fn user_templates(ctx: &Ctx) -> Vec<Template> {
    let Some(dir) = user_dir(ctx) else {
        return Vec::new();
    };
    let Ok(entries) = std::fs::read_dir(&dir) else {
        return Vec::new();
    };
    let mut dirs: Vec<PathBuf> = entries
        .filter_map(|e| e.ok())
        .map(|e| e.path())
        .filter(|p| p.is_dir())
        .collect();
    dirs.sort();
    dirs.iter().filter_map(|d| load_dir(d)).collect()
}

/// Built-ins plus user templates; a user template with a built-in's id replaces it.
fn all_templates(ctx: &Ctx) -> Vec<Template> {
    let user = user_templates(ctx);
    let mut out: Vec<Template> = builtins()
        .iter()
        .filter(|b| !user.iter().any(|u| u.id == b.id))
        .cloned()
        .collect();
    out.extend(user);
    out
}

fn find_template(ctx: &Ctx, model: &ModelSpec) -> ProviderResult<Template> {
    let id = model.remote_id();
    if let Some(dir) = model.meta.get("dir").and_then(Value::as_str) {
        if let Some(t) = load_dir(Path::new(dir)) {
            return Ok(t);
        }
    }
    all_templates(ctx)
        .into_iter()
        .find(|t| t.id == id)
        .ok_or_else(|| {
            ProviderError::NotSupported(format!(
                "ComfyUI template {id} was not found; refresh the model list"
            ))
        })
}

// ---------------------------------------------------------------------------------------------
// Model specs and server checks

fn spec_for(t: &Template) -> ModelSpec {
    let m = &t.manifest;
    let mut s = ModelSpec::new(ID, &t.id, &m.name, m.output, &m.tasks);
    s.vendor = m.vendor.clone();
    s.description = describe(t, None);
    s.open_weights = m.open_weights;
    s.pricing = Some(Pricing::Local);
    s.prompt = !m.prompt.is_empty();
    s.negative_prompt = !m.negative_prompt.is_empty();
    s.inputs = m
        .inputs
        .iter()
        .map(|d| InputSlot {
            role: d.role,
            kind: d.kind,
            min: d.min,
            max: d.nodes.len() as u32,
            label: d.label.clone(),
        })
        .collect();
    s.params = m.params.iter().map(|p| p.spec.clone()).collect();
    s.max_outputs = if m.batch.is_empty() {
        1
    } else {
        m.max_outputs.max(1)
    };
    s.tags = m.tags.clone();
    s.tags.push(
        if t.dir.is_some() {
            "user-workflow"
        } else {
            "workflow"
        }
        .into(),
    );
    let mut meta = json!({ "template": t.id, "license": m.license, "sources": m.sources });
    if let Some(d) = &t.dir {
        meta["dir"] = json!(d.to_string_lossy());
    }
    s.meta = meta;
    s
}

/// The template description plus the model files it needs and where they go, naming the
/// missing ones and where to download them when a server check found some.
fn describe(t: &Template, missing: Option<&[String]>) -> Option<String> {
    let m = &t.manifest;
    let place = |name: &str| match m
        .requires
        .models
        .iter()
        .find(|f| f.name == name)
        .and_then(|f| f.directory.as_deref())
    {
        Some(dir) => format!("models/{dir}/{name}"),
        None => name.to_string(),
    };
    let note = match missing {
        Some(list) if !list.is_empty() => {
            let files: Vec<String> = list.iter().map(|n| place(n)).collect();
            let mut sources: Vec<String> = m
                .requires
                .models
                .iter()
                .filter(|f| list.contains(&f.name))
                .filter_map(|f| f.url.as_deref().map(download_source))
                .collect();
            sources.dedup();
            let from = if sources.is_empty() {
                String::new()
            } else {
                format!(" Download from {}.", sources.join(", "))
            };
            Some(format!(
                "Missing on the server: {}.{from}",
                files.join(", ")
            ))
        }
        _ if !m.requires.models.is_empty() => {
            let files: Vec<String> = m.requires.models.iter().map(|f| place(&f.name)).collect();
            Some(format!("Needs {}.", files.join(", ")))
        }
        _ => None,
    };
    match (m.description.as_deref(), note) {
        (Some(d), Some(n)) => Some(format!("{d} {n}")),
        (Some(d), None) => Some(d.to_string()),
        (None, n) => n,
    }
}

/// `huggingface.co/<org>/<repo>` for Hugging Face file links, else the host.
fn download_source(url: &str) -> String {
    let Ok(u) = url::Url::parse(url) else {
        return url.to_string();
    };
    let host = u.host_str().unwrap_or_default();
    let segs: Vec<&str> = u.path_segments().map(|s| s.collect()).unwrap_or_default();
    if host == "huggingface.co" && segs.len() >= 2 {
        format!("{host}/{}/{}", segs[0], segs[1])
    } else {
        host.to_string()
    }
}

const MODEL_EXTS: &[&str] = &[
    ".safetensors",
    ".sft",
    ".ckpt",
    ".pt",
    ".pth",
    ".bin",
    ".gguf",
    ".onnx",
];

/// The choices of a COMBO input in `/object_info`, in either the V1 (`[[..], {}]`) or V3
/// (`["COMBO", {"options": [..]}]`) shape.
fn combo_options(spec: &Value) -> Option<Vec<&str>> {
    let first = spec.get(0)?;
    let list = match first {
        Value::Array(a) => a,
        Value::String(s) if s == "COMBO" => spec.get(1)?.get("options")?.as_array()?,
        _ => return None,
    };
    Some(list.iter().filter_map(Value::as_str).collect())
}

fn input_spec<'a>(info: &'a Value, class: &str, input: &str) -> Option<&'a Value> {
    let inputs = info.get(class)?.get("input")?;
    inputs
        .get("required")
        .and_then(|r| r.get(input))
        .or_else(|| inputs.get("optional").and_then(|o| o.get(input)))
}

/// What one server lacks for a template, and model files it keeps under another folder.
#[derive(Debug, Default, PartialEq)]
pub(crate) struct Availability {
    pub missing_nodes: Vec<String>,
    pub missing_models: Vec<String>,
    /// Template file name to the server's path for it (a model kept in a subfolder).
    pub rename: BTreeMap<String, String>,
}

pub(crate) fn availability(t: &Template, info: &Value) -> Availability {
    let mut a = Availability::default();
    let mut classes: Vec<&str> = t
        .workflow
        .values()
        .filter_map(|n| n.get("class_type").and_then(Value::as_str))
        .collect();
    classes.extend(t.manifest.requires.nodes.iter().map(String::as_str));
    classes.sort_unstable();
    classes.dedup();
    a.missing_nodes = classes
        .into_iter()
        .filter(|c| info.get(*c).is_none())
        .map(String::from)
        .collect();
    let input_nodes: Vec<&str> = t
        .manifest
        .inputs
        .iter()
        .flat_map(|d| d.nodes.iter().map(String::as_str))
        .collect();
    for (id, node) in &t.workflow {
        if input_nodes.contains(&id.as_str()) {
            continue;
        }
        let Some(class) = node.get("class_type").and_then(Value::as_str) else {
            continue;
        };
        let Some(inputs) = node.get("inputs").and_then(Value::as_object) else {
            continue;
        };
        for (name, value) in inputs {
            let Some(file) = value.as_str() else { continue };
            let lower = file.to_ascii_lowercase();
            if !MODEL_EXTS.iter().any(|e| lower.ends_with(e)) {
                continue;
            }
            let Some(options) = input_spec(info, class, name).and_then(combo_options) else {
                continue;
            };
            if options.contains(&file) {
                continue;
            }
            let nested = options
                .iter()
                .find(|o| o.ends_with(&format!("/{file}")) || o.ends_with(&format!("\\{file}")));
            match nested {
                Some(path) => {
                    a.rename.insert(file.to_string(), path.to_string());
                }
                None if !a.missing_models.iter().any(|m| m == file) => {
                    a.missing_models.push(file.to_string())
                }
                None => {}
            }
        }
    }
    a
}

// ---------------------------------------------------------------------------------------------
// Building the prompt graph

fn set_input(g: &mut Graph, node: &str, input: &str, v: Value) -> ProviderResult<()> {
    let inputs = g
        .get_mut(node)
        .and_then(|n| n.get_mut("inputs"))
        .and_then(Value::as_object_mut)
        .ok_or_else(|| ProviderError::Failed(format!("workflow has no node {node}")))?;
    inputs.insert(input.to_string(), v);
    Ok(())
}

/// Deletes a node and every link that pointed at it.
fn remove_node(g: &mut Graph, node: &str) {
    g.remove(node);
    for n in g.values_mut() {
        if let Some(inputs) = n.get_mut("inputs").and_then(Value::as_object_mut) {
            inputs.retain(|_, v| {
                !(v.as_array().is_some_and(|a| {
                    a.len() == 2 && a[0].as_str() == Some(node) && a[1].is_number()
                }))
            });
        }
    }
}

fn value_key(v: &Value) -> String {
    match v {
        Value::String(s) => s.clone(),
        other => other.to_string(),
    }
}

fn transform(bind: &Bind, value: &Value, req: &GenerationRequest) -> Value {
    match bind {
        Bind::At(..) => value.clone(),
        Bind::With {
            frames: Some(f), ..
        } => {
            let seconds = value
                .as_f64()
                .or_else(|| {
                    value
                        .as_str()
                        .and_then(|s| s.trim_end_matches('s').parse().ok())
                })
                .unwrap_or(0.0);
            let fps = f
                .fps_param
                .as_deref()
                .and_then(|k| req.f64(k).or_else(|| req.i64(k).map(|n| n as f64)))
                .unwrap_or(f.fps);
            let step = f.step.max(1) as f64;
            let frames = ((seconds * fps) / step).floor() * step + f.offset as f64;
            json!(frames.max(1.0) as u64)
        }
        Bind::With { map: Some(m), .. } => m
            .get(&value_key(value))
            .cloned()
            .unwrap_or_else(|| value.clone()),
        Bind::With { .. } => value.clone(),
    }
}

fn parse_ratio(s: &str) -> Option<f64> {
    let (w, h) = s.split_once(':')?;
    let (w, h): (f64, f64) = (w.trim().parse().ok()?, h.trim().parse().ok()?);
    (w > 0.0 && h > 0.0).then(|| w / h)
}

fn round_to(x: f64, multiple: u32) -> u32 {
    let m = multiple.max(1) as f64;
    ((x / m).round().max(1.0) * m) as u32
}

/// Output width and height for a request.
pub(crate) fn compute_size(
    size: &SizeDef,
    output: MediaKind,
    req: &GenerationRequest,
    dims: &HashMap<InputRole, (u32, u32)>,
) -> (u32, u32) {
    if let (Some(w), Some(h)) = (req.i64(keys::WIDTH), req.i64(keys::HEIGHT)) {
        return (
            round_to(w as f64, size.multiple),
            round_to(h as f64, size.multiple),
        );
    }
    let from_input = size
        .match_input
        .and_then(|r| dims.get(&r))
        .filter(|(w, h)| *w > 0 && *h > 0)
        .map(|(w, h)| *w as f64 / *h as f64);
    let ratio = match req.str(keys::ASPECT_RATIO) {
        Some(r) if r != "auto" => parse_ratio(r).or(from_input),
        _ => from_input,
    }
    .unwrap_or(if output == MediaKind::Video {
        16.0 / 9.0
    } else {
        1.0
    });
    let (w, h) = match req
        .str(keys::RESOLUTION)
        .and_then(|r| size.resolutions.get(r))
        .copied()
    {
        Some(short) if ratio >= 1.0 => (short * ratio, short),
        Some(short) => (short, short / ratio),
        None => {
            let area = size.megapixels * MEGAPIXEL;
            ((area * ratio).sqrt(), (area / ratio).sqrt())
        }
    };
    (round_to(w, size.multiple), round_to(h, size.multiple))
}

fn random_seed() -> u64 {
    (ulid::Ulid::new().0 as u64) & 0xFFFF_FFFF
}

/// Applies a request to a template graph. `uploads` holds the server-side names of the uploaded
/// inputs per role; `rename` maps template model files to the server's paths.
pub(crate) fn build_graph(
    t: &Template,
    req: &GenerationRequest,
    uploads: &HashMap<InputRole, Vec<String>>,
    dims: &HashMap<InputRole, (u32, u32)>,
    rename: &Map<String, Value>,
) -> ProviderResult<Graph> {
    let m = &t.manifest;
    let mut g = t.workflow.clone();
    if !rename.is_empty() {
        for node in g.values_mut() {
            if let Some(inputs) = node.get_mut("inputs").and_then(Value::as_object_mut) {
                for v in inputs.values_mut() {
                    if let Some(new) = v
                        .as_str()
                        .and_then(|s| rename.get(s))
                        .filter(|n| n.is_string())
                    {
                        *v = new.clone();
                    }
                }
            }
        }
    }
    for d in &m.inputs {
        let names = uploads.get(&d.role).map(Vec::as_slice).unwrap_or_default();
        if (names.len() as u32) < d.min {
            let what = d
                .label
                .clone()
                .unwrap_or_else(|| format!("{:?}", d.role).to_lowercase());
            return Err(ProviderError::InvalidInput(format!(
                "{} needs at least {} file(s)",
                what, d.min
            )));
        }
        for (i, node) in d.nodes.iter().enumerate() {
            match names.get(i) {
                Some(name) => set_input(&mut g, node, d.input_name(), json!(name))?,
                None => remove_node(&mut g, node),
            }
        }
    }
    for b in &m.prompt {
        set_input(&mut g, b.node(), b.input(), json!(req.prompt))?;
    }
    if let Some(neg) = req
        .negative_prompt
        .as_deref()
        .filter(|n| !n.trim().is_empty())
    {
        for b in &m.negative_prompt {
            set_input(&mut g, b.node(), b.input(), json!(neg))?;
        }
    }
    for p in &m.params {
        let value = match req.params.get(&p.spec.key).or(p.spec.default.as_ref()) {
            Some(v) => v.clone(),
            None if p.spec.key == keys::SEED => json!(random_seed()),
            None => continue,
        };
        for b in &p.bind {
            set_input(&mut g, b.node(), b.input(), transform(b, &value, req))?;
        }
    }
    if let Some(size) = &m.size {
        let (w, h) = compute_size(size, m.output, req, dims);
        for b in &size.width {
            set_input(&mut g, b.node(), b.input(), json!(w))?;
        }
        for b in &size.height {
            set_input(&mut g, b.node(), b.input(), json!(h))?;
        }
    }
    for b in &m.batch {
        set_input(
            &mut g,
            b.node(),
            b.input(),
            json!(req.num_outputs.clamp(1, m.max_outputs.max(1))),
        )?;
    }
    Ok(g)
}

/// Reads width and height from a PNG, JPEG, GIF or WebP header, for inputs the engine did not measure.
fn image_dims(path: &Path) -> Option<(u32, u32)> {
    use std::io::Read;
    let mut buf = Vec::with_capacity(65536);
    std::fs::File::open(path)
        .ok()?
        .take(262_144)
        .read_to_end(&mut buf)
        .ok()?;
    let b = buf.as_slice();
    let be16 = |i: usize| Some(u16::from_be_bytes([*b.get(i)?, *b.get(i + 1)?]) as u32);
    let le16 = |i: usize| Some(u16::from_le_bytes([*b.get(i)?, *b.get(i + 1)?]) as u32);
    let be32 = |i: usize| {
        Some(u32::from_be_bytes([
            *b.get(i)?,
            *b.get(i + 1)?,
            *b.get(i + 2)?,
            *b.get(i + 3)?,
        ]))
    };
    if b.starts_with(b"\x89PNG\r\n\x1a\n") {
        return Some((be32(16)?, be32(20)?));
    }
    if b.starts_with(b"GIF8") {
        return Some((le16(6)?, le16(8)?));
    }
    if b.len() > 30 && &b[0..4] == b"RIFF" && &b[8..12] == b"WEBP" {
        return match &b[12..16] {
            b"VP8 " => Some((le16(26)? & 0x3fff, le16(28)? & 0x3fff)),
            b"VP8L" => {
                let v = u32::from_le_bytes([b[21], b[22], b[23], b[24]]);
                Some(((v & 0x3fff) + 1, ((v >> 14) & 0x3fff) + 1))
            }
            b"VP8X" => {
                let w = u32::from_le_bytes([b[24], b[25], b[26], 0]) + 1;
                let h = u32::from_le_bytes([b[27], b[28], b[29], 0]) + 1;
                Some((w, h))
            }
            _ => None,
        };
    }
    if b.starts_with(&[0xFF, 0xD8]) {
        let mut i = 2;
        while i + 9 < b.len() {
            if b[i] != 0xFF {
                i += 1;
                continue;
            }
            let marker = b[i + 1];
            if (0xC0..=0xCF).contains(&marker) && !matches!(marker, 0xC4 | 0xC8 | 0xCC) {
                return Some((be16(i + 7)?, be16(i + 5)?));
            }
            i += 2 + be16(i + 2)? as usize;
        }
    }
    None
}

fn input_dims(input: &MediaInput) -> Option<(u32, u32)> {
    match (input.width, input.height) {
        (Some(w), Some(h)) if w > 0 && h > 0 => Some((w, h)),
        _ if input.kind == MediaKind::Image => image_dims(&input.path),
        _ => None,
    }
}

// ---------------------------------------------------------------------------------------------
// HTTP

fn base(ctx: &Ctx, handle: Option<&RemoteHandle>) -> String {
    handle
        .and_then(|h| h.data_str("base_url"))
        .map(|s| s.trim_end_matches('/').to_string())
        .unwrap_or_else(|| ctx.base_url(DEFAULT_URL))
}

/// `comfyui-` keys (Comfy Cloud, Comfy platform) go in `X-API-Key`; anything else is a bearer token.
fn auth_headers(ctx: &Ctx) -> Vec<(String, String)> {
    match ctx.get("api_key") {
        Some(k) if k.starts_with("comfyui-") => vec![("X-API-Key".into(), k.to_string())],
        Some(k) => vec![("Authorization".into(), format!("Bearer {k}"))],
        None => Vec::new(),
    }
}

fn authed(ctx: &Ctx, mut rb: reqwest::RequestBuilder) -> reqwest::RequestBuilder {
    for (k, v) in auth_headers(ctx) {
        rb = rb.header(k, v);
    }
    rb.timeout(API_TIMEOUT)
}

async fn get(ctx: &Ctx, url: &str) -> ProviderResult<Value> {
    let resp = authed(ctx, ctx.http.get(url))
        .send()
        .await
        .map_err(net_error)?;
    json_response(resp).await
}

/// A stable id for this process, so the event stream and the prompts it sends match.
fn client_id() -> &'static str {
    static ID: OnceLock<String> = OnceLock::new();
    ID.get_or_init(|| {
        format!(
            "pictesa-{}",
            ulid::Ulid::new().to_string().to_ascii_lowercase()
        )
    })
}

fn extension(input: &MediaInput) -> String {
    let from_name = input
        .path
        .extension()
        .map(|e| e.to_string_lossy().to_ascii_lowercase())
        .filter(|e| !e.is_empty() && e.len() <= 5);
    from_name.unwrap_or_else(|| {
        match input.mime.as_str() {
            "image/jpeg" => "jpg",
            "image/webp" => "webp",
            "image/gif" => "gif",
            "video/mp4" => "mp4",
            "video/webm" => "webm",
            "video/quicktime" => "mov",
            "audio/mpeg" => "mp3",
            "audio/wav" | "audio/x-wav" => "wav",
            "audio/flac" => "flac",
            m if m.starts_with("video/") => "mp4",
            m if m.starts_with("audio/") => "wav",
            _ => "png",
        }
        .to_string()
    })
}

/// Uploads one input to the server's `input/pictesa/` folder under a content-hash name, so the same
/// asset is stored once. Returns the value a loader node expects (`pictesa/<name>`).
async fn upload(ctx: &Ctx, base: &str, input: &MediaInput) -> ProviderResult<String> {
    let bytes = input.bytes().await?;
    let digest = Sha256::digest(&bytes);
    let hash: String = digest.iter().take(10).map(|b| format!("{b:02x}")).collect();
    let name = format!("{hash}.{}", extension(input));
    let part = reqwest::multipart::Part::bytes(bytes)
        .file_name(name)
        .mime_str(&input.mime)
        .map_err(|e| ProviderError::InvalidInput(format!("bad input type {}: {e}", input.mime)))?;
    let form = reqwest::multipart::Form::new()
        .part("image", part)
        .text("type", "input")
        .text("subfolder", "pictesa")
        .text("overwrite", "true");
    let resp = authed(ctx, ctx.http.post(format!("{base}/api/upload/image")))
        .timeout(UPLOAD_TIMEOUT)
        .multipart(form)
        .send()
        .await
        .map_err(net_error)?;
    let v = json_response(resp).await?;
    let name = v
        .get("name")
        .and_then(Value::as_str)
        .ok_or_else(|| ProviderError::Failed(format!("ComfyUI upload returned no name: {v}")))?;
    Ok(
        match v
            .get("subfolder")
            .and_then(Value::as_str)
            .filter(|s| !s.is_empty())
        {
            Some(sub) => format!("{sub}/{name}"),
            None => name.to_string(),
        },
    )
}

/// Turns a `/prompt` rejection into a readable error. Missing model files become `NotSupported`.
pub(crate) fn prompt_error(status: u16, body: &str) -> ProviderError {
    let v: Value = serde_json::from_str(body).unwrap_or(Value::Null);
    let mut parts = Vec::new();
    let mut missing_files = 0;
    if let Some(errors) = v.get("node_errors").and_then(Value::as_object) {
        for (node, e) in errors {
            let class = e
                .get("class_type")
                .and_then(Value::as_str)
                .unwrap_or("node");
            for x in e
                .get("errors")
                .and_then(Value::as_array)
                .into_iter()
                .flatten()
            {
                let details = x
                    .get("details")
                    .and_then(Value::as_str)
                    .filter(|d| !d.is_empty());
                let message = x
                    .get("message")
                    .and_then(Value::as_str)
                    .unwrap_or("invalid");
                let kind = x.get("type").and_then(Value::as_str).unwrap_or("");
                let file = details
                    .is_some_and(|d| MODEL_EXTS.iter().any(|ext| d.contains(&format!("{ext}'"))));
                if kind == "value_not_in_list" && file {
                    missing_files += 1;
                    parts.push(format!(
                        "missing model file: {} ({class}, node {node})",
                        details.unwrap_or(message)
                    ));
                } else {
                    parts.push(format!(
                        "{class} (node {node}): {}",
                        details.unwrap_or(message)
                    ));
                }
            }
        }
    }
    if parts.is_empty() {
        let top = v.get("error").map(|e| {
            let msg = e.get("message").and_then(Value::as_str).unwrap_or("");
            let det = e.get("details").and_then(Value::as_str).unwrap_or("");
            if det.is_empty() {
                msg.to_string()
            } else {
                format!("{msg}: {det}")
            }
        });
        return match top {
            Some(t) if status < 500 && !t.is_empty() => {
                ProviderError::InvalidInput(format!("ComfyUI rejected the workflow: {t}"))
            }
            _ => ProviderError::from_http(status, body, None),
        };
    }
    let text = pictesa_core::error::truncate(&parts.join("; "), 900);
    if missing_files == parts.len() {
        ProviderError::NotSupported(format!(
            "the ComfyUI server lacks files this template needs: {text}"
        ))
    } else {
        ProviderError::InvalidInput(format!("ComfyUI rejected the workflow: {text}"))
    }
}

/// Node definitions for every class the templates use. Asks `/api/object_info/{class}` per class,
/// which stays small on servers with many custom nodes; falls back to the full `/api/object_info`
/// on servers without that route.
async fn node_info(ctx: &Ctx, base: &str, templates: &[Template]) -> ProviderResult<Value> {
    use futures::StreamExt;
    let mut classes: Vec<String> = templates
        .iter()
        .flat_map(|t| {
            t.workflow
                .values()
                .filter_map(|n| {
                    n.get("class_type")
                        .and_then(Value::as_str)
                        .map(String::from)
                })
                .chain(t.manifest.requires.nodes.iter().cloned())
        })
        .collect();
    if ctx.get("lora_dir").is_some() {classes.extend(["LoraLoader".into(),"LoraLoaderModelOnly".into()]);}
    classes.sort();
    classes.dedup();
    if classes.is_empty() {
        return get(ctx, &format!("{base}/api/system_stats"))
            .await
            .map(|_| json!({}));
    }
    let answers: Vec<ProviderResult<Value>> = futures::stream::iter(classes)
        .map(|c| async move {
            let mut url = url::Url::parse(&format!("{base}/api/object_info/"))
                .map_err(|e| ProviderError::Failed(e.to_string()))?;
            url.path_segments_mut()
                .map_err(|_| ProviderError::Failed("bad server URL".into()))?
                .pop_if_empty()
                .push(&c);
            get(ctx, url.as_str()).await
        })
        .buffer_unordered(8)
        .collect()
        .await;
    let mut merged = Map::new();
    for a in answers {
        match a {
            Ok(Value::Object(m)) => merged.extend(m),
            Ok(_) => {}
            Err(e) if matches!(&e, ProviderError::InvalidInput(m) | ProviderError::Failed(m) if m.starts_with("HTTP 404") || m.starts_with("HTTP 405")) =>
            {
                return get(ctx, &format!("{base}/api/object_info")).await;
            }
            Err(e) => return Err(e),
        }
    }
    Ok(Value::Object(merged))
}

// ---------------------------------------------------------------------------------------------
// Results

#[derive(Debug, Clone, PartialEq)]
pub(crate) struct FileRef {
    pub filename: String,
    pub subfolder: String,
    pub kind: String,
}

fn kind_of(filename: &str) -> Option<MediaKind> {
    let lower = filename.to_ascii_lowercase();
    let ext = lower.rsplit('.').next().unwrap_or("");
    match ext {
        "png" | "jpg" | "jpeg" | "webp" | "bmp" | "tif" | "tiff" | "exr" | "avif" => {
            Some(MediaKind::Image)
        }
        "mp4" | "webm" | "mov" | "mkv" | "m4v" | "gif" | "avi" => Some(MediaKind::Video),
        "mp3" | "wav" | "flac" | "ogg" | "opus" | "m4a" | "aac" => Some(MediaKind::Audio),
        _ => None,
    }
}

/// Picks the result files from a prompt's `outputs` (history or jobs API): saved files of the
/// wanted kind, from the template's output nodes when it names them.
pub(crate) fn output_files(
    outputs: &Value,
    want: Option<MediaKind>,
    nodes: &[String],
) -> Vec<FileRef> {
    let Some(map) = outputs.as_object() else {
        return Vec::new();
    };
    let collect = |only: &[String], allow_temp: bool| -> Vec<FileRef> {
        let mut ids: Vec<&String> = map
            .keys()
            .filter(|k| only.is_empty() || only.contains(k))
            .collect();
        ids.sort_by_key(|k| {
            k.split(':')
                .map(|p| p.parse::<u64>().unwrap_or(u64::MAX))
                .collect::<Vec<_>>()
        });
        let mut out = Vec::new();
        for id in ids {
            let Some(node) = map[id].as_object() else {
                continue;
            };
            for (key, items) in node {
                if !matches!(
                    key.as_str(),
                    "images" | "gifs" | "videos" | "video" | "audio" | "animated_images"
                ) {
                    continue;
                }
                for item in items.as_array().into_iter().flatten() {
                    let Some(filename) = item.get("filename").and_then(Value::as_str) else {
                        continue;
                    };
                    let kind = item.get("type").and_then(Value::as_str).unwrap_or("output");
                    if kind != "output" && !allow_temp {
                        continue;
                    }
                    if let (Some(w), Some(k)) = (want, kind_of(filename)) {
                        if w != k {
                            continue;
                        }
                    }
                    let f = FileRef {
                        filename: filename.to_string(),
                        subfolder: item
                            .get("subfolder")
                            .and_then(Value::as_str)
                            .unwrap_or("")
                            .to_string(),
                        kind: kind.to_string(),
                    };
                    if !out.contains(&f) {
                        out.push(f);
                    }
                }
            }
        }
        out
    };
    for (only, temp) in [(nodes, false), (&[][..], false), (&[][..], true)] {
        let found = collect(only, temp);
        if !found.is_empty() {
            return found;
        }
    }
    Vec::new()
}

fn view_url(base: &str, f: &FileRef) -> String {
    let mut url = url::Url::parse(&format!("{base}/api/view"))
        .unwrap_or_else(|_| url::Url::parse("http://127.0.0.1:8188/api/view").unwrap());
    url.query_pairs_mut()
        .append_pair("filename", &f.filename)
        .append_pair("subfolder", &f.subfolder)
        .append_pair("type", &f.kind);
    url.to_string()
}

/// A download for one result. Comfy Cloud answers `/api/view` with a redirect to a signed URL that
/// must be fetched without the API key, so keyed servers are asked first without following it.
async fn output_for(ctx: &Ctx, url: String) -> ProviderResult<Output> {
    let headers = auth_headers(ctx);
    if headers.is_empty() {
        return Ok(Output::url(url));
    }
    static NO_REDIRECT: OnceLock<reqwest::Client> = OnceLock::new();
    let client = NO_REDIRECT.get_or_init(|| {
        reqwest::Client::builder()
            .redirect(reqwest::redirect::Policy::none())
            .connect_timeout(std::time::Duration::from_secs(15))
            .build()
            .unwrap_or_default()
    });
    let mut rb = client.get(&url).timeout(API_TIMEOUT);
    for (k, v) in &headers {
        rb = rb.header(k, v);
    }
    let resp = rb.send().await.map_err(net_error)?;
    if resp.status().is_redirection() {
        if let Some(loc) = resp
            .headers()
            .get(reqwest::header::LOCATION)
            .and_then(|l| l.to_str().ok())
        {
            let target = url::Url::parse(&url)
                .and_then(|u| u.join(loc))
                .map(|u| u.to_string())
                .unwrap_or_else(|_| loc.to_string());
            return Ok(Output::url(target));
        }
    }
    if !resp.status().is_success() {
        let code = resp.status().as_u16();
        let text = resp.text().await.unwrap_or_default();
        return Err(ProviderError::from_http(code, &text, None));
    }
    Ok(Output::Url { url, headers })
}

/// Maps a failed run's history or job record to an error.
fn run_error(messages: Option<&Value>, job_error: Option<&Value>) -> ProviderError {
    let mut interrupted = false;
    let mut detail: Option<String> = None;
    let mut from_event = |data: &Value| {
        let msg = data
            .get("exception_message")
            .and_then(Value::as_str)
            .unwrap_or("")
            .trim()
            .to_string();
        let node = data.get("node_type").and_then(Value::as_str).unwrap_or("");
        let kind = data
            .get("exception_type")
            .and_then(Value::as_str)
            .unwrap_or("");
        let text = match (node.is_empty(), kind.is_empty()) {
            (false, false) => format!("{node}: {kind}: {msg}"),
            (false, true) => format!("{node}: {msg}"),
            _ => msg,
        };
        if !text.is_empty() {
            detail = Some(text);
        }
    };
    for m in messages.and_then(Value::as_array).into_iter().flatten() {
        match m.get(0).and_then(Value::as_str) {
            Some("execution_interrupted") => interrupted = true,
            Some("execution_error") => from_event(m.get(1).unwrap_or(&Value::Null)),
            _ => {}
        }
    }
    if let Some(e) = job_error {
        from_event(e);
    }
    if interrupted && detail.is_none() {
        return ProviderError::Failed("cancelled on the ComfyUI server".into());
    }
    let text = detail.unwrap_or_else(|| "the ComfyUI run failed".into());
    match failure(pictesa_core::error::truncate(&text, 900)) {
        ProviderError::Failed(m) if m.contains("OutOfMemory") || m.contains("out of memory") => {
            ProviderError::Failed(format!(
                "{m} (the GPU ran out of memory; try a smaller size or duration)"
            ))
        }
        other => other,
    }
}

/// The queue position of a pending prompt from `/api/queue` (`queue_pending` items are
/// `[number, prompt_id, ...]`), and whether it is running.
pub(crate) fn queue_state(queue: &Value, prompt_id: &str) -> Option<Result<u32, ()>> {
    let id_of = |item: &Value| item.get(1).and_then(Value::as_str).map(String::from);
    if queue
        .get("queue_running")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .any(|i| id_of(i).as_deref() == Some(prompt_id))
    {
        return Some(Err(()));
    }
    let mut pending: Vec<(f64, String)> = queue
        .get("queue_pending")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|i| Some((i.get(0)?.as_f64()?, id_of(i)?)))
        .collect();
    pending.sort_by(|a, b| a.0.partial_cmp(&b.0).unwrap_or(std::cmp::Ordering::Equal));
    let running = queue
        .get("queue_running")
        .and_then(Value::as_array)
        .map_or(0, Vec::len) as u32;
    pending
        .iter()
        .position(|(_, id)| id == prompt_id)
        .map(|p| Ok(p as u32 + running))
}

fn running(handle: &RemoteHandle) -> PollStatus {
    match live::get(&handle.id) {
        Some(step) if step.max > 0 => {
            let class = step
                .node
                .as_deref()
                .and_then(|n| handle.data.pointer(&format!("/classes/{n}")))
                .and_then(Value::as_str)
                .unwrap_or("");
            let what = if class.contains("Sampler") {
                "Sampling"
            } else {
                "Step"
            };
            PollStatus::Running {
                progress: Some((step.value as f32 / step.max as f32).clamp(0.0, 1.0)),
                message: Some(format!("{what} {}/{}", step.value, step.max)),
            }
        }
        _ => PollStatus::Running {
            progress: None,
            message: None,
        },
    }
}

impl ComfyUi {
    async fn finish(
        &self,
        ctx: &Ctx,
        base: &str,
        handle: &RemoteHandle,
        outputs: &Value,
    ) -> ProviderResult<PollStatus> {
        let want = handle
            .data
            .get("output")
            .and_then(|o| serde_json::from_value::<MediaKind>(o.clone()).ok());
        let nodes: Vec<String> = handle
            .data
            .get("outputs")
            .and_then(|o| serde_json::from_value(o.clone()).ok())
            .unwrap_or_default();
        let files = output_files(outputs, want, &nodes);
        if files.is_empty() {
            return Ok(PollStatus::Failed(ProviderError::Failed(format!(
                "ComfyUI finished without saving a {} (does the workflow end in a save node?)",
                want.map_or("file", MediaKind::as_str)
            ))));
        }
        let mut out = Vec::new();
        for f in files {
            out.push(output_for(ctx, view_url(base, &f)).await?);
        }
        live::forget(&handle.id);
        Ok(PollStatus::Done(out))
    }

    /// Servers before the jobs API: `/api/history/{id}` then `/api/queue`.
    async fn poll_legacy(
        &self,
        ctx: &Ctx,
        base: &str,
        handle: &RemoteHandle,
    ) -> ProviderResult<PollStatus> {
        let id = &handle.id;
        let history = get(ctx, &format!("{base}/api/history/{id}")).await?;
        if let Some(entry) = history.get(id.as_str()) {
            let status = entry.get("status");
            let ok = status
                .and_then(|s| s.get("status_str"))
                .and_then(Value::as_str);
            if ok == Some("error") {
                live::forget(id);
                return Ok(PollStatus::Failed(run_error(
                    status.and_then(|s| s.get("messages")),
                    None,
                )));
            }
            let outputs = entry.get("outputs").cloned().unwrap_or(Value::Null);
            return self.finish(ctx, base, handle, &outputs).await;
        }
        let queue = get(ctx, &format!("{base}/api/queue")).await?;
        Ok(match queue_state(&queue, id) {
            Some(Err(())) => running(handle),
            Some(Ok(position)) => PollStatus::Queued {
                position: Some(position),
            },
            None if live::was_cancelled(id) => {
                PollStatus::Failed(ProviderError::Failed("cancelled".into()))
            }
            None => PollStatus::Failed(ProviderError::Failed(
                "the ComfyUI server no longer knows this job (was it restarted or cleared?)".into(),
            )),
        })
    }
}

// ---------------------------------------------------------------------------------------------
// Provider

#[async_trait]
impl Provider for ComfyUi {
    fn info(&self) -> ProviderInfo {
        ProviderInfo {
            id: ID.into(),
            name: "ComfyUI".into(),
            kind: ProviderKind::Local,
            fields: vec![
                CredentialField::base_url(DEFAULT_URL, &["COMFYUI_URL"]),
                crate::training::lora_directory("PICTESA_COMFYUI_LORA_DIR"),
                CredentialField {
                    key: "api_key".into(),
                    label: "API key or bearer token (Comfy Cloud, reverse proxy)".into(),
                    env: vec!["COMFYUI_API_KEY".into(), "COMFY_CLOUD_API_KEY".into()],
                    secret: true,
                    optional: true,
                    default: None,
                    placeholder: Some("comfyui-...".into()),
                },
                CredentialField {
                    key: "workflows_dir".into(),
                    label: "Extra workflows folder".into(),
                    env: vec!["PICTESA_WORKFLOWS_DIR".into()],
                    secret: false,
                    optional: true,
                    default: None,
                    placeholder: Some("<data dir>/workflows/comfy".into()),
                },
            ],
            signup_url: Some("https://www.comfy.org/download".into()),
            docs_url: Some("https://docs.comfy.org/development/comfyui-server/comms_routes".into()),
            max_concurrency: 4,
        }
    }

    /// Templates are listed by `discover()`, which also checks them against the server.
    fn models(&self) -> Vec<ModelSpec> {
        Vec::new()
    }

    async fn discover(&self, ctx: &Ctx) -> ProviderResult<Vec<ModelSpec>> {
        let base = base(ctx, None);
        let templates = all_templates(ctx);
        let info = match node_info(ctx, &base, &templates).await {
            Ok(v) => Some(v),
            Err(ProviderError::Auth(e)) => return Err(ProviderError::Auth(e)),
            Err(e) => {
                tracing::debug!("ComfyUI at {base} not reachable for discovery: {e}");
                None
            }
        };
        Ok(templates
            .iter()
            .map(|t| {
                let mut s = spec_for(t);
                match &info {
                    Some(info) => {
                        let a = availability(t, info);
                        if !a.missing_nodes.is_empty() {
                            s.tags.push("needs-nodes".into());
                        }
                        if !a.missing_models.is_empty() {
                            s.tags.push("needs-models".into());
                            s.description = describe(t, Some(&a.missing_models));
                        }
                        s.meta["missing_nodes"] = json!(a.missing_nodes);
                        s.meta["missing_models"] = json!(a.missing_models);
                        let missing: Vec<&ModelFile> =
                            t.manifest.requires.models.iter().filter(|m| a.missing_models.contains(&m.name)).collect();
                        s.meta["downloads"] = json!(missing
                            .iter()
                            .map(|m| json!({ "name": m.name, "directory": m.directory, "url": m.url }))
                            .collect::<Vec<_>>());
                        if !a.rename.is_empty() {
                            s.meta["rename"] = json!(a.rename);
                        }
                    }
                    None => {
                        s.tags.push("unavailable".into());
                        s.meta["unavailable"] = json!(format!("ComfyUI at {base} did not answer; start it and refresh the model list"));
                    }
                }
                if ctx.get("lora_dir").is_some() && info.is_some() {
                    let family=pictesa_core::lora::family(&s.id,&s.meta);
                    if family.is_some() && lora_links(&t.workflow).is_some_and(|(_,clip)|info.as_ref().and_then(|i|i.get(if clip.is_some(){"LoraLoader"}else{"LoraLoaderModelOnly"})).is_some()) {s.meta["supports_loras"]=json!(true);s.meta["lora_family"]=json!(family);}
                }
                s
            })
            .collect())
    }

    async fn check(&self, ctx: &Ctx) -> ProviderResult<()> {
        get(ctx, &format!("{}/api/system_stats", base(ctx, None))).await?;
        Ok(())
    }

    async fn submit(
        &self,
        ctx: &Ctx,
        model: &ModelSpec,
        req: &GenerationRequest,
    ) -> ProviderResult<Submission> {
        let base = base(ctx, None);
        let t = find_template(ctx, model)?;
        let mut uploads: HashMap<InputRole, Vec<String>> = HashMap::new();
        let mut dims: HashMap<InputRole, (u32, u32)> = HashMap::new();
        for d in &t.manifest.inputs {
            for input in req.inputs(d.role).take(d.nodes.len()) {
                if let Some(size) = input_dims(input) {
                    dims.entry(d.role).or_insert(size);
                }
                let name = upload(ctx, &base, input).await?;
                uploads.entry(d.role).or_default().push(name);
            }
        }
        let rename = model
            .meta
            .get("rename")
            .and_then(Value::as_object)
            .cloned()
            .unwrap_or_default();
        let mut graph = build_graph(&t, req, &uploads, &dims, &rename)?;
        attach_loras(&mut graph,req)?;
        let classes: Map<String, Value> = graph
            .iter()
            .filter_map(|(k, n)| Some((k.clone(), n.get("class_type")?.clone())))
            .collect();
        let cid = client_id();
        // Open the event stream first so short runs do not finish before it listens.
        live::watch(ctx, &base, cid);
        // ComfyUI sends sampler previews only when asked: its default is `--preview-method none`.
        // "auto" uses TAESD when installed, else a cheap latent-to-RGB image.
        let mut body = json!({ "prompt": graph, "client_id": cid, "extra_data": { "preview_method": "auto" } });
        if let Some(k) = ctx.get("api_key").filter(|k| k.starts_with("comfyui-")) {
            // Lets partner (API) nodes inside a workflow bill the same Comfy account.
            body["extra_data"]["api_key_comfy_org"] = json!(k);
        }
        let resp = authed(ctx, ctx.http.post(format!("{base}/api/prompt")))
            .json(&body)
            .send()
            .await
            .map_err(net_error)?;
        let status = resp.status();
        let text = resp.text().await.map_err(net_error)?;
        if !status.is_success() {
            return Err(prompt_error(status.as_u16(), &text));
        }
        let answer: Value = serde_json::from_str(&text)?;
        let id = answer
            .get("prompt_id")
            .and_then(Value::as_str)
            .ok_or_else(|| {
                ProviderError::Failed(format!(
                    "ComfyUI returned no prompt id: {}",
                    pictesa_core::error::truncate(&text, 300)
                ))
            })?;
        Ok(Submission::Pending(RemoteHandle::with(
            id,
            json!({
                "base_url": base,
                "client_id": cid,
                "output": t.manifest.output,
                "outputs": t.manifest.outputs,
                "classes": classes,
            }),
        )))
    }

    async fn poll(&self, ctx: &Ctx, handle: &RemoteHandle) -> ProviderResult<PollStatus> {
        let base = base(ctx, Some(handle));
        let id = &handle.id;
        live::watch(
            ctx,
            &base,
            handle.data_str("client_id").unwrap_or(client_id()),
        );
        let resp = authed(ctx, ctx.http.get(format!("{base}/api/jobs/{id}")))
            .send()
            .await
            .map_err(net_error)?;
        if matches!(resp.status().as_u16(), 404 | 405) {
            return self.poll_legacy(ctx, &base, handle).await;
        }
        let job = json_response(resp).await?;
        let status = job
            .get("status")
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_ascii_lowercase();
        match status.as_str() {
            "completed" | "success" | "succeeded" => {
                let outputs = job.get("outputs").cloned().unwrap_or(Value::Null);
                if outputs.as_object().is_some_and(|o| !o.is_empty()) {
                    self.finish(ctx, &base, handle, &outputs).await
                } else {
                    // Some servers list outputs only in history.
                    self.poll_legacy(ctx, &base, handle).await
                }
            }
            "failed" | "error" | "non_retryable_error" | "lost" => {
                live::forget(id);
                let messages = job.pointer("/execution_status/messages");
                Ok(PollStatus::Failed(run_error(
                    messages,
                    job.get("execution_error"),
                )))
            }
            "cancelled" | "canceled" | "interrupted" => {
                live::forget(id);
                Ok(PollStatus::Failed(ProviderError::Failed(
                    "cancelled on the ComfyUI server".into(),
                )))
            }
            "pending" | "queued" | "submitted" | "queued_waiting" | "preparing" => {
                let queue = get(ctx, &format!("{base}/api/queue")).await.ok();
                let position = queue
                    .as_ref()
                    .and_then(|q| queue_state(q, id))
                    .and_then(Result::ok);
                Ok(PollStatus::Queued { position })
            }
            _ => Ok(running(handle)),
        }
    }

    async fn cancel(&self, ctx: &Ctx, handle: &RemoteHandle) -> ProviderResult<()> {
        let base = base(ctx, Some(handle));
        let id = &handle.id;
        // Dequeue when pending; interrupt only when this prompt is the one running, so another
        // user's job on a shared server is never stopped (older servers interrupt globally).
        let _ = authed(ctx, ctx.http.post(format!("{base}/api/queue")))
            .json(&json!({ "delete": [id] }))
            .send()
            .await;
        if let Ok(queue) = get(ctx, &format!("{base}/api/queue")).await {
            if queue_state(&queue, id) == Some(Err(())) {
                let _ = authed(ctx, ctx.http.post(format!("{base}/api/interrupt")))
                    .json(&json!({ "prompt_id": id }))
                    .send()
                    .await;
            }
        }
        live::forget(id);
        live::mark_cancelled(id);
        Ok(())
    }

    fn preview(&self, handle: &RemoteHandle) -> Option<(Vec<u8>, String)> {
        live::preview(&handle.id)
    }

    fn poll_interval(&self, attempt: u32) -> std::time::Duration {
        std::time::Duration::from_millis((500 + 250 * attempt as u64).min(2_000))
    }
}

// ---------------------------------------------------------------------------------------------
// Live progress from the `/ws` event stream

/// Listens to ComfyUI's WebSocket events (`progress`, `executing`, ...) and keeps the latest step of
/// each prompt. One listener per server and client id; it stops after two minutes without polls.
/// Progress is a bonus: when the socket cannot be opened, polling still reports status and results.
mod live {
    use std::{
        collections::HashMap,
        sync::{Mutex, OnceLock},
        time::{Duration, Instant},
    };

    use base64::Engine as _;
    use pictesa_core::Ctx;
    use serde_json::Value;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    #[derive(Debug, Clone, Default, PartialEq)]
    pub struct Step {
        pub value: u64,
        pub max: u64,
        pub node: Option<String>,
        at: Option<Instant>,
    }

    fn steps() -> &'static Mutex<HashMap<String, Step>> {
        static S: OnceLock<Mutex<HashMap<String, Step>>> = OnceLock::new();
        S.get_or_init(Default::default)
    }

    /// Latest preview image per prompt, from ComfyUI's binary websocket messages.
    fn previews() -> &'static Mutex<HashMap<String, (Vec<u8>, String, Instant)>> {
        static P: OnceLock<Mutex<HashMap<String, (Vec<u8>, String, Instant)>>> = OnceLock::new();
        P.get_or_init(Default::default)
    }

    /// The prompt each server is executing, by listener key (`base|client_id`). Previews without
    /// metadata belong to it; frames of any other prompt are late or foreign and dropped.
    fn running() -> &'static Mutex<HashMap<String, String>> {
        static R: OnceLock<Mutex<HashMap<String, String>>> = OnceLock::new();
        R.get_or_init(Default::default)
    }

    pub fn preview(prompt_id: &str) -> Option<(Vec<u8>, String)> {
        previews().lock().ok()?.get(prompt_id).map(|(b, m, _)| (b.clone(), m.clone()))
    }

    /// Binary event from one listener: type 1 is a preview image (u32 image format, then bytes);
    /// type 4 adds a JSON metadata block with the prompt id before the image.
    pub fn apply_binary(listener: &str, msg: &[u8]) {
        if msg.len() < 8 {
            return;
        }
        let Some(prompt) = running().lock().ok().and_then(|r| r.get(listener).cloned()) else { return };
        let event = u32::from_be_bytes([msg[0], msg[1], msg[2], msg[3]]);
        let (format, image) = match event {
            1 => (u32::from_be_bytes([msg[4], msg[5], msg[6], msg[7]]), &msg[8..]),
            4 => {
                let len = u32::from_be_bytes([msg[4], msg[5], msg[6], msg[7]]) as usize;
                if msg.len() < 8 + len {
                    return;
                }
                let Ok(meta) = serde_json::from_slice::<Value>(&msg[8..8 + len]) else { return };
                if meta.get("prompt_id").and_then(Value::as_str) != Some(prompt.as_str()) {
                    return;
                }
                let format = if meta.get("image_type").and_then(Value::as_str).is_some_and(|t| t.contains("png")) { 2 } else { 1 };
                (format, &msg[8 + len..])
            }
            _ => return,
        };
        if image.len() < 16 || image.len() > 16 << 20 {
            return;
        }
        let mime = if format == 2 { "image/png" } else { "image/jpeg" };
        if let Ok(mut p) = previews().lock() {
            if p.len() > 64 {
                p.retain(|_, (_, _, at)| at.elapsed() < Duration::from_secs(3600));
            }
            p.insert(prompt, (image.to_vec(), mime.to_string(), Instant::now()));
        }
    }

    fn listeners() -> &'static Mutex<HashMap<String, Instant>> {
        static L: OnceLock<Mutex<HashMap<String, Instant>>> = OnceLock::new();
        L.get_or_init(Default::default)
    }

    pub fn get(prompt_id: &str) -> Option<Step> {
        steps().lock().ok()?.get(prompt_id).cloned()
    }

    pub fn forget(prompt_id: &str) {
        if let Ok(mut s) = steps().lock() {
            s.remove(prompt_id);
        }
    }

    fn cancelled() -> &'static Mutex<Vec<String>> {
        static C: OnceLock<Mutex<Vec<String>>> = OnceLock::new();
        C.get_or_init(Default::default)
    }

    /// Remembers prompts this process cancelled, so a dequeued prompt reads as cancelled, not lost.
    pub fn mark_cancelled(prompt_id: &str) {
        if let Ok(mut c) = cancelled().lock() {
            if c.len() >= 256 {
                c.remove(0);
            }
            c.push(prompt_id.to_string());
        }
    }

    pub fn was_cancelled(prompt_id: &str) -> bool {
        cancelled()
            .lock()
            .is_ok_and(|c| c.iter().any(|x| x == prompt_id))
    }

    const IDLE: Duration = Duration::from_secs(120);

    /// Marks the listener for this server as wanted, starting it when needed.
    pub fn watch(ctx: &Ctx, base: &str, client_id: &str) {
        let key = format!("{base}|{client_id}");
        let fresh = {
            let Ok(mut l) = listeners().lock() else {
                return;
            };
            l.insert(key.clone(), Instant::now()).is_none()
        };
        if fresh && tokio::runtime::Handle::try_current().is_ok() {
            tokio::spawn(run(
                ctx.clone(),
                base.to_string(),
                client_id.to_string(),
                key,
            ));
        }
    }

    fn idle(key: &str) -> bool {
        listeners()
            .lock()
            .map(|l| l.get(key).is_none_or(|t| t.elapsed() > IDLE))
            .unwrap_or(true)
    }

    async fn run(ctx: Ctx, base: String, client_id: String, key: String) {
        let mut failures = 0u32;
        while !idle(&key) {
            match connect(&ctx, &base, &client_id).await {
                Ok(mut io) => {
                    failures = 0;
                    if let Err(e) = read_loop(&mut io, &key).await {
                        tracing::debug!("ComfyUI event stream closed: {e}");
                    }
                }
                Err(e) => {
                    failures += 1;
                    tracing::debug!("ComfyUI event stream unavailable: {e}");
                }
            }
            tokio::time::sleep(Duration::from_secs((2 * failures as u64).clamp(2, 30))).await;
        }
        if let Ok(mut l) = listeners().lock() {
            l.remove(&key);
        }
    }

    async fn connect(ctx: &Ctx, base: &str, client_id: &str) -> Result<reqwest::Upgraded, String> {
        let mut url = url::Url::parse(&format!("{base}/ws")).map_err(|e| e.to_string())?;
        url.query_pairs_mut().append_pair("clientId", client_id);
        let mut rb = ctx.http.get(url.as_str());
        if let Some(k) = ctx.get("api_key") {
            if k.starts_with("comfyui-") {
                // Comfy Cloud authenticates the socket with a token parameter.
                url.query_pairs_mut().append_pair("token", k);
                rb = ctx.http.get(url.as_str()).header("X-API-Key", k);
            } else {
                rb = rb.bearer_auth(k);
            }
        }
        let key =
            base64::engine::general_purpose::STANDARD.encode(ulid::Ulid::new().0.to_le_bytes());
        let handshake = rb
            .header(reqwest::header::CONNECTION, "Upgrade")
            .header(reqwest::header::UPGRADE, "websocket")
            .header("Sec-WebSocket-Version", "13")
            .header("Sec-WebSocket-Key", key)
            .send();
        // Bound only the handshake: a request timeout would also end the long-lived socket.
        let resp = tokio::time::timeout(Duration::from_secs(20), handshake)
            .await
            .map_err(|_| "handshake timed out".to_string())?
            .map_err(|e| e.to_string())?;
        if resp.status() != reqwest::StatusCode::SWITCHING_PROTOCOLS {
            return Err(format!("HTTP {}", resp.status()));
        }
        resp.upgrade().await.map_err(|e| e.to_string())
    }

    async fn read_loop(io: &mut reqwest::Upgraded, key: &str) -> std::io::Result<()> {
        let mut buf: Vec<u8> = Vec::new();
        let mut message: Vec<u8> = Vec::new();
        let mut text = false;
        let mut chunk = vec![0u8; 16384];
        loop {
            while let Some(frame) = parse_frame(&mut buf) {
                match frame.opcode {
                    0x1 | 0x2 => {
                        text = frame.opcode == 0x1;
                        message = frame.payload;
                    }
                    0x0 => message.extend_from_slice(&frame.payload),
                    0x8 => {
                        let _ = io.write_all(&client_frame(0x8, &[])).await;
                        return Ok(());
                    }
                    0x9 => io.write_all(&client_frame(0xA, &frame.payload)).await?,
                    _ => continue,
                }
                if frame.fin {
                    if text {
                        if let Ok(v) = serde_json::from_slice::<Value>(&message) {
                            apply(key, &v);
                        }
                    } else {
                        apply_binary(key, &message);
                    }
                    message.clear();
                }
            }
            if buf.len() > 64 << 20 || message.len() > 64 << 20 {
                return Err(std::io::Error::other("ComfyUI sent an oversized event frame"));
            }
            match tokio::time::timeout(Duration::from_secs(30), io.read(&mut chunk)).await {
                Err(_) if idle(key) => return Ok(()),
                Err(_) => {}
                Ok(Ok(0)) => return Ok(()),
                Ok(Ok(n)) => buf.extend_from_slice(&chunk[..n]),
                Ok(Err(e)) => return Err(e),
            }
            if idle(key) {
                return Ok(());
            }
        }
    }

    pub struct Frame {
        pub fin: bool,
        pub opcode: u8,
        pub payload: Vec<u8>,
    }

    /// Takes one complete frame off the front of `buf`, or returns `None` until more bytes arrive.
    pub fn parse_frame(buf: &mut Vec<u8>) -> Option<Frame> {
        if buf.len() < 2 {
            return None;
        }
        let fin = buf[0] & 0x80 != 0;
        let opcode = buf[0] & 0x0F;
        let masked = buf[1] & 0x80 != 0;
        let mut len = (buf[1] & 0x7F) as usize;
        let mut pos = 2;
        if len == 126 {
            len = u16::from_be_bytes([*buf.get(2)?, *buf.get(3)?]) as usize;
            pos = 4;
        } else if len == 127 {
            let b: [u8; 8] = buf.get(2..10)?.try_into().ok()?;
            len = u64::from_be_bytes(b) as usize;
            pos = 10;
        }
        let mask: Option<[u8; 4]> = if masked {
            let m = buf.get(pos..pos + 4)?.try_into().ok()?;
            pos += 4;
            Some(m)
        } else {
            None
        };
        if buf.len() < pos + len {
            return None;
        }
        let mut payload = buf[pos..pos + len].to_vec();
        if let Some(m) = mask {
            payload
                .iter_mut()
                .enumerate()
                .for_each(|(i, b)| *b ^= m[i % 4]);
        }
        buf.drain(..pos + len);
        Some(Frame {
            fin,
            opcode,
            payload,
        })
    }

    /// A masked client frame, as RFC 6455 requires for client-to-server traffic.
    pub fn client_frame(opcode: u8, payload: &[u8]) -> Vec<u8> {
        let mut out = vec![0x80 | opcode];
        match payload.len() {
            n if n < 126 => out.push(0x80 | n as u8),
            n if n <= u16::MAX as usize => {
                out.push(0x80 | 126);
                out.extend_from_slice(&(n as u16).to_be_bytes());
            }
            n => {
                out.push(0x80 | 127);
                out.extend_from_slice(&(n as u64).to_be_bytes());
            }
        }
        let mask = (ulid::Ulid::new().0 as u32).to_be_bytes();
        out.extend_from_slice(&mask);
        out.extend(payload.iter().enumerate().map(|(i, b)| b ^ mask[i % 4]));
        out
    }

    /// Records one ComfyUI event.
    pub fn apply(listener: &str, v: &Value) {
        let data = v.get("data").unwrap_or(&Value::Null);
        let Some(prompt) = data.get("prompt_id").and_then(Value::as_str) else {
            return;
        };
        if let Ok(mut r) = running().lock() {
            match v.get("type").and_then(Value::as_str) {
                Some("progress" | "executing" | "execution_start") => {
                    r.insert(listener.to_string(), prompt.to_string());
                }
                Some("execution_success" | "execution_error" | "execution_interrupted") if r.get(listener).is_some_and(|p| p == prompt) => {
                    r.remove(listener);
                }
                _ => {}
            }
        }
        let Ok(mut s) = steps().lock() else { return };
        match v.get("type").and_then(Value::as_str).unwrap_or("") {
            "progress" => {
                let (Some(value), Some(max)) = (
                    data.get("value").and_then(Value::as_u64),
                    data.get("max").and_then(Value::as_u64),
                ) else {
                    return;
                };
                let node = data.get("node").and_then(|n| {
                    n.as_str()
                        .map(String::from)
                        .or_else(|| n.as_u64().map(|x| x.to_string()))
                });
                s.insert(
                    prompt.to_string(),
                    Step {
                        value,
                        max,
                        node,
                        at: Some(Instant::now()),
                    },
                );
            }
            "execution_success" | "execution_error" | "execution_interrupted" => {
                s.remove(prompt);
                if let Ok(mut p) = previews().lock() {
                    p.remove(prompt);
                }
            }
            _ => {}
        }
        // Drop entries of prompts nobody polls any more.
        if s.len() > 256 {
            s.retain(|_, st| {
                st.at
                    .is_some_and(|t| t.elapsed() < Duration::from_secs(3600))
            });
        }
    }
}

// ---------------------------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    fn req(params: Value) -> GenerationRequest {
        GenerationRequest {
            job_id: "job".into(),
            model: "x".into(),
            prompt: "a red fox".into(),
            negative_prompt: None,
            params: params.as_object().cloned().unwrap_or_default(),
            inputs: vec![],
            num_outputs: 2,
        }
    }

    fn builtin(id: &str) -> Template {
        builtins()
            .iter()
            .find(|t| t.id == id)
            .cloned()
            .unwrap_or_else(|| panic!("no built-in {id}"))
    }

    #[test]
    fn every_builtin_template_parses_and_binds_existing_nodes() {
        assert_eq!(
            builtins().len(),
            BUILTIN.len(),
            "a built-in template failed to parse (see the error log)"
        );
        for t in builtins() {
            let s = spec_for(t);
            assert!(s.open_weights, "{}", t.id);
            assert_eq!(
                s.pricing.as_ref().map(|p| matches!(p, Pricing::Local)),
                Some(true)
            );
            for b in t.manifest.params.iter().flat_map(|p| &p.bind) {
                let node = &t.workflow[b.node()];
                assert!(
                    node["inputs"].get(b.input()).is_some(),
                    "{}: node {} has no input {}",
                    t.id,
                    b.node(),
                    b.input()
                );
            }
            // The manifest lists what the graph loads.
            let mut classes: Vec<&str> = t
                .workflow
                .values()
                .filter_map(|n| n["class_type"].as_str())
                .collect();
            classes.sort_unstable();
            classes.dedup();
            assert_eq!(
                classes,
                t.manifest
                    .requires
                    .nodes
                    .iter()
                    .map(String::as_str)
                    .collect::<Vec<_>>(),
                "{}",
                t.id
            );
        }
    }

    #[test]
    fn rejects_ui_format_and_unknown_nodes() {
        let manifest =
            r#"{"name":"x","output":"image","tasks":["text_to_image"],"prompt":[["9","text"]]}"#;
        let ui = r#"{"nodes":[],"links":[]}"#;
        assert!(parse_template("x", manifest, ui, None)
            .unwrap_err()
            .contains("Export (API)"));
        let graph = r#"{"1":{"class_type":"CLIPTextEncode","inputs":{"text":""}}}"#;
        assert!(parse_template("x", manifest, graph, None)
            .unwrap_err()
            .contains("node 9"));
        assert!(parse_template("Bad Id", manifest, graph, None).is_err());
    }

    #[test]
    fn text_to_image_graph_gets_prompt_size_seed_and_batch() {
        let t = builtin("flux3-klein-t2i");
        let g = build_graph(
            &t,
            &req(json!({"aspect_ratio": "16:9", "seed": 7, "steps": 12})),
            &HashMap::new(),
            &HashMap::new(),
            &Map::new(),
        )
        .unwrap();
        let find = |class: &str| {
            g.values()
                .find(|n| n["class_type"] == class)
                .unwrap()
                .clone()
        };
        let positive = t.manifest.prompt[0].node().to_string();
        assert_eq!(g[&positive]["inputs"]["text"], "a red fox");
        let latent = find("EmptyFlux2LatentImage");
        assert_eq!(
            (
                latent["inputs"]["width"].clone(),
                latent["inputs"]["height"].clone()
            ),
            (json!(1360), json!(768))
        );
        assert_eq!(latent["inputs"]["batch_size"], 2);
        let sampler = find("KSampler");
        assert_eq!(sampler["inputs"]["seed"], 7);
        assert_eq!(sampler["inputs"]["steps"], 12);
    }

    #[test]
    fn missing_seed_is_randomized_and_defaults_apply() {
        let t = builtin("flux3-klein-t2i");
        let a = build_graph(
            &t,
            &req(json!({})),
            &HashMap::new(),
            &HashMap::new(),
            &Map::new(),
        )
        .unwrap();
        let b = build_graph(
            &t,
            &req(json!({})),
            &HashMap::new(),
            &HashMap::new(),
            &Map::new(),
        )
        .unwrap();
        let seed = |g: &Graph| {
            g.values().find(|n| n["class_type"] == "KSampler").unwrap()["inputs"]["seed"].clone()
        };
        assert!(seed(&a).is_u64());
        assert_ne!(seed(&a), seed(&b));
        let steps =
            a.values().find(|n| n["class_type"] == "KSampler").unwrap()["inputs"]["steps"].clone();
        assert_eq!(steps, 16, "manifest default");
    }

    #[test]
    fn optional_input_nodes_are_removed_with_their_links() {
        let t = builtin("wan22-5b");
        let g = build_graph(
            &t,
            &req(json!({"duration": 3})),
            &HashMap::new(),
            &HashMap::new(),
            &Map::new(),
        )
        .unwrap();
        assert!(!g.values().any(|n| n["class_type"] == "LoadImage"));
        let latent = g
            .values()
            .find(|n| n["class_type"] == "Wan22ImageToVideoLatent")
            .unwrap();
        assert!(latent["inputs"].get("start_image").is_none());
        assert_eq!(latent["inputs"]["length"], 73, "3 s at 24 fps, 4n+1 frames");
        assert_eq!(
            (
                latent["inputs"]["width"].clone(),
                latent["inputs"]["height"].clone()
            ),
            (json!(1280), json!(704))
        );

        let uploads = HashMap::from([(InputRole::StartFrame, vec!["pictesa/abc.png".to_string()])]);
        let dims = HashMap::from([(InputRole::StartFrame, (720u32, 1280u32))]);
        let g = build_graph(&t, &req(json!({})), &uploads, &dims, &Map::new()).unwrap();
        let load = g.values().find(|n| n["class_type"] == "LoadImage").unwrap();
        assert_eq!(load["inputs"]["image"], "pictesa/abc.png");
        let latent = g
            .values()
            .find(|n| n["class_type"] == "Wan22ImageToVideoLatent")
            .unwrap();
        assert!(latent["inputs"]["start_image"].is_array());
        assert_eq!(
            (
                latent["inputs"]["width"].clone(),
                latent["inputs"]["height"].clone()
            ),
            (json!(704), json!(1280)),
            "portrait from the image"
        );
    }

    #[test]
    fn required_inputs_are_enforced_and_extra_references_fill_more_loaders() {
        let t = builtin("flux3-klein-edit");
        let err = build_graph(
            &t,
            &req(json!({})),
            &HashMap::new(),
            &HashMap::new(),
            &Map::new(),
        )
        .unwrap_err();
        assert!(matches!(err, ProviderError::InvalidInput(_)));
        let uploads = HashMap::from([(
            InputRole::Reference,
            vec!["pictesa/a.png".to_string(), "pictesa/b.png".to_string()],
        )]);
        let g = build_graph(&t, &req(json!({})), &uploads, &HashMap::new(), &Map::new()).unwrap();
        let loads: Vec<&Value> = g
            .values()
            .filter(|n| n["class_type"] == "LoadImage")
            .collect();
        assert_eq!(loads.len(), 2);
        let refs = g
            .values()
            .find(|n| n["class_type"] == "ReferenceMultiLatent")
            .unwrap();
        let linked = refs["inputs"]
            .as_object()
            .unwrap()
            .keys()
            .filter(|k| k.starts_with("images."))
            .count();
        assert_eq!(linked, 2);
    }

    #[test]
    fn negative_prompt_keeps_template_text_unless_given() {
        let t = builtin("wan22-i2v");
        let uploads = HashMap::from([(InputRole::StartFrame, vec!["a.png".to_string()])]);
        let neg = t.manifest.negative_prompt[0].node().to_string();
        let original = t.workflow[&neg]["inputs"]["text"].clone();
        let g = build_graph(&t, &req(json!({})), &uploads, &HashMap::new(), &Map::new()).unwrap();
        assert_eq!(g[&neg]["inputs"]["text"], original);
        let mut r = req(json!({}));
        r.negative_prompt = Some("blurry".into());
        let g = build_graph(&t, &r, &uploads, &HashMap::new(), &Map::new()).unwrap();
        assert_eq!(g[&neg]["inputs"]["text"], "blurry");
    }

    #[test]
    fn mapped_params_and_resolution_presets() {
        let t = builtin("esrgan-upscale");
        let uploads = HashMap::from([(InputRole::Reference, vec!["x.png".to_string()])]);
        let g = build_graph(
            &t,
            &req(json!({"scale": 2})),
            &uploads,
            &HashMap::new(),
            &Map::new(),
        )
        .unwrap();
        let scale = g
            .values()
            .find(|n| n["class_type"] == "ImageScaleBy")
            .unwrap();
        assert_eq!(scale["inputs"]["scale_by"], 0.5);

        let size = t_size("ltx25-t2v");
        let r = req(json!({"aspect_ratio": "9:16", "resolution": "1080p"}));
        assert_eq!(
            compute_size(&size, MediaKind::Video, &r, &HashMap::new()),
            (1088, 1920)
        );
        let r = req(json!({"width": 1000, "height": 500}));
        assert_eq!(
            compute_size(&size, MediaKind::Video, &r, &HashMap::new()),
            (1024, 512)
        );
    }

    fn t_size(id: &str) -> SizeDef {
        builtin(id).manifest.size.clone().unwrap()
    }

    #[test]
    fn model_files_in_subfolders_are_renamed() {
        let t = builtin("esrgan-upscale");
        let info = json!({
            "UpscaleModelLoader": {"input": {"required": {"model_name": ["COMBO", {"options": ["upscale/RealESRGAN_x4plus.safetensors"]}]}}},
            "LoadImage": {"input": {"required": {"image": [["example.png"], {}]}}},
            "ImageUpscaleWithModel": {"input": {}}, "ImageScaleBy": {"input": {}}
        });
        let a = availability(&t, &info);
        assert_eq!(a.missing_nodes, vec!["SaveImage".to_string()]);
        assert!(a.missing_models.is_empty());
        assert_eq!(
            a.rename
                .get("RealESRGAN_x4plus.safetensors")
                .map(String::as_str),
            Some("upscale/RealESRGAN_x4plus.safetensors")
        );
        let rename: Map<String, Value> = a
            .rename
            .iter()
            .map(|(k, v)| (k.clone(), json!(v)))
            .collect();
        let uploads = HashMap::from([(InputRole::Reference, vec!["x.png".to_string()])]);
        let g = build_graph(&t, &req(json!({})), &uploads, &HashMap::new(), &rename).unwrap();
        let loader = g
            .values()
            .find(|n| n["class_type"] == "UpscaleModelLoader")
            .unwrap();
        assert_eq!(
            loader["inputs"]["model_name"],
            "upscale/RealESRGAN_x4plus.safetensors"
        );
    }

    #[test]
    fn availability_against_recorded_object_info() {
        // Trimmed from a CPU-only ComfyUI 0.42.1 with no model files installed.
        let info: Value =
            serde_json::from_str(include_str!("../tests/fixtures/comfyui_object_info.json"))
                .unwrap();
        for t in builtins() {
            let a = availability(t, &info);
            assert!(
                a.missing_nodes.is_empty(),
                "{} needs nodes ComfyUI 0.42.1 lacks: {:?}",
                t.id,
                a.missing_nodes
            );
            let required: Vec<&str> = t
                .manifest
                .requires
                .models
                .iter()
                .map(|m| m.name.as_str())
                .collect();
            assert!(!a.missing_models.is_empty(), "{}", t.id);
            for m in &a.missing_models {
                assert!(
                    required.contains(&m.as_str()),
                    "{}: {m} is loaded but not listed in requires.models",
                    t.id
                );
            }
        }
        let a = availability(&builtin("wan22-i2v"), &info);
        assert!(a
            .missing_models
            .contains(&"wan2.2_i2v_lightx2v_4steps_lora_v1_low_noise.safetensors".to_string()));
        assert!(
            !a.missing_models.iter().any(|m| m.ends_with(".png")),
            "uploaded inputs are not model files"
        );
    }

    #[test]
    fn prompt_errors_are_readable() {
        let body = r#"{"error": {"type": "prompt_outputs_failed_validation", "message": "Prompt outputs failed validation", "details": "", "extra_info": {}},
            "node_errors": {"2": {"errors": [{"type": "value_not_in_list", "message": "Value not in list", "details": "unet_name: 'flux3-image-klein-int8-convrot.safetensors' not in []", "extra_info": {}}], "dependent_outputs": ["9"], "class_type": "UNETLoader"}}}"#;
        match prompt_error(400, body) {
            ProviderError::NotSupported(m) => assert!(
                m.contains("flux3-image-klein-int8-convrot.safetensors")
                    && m.contains("UNETLoader"),
                "{m}"
            ),
            other => panic!("{other:?}"),
        }
        let body = r#"{"error": {"type": "prompt_outputs_failed_validation", "message": "Prompt outputs failed validation", "details": ""},
            "node_errors": {"7": {"errors": [{"type": "value_bigger_than_max", "message": "Value bigger than max", "details": "steps: 20000 bigger than max of 10000"}], "class_type": "KSampler"}}}"#;
        assert!(
            matches!(prompt_error(400, body), ProviderError::InvalidInput(m) if m.contains("KSampler"))
        );
        let body = r#"{"error": {"type": "invalid_prompt", "message": "Cannot execute because a node is missing the class_type property.", "details": "Node ID '#5'"}, "node_errors": {}}"#;
        assert!(
            matches!(prompt_error(400, body), ProviderError::InvalidInput(m) if m.contains("class_type"))
        );
    }

    #[test]
    fn history_outputs_pick_saved_files_of_the_right_kind() {
        let outputs = json!({
            "3": {"images": [{"filename": "ComfyUI_temp_x.png", "subfolder": "", "type": "temp"}]},
            "12": {"images": [{"filename": "upscale-video_00001_.mp4", "subfolder": "pictesa", "type": "output"}], "animated": [true]},
            "15": {"images": [{"filename": "frame.png", "subfolder": "pictesa", "type": "output"}]}
        });
        let v = output_files(&outputs, Some(MediaKind::Video), &[]);
        assert_eq!(
            v,
            vec![FileRef {
                filename: "upscale-video_00001_.mp4".into(),
                subfolder: "pictesa".into(),
                kind: "output".into()
            }]
        );
        let i = output_files(&outputs, Some(MediaKind::Image), &["15".to_string()]);
        assert_eq!(i.len(), 1);
        assert_eq!(i[0].filename, "frame.png");
        // Only previews: fall back to temp files rather than failing.
        let only_temp =
            json!({"3": {"images": [{"filename": "p.png", "subfolder": "", "type": "temp"}]}});
        assert_eq!(
            output_files(&only_temp, Some(MediaKind::Image), &[]).len(),
            1
        );
        assert_eq!(
            view_url("http://127.0.0.1:8188", &v[0]),
            "http://127.0.0.1:8188/api/view?filename=upscale-video_00001_.mp4&subfolder=pictesa&type=output"
        );
    }

    #[test]
    fn queue_positions_and_run_errors() {
        let queue = json!({
            "queue_running": [[5, "run-1", {}, {}, []]],
            "queue_pending": [[9, "late", {}, {}, []], [7, "mine", {}, {}, []]]
        });
        assert_eq!(queue_state(&queue, "run-1"), Some(Err(())));
        assert_eq!(queue_state(&queue, "mine"), Some(Ok(1)));
        assert_eq!(queue_state(&queue, "late"), Some(Ok(2)));
        assert_eq!(queue_state(&queue, "gone"), None);

        let messages = json!([
            ["execution_start", {"prompt_id": "p"}],
            ["execution_error", {"prompt_id": "p", "node_type": "KSampler", "exception_type": "torch.OutOfMemoryError", "exception_message": "CUDA out of memory."}]
        ]);
        match run_error(Some(&messages), None) {
            ProviderError::Failed(m) => {
                assert!(m.contains("KSampler") && m.contains("smaller"), "{m}")
            }
            other => panic!("{other:?}"),
        }
        let interrupted = json!([["execution_interrupted", {"prompt_id": "p"}]]);
        assert!(
            matches!(run_error(Some(&interrupted), None), ProviderError::Failed(m) if m.contains("cancelled"))
        );
    }

    #[test]
    fn websocket_frames_round_trip_and_progress_events() {
        let mut buf = vec![0x81, 5, b'h', b'e', b'l', b'l', b'o', 0x89];
        let f = live::parse_frame(&mut buf).unwrap();
        assert!(f.fin);
        assert_eq!((f.opcode, f.payload.as_slice()), (1, &b"hello"[..]));
        assert!(
            live::parse_frame(&mut buf).is_none(),
            "incomplete frame waits for more bytes"
        );
        buf.push(0);
        assert_eq!(live::parse_frame(&mut buf).unwrap().opcode, 9);
    }

    #[test]
    fn keeps_preview_images_from_binary_events() {
        let (a, b) = ("http://a|c1", "http://b|c1");
        // Type 4 carries the prompt id in a JSON block before the image.
        live::apply(a, &json!({"type": "execution_start", "data": {"prompt_id": "p-meta"}}));
        let meta = br#"{"prompt_id":"p-meta","image_type":"image/png"}"#;
        let mut msg = 4u32.to_be_bytes().to_vec();
        msg.extend_from_slice(&(meta.len() as u32).to_be_bytes());
        msg.extend_from_slice(meta);
        msg.extend_from_slice(&[0x89, b'P', b'N', b'G', 1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12]);
        live::apply_binary(a, &msg);
        let (bytes, mime) = live::preview("p-meta").expect("stored");
        assert_eq!(mime, "image/png");
        assert_eq!(bytes.len(), 16);
        // Type 1 belongs to the prompt running on the same server, never to another server's.
        live::apply(a, &json!({"type": "executing", "data": {"prompt_id": "p-run", "node": "3"}}));
        let mut msg = 1u32.to_be_bytes().to_vec();
        msg.extend_from_slice(&1u32.to_be_bytes());
        msg.extend_from_slice(&[0xFF, 0xD8, 0xFF, 0xE0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0]);
        live::apply_binary(b, &msg);
        assert!(live::preview("p-run").is_none(), "a frame from server b is not a's");
        live::apply_binary(a, &msg);
        assert_eq!(live::preview("p-run").unwrap().1, "image/jpeg");
        // Once the prompt ends, late frames are dropped instead of stored again.
        live::apply(a, &json!({"type": "execution_success", "data": {"prompt_id": "p-run"}}));
        live::apply_binary(a, &msg);
        assert!(live::preview("p-run").is_none());

        let big = vec![7u8; 300];
        let mut framed = live::client_frame(0x2, &big);
        let f = live::parse_frame(&mut framed).unwrap();
        assert_eq!(f.payload, big, "masked client frames decode back");
        assert!(framed.is_empty());

        live::apply(
            a,
            &json!({"type": "progress", "data": {"value": 3, "max": 20, "prompt_id": "pid-test", "node": "7"}}),
        );
        let step = live::get("pid-test").unwrap();
        assert_eq!(
            (step.value, step.max, step.node.as_deref()),
            (3, 20, Some("7"))
        );
        let handle = RemoteHandle::with("pid-test", json!({"classes": {"7": "KSampler"}}));
        match running(&handle) {
            PollStatus::Running {
                progress: Some(p),
                message: Some(m),
            } => {
                assert!((p - 0.15).abs() < 1e-6);
                assert_eq!(m, "Sampling 3/20");
            }
            other => panic!("{other:?}"),
        }
        live::apply(a, &json!({"type": "execution_success", "data": {"prompt_id": "pid-test"}}));
        assert!(live::get("pid-test").is_none());
    }

    #[test]
    fn reads_image_dimensions_from_headers() {
        let dir = std::env::temp_dir().join(format!("pictesa-comfy-dims-{}", ulid::Ulid::new()));
        std::fs::create_dir_all(&dir).unwrap();
        let png = dir.join("a.png");
        let mut bytes = b"\x89PNG\r\n\x1a\n\0\0\0\rIHDR".to_vec();
        bytes.extend_from_slice(&640u32.to_be_bytes());
        bytes.extend_from_slice(&360u32.to_be_bytes());
        bytes.extend_from_slice(&[8, 2, 0, 0, 0]);
        std::fs::write(&png, &bytes).unwrap();
        assert_eq!(image_dims(&png), Some((640, 360)));
        let jpg = dir.join("a.jpg");
        let jpeg = [
            0xFF, 0xD8, 0xFF, 0xE0, 0x00, 0x04, 0x00, 0x00, 0xFF, 0xC0, 0x00, 0x11, 0x08, 0x01,
            0xE0, 0x02, 0x80, 0x03, 0x01,
        ];
        std::fs::write(&jpg, jpeg).unwrap();
        assert_eq!(image_dims(&jpg), Some((640, 480)));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn user_templates_override_builtins() {
        let root = std::env::temp_dir().join(format!("pictesa-comfy-user-{}", ulid::Ulid::new()));
        let dir = root.join("esrgan-upscale");
        std::fs::create_dir_all(&dir).unwrap();
        let manifest = r#"{"schema":1,"name":"My upscaler","output":"image","tasks":["upscale"],
            "inputs":[{"role":"reference","kind":"image","nodes":["1"],"min":1}]}"#;
        let workflow = r#"{"1":{"class_type":"LoadImage","inputs":{"image":"x.png"}},"2":{"class_type":"SaveImage","inputs":{"images":["1",0],"filename_prefix":"p"}}}"#;
        std::fs::write(dir.join("manifest.json"), manifest).unwrap();
        std::fs::write(dir.join("workflow.json"), workflow).unwrap();
        std::fs::create_dir_all(root.join("broken")).unwrap();
        std::fs::write(root.join("broken/manifest.json"), "{").unwrap();
        std::fs::write(root.join("broken/workflow.json"), "{}").unwrap();
        let ctx = Ctx::new(
            reqwest::Client::new(),
            HashMap::from([("workflows_dir".to_string(), root.to_string_lossy().into_owned())]),
        );
        let all = all_templates(&ctx);
        let mine: Vec<&Template> = all.iter().filter(|t| t.id == "esrgan-upscale").collect();
        assert_eq!(mine.len(), 1);
        assert_eq!(mine[0].manifest.name, "My upscaler");
        assert_eq!(all.len(), BUILTIN.len(), "the broken folder is skipped");
        let spec = spec_for(mine[0]);
        assert!(spec.tags.contains(&"user-workflow".to_string()));
        let found = find_template(&ctx, &spec).unwrap();
        assert_eq!(found.manifest.name, "My upscaler");
        let _ = std::fs::remove_dir_all(&root);
    }
}


fn lora_links(graph:&Graph)->Option<(Value,Option<Value>)> {
    // Existing LoRA chains can have architecture-specific routing; leave those workflows explicit.
    if graph.values().any(|n|n.get("class_type").and_then(Value::as_str).is_some_and(|c|c.to_ascii_lowercase().contains("lora"))) {return None;}
    let mut model=None;let mut clip=None;
    for node in graph.values() {
        if let Some(inputs)=node.get("inputs").and_then(Value::as_object) {
            if model.is_none() {model=inputs.get("model").filter(|v|v.is_array()).cloned();}
            if clip.is_none() {clip=inputs.get("clip").filter(|v|v.is_array()).cloned();}
        }
    }
    model.map(|m|(m,clip))
}

fn attach_loras(graph:&mut Graph,req:&GenerationRequest)->ProviderResult<()> {
    let Some(loras)=req.params.get("loras").and_then(Value::as_array).filter(|v|!v.is_empty()) else {return Ok(());};
    let (source_model,source_clip)=lora_links(graph).ok_or_else(||ProviderError::NotSupported("This workflow has no model link for a LoRA.".into()))?;
    let (mut model,mut clip)=(source_model.clone(),source_clip.clone());
    let originals=graph.keys().cloned().collect::<Vec<_>>();
    let mut id=graph.keys().filter_map(|k|k.parse::<u64>().ok()).max().unwrap_or(0)+1;
    for lora in loras {
        while graph.contains_key(&id.to_string()) {id+=1;}
        let key=id.to_string();let mut inputs=json!({"model":model,"lora_name":lora["path"],"strength_model":lora["strength"]});
        let class=if let Some(link)=clip {inputs["clip"]=link;inputs["strength_clip"]=lora["strength"].clone();"LoraLoader"}else{"LoraLoaderModelOnly"};
        graph.insert(key.clone(),json!({"class_type":class,"inputs":inputs}));
        model=json!([key,0]);clip=source_clip.as_ref().map(|_|json!([key,1]));id+=1;
    }
    for key in originals {
        if let Some(inputs)=graph.get_mut(&key).and_then(|n|n.get_mut("inputs")).and_then(Value::as_object_mut) {
            for value in inputs.values_mut() {
                if *value==source_model {*value=model.clone();}
                else if source_clip.as_ref()==Some(value) {if let Some(link)=&clip {*value=link.clone();}}
            }
        }
    }
    Ok(())
}

#[cfg(test)]
mod lora_tests {
    use super::*;
    #[test]
    fn lora_graph_rewires_model_and_clip_without_changing_other_links() {
        let mut graph:Graph=serde_json::from_value(json!({
            "1":{"class_type":"CheckpointLoaderSimple","inputs":{"ckpt_name":"flux.safetensors"}},
            "2":{"class_type":"CLIPTextEncode","inputs":{"text":"maya at a lake","clip":["1",1]}},
            "3":{"class_type":"KSampler","inputs":{"model":["1",0],"positive":["2",0],"seed":42}}
        })).unwrap();
        let req=GenerationRequest{job_id:"job".into(),model:"flux1".into(),prompt:String::new(),negative_prompt:None,params:json!({"loras":[{"path":"pictesa-maya.safetensors","strength":0.7},{"path":"pictesa-style.safetensors","strength":0.5}]}).as_object().unwrap().clone(),inputs:vec![],num_outputs:1};
        attach_loras(&mut graph,&req).unwrap();
        assert_eq!(graph["4"]["inputs"]["model"],json!(["1",0]));assert_eq!(graph["4"]["inputs"]["clip"],json!(["1",1]));
        assert_eq!(graph["5"]["inputs"]["model"],json!(["4",0]));assert_eq!(graph["5"]["inputs"]["clip"],json!(["4",1]));
        assert_eq!(graph["5"]["inputs"]["lora_name"],"pictesa-style.safetensors");assert_eq!(graph["5"]["inputs"]["strength_model"],0.5);
        assert_eq!(graph["2"]["inputs"]["clip"],json!(["5",1]));assert_eq!(graph["3"]["inputs"]["model"],json!(["5",0]));assert_eq!(graph["3"]["inputs"]["positive"],json!(["2",0]));
        assert!(lora_links(&graph).is_none(),"existing LoRA routing is left explicit");
    }
}
