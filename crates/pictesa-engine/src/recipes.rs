//! Recipes: open, editable multi-step pipelines. The studio shows them as Apps.
//!
//! A recipe declares the fields a person fills in, optional input files, and steps. Each step
//! names the models it prefers (the first one that is set up and fits wins, cloud or local), a
//! prompt template (`{{field}}`, with `{{#key}}...{{/key}}` sections), parameters, a preset, and
//! where its input files come from: the person's uploads (`input:<key>`) or an earlier step's
//! result (`step:<id>`). A step with `each` runs once per item and returns a set.

use std::{
    collections::{HashMap, HashSet},
    path::PathBuf,
    time::Duration,
};

use pictesa_core::{now_ms, InputRole, MediaKind, ModelSpec};
use serde::{Deserialize, Serialize};
use serde_json::{json, Map, Value};

use crate::{characters::Character, CreateGeneration, Engine, EngineError, JobInput, JobStatus, Result};

const BUILTIN: &str = include_str!("../../../recipes/recipes.json");
/// Most runs one `each` step may start, so a long list cannot run up a bill by accident.
pub const MAX_ITEMS: usize = 16;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RecipeField {
    pub key: String,
    pub label: String,
    /// `text`, `enum`, `multi` (several of `options`) or `preset`.
    #[serde(rename = "type")]
    pub kind: String,
    #[serde(default)]
    pub options: Vec<String>,
    /// For `preset` fields: which preset category to offer (`camera`, `style`, `grade`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub category: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub default: Option<Value>,
    #[serde(default)]
    pub required: bool,
    /// Required unless this input or field has a value ("describe the scene, or add a scene photo").
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub required_without: Option<String>,
    #[serde(default)]
    pub multiline: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub help: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RecipeInput {
    pub key: String,
    pub label: String,
    pub kind: MediaKind,
    #[serde(default = "one")]
    pub max: u32,
    #[serde(default)]
    pub required: bool,
}

fn one() -> u32 {
    1
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StepInput {
    pub role: InputRole,
    /// `input:<key>` or `step:<id>`.
    pub from: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RecipeStep {
    pub id: String,
    pub label: String,
    pub output: MediaKind,
    /// Preferred model ids, best first. The first configured one that fits runs.
    pub models: Vec<String>,
    #[serde(default)]
    pub prompt: String,
    #[serde(default)]
    pub params: Map<String, Value>,
    /// A preset id or a `{{field}}` that names one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub preset: Option<String>,
    #[serde(default)]
    pub inputs: Vec<StepInput>,
    /// Run the step once per item; `{{each}}` is the item. A lone `{{field}}` entry stands for all
    /// of the field's items: the choices of a `multi` field or the lines of a text field.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub each: Vec<String>,
    /// What to set up when none of `models` is ready, in words people and agents can act on.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub needs: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Recipe {
    pub id: String,
    pub name: String,
    pub description: String,
    pub output: MediaKind,
    /// The card picture in the studio: a built-in illustration name or a library image id.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub picture: Option<String>,
    /// Offer a character or brand kit that applies to every step.
    #[serde(default)]
    pub character: bool,
    /// A studio page that runs this recipe with a richer tool (`/edit?tool=relight`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub open: Option<String>,
    #[serde(default)]
    pub fields: Vec<RecipeField>,
    #[serde(default)]
    pub inputs: Vec<RecipeInput>,
    pub steps: Vec<RecipeStep>,
}

/// A recipe as listed for a user: which model each step would use right now.
#[derive(Debug, Clone, Serialize)]
pub struct RecipeView {
    #[serde(flatten)]
    pub recipe: Recipe,
    pub step_models: Vec<Option<String>>,
    pub runnable: bool,
}

#[derive(Debug, Clone, Default, Deserialize, Serialize)]
pub struct RunRecipe {
    #[serde(default)]
    pub values: Map<String, Value>,
    /// Uploaded inputs by recipe input key: asset ids.
    #[serde(default)]
    pub inputs: HashMap<String, Vec<String>>,
    /// Model override per step id.
    #[serde(default)]
    pub models: HashMap<String, String>,
    /// A character or brand kit applied to every step: its images join as references where the
    /// model takes them, and a brand kit's colors and words join every prompt.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub character: Option<String>,
}

#[derive(Debug, Clone, Serialize, sqlx::FromRow)]
pub struct RecipeRun {
    pub id: String,
    pub recipe_id: String,
    pub recipe_name: String,
    pub status: String,
    pub step: i64,
    pub steps: i64,
    /// Every job of the run, in order.
    #[sqlx(skip)]
    pub jobs: Vec<String>,
    /// Jobs per step. A step that runs `each` has one job per item.
    #[sqlx(skip)]
    pub step_jobs: Vec<Vec<String>>,
    /// The final step's jobs: their outputs are the run's results (a set when it ran `each`).
    #[sqlx(skip)]
    pub results: Vec<String>,
    #[serde(skip)]
    pub job_ids: String,
    pub error: Option<String>,
    pub created_at: i64,
    pub updated_at: i64,
}

impl RecipeRun {
    fn finish(mut self) -> Self {
        // Runs from before sets stored one job per step as a flat list.
        self.step_jobs = serde_json::from_str::<Vec<Vec<String>>>(&self.job_ids)
            .or_else(|_| serde_json::from_str::<Vec<String>>(&self.job_ids).map(|flat| flat.into_iter().map(|j| vec![j]).collect()))
            .unwrap_or_default();
        self.jobs = self.step_jobs.iter().flatten().cloned().collect();
        self.results = if self.step_jobs.len() as i64 == self.steps { self.step_jobs.last().cloned().unwrap_or_default() } else { Vec::new() };
        self
    }
}

pub fn load(dirs: &[PathBuf]) -> Vec<Recipe> {
    crate::load_json(BUILTIN, dirs, "recipes", |r: &Recipe| r.id.clone(), |r, source| match check(r) {
        Ok(()) => true,
        Err(why) => {
            tracing::warn!("skipping recipe {} in {source}: {why}", r.id);
            false
        }
    })
}

/// Mistakes that would only show half way through a run: unknown sources, clashing keys, a
/// default that is not an option.
pub fn check(r: &Recipe) -> std::result::Result<(), String> {
    let mut keys = HashSet::new();
    for f in &r.fields {
        if !keys.insert(f.key.as_str()) {
            return Err(format!("the key {} is used twice", f.key));
        }
        match f.kind.as_str() {
            "text" | "preset" => {}
            "enum" | "multi" if f.options.is_empty() => return Err(format!("{} has no options", f.key)),
            "enum" | "multi" => {
                let defaults: Vec<&Value> = match &f.default {
                    Some(Value::Array(list)) => list.iter().collect(),
                    Some(other) => vec![other],
                    None => Vec::new(),
                };
                if let Some(bad) = defaults.iter().find(|d| !d.as_str().is_some_and(|d| f.options.iter().any(|o| o == d))) {
                    return Err(format!("the default {bad} of {} is not one of its options", f.key));
                }
            }
            other => return Err(format!("{} has the unknown type {other}", f.key)),
        }
    }
    for i in &r.inputs {
        if !keys.insert(i.key.as_str()) {
            return Err(format!("the key {} is used twice", i.key));
        }
    }
    if keys.contains("each") {
        return Err("each is reserved for the item of a step that runs each".into());
    }
    if let Some(f) = r.fields.iter().find(|f| f.required_without.as_deref().is_some_and(|k| !keys.contains(k))) {
        return Err(format!("{} is required without an unknown key", f.key));
    }
    if r.steps.is_empty() {
        return Err("a recipe needs at least one step".into());
    }
    let mut earlier = HashSet::new();
    for s in &r.steps {
        if s.models.is_empty() {
            return Err(format!("the step {} lists no models", s.id));
        }
        for si in &s.inputs {
            let known = match (si.from.strip_prefix("input:"), si.from.strip_prefix("step:")) {
                (Some(key), _) => r.inputs.iter().any(|i| i.key == key),
                (_, Some(id)) => earlier.contains(id),
                _ => false,
            };
            if !known {
                return Err(format!("the step {} takes files from {}, which is not an input or an earlier step", s.id, si.from));
            }
        }
        if let Some(entry) = s.each.iter().find(|e| lone_key(e).is_some_and(|k| !r.fields.iter().any(|f| f.key == k))) {
            return Err(format!("the step {} runs each {entry}, which is not a field", s.id));
        }
        if !earlier.insert(s.id.as_str()) {
            return Err(format!("the step id {} is used twice", s.id));
        }
    }
    Ok(())
}

/// Whether a value counts as filled in: not null, false, blank text or an empty list.
fn filled(v: Option<&Value>) -> bool {
    match v {
        None | Some(Value::Null) | Some(Value::Bool(false)) => false,
        Some(Value::String(s)) => !s.trim().is_empty(),
        Some(Value::Array(a)) => !a.is_empty(),
        Some(_) => true,
    }
}

/// The key of a template that is exactly one `{{key}}`.
fn lone_key(s: &str) -> Option<&str> {
    let inner = s.trim().strip_prefix("{{")?.strip_suffix("}}")?;
    let key = inner.trim();
    (!inner.contains("{{") && !inner.contains("}}") && !key.starts_with(['#', '^', '/'])).then_some(key)
}

/// Fills a template: `{{key}}` becomes its value (lists join with commas), `{{#key}}...{{/key}}`
/// stays only when the key is filled in and `{{^key}}...{{/key}}` only when it is not. Input keys
/// count as filled in when files were added. Unknown keys become empty.
pub fn expand(template: &str, values: &Map<String, Value>) -> String {
    let mut out = String::with_capacity(template.len());
    let mut rest = template;
    while let Some(start) = rest.find("{{") {
        out.push_str(&rest[..start]);
        let Some(end) = rest[start..].find("}}") else {
            out.push_str(&rest[start..]);
            return out;
        };
        let tag = rest[start + 2..start + end].trim();
        rest = &rest[start + end + 2..];
        if let Some(key) = tag.strip_prefix('#').or_else(|| tag.strip_prefix('^')) {
            let close = format!("{{{{/{}}}}}", key.trim());
            let (body, after) = rest.find(&close).map_or((rest, ""), |i| (&rest[..i], &rest[i + close.len()..]));
            if filled(values.get(key.trim())) == tag.starts_with('#') {
                out.push_str(&expand(body, values));
            }
            rest = after;
            continue;
        }
        match values.get(tag) {
            Some(Value::String(s)) => out.push_str(s),
            Some(Value::Array(items)) => out.push_str(&items.iter().map(|v| v.as_str().map(String::from).unwrap_or_else(|| v.to_string())).collect::<Vec<_>>().join(", ")),
            Some(Value::Null) | Some(Value::Bool(_)) | None => {}
            Some(other) => out.push_str(&other.to_string()),
        }
    }
    out.push_str(rest);
    out
}

/// A prompt from a template, with the leftovers of empty optional fields (", ,") tidied away.
fn render(template: &str, values: &Map<String, Value>) -> String {
    let filled = expand(template, values);
    let mut tidy = filled.split(',').map(str::trim).filter(|p| !p.is_empty()).collect::<Vec<_>>().join(", ");
    while tidy.contains("  ") {
        tidy = tidy.replace("  ", " ");
    }
    tidy
}

/// Renders a parameter: a lone `{{key}}` keeps the field's type (numbers stay numbers).
fn render_value(v: &Value, values: &Map<String, Value>) -> Option<Value> {
    match v {
        Value::String(s) => match lone_key(s) {
            Some(key) => values.get(key).cloned().filter(|v| !v.is_null() && v.as_str() != Some("")),
            None => Some(Value::String(render(s, values))),
        },
        other => Some(other.clone()),
    }
}

/// A line without a list marker ("- ", "2. ", "3) ").
fn strip_marker(line: &str) -> &str {
    let line = line.trim();
    if let Some(rest) = line.strip_prefix(['-', '*', '•']) {
        return rest.trim_start();
    }
    let digits = line.chars().take_while(char::is_ascii_digit).count();
    match line[digits..].strip_prefix(['.', ')']) {
        Some(rest) if digits > 0 && rest.starts_with(' ') => rest.trim_start(),
        _ => line,
    }
}

/// The items a step runs for. A step without `each` has none (it runs once).
pub fn each_items(step: &RecipeStep, values: &Map<String, Value>) -> Vec<String> {
    let mut items = Vec::new();
    for entry in &step.each {
        match lone_key(entry) {
            Some(key) => match values.get(key) {
                Some(Value::Array(list)) => items.extend(list.iter().filter_map(Value::as_str).map(str::to_string)),
                Some(Value::String(text)) => items.extend(text.lines().map(|line| strip_marker(line).to_string())),
                Some(Value::Null) | None => {}
                Some(other) => items.push(other.to_string()),
            },
            None => items.push(render(entry, values)),
        }
    }
    // Items are phrases inside a sentence: no end stops, no blanks.
    items.into_iter().map(|item| item.trim().trim_end_matches(['.', ' ']).to_string()).filter(|item| !item.is_empty()).collect()
}

/// The files of one role and kind a step hands its model, per run of the step.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Need {
    pub role: InputRole,
    pub kind: MediaKind,
    pub count: u32,
}

/// What a step passes to its model. `files` counts the person's files for an input.
pub fn step_needs(recipe: &Recipe, step: &RecipeStep, files: impl Fn(&RecipeInput) -> u32) -> Vec<Need> {
    let mut needs: Vec<Need> = Vec::new();
    for si in &step.inputs {
        let source = if let Some(key) = si.from.strip_prefix("input:") {
            recipe.inputs.iter().find(|i| i.key == key).map(|i| (i.kind, files(i)))
        } else {
            si.from.strip_prefix("step:").and_then(|id| recipe.steps.iter().find(|s| s.id == id)).map(|s| (s.output, 1))
        };
        let Some((kind, count)) = source else { continue };
        match needs.iter_mut().find(|n| n.role == si.role && n.kind == kind) {
            Some(n) => n.count += count,
            None => needs.push(Need { role: si.role, kind, count }),
        }
    }
    needs
}

/// What a step needs at least: one file per required input and per earlier step.
pub fn listed_needs(recipe: &Recipe, step: &RecipeStep) -> Vec<Need> {
    step_needs(recipe, step, |input| u32::from(input.required))
}

fn kind_plural(kind: MediaKind) -> &'static str {
    match kind {
        MediaKind::Image => "images",
        MediaKind::Video => "videos",
        MediaKind::Audio => "audio",
    }
}

fn role_name(role: InputRole) -> &'static str {
    match role {
        InputRole::Reference => "reference",
        InputRole::StartFrame => "start frame",
        InputRole::EndFrame => "end frame",
        InputRole::Video => "video",
        InputRole::Audio => "audio",
        InputRole::Mask => "mask",
    }
}

