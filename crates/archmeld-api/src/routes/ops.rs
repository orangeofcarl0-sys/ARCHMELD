// Copyright 2026 ARCHMELD contributors
// SPDX-License-Identifier: Apache-2.0

//! `ops` HTTP handlers.

use crate::*;

pub(crate) async fn instigator_configure_handler(
    State(state): State<ApiState>,
    Json(body): Json<Value>,
) -> Result<Json<Value>, ApiError> {
    let mut store = lock_mut(&state)?;
    let status = configure_instigator(&mut store, &body).map_err(ApiError::core_err)?;
    Ok(Json(status))
}

pub(crate) async fn instigator_nudge_handler(
    State(state): State<ApiState>,
    Json(body): Json<Value>,
) -> Result<Json<Value>, ApiError> {
    let mut store = lock_mut(&state)?;
    let decision = propose_nudge(&mut store, &body).map_err(ApiError::core_err)?;
    Ok(Json(decision))
}

pub(crate) async fn instigator_deliver_handler(
    State(state): State<ApiState>,
    Json(body): Json<Value>,
) -> Result<Json<Value>, ApiError> {
    let mut store = lock_mut(&state)?;
    let delivered = deliver_due(&mut store, &body).map_err(ApiError::core_err)?;
    Ok(Json(delivered))
}

pub(crate) async fn instigator_status_handler(
    State(state): State<ApiState>,
    Query(params): Query<ProjectQuery>,
) -> Result<Json<Value>, ApiError> {
    let project_id = params.project_id.ok_or_else(|| {
        ApiError::core(CoreError::invalid("project_id is required"), "getInstigator")
    })?;
    let store = lock(&state)?;
    let status = instigator_status(&store, &project_id).map_err(ApiError::core_err)?;
    Ok(Json(status))
}

pub(crate) async fn record_drift_handler(
    State(state): State<ApiState>,
    Json(body): Json<Value>,
) -> Result<Json<Value>, ApiError> {
    let mut store = lock_mut(&state)?;
    let report = record_drift(&mut store, &body).map_err(ApiError::core_err)?;
    Ok(Json(report))
}

pub(crate) async fn list_drift_handler(
    State(state): State<ApiState>,
    Query(params): Query<ProjectQuery>,
) -> Result<Json<Value>, ApiError> {
    let project_id = params.project_id.ok_or_else(|| {
        ApiError::core(CoreError::invalid("project_id is required"), "listDrift")
    })?;
    let store = lock(&state)?;
    let reports = list_drift_reports(&store, &project_id).map_err(ApiError::core_err)?;
    Ok(Json(json!({"project_id": project_id, "reports": reports})))
}

pub(crate) async fn create_backup_handler(
    State(state): State<ApiState>,
    Json(body): Json<Value>,
) -> Result<Json<Value>, ApiError> {
    let project_id = body["project_id"].as_str().ok_or_else(|| {
        ApiError::core(CoreError::invalid("project_id is required"), "createBackup")
    })?;
    let destination = state.state_dir.join("backups").join(format!(
        "backup-{}",
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs()
    ));
    let mut store = lock_mut(&state)?;
    let backup = create_backup(&mut store, project_id, &destination).map_err(ApiError::core_err)?;
    Ok(Json(backup))
}

pub(crate) async fn gc_handler(
    State(state): State<ApiState>,
    Json(body): Json<Value>,
) -> Result<Json<Value>, ApiError> {
    let dry_run = body.get("dry_run").and_then(Value::as_bool).unwrap_or(true);
    let mut store = lock_mut(&state)?;
    let result = gc_objects(&mut store, dry_run).map_err(ApiError::core_err)?;
    Ok(Json(result))
}

pub(crate) async fn export_report_handler(
    State(state): State<ApiState>,
    Json(mut body): Json<Value>,
) -> Result<Json<Value>, ApiError> {
    let output_name = body["output_name"]
        .as_str()
        .filter(|name| {
            !name.is_empty()
                && name.len() <= 128
                && name
                    .bytes()
                    .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'-' | b'_'))
                && !name.contains("..")
        })
        .ok_or_else(|| {
            ApiError::core(
                CoreError::invalid("output_name must be a safe bundle name"),
                "exportReport",
            )
        })?
        .to_string();
    let destination = state.state_dir.join("exports").join(output_name);
    let object = body.as_object_mut().ok_or_else(|| {
        ApiError::core(
            CoreError::invalid("report body must be an object"),
            "exportReport",
        )
    })?;
    object.remove("output_name");
    object.insert(
        "output_dir".to_string(),
        json!(destination.to_string_lossy()),
    );
    let mut store = lock_mut(&state)?;
    let report = export_report(&mut store, &body).map_err(ApiError::core_err)?;
    Ok(Json(report))
}

pub(crate) async fn configure_retention_handler(
    State(state): State<ApiState>,
    Json(body): Json<Value>,
) -> Result<Json<Value>, ApiError> {
    let mut store = lock_mut(&state)?;
    let status = configure_retention(&mut store, &body).map_err(ApiError::core_err)?;
    Ok(Json(status))
}

pub(crate) async fn retention_status_handler(
    State(state): State<ApiState>,
    Query(params): Query<ProjectQuery>,
) -> Result<Json<Value>, ApiError> {
    let project_id = params.project_id.ok_or_else(|| {
        ApiError::core(
            CoreError::invalid("project_id is required"),
            "retentionStatus",
        )
    })?;
    let store = lock(&state)?;
    let status = retention_status(&store, &project_id).map_err(ApiError::core_err)?;
    Ok(Json(status))
}

pub(crate) async fn prune_retention_handler(
    State(state): State<ApiState>,
    Json(body): Json<Value>,
) -> Result<Json<Value>, ApiError> {
    let mut store = lock_mut(&state)?;
    let result = prune_snapshots(&mut store, &body).map_err(ApiError::core_err)?;
    Ok(Json(result))
}

pub(crate) async fn list_retention_runs_handler(
    State(state): State<ApiState>,
    Query(params): Query<ProjectQuery>,
) -> Result<Json<Value>, ApiError> {
    let project_id = params.project_id.ok_or_else(|| {
        ApiError::core(
            CoreError::invalid("project_id is required"),
            "listRetentionRuns",
        )
    })?;
    let store = lock(&state)?;
    let runs = list_retention_runs(&store, &project_id).map_err(ApiError::core_err)?;
    Ok(Json(json!({"project_id": project_id, "runs": runs})))
}
pub(crate) async fn project_lease_handler(
    State(state): State<ApiState>,
    Path(project_id): Path<String>,
) -> Result<Json<Value>, ApiError> {
    let store = lock(&state)?;
    let lease = writer_lease(&store, &project_id).map_err(ApiError::core_err)?;
    Ok(Json(json!({"project_id": project_id, "lease": lease})))
}

pub(crate) async fn interrupted_handler(
    State(state): State<ApiState>,
    Path(project_id): Path<String>,
) -> Result<Json<Value>, ApiError> {
    let store = lock(&state)?;
    let items = interrupted_changesets(&store, &project_id).map_err(ApiError::core_err)?;
    Ok(Json(json!({"project_id": project_id, "interrupted": items})))
}
