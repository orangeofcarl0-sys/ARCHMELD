// Copyright 2026 ARCHMELD contributors
// SPDX-License-Identifier: Apache-2.0

//! ARCHMELD local HTTP API (S11).
//!
//! One core, one state: every route goes through the same `Store` and the same
//! permission/state rules. The server binds loopback only; when a token is
//! configured, every non-health route requires `Authorization: Bearer`.

use archmeld_core::error::CoreError;
use archmeld_core::apply::{
    apply_changeset, interrupted_changesets, recover_changeset, writer_lease,
};
use archmeld_core::blueprint::{check_architecture_rules, diff_architecture};
use archmeld_core::changeset::{
    approve_prepared_change, changeset_diff, create_changeset, get_changeset, list_changesets,
    prepare_changeset, reject_changeset,
};
use archmeld_core::drift::{
    create_backup, gc as gc_objects, list_drift_reports, record_drift,
};
use archmeld_core::events::events_after;
use archmeld_core::hash;
use archmeld_core::interpret::{
    architecture_review, get_architecture, get_claim, get_investigation, import_codeboarding,
    list_architecture_reviews, propose_architecture, propose_claim, propose_investigation,
    provider_capabilities, review_architecture, review_claim, CodeBoardingRequest, ModelProvenance,
};
use archmeld_core::instigator::{configure as configure_instigator, deliver_due, propose_nudge, status as instigator_status};
use archmeld_core::jobs::{cancel_job_idempotent, get_job};
use archmeld_core::plan::{approve_plan, get_contract, get_plan, propose_contract, propose_plan};
use archmeld_core::manager::{
    close_project_checked, get_project, open_project_for_handle, promote_project_checked,
};
use archmeld_core::query::run_query;
use archmeld_core::report::export_report;
use archmeld_core::scenario::{
    get_scenario_run, run_scenario, ExecutionGrant, ScenarioSandbox, ScenarioSpec,
};
use archmeld_core::retention::{
    configure_retention, list_retention_runs, prune_snapshots, retention_status,
};
use archmeld_core::runtime::{runtime_summary, timeline};
use archmeld_core::source_api::{read_source, read_source_file};
use archmeld_core::store::Store;
use archmeld_core::verification::{
    accept_changeset, get_verification, list_verifications, verify_changeset,
};
use axum::body::Body;
use axum::extract::{Path, Query, Request, State};
use axum::http::{header, HeaderValue, StatusCode};
use axum::middleware::{self, Next};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Json, Router};
use serde::Deserialize;
use serde_json::{json, Value};
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

pub const CONTRACT_VERSION: &str = "1.0";
pub const PRODUCT_VERSION: &str = env!("CARGO_PKG_VERSION");
const MAX_BODY_BYTES: usize = 16 * 1024 * 1024;

#[derive(Clone)]
pub struct ApiState {
    pub store: Arc<Mutex<Store>>,
    pub token: Option<String>,
    /// Trusted allowlist: workspace handle -> canonical root.
    pub workspaces: HashMap<String, PathBuf>,
    pub state_dir: PathBuf,
}

pub fn router(state: ApiState) -> Router {
    let shared = state.clone();
    Router::new()
        .route("/v1/health", get(health))
        .route("/v1/scenarios", post(run_scenario_handler))
        .route("/v1/scenarios/{run_id}", get(get_scenario_handler))
        .route("/v1/projects/open", post(open_project))
        .route("/v1/projects/{project_id}", get(get_project_handler))
        .route("/v1/projects/{project_id}/close", post(close_project_handler))
        .route("/v1/projects/{project_id}/promote", post(promote_project_handler))
        .route("/v1/query", post(query))
        .route("/v1/entities/{entity_id}", get(get_entity))
        .route("/v1/source", post(source))
        .route("/v1/source/file", post(source_file))
        .route("/v1/runtime/{run_id}", get(runtime_timeline_handler))
        .route("/v1/runtime/{run_id}/summary", get(runtime_summary_handler))
        .route("/v1/jobs/{job_id}", get(job))
        .route("/v1/jobs/{job_id}/cancel", post(cancel))
        .route("/v1/events", get(events))
        .route("/v1/claims", post(propose_claim_handler))
        .route("/v1/claims/{claim_id}", get(get_claim_handler))
        .route("/v1/claims/{claim_id}/review", post(review_claim_handler))
        .route("/v1/investigations", post(propose_investigation_handler))
        .route("/v1/investigations/{investigation_id}", get(get_investigation_handler))
        .route("/v1/architectures", post(propose_architecture_handler))
        .route("/v1/architectures/codeboarding", post(import_codeboarding_handler))
        .route("/v1/architectures/{architecture_id}", get(get_architecture_handler))
        .route(
            "/v1/architectures/{architecture_id}/review",
            get(architecture_review_handler).post(review_architecture_handler),
        )
        .route(
            "/v1/architectures/{architecture_id}/reviews",
            get(list_architecture_reviews_handler),
        )
        .route("/v1/architectures/{architecture_id}/diff", post(architecture_diff_handler))
        .route("/v1/architectures/{architecture_id}/check", post(architecture_check_handler))
        .route("/v1/capabilities", get(capabilities_handler))
        .route("/v1/contracts", post(propose_contract_handler))
        .route("/v1/contracts/{contract_id}", get(get_contract_handler))
        .route("/v1/plans", post(propose_plan_handler))
        .route("/v1/plans/{plan_id}", get(get_plan_handler))
        .route("/v1/plans/{plan_id}/approve", post(approve_plan_handler))
        .route("/v1/changesets", get(list_changesets_handler).post(create_changeset_handler))
        .route("/v1/changesets/{changeset_id}", get(get_changeset_handler))
        .route("/v1/changesets/{changeset_id}/prepare", post(prepare_changeset_handler))
        .route("/v1/changesets/{changeset_id}/approve", post(approve_changeset_handler))
        .route("/v1/changesets/{changeset_id}/reject", post(reject_changeset_handler))
        .route("/v1/changesets/{changeset_id}/diff", get(changeset_diff_handler))
        .route("/v1/changesets/{changeset_id}/apply", post(apply_changeset_handler))
        .route("/v1/changesets/{changeset_id}/recover", post(recover_changeset_handler))
        .route("/v1/changesets/{changeset_id}/verify", post(verify_changeset_handler))
        .route("/v1/changesets/{changeset_id}/accept", post(accept_changeset_handler))
        .route("/v1/verifications", get(list_verifications_handler))
        .route("/v1/verifications/{verification_id}", get(get_verification_handler))
        .route("/v1/projects/{project_id}/lease", get(project_lease_handler))
        .route("/v1/instigator/configure", post(instigator_configure_handler))
        .route("/v1/instigator/nudge", post(instigator_nudge_handler))
        .route("/v1/instigator/deliver", post(instigator_deliver_handler))
        .route("/v1/instigator", get(instigator_status_handler))
        .route("/v1/drift", get(list_drift_handler).post(record_drift_handler))
        .route("/v1/backups", post(create_backup_handler))
        .route("/v1/gc", post(gc_handler))
        .route("/v1/reports", post(export_report_handler))
        .route(
            "/v1/retention",
            get(retention_status_handler).post(configure_retention_handler),
        )
        .route("/v1/retention/prune", post(prune_retention_handler))
        .route("/v1/retention/runs", get(list_retention_runs_handler))
        .route("/v1/projects/{project_id}/interrupted", get(interrupted_handler))
        .layer(middleware::from_fn_with_state(shared, auth))
        .layer(axum::extract::DefaultBodyLimit::max(MAX_BODY_BYTES))
        .with_state(state)
}

