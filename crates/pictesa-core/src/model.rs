use serde::{Deserialize, Serialize};
use serde_json::Value;

/// What a model produces.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MediaKind {
    Image,
    Video,
    Audio,
}

impl MediaKind {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Image => "image",
            Self::Video => "video",
            Self::Audio => "audio",
        }
    }

    pub fn from_mime(mime: &str) -> Option<Self> {
        if mime.starts_with("image/") {
            Some(Self::Image)
        } else if mime.starts_with("video/") {
            Some(Self::Video)
        } else if mime.starts_with("audio/") {
            Some(Self::Audio)
        } else {
            None
        }
    }
}

/// The job a model does. One model can do several (Kling does text and image to video).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Task {
    TextToImage,
    ImageEdit,
    TextToVideo,
    ImageToVideo,
    VideoToVideo,
    Upscale,
    LipSync,
    TextToAudio,
}

/// The role an input file plays in a generation.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum InputRole {
    /// Reference or source images (edit, style, subject or character references).
    Reference,
    /// First frame of a video.
    StartFrame,
    /// Last frame of a video.
    EndFrame,
    /// Source video for video-to-video, extension or upscaling.
    Video,
    /// Audio track (lip sync, music video).
    Audio,
    /// Inpainting mask, white = change.
    Mask,
}

/// One input slot a model accepts, so the UI knows which drop zones to show.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct InputSlot {
    pub role: InputRole,
    pub kind: MediaKind,
    #[serde(default)]
    pub min: u32,
    pub max: u32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub label: Option<String>,
}

