// Copyright 2026 ARCHMELD contributors
// SPDX-License-Identifier: Apache-2.0

//! `runtime` HTTP handlers.

use crate::*;

#[derive(Deserialize)]
pub(crate) struct RuntimeQuery {
    project_id: Option<String>,
    limit: Option<usize>,
}

pub(crate) async fn runtime_timeline_handler(
    State(state): State<ApiState>,
    Path(run_id): Path<String>,
    Query(params): Query<RuntimeQuery>,
) -> Result<Json<Value>, ApiError> {
    let project_id = params.project_id.ok_or_else(|| {
        ApiError::core(CoreError::invalid("project_id is required"), "getRuntimeTimeline")
    })?;
    let store = lock(&state)?;
    let value = timeline(
        &store,
        &project_id,
        &run_id,
        None,
        None,
        params.limit.unwrap_or(1000),
    )
    .map_err(ApiError::core_err)?;
    Ok(Json(value))
}

pub(crate) async fn runtime_summary_handler(
    State(state): State<ApiState>,
    Path(run_id): Path<String>,
    Query(params): Query<RuntimeQuery>,
) -> Result<Json<Value>, ApiError> {
    let project_id = params.project_id.ok_or_else(|| {
        ApiError::core(CoreError::invalid("project_id is required"), "getRuntimeSummary")
    })?;
    let store = lock(&state)?;
    let value = runtime_summary(&store, &project_id, &run_id).map_err(ApiError::core_err)?;
    Ok(Json(value))
}
