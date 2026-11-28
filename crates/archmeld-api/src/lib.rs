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
use archmeld_core::review::review_workspace_diff;
use archmeld_core::scenario::{
    execute_prepared_owned, finalize_scenario, get_scenario_run, prepare_scenario,
    ExecutionGrant, ScenarioSandbox, ScenarioSpec,
};
use archmeld_core::retention::{
    configure_retention, list_retention_runs, prune_snapshots, retention_status,
};
use archmeld_core::runtime::{runtime_summary, timeline};
use archmeld_core::source_api::{read_source, read_source_file};
use archmeld_core::status::status_report;
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
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

pub const CONTRACT_VERSION: &str = "1.0";
pub const PRODUCT_VERSION: &str = env!("CARGO_PKG_VERSION");
const MAX_BODY_BYTES: usize = 16 * 1024 * 1024;

pub mod onboard;

mod routes;
use routes::*;

/// Shared `?project_id=` query used by project-scoped GET handlers.
#[derive(Deserialize)]
pub(crate) struct ProjectQuery {
    pub(crate) project_id: Option<String>,
}

#[derive(Clone)]
pub struct ApiState {
    pub store: Arc<Mutex<Store>>,
    pub token: Option<String>,
    /// Trusted allowlist: workspace handle -> canonical root.
    pub workspaces: HashMap<String, PathBuf>,
    pub state_dir: PathBuf,
    /// Cooperative cancellation flags for live scenario runs. This lock is
    /// independent of the Store so a cancel request never waits for a child.
    pub scenario_cancels: Arc<Mutex<HashMap<(String, String), Arc<AtomicBool>>>>,
}

pub fn router(state: ApiState) -> Router {
    let shared = state.clone();
    Router::new()
        .route("/v1/health", get(health))
        .route("/v1/status", get(status_handler))
        .route(
            "/v1/reviews/workspace-diff",
            post(review_workspace_diff_handler),
        )
        .route("/v1/scenarios", post(run_scenario_handler))
        .route("/v1/scenarios/{run_id}", get(get_scenario_handler))
        .route(
            "/v1/scenarios/{run_id}/cancel",
            post(cancel_scenario_handler),
        )
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
