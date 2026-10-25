use std::collections::HashMap;
use axum::{extract::{Path, Query, State}, http::StatusCode, Json};
use pictesa_engine::prompts::{PromptFilter, SavePrompt};
use serde::Deserialize;
use serde_json::{json, Value};
use crate::{ApiResult, AppState, Caller};

pub async fn history(State(s): State<AppState>, c: Caller, Query(f): Query<PromptFilter>) -> ApiResult<Json<Value>> {
    Ok(Json(json!({ "history": s.engine.prompt_history(&c.user_id, &f).await? })))
}
pub async fn list(State(s): State<AppState>, c: Caller, Query(f): Query<PromptFilter>) -> ApiResult<Json<Value>> {
    Ok(Json(json!({ "prompts": s.engine.saved_prompts(&c.user_id, &f).await? })))
}
pub async fn save(State(s): State<AppState>, c: Caller, Json(b): Json<SavePrompt>) -> ApiResult<(StatusCode, Json<Value>)> {
    Ok((StatusCode::CREATED, Json(json!(s.engine.save_prompt(&c.user_id, b).await?))))
}
#[derive(Deserialize)]
pub struct Name { name: String }
pub async fn rename(State(s): State<AppState>, c: Caller, Path(id): Path<String>, Json(b): Json<Name>) -> ApiResult<Json<Value>> {
    Ok(Json(json!(s.engine.rename_prompt(&c.user_id, &id, &b.name).await?)))
}
pub async fn delete(State(s): State<AppState>, c: Caller, Path(id): Path<String>) -> ApiResult<StatusCode> {
    s.engine.delete_prompt(&c.user_id, &id).await?;
    Ok(StatusCode::NO_CONTENT)
}
#[derive(Deserialize)]
pub struct Values { #[serde(default)] values: HashMap<String, String> }
pub async fn render(State(s): State<AppState>, c: Caller, Path(id): Path<String>, Json(b): Json<Values>) -> ApiResult<Json<Value>> {
    Ok(Json(json!(s.engine.use_prompt(&c.user_id, &id, b.values).await?)))
}
