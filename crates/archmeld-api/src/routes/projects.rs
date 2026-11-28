// Copyright 2026 ARCHMELD contributors
// SPDX-License-Identifier: Apache-2.0

//! `projects` HTTP handlers.

use crate::*;

pub(crate) async fn open_project(State(state): State<ApiState>, Json(body): Json<Value>) -> Result<Json<Value>, ApiError> {
    let object = body.as_object().ok_or_else(|| {
        ApiError::core(CoreError::invalid("openProject body must be an object"), "openProject")
    })?;
    let handle = object
        .get("workspace_handle")
        .and_then(Value::as_str)
        .ok_or_else(|| ApiError::core(CoreError::invalid("workspace_handle is required"), "openProject"))?;
    let mode = object
        .get("mode")
        .and_then(Value::as_str)
        .ok_or_else(|| ApiError::core(CoreError::invalid("mode is required"), "openProject"))?;
    let config = object
        .get("config")
        .cloned()
        .ok_or_else(|| ApiError::core(CoreError::invalid("config is required"), "openProject"))?;
    let idempotency_key = object
        .get("idempotency_key")
        .and_then(Value::as_str)
        .filter(|key| !key.is_empty())
        .ok_or_else(|| ApiError::core(CoreError::invalid("idempotency_key is required"), "openProject"))?;
    let root = state.workspaces.get(handle).cloned().ok_or_else(|| {
        ApiError::core(
            CoreError::Unsupported("permission_denied: unknown workspace_handle".into()),
            "openProject",
        )
    })?;
    let digest = hash::digest_value(&json!({"operation": "openProject", "body": body}))
        .map_err(|err| ApiError::core(CoreError::invalid(format!("request digest: {err}")), "openProject"))?;
    let mut store = lock_mut(&state)?;
    if let Some((recorded, status, result)) = store
        .idempotency_get(handle, "openProject", idempotency_key)
        .map_err(ApiError::core_err)?
    {
        if recorded != digest {
            return Err(ApiError::core(
                CoreError::Conflict(
                    "scope_conflict: idempotency_key was used with a different request body".into(),
                ),
                "openProject",
            ));
        }
        let result: Value = serde_json::from_str(&result).map_err(|err| {
            ApiError::core(CoreError::Storage(format!("stored result corrupt: {err}")), "openProject")
        })?;
        let _ = status;
        return Ok(Json(result));
    }
    let workspace_state_root = state.state_dir.join("workspaces").join(handle);
    let project = open_project_for_handle(&mut store, handle, &root, mode, &workspace_state_root, config)
        .map_err(ApiError::core_err)?;
    store
        .idempotency_put(handle, "openProject", idempotency_key, &digest, 200, &project)
        .map_err(ApiError::core_err)?;
    Ok(Json(project))
}

pub(crate) async fn get_project_handler(
    State(state): State<ApiState>,
    Path(project_id): Path<String>,
) -> Result<Json<Value>, ApiError> {
    let store = lock(&state)?;
    let project = get_project(&store, &project_id)
        .map_err(ApiError::core_err)?
        .ok_or_else(|| {
            ApiError::core(
                CoreError::not_found(format!("project {project_id} not found")),
                "getProject",
            )
        })?;
    Ok(Json(project))
}

pub(crate) async fn close_project_handler(
    State(state): State<ApiState>,
    Path(project_id): Path<String>,
    Json(body): Json<Value>,
) -> Result<Json<Value>, ApiError> {
    lifecycle_action(state, project_id, body, "closeProject", |store, project_id, revision| {
        close_project_checked(store, project_id, revision, true)
    })
}

pub(crate) async fn promote_project_handler(
    State(state): State<ApiState>,
    Path(project_id): Path<String>,
    Json(body): Json<Value>,
) -> Result<Json<Value>, ApiError> {
    lifecycle_action(state, project_id, body, "promoteProject", |store, project_id, revision| {
        promote_project_checked(store, project_id, revision)
    })
}

fn lifecycle_action<F>(
    state: ApiState,
    project_id: String,
    body: Value,
    operation: &'static str,
    action: F,
) -> Result<Json<Value>, ApiError>
where
    F: FnOnce(&mut Store, &str, i64) -> archmeld_core::Result<Value>,
{
    let object = body.as_object().ok_or_else(|| {
        ApiError::core(
            CoreError::invalid(format!("{operation} body must be an object")),
            operation,
        )
    })?;
    let expected_revision = object
        .get("expected_revision")
        .and_then(Value::as_i64)
        .ok_or_else(|| ApiError::core(CoreError::invalid("expected_revision is required"), operation))?;
    let idempotency_key = object
        .get("idempotency_key")
        .and_then(Value::as_str)
        .filter(|key| !key.is_empty())
        .ok_or_else(|| ApiError::core(CoreError::invalid("idempotency_key is required"), operation))?;
    let digest = hash::digest_value(&json!({
        "operation": operation,
        "project_id": project_id,
        "body": body,
    }))
    .map_err(|err| ApiError::core(CoreError::invalid(format!("request digest: {err}")), operation))?;
    let mut store = lock_mut(&state)?;
    if let Some((recorded, status, result)) = store
        .idempotency_get(&project_id, operation, idempotency_key)
        .map_err(ApiError::core_err)?
    {
        if recorded != digest {
            return Err(ApiError::core(
                CoreError::Conflict(
                    "scope_conflict: idempotency_key was used with a different request body".into(),
                ),
                operation,
            ));
        }
        let result: Value = serde_json::from_str(&result).map_err(|err| {
            ApiError::core(CoreError::Storage(format!("stored result corrupt: {err}")), operation)
        })?;
        let _ = status;
        return Ok(Json(result));
    }
    let project = action(&mut store, &project_id, expected_revision).map_err(ApiError::core_err)?;
    store
        .idempotency_put(&project_id, operation, idempotency_key, &digest, 200, &project)
        .map_err(ApiError::core_err)?;
    Ok(Json(project))
}
