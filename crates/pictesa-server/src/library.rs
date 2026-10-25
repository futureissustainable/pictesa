use axum::{
    extract::{Path, State},
    http::{header, StatusCode},
    response::{IntoResponse, Response},
    Json,
};
use pictesa_engine::{library::AssetIds, spend::BudgetInput};
use serde::Deserialize;
use serde_json::{json, Value};

use crate::{ApiResult, AppState, Caller};

#[derive(Deserialize)]
pub struct Name {
    name: String,
}

pub async fn collections(State(s): State<AppState>, c: Caller) -> ApiResult<Json<Value>> {
    Ok(Json(json!({ "collections": s.engine.collections(&c.user_id).await? })))
}
pub async fn create_collection(State(s): State<AppState>, c: Caller, Json(b): Json<Name>) -> ApiResult<(StatusCode, Json<Value>)> {
    Ok((StatusCode::CREATED, Json(json!(s.engine.create_collection(&c.user_id, &b.name).await?))))
}
pub async fn rename_collection(State(s): State<AppState>, c: Caller, Path(id): Path<String>, Json(b): Json<Name>) -> ApiResult<StatusCode> {
    s.engine.rename_collection(&c.user_id, &id, &b.name).await?;
    Ok(StatusCode::NO_CONTENT)
}
pub async fn delete_collection(State(s): State<AppState>, c: Caller, Path(id): Path<String>) -> ApiResult<StatusCode> {
    s.engine.delete_collection(&c.user_id, &id).await?;
    Ok(StatusCode::NO_CONTENT)
}
pub async fn add_to_collection(State(s): State<AppState>, c: Caller, Path(id): Path<String>, Json(b): Json<AssetIds>) -> ApiResult<StatusCode> {
    s.engine.collection_assets(&c.user_id, &id, &b.asset_ids, false).await?;
    Ok(StatusCode::NO_CONTENT)
}
pub async fn remove_from_collection(State(s): State<AppState>, c: Caller, Path(id): Path<String>, Json(b): Json<AssetIds>) -> ApiResult<StatusCode> {
    s.engine.collection_assets(&c.user_id, &id, &b.asset_ids, true).await?;
    Ok(StatusCode::NO_CONTENT)
}
pub async fn tags(State(s): State<AppState>, c: Caller) -> ApiResult<Json<Value>> {
    Ok(Json(json!({ "tags": s.engine.tags(&c.user_id).await? })))
}
pub async fn asset_tags(State(s): State<AppState>, c: Caller, Path(id): Path<String>) -> ApiResult<Json<Value>> {
    Ok(Json(json!({ "tags": s.engine.asset_tags(&c.user_id, &id).await? })))
}
#[derive(Deserialize)]
pub struct TagBody {
    asset_ids: Vec<String>,
    tag: String,
    #[serde(default)]
    remove: bool,
}
pub async fn tag_assets(State(s): State<AppState>, c: Caller, Json(b): Json<TagBody>) -> ApiResult<StatusCode> {
    s.engine.tag_assets(&c.user_id, &b.asset_ids, &b.tag, b.remove).await?;
    Ok(StatusCode::NO_CONTENT)
}
#[derive(Deserialize)]
pub struct BulkBody {
    asset_ids: Vec<String>,
    action: String,
}
pub async fn bulk(State(s): State<AppState>, c: Caller, Json(b): Json<BulkBody>) -> ApiResult<StatusCode> {
    s.engine.bulk_assets(&c.user_id, &b.asset_ids, &b.action).await?;
    Ok(StatusCode::NO_CONTENT)
}
pub async fn download(State(s): State<AppState>, c: Caller, Json(b): Json<AssetIds>) -> ApiResult<Response> {
    let entries = s.engine.zip_entries(&c.user_id, &b.asset_ids).await?;
    Ok((
        [
            (header::CONTENT_TYPE, "application/zip"),
            (header::CONTENT_DISPOSITION, "attachment; filename=\"pictesa-library.zip\""),
            (header::CACHE_CONTROL, "private, no-store"),
        ],
        crate::zip::body(entries),
    )
        .into_response())
}
pub async fn spend(State(s): State<AppState>, c: Caller) -> ApiResult<Json<Value>> {
    Ok(Json(json!(s.engine.spend_view(&c.user_id).await?)))
}
pub async fn budgets(State(s): State<AppState>, c: Caller, Json(b): Json<BudgetInput>) -> ApiResult<Json<Value>> {
    Ok(Json(json!(s.engine.set_budgets(&c.user_id, b).await?)))
}