/// Whether a model can run a step: it makes the step's kind of media, takes a prompt when the step
/// writes one, and has room for the step's files. Gateway models whose details are not loaded yet
/// count as fitting: the recipe lists them on purpose, and the full check runs before a run starts.
pub fn fits(spec: &ModelSpec, step: &RecipeStep, needs: &[Need]) -> std::result::Result<(), String> {
    if spec.output != step.output {
        return Err(format!("{} makes {}, and this step makes {}.", spec.name, kind_plural(spec.output), kind_plural(step.output)));
    }
    if !spec.complete {
        return Ok(());
    }
    if !spec.prompt && !step.prompt.trim().is_empty() {
        return Err(format!("{} takes no written instructions.", spec.name));
    }
    for need in needs.iter().filter(|n| n.count > 0) {
        let room = spec.inputs.iter().filter(|s| s.role == need.role && s.kind == need.kind).map(|s| s.max).max().unwrap_or(0);
        if room < need.count {
            let what = format!("{} {}", role_name(need.role), kind_plural(need.kind));
            return Err(if room == 0 { format!("{} takes no {what}.", spec.name) } else { format!("{} takes {room} {what}, and this step passes {}.", spec.name, need.count) });
        }
    }
    Ok(())
}

/// The first listed model that is set up and fits, from the models as listed.
pub fn pick_model(step: &RecipeStep, ready: &HashMap<String, ModelSpec>, needs: &[Need]) -> Option<String> {
    step.models.iter().find(|id| ready.get(*id).is_some_and(|spec| fits(spec, step, needs).is_ok())).cloned()
}

