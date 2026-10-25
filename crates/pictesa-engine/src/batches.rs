//! Bounded batch expansion. Queuing and pricing stay in the normal job planner.
use pictesa_core::{ModelSpec, ParamKind, ParamSpec};
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::{CreateGeneration, EngineError, Result};

pub const MAX_BATCH_RUNS: usize = 64;
pub(crate) const MAX_SAFE_SEED: u64 = 9_007_199_254_740_991;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SeedSweep {
    pub start: u64,
    pub count: u32,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GridAxis {
    pub key: String,
    pub values: Vec<Value>,
}

#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PromptFormat { #[default] Lines, Csv }

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct BatchRequest {
    #[serde(flatten)]
    pub generation: CreateGeneration,
    #[serde(default)]
    pub prompts: Vec<String>,
    /// One prompt per line, or CSV (first column, no header). Mutually exclusive with prompts.
    pub prompt_list: Option<String>,
    #[serde(default)]
    pub prompt_format: PromptFormat,
    pub seeds: Option<SeedSweep>,
    #[serde(default)]
    pub grid: Vec<GridAxis>,
    #[serde(default)]
    pub dry_run: bool,
}

#[derive(Debug, Serialize)]
pub struct BatchEstimate {
    pub run_count: usize,
    pub total_estimated_cost_usd: Option<f64>,
    pub max_runs: usize,
}

/// Lines preserve punctuation. CSV reads the first column without a header.
/// Quoted fields support commas, newlines and doubled quotes. Empty rows are skipped.
pub fn parse_prompt_list(text: &str, format: PromptFormat) -> Result<Vec<String>> {
    let text = text.trim_start_matches('\u{feff}');
    if matches!(format, PromptFormat::Lines) {
        return Ok(text.lines().map(str::trim).filter(|s| !s.is_empty()).map(str::to_string).collect());
    }
    let mut chars = text.chars().peekable();
    let mut first = String::new();
    let mut prompts = Vec::new();
    let mut column = 0;
    let mut quoted = false;
    let mut at_start = true;
    let mut closed = false;
    while let Some(c) = chars.next() {
        if quoted {
            if c == '"' {
                if chars.peek() == Some(&'"') { chars.next(); if column == 0 { first.push('"'); } }
                else { quoted = false; closed = true; }
            } else if column == 0 { first.push(c); }
            continue;
        }
        match c {
            '"' if at_start => { quoted = true; at_start = false; }
            ',' => { column += 1; at_start = true; closed = false; }
            '\n' | '\r' => {
                if c == '\r' && chars.peek() == Some(&'\n') { chars.next(); }
                if !first.trim().is_empty() { prompts.push(first.trim().to_string()); }
                first.clear(); column = 0; at_start = true; closed = false;
            }
            _ if closed => return Err(EngineError::invalid("Unexpected text after a CSV quote.", Some("prompt_list"))),
            _ => { if column == 0 { first.push(c); } at_start = false; }
        }
    }
    if quoted { return Err(EngineError::invalid("Close the quoted CSV field.", Some("prompt_list"))); }
    if !first.trim().is_empty() { prompts.push(first.trim().to_string()); }
    Ok(prompts)
}

pub(crate) fn seed_param(spec: &ModelSpec) -> Option<&ParamSpec> {
    spec.params.iter().find(|p| matches!(p.kind, ParamKind::Seed) || (p.key == "seed" && matches!(p.kind, ParamKind::Int { .. })))
}

pub(crate) fn expand(req: &BatchRequest, spec: &ModelSpec) -> Result<Vec<CreateGeneration>> {
    let invalid = |msg| EngineError::invalid(msg, Some("batch"));
    if !req.generation.compare.is_empty() { return Err(invalid("A batch uses one model. Clear compare first.")); }
    if req.generation.count.is_some_and(|n| n != 1) { return Err(invalid("Each batch run makes one output. Use seeds or a prompt list for more runs.")); }
    if req.prompt_list.is_some() && !req.prompts.is_empty() { return Err(invalid("Use prompts or prompt_list, not both.")); }
    let prompts = if let Some(text) = &req.prompt_list { parse_prompt_list(text, req.prompt_format)? } else if !req.prompts.is_empty() { req.prompts.clone() } else { vec![req.generation.prompt.clone()] };
    if prompts.is_empty() || prompts.len() > MAX_BATCH_RUNS { return Err(invalid("Use 1 to 64 prompts.")); }
    if req.grid.len() > 3 { return Err(invalid("Sweep at most 3 parameters.")); }
    let seed_key = seed_param(spec).map(|p| p.key.as_str());
    let seed_count = match &req.seeds {
        Some(s) => {
            if seed_key.is_none() { return Err(EngineError::invalid("This model has no seed parameter.", Some("seeds"))); }
            if s.count == 0 || s.count as usize > MAX_BATCH_RUNS || s.start.checked_add(s.count as u64 - 1).is_none_or(|last| last > MAX_SAFE_SEED) {
                return Err(EngineError::invalid("Use 1 to 64 seeds, between 0 and 9007199254740991.", Some("seeds")));
            }
            s.count as usize
        }
        None => 1,
    };
    let mut count = prompts.len() * seed_count;
    for (i, axis) in req.grid.iter().enumerate() {
        if axis.values.is_empty() || axis.values.len() > MAX_BATCH_RUNS || spec.param_spec(&axis.key).is_none() || req.grid[..i].iter().any(|a| a.key == axis.key) || (req.seeds.is_some() && seed_key == Some(axis.key.as_str())) {
            return Err(EngineError::invalid("Use distinct model parameters with 1 to 64 values. Sweep seed in one place only.", Some("grid")));
        }
        count = count.saturating_mul(axis.values.len());
    }
    if count > MAX_BATCH_RUNS { return Err(invalid("A batch can contain at most 64 runs.")); }
    let mut out = Vec::with_capacity(count);
    // Prompt order first, then seeds ascending, then axes in request order (last axis fastest).
    for prompt in prompts {
        for seed in 0..seed_count {
            let mut one = req.generation.clone();
            one.prompt = prompt.clone();
            one.count = Some(1);
            if let (Some(sweep), Some(key)) = (&req.seeds, seed_key) { one.params.insert(key.to_string(), Value::from(sweep.start + seed as u64)); }
            let mut rows = vec![one];
            for axis in &req.grid {
                rows = rows.into_iter().flat_map(|row| axis.values.iter().map(move |v| {
                    let mut one = row.clone();
                    one.params.insert(axis.key.clone(), v.clone());
                    one
                })).collect();
            }
            out.extend(rows);
        }
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn csv_reads_first_column_including_quoted_newlines_and_quotes() {
        assert_eq!(parse_prompt_list("\u{feff}\"a, cabin\",one\r\n\"say \"\"hello\"\"\nagain\",two\n\nplain,three", PromptFormat::Csv).unwrap(), ["a, cabin", "say \"hello\"\nagain", "plain"]);
        assert_eq!(parse_prompt_list("a cabin\n\nbirch trees\n", PromptFormat::Lines).unwrap(), ["a cabin", "birch trees"]);
        assert!(parse_prompt_list("\"unfinished", PromptFormat::Csv).is_err());
        assert!(parse_prompt_list("\"closed\"junk,one", PromptFormat::Csv).is_err());
    }
    #[test]
    fn integer_seed_models_expand_zero_using_the_declared_parameter() {
        let mut spec = ModelSpec::new("test", "model", "Integer seed", pictesa_core::MediaKind::Image, &[pictesa_core::Task::TextToImage]);
        spec.params.push(ParamSpec { key: "seed".into(), label: "Seed".into(), kind: ParamKind::Int { min: 0, max: 10, step: 1 }, default: None, advanced: true, help: None });
        let req = BatchRequest { seeds: Some(SeedSweep { start: 0, count: 2 }), ..Default::default() };
        let runs = expand(&req, &spec).unwrap();
        assert_eq!(runs[0].params["seed"], 0);
        assert_eq!(runs[1].params["seed"], 1);
    }

}