async fn auth(
    State(state): State<ApiState>,
    request: Request,
    next: Next,
) -> Response {
    // A local HTTP service is not a browser security boundary by itself. Only
    // loopback Host values are accepted, and browser requests carrying any
    // Origin are rejected so a third-party page cannot use a DNS-rebinding or
    // form-post path against the API.
    let host_ok = request
        .headers()
        .get(header::HOST)
        .and_then(|value| value.to_str().ok())
        .map(host_is_loopback)
        .unwrap_or(false);
    if !host_ok {
        return ApiError::core(
            CoreError::Unsupported("permission_denied: Host must be loopback".into()),
            "host",
        )
        .into_response();
    }
    if request.headers().contains_key(header::ORIGIN) {
        return ApiError::core(
            CoreError::Unsupported(
                "permission_denied: browser Origin requests are not allowed; use the trusted local client"
                    .into(),
            ),
            "origin",
        )
        .into_response();
    }
    if request.uri().path() == "/v1/health" {
        return next.run(request).await;
    }
    if let Some(expected) = &state.token {
        let supplied = request
            .headers()
            .get(header::AUTHORIZATION)
            .and_then(|value| value.to_str().ok())
            .and_then(|value| value.strip_prefix("Bearer "));
        if supplied != Some(expected.as_str()) {
            return ApiError::core(
                CoreError::Unsupported("permission_denied: missing or invalid bearer token".into()),
                "auth",
            )
            .into_response();
        }
    }
    next.run(request).await
}

async fn run_scenario_handler(
    State(state): State<ApiState>,
    Json(body): Json<Value>,
) -> Result<Json<Value>, ApiError> {
    let project_id = body["project_id"]
        .as_str()
        .ok_or_else(|| ApiError::core(CoreError::invalid("project_id is required"), "runScenario"))?;
    let snapshot_id = body["snapshot_id"]
        .as_str()
        .ok_or_else(|| ApiError::core(CoreError::invalid("snapshot_id is required"), "runScenario"))?;
    let handle = body["workspace_handle"]
        .as_str()
        .ok_or_else(|| ApiError::core(CoreError::invalid("workspace_handle is required"), "runScenario"))?;
    let root = state.workspaces.get(handle).cloned().ok_or_else(|| {
        ApiError::core(
            CoreError::Unsupported("permission_denied: workspace_handle is not authorized".into()),
            "runScenario",
        )
    })?;
    let cwd = body["cwd"]
        .as_str()
        .map(PathBuf::from)
        .unwrap_or_else(|| root.clone());
    let cwd = std::fs::canonicalize(&cwd).map_err(|error| {
        ApiError::core(CoreError::invalid(format!("command cwd: {error}")), "runScenario")
    })?;
    if !cwd.starts_with(&root) {
        return Err(ApiError::core(
            CoreError::invalid("command cwd resolves outside the authorized workspace"),
            "runScenario",
        ));
    }
    let mut sandbox: Option<ScenarioSandbox> = match body.get("sandbox") {
        None | Some(Value::Null) => None,
        Some(value) => Some(serde_json::from_value(value.clone()).map_err(|error| {
            ApiError::core(CoreError::invalid(format!("sandbox: {error}")), "runScenario")
        })?),
    };
    if let Some(sandbox) = &mut sandbox {
        if sandbox.mode != "workspace" && sandbox.cwd.is_none() {
            let relative = cwd
                .strip_prefix(&root)
                .map_err(|_| {
                    ApiError::core(
                        CoreError::invalid("command cwd resolves outside the authorized workspace"),
                        "runScenario",
                    )
                })?
                .to_string_lossy()
                .replace('\\', "/");
            if !relative.is_empty() {
                sandbox.cwd = Some(relative);
            }
        }
    }
    let argv: Vec<String> = body["argv"]
        .as_array()
        .ok_or_else(|| ApiError::core(CoreError::invalid("argv must be an array"), "runScenario"))?
        .iter()
        .map(|value| {
            value
                .as_str()
                .map(str::to_string)
                .ok_or_else(|| CoreError::invalid("argv entries must be strings"))
        })
        .collect::<Result<Vec<_>, _>>()
        .map_err(|error| ApiError::core(error, "runScenario"))?;
    if argv.is_empty() || argv[0].is_empty() {
        return Err(ApiError::core(
            CoreError::invalid("argv must not be empty"),
            "runScenario",
        ));
    }
    let mut env = Vec::new();
    if let Some(values) = body["env"].as_object() {
        for (name, value) in values {
            let Some(value) = value.as_str() else {
                return Err(ApiError::core(
                    CoreError::invalid("env values must be strings"),
                    "runScenario",
                ));
            };
            env.push((name.clone(), value.to_string()));
        }
    }
    let timeout_ms = body["timeout_ms"].as_u64().unwrap_or(60_000).clamp(1_000, 600_000);
    let grant = &body["execution_grant"];
    if grant["granted"].as_bool() != Some(true) {
        return Err(ApiError::core(
            CoreError::Unsupported("permission_denied: scenario execution requires an explicit grant".into()),
            "runScenario",
        ));
    }
    let grant_id = grant["grant_id"]
        .as_str()
        .ok_or_else(|| ApiError::core(CoreError::invalid("execution_grant.grant_id is required"), "runScenario"))?;
    let reason = grant["reason"]
        .as_str()
        .ok_or_else(|| ApiError::core(CoreError::invalid("execution_grant.reason is required"), "runScenario"))?;
    let run_id = body["run_id"].as_str().map(str::to_string).unwrap_or_else(|| {
        format!(
            "run.{project_id}.scenario.{}",
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .map(|duration| duration.as_secs())
                .unwrap_or(0)
        )
    });
    let mut store = lock_mut(&state)?;
    let payload = store
        .snapshot_payload(project_id, snapshot_id)
        .map_err(ApiError::core_err)?
        .ok_or_else(|| ApiError::core(CoreError::not_found("snapshot not found"), "runScenario"))?;
    let config_digest = payload["config_digest"]
        .as_str()
        .ok_or_else(|| ApiError::core(CoreError::Storage("snapshot has no config_digest".into()), "runScenario"))?
        .to_string();
    let spec = ScenarioSpec {
        project_id: project_id.to_string(),
        run_id: run_id.clone(),
        snapshot_id: snapshot_id.to_string(),
        config_digest,
        argv,
        cwd,
        env,
        timeout: Duration::from_millis(timeout_ms),
        sandbox,
    };
    let scenario = run_scenario(
        &mut store,
        &spec,
        &ExecutionGrant::grant(grant_id, reason),
    )
    .map_err(ApiError::core_err)?;
    Ok(Json(scenario))
}

