//! Characters: named sets of reference images (a person, a product, a style) that any run can
//! attach with one click. Images work as references; optional training creates a family-specific
//! LoRA linked to the character.

use pictesa_core::{now_ms, InputRole, MediaKind};
use serde::{Deserialize, Serialize};
use serde_json::json;

use crate::{store, Engine, EngineError, JobInput, Result};

pub const MAX_REFERENCES: usize = 14;

#[derive(Debug, Clone, Serialize, sqlx::FromRow)]
pub struct Character {
    pub id: String,
    pub name: String,
    pub description: String,
    #[sqlx(skip)]
    pub assets: Vec<crate::Asset>,
    #[serde(skip)]
    pub asset_ids: String,
    pub created_at: i64,
    pub updated_at: i64,
}

#[derive(Debug, Clone, Default, Deserialize)]
pub struct CharacterInput {
    pub name: Option<String>,
    pub description: Option<String>,
    pub asset_ids: Option<Vec<String>>,
}

impl Engine {
    async fn fill(&self, user: &str, mut c: Character) -> Character {
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
        let rows: Vec<Character> = sqlx::query_as("SELECT id, name, description, asset_ids, created_at, updated_at FROM characters WHERE user_id = ? ORDER BY updated_at DESC")
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
        let row: Character = sqlx::query_as("SELECT id, name, description, asset_ids, created_at, updated_at FROM characters WHERE id = ? AND user_id = ?")
            .bind(id)
            .bind(user)
            .fetch_one(&self.inner.db)
            .await
            .map_err(|_| EngineError::not_found("character"))?;
        Ok(self.fill(user, row).await)
    }

    pub async fn create_character(&self, user: &str, input: CharacterInput) -> Result<Character> {
        let name = input.name.unwrap_or_default().trim().to_string();
        if name.is_empty() || name.chars().count() > 80 {
            return Err(EngineError::invalid("Give the character a short name.", Some("name")));
        }
        let ids = input.asset_ids.unwrap_or_default();
        self.check_assets(user, &ids).await?;
        let id = crate::new_id();
        let now = now_ms();
        sqlx::query("INSERT INTO characters (id, user_id, name, description, asset_ids, created_at, updated_at) VALUES (?, ?, ?, ?, ?, ?, ?)")
            .bind(&id)
            .bind(user)
            .bind(&name)
            .bind(input.description.unwrap_or_default().chars().take(500).collect::<String>())
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
        let ids = match input.asset_ids {
            Some(ids) => {
                self.check_assets(user, &ids).await?;
                ids
            }
            None => current.assets.iter().map(|a| a.id.clone()).collect(),
        };
        sqlx::query("UPDATE characters SET name = ?, description = ?, asset_ids = ?, updated_at = ? WHERE id = ? AND user_id = ?")
            .bind(&current.name)
            .bind(&current.description)
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

    /// The character's images as reference inputs, as many as `max` allows.
    pub(crate) async fn character_inputs(&self, user: &str, id: &str, max: u32) -> Result<Vec<JobInput>> {
        let c = self.character(user, id).await?;
        Ok(c.assets.iter().take(max as usize).map(|a| JobInput { asset_id: a.id.clone(), role: InputRole::Reference }).collect())
    }
}
