//! User-owned collections, tags and bounded download manifests.
use std::collections::HashSet;

use pictesa_core::now_ms;
use serde::{Deserialize, Serialize};
use sqlx::FromRow;

use crate::{store, Engine, EngineError, Result};

pub const MAX_ZIP_ASSETS: usize = 100;
pub const MAX_ZIP_BYTES: u64 = 1024 * 1024 * 1024;

#[derive(Debug, Serialize, FromRow)]
pub struct Collection {
    pub id: String,
    pub name: String,
    pub created_at: i64,
    pub asset_count: i64,
}

#[derive(Debug, Deserialize)]
pub struct AssetIds {
    pub asset_ids: Vec<String>,
}

pub struct ZipEntry {
    pub file: tokio::fs::File,
    pub name: String,
    pub size: u64,
}

fn label(value: &str, max: usize, field: &str) -> Result<String> {
    let value = value.trim();
    if value.is_empty() || value.chars().count() > max || value.chars().any(char::is_control) {
        return Err(EngineError::invalid(format!("Use 1 to {max} characters for {field}."), Some(field)));
    }
    Ok(value.to_string())
}

impl Engine {
    pub async fn collections(&self, user: &str) -> Result<Vec<Collection>> {
        Ok(sqlx::query_as(
            "SELECT c.id, c.name, c.created_at, count(a.id) AS asset_count FROM collections c
            LEFT JOIN collection_assets ca ON ca.collection_id = c.id
            LEFT JOIN assets a ON a.id = ca.asset_id AND a.user_id = c.user_id AND a.deleted_at IS NULL
            WHERE c.user_id = ? GROUP BY c.id ORDER BY c.name COLLATE NOCASE, c.id",
        )
        .bind(user)
        .fetch_all(&self.inner.db)
        .await?)
    }

    pub async fn create_collection(&self, user: &str, name: &str) -> Result<Collection> {
        let name = label(name, 100, "name")?;
        let id = crate::new_id();
        let now = now_ms();
        sqlx::query("INSERT INTO collections (id, user_id, name, created_at) VALUES (?, ?, ?, ?)")
            .bind(&id)
            .bind(user)
            .bind(&name)
            .bind(now)
            .execute(&self.inner.db)
            .await?;
        self.emit(user, "library", serde_json::json!({}));
        Ok(Collection { id, name, created_at: now, asset_count: 0 })
    }

    pub async fn rename_collection(&self, user: &str, id: &str, name: &str) -> Result<()> {
        let name = label(name, 100, "name")?;
        let changed =
            sqlx::query("UPDATE collections SET name = ? WHERE id = ? AND user_id = ?").bind(name).bind(id).bind(user).execute(&self.inner.db).await?;
        if changed.rows_affected() == 0 {
            return Err(EngineError::not_found("collection"));
        }
        self.emit(user, "library", serde_json::json!({}));
        Ok(())
    }

    pub async fn delete_collection(&self, user: &str, id: &str) -> Result<()> {
        let changed = sqlx::query("DELETE FROM collections WHERE id = ? AND user_id = ?").bind(id).bind(user).execute(&self.inner.db).await?;
        if changed.rows_affected() == 0 {
            return Err(EngineError::not_found("collection"));
        }
        self.emit(user, "library", serde_json::json!({}));
        Ok(())
    }

    /// Validate all owners before writing any membership, including removals.
    pub async fn collection_assets(&self, user: &str, collection: &str, ids: &[String], remove: bool) -> Result<()> {
        self.validate_asset_ids(user, ids).await?;
        let mut tx = self.inner.db.begin_with("BEGIN IMMEDIATE").await?;
        let owned: Option<(String,)> =
            sqlx::query_as("SELECT id FROM collections WHERE id = ? AND user_id = ?").bind(collection).bind(user).fetch_optional(&mut *tx).await?;
        if owned.is_none() {
            return Err(EngineError::not_found("collection"));
        }
        for id in ids {
            if remove {
                sqlx::query("DELETE FROM collection_assets WHERE collection_id = ? AND asset_id = ?").bind(collection).bind(id).execute(&mut *tx).await?;
            } else {
                sqlx::query("INSERT OR IGNORE INTO collection_assets (collection_id, asset_id) SELECT ?, id FROM assets WHERE id = ? AND user_id = ? AND deleted_at IS NULL")
                    .bind(collection).bind(id).bind(user).execute(&mut *tx).await?;
            }
        }
        tx.commit().await?;
        self.emit(user, "library", serde_json::json!({}));
        Ok(())
    }

