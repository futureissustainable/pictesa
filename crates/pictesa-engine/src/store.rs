//! Row types and the queries that turn them into API shapes.

use pictesa_core::MediaKind;
use serde_json::{Map, Value};
use sqlx::{FromRow, SqlitePool};

use crate::{Asset, Job, JobError, JobInput, JobStatus, Result};

#[derive(Debug, Clone, FromRow)]
pub struct JobRow {
    pub id: String,
    pub user_id: String,
    pub batch_id: String,
    pub model: String,
    pub provider: String,
    pub model_name: String,
    pub output: String,
    pub status: String,
    pub prompt: String,
    pub negative_prompt: Option<String>,
    pub params: String,
    pub inputs: String,
    pub preset: Option<String>,
    pub count: i64,
    pub remote: Option<String>,
    pub progress: Option<f64>,
    pub message: Option<String>,
    pub error_kind: Option<String>,
    pub error: Option<String>,
    pub cost_usd: Option<f64>,
    pub attempts: i64,
    #[sqlx(default)]
    pub billing: String,
    #[sqlx(default)]
    pub submitting: i64,
    #[sqlx(default)]
    pub owner: Option<String>,
    pub created_at: i64,
    pub started_at: Option<i64>,
    pub finished_at: Option<i64>,
    pub updated_at: i64,
}

impl JobRow {
    pub fn status(&self) -> JobStatus {
        JobStatus::parse(&self.status)
    }
    pub fn params_map(&self) -> Map<String, Value> {
        serde_json::from_str(&self.params).unwrap_or_default()
    }
    pub fn inputs_list(&self) -> Vec<JobInput> {
        serde_json::from_str(&self.inputs).unwrap_or_default()
    }
    pub fn output_kind(&self) -> MediaKind {
        parse_kind(&self.output)
    }
    pub fn to_job(&self, outputs: Vec<Asset>) -> Job {
        Job {
            id: self.id.clone(),
            batch_id: self.batch_id.clone(),
            model: self.model.clone(),
            provider: self.provider.clone(),
            model_name: self.model_name.clone(),
            output: self.output_kind(),
            status: self.status(),
            prompt: self.prompt.clone(),
            negative_prompt: self.negative_prompt.clone(),
            params: self.params_map(),
            inputs: self.inputs_list(),
            preset: self.preset.clone(),
            count: self.count.max(1) as u32,
            progress: self.progress,
            message: self.message.clone(),
            error: self.error.as_ref().map(|m| JobError { kind: self.error_kind.clone().unwrap_or_else(|| "failed".into()), message: m.clone() }),
            cost_usd: self.cost_usd,
            attempts: self.attempts,
            created_at: self.created_at,
            started_at: self.started_at,
            finished_at: self.finished_at,
            outputs,
        }
    }
}

#[derive(Debug, Clone, FromRow)]
pub struct AssetRow {
    pub id: String,
    pub user_id: String,
    pub job_id: Option<String>,
    pub kind: String,
    pub mime: String,
    pub hash: String,
    pub ext: String,
    pub size: i64,
    pub width: Option<i64>,
    pub height: Option<i64>,
    pub duration_ms: Option<i64>,
    pub has_thumb: bool,
    pub source: String,
    pub model: Option<String>,
    pub prompt: Option<String>,
    pub favorite: bool,
    pub source_url: Option<String>,
    pub created_at: i64,
    pub deleted_at: Option<i64>,
    #[sqlx(default)]
    pub name: Option<String>,
}

impl AssetRow {
    pub fn to_asset(&self) -> Asset {
        Asset {
            id: self.id.clone(),
            kind: parse_kind(&self.kind),
            mime: self.mime.clone(),
            width: self.width,
            height: self.height,
            duration_ms: self.duration_ms,
            size: self.size,
            url: format!("/files/{}", self.id),
            thumb_url: self.has_thumb.then(|| format!("/thumbs/{}", self.id)),
            source: self.source.clone(),
            job_id: self.job_id.clone(),
            model: self.model.clone(),
            prompt: self.prompt.clone(),
            favorite: self.favorite,
            created_at: self.created_at,
            deleted_at: self.deleted_at,
            name: self.name.clone(),
        }
    }
}

pub fn parse_kind(s: &str) -> MediaKind {
    match s {
        "video" => MediaKind::Video,
        "audio" => MediaKind::Audio,
        _ => MediaKind::Image,
    }
}

pub async fn job_row(db: &SqlitePool, user: &str, id: &str) -> Result<JobRow> {
    Ok(sqlx::query_as::<_, JobRow>("SELECT * FROM jobs WHERE id = ? AND user_id = ?").bind(id).bind(user).fetch_one(db).await?)
}

pub async fn job_row_any(db: &SqlitePool, id: &str) -> Result<JobRow> {
    Ok(sqlx::query_as::<_, JobRow>("SELECT * FROM jobs WHERE id = ?").bind(id).fetch_one(db).await?)
}

pub async fn outputs_of(db: &SqlitePool, job_id: &str) -> Result<Vec<Asset>> {
    let rows = sqlx::query_as::<_, AssetRow>("SELECT * FROM assets WHERE job_id = ? AND deleted_at IS NULL ORDER BY id")
        .bind(job_id)
        .fetch_all(db)
        .await?;
    Ok(rows.iter().map(AssetRow::to_asset).collect())
}

pub async fn job_view(db: &SqlitePool, row: &JobRow) -> Result<Job> {
    Ok(row.to_job(outputs_of(db, &row.id).await?))
}

pub async fn asset_row(db: &SqlitePool, user: &str, id: &str) -> Result<AssetRow> {
    Ok(sqlx::query_as::<_, AssetRow>("SELECT * FROM assets WHERE id = ? AND user_id = ?").bind(id).bind(user).fetch_one(db).await?)
}

/// Stores one value of the `settings` table (per user, or `system` for the server's own).
pub async fn put_setting(db: &SqlitePool, user: &str, key: &str, value: &str) -> Result<()> {
    sqlx::query("INSERT INTO settings (user_id, key, value) VALUES (?, ?, ?) ON CONFLICT (user_id, key) DO UPDATE SET value = excluded.value")
        .bind(user)
        .bind(key)
        .bind(value)
        .execute(db)
        .await?;
    Ok(())
}

/// Builds an FTS5 query from free text: every word must appear (prefix match).
pub fn fts_query(q: &str) -> Option<String> {
    let words: Vec<String> = q
        .split(|c: char| !c.is_alphanumeric())
        .filter(|w| !w.is_empty())
        .take(8)
        .map(|w| format!("\"{}\"*", w.replace('"', "")))
        .collect();
    (!words.is_empty()).then(|| words.join(" AND "))
}
