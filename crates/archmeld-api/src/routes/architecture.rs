// Copyright 2026 ARCHMELD contributors
// SPDX-License-Identifier: Apache-2.0

//! `architecture` HTTP handlers.

use crate::*;

pub(crate) async fn review_architecture_handler(
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

pub(crate) async fn list_architecture_reviews_handler(
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
pub(crate) async fn architecture_diff_handler(
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

pub(crate) async fn architecture_check_handler(
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