#[derive(Deserialize)]
struct ScenarioQuery {
    project_id: Option<String>,
}

async fn get_scenario_handler(
    State(state): State<ApiState>,
    Path(run_id): Path<String>,
    Query(params): Query<ScenarioQuery>,
) -> Result<Json<Value>, ApiError> {
    let project_id = params.project_id.ok_or_else(|| {
        ApiError::core(CoreError::invalid("project_id is required"), "getScenario")
    })?;
    let store = lock(&state)?;
    let run = get_scenario_run(&store, &project_id, &run_id).map_err(ApiError::core_err)?;
    Ok(Json(run))
}

async fn open_project(State(state): State<ApiState>, Json(body): Json<Value>) -> Result<Json<Value>, ApiError> {
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

async fn get_project_handler(
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

async fn close_project_handler(
    State(state): State<ApiState>,
    Path(project_id): Path<String>,
    Json(body): Json<Value>,
) -> Result<Json<Value>, ApiError> {
    lifecycle_action(state, project_id, body, "closeProject", |store, project_id, revision| {
        close_project_checked(store, project_id, revision, true)
    })
}

async fn promote_project_handler(
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

async fn health() -> Json<Value> {
    Json(json!({
        "service": "archmeld",
        "contract_version": CONTRACT_VERSION,
        "status": "ready",
        "product_version": PRODUCT_VERSION,
    }))
}

async fn query(State(state): State<ApiState>, Json(body): Json<Value>) -> Result<Json<Value>, ApiError> {
    let store = lock(&state)?;
    run_query(&store, &body).map(Json).map_err(ApiError::core_err)
}

#[derive(Deserialize)]
struct EntityQuery {
    project_id: Option<String>,
    snapshot_id: Option<String>,
}

async fn get_entity(
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

async fn source(State(state): State<ApiState>, Json(body): Json<Value>) -> Result<Json<Value>, ApiError> {
    let store = lock(&state)?;
    read_source(&store, &body).map(Json).map_err(ApiError::core_err)
}

async fn source_file(
    State(state): State<ApiState>,
    Json(body): Json<Value>,
) -> Result<Json<Value>, ApiError> {
    let store = lock(&state)?;
    read_source_file(&store, &body)
        .map(Json)
        .map_err(ApiError::core_err)
}

#[derive(Deserialize)]
struct RuntimeQuery {
    project_id: Option<String>,
    limit: Option<usize>,
}

async fn runtime_timeline_handler(
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

async fn runtime_summary_handler(
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

#[derive(Deserialize)]
struct ProjectQuery {
    project_id: Option<String>,
}

async fn job(
    State(state): State<ApiState>,
    Path(job_id): Path<String>,
    Query(params): Query<ProjectQuery>,
) -> Result<Json<Value>, ApiError> {
    let project_id = params
        .project_id
        .ok_or_else(|| ApiError::core(CoreError::invalid("project_id is required"), "getJob"))?;
    let store = lock(&state)?;
    let job = get_job(&store, &project_id, &job_id)
        .map_err(ApiError::core_err)?
        .ok_or_else(|| ApiError::core(CoreError::not_found(format!("job {job_id} not found")), "getJob"))?;
    Ok(Json(job))
}

async fn cancel(
    State(state): State<ApiState>,
    Path(job_id): Path<String>,
    Json(body): Json<Value>,
) -> Result<(StatusCode, Json<Value>), ApiError> {
    let object = body
        .as_object()
        .ok_or_else(|| ApiError::core(CoreError::invalid("cancel body must be an object"), "cancelJob"))?;
    let project_id = object
        .get("project_id")
        .and_then(Value::as_str)
        .ok_or_else(|| ApiError::core(CoreError::invalid("project_id is required"), "cancelJob"))?;
    let expected_revision = object
        .get("expected_revision")
        .and_then(Value::as_i64)
        .ok_or_else(|| {
            ApiError::core(
                CoreError::invalid("expected_revision is required"),
                "cancelJob",
            )
        })?;
    let idempotency_key = object
        .get("idempotency_key")
        .and_then(Value::as_str)
        .filter(|key| !key.is_empty())
        .ok_or_else(|| {
            ApiError::core(
                CoreError::invalid("idempotency_key is required"),
                "cancelJob",
            )
        })?;
    if object.get("approval_id").is_some_and(|value| !value.is_null()) {
        return Err(ApiError::core(
            CoreError::Unsupported("approval_id is not accepted for job cancellation".into()),
            "cancelJob",
        ));
    }
    let request_digest = hash::digest_value(&json!({"job_id": job_id, "body": body}))
        .map_err(|err| ApiError::core(CoreError::invalid(format!("request digest: {err}")), "cancelJob"))?;
    let (status, job) = {
        let mut store = state
            .store
            .lock()
            .map_err(|_| ApiError::core(CoreError::Storage("store lock poisoned".into()), "cancelJob"))?;
        cancel_job_idempotent(
            &mut store,
            project_id,
            &job_id,
            expected_revision,
            idempotency_key,
            &request_digest,
        )
        .map_err(ApiError::core_err)?
    };
    let status = StatusCode::from_u16(status).unwrap_or(StatusCode::ACCEPTED);
    Ok((status, Json(job)))
}

#[derive(Deserialize)]
struct EventsQuery {
    project_id: Option<String>,
    after_sequence: Option<String>,
}

async fn events(
    State(state): State<ApiState>,
    Query(params): Query<EventsQuery>,
) -> Result<Response, ApiError> {
    let project_id = params
        .project_id
        .ok_or_else(|| ApiError::core(CoreError::invalid("project_id is required"), "watchEvents"))?;
    let after = match params.after_sequence.as_deref() {
        None => None,
        Some(text) => Some(text.parse::<u64>().map_err(|_| {
            ApiError::core(
                CoreError::invalid("after_sequence must be a decimal string"),
                "watchEvents",
            )
        })?),
    };
    let store = lock(&state)?;
    let page = events_after(&store, &project_id, after).map_err(ApiError::core_err)?;
    if page.resync_required {
        return Err(ApiError::core(
            CoreError::Conflict("resync_required: cursor predates retained events".into()),
            "watchEvents",
        ));
    }
    let mut body = String::new();
    for event in page.events {
        if let Some(sequence) = event["sequence"].as_str() {
            body.push_str(&format!("id: {sequence}\n"));
        }
        body.push_str("data: ");
        body.push_str(&event.to_string());
        body.push_str("\n\n");
    }
    let mut response = Response::new(Body::from(body));
    response.headers_mut().insert(
        header::CONTENT_TYPE,
        HeaderValue::from_static("text/event-stream"),
    );
    Ok(response)
}

async fn propose_claim_handler(
    State(state): State<ApiState>,
    Json(body): Json<Value>,
) -> Result<Json<Value>, ApiError> {
    let mut store = lock_mut(&state)?;
    let claim = propose_claim(&mut store, &body).map_err(ApiError::core_err)?;
    Ok(Json(claim))
}

async fn get_claim_handler(
    State(state): State<ApiState>,
    Path(claim_id): Path<String>,
    Query(params): Query<ProjectQuery>,
) -> Result<Json<Value>, ApiError> {
    let project_id = params
        .project_id
        .ok_or_else(|| ApiError::core(CoreError::invalid("project_id is required"), "getClaim"))?;
    let store = lock(&state)?;
    let claim = get_claim(&store, &project_id, &claim_id).map_err(ApiError::core_err)?;
    Ok(Json(claim))
}

async fn review_claim_handler(
    State(state): State<ApiState>,
    Path(claim_id): Path<String>,
    Json(body): Json<Value>,
) -> Result<Json<Value>, ApiError> {
    let object = body.as_object().ok_or_else(|| {
        ApiError::core(CoreError::invalid("claim review body must be an object"), "reviewClaim")
    })?;
    let project_id = object
        .get("project_id")
        .and_then(Value::as_str)
        .ok_or_else(|| ApiError::core(CoreError::invalid("project_id is required"), "reviewClaim"))?;
    let expected_revision = object
        .get("expected_revision")
        .and_then(Value::as_i64)
        .ok_or_else(|| {
            ApiError::core(CoreError::invalid("expected_revision is required"), "reviewClaim")
        })?;
    let status = object
        .get("status")
        .and_then(Value::as_str)
        .ok_or_else(|| ApiError::core(CoreError::invalid("status is required"), "reviewClaim"))?;
    let reason = object
        .get("reason")
        .and_then(Value::as_str)
        .ok_or_else(|| ApiError::core(CoreError::invalid("reason is required"), "reviewClaim"))?;
    let idempotency_key = object
        .get("idempotency_key")
        .and_then(Value::as_str)
        .filter(|key| !key.is_empty())
        .ok_or_else(|| {
            ApiError::core(CoreError::invalid("idempotency_key is required"), "reviewClaim")
        })?;
    let actor = object
        .get("actor")
        .cloned()
        .unwrap_or_else(|| json!({"name": "local-user"}));
    let support = string_array(object.get("support_ids"), "support_ids")?;
    let counter = string_array(object.get("counterevidence_ids"), "counterevidence_ids")?;
    let mut store = lock_mut(&state)?;
    let claim = review_claim(
        &mut store,
        project_id,
        &claim_id,
        expected_revision,
        status,
        reason,
        &actor,
        &support,
        &counter,
        idempotency_key,
    )
    .map_err(ApiError::core_err)?;
    Ok(Json(claim))
}

async fn propose_investigation_handler(
    State(state): State<ApiState>,
    Json(body): Json<Value>,
) -> Result<Json<Value>, ApiError> {
    let mut store = lock_mut(&state)?;
    let investigation = propose_investigation(&mut store, &body).map_err(ApiError::core_err)?;
    Ok(Json(investigation))
}

async fn get_investigation_handler(
    State(state): State<ApiState>,
    Path(investigation_id): Path<String>,
    Query(params): Query<ProjectQuery>,
) -> Result<Json<Value>, ApiError> {
    let project_id = params
        .project_id
        .ok_or_else(|| ApiError::core(CoreError::invalid("project_id is required"), "getInvestigation"))?;
    let store = lock(&state)?;
    let investigation = get_investigation(&store, &project_id, &investigation_id)
        .map_err(ApiError::core_err)?;
    Ok(Json(investigation))
}

async fn propose_architecture_handler(
    State(state): State<ApiState>,
    Json(body): Json<Value>,
) -> Result<Json<Value>, ApiError> {
    let mut store = lock_mut(&state)?;
    let architecture = propose_architecture(&mut store, &body).map_err(ApiError::core_err)?;
    Ok(Json(architecture))
}

async fn get_architecture_handler(
    State(state): State<ApiState>,
    Path(architecture_id): Path<String>,
    Query(params): Query<ProjectQuery>,
) -> Result<Json<Value>, ApiError> {
    let project_id = params
        .project_id
        .ok_or_else(|| ApiError::core(CoreError::invalid("project_id is required"), "getArchitecture"))?;
    let store = lock(&state)?;
    let architecture = get_architecture(&store, &project_id, &architecture_id)
        .map_err(ApiError::core_err)?;
    Ok(Json(architecture))
}

async fn architecture_review_handler(
    State(state): State<ApiState>,
    Path(architecture_id): Path<String>,
    Query(params): Query<ProjectQuery>,
) -> Result<Json<Value>, ApiError> {
    let project_id = params
        .project_id
        .ok_or_else(|| ApiError::core(CoreError::invalid("project_id is required"), "getArchitectureReview"))?;
    let store = lock(&state)?;
    let review = architecture_review(&store, &project_id, &architecture_id)
        .map_err(ApiError::core_err)?;
    Ok(Json(review))
}

async fn import_codeboarding_handler(
    State(state): State<ApiState>,
    Json(body): Json<Value>,
) -> Result<Json<Value>, ApiError> {
    let object = body.as_object().ok_or_else(|| {
        ApiError::core(CoreError::invalid("body must be an object"), "importCodeBoarding")
    })?;
    let text = |field: &str| -> Result<String, ApiError> {
        object
            .get(field)
            .and_then(Value::as_str)
            .filter(|value| !value.is_empty())
            .map(str::to_string)
            .ok_or_else(|| {
                ApiError::core(
                    CoreError::invalid(format!("{field} is required")),
                    "importCodeBoarding",
                )
            })
    };
    let project_id = text("project_id")?;
    let snapshot_id = text("snapshot_id")?;
    let idempotency_key = text("idempotency_key")?;
    let run_id = text("run_id")?;
    let provider_version = text("provider_version")?;
    let expected_revision = object
        .get("expected_revision")
        .and_then(Value::as_i64)
        .ok_or_else(|| {
            ApiError::core(
                CoreError::invalid("expected_revision is required"),
                "importCodeBoarding",
            )
        })?;
    let commands = string_array(object.get("commands"), "commands")?;
    let analysis = object.get("analysis").cloned().ok_or_else(|| {
        ApiError::core(
            CoreError::invalid("analysis is required"),
            "importCodeBoarding",
        )
    })?;
    let model = object
        .get("model")
        .filter(|value| !value.is_null())
        .map(|value| model_provenance(value))
        .transpose()?;
    let mut store = lock_mut(&state)?;
    let architecture = import_codeboarding(
        &mut store,
        &CodeBoardingRequest {
            project_id: &project_id,
            snapshot_id: &snapshot_id,
            expected_revision,
            idempotency_key: &idempotency_key,
            provider_version: &provider_version,
            run_id: &run_id,
            commands,
            model: model.as_ref().map(|value| ModelProvenance {
                provider: &value.0,
                model: &value.1,
                base_url_host: &value.2,
                calls: value.3,
                input_tokens: value.4,
                output_tokens: value.5,
            }),
        },
        &analysis,
    )
    .map_err(ApiError::core_err)?;
    Ok(Json(architecture))
}

fn model_provenance(value: &Value) -> Result<(String, String, String, u64, Option<u64>, Option<u64>), ApiError> {
    let object = value.as_object().ok_or_else(|| {
        ApiError::core(CoreError::invalid("model must be an object"), "importCodeBoarding")
    })?;
    for key in object.keys() {
        if !matches!(
            key.as_str(),
            "provider" | "model" | "base_url_host" | "calls" | "input_tokens" | "output_tokens"
        ) {
            return Err(ApiError::core(
                CoreError::invalid(format!("model contains an unknown field {key}")),
                "importCodeBoarding",
            ));
        }
    }
    let text = |name: &str| -> Result<String, ApiError> {
        object
            .get(name)
            .and_then(Value::as_str)
            .filter(|value| !value.is_empty())
            .map(str::to_string)
            .ok_or_else(|| {
                ApiError::core(
                    CoreError::invalid(format!("model.{name} is required")),
                    "importCodeBoarding",
                )
            })
    };
    let calls = object
        .get("calls")
        .and_then(Value::as_u64)
        .filter(|value| *value > 0)
        .ok_or_else(|| {
            ApiError::core(
                CoreError::invalid("model.calls must be a positive integer"),
                "importCodeBoarding",
            )
        })?;
    let optional = |name: &str| -> Result<Option<u64>, ApiError> {
        match object.get(name) {
            None | Some(Value::Null) => Ok(None),
            Some(Value::Number(number)) => number.as_u64().map(Some).ok_or_else(|| {
                ApiError::core(
                    CoreError::invalid(format!("model.{name} must be a non-negative integer")),
                    "importCodeBoarding",
                )
            }),
            Some(_) => Err(ApiError::core(
                CoreError::invalid(format!("model.{name} must be a non-negative integer")),
                "importCodeBoarding",
            )),
        }
    };
    Ok((text("provider")?, text("model")?, text("base_url_host")?, calls, optional("input_tokens")?, optional("output_tokens")?))
}

async fn propose_contract_handler(
    State(state): State<ApiState>,
    Json(body): Json<Value>,
) -> Result<Json<Value>, ApiError> {
    let mut store = lock_mut(&state)?;
    let contract = propose_contract(&mut store, &body).map_err(ApiError::core_err)?;
    Ok(Json(contract))
}

async fn get_contract_handler(
    State(state): State<ApiState>,
    Path(contract_id): Path<String>,
    Query(params): Query<ProjectQuery>,
) -> Result<Json<Value>, ApiError> {
    let project_id = params.project_id.ok_or_else(|| {
        ApiError::core(CoreError::invalid("project_id is required"), "getContract")
    })?;
    let store = lock(&state)?;
    let contract = get_contract(&store, &project_id, &contract_id).map_err(ApiError::core_err)?;
    Ok(Json(contract))
}

async fn propose_plan_handler(
    State(state): State<ApiState>,
    Json(body): Json<Value>,
) -> Result<Json<Value>, ApiError> {
    let mut store = lock_mut(&state)?;
    let plan = propose_plan(&mut store, &body).map_err(ApiError::core_err)?;
    Ok(Json(plan))
}

async fn get_plan_handler(
    State(state): State<ApiState>,
    Path(plan_id): Path<String>,
    Query(params): Query<ProjectQuery>,
) -> Result<Json<Value>, ApiError> {
    let project_id = params
        .project_id
        .ok_or_else(|| ApiError::core(CoreError::invalid("project_id is required"), "getPlan"))?;
    let store = lock(&state)?;
    let plan = get_plan(&store, &project_id, &plan_id).map_err(ApiError::core_err)?;
    Ok(Json(plan))
}

async fn approve_plan_handler(
    State(state): State<ApiState>,
    Path(plan_id): Path<String>,
    Json(body): Json<Value>,
) -> Result<Json<Value>, ApiError> {
    let object = body.as_object().ok_or_else(|| {
        ApiError::core(CoreError::invalid("approval body must be an object"), "approvePlan")
    })?;
    let project_id = object["project_id"]
        .as_str()
        .ok_or_else(|| ApiError::core(CoreError::invalid("project_id is required"), "approvePlan"))?;
    let expected_revision = object["expected_revision"]
        .as_i64()
        .ok_or_else(|| {
            ApiError::core(CoreError::invalid("expected_revision is required"), "approvePlan")
        })?;
    let contract_digest = object["contract_digest"]
        .as_str()
        .ok_or_else(|| {
            ApiError::core(CoreError::invalid("contract_digest is required"), "approvePlan")
        })?;
    let actor = object["actor"]
        .as_str()
        .filter(|value| !value.is_empty())
        .ok_or_else(|| ApiError::core(CoreError::invalid("actor is required"), "approvePlan"))?;
    let idempotency_key = object["idempotency_key"]
        .as_str()
        .filter(|value| !value.is_empty())
        .ok_or_else(|| {
            ApiError::core(CoreError::invalid("idempotency_key is required"), "approvePlan")
        })?;
    let mut store = lock_mut(&state)?;
    let plan = approve_plan(
        &mut store,
        project_id,
        &plan_id,
        expected_revision,
        contract_digest,
        actor,
        idempotency_key,
    )
    .map_err(ApiError::core_err)?;
    Ok(Json(plan))
}

async fn create_changeset_handler(
    State(state): State<ApiState>,
    Json(body): Json<Value>,
) -> Result<Json<Value>, ApiError> {
    let mut store = lock_mut(&state)?;
    let changeset = create_changeset(&mut store, &body).map_err(ApiError::core_err)?;
    Ok(Json(changeset))
}

async fn get_changeset_handler(
    State(state): State<ApiState>,
    Path(changeset_id): Path<String>,
    Query(params): Query<ProjectQuery>,
) -> Result<Json<Value>, ApiError> {
    let project_id = params.project_id.ok_or_else(|| {
        ApiError::core(CoreError::invalid("project_id is required"), "getChangeSet")
    })?;
    let store = lock(&state)?;
    let changeset = get_changeset(&store, &project_id, &changeset_id).map_err(ApiError::core_err)?;
    Ok(Json(changeset))
}

#[derive(Deserialize)]
struct ChangesetListQuery {
    project_id: Option<String>,
    state: Option<String>,
}

async fn list_changesets_handler(
    State(state): State<ApiState>,
    Query(params): Query<ChangesetListQuery>,
) -> Result<Json<Value>, ApiError> {
    let project_id = params.project_id.ok_or_else(|| {
        ApiError::core(CoreError::invalid("project_id is required"), "listChangeSets")
    })?;
    let store = lock(&state)?;
    let changesets =
        list_changesets(&store, &project_id, params.state.as_deref()).map_err(ApiError::core_err)?;
    Ok(Json(json!({"changesets": changesets})))
}

#[derive(Deserialize)]
struct ChangesetDiffQuery {
    project_id: Option<String>,
    max_bytes: Option<u64>,
}

async fn changeset_diff_handler(
    State(state): State<ApiState>,
    Path(changeset_id): Path<String>,
    Query(params): Query<ChangesetDiffQuery>,
) -> Result<Json<Value>, ApiError> {
    let project_id = params.project_id.ok_or_else(|| {
        ApiError::core(CoreError::invalid("project_id is required"), "diffChangeSet")
    })?;
    let store = lock(&state)?;
    let diff = changeset_diff(
        &store,
        &project_id,
        &changeset_id,
        params.max_bytes.unwrap_or(65536),
    )
    .map_err(ApiError::core_err)?;
    Ok(Json(diff))
}

async fn prepare_changeset_handler(
    State(state): State<ApiState>,
    Path(changeset_id): Path<String>,
    Json(body): Json<Value>,
) -> Result<Json<Value>, ApiError> {
    let body = with_changeset_id(body, &changeset_id, "prepareChangeSet")?;
    let mut store = lock_mut(&state)?;
    let changeset = prepare_changeset(&mut store, &body).map_err(ApiError::core_err)?;
    Ok(Json(changeset))
}

async fn approve_changeset_handler(
    State(state): State<ApiState>,
    Path(changeset_id): Path<String>,
    Json(body): Json<Value>,
) -> Result<Json<Value>, ApiError> {
    let body = with_changeset_id(body, &changeset_id, "approveChangeSet")?;
    let mut store = lock_mut(&state)?;
    let approval = approve_prepared_change(&mut store, &body).map_err(ApiError::core_err)?;
    Ok(Json(approval))
}

async fn reject_changeset_handler(
    State(state): State<ApiState>,
    Path(changeset_id): Path<String>,
    Json(body): Json<Value>,
) -> Result<Json<Value>, ApiError> {
    let object = body.as_object().ok_or_else(|| {
        ApiError::core(CoreError::invalid("reject body must be an object"), "rejectChangeSet")
    })?;
    let project_id = object["project_id"].as_str().ok_or_else(|| {
        ApiError::core(CoreError::invalid("project_id is required"), "rejectChangeSet")
    })?;
    let expected_revision = object["expected_revision"].as_i64().ok_or_else(|| {
        ApiError::core(
            CoreError::invalid("expected_revision is required"),
            "rejectChangeSet",
        )
    })?;
    let actor = object["actor"]
        .as_str()
        .filter(|value| !value.is_empty())
        .ok_or_else(|| {
            ApiError::core(CoreError::invalid("actor is required"), "rejectChangeSet")
        })?;
    let mut store = lock_mut(&state)?;
    let changeset = reject_changeset(
        &mut store,
        project_id,
        &changeset_id,
        expected_revision,
        actor,
    )
    .map_err(ApiError::core_err)?;
    Ok(Json(changeset))
}

async fn apply_changeset_handler(
    State(state): State<ApiState>,
    Path(changeset_id): Path<String>,
    Json(body): Json<Value>,
) -> Result<Json<Value>, ApiError> {
    let body = with_changeset_id(body, &changeset_id, "applyChangeSet")?;
    let mut store = lock_mut(&state)?;
    let changeset = apply_changeset(&mut store, &body).map_err(ApiError::core_err)?;
    Ok(Json(changeset))
}

async fn recover_changeset_handler(
    State(state): State<ApiState>,
    Path(changeset_id): Path<String>,
    Json(body): Json<Value>,
) -> Result<Json<Value>, ApiError> {
    let body = with_changeset_id(body, &changeset_id, "recoverChangeSet")?;
    let mut store = lock_mut(&state)?;
    let changeset = recover_changeset(&mut store, &body).map_err(ApiError::core_err)?;
    Ok(Json(changeset))
}

async fn verify_changeset_handler(
    State(state): State<ApiState>,
    Path(changeset_id): Path<String>,
    Json(body): Json<Value>,
) -> Result<Json<Value>, ApiError> {
    let body = with_changeset_id(body, &changeset_id, "verifyChangeSet")?;
    let mut store = lock_mut(&state)?;
    let verification = verify_changeset(&mut store, &body).map_err(ApiError::core_err)?;
    Ok(Json(verification))
}

async fn accept_changeset_handler(
    State(state): State<ApiState>,
    Path(changeset_id): Path<String>,
    Json(body): Json<Value>,
) -> Result<Json<Value>, ApiError> {
    let body = with_changeset_id(body, &changeset_id, "acceptChangeSet")?;
    let mut store = lock_mut(&state)?;
    let changeset = accept_changeset(&mut store, &body).map_err(ApiError::core_err)?;
    Ok(Json(changeset))
}

async fn get_verification_handler(
    State(state): State<ApiState>,
    Path(verification_id): Path<String>,
    Query(params): Query<ProjectQuery>,
) -> Result<Json<Value>, ApiError> {
    let project_id = params.project_id.ok_or_else(|| {
        ApiError::core(CoreError::invalid("project_id is required"), "getVerification")
    })?;
    let store = lock(&state)?;
    let verification =
        get_verification(&store, &project_id, &verification_id).map_err(ApiError::core_err)?;
    Ok(Json(verification))
}

#[derive(Deserialize)]
struct VerificationListQuery {
    project_id: Option<String>,
    changeset_id: Option<String>,
}

async fn list_verifications_handler(
    State(state): State<ApiState>,
    Query(params): Query<VerificationListQuery>,
) -> Result<Json<Value>, ApiError> {
    let project_id = params.project_id.ok_or_else(|| {
        ApiError::core(CoreError::invalid("project_id is required"), "listVerifications")
    })?;
    let store = lock(&state)?;
    let verifications = list_verifications(
        &store,
        &project_id,
        params.changeset_id.as_deref(),
    )
    .map_err(ApiError::core_err)?;
    Ok(Json(json!({"verifications": verifications})))
}

async fn instigator_configure_handler(
    State(state): State<ApiState>,
    Json(body): Json<Value>,
) -> Result<Json<Value>, ApiError> {
    let mut store = lock_mut(&state)?;
    let status = configure_instigator(&mut store, &body).map_err(ApiError::core_err)?;
    Ok(Json(status))
}

async fn instigator_nudge_handler(
    State(state): State<ApiState>,
    Json(body): Json<Value>,
) -> Result<Json<Value>, ApiError> {
    let mut store = lock_mut(&state)?;
    let decision = propose_nudge(&mut store, &body).map_err(ApiError::core_err)?;
    Ok(Json(decision))
}

async fn instigator_deliver_handler(
    State(state): State<ApiState>,
    Json(body): Json<Value>,
) -> Result<Json<Value>, ApiError> {
    let mut store = lock_mut(&state)?;
    let delivered = deliver_due(&mut store, &body).map_err(ApiError::core_err)?;
    Ok(Json(delivered))
}

async fn instigator_status_handler(
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

async fn record_drift_handler(
    State(state): State<ApiState>,
    Json(body): Json<Value>,
) -> Result<Json<Value>, ApiError> {
    let mut store = lock_mut(&state)?;
    let report = record_drift(&mut store, &body).map_err(ApiError::core_err)?;
    Ok(Json(report))
}

async fn list_drift_handler(
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

async fn create_backup_handler(
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

async fn gc_handler(
    State(state): State<ApiState>,
    Json(body): Json<Value>,
) -> Result<Json<Value>, ApiError> {
    let dry_run = body.get("dry_run").and_then(Value::as_bool).unwrap_or(true);
    let mut store = lock_mut(&state)?;
    let result = gc_objects(&mut store, dry_run).map_err(ApiError::core_err)?;
    Ok(Json(result))
}

async fn export_report_handler(
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

async fn configure_retention_handler(
    State(state): State<ApiState>,
    Json(body): Json<Value>,
) -> Result<Json<Value>, ApiError> {
    let mut store = lock_mut(&state)?;
    let status = configure_retention(&mut store, &body).map_err(ApiError::core_err)?;
    Ok(Json(status))
}

async fn retention_status_handler(
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

async fn prune_retention_handler(
    State(state): State<ApiState>,
    Json(body): Json<Value>,
) -> Result<Json<Value>, ApiError> {
    let mut store = lock_mut(&state)?;
    let result = prune_snapshots(&mut store, &body).map_err(ApiError::core_err)?;
    Ok(Json(result))
}

async fn list_retention_runs_handler(
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

async fn review_architecture_handler(
    State(state): State<ApiState>,
    Path(architecture_id): Path<String>,
    Json(mut body): Json<Value>,
) -> Result<Json<Value>, ApiError> {
    let Some(object) = body.as_object_mut() else {
        return Err(ApiError::core(
            CoreError::invalid("architecture review body must be an object"),
            "reviewArchitecture",
        ));
    };
    if let Some(declared) = object.get("architecture_id").and_then(Value::as_str) {
        if declared != architecture_id {
            return Err(ApiError::core(
                CoreError::invalid("architecture_id in the path and body disagree"),
                "reviewArchitecture",
            ));
        }
    } else {
        object.insert("architecture_id".to_string(), json!(architecture_id));
    }
    let mut store = lock_mut(&state)?;
    let reviewed = review_architecture(&mut store, &body).map_err(ApiError::core_err)?;
    Ok(Json(reviewed))
}

async fn list_architecture_reviews_handler(
    State(state): State<ApiState>,
    Path(architecture_id): Path<String>,
    Query(params): Query<ProjectQuery>,
) -> Result<Json<Value>, ApiError> {
    let project_id = params.project_id.ok_or_else(|| {
        ApiError::core(
            CoreError::invalid("project_id is required"),
            "listArchitectureReviews",
        )
    })?;
    let store = lock(&state)?;
    let reviews = list_architecture_reviews(&store, &project_id, &architecture_id)
        .map_err(ApiError::core_err)?;
    Ok(Json(json!({
        "project_id": project_id,
        "architecture_id": architecture_id,
        "reviews": reviews,
    })))
}

async fn project_lease_handler(
    State(state): State<ApiState>,
    Path(project_id): Path<String>,
) -> Result<Json<Value>, ApiError> {
    let store = lock(&state)?;
    let lease = writer_lease(&store, &project_id).map_err(ApiError::core_err)?;
    Ok(Json(json!({"project_id": project_id, "lease": lease})))
}

async fn interrupted_handler(
    State(state): State<ApiState>,
    Path(project_id): Path<String>,
) -> Result<Json<Value>, ApiError> {
    let store = lock(&state)?;
    let items = interrupted_changesets(&store, &project_id).map_err(ApiError::core_err)?;
    Ok(Json(json!({"project_id": project_id, "interrupted": items})))
}

fn with_changeset_id(
    mut body: Value,
    changeset_id: &str,
    operation: &str,
) -> Result<Value, ApiError> {
    let object = body.as_object_mut().ok_or_else(|| {
        ApiError::core(
            CoreError::invalid(format!("{operation} body must be an object")),
            operation,
        )
    })?;
    if let Some(declared) = object.get("changeset_id").and_then(Value::as_str) {
        if declared != changeset_id {
            return Err(ApiError::core(
                CoreError::invalid("changeset_id in the path and body disagree"),
                operation,
            ));
        }
    }
    object.insert("changeset_id".to_string(), json!(changeset_id));
    Ok(body)
}

async fn architecture_diff_handler(
    State(state): State<ApiState>,
    Path(architecture_id): Path<String>,
    Json(body): Json<Value>,
) -> Result<Json<Value>, ApiError> {
    let project_id = body["project_id"].as_str().ok_or_else(|| {
        ApiError::core(CoreError::invalid("project_id is required"), "diffArchitecture")
    })?;
    let target = body.get("target").cloned().ok_or_else(|| {
        ApiError::core(CoreError::invalid("target is required"), "diffArchitecture")
    })?;
    let store = lock(&state)?;
    let diff = diff_architecture(&store, project_id, &architecture_id, &target)
        .map_err(ApiError::core_err)?;
    Ok(Json(diff))
}

async fn architecture_check_handler(
    State(state): State<ApiState>,
    Path(architecture_id): Path<String>,
    Json(body): Json<Value>,
) -> Result<Json<Value>, ApiError> {
    let project_id = body["project_id"].as_str().ok_or_else(|| {
        ApiError::core(CoreError::invalid("project_id is required"), "checkArchitecture")
    })?;
    let rule_ids = string_array(body.get("rule_ids"), "rule_ids")?;
    let store = lock(&state)?;
    let rules = (!rule_ids.is_empty()).then_some(rule_ids.as_slice());
    let result = check_architecture_rules(&store, project_id, &architecture_id, rules)
        .map_err(ApiError::core_err)?;
    Ok(Json(result))
}

async fn capabilities_handler() -> Json<Value> {
    Json(provider_capabilities())
}

fn string_array(value: Option<&Value>, field: &str) -> Result<Vec<String>, ApiError> {
    let Some(value) = value else {
        return Ok(Vec::new());
    };
    let array = value.as_array().ok_or_else(|| {
        ApiError::core(CoreError::invalid(format!("{field} must be an array")), "request")
    })?;
    array
        .iter()
        .map(|item| {
            item.as_str()
                .filter(|text| !text.is_empty())
                .map(str::to_string)
                .ok_or_else(|| {
                    ApiError::core(
                        CoreError::invalid(format!("{field} entries must be non-empty strings")),
                        "request",
                    )
                })
        })
        .collect()
}

fn lock_mut(state: &ApiState) -> Result<std::sync::MutexGuard<'_, Store>, ApiError> {
    state
        .store
        .lock()
        .map_err(|_| ApiError::core(CoreError::Storage("store lock poisoned".into()), "lock"))
}

fn lock(state: &ApiState) -> Result<std::sync::MutexGuard<'_, Store>, ApiError> {
    state
        .store
        .lock()
        .map_err(|_| ApiError::core(CoreError::Storage("store lock poisoned".into()), "lock"))
}

pub struct ApiError {
    status: StatusCode,
    body: Value,
}

impl ApiError {
    pub fn core(error: CoreError, request_id: &str) -> Self {
        let message = error.to_string();
        let permission = message.contains("permission_denied");
        let resync = message.contains("resync_required");
        let status = match error.code() {
            "invalid_request" => StatusCode::BAD_REQUEST,
            "not_found" => StatusCode::NOT_FOUND,
            "permission_denied" => StatusCode::FORBIDDEN,
            "scope_conflict" | "stale_snapshot" | "recovery_required" => StatusCode::CONFLICT,
            "unsupported" => StatusCode::UNPROCESSABLE_ENTITY,
            "resource_exhausted" => StatusCode::TOO_MANY_REQUESTS,
            "tool_unavailable" => StatusCode::SERVICE_UNAVAILABLE,
            _ => StatusCode::INTERNAL_SERVER_ERROR,
        };
        let status = if permission { StatusCode::FORBIDDEN } else { status };
        let body = if permission {
            json!({
                "code": "permission_denied",
                "message": error.to_string(),
                "request_id": request_id,
                "retryable": false,
                "details": {},
                "evidence_refs": [],
            })
        } else if resync {
            json!({
                "code": "resync_required",
                "message": error.to_string(),
                "request_id": request_id,
                "retryable": false,
                "details": {},
                "evidence_refs": [],
            })
        } else {
            error.to_contract_error(request_id)
        };
        Self { status, body }
    }

    pub fn core_err(error: CoreError) -> Self {
        Self::core(error, &request_id())
    }
}

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        (self.status, Json(self.body)).into_response()
    }
}

fn host_is_loopback(value: &str) -> bool {
    let host = if let Some(rest) = value.strip_prefix('[') {
        rest.split(']').next().unwrap_or("")
    } else {
        value.split(':').next().unwrap_or("")
    };
    matches!(host, "127.0.0.1" | "localhost" | "::1")
}

fn request_id() -> String {
    static COUNTER: AtomicU64 = AtomicU64::new(0);
    let millis = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_millis())
        .unwrap_or(0);
    format!("req.{millis}.{}", COUNTER.fetch_add(1, Ordering::Relaxed))
}

/// Generate a per-process local token. Printed once by the binary; the parent
/// passes it to clients over its own channel, never in a URL.
pub fn generate_token() -> String {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_nanos())
        .unwrap_or(0);
    let digest = hash::digest_bytes(format!("{nanos}:{}", std::process::id()).as_bytes());
    digest.trim_start_matches("sha256:").chars().take(32).collect()
}
