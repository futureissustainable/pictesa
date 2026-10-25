//! Personal prompt history, saved prompts and literal, single-pass templates.
use std::collections::HashMap;

use pictesa_core::now_ms;
use serde::{Deserialize, Serialize};
use serde_json::{json, Map, Value};
use sqlx::FromRow;

use crate::{Engine, EngineError, Result};

#[derive(Debug, Default, Deserialize)]
pub struct PromptFilter {
    pub q: Option<String>,
    pub limit: Option<u32>,
}

#[derive(Debug, Serialize, FromRow)]
pub struct PromptHistory {
    pub prompt: String,
    pub model: String,
    pub model_name: String,
    pub last_used_at: i64,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct SavePrompt {
    pub name: String,
    pub prompt: String,
    pub negative_prompt: Option<String>,
    pub model: Option<String>,
    pub params: Option<Map<String, Value>>,
}

#[derive(Debug, Serialize)]
pub struct SavedPrompt {
    pub id: String,
    #[serde(flatten)]
    pub content: SavePrompt,
    pub placeholders: Vec<String>,
    pub created_at: i64,
    pub updated_at: i64,
}

#[derive(FromRow)]
struct Row {
    id: String,
    name: String,
    prompt: String,
    negative_prompt: Option<String>,
    model: Option<String>,
    params: Option<String>,
    created_at: i64,
    updated_at: i64,
}

impl Row {
    fn view(self) -> SavedPrompt {
        let mut placeholders = fields(&self.prompt);
        for field in fields(self.negative_prompt.as_deref().unwrap_or_default()) {
            if !placeholders.contains(&field) { placeholders.push(field); }
        }
        SavedPrompt {
            id: self.id,
            content: SavePrompt { name: self.name, prompt: self.prompt, negative_prompt: self.negative_prompt, model: self.model, params: self.params.and_then(|p| serde_json::from_str(&p).ok()) },
            placeholders,
            created_at: self.created_at,
            updated_at: self.updated_at,
        }
    }
}

// Only {identifiers} are fields. JSON braces remain literal; {{ and }} escape braces.
fn template(text: &str, mut field: impl FnMut(&str) -> Result<String>) -> Result<String> {
    let mut rest = text;
    let mut out = String::new();
    let mut escaped = 0;
    while !rest.is_empty() {
        if rest.starts_with("{{") {
            escaped += 1;
            out.push('{');
            rest = &rest[2..];
            continue;
        }
        if escaped > 0 && rest.starts_with("}}") {
            escaped -= 1;
            out.push('}');
            rest = &rest[2..];
            continue;
        }
        if let Some(after) = rest.strip_prefix('{') {
            if let Some(end) = after.find('}') {
                let key = &after[..end];
                if key.starts_with(|c: char| c.is_ascii_alphabetic() || c == '_') && key.chars().all(|c| c.is_ascii_alphanumeric() || c == '_') {
                    out.push_str(&field(key)?);
                    if out.len() > 80_000 { return Err(EngineError::invalid("The rendered prompt is too long.", Some("values"))); }
                    rest = &after[end + 1..];
                    continue;
                }
            }
        }
        let c = rest.chars().next().unwrap();
        out.push(c);
        rest = &rest[c.len_utf8()..];
    }
    Ok(out)
}

fn fields(text: &str) -> Vec<String> {
    let mut fields = Vec::new();
    let _ = template(text, |key| {
        if !fields.iter().any(|s| s == key) { fields.push(key.to_string()); }
        Ok(String::new())
    });
    fields
}

pub fn render_template(text: &str, values: &HashMap<String, String>) -> Result<String> {
    let out = template(text, |key| values.get(key).filter(|v| !v.trim().is_empty()).cloned().ok_or_else(|| EngineError::invalid(format!("Fill in {key}."), Some(&format!("values.{key}")))))?;
    if out.chars().count() > 20_000 { return Err(EngineError::invalid("A rendered prompt can contain up to 20,000 characters.", Some("values"))); }
    Ok(out)
}

impl Engine {
    pub async fn prompt_history(&self, user: &str, f: &PromptFilter) -> Result<Vec<PromptHistory>> {
        Ok(sqlx::query_as("SELECT prompt, model, model_name, created_at AS last_used_at FROM (SELECT prompt, model, model_name, created_at, id, ROW_NUMBER() OVER (PARTITION BY prompt ORDER BY created_at DESC, id DESC) AS rank FROM jobs WHERE user_id = ? AND trim(prompt) != '') WHERE rank = 1 AND instr(lower(prompt), lower(?)) > 0 ORDER BY created_at DESC, id DESC LIMIT ?")
            .bind(user).bind(f.q.as_deref().unwrap_or_default()).bind(f.limit.unwrap_or(50).clamp(1, 100)).fetch_all(&self.inner.db).await?)
    }

