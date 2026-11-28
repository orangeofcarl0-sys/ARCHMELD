// Copyright 2026 ARCHMELD contributors
// SPDX-License-Identifier: Apache-2.0

//! `jobs` HTTP handlers.

use crate::*;

pub(crate) async fn job(
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

pub(crate) async fn cancel(
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
pub(crate) struct EventsQuery {
    project_id: Option<String>,
    after_sequence: Option<String>,
}

pub(crate) async fn events(
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
