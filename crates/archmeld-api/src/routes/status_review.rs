// Copyright 2026 ARCHMELD contributors
// SPDX-License-Identifier: Apache-2.0

//! `status_review` HTTP handlers.

use crate::*;

pub(crate) async fn health() -> Json<Value> {
    Json(json!({
        "service": "archmeld",
        "contract_version": CONTRACT_VERSION,
        "status": "ready",
        "product_version": PRODUCT_VERSION,
    }))
}

#[derive(Deserialize)]
pub(crate) struct StatusQuery {
    project_id: Option<String>,
    workspace_handle: Option<String>,
}

pub(crate) async fn status_handler(
    State(state): State<ApiState>,
    Query(params): Query<StatusQuery>,
) -> Result<Json<Value>, ApiError> {
    let workspace = match params.workspace_handle {
        Some(handle) => Some(state.workspaces.get(&handle).cloned().ok_or_else(|| {
            ApiError::core(
                CoreError::Unsupported(
                    "permission_denied: workspace_handle is not authorized".into(),
                ),
                "status",
            )
        })?),
        None => None,
    };
    let store = lock(&state)?;
    status_report(&store, params.project_id.as_deref(), workspace.as_deref())
        .map(Json)
        .map_err(ApiError::core_err)
}

pub(crate) async fn review_workspace_diff_handler(
    State(state): State<ApiState>,
    Json(body): Json<Value>,
) -> Result<Json<Value>, ApiError> {
    let project_id = body["project_id"].as_str().ok_or_else(|| {
        ApiError::core(
            CoreError::invalid("project_id is required"),
            "reviewWorkspaceDiff",
        )
    })?;
    let handle = body["workspace_handle"].as_str().ok_or_else(|| {
        ApiError::core(
            CoreError::invalid("workspace_handle is required"),
            "reviewWorkspaceDiff",
        )
    })?;
    let root = state.workspaces.get(handle).cloned().ok_or_else(|| {
        ApiError::core(
            CoreError::Unsupported("permission_denied: workspace_handle is not authorized".into()),
            "reviewWorkspaceDiff",
        )
    })?;
    let store = lock(&state)?;
    let base_snapshot_id = match body["base_snapshot_id"].as_str() {
        Some(snapshot_id) if !snapshot_id.is_empty() => snapshot_id.to_string(),
        _ => store
            .latest_snapshot_id(project_id)
            .map_err(ApiError::core_err)?
            .ok_or_else(|| {
                ApiError::core(
                    CoreError::not_found("project has no snapshot to review against"),
                    "reviewWorkspaceDiff",
                )
            })?,
    };
    review_workspace_diff(&store, project_id, &base_snapshot_id, &root)
        .map(Json)
        .map_err(ApiError::core_err)
}
pub(crate) async fn capabilities_handler() -> Json<Value> {
    Json(provider_capabilities())
}