    pub async fn saved_prompts(&self, user: &str, f: &PromptFilter) -> Result<Vec<SavedPrompt>> {
        let rows: Vec<Row> = sqlx::query_as("SELECT * FROM saved_prompts WHERE user_id = ? AND (instr(lower(name), lower(?)) > 0 OR instr(lower(prompt), lower(?)) > 0) ORDER BY updated_at DESC, id DESC LIMIT ?")
            .bind(user).bind(f.q.as_deref().unwrap_or_default()).bind(f.q.as_deref().unwrap_or_default()).bind(f.limit.unwrap_or(50).clamp(1, 100)).fetch_all(&self.inner.db).await?;
        Ok(rows.into_iter().map(Row::view).collect())
    }

    pub async fn saved_prompt(&self, user: &str, id: &str) -> Result<SavedPrompt> {
        let row: Option<Row> = sqlx::query_as("SELECT * FROM saved_prompts WHERE user_id = ? AND id = ?").bind(user).bind(id).fetch_optional(&self.inner.db).await?;
        row.map(Row::view).ok_or_else(|| EngineError::not_found("prompt"))
    }

    pub async fn save_prompt(&self, user: &str, mut input: SavePrompt) -> Result<SavedPrompt> {
        input.name = checked_name(&input.name)?;
        if input.prompt.trim().is_empty() || input.prompt.chars().count() > 20_000 || input.negative_prompt.as_ref().is_some_and(|n| n.chars().count() > 20_000) {
            return Err(EngineError::invalid("Save a prompt of 1 to 20,000 characters.", Some("prompt")));
        }
        if input.model.as_ref().is_some_and(|m| m.is_empty() || m.len() > 500) || input.params.as_ref().is_some_and(|p| p.len() > 64 || json!(p).to_string().len() > 65536 || p.values().any(|v| v.is_array() || v.is_object())) {
            return Err(EngineError::invalid("Save up to 64 scalar model parameters (64 KB).", Some("params")));
        }
        let id = crate::new_id();
        let now = now_ms();
        sqlx::query("INSERT INTO saved_prompts (id, user_id, name, prompt, negative_prompt, model, params, created_at, updated_at) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?)")
            .bind(&id).bind(user).bind(input.name).bind(input.prompt).bind(input.negative_prompt).bind(input.model).bind(input.params.map(|p| json!(p).to_string())).bind(now).bind(now).execute(&self.inner.db).await?;
        self.saved_prompt(user, &id).await
    }

    pub async fn rename_prompt(&self, user: &str, id: &str, name: &str) -> Result<SavedPrompt> {
        let name = checked_name(name)?;
        let result = sqlx::query("UPDATE saved_prompts SET name = ?, updated_at = ? WHERE user_id = ? AND id = ?").bind(name).bind(now_ms()).bind(user).bind(id).execute(&self.inner.db).await?;
        if result.rows_affected() == 0 { return Err(EngineError::not_found("prompt")); }
        self.saved_prompt(user, id).await
    }

    pub async fn delete_prompt(&self, user: &str, id: &str) -> Result<()> {
        let result = sqlx::query("DELETE FROM saved_prompts WHERE user_id = ? AND id = ?").bind(user).bind(id).execute(&self.inner.db).await?;
        if result.rows_affected() == 0 { return Err(EngineError::not_found("prompt")); }
        Ok(())
    }

    pub async fn use_prompt(&self, user: &str, id: &str, values: HashMap<String, String>) -> Result<SavePrompt> {
        let mut saved = self.saved_prompt(user, id).await?.content;
        saved.prompt = render_template(&saved.prompt, &values)?;
        saved.negative_prompt = saved.negative_prompt.map(|s| render_template(&s, &values)).transpose()?;
        Ok(saved)
    }
}

fn checked_name(name: &str) -> Result<String> {
    let name = name.trim();
    if name.is_empty() || name.chars().count() > 100 { return Err(EngineError::invalid("Use a name of 1 to 100 characters.", Some("name"))); }
    Ok(name.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn templates_are_literal_single_pass_and_require_each_value() {
        let text = "{subject} in {place}, {subject}. {{literal}} {\"json\":1} 🌲";
        assert_eq!(fields(text), ["subject", "place"]);
        let values = HashMap::from([("subject".into(), "{place}".into()), ("place".into(), "Brașov".into())]);
        assert_eq!(render_template(text, &values).unwrap(), "{place} in Brașov, {place}. {literal} {\"json\":1} 🌲");
        assert!(render_template("{missing}", &values).is_err());
        assert!(render_template("{subject}", &HashMap::from([("subject".into(), " ".into())])).is_err());
        assert!(render_template("{subject}", &HashMap::from([("subject".into(), "x".repeat(20001))])).is_err());
        assert_eq!(render_template("unmatched { and }", &HashMap::new()).unwrap(), "unmatched { and }");
        let nested_json = r#"{"scene":{"name":"cabin"}}"#;
        assert_eq!(render_template(nested_json, &HashMap::new()).unwrap(), nested_json);
    }
}
