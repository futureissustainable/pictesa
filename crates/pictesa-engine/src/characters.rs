//! Characters: named sets of reference images (a person, a product, a style) that any run can
//! attach with one click. Images work as references; optional training creates a family-specific
//! LoRA linked to the character.
//!
//! A brand kit is a character of kind `brand`: its images are the brand's references, and its
//! colors and words join the prompt of every run it is attached to.

use pictesa_core::{now_ms, InputRole, MediaKind};
use serde::{Deserialize, Serialize};
use serde_json::json;

use crate::{store, Engine, EngineError, JobInput, Result};

pub const MAX_REFERENCES: usize = 14;
pub const MAX_COLORS: usize = 8;
pub const MAX_WORDS: usize = 300;
const KINDS: [&str; 2] = ["character", "brand"];

#[derive(Debug, Clone, Serialize, sqlx::FromRow)]
pub struct Character {
    pub id: String,
    pub name: String,
    pub description: String,
    /// `character` or `brand` (a brand kit).
    pub kind: String,
    /// Brand colors as `#rrggbb`.
    #[sqlx(skip)]
    pub colors: Vec<String>,
    /// Brand words: tone, style and phrases that describe the brand.
    pub words: String,
    #[sqlx(skip)]
    pub assets: Vec<crate::Asset>,
    #[serde(skip)]
    pub asset_ids: String,
    #[serde(skip)]
    #[sqlx(rename = "colors")]
    pub colors_json: String,
    pub created_at: i64,
    pub updated_at: i64,
}

#[derive(Debug, Clone, Default, Deserialize)]
pub struct CharacterInput {
    pub name: Option<String>,
    pub description: Option<String>,
    pub asset_ids: Option<Vec<String>>,
    pub kind: Option<String>,
    pub colors: Option<Vec<String>>,
    pub words: Option<String>,
}

impl Character {
    pub fn is_brand(&self) -> bool {
        self.kind == "brand"
    }

    /// The sentence a brand kit adds to every prompt. `with_images` when its images were attached.
    pub fn brand_line(&self, with_images: bool) -> Option<String> {
        if !self.is_brand() {
            return None;
        }
        let mut parts = Vec::new();
        let words = self.words.trim().trim_end_matches(['.', ' ']);
        if !words.is_empty() {
            parts.push(format!("Brand look: {words}."));
        }
        if let Some((last, rest)) = self.colors.split_last() {
            parts.push(if rest.is_empty() { format!("Brand color: {last}.") } else { format!("Brand colors: {} and {last}.", rest.join(", ")) });
        }
        if with_images {
            parts.push("Keep the look of the brand reference images.".into());
        }
        (!parts.is_empty()).then(|| parts.join(" "))
    }

    /// Its images as reference inputs, as many as `room` allows.
    pub(crate) fn reference_inputs(&self, room: u32) -> Vec<JobInput> {
        self.assets.iter().take(room as usize).map(|a| JobInput { asset_id: a.id.clone(), role: InputRole::Reference }).collect()
    }
}

/// `#RGB` or `#RRGGBB` (any case), returned as lowercase `#rrggbb`.
pub fn normalize_color(raw: &str) -> Option<String> {
    let hex = raw.trim().strip_prefix('#')?;
    if !hex.chars().all(|c| c.is_ascii_hexdigit()) {
        return None;
    }
    match hex.len() {
        6 => Some(format!("#{}", hex.to_ascii_lowercase())),
        3 => Some(format!("#{}", hex.chars().flat_map(|c| [c, c]).collect::<String>().to_ascii_lowercase())),
        _ => None,
    }
}

fn check_kind(kind: &str) -> Result<String> {
    KINDS.contains(&kind).then(|| kind.to_string()).ok_or_else(|| EngineError::invalid("Kind must be character or brand.", Some("kind")))
}

fn check_colors(colors: &[String]) -> Result<Vec<String>> {
    if colors.len() > MAX_COLORS {
        return Err(EngineError::invalid(format!("A brand kit has up to {MAX_COLORS} colors."), Some("colors")));
    }
    let mut out: Vec<String> = Vec::new();
    for raw in colors {
        let color = normalize_color(raw).ok_or_else(|| EngineError::invalid(format!("{raw} is not a color. Use a hex code like #0b6e4f."), Some("colors")))?;
        if !out.contains(&color) {
            out.push(color);
        }
    }
    Ok(out)
}