/// What to set up when no listed model can run a step.
pub fn missing_text(step: &RecipeStep) -> String {
    match &step.needs {
        Some(needs) => format!("No model for \"{}\" is set up. It needs {needs}. Add one in Settings > Providers.", step.label),
        None => format!("No model for \"{}\" is set up. Add a key for one of: {}.", step.label, step.models.join(", ")),
    }
}

/// Checks the person's values against the fields and fills in defaults. `has_input` tells whether
/// files were added for an input key, `preset_known` whether a preset id exists.
pub fn settle_values(recipe: &Recipe, values: &mut Map<String, Value>, has_input: &dyn Fn(&str) -> bool, preset_known: &dyn Fn(&str) -> bool) -> Result<()> {
    for f in &recipe.fields {
        let field = format!("values.{}", f.key);
        let invalid = |message: String| EngineError::invalid(message, Some(&field));
        if !filled(values.get(&f.key)) {
            if let Some(d) = &f.default {
                values.insert(f.key.clone(), d.clone());
            } else {
                let excused = f.required_without.as_deref().is_some_and(|k| has_input(k) || filled(values.get(k)));
                if f.required || (f.required_without.is_some() && !excused) {
                    return Err(invalid(match (&f.required_without, &f.help) {
                        (Some(_), Some(help)) => help.clone(),
                        _ => format!("Fill in {}.", f.label),
                    }));
                }
                values.remove(&f.key);
                continue;
            }
        }
        match f.kind.as_str() {
            "enum" => {
                if let Some(v) = values.get(&f.key).and_then(Value::as_str) {
                    if !f.options.iter().any(|o| o == v) {
                        return Err(invalid(format!("{} must be one of {}.", f.label, f.options.join(", "))));
                    }
                }
            }
            "multi" => {
                let picked: Vec<String> = match values.get(&f.key) {
                    Some(Value::String(one)) => vec![one.clone()],
                    Some(Value::Array(list)) => list.iter().map(|v| v.as_str().map(String::from)).collect::<Option<_>>().ok_or_else(|| invalid(format!("{} takes a list of options.", f.label)))?,
                    _ => return Err(invalid(format!("{} takes a list of options.", f.label))),
                };
                if let Some(bad) = picked.iter().find(|v| !f.options.contains(v)) {
                    return Err(invalid(format!("{bad} is not an option of {}. Pick from: {}.", f.label, f.options.join(", "))));
                }
                let mut unique: Vec<String> = Vec::new();
                for v in picked {
                    if !unique.contains(&v) {
                        unique.push(v);
                    }
                }
                values.insert(f.key.clone(), json!(unique));
            }
            "preset" => {
                if let Some(v) = values.get(&f.key).and_then(Value::as_str).filter(|v| !v.is_empty()) {
                    if !preset_known(v) {
                        return Err(invalid(format!("{v} is not a preset. Pick one from the {} presets.", f.category.as_deref().unwrap_or("listed"))));
                    }
                }
            }
            _ => {}
        }
    }
    Ok(())
}