impl InputSlot {
    pub fn new(role: InputRole, kind: MediaKind, max: u32) -> Self {
        Self { role, kind, min: 0, max, label: None }
    }
    pub fn required(mut self) -> Self {
        self.min = self.min.max(1);
        self
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EnumOption {
    pub value: Value,
    pub label: String,
}

/// The type and range of a parameter. Rendered as a control by the UI and validated by the engine.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ParamKind {
    Enum { options: Vec<EnumOption> },
    Int { min: i64, max: i64, #[serde(default = "one_i")] step: i64 },
    Float { min: f64, max: f64, #[serde(default = "small_f")] step: f64 },
    Bool,
    Text { #[serde(default)] multiline: bool, #[serde(default)] max_len: Option<u32> },
    Seed,
}

fn one_i() -> i64 {
    1
}
fn small_f() -> f64 {
    0.01
}

/// One generation parameter. `key` uses the shared vocabulary in [`keys`] whenever one fits,
/// so presets, the CLI and agents can set the same option across every model.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ParamSpec {
    pub key: String,
    pub label: String,
    #[serde(flatten)]
    pub kind: ParamKind,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub default: Option<Value>,
    /// Shown behind "More" in the UI.
    #[serde(default)]
    pub advanced: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub help: Option<String>,
}

/// Shared parameter names. Providers translate them to their own field names.
pub mod keys {
    pub const ASPECT_RATIO: &str = "aspect_ratio";
    pub const RESOLUTION: &str = "resolution";
    pub const WIDTH: &str = "width";
    pub const HEIGHT: &str = "height";
    pub const SEED: &str = "seed";
    pub const DURATION: &str = "duration";
    pub const FPS: &str = "fps";
    pub const GUIDANCE: &str = "guidance";
    pub const STEPS: &str = "steps";
    pub const STRENGTH: &str = "strength";
    pub const QUALITY: &str = "quality";
    pub const OUTPUT_FORMAT: &str = "output_format";
    pub const GENERATE_AUDIO: &str = "generate_audio";
    pub const CAMERA_MOTION: &str = "camera_motion";
    pub const ENHANCE_PROMPT: &str = "enhance_prompt";
    pub const NEGATIVE_PROMPT: &str = "negative_prompt";
    pub const STYLE: &str = "style";
    pub const SCALE: &str = "scale";
    pub const LOOP: &str = "loop";
}

impl ParamSpec {
    fn base(key: &str, label: &str, kind: ParamKind) -> Self {
        Self { key: key.into(), label: label.into(), kind, default: None, advanced: false, help: None }
    }
    pub fn enumeration<V: Into<Value> + Clone>(key: &str, label: &str, values: &[(V, &str)]) -> Self {
        let options = values
            .iter()
            .map(|(v, l)| EnumOption { value: v.clone().into(), label: (*l).into() })
            .collect();
        Self::base(key, label, ParamKind::Enum { options })
    }
    /// An enum whose labels equal its string values ("16:9", "1080p", ...).
    pub fn choices(key: &str, label: &str, values: &[&str]) -> Self {
        let pairs: Vec<(&str, &str)> = values.iter().map(|v| (*v, *v)).collect();
        Self::enumeration(key, label, &pairs)
    }
    pub fn int(key: &str, label: &str, min: i64, max: i64) -> Self {
        Self::base(key, label, ParamKind::Int { min, max, step: 1 })
    }
    pub fn float(key: &str, label: &str, min: f64, max: f64, step: f64) -> Self {
        Self::base(key, label, ParamKind::Float { min, max, step })
    }
    pub fn boolean(key: &str, label: &str) -> Self {
        Self::base(key, label, ParamKind::Bool)
    }
    pub fn text(key: &str, label: &str) -> Self {
        Self::base(key, label, ParamKind::Text { multiline: false, max_len: None })
    }
    pub fn seed() -> Self {
        let mut s = Self::base(keys::SEED, "Seed", ParamKind::Seed);
        s.advanced = true;
        s
    }
    pub fn default_value(mut self, v: impl Into<Value>) -> Self {
        self.default = Some(v.into());
        self
    }
    pub fn advanced(mut self) -> Self {
        self.advanced = true;
        self
    }
    pub fn help(mut self, h: &str) -> Self {
        self.help = Some(h.into());
        self
    }

    /// Checks one value against the spec. Returns a readable reason when it does not fit.
    pub fn validate(&self, v: &Value) -> Result<(), String> {
        let name = &self.label;
        match &self.kind {
            ParamKind::Enum { options } => {
                if options.iter().any(|o| &o.value == v || o.value.as_str().is_some_and(|s| v.as_str() == Some(s))) {
                    Ok(())
                } else {
                    let allowed: Vec<String> = options.iter().map(|o| o.value.to_string()).collect();
                    Err(format!("{name} must be one of {}", allowed.join(", ")))
                }
            }
            ParamKind::Int { min, max, .. } => match v.as_i64() {
                Some(n) if n >= *min && n <= *max => Ok(()),
                _ => Err(format!("{name} must be a whole number from {min} to {max}")),
            },
            ParamKind::Float { min, max, .. } => match v.as_f64() {
                Some(n) if n >= *min && n <= *max => Ok(()),
                _ => Err(format!("{name} must be a number from {min} to {max}")),
            },
            ParamKind::Bool => v.as_bool().map(|_| ()).ok_or_else(|| format!("{name} must be true or false")),
            ParamKind::Text { max_len, .. } => match v.as_str() {
                Some(s) if max_len.is_none_or(|m| s.chars().count() as u32 <= m) => Ok(()),
                Some(_) => Err(format!("{name} is too long")),
                None => Err(format!("{name} must be text")),
            },
            ParamKind::Seed => match v.as_u64() {
                Some(n) if n <= u32::MAX as u64 * 2 => Ok(()),
                _ => Err(format!("{name} must be a positive whole number")),
            },
        }
    }
}

/// How a run is priced, when the provider publishes it. Amounts are US dollars.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "unit", rename_all = "snake_case")]
pub enum Pricing {
    PerImage { usd: f64 },
    PerSecond { usd: f64 },
    PerVideo { usd: f64 },
    PerMegapixel { usd: f64 },
    /// Runs on the user's own hardware.
    Local,
    /// The provider bills by tokens or in a way we cannot estimate up front.
    Variable { note: String },
}

/// A model as shown in the picker and exposed to agents.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ModelSpec {
    /// Full id: `provider:model`.
    pub id: String,
    pub provider: String,
    pub name: String,
    /// Maker of the weights (Google, Kuaishou, Black Forest Labs, ...), when different from the provider.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub vendor: Option<String>,
    pub output: MediaKind,
    pub tasks: Vec<Task>,
    #[serde(default)]
    pub inputs: Vec<InputSlot>,
    #[serde(default)]
    pub params: Vec<ParamSpec>,
    /// Whether a text prompt is required (false for upscalers and some edits).
    #[serde(default = "yes")]
    pub prompt: bool,
    #[serde(default = "yes")]
    pub negative_prompt: bool,
    /// How many outputs one request can return. The engine splits larger counts into runs.
    #[serde(default = "one")]
    pub max_outputs: u32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pricing: Option<Pricing>,
    /// Open weights can run locally and qualify for open-weights-only setups.
    #[serde(default)]
    pub open_weights: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub tags: Vec<String>,
    /// False for a discovered model whose params and inputs are not loaded yet;
    /// `GET /api/models/{id}` resolves it.
    #[serde(default = "yes")]
    pub complete: bool,
    /// Provider-private data (field bindings for discovered models, workflow ids, ...).
    #[serde(default, skip_serializing_if = "Value::is_null")]
    pub meta: Value,
}

fn yes() -> bool {
    true
}
fn one() -> u32 {
    1
}

impl ModelSpec {
    pub fn new(provider: &str, model: &str, name: &str, output: MediaKind, tasks: &[Task]) -> Self {
        Self {
            id: format!("{provider}:{model}"),
            provider: provider.into(),
            name: name.into(),
            vendor: None,
            output,
            tasks: tasks.to_vec(),
            inputs: Vec::new(),
            params: Vec::new(),
            prompt: true,
            negative_prompt: false,
            max_outputs: 1,
            pricing: None,
            open_weights: false,
            description: None,
            tags: Vec::new(),
            complete: true,
            meta: Value::Null,
        }
    }
    /// The provider-side model id (the part after `provider:`).
    pub fn remote_id(&self) -> &str {
        self.id.split_once(':').map(|(_, m)| m).unwrap_or(&self.id)
    }
    pub fn vendor(mut self, v: &str) -> Self {
        self.vendor = Some(v.into());
        self
    }
    pub fn input(mut self, slot: InputSlot) -> Self {
        self.inputs.push(slot);
        self
    }
    pub fn param(mut self, p: ParamSpec) -> Self {
        self.params.push(p);
        self
    }
    pub fn params(mut self, ps: impl IntoIterator<Item = ParamSpec>) -> Self {
        self.params.extend(ps);
        self
    }
    pub fn max_outputs(mut self, n: u32) -> Self {
        self.max_outputs = n.max(1);
        self
    }
    pub fn pricing(mut self, p: Pricing) -> Self {
        self.pricing = Some(p);
        self
    }
    pub fn negative(mut self) -> Self {
        self.negative_prompt = true;
        self
    }
    pub fn no_prompt(mut self) -> Self {
        self.prompt = false;
        self
    }
    pub fn open_weights(mut self) -> Self {
        self.open_weights = true;
        self
    }
    pub fn describe(mut self, d: &str) -> Self {
        self.description = Some(d.into());
        self
    }
    pub fn tag(mut self, t: &str) -> Self {
        self.tags.push(t.into());
        self
    }
    pub fn param_spec(&self, key: &str) -> Option<&ParamSpec> {
        self.params.iter().find(|p| p.key == key)
    }
    pub fn accepts(&self, role: InputRole) -> Option<&InputSlot> {
        self.inputs.iter().find(|s| s.role == role)
    }
}

/// Common parameter sets, so provider catalogs stay short and consistent.
pub mod common {
    use super::*;

    pub const IMAGE_RATIOS: &[&str] = &["1:1", "4:5", "3:4", "2:3", "9:16", "16:9", "3:2", "4:3", "5:4", "21:9"];
    pub const VIDEO_RATIOS: &[&str] = &["16:9", "9:16", "1:1"];

    pub fn aspect_ratio(values: &[&str], default: &str) -> ParamSpec {
        ParamSpec::choices(keys::ASPECT_RATIO, "Aspect ratio", values).default_value(default)
    }
    pub fn resolution(values: &[&str], default: &str) -> ParamSpec {
        ParamSpec::choices(keys::RESOLUTION, "Resolution", values).default_value(default)
    }
    pub fn duration(values: &[i64], default: i64) -> ParamSpec {
        let pairs: Vec<(i64, String)> = values.iter().map(|v| (*v, format!("{v}s"))).collect();
        let refs: Vec<(i64, &str)> = pairs.iter().map(|(v, l)| (*v, l.as_str())).collect();
        ParamSpec::enumeration(keys::DURATION, "Duration", &refs).default_value(default)
    }
    pub fn output_format(default: &str) -> ParamSpec {
        ParamSpec::choices(keys::OUTPUT_FORMAT, "Format", &["png", "jpeg", "webp"]).default_value(default).advanced()
    }
    pub fn generate_audio(default: bool) -> ParamSpec {
        ParamSpec::boolean(keys::GENERATE_AUDIO, "Sound").default_value(default)
    }
    pub fn steps(min: i64, max: i64, default: i64) -> ParamSpec {
        ParamSpec::int(keys::STEPS, "Steps", min, max).default_value(default).advanced()
    }
    pub fn guidance(min: f64, max: f64, default: f64) -> ParamSpec {
        ParamSpec::float(keys::GUIDANCE, "Guidance", min, max, 0.1).default_value(default).advanced()
    }
}