fn check_words(words: &str) -> Result<String> {
    let words = words.trim();
    if words.chars().count() > MAX_WORDS {
        return Err(EngineError::invalid(format!("Keep the brand words under {MAX_WORDS} characters."), Some("words")));
    }
    Ok(words.to_string())
}

impl Engine {
    async fn fill(&self, user: &str, mut c: Character) -> Character {
        c.colors = serde_json::from_str(&c.colors_json).unwrap_or_default();
        let ids: Vec<String> = serde_json::from_str(&c.asset_ids).unwrap_or_default();
        for id in ids {
            if let Ok(row) = store::asset_row(&self.inner.db, user, &id).await {
                if row.deleted_at.is_none() {
                    c.assets.push(row.to_asset());
                }
            }
        }
        c
    }

    async fn check_assets(&self, user: &str, ids: &[String]) -> Result<()> {
        if ids.is_empty() || ids.len() > MAX_REFERENCES {
            return Err(EngineError::invalid(format!("A character needs 1 to {MAX_REFERENCES} images."), Some("asset_ids")));
        }
        for id in ids {
            let row = store::asset_row(&self.inner.db, user, id).await.map_err(|_| EngineError::invalid("An image is not in the library.", Some("asset_ids")))?;
            if store::parse_kind(&row.kind) != MediaKind::Image || row.deleted_at.is_some() {
                return Err(EngineError::invalid("Characters use images from the library.", Some("asset_ids")));
            }
        }
        Ok(())
    }

    pub async fn characters(&self, user: &str) -> Result<Vec<Character>> {
        let rows: Vec<Character> = sqlx::query_as("SELECT id, name, description, kind, colors, words, asset_ids, created_at, updated_at FROM characters WHERE user_id = ? ORDER BY updated_at DESC")
            .bind(user)
            .fetch_all(&self.inner.db)
            .await?;
        let mut out = Vec::new();
        for c in rows {
            out.push(self.fill(user, c).await);
        }
        Ok(out)
    }

    pub async fn character(&self, user: &str, id: &str) -> Result<Character> {
        let row: Character = sqlx::query_as("SELECT id, name, description, kind, colors, words, asset_ids, created_at, updated_at FROM characters WHERE id = ? AND user_id = ?")
            .bind(id)
            .bind(user)
            .fetch_one(&self.inner.db)
            .await
            .map_err(|_| EngineError::not_found("character"))?;
        Ok(self.fill(user, row).await)
    }

    /// A character by id, or by name when no id matches (names compare without case).
    pub async fn find_character(&self, user: &str, id_or_name: &str) -> Result<Character> {
        let wanted = id_or_name.trim();
        let found: Option<(String,)> = sqlx::query_as("SELECT id FROM characters WHERE user_id = ? AND (id = ? OR lower(name) = lower(?)) ORDER BY id = ? DESC, updated_at DESC LIMIT 1")
            .bind(user)
            .bind(wanted)
            .bind(wanted)
            .bind(wanted)
            .fetch_optional(&self.inner.db)
            .await?;
        let (id,) = found.ok_or_else(|| EngineError::not_found("character"))?;
        self.character(user, &id).await
    }

    pub async fn create_character(&self, user: &str, input: CharacterInput) -> Result<Character> {
        let name = input.name.unwrap_or_default().trim().to_string();
        if name.is_empty() || name.chars().count() > 80 {
            return Err(EngineError::invalid("Give the character a short name.", Some("name")));
        }
        let ids = input.asset_ids.unwrap_or_default();
        self.check_assets(user, &ids).await?;
        let kind = check_kind(input.kind.as_deref().unwrap_or("character"))?;
        let colors = check_colors(&input.colors.unwrap_or_default())?;
        let words = check_words(&input.words.unwrap_or_default())?;
        let id = crate::new_id();
        let now = now_ms();
        sqlx::query("INSERT INTO characters (id, user_id, name, description, kind, colors, words, asset_ids, created_at, updated_at) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?)")
            .bind(&id)
            .bind(user)
            .bind(&name)
            .bind(input.description.unwrap_or_default().chars().take(500).collect::<String>())
            .bind(&kind)
            .bind(serde_json::to_string(&colors).unwrap_or_default())
            .bind(&words)
            .bind(serde_json::to_string(&ids).unwrap_or_default())
            .bind(now)
            .bind(now)
            .execute(&self.inner.db)
            .await?;
        let c = self.character(user, &id).await?;
        self.emit(user, "character", json!(c));
        Ok(c)
    }