/// The files item `k` of a step gets. After another step that ran `each`, items pair up by
/// position; None when the paired item failed.
fn step_inputs(step: &RecipeStep, req: &RunRecipe, outputs: &HashMap<String, Vec<Option<String>>>, k: usize, items: usize) -> Option<Vec<JobInput>> {
    let mut inputs = Vec::new();
    for si in &step.inputs {
        if let Some(key) = si.from.strip_prefix("input:") {
            for asset in req.inputs.get(key).into_iter().flatten() {
                inputs.push(JobInput { asset_id: asset.clone(), role: si.role });
            }
        } else if let Some(prev) = si.from.strip_prefix("step:") {
            let outs = outputs.get(prev)?;
            let asset = if items > 1 && outs.len() > 1 { outs.get(k)?.clone()? } else { outs.iter().flatten().next()?.clone() };
            inputs.push(JobInput { asset_id: asset, role: si.role });
        }
    }
    Some(inputs)
}

impl Engine {
    pub fn recipe(&self, id: &str) -> Option<Recipe> {
        self.inner.recipes.iter().find(|r| r.id == id).cloned()
    }

    /// Set-up models by id, as listed (gateway models may not have their details loaded yet).
    async fn ready_models(&self, user: &str) -> Result<HashMap<String, ModelSpec>> {
        let all = self.models(user, &crate::ModelFilter { configured: Some(true), ..Default::default() }).await?;
        Ok(all.into_iter().map(|m| (m.spec.id.clone(), m.spec)).collect())
    }

    pub async fn recipes(&self, user: &str) -> Result<Vec<RecipeView>> {
        let ready = self.ready_models(user).await?;
        Ok(self
            .inner
            .recipes
            .iter()
            .map(|r| {
                let step_models: Vec<Option<String>> = r.steps.iter().map(|s| pick_model(s, &ready, &listed_needs(r, s))).collect();
                let runnable = step_models.iter().all(Option::is_some);
                RecipeView { recipe: r.clone(), step_models, runnable }
            })
            .collect())
    }

    /// The first set-up model of a step that fits once its details are loaded.
    async fn first_fitting(&self, user: &str, step: &RecipeStep, needs: &[Need], ready: &HashMap<String, ModelSpec>) -> Result<Option<ModelSpec>> {
        let mut trouble = None;
        for id in step.models.iter().filter(|id| ready.contains_key(*id)) {
            match self.model(user, id).await {
                Ok(spec) if fits(&spec, step, needs).is_ok() => return Ok(Some(spec)),
                Ok(_) => {}
                Err(e) => trouble = trouble.or(Some(e)),
            }
        }
        trouble.map_or(Ok(None), Err)
    }

