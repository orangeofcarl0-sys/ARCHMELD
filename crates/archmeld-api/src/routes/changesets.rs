// Copyright 2026 ARCHMELD contributors
// SPDX-License-Identifier: Apache-2.0

//! `changesets` HTTP handlers.

use crate::*;

pub(crate) async fn propose_contract_handler(
    State(state): State<ApiState>,
    Json(body): Json<Value>,
) -> Result<Json<Value>, ApiError> {
    let mut store = lock_mut(&state)?;
    let contract = propose_contract(&mut store, &body).map_err(ApiError::core_err)?;
    Ok(Json(contract))
}

pub(crate) async fn get_contract_handler(
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

pub(crate) async fn propose_plan_handler(
    State(state): State<ApiState>,
    Json(body): Json<Value>,
) -> Result<Json<Value>, ApiError> {
    let mut store = lock_mut(&state)?;
    let plan = propose_plan(&mut store, &body).map_err(ApiError::core_err)?;
    Ok(Json(plan))
}

pub(crate) async fn get_plan_handler(
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

pub(crate) async fn approve_plan_handler(
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

pub(crate) async fn create_changeset_handler(
    State(state): State<ApiState>,
    Json(body): Json<Value>,
) -> Result<Json<Value>, ApiError> {
    let mut store = lock_mut(&state)?;
    let changeset = create_changeset(&mut store, &body).map_err(ApiError::core_err)?;
    Ok(Json(changeset))
}

pub(crate) async fn get_changeset_handler(
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
pub(crate) struct ChangesetListQuery {
    project_id: Option<String>,
    state: Option<String>,
}

pub(crate) async fn list_changesets_handler(
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
pub(crate) struct ChangesetDiffQuery {
    project_id: Option<String>,
    max_bytes: Option<u64>,
}

pub(crate) async fn changeset_diff_handler(
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

pub(crate) async fn prepare_changeset_handler(
    State(state): State<ApiState>,
    Path(changeset_id): Path<String>,
    Json(body): Json<Value>,
) -> Result<Json<Value>, ApiError> {
    let body = with_changeset_id(body, &changeset_id, "prepareChangeSet")?;
    let mut store = lock_mut(&state)?;
    let changeset = prepare_changeset(&mut store, &body).map_err(ApiError::core_err)?;
    Ok(Json(changeset))
}

pub(crate) async fn approve_changeset_handler(
    State(state): State<ApiState>,
    Path(changeset_id): Path<String>,
    Json(body): Json<Value>,
) -> Result<Json<Value>, ApiError> {
    let body = with_changeset_id(body, &changeset_id, "approveChangeSet")?;
    let mut store = lock_mut(&state)?;
    let approval = approve_prepared_change(&mut store, &body).map_err(ApiError::core_err)?;
    Ok(Json(approval))
}

pub(crate) async fn reject_changeset_handler(
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

pub(crate) async fn apply_changeset_handler(
    State(state): State<ApiState>,
    Path(changeset_id): Path<String>,
    Json(body): Json<Value>,
) -> Result<Json<Value>, ApiError> {
    let body = with_changeset_id(body, &changeset_id, "applyChangeSet")?;
    let mut store = lock_mut(&state)?;
    let changeset = apply_changeset(&mut store, &body).map_err(ApiError::core_err)?;
    Ok(Json(changeset))
}

pub(crate) async fn recover_changeset_handler(
    State(state): State<ApiState>,
    Path(changeset_id): Path<String>,
    Json(body): Json<Value>,
) -> Result<Json<Value>, ApiError> {
    let body = with_changeset_id(body, &changeset_id, "recoverChangeSet")?;
    let mut store = lock_mut(&state)?;
    let changeset = recover_changeset(&mut store, &body).map_err(ApiError::core_err)?;
    Ok(Json(changeset))
}

pub(crate) async fn verify_changeset_handler(
    State(state): State<ApiState>,
    Path(changeset_id): Path<String>,
    Json(body): Json<Value>,
) -> Result<Json<Value>, ApiError> {
    let body = with_changeset_id(body, &changeset_id, "verifyChangeSet")?;
    let mut store = lock_mut(&state)?;
    let verification = verify_changeset(&mut store, &body).map_err(ApiError::core_err)?;
    Ok(Json(verification))
}

pub(crate) async fn accept_changeset_handler(
    State(state): State<ApiState>,
    Path(changeset_id): Path<String>,
    Json(body): Json<Value>,
) -> Result<Json<Value>, ApiError> {
    let body = with_changeset_id(body, &changeset_id, "acceptChangeSet")?;
    let mut store = lock_mut(&state)?;
    let changeset = accept_changeset(&mut store, &body).map_err(ApiError::core_err)?;
    Ok(Json(changeset))
}

pub(crate) async fn get_verification_handler(
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
pub(crate) struct VerificationListQuery {
    project_id: Option<String>,
    changeset_id: Option<String>,
}

pub(crate) async fn list_verifications_handler(
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
