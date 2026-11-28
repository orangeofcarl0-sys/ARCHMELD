// Copyright 2026 ARCHMELD contributors
// SPDX-License-Identifier: Apache-2.0

//! `query` HTTP handlers.

use crate::*;

pub(crate) async fn query(State(state): State<ApiState>, Json(body): Json<Value>) -> Result<Json<Value>, ApiError> {
    let store = lock(&state)?;
    run_query(&store, &body).map(Json).map_err(ApiError::core_err)
}

#[derive(Deserialize)]
pub(crate) struct EntityQuery {
    project_id: Option<String>,
    snapshot_id: Option<String>,
}

pub(crate) async fn get_entity(
    State(state): State<ApiState>,
    Path(entity_id): Path<String>,
    Query(params): Query<EntityQuery>,
) -> Result<Json<Value>, ApiError> {
    let project_id = params
        .project_id
        .ok_or_else(|| ApiError::core(CoreError::invalid("project_id is required"), "getEntity"))?;
    let snapshot_id = params
        .snapshot_id
        .ok_or_else(|| ApiError::core(CoreError::invalid("snapshot_id is required"), "getEntity"))?;
    let store = lock(&state)?;
    let entity = store
        .get_entity(&project_id, &snapshot_id, &entity_id)
        .map_err(ApiError::core_err)?
        .ok_or_else(|| ApiError::core(CoreError::not_found(format!("entity {entity_id} not found")), "getEntity"))?;
    Ok(Json(entity))
}

pub(crate) async fn source(State(state): State<ApiState>, Json(body): Json<Value>) -> Result<Json<Value>, ApiError> {
    let store = lock(&state)?;
    read_source(&store, &body).map(Json).map_err(ApiError::core_err)
}

pub(crate) async fn source_file(
    State(state): State<ApiState>,
    Json(body): Json<Value>,
) -> Result<Json<Value>, ApiError> {
    let store = lock(&state)?;
    read_source_file(&store, &body)
        .map(Json)
        .map_err(ApiError::core_err)
}
