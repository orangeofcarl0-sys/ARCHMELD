// Copyright 2026 ARCHMELD contributors
// SPDX-License-Identifier: Apache-2.0

//! `interpret` HTTP handlers.

use crate::*;

pub(crate) async fn propose_claim_handler(
    State(state): State<ApiState>,
    Json(body): Json<Value>,
) -> Result<Json<Value>, ApiError> {
    let mut store = lock_mut(&state)?;
    let claim = propose_claim(&mut store, &body).map_err(ApiError::core_err)?;
    Ok(Json(claim))
}

pub(crate) async fn get_claim_handler(
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

pub(crate) async fn review_claim_handler(
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

pub(crate) async fn propose_investigation_handler(
    State(state): State<ApiState>,
    Json(body): Json<Value>,
) -> Result<Json<Value>, ApiError> {
    let mut store = lock_mut(&state)?;
    let investigation = propose_investigation(&mut store, &body).map_err(ApiError::core_err)?;
    Ok(Json(investigation))
}

pub(crate) async fn get_investigation_handler(
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

pub(crate) async fn propose_architecture_handler(
    State(state): State<ApiState>,
    Json(body): Json<Value>,
) -> Result<Json<Value>, ApiError> {
    let mut store = lock_mut(&state)?;
    let architecture = propose_architecture(&mut store, &body).map_err(ApiError::core_err)?;
    Ok(Json(architecture))
}

pub(crate) async fn get_architecture_handler(
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

pub(crate) async fn architecture_review_handler(
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

pub(crate) async fn import_codeboarding_handler(
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