    pub async fn update_character(&self, user: &str, id: &str, input: CharacterInput) -> Result<Character> {
        let mut current = self.character(user, id).await?;
        if let Some(n) = input.name {
            let n = n.trim().to_string();
            if n.is_empty() || n.chars().count() > 80 {
                return Err(EngineError::invalid("Give the character a short name.", Some("name")));
            }
            current.name = n;
        }
        if let Some(d) = input.description {
            current.description = d.chars().take(500).collect();
        }
        if let Some(kind) = input.kind {
            current.kind = check_kind(&kind)?;
        }
        if let Some(colors) = input.colors {
            current.colors = check_colors(&colors)?;
        }
        if let Some(words) = input.words {
            current.words = check_words(&words)?;
        }
        let ids = match input.asset_ids {
            Some(ids) => {
                self.check_assets(user, &ids).await?;
                ids
            }
            None => current.assets.iter().map(|a| a.id.clone()).collect(),
        };
        sqlx::query("UPDATE characters SET name = ?, description = ?, kind = ?, colors = ?, words = ?, asset_ids = ?, updated_at = ? WHERE id = ? AND user_id = ?")
            .bind(&current.name)
            .bind(&current.description)
            .bind(&current.kind)
            .bind(serde_json::to_string(&current.colors).unwrap_or_default())
            .bind(&current.words)
            .bind(serde_json::to_string(&ids).unwrap_or_default())
            .bind(now_ms())
            .bind(id)
            .bind(user)
            .execute(&self.inner.db)
            .await?;
        let c = self.character(user, id).await?;
        self.emit(user, "character", json!(c));
        Ok(c)
    }

    pub async fn delete_character(&self, user: &str, id: &str) -> Result<()> {
        let done = sqlx::query("DELETE FROM characters WHERE id = ? AND user_id = ?").bind(id).bind(user).execute(&self.inner.db).await?;
        if done.rows_affected() == 0 {
            return Err(EngineError::not_found("character"));
        }
        self.emit(user, "character", json!({ "id": id, "deleted": true }));
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn kit(kind: &str, colors: &[&str], words: &str) -> Character {
        Character {
            id: "c".into(),
            name: "Oak".into(),
            description: String::new(),
            kind: kind.into(),
            colors: colors.iter().map(|c| c.to_string()).collect(),
            words: words.into(),
            assets: Vec::new(),
            asset_ids: "[]".into(),
            colors_json: "[]".into(),
            created_at: 0,
            updated_at: 0,
        }
    }

    #[test]
    fn colors_are_hex_codes() {
        assert_eq!(normalize_color(" #0B6E4F "), Some("#0b6e4f".into()));
        assert_eq!(normalize_color("#fA0"), Some("#ffaa00".into()));
        for bad in ["0b6e4f", "#0b6e4", "#0b6e4g", "red", "#"] {
            assert_eq!(normalize_color(bad), None, "{bad}");
        }
        assert_eq!(check_colors(&["#ABC".into(), "#aabbcc".into()]).unwrap(), ["#aabbcc"], "repeats are dropped");
        assert!(check_colors(&vec!["#000000".to_string(); MAX_COLORS + 1]).is_err());
        assert!(check_kind("logo").is_err());
    }

    #[test]
    fn only_brand_kits_add_a_line() {
        assert_eq!(kit("character", &["#000000"], "calm").brand_line(true), None);
        let line = kit("brand", &["#0b6e4f", "#f2c14e", "#ffffff"], "calm, natural, honest.").brand_line(true).unwrap();
        assert_eq!(line, "Brand look: calm, natural, honest. Brand colors: #0b6e4f, #f2c14e and #ffffff. Keep the look of the brand reference images.");
        assert_eq!(kit("brand", &["#0b6e4f"], "").brand_line(false).as_deref(), Some("Brand color: #0b6e4f."));
        assert_eq!(kit("brand", &[], " ").brand_line(false), None);
    }
}
