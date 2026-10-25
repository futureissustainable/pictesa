//! Skill packs: ready-made workflows agents follow (UGC ad, hook variants, explainer, faceless
//! short, brand kit). Each pack is a short instruction text with arguments, served as an MCP prompt
//! and by `pictesa pack`. Packs are plain JSON like recipes and presets: built-ins live in
//! `packs/packs.json`, your own in the `packs/` folder of the data folder.

use std::path::PathBuf;

use serde::{Deserialize, Serialize};
use serde_json::{json, Map, Value};

use crate::{recipes::expand, Engine, EngineError, Result};

const BUILTIN: &str = include_str!("../../../packs/packs.json");

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PackArgument {
    pub name: String,
    pub description: String,
    #[serde(default)]
    pub required: bool,
    /// `text`, `number` (between `min` and `max`), `choice` (one of `options`), `file` (an asset id,
    /// URL or path), `character` or `brand` (a saved character or brand kit, by id or name).
    #[serde(rename = "type", default = "text_kind")]
    pub kind: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub options: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub min: Option<i64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max: Option<i64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub default: Option<String>,
}

fn text_kind() -> String {
    "text".into()
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Pack {
    /// The MCP prompt name.
    pub name: String,
    pub title: String,
    pub description: String,
    /// The recipe the pack runs, when its steps are fixed.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub recipe: Option<String>,
    #[serde(default)]
    pub arguments: Vec<PackArgument>,
    /// Templates, with `{{argument}}` and `{{#argument}}...{{/argument}}` as in recipes.
    pub intro: String,
    pub steps: Vec<String>,
    #[serde(default)]
    pub rules: Vec<String>,
}

pub fn load(dirs: &[PathBuf]) -> Vec<Pack> {
    crate::load_json(BUILTIN, dirs, "packs", |p: &Pack| p.name.clone(), |_, _| true)
}

fn bad(message: String, argument: &str) -> EngineError {
    EngineError::invalid(message, Some(&format!("arguments.{argument}")))
}

/// How an agent passes a file to run_recipe: `{"url": ...}`, `{"asset_id": ...}` or `{"path": ...}`.
pub fn file_input(raw: &str) -> Value {
    let raw = raw.trim();
    if raw.starts_with("https://") || raw.starts_with("http://") {
        json!({ "url": raw })
    } else if raw.len() == 26 && raw.chars().all(|c| c.is_ascii_digit() || c.is_ascii_uppercase()) {
        json!({ "asset_id": raw })
    } else {
        json!({ "path": raw })
    }
}

/// Checks an argument value against its declaration and returns it tidied.
fn check_value(arg: &PackArgument, raw: &str) -> Result<String> {
    let value = raw.trim();
    match arg.kind.as_str() {
        "number" => {
            let n: i64 = value.parse().map_err(|_| bad(format!("{} must be a whole number.", arg.name), &arg.name))?;
            let (min, max) = (arg.min.unwrap_or(i64::MIN), arg.max.unwrap_or(i64::MAX));
            if n < min || n > max {
                return Err(bad(format!("{} must be from {min} to {max}.", arg.name), &arg.name));
            }
            Ok(n.to_string())
        }
        "choice" if !arg.options.iter().any(|o| o == value) => Err(bad(format!("{} must be one of {}.", arg.name, arg.options.join(", ")), &arg.name)),
        _ if value.chars().count() > 4000 => Err(bad(format!("{} is longer than 4,000 characters.", arg.name), &arg.name)),
        _ => Ok(value.to_string()),
    }
}

impl Engine {
    pub fn packs(&self) -> &[Pack] {
        &self.inner.packs
    }

    pub fn pack(&self, name: &str) -> Option<&Pack> {
        self.inner.packs.iter().find(|p| p.name == name)
    }

    /// The pack's instructions for these arguments. Arguments are text, as MCP sends them; unknown,
    /// missing and invalid ones are refused with the argument named.
    pub async fn pack_prompt(&self, user: &str, name: &str, args: &Map<String, Value>) -> Result<(&Pack, String)> {
        let pack = self.pack(name).ok_or_else(|| EngineError::NotFound(format!("No skill pack is called {name}. Call prompts/list to see them.")))?;
        if let Some(unknown) = args.keys().find(|k| !pack.arguments.iter().any(|a| &a.name == *k)) {
            let known: Vec<&str> = pack.arguments.iter().map(|a| a.name.as_str()).collect();
            return Err(bad(format!("{name} has no argument {unknown}. It takes: {}.", known.join(", ")), unknown));
        }
        let mut values = Map::new();
        for arg in &pack.arguments {
            let given = match args.get(&arg.name) {
                Some(Value::String(s)) => Some(s.clone()),
                Some(Value::Null) | None => None,
                Some(other @ (Value::Number(_) | Value::Bool(_))) => Some(other.to_string()),
                Some(_) => return Err(bad(format!("{} must be text.", arg.name), &arg.name)),
            }
            .filter(|s| !s.trim().is_empty());
            let Some(raw) = given.or_else(|| arg.default.clone()) else {
                if arg.required {
                    return Err(bad(format!("{name} needs the argument {}: {}", arg.name, arg.description), &arg.name));
                }
                continue;
            };
            let value = check_value(arg, &raw)?;
            match arg.kind.as_str() {
                "file" => {
                    values.insert(format!("{}_input", arg.name), json!(file_input(&value).to_string()));
                }
                "character" | "brand" => {
                    let c = self.find_character(user, &value).await.map_err(|_| bad(format!("No character or brand kit is called {value}. Call list_characters to see them."), &arg.name))?;
                    if arg.kind == "brand" && !c.is_brand() {
                        return Err(bad(format!("{} is a character, not a brand kit. Mark it as a brand kit on the Characters page.", c.name), &arg.name));
                    }
                    let images: Vec<Value> = c.assets.iter().map(|a| json!({ "asset_id": a.id })).collect();
                    values.insert(format!("{}_name", arg.name), json!(c.name));
                    values.insert(format!("{}_images", arg.name), json!(if images.len() == 1 { "1 reference image".to_string() } else { format!("{} reference images", images.len()) }));
                    values.insert(format!("{}_inputs", arg.name), json!(Value::Array(images).to_string()));
                    if let Some(line) = c.brand_line(!c.assets.is_empty()) {
                        values.insert(format!("{}_line", arg.name), json!(line));
                    }
                    values.insert(arg.name.clone(), json!(c.id));
                    continue;
                }
                _ => {}
            }
            values.insert(arg.name.clone(), json!(value));
        }
        let mut text = expand(&pack.intro, &values).trim().to_string();
        text.push_str("\n\nSteps:");
        for (i, step) in pack.steps.iter().enumerate() {
            text.push_str(&format!("\n{}. {}", i + 1, expand(step, &values).trim()));
        }
        if !pack.rules.is_empty() {
            text.push_str("\n\nRules:");
            for rule in &pack.rules {
                text.push_str(&format!("\n- {}", expand(rule, &values).trim()));
            }
        }
        Ok((pack, text))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn builtin_packs_parse_and_name_real_recipes() {
        let packs = load(&[]);
        let names: Vec<&str> = packs.iter().map(|p| p.name.as_str()).collect();
        assert_eq!(names, ["ugc_ad", "hook_variants", "explainer", "faceless_short", "brand_kit"]);
        let recipes = crate::recipes::load(&[]);
        for p in &packs {
            if let Some(id) = &p.recipe {
                assert!(recipes.iter().any(|r| &r.id == id), "{} runs {id}", p.name);
                assert!(p.steps.iter().any(|s| s.contains(&format!("\"{id}\""))), "{} tells the agent to run {id}", p.name);
            }
            for a in &p.arguments {
                assert!(["text", "number", "choice", "file", "character", "brand"].contains(&a.kind.as_str()), "{}.{}", p.name, a.name);
                if let Some(d) = &a.default {
                    check_value(a, d).unwrap_or_else(|e| panic!("{}.{} default: {e}", p.name, a.name));
                }
            }
        }
    }

    #[test]
    fn file_arguments_become_run_inputs() {
        assert_eq!(file_input("https://example.com/a.png"), json!({ "url": "https://example.com/a.png" }));
        assert_eq!(file_input("01JB2Q7Z8X9Y0ABCDEFGHJKMNP"), json!({ "asset_id": "01JB2Q7Z8X9Y0ABCDEFGHJKMNP" }));
        assert_eq!(file_input("./product.png"), json!({ "path": "./product.png" }));
    }

    #[test]
    fn argument_values_are_checked() {
        let count = PackArgument { name: "count".into(), description: String::new(), required: false, kind: "number".into(), options: vec![], min: Some(2), max: Some(8), default: None };
        assert_eq!(check_value(&count, " 5 ").unwrap(), "5");
        assert!(check_value(&count, "9").is_err());
        assert!(check_value(&count, "many").is_err());
        let shape = PackArgument { kind: "choice".into(), options: vec!["16:9".into(), "9:16".into()], min: None, max: None, ..count };
        assert!(check_value(&shape, "9:16").is_ok());
        assert!(check_value(&shape, "4:3").is_err());
    }
}
