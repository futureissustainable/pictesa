//! Board routes: the canvas document, its operations, step runs, sharing and presence.

use axum::{
    extract::{Path, State},
    http::StatusCode,
    response::{IntoResponse, Response},
    Json,
};
use pictesa_engine::{
    boards::{BoardRole, Op},
    library::AssetIds,
};
use serde::Deserialize;
use serde_json::{json, Value};

use crate::{bad, ApiResult, AppState, Caller};

#[derive(Deserialize)]
pub struct Name {
    name: String,
}

pub async fn list(State(s): State<AppState>, c: Caller) -> ApiResult<Json<Value>> {
    Ok(Json(json!({ "boards": s.engine.boards(&c.user_id).await? })))
}

pub async fn create(State(s): State<AppState>, c: Caller, Json(b): Json<Name>) -> ApiResult<(StatusCode, Json<Value>)> {
    Ok((StatusCode::CREATED, Json(json!(s.engine.create_board(&c.user_id, &b.name).await?))))
}

pub async fn get(State(s): State<AppState>, c: Caller, Path(id): Path<String>) -> ApiResult<Json<Value>> {
    Ok(Json(json!(s.engine.board(&c.user_id, &id).await?)))
}

pub async fn rename(State(s): State<AppState>, c: Caller, Path(id): Path<String>, Json(b): Json<Name>) -> ApiResult<Json<Value>> {
    Ok(Json(json!(s.engine.rename_board(&c.user_id, &id, &b.name).await?)))
}

pub async fn delete(State(s): State<AppState>, c: Caller, Path(id): Path<String>) -> ApiResult<StatusCode> {
    s.engine.delete_board(&c.user_id, &id).await?;
    Ok(StatusCode::NO_CONTENT)
}

/// Applies operations; a stale one answers 409 with the current state of everything they name.
pub async fn ops(State(s): State<AppState>, c: Caller, Path(id): Path<String>, Json(b): Json<Value>) -> ApiResult<Response> {
    let ops: Vec<Op> = serde_json::from_value(b.get("ops").cloned().unwrap_or(Value::Null)).map_err(|e| bad(&format!("Send {{ \"ops\": [...] }} with valid operations: {e}"), Some("ops")))?;
    Ok(match s.engine.apply_board_ops(&c.user_id, &id, ops).await? {
        Ok(changes) => Json(json!(changes)).into_response(),
        Err(stale) => (StatusCode::CONFLICT, Json(json!({ "error": stale.message, "kind": "conflict", "current": stale.current }))).into_response(),
    })
}

#[derive(Deserialize)]
pub struct RunBody {
    #[serde(default)]
    downstream: bool,
}

/// Body (optional): `{ "downstream": true }` also runs the steps that take this step's results.
pub async fn run(State(s): State<AppState>, c: Caller, Path((id, node)): Path<(String, String)>, body: axum::body::Bytes) -> ApiResult<(StatusCode, Json<Value>)> {
    let downstream = if body.is_empty() { false } else { serde_json::from_slice::<RunBody>(&body).map_err(|e| bad(&format!("Invalid run options: {e}"), None))?.downstream };
    Ok((StatusCode::CREATED, Json(json!(s.engine.run_board_step(&c.user_id, &id, &node, downstream).await?))))
}

pub async fn cancel(State(s): State<AppState>, c: Caller, Path((id, node)): Path<(String, String)>) -> ApiResult<Json<Value>> {
    Ok(Json(json!(s.engine.cancel_board_step(&c.user_id, &id, &node).await?)))
}

#[derive(Deserialize)]
pub struct ShareBody {
    email: String,
    role: BoardRole,
}

pub async fn share(State(s): State<AppState>, c: Caller, Path(id): Path<String>, Json(b): Json<ShareBody>) -> ApiResult<Json<Value>> {
    Ok(Json(json!({ "members": s.engine.share_board(&c.user_id, &id, &b.email, b.role).await? })))
}

pub async fn remove_member(State(s): State<AppState>, c: Caller, Path((id, user)): Path<(String, String)>) -> ApiResult<StatusCode> {
    s.engine.remove_board_member(&c.user_id, &id, &user).await?;
    Ok(StatusCode::NO_CONTENT)
}

#[derive(Deserialize)]
pub struct PresenceBody {
    client: String,
    #[serde(default)]
    leave: bool,
}

pub async fn presence(State(s): State<AppState>, c: Caller, Path(id): Path<String>, Json(b): Json<PresenceBody>) -> ApiResult<Json<Value>> {
    Ok(Json(json!({ "viewers": s.engine.board_presence(&c.user_id, &id, &b.client, b.leave).await? })))
}

pub async fn copy(State(s): State<AppState>, c: Caller, Path(id): Path<String>, Json(b): Json<AssetIds>) -> ApiResult<Json<Value>> {
    Ok(Json(json!({ "assets": s.engine.copy_board_assets(&c.user_id, &id, &b.asset_ids).await? })))
}