    /// Validates the request, records the run and starts it in the background.
    pub async fn run_recipe(&self, user: &str, recipe_id: &str, mut req: RunRecipe) -> Result<RecipeRun> {
        let recipe = self.recipe(recipe_id).ok_or_else(|| EngineError::not_found("recipe"))?;
        let added = |key: &str| req.inputs.get(key).is_some_and(|files| !files.is_empty());
        settle_values(&recipe, &mut req.values, &added, &|id| self.preset(id).is_some())?;
        for input in &recipe.inputs {
            let n = req.inputs.get(&input.key).map(Vec::len).unwrap_or(0);
            if input.required && n == 0 {
                return Err(EngineError::invalid(format!("Add {}.", input.label), Some(&format!("inputs.{}", input.key))));
            }
            if n as u32 > input.max {
                return Err(EngineError::invalid(format!("{} takes at most {} file(s).", input.label, input.max), Some(&format!("inputs.{}", input.key))));
            }
        }
        for step in recipe.steps.iter().filter(|s| !s.each.is_empty()) {
            let n = each_items(step, &req.values).len();
            if n == 0 || n > MAX_ITEMS {
                let field = step.each.iter().find_map(|e| lone_key(e)).and_then(|k| recipe.fields.iter().find(|f| f.key == k));
                let what = field.map_or_else(|| "its list".to_string(), |f| f.label.to_lowercase());
                let message = if n == 0 { format!("\"{}\" has nothing to run. Fill in {what}.", step.label) } else { format!("\"{}\" runs at most {MAX_ITEMS} times. Shorten {what}.", step.label) };
                return Err(EngineError::invalid(message, field.map(|f| format!("values.{}", f.key)).as_deref()));
            }
        }
        // A character or brand kit by id or by name; runs record its id.
        req.character = match req.character.take().filter(|c| !c.trim().is_empty()) {
            Some(c) => Some(self.find_character(user, &c).await.map_err(|_| EngineError::invalid(format!("No character or brand kit is called {c}."), Some("character")))?.id),
            None => None,
        };
        let ready = self.ready_models(user).await?;
        let mut chosen = Vec::new();
        for step in &recipe.steps {
            let needs = step_needs(&recipe, step, |input| req.inputs.get(&input.key).map_or(0, |files| files.len() as u32));
            let field = format!("models.{}", step.id);
            let model = match req.models.get(&step.id) {
                Some(m) if ready.contains_key(m) => {
                    let spec = self.model(user, m).await?;
                    fits(&spec, step, &needs).map_err(|why| EngineError::invalid(format!("{why} Pick another model for \"{}\".", step.label), Some(&field)))?;
                    spec
                }
                Some(m) => return Err(EngineError::invalid(format!("{m} is not set up."), Some(&field))),
                None => self.first_fitting(user, step, &needs, &ready).await?.ok_or_else(|| EngineError::invalid(missing_text(step), Some("models")))?,
            };
            chosen.push(model);
        }
        let id = crate::new_id();
        let now = now_ms();
        sqlx::query("INSERT INTO recipe_runs (id, user_id, recipe_id, recipe_name, status, step, steps, request, created_at, updated_at) VALUES (?, ?, ?, ?, 'running', 0, ?, ?, ?, ?)")
            .bind(&id)
            .bind(user)
            .bind(&recipe.id)
            .bind(&recipe.name)
            .bind(recipe.steps.len() as i64)
            .bind(serde_json::to_string(&req).map_err(anyhow::Error::from)?)
            .bind(now)
            .bind(now)
            .execute(&self.inner.db)
            .await?;
        let engine = self.clone();
        let (user_owned, run_id) = (user.to_string(), id.clone());
        tokio::spawn(async move {
            if let Err(e) = engine.drive_recipe(&user_owned, &run_id, &recipe, &req, &chosen).await {
                engine.end_run(&user_owned, &run_id, "failed", Some(&e.to_string())).await;
            }
        });
        self.recipe_run(user, &id).await
    }

    /// One generation of a step, for one item.
    fn step_request(step: &RecipeStep, spec: &ModelSpec, values: &Map<String, Value>, inputs: Vec<JobInput>, character: Option<&Character>) -> CreateGeneration {
        let mut params = Map::new();
        for (k, v) in &step.params {
            if let Some(v) = render_value(v, values) {
                params.insert(k.clone(), v);
            }
        }
        // Recipes stay portable: drop params the model lacks and snap values to what it accepts.
        if !spec.params.is_empty() {
            params = params
                .into_iter()
                .filter_map(|(k, v)| {
                    let p = spec.param_spec(&k)?;
                    fit_param(p, &v).map(|v| (k, v))
                })
                .collect();
        }
        let preset = step.preset.as_ref().and_then(|p| render_value(&json!(p), values)).and_then(|v| v.as_str().map(String::from)).filter(|p| !p.is_empty());
        // A person's images help only where the model takes references; a brand kit's words help everywhere.
        let takes_images = spec.inputs.iter().any(|s| s.role == InputRole::Reference && s.kind == MediaKind::Image);
        let character = character.filter(|c| takes_images || c.is_brand()).map(|c| c.id.clone());
        CreateGeneration { model: spec.id.clone(), prompt: render(&step.prompt, values), negative_prompt: None, params, inputs, count: Some(1), preset, compare: Vec::new(), character }
    }

    async fn drive_recipe(&self, user: &str, run_id: &str, recipe: &Recipe, req: &RunRecipe, models: &[ModelSpec]) -> Result<()> {
        let mut values = req.values.clone();
        // Inputs count in template sections: {{#photo}}...{{/photo}} shows when photos were added.
        for input in &recipe.inputs {
            if req.inputs.get(&input.key).is_some_and(|files| !files.is_empty()) {
                values.insert(input.key.clone(), Value::Bool(true));
            }
        }
        let character = match &req.character {
            Some(id) => Some(self.character(user, id).await?),
            None => None,
        };
        // The first output of each item of each finished step.
        let mut outputs: HashMap<String, Vec<Option<String>>> = HashMap::new();
        let mut step_jobs: Vec<Vec<String>> = Vec::new();
        for (i, step) in recipe.steps.iter().enumerate() {
            // Stop when the run was canceled, or marked failed by another process.
            if self.run_status(run_id).await.as_deref() != Some("running") {
                return Ok(());
            }
            let spec = &models[i];
            let items: Vec<Option<String>> = if step.each.is_empty() { vec![None] } else { each_items(step, &values).into_iter().map(Some).collect() };
            let (mut requests, mut slots) = (Vec::new(), Vec::new());
            for (k, item) in items.iter().enumerate() {
                let Some(inputs) = step_inputs(step, req, &outputs, k, items.len()) else { continue };
                let mut item_values = values.clone();
                if let Some(item) = item {
                    item_values.insert("each".into(), json!(item));
                }
                requests.push(Self::step_request(step, spec, &item_values, inputs, character.as_ref()));
                slots.push(k);
            }
            if requests.is_empty() {
                self.end_run(user, run_id, "failed", Some(&format!("{}: nothing to run, the step before it failed.", step.label))).await;
                return Ok(());
            }
            // One batch per step: the whole set passes the budget check together, or not at all.
            let created = self.create_set(user, requests).await?;
            let ids: Vec<String> = created.jobs.iter().map(|j| j.id.clone()).collect();
            step_jobs.push(ids.clone());
            sqlx::query("UPDATE recipe_runs SET step = ?, job_ids = ?, updated_at = ? WHERE id = ?")
                .bind(i as i64)
                .bind(serde_json::to_string(&step_jobs).unwrap_or_default())
                .bind(now_ms())
                .bind(run_id)
                .execute(&self.inner.db)
                .await?;
            self.publish_run(user, run_id).await;
            // Heartbeat while the step runs, so other processes can tell a live run from an orphan.
            let beat = {
                let (engine, run) = (self.clone(), run_id.to_string());
                tokio::spawn(async move {
                    loop {
                        tokio::time::sleep(Duration::from_secs(15)).await;
                        let _ = sqlx::query("UPDATE recipe_runs SET updated_at = ? WHERE id = ? AND status = 'running'").bind(now_ms()).bind(&run).execute(&engine.inner.db).await;
                    }
                })
            };
            let mut finished = Vec::new();
            for id in &ids {
                finished.push(self.wait_job(user, id, Duration::from_secs(3 * 3600)).await);
            }
            beat.abort();
            let mut outs = vec![None; items.len()];
            let mut failure = None;
            for (job, k) in finished.into_iter().zip(slots) {
                let job = job?;
                match job.status {
                    JobStatus::Succeeded => outs[k] = job.outputs.first().map(|a| a.id.clone()),
                    JobStatus::Canceled if self.run_status(run_id).await.as_deref() != Some("running") => return Ok(()),
                    _ => failure = failure.or(Some(job.error.map(|e| e.message).unwrap_or_else(|| "it did not finish".into()))),
                }
            }
            // A set keeps the items that worked; the run fails only when none did.
            let Some(first) = outs.iter().flatten().next().cloned() else {
                let reason = failure.unwrap_or_else(|| "it returned no files".into());
                self.end_run(user, run_id, "failed", Some(&format!("{}: {reason}", step.label))).await;
                return Ok(());
            };
            values.insert(format!("{}_asset", step.id), json!(first));
            outputs.insert(step.id.clone(), outs);
        }
        self.end_run(user, run_id, "succeeded", None).await;
        Ok(())
    }

