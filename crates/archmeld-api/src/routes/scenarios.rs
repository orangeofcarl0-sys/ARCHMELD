// Copyright 2026 ARCHMELD contributors
// SPDX-License-Identifier: Apache-2.0

//! `scenarios` HTTP handlers.

use crate::*;

pub(crate) async fn run_scenario_handler(
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
    let spec = {
        let store = lock(&state)?;
        let payload = store
            .snapshot_payload(project_id, snapshot_id)
            .map_err(ApiError::core_err)?
            .ok_or_else(|| ApiError::core(CoreError::not_found("snapshot not found"), "runScenario"))?;
        let config_digest = payload["config_digest"]
            .as_str()
            .ok_or_else(|| ApiError::core(CoreError::Storage("snapshot has no config_digest".into()), "runScenario"))?
            .to_string();
        ScenarioSpec {
            project_id: project_id.to_string(),
            run_id: run_id.clone(),
            snapshot_id: snapshot_id.to_string(),
            config_digest,
            argv,
            cwd,
            env,
            timeout: Duration::from_millis(timeout_ms),
            sandbox,
        }
    };
    let prepared = {
        let mut store = lock_mut(&state)?;
        prepare_scenario(&mut store, spec, ExecutionGrant::grant(grant_id, reason))
            .map_err(ApiError::core_err)?
    };
    // The child process can take minutes; run it on a blocking worker with no
    // Store guard, so queries, events and cancellation remain responsive.
    let cancel = Arc::new(AtomicBool::new(false));
    let cancel_key = (project_id.to_string(), run_id.clone());
    if let Ok(mut active) = state.scenario_cancels.lock() {
        active.insert(cancel_key.clone(), cancel.clone());
    }
    let outcome = tokio::task::spawn_blocking(move || {
        execute_prepared_owned(prepared, Some(cancel))
    })
    .await;
    if let Ok(mut active) = state.scenario_cancels.lock() {
        active.remove(&cancel_key);
    }
    let outcome = outcome
        .map_err(|error| {
            ApiError::core(
                CoreError::Storage(format!("scenario task join: {error}")),
                "runScenario",
            )
        })?
        .map_err(ApiError::core_err)?;
    let mut store = lock_mut(&state)?;
    let scenario = finalize_scenario(&mut store, outcome).map_err(ApiError::core_err)?;
    Ok(Json(scenario))
}

pub(crate) async fn cancel_scenario_handler(
    State(state): State<ApiState>,
    Path(run_id): Path<String>,
    Json(body): Json<Value>,
) -> Result<Json<Value>, ApiError> {
    let project_id = body["project_id"]
        .as_str()
        .filter(|value| !value.is_empty())
        .ok_or_else(|| {
            ApiError::core(
                CoreError::invalid("project_id is required"),
                "cancelScenario",
            )
        })?;
    let key = (project_id.to_string(), run_id.clone());
    let flag = state
        .scenario_cancels
        .lock()
        .map_err(|_| {
            ApiError::core(
                CoreError::Storage("scenario cancellation registry is poisoned".into()),
                "cancelScenario",
            )
        })?
        .get(&key)
        .cloned();
    match flag {
        Some(flag) => {
            flag.store(true, Ordering::SeqCst);
            Ok(Json(json!({
                "run_id": run_id,
                "project_id": project_id,
                "state": "cancel_requested",
                "cancelled": true,
            })))
        }
        None => Ok(Json(json!({
            "run_id": run_id,
            "project_id": project_id,
            "state": "not_running",
            "cancelled": false,
        }))),
    }
}

#[derive(Deserialize)]
pub(crate) struct ScenarioQuery {
    project_id: Option<String>,
}

pub(crate) async fn get_scenario_handler(
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
