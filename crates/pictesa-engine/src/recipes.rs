//! Recipes: open, editable multi-step pipelines (the equivalent of a closed studio's "apps").
//!
//! A recipe declares the fields a person fills in, optional input files, and steps. Each step
//! names the models it prefers (the first one that is set up wins, cloud or local), a prompt
//! template (`{{field}}`), parameters, a preset, and where its input files come from: the
//! person's uploads (`input:<key>`) or an earlier step's result (`step:<id>`).

use std::{collections::HashMap, path::PathBuf, time::Duration};

use pictesa_core::{now_ms, InputRole, MediaKind};
use serde::{Deserialize, Serialize};
use serde_json::{json, Map, Value};

use crate::{CreateGeneration, Engine, EngineError, JobInput, JobStatus, Result};

const BUILTIN: &str = include_str!("../../../recipes/recipes.json");

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RecipeField {
    pub key: String,
    pub label: String,
    /// `text`, `enum` or `preset`.
    #[serde(rename = "type")]
    pub kind: String,
    #[serde(default)]
    pub options: Vec<String>,
    /// For `preset` fields: which preset category to offer (`camera`, `style`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub category: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub default: Option<Value>,
    #[serde(default)]
    pub required: bool,
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
    /// Preferred model ids, best first. The first configured one runs.
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
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Recipe {
    pub id: String,
    pub name: String,
    pub description: String,
    pub output: MediaKind,
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
}

#[derive(Debug, Clone, Serialize, sqlx::FromRow)]
pub struct RecipeRun {
    pub id: String,
    pub recipe_id: String,
    pub recipe_name: String,
    pub status: String,
    pub step: i64,
    pub steps: i64,
    #[sqlx(skip)]
    pub jobs: Vec<String>,
    #[serde(skip)]
    pub job_ids: String,
    pub error: Option<String>,
    pub created_at: i64,
    pub updated_at: i64,
}

impl RecipeRun {
    fn finish(mut self) -> Self {
        self.jobs = serde_json::from_str(&self.job_ids).unwrap_or_default();
        self
    }
}

pub fn load(dirs: &[PathBuf]) -> Vec<Recipe> {
    let mut out: Vec<Recipe> = serde_json::from_str(BUILTIN).unwrap_or_else(|e| {
        tracing::error!("built-in recipes are invalid: {e}");
        Vec::new()
    });
    for dir in dirs {
        let Ok(entries) = std::fs::read_dir(dir) else { continue };
        for e in entries.flatten() {
            let path = e.path();
            if path.extension().and_then(|x| x.to_str()) != Some("json") {
                continue;
            }
            match std::fs::read_to_string(&path).map_err(|e| e.to_string()).and_then(|t| serde_json::from_str::<Vec<Recipe>>(&t).map_err(|e| e.to_string())) {
                Ok(list) => {
                    for r in list {
                        out.retain(|q| q.id != r.id);
                        out.push(r);
                    }
                }
                Err(err) => tracing::warn!("skipping recipe file {}: {err}", path.display()),
            }
        }
    }
    out
}

/// Replaces `{{key}}` with field values; unknown keys become empty and spaces are tidied.
fn render(template: &str, values: &Map<String, Value>) -> String {
    let mut out = String::with_capacity(template.len());
    let mut rest = template;
    while let Some(start) = rest.find("{{") {
        out.push_str(&rest[..start]);
        let Some(end) = rest[start..].find("}}") else {
            out.push_str(&rest[start..]);
            rest = "";
            break;
        };
        let key = rest[start + 2..start + end].trim();
        if let Some(v) = values.get(key) {
            match v {
                Value::String(s) => out.push_str(s),
                Value::Null => {}
                other => out.push_str(&other.to_string()),
            }
        }
        rest = &rest[start + end + 2..];
    }
    out.push_str(rest);
    // Collapse leftovers like ", ," from empty optional fields.
    let mut tidy = out.split(',').map(str::trim).filter(|p| !p.is_empty()).collect::<Vec<_>>().join(", ");
    while tidy.contains("  ") {
        tidy = tidy.replace("  ", " ");
    }
    tidy
}

/// Renders a parameter: a lone `{{key}}` keeps the field's type (numbers stay numbers).
fn render_value(v: &Value, values: &Map<String, Value>) -> Option<Value> {
    match v {
        Value::String(s) if s.starts_with("{{") && s.ends_with("}}") && s.matches("{{").count() == 1 => {
            values.get(s[2..s.len() - 2].trim()).cloned().filter(|v| !v.is_null() && v.as_str() != Some(""))
        }
        Value::String(s) => Some(Value::String(render(s, values))),
        other => Some(other.clone()),
    }
}

impl Engine {
    pub fn recipe(&self, id: &str) -> Option<Recipe> {
        self.inner.recipes.iter().find(|r| r.id == id).cloned()
    }

    async fn configured_models(&self, user: &str) -> Result<std::collections::HashSet<String>> {
        let all = self.models(user, &crate::ModelFilter { configured: Some(true), ..Default::default() }).await?;
        Ok(all.into_iter().map(|m| m.spec.id).collect())
    }

    pub async fn recipes(&self, user: &str) -> Result<Vec<RecipeView>> {
        let ready = self.configured_models(user).await?;
        Ok(self
            .inner
            .recipes
            .iter()
            .map(|r| {
                let step_models: Vec<Option<String>> = r.steps.iter().map(|s| s.models.iter().find(|m| ready.contains(*m)).cloned()).collect();
                let runnable = step_models.iter().all(Option::is_some);
                RecipeView { recipe: r.clone(), step_models, runnable }
            })
            .collect())
    }

    /// Validates the request, records the run and starts it in the background.
    pub async fn run_recipe(&self, user: &str, recipe_id: &str, mut req: RunRecipe) -> Result<RecipeRun> {
        let recipe = self.recipe(recipe_id).ok_or_else(|| EngineError::not_found("recipe"))?;
        for f in &recipe.fields {
            let missing = req.values.get(&f.key).is_none_or(|v| v.is_null() || v.as_str().is_some_and(|s| s.trim().is_empty()));
            if missing {
                if let Some(d) = &f.default {
                    req.values.insert(f.key.clone(), d.clone());
                } else if f.required {
                    return Err(EngineError::invalid(format!("Fill in {}.", f.label), Some(&format!("values.{}", f.key))));
                }
            }
            if f.kind == "enum" {
                if let Some(v) = req.values.get(&f.key).and_then(Value::as_str) {
                    if !f.options.iter().any(|o| o == v) {
                        return Err(EngineError::invalid(format!("{} must be one of {}.", f.label, f.options.join(", ")), Some(&format!("values.{}", f.key))));
                    }
                }
            }
        }
        for input in &recipe.inputs {
            let n = req.inputs.get(&input.key).map(Vec::len).unwrap_or(0);
            if input.required && n == 0 {
                return Err(EngineError::invalid(format!("Add {}.", input.label), Some(&format!("inputs.{}", input.key))));
            }
            if n as u32 > input.max {
                return Err(EngineError::invalid(format!("{} takes at most {} file(s).", input.label, input.max), Some(&format!("inputs.{}", input.key))));
            }
        }
        let ready = self.configured_models(user).await?;
        let mut chosen = Vec::new();
        for step in &recipe.steps {
            let pick = match req.models.get(&step.id) {
                Some(m) if ready.contains(m) => Some(m.clone()),
                Some(m) => return Err(EngineError::invalid(format!("{m} is not set up."), Some(&format!("models.{}", step.id)))),
                None => step.models.iter().find(|m| ready.contains(*m)).cloned(),
            };
            let model = pick.ok_or_else(|| {
                EngineError::invalid(format!("No model is set up for the step \"{}\". Add a key for one of: {}.", step.label, step.models.join(", ")), Some("models"))
            })?;
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

    async fn drive_recipe(&self, user: &str, run_id: &str, recipe: &Recipe, req: &RunRecipe, models: &[String]) -> Result<()> {
        let mut values = req.values.clone();
        let mut step_outputs: HashMap<String, String> = HashMap::new();
        let mut job_ids: Vec<String> = Vec::new();
        for (i, step) in recipe.steps.iter().enumerate() {
            // Stop when the run was canceled, or marked failed by another process.
            if self.run_status(run_id).await.as_deref() != Some("running") {
                return Ok(());
            }
            let mut inputs = Vec::new();
            for si in &step.inputs {
                if let Some(key) = si.from.strip_prefix("input:") {
                    for asset in req.inputs.get(key).into_iter().flatten() {
                        inputs.push(JobInput { asset_id: asset.clone(), role: si.role });
                    }
                } else if let Some(prev) = si.from.strip_prefix("step:") {
                    if let Some(asset) = step_outputs.get(prev) {
                        inputs.push(JobInput { asset_id: asset.clone(), role: si.role });
                    }
                }
            }
            let mut params = Map::new();
            for (k, v) in &step.params {
                if let Some(v) = render_value(v, &values) {
                    params.insert(k.clone(), v);
                }
            }
            // Recipes stay portable: drop params the model lacks and snap values to what it accepts.
            if let Ok(spec) = self.model(user, &models[i]).await {
                if !spec.params.is_empty() {
                    params = params
                        .into_iter()
                        .filter_map(|(k, v)| {
                            let p = spec.param_spec(&k)?;
                            fit_param(p, &v).map(|v| (k, v))
                        })
                        .collect();
                }
            }
            let preset = step.preset.as_ref().and_then(|p| render_value(&json!(p), &values)).and_then(|v| v.as_str().map(String::from)).filter(|p| !p.is_empty());
            let prompt = render(&step.prompt, &values);
            let created = self
                .create_generation(user, CreateGeneration { model: models[i].clone(), prompt, negative_prompt: None, params, inputs, count: Some(1), preset, compare: Vec::new(), character: None })
                .await?;
            let job_id = created.jobs[0].id.clone();
            job_ids.push(job_id.clone());
            sqlx::query("UPDATE recipe_runs SET step = ?, job_ids = ?, updated_at = ? WHERE id = ?")
                .bind(i as i64)
                .bind(serde_json::to_string(&job_ids).unwrap_or_default())
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
            let job = self.wait_job(user, &job_id, Duration::from_secs(3 * 3600)).await;
            beat.abort();
            let job = job?;
            match job.status {
                JobStatus::Succeeded => {
                    if let Some(first) = job.outputs.first() {
                        step_outputs.insert(step.id.clone(), first.id.clone());
                        values.insert(format!("{}_asset", step.id), json!(first.id));
                    }
                }
                JobStatus::Canceled => {
                    self.end_run(user, run_id, "canceled", None).await;
                    return Ok(());
                }
                _ => {
                    let msg = job.error.map(|e| e.message).unwrap_or_else(|| "a step failed".into());
                    self.end_run(user, run_id, "failed", Some(&format!("{}: {msg}", step.label))).await;
                    return Ok(());
                }
            }
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

    /// Stops a run: cancels its current job and skips the remaining steps.
    pub async fn cancel_recipe_run(&self, user: &str, id: &str) -> Result<RecipeRun> {
        let run = self.recipe_run(user, id).await?;
        if run.status == "running" {
            self.end_run(user, id, "canceled", None).await;
            if let Some(last) = run.jobs.last() {
                let _ = self.cancel(user, last).await;
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
    fn builtin_recipes_parse() {
        let list: Vec<Recipe> = serde_json::from_str(BUILTIN).expect("recipes.json must parse");
        assert!(!list.is_empty());
        for r in &list {
            for s in &r.steps {
                for i in &s.inputs {
                    assert!(i.from.starts_with("input:") || i.from.starts_with("step:"), "{}: bad source {}", r.id, i.from);
                }
            }
        }
    }
}