    async fn run_status(&self, run_id: &str) -> Option<String> {
        sqlx::query_as::<_, (String,)>("SELECT status FROM recipe_runs WHERE id = ?").bind(run_id).fetch_optional(&self.inner.db).await.ok().flatten().map(|r| r.0)
    }

    async fn end_run(&self, user: &str, run_id: &str, status: &str, error: Option<&str>) {
        let _ = sqlx::query("UPDATE recipe_runs SET status = ?, error = ?, updated_at = ? WHERE id = ? AND status = 'running'")
            .bind(status)
            .bind(error)
            .bind(now_ms())
            .bind(run_id)
            .execute(&self.inner.db)
            .await;
        self.publish_run(user, run_id).await;
    }

    async fn publish_run(&self, user: &str, run_id: &str) {
        if let Ok(run) = self.recipe_run(user, run_id).await {
            self.emit(user, "recipe", json!(run));
        }
    }

    pub async fn recipe_run(&self, user: &str, id: &str) -> Result<RecipeRun> {
        let row: RecipeRun = sqlx::query_as("SELECT id, recipe_id, recipe_name, status, step, steps, job_ids, error, created_at, updated_at FROM recipe_runs WHERE id = ? AND user_id = ?")
            .bind(id)
            .bind(user)
            .fetch_one(&self.inner.db)
            .await
            .map_err(|_| EngineError::not_found("recipe run"))?;
        Ok(row.finish())
    }

    pub async fn recipe_runs(&self, user: &str, limit: u32) -> Result<Vec<RecipeRun>> {
        let rows: Vec<RecipeRun> = sqlx::query_as("SELECT id, recipe_id, recipe_name, status, step, steps, job_ids, error, created_at, updated_at FROM recipe_runs WHERE user_id = ? ORDER BY id DESC LIMIT ?")
            .bind(user)
            .bind(limit.clamp(1, 200) as i64)
            .fetch_all(&self.inner.db)
            .await?;
        Ok(rows.into_iter().map(RecipeRun::finish).collect())
    }

    /// Stops a run: cancels the jobs of its current step and skips the remaining steps.
    pub async fn cancel_recipe_run(&self, user: &str, id: &str) -> Result<RecipeRun> {
        let run = self.recipe_run(user, id).await?;
        if run.status == "running" {
            self.end_run(user, id, "canceled", None).await;
            for job in run.step_jobs.last().into_iter().flatten() {
                let _ = self.cancel(user, job).await;
            }
        }
        self.recipe_run(user, id).await
    }

    /// Runs whose process died cannot resume their orchestration; mark them failed once their
    /// heartbeat is clearly stale (a live run updates it every 15 seconds).
    pub(crate) async fn fail_orphan_runs(&self) {
        let _ = sqlx::query("UPDATE recipe_runs SET status = 'failed', error = 'Pictesa restarted during this run. Finished steps are in the library.', updated_at = ? WHERE status = 'running' AND updated_at < ?")
            .bind(now_ms())
            .bind(now_ms() - 120_000)
            .execute(&self.inner.db)
            .await;
    }
}