    pub async fn tags(&self, user: &str) -> Result<Vec<String>> {
        Ok(sqlx::query_scalar(
            "SELECT DISTINCT t.tag FROM asset_tags t JOIN assets a ON a.id = t.asset_id
            WHERE a.user_id = ? AND a.deleted_at IS NULL ORDER BY t.tag COLLATE NOCASE",
        )
        .bind(user)
        .fetch_all(&self.inner.db)
        .await?)
    }

    pub async fn asset_tags(&self, user: &str, id: &str) -> Result<Vec<String>> {
        store::asset_row(&self.inner.db, user, id).await?;
        Ok(sqlx::query_scalar("SELECT tag FROM asset_tags WHERE asset_id = ? ORDER BY tag COLLATE NOCASE").bind(id).fetch_all(&self.inner.db).await?)
    }

    pub async fn tag_assets(&self, user: &str, ids: &[String], tag: &str, remove: bool) -> Result<()> {
        let tag = label(tag, 60, "tag")?;
        self.validate_asset_ids(user, ids).await?;
        let mut tx = self.inner.db.begin_with("BEGIN IMMEDIATE").await?;
        for id in ids {
            if remove {
                sqlx::query("DELETE FROM asset_tags WHERE asset_id = ? AND tag = ?").bind(id).bind(&tag).execute(&mut *tx).await?;
            } else {
                sqlx::query("INSERT OR IGNORE INTO asset_tags (asset_id, tag) SELECT id, ? FROM assets WHERE id = ? AND user_id = ?")
                    .bind(&tag)
                    .bind(id)
                    .bind(user)
                    .execute(&mut *tx)
                    .await?;
            }
        }
        tx.commit().await?;
        self.emit(user, "library", serde_json::json!({}));
        Ok(())
    }

    pub async fn bulk_assets(&self, user: &str, ids: &[String], action: &str) -> Result<()> {
        self.validate_asset_ids(user, ids).await?;
        let sql = match action {
            "favorite" => "UPDATE assets SET favorite = 1 WHERE id = ? AND user_id = ?",
            "unfavorite" => "UPDATE assets SET favorite = 0 WHERE id = ? AND user_id = ?",
            "trash" => "UPDATE assets SET deleted_at = CAST(strftime('%s', 'now') AS INTEGER) * 1000 WHERE id = ? AND user_id = ?",
            _ => return Err(EngineError::invalid("Choose favorite, unfavorite or trash.", Some("action"))),
        };
        let mut tx = self.inner.db.begin_with("BEGIN IMMEDIATE").await?;
        for id in ids {
            sqlx::query(sql).bind(id).bind(user).execute(&mut *tx).await?;
        }
        tx.commit().await?;
        for id in ids {
            self.emit(user, "asset", serde_json::json!(store::asset_row(&self.inner.db, user, id).await?.to_asset()));
        }
        Ok(())
    }

    pub async fn validate_asset_ids(&self, user: &str, ids: &[String]) -> Result<()> {
        if ids.is_empty() || ids.len() > MAX_ZIP_ASSETS {
            return Err(EngineError::invalid("Choose 1 to 100 assets.", Some("asset_ids")));
        }
        for id in ids {
            store::asset_row(&self.inner.db, user, id).await?;
        }
        Ok(())
    }

    /// Open originals before starting the response, with no media buffered in memory.
    pub async fn zip_entries(&self, user: &str, ids: &[String]) -> Result<Vec<ZipEntry>> {
        self.validate_asset_ids(user, ids).await?;
        let mut seen = HashSet::new();
        let mut entries = Vec::new();
        let mut total = 0u64;
        for id in ids.iter().filter(|id| seen.insert(id.as_str())) {
            let row = store::asset_row(&self.inner.db, user, id).await?;
            if row.deleted_at.is_some() {
                return Err(EngineError::invalid("Restore trashed assets before downloading.", Some("asset_ids")));
            }
            let file = tokio::fs::File::open(self.inner.files.path(&row.hash, &row.ext)).await.map_err(anyhow::Error::from)?;
            let size = file.metadata().await.map_err(anyhow::Error::from)?.len();
            total = total.saturating_add(size);
            if total > MAX_ZIP_BYTES {
                return Err(EngineError::invalid("ZIP downloads are limited to 1 GiB. Choose fewer assets.", Some("asset_ids")));
            }
            let title = row.name.as_deref().or(row.prompt.as_deref()).unwrap_or("pictesa");
            let title: String = title.chars().map(|c| if c.is_alphanumeric() || c == '-' || c == '_' { c } else { ' ' }).take(70).collect();
            let title = title.split_whitespace().collect::<Vec<_>>().join(" ");
            let title = if title.is_empty() { "pictesa" } else { &title };
            let name = format!("{}-{}.{}", title, id.to_lowercase(), row.ext);
            entries.push(ZipEntry { file, name, size });
        }
        Ok(entries)
    }
}