/// Returns `v` if the parameter accepts it, else the closest accepted value, else None
/// (the model's default then applies).
pub fn fit_param(p: &pictesa_core::ParamSpec, v: &Value) -> Option<Value> {
    use pictesa_core::ParamKind::*;
    if p.validate(v).is_ok() {
        return Some(v.clone());
    }
    let num = v.as_f64().or_else(|| v.as_str().and_then(|s| s.trim().trim_end_matches('s').parse::<f64>().ok()));
    match &p.kind {
        Enum { options } => {
            let text = v.as_str().map(str::to_string).unwrap_or_else(|| v.to_string());
            if let Some(o) = options.iter().find(|o| o.value.as_str().map(|s| s.eq_ignore_ascii_case(&text)).unwrap_or(o.value.to_string() == text)) {
                return Some(o.value.clone());
            }
            let n = num?;
            options
                .iter()
                .filter_map(|o| {
                    let ov = o.value.as_f64().or_else(|| o.value.as_str().and_then(|s| s.trim().trim_end_matches('s').parse::<f64>().ok()))?;
                    Some(((ov - n).abs(), o.value.clone()))
                })
                .min_by(|a, b| a.0.partial_cmp(&b.0).unwrap_or(std::cmp::Ordering::Equal))
                .map(|(_, v)| v)
        }
        Int { min, max, .. } => num.map(|n| Value::from((n.round() as i64).clamp(*min, *max))),
        Float { min, max, .. } => num.map(|n| Value::from(n.clamp(*min, *max))),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use pictesa_core::{InputSlot, Provider, Task};

    const NEW_APPS: [&str; 10] = ["photo/shots", "photo/angles", "photo/zooms", "photo/relight", "photo/face-swap", "product/placement", "photo/color-grade", "ads/ugc", "ads/hook-variants", "video/explainer"];

    fn builtin() -> Vec<Recipe> {
        serde_json::from_str(BUILTIN).expect("recipes.json must parse")
    }

    fn get(id: &str) -> Recipe {
        builtin().into_iter().find(|r| r.id == id).unwrap_or_else(|| panic!("{id} is a built-in recipe"))
    }

    fn values(pairs: &[(&str, Value)]) -> Map<String, Value> {
        pairs.iter().map(|(k, v)| (k.to_string(), v.clone())).collect()
    }

    /// The free mock models, as a person with no keys has them.
    fn mock_ready() -> HashMap<String, ModelSpec> {
        pictesa_providers::mock::Mock.models().into_iter().map(|m| (m.id.clone(), m)).collect()
    }

    fn editor(id: &str, references: u32) -> ModelSpec {
        ModelSpec::new(id.split_once(':').unwrap().0, id.split_once(':').unwrap().1, "Editor", MediaKind::Image, &[Task::ImageEdit]).input(InputSlot::new(InputRole::Reference, MediaKind::Image, references))
    }

    /// The model each step of a recipe picks from `ready`.
    fn picks(recipe: &Recipe, ready: &HashMap<String, ModelSpec>) -> Vec<Option<String>> {
        recipe.steps.iter().map(|s| pick_model(s, ready, &listed_needs(recipe, s))).collect()
    }

    #[test]
    fn snaps_params_to_the_model() {
        let d = pictesa_core::common::duration(&[2, 4, 6], 2);
        assert_eq!(fit_param(&d, &json!("5")), Some(json!(4)));
        assert_eq!(fit_param(&d, &json!(6)), Some(json!(6)));
        let r = pictesa_core::ParamSpec::choices("aspect_ratio", "Aspect ratio", &["16:9", "9:16"]);
        assert_eq!(fit_param(&r, &json!("1:1")), None);
        let i = pictesa_core::ParamSpec::int("steps", "Steps", 1, 30);
        assert_eq!(fit_param(&i, &json!("50")), Some(json!(30)));
    }

    #[test]
    fn renders_templates_and_tidies_empty_fields() {
        let mut v = Map::new();
        v.insert("subject".into(), json!("a cabin in fog"));
        v.insert("lens".into(), json!("35mm"));
        v.insert("light".into(), json!(""));
        assert_eq!(render("{{subject}}, {{lens}} lens, {{light}}, film grain", &v), "a cabin in fog, 35mm lens, film grain");
        v.insert("duration".into(), json!(5));
        assert_eq!(render_value(&json!("{{duration}}"), &v), Some(json!(5)));
        assert_eq!(render_value(&json!("{{missing}}"), &v), None);
    }

    #[test]
    fn sections_follow_fields_and_inputs() {
        let t = "Place the product {{#scene}}into the scene from the second image{{/scene}}{{^scene}}into a new photo{{/scene}}{{#where}}: {{where}}{{/where}}.";
        assert_eq!(expand(t, &values(&[("scene", json!(true))])), "Place the product into the scene from the second image.");
        assert_eq!(expand(t, &values(&[("where", json!("on a table"))])), "Place the product into a new photo: on a table.");
        assert_eq!(expand(t, &values(&[("where", json!("  ")), ("scene", json!(false))])), "Place the product into a new photo.");
        assert_eq!(expand("{{angles}}", &values(&[("angles", json!(["low", "high"]))])), "low, high", "lists join with commas");
        assert_eq!(expand("a {{#open}}b", &Map::new()), "a ", "an unclosed section runs to the end");
    }

    #[test]
    fn each_expands_fixed_items_choices_and_lines() {
        let mut step = get("photo/shots").steps.remove(0);
        assert_eq!(each_items(&step, &Map::new()).len(), 9, "Shots makes nine different shots");
        let shots: HashSet<String> = each_items(&step, &Map::new()).into_iter().collect();
        assert_eq!(shots.len(), 9, "every shot is different");
        step.each = vec!["{{angles}}".into()];
        assert_eq!(each_items(&step, &values(&[("angles", json!(["from behind", " ", "low angle"]))])), ["from behind", "low angle"]);
        step.each = vec!["{{scenes}}".into()];
        let text = json!("1. A kettle boils\n\n- Steam rises\n* Tea pours\n2) Cups clink\n10.5 percent stays");
        assert_eq!(each_items(&step, &values(&[("scenes", text)])), ["A kettle boils", "Steam rises", "Tea pours", "Cups clink", "10.5 percent stays"]);
        assert!(each_items(&step, &Map::new()).is_empty(), "an empty field has no items");
        assert!(each_items(&get("photo/face-swap").steps[0], &Map::new()).is_empty(), "a plain step runs once");
    }

    #[test]
    fn builtin_recipes_parse_and_pass_checks() {
        let list = builtin();
        for r in &list {
            check(r).unwrap_or_else(|why| panic!("{}: {why}", r.id));
            assert!(r.picture.is_some(), "{} has a card picture", r.id);
            assert!(r.steps.iter().all(|s| s.needs.is_some()), "{}: every step says what to set up", r.id);
        }
        for id in NEW_APPS {
            let r = list.iter().find(|r| r.id == id).unwrap_or_else(|| panic!("{id} is built in"));
            assert!(r.steps.iter().all(|s| s.models.iter().any(|m| m.starts_with("mock:"))), "{id} runs on the free mock");
        }
        let ids: HashSet<&str> = list.iter().map(|r| r.id.as_str()).collect();
        assert_eq!(ids.len(), list.len(), "recipe ids are unique");
    }

    #[test]
    fn checks_catch_broken_recipes() {
        let mut r = get("product/placement");
        r.steps[0].inputs.push(StepInput { role: InputRole::Reference, from: "step:later".into() });
        assert!(check(&r).unwrap_err().contains("step:later"));
        let mut r = get("photo/angles");
        r.fields[0].default = Some(json!(["sideways"]));
        assert!(check(&r).unwrap_err().contains("sideways"));
        let mut r = get("photo/shots");
        r.steps[0].each = vec!["{{nothing}}".into()];
        assert!(check(&r).is_err());
        let mut r = get("photo/face-swap");
        r.inputs[1].key = r.inputs[0].key.clone();
        assert!(check(&r).unwrap_err().contains("twice"));
    }

    #[test]
    fn every_new_app_runs_on_the_mock_models() {
        let ready = mock_ready();
        for id in NEW_APPS {
            let r = get(id);
            for (step, pick) in r.steps.iter().zip(picks(&r, &ready)) {
                let want = if step.output == MediaKind::Video { "mock:video" } else { "mock:image" };
                assert_eq!(pick.as_deref(), Some(want), "{id}/{}", step.id);
            }
        }
    }

    #[test]
    fn steps_pick_the_first_set_up_model_that_fits() {
        let face_swap = get("photo/face-swap");
        let shots = get("photo/shots");
        let first = shots.steps[0].models[0].clone();
        assert_eq!(face_swap.steps[0].models[0], first, "both prefer the same editor");
        // A complete one-image editor fits Shots (one photo) but not Face swap (face and target).
        let mut ready = mock_ready();
        ready.insert(first.clone(), editor(&first, 1));
        assert_eq!(picks(&shots, &ready), [Some(first.clone())]);
        assert_eq!(picks(&face_swap, &ready), [Some("mock:image".to_string())]);
        // Gateway models whose inputs are not loaded yet are trusted, in list order.
        let mut gateway = editor(&first, 0);
        gateway.complete = false;
        ready.insert(first.clone(), gateway);
        assert_eq!(picks(&face_swap, &ready), [Some(first.clone())]);
        // A model that cannot follow written instructions never runs a written step.
        let relight = get("photo/relight");
        let silent = ModelSpec::new("mock", "image", "Silent", MediaKind::Image, &[Task::ImageEdit]).input(InputSlot::new(InputRole::Reference, MediaKind::Image, 1)).no_prompt();
        assert!(fits(&silent, &relight.steps[0], &listed_needs(&relight, &relight.steps[0])).unwrap_err().contains("written"));
        // Video steps need a model that takes the still as a start frame.
        let ugc = get("ads/ugc");
        let clip = &ugc.steps[1];
        let text_only = ModelSpec::new("mock", "video", "Text video", MediaKind::Video, &[Task::TextToVideo]);
        assert!(fits(&text_only, clip, &listed_needs(&ugc, clip)).unwrap_err().contains("start frame"));
        assert!(fits(&editor("mock:image", 4), clip, &[]).unwrap_err().contains("makes images"));
    }

    #[test]
    fn steps_without_a_model_say_what_to_set_up() {
        let empty = HashMap::new();
        for r in builtin() {
            assert!(picks(&r, &empty).iter().all(Option::is_none), "{}", r.id);
            for s in &r.steps {
                let text = missing_text(s);
                assert!(text.contains(&s.label) && text.contains(s.needs.as_deref().unwrap()), "{text}");
            }
        }
    }

    #[test]
    fn face_swap_and_placement_count_every_file() {
        let swap = get("photo/face-swap");
        assert_eq!(listed_needs(&swap, &swap.steps[0]), [Need { role: InputRole::Reference, kind: MediaKind::Image, count: 2 }]);
        let placement = get("product/placement");
        let with_scene = step_needs(&placement, &placement.steps[0], |_| 1);
        assert_eq!(with_scene[0].count, 2, "product and scene photo");
        assert_eq!(listed_needs(&placement, &placement.steps[0])[0].count, 1, "the scene photo is optional");
    }

    #[test]
    fn values_are_settled_against_the_fields() {
        let none = |_: &str| false;
        let any_preset = |_: &str| true;
        // Multi fields: defaults, single strings, unknown options and repeats.
        let angles = get("photo/angles");
        let mut v = Map::new();
        settle_values(&angles, &mut v, &none, &any_preset).unwrap();
        assert!(v["angles"].as_array().is_some_and(|a| !a.is_empty()), "the default angles apply");
        let option = angles.fields[0].options[0].clone();
        let mut v = values(&[("angles", json!(option))]);
        settle_values(&angles, &mut v, &none, &any_preset).unwrap();
        assert_eq!(v["angles"], json!([option]), "one choice may come as text");
        let mut v = values(&[("angles", json!([option, option]))]);
        settle_values(&angles, &mut v, &none, &any_preset).unwrap();
        assert_eq!(v["angles"], json!([option]), "repeats are dropped");
        let mut v = values(&[("angles", json!(["sideways"]))]);
        let err = settle_values(&angles, &mut v, &none, &any_preset).unwrap_err();
        assert!(matches!(err, EngineError::Validation { field: Some(ref f), .. } if f == "values.angles"), "{err:?}");
        let mut v = values(&[("angles", json!([1, 2]))]);
        assert!(settle_values(&angles, &mut v, &none, &any_preset).is_err());
        // Product placement needs a described scene or a scene photo.
        let placement = get("product/placement");
        let mut v = Map::new();
        let err = settle_values(&placement, &mut v, &none, &any_preset).unwrap_err();
        assert!(err.to_string().contains("scene photo"), "{err}");
        let scene_added = |key: &str| key == "scene";
        settle_values(&placement, &mut Map::new(), &scene_added, &any_preset).unwrap();
        settle_values(&placement, &mut values(&[("where", json!("on a marble counter"))]), &none, &any_preset).unwrap();
        // Preset fields only take known presets.
        let grade = get("photo/color-grade");
        let mut v = values(&[("look", json!("grade/does-not-exist"))]);
        assert!(settle_values(&grade, &mut v, &none, &|id| id != "grade/does-not-exist").is_err());
    }
}
