// Copyright 2026 ARCHMELD contributors
// SPDX-License-Identifier: Apache-2.0

//! Independent verification and acceptance (S10, S19 §3.9).
//!
//! The executor runs exactly the checks frozen in the BehaviorContract against
//! exactly one resulting snapshot. Missing, unsupported, inconclusive or
//! timed-out checks are never rewritten into `passed`, and an old/staged
//! snapshot can never produce `accepted`.

use crate::apply::{resolve_workspace_root, safe_resolve};
use crate::blueprint::check_architecture_rules;
use crate::changeset::{append_state, latest_changeset};
use crate::error::{CoreError, Result};
use crate::events::append_event;
use crate::hash;
use crate::interpret::get_architecture;
use crate::plan::get_contract;
use crate::predicate;
use crate::scenario::run_command_capture;
use crate::store::{validate, Store};
use serde::Deserialize;
use serde_json::{json, Value};
use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::{Path, PathBuf};
use std::time::Duration;

const MAX_CHECKS: usize = 200;
const MAX_TIMEOUT_MS: u64 = 600_000;

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
struct VerifyRequest {
    project_id: String,
    changeset_id: String,
    expected_revision: i64,
    workspace_handle: String,
    execution_grant: ExecutionGrantRequest,
    environment: Option<Value>,
    snapshot_id: Option<String>,
    idempotency_key: String,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
struct ExecutionGrantRequest {
    grant_id: String,
    reason: String,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
struct AcceptRequest {
    project_id: String,
    changeset_id: String,
    expected_revision: i64,
    verification_id: String,
    workspace_handle: String,
    actor: String,
    idempotency_key: String,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
#[allow(dead_code)]
struct ContractCheck {
    id: String,
    kind: String,
    required: bool,
    description: String,
    command: Option<Value>,
    checker_id: Option<String>,
    predicate: Value,
    input_artifacts: Vec<String>,
    #[serde(default)]
    sandbox: Option<ContractSandbox>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
struct ContractSandbox {
    mode: String,
    #[serde(default)]
    include: Vec<String>,
    #[serde(default)]
    exclude: Vec<String>,
    #[serde(default)]
    image: Option<String>,
    #[serde(default)]
    network: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
struct CommandSpec {
    argv: Vec<String>,
    cwd: Option<String>,
    timeout_ms: Option<u64>,
    env: Option<BTreeMap<String, String>>,
}

#[derive(Debug, Clone)]
struct CheckOutcome {
    state: String,
    exit_code: Option<i32>,
    timed_out: bool,
    stdout_digest: Option<String>,
    stderr_digest: Option<String>,
    started_ms: Option<i64>,
    finished_ms: Option<i64>,
    artifacts: Vec<Value>,
    detail: Value,
    command_json: Option<String>,
}

#[derive(Debug, Clone, Default)]
struct ReportCounts {
    tests: i64,
    failures: i64,
    errors: i64,
    skipped: i64,
}

/// Run the frozen contract for an applied ChangeSet against its resulting
/// snapshot. This never applies, reverts or edits source.
pub fn verify_changeset(store: &mut Store, raw: &Value) -> Result<Value> {
    let request: VerifyRequest = serde_json::from_value(raw.clone())
        .map_err(|err| CoreError::invalid(format!("verification request: {err}")))?;
    validate::require_id("project_id", &request.project_id)?;
    validate::require_id("changeset_id", &request.changeset_id)?;
    validate::require_id("workspace_handle", &request.workspace_handle)?;
    validate::require_id(
        "execution_grant.grant_id",
        &request.execution_grant.grant_id,
    )?;
    validate::require_non_empty("execution_grant.reason", &request.execution_grant.reason)?;
    validate::require_id("idempotency_key", &request.idempotency_key)?;
    let environment = request.environment.clone().unwrap_or_else(|| json!({}));
    if !environment.is_object() {
        return Err(CoreError::invalid("environment must be an object"));
    }
    let request_digest = hash::digest_value(raw)
        .map_err(|err| CoreError::invalid(format!("verification request: {err}")))?;
    if let Some((recorded, _, result)) = store.idempotency_get(
        &request.project_id,
        "verifyChangeSet",
        &request.idempotency_key,
    )? {
        if recorded != request_digest {
            return Err(CoreError::conflict(
                "scope_conflict: idempotency_key was used with a different verification request",
            ));
        }
        return serde_json::from_str(&result)
            .map_err(|err| CoreError::Storage(format!("stored verification corrupt: {err}")));
    }
    let (revision, changeset) =
        latest_changeset(store, &request.project_id, &request.changeset_id)?.ok_or_else(|| {
            CoreError::not_found(format!("changeset {} not found", request.changeset_id))
        })?;
    if request.expected_revision != revision {
        return Err(CoreError::conflict(format!(
            "stale_snapshot: expected changeset revision {} does not match {revision}",
            request.expected_revision
        )));
    }
    if changeset["state"] != "applied" {
        return Err(CoreError::conflict(format!(
            "changeset {} is {}; verification requires the applied state",
            request.changeset_id, changeset["state"]
        )));
    }
    let resulting_snapshot = changeset["resulting_snapshot_id"]
        .as_str()
        .filter(|value| !value.is_empty())
        .ok_or_else(|| {
            CoreError::conflict("the ChangeSet has no resulting snapshot; apply it first")
        })?
        .to_string();
    if let Some(requested) = &request.snapshot_id {
        if requested != &resulting_snapshot {
            return Err(CoreError::conflict(
                "stale_snapshot: verification must bind the resulting applied snapshot, not a staged or older snapshot",
            ));
        }
    }
    let contract_id = changeset["contract_id"].as_str().unwrap_or("");
    let contract = get_contract(store, &request.project_id, contract_id)?;
    if contract["digest"].as_str() != changeset["contract_digest"].as_str() {
        return Err(CoreError::conflict(
            "stale_contract: the frozen contract changed after apply",
        ));
    }
    if contract["frozen"] != true {
        return Err(CoreError::conflict("the contract is not frozen"));
    }
    let checks: Vec<ContractCheck> = serde_json::from_value(contract["checks"].clone())
        .map_err(|err| CoreError::Storage(format!("contract checks corrupt: {err}")))?;
    if checks.is_empty() {
        return Err(CoreError::invalid("the frozen contract has no checks"));
    }
    if checks.len() > MAX_CHECKS {
        return Err(CoreError::invalid(format!(
            "resource_exhausted: {} checks exceed {MAX_CHECKS}",
            checks.len()
        )));
    }
    let toolchain_digest = changeset["toolchain_digest"]
        .as_str()
        .filter(|value| !value.is_empty())
        .ok_or_else(|| CoreError::conflict("the ChangeSet has no toolchain digest"))?
        .to_string();
    if !store.blob_exists(&toolchain_digest)? {
        return Err(CoreError::conflict(
            "the recorded toolchain artifact is missing from the CAS",
        ));
    }
    let root = resolve_workspace_root(store, &request.workspace_handle, &request.project_id)?;
    let verification_id = format!(
        "verification.{}",
        crate::sandbox::short_hash(&format!(
            "{}:{}:{}:{}",
            request.project_id, request.changeset_id, revision, request.idempotency_key
        ))
    );
    let environment_json = hash::canonical_json(&environment)
        .map_err(|err| CoreError::invalid(format!("environment: {err}")))?;
    let started_at = crate::store::now_unix();
    store.conn.execute(
        "INSERT INTO verification_runs
         (project_id,verification_id,changeset_id,changeset_revision,snapshot_id,
          base_snapshot_id,contract_id,contract_digest,toolchain_digest,environment_json,
          state,verdict_json,started_at,finished_at,created_at)
         VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,'running','{}',?11,NULL,?11)",
        (
            &request.project_id,
            &verification_id,
            &request.changeset_id,
            revision,
            &resulting_snapshot,
            changeset["base_snapshot_id"].as_str().unwrap_or(""),
            contract_id,
            changeset["contract_digest"].as_str().unwrap_or(""),
            &toolchain_digest,
            &environment_json,
            started_at,
        ),
    )?;
    let verifying = append_state(
        store,
        changeset.clone(),
        "verifying",
        json!({
            "verification_id": verification_id,
            "verification_started_at": started_at,
            "verification_snapshot_id": resulting_snapshot,
        }),
    )?;

    // A staged or pre-apply result can never be reused: the applied bytes must
    // still equal the resulting snapshot before any check runs.
    let before_drift = snapshot_drift(store, &request.project_id, &resulting_snapshot, &root)?;
    let mut outcomes: Vec<(ContractCheck, CheckOutcome)> = Vec::new();
    if before_drift.is_empty() {
        for check in &checks {
            let outcome = run_check(
                store,
                &request,
                &changeset,
                &verification_id,
                check,
                &root,
                &resulting_snapshot,
            );
            outcomes.push((check.clone(), outcome));
        }
    }
    let after_drift = snapshot_drift(store, &request.project_id, &resulting_snapshot, &root)?;
    let drift = if before_drift.is_empty() {
        after_drift
    } else {
        before_drift
    };
    let verdict = evaluate(
        &changeset,
        &contract,
        &checks,
        &outcomes,
        &drift,
        &resulting_snapshot,
        &toolchain_digest,
    );
    let finished_at = crate::store::now_unix();
    let verdict_json = serde_json::to_string(&verdict)
        .map_err(|err| CoreError::Storage(format!("verdict encode: {err}")))?;
    let tx = store.conn.transaction()?;
    tx.execute(
        "UPDATE verification_runs SET state=?1, verdict_json=?2, finished_at=?3
         WHERE project_id=?4 AND verification_id=?5",
        (
            verdict["state"].as_str().unwrap_or("failed"),
            &verdict_json,
            finished_at,
            &request.project_id,
            &verification_id,
        ),
    )?;
    for (check, outcome) in &outcomes {
        tx.execute(
            "INSERT INTO check_results
             (project_id,verification_id,check_id,kind,required,state,command_json,exit_code,
              timed_out,stdout_digest,stderr_digest,started_ms,finished_ms,artifacts_json,detail_json)
             VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12,?13,?14,?15)",
            (
                &request.project_id,
                &verification_id,
                &check.id,
                &check.kind,
                if check.required { 1 } else { 0 },
                &outcome.state,
                outcome.command_json.as_deref(),
                outcome.exit_code,
                if outcome.timed_out { 1 } else { 0 },
                outcome.stdout_digest.as_deref(),
                outcome.stderr_digest.as_deref(),
                outcome.started_ms,
                outcome.finished_ms,
                hash::canonical_json(&json!(outcome.artifacts))
                    .map_err(|err| CoreError::invalid(format!("check artifacts: {err}")))?,
                hash::canonical_json(&outcome.detail)
                    .map_err(|err| CoreError::invalid(format!("check detail: {err}")))?,
            ),
        )?;
    }
    append_event(
        &tx,
        &request.project_id,
        None,
        Some(&resulting_snapshot),
        "verification.completed",
        json!({
            "verification_id": verification_id,
            "verdict": verdict["state"],
            "acceptance": verdict["acceptance"],
            "snapshot_id": resulting_snapshot,
            "required_not_passed": verdict["required_not_passed"],
        }),
    )?;
    tx.commit()?;

    if verdict["acceptance"] != true {
        append_state(
            store,
            verifying,
            "failed",
            json!({
                "error": "verification_failed: required checks are not all passed",
                "verification_id": verification_id,
                "verdict_state": verdict["state"],
            }),
        )?;
    }
    let result = verification_payload(store, &request.project_id, &verification_id)?;
    let result_json = serde_json::to_string(&result)
        .map_err(|err| CoreError::Storage(format!("verification encode: {err}")))?;
    store.conn.execute(
        "INSERT INTO idempotency
         (project_id,operation,idempotency_key,request_digest,status_code,result_json,created_at)
         VALUES (?1,'verifyChangeSet',?2,?3,200,?4,?5)",
        (
            &request.project_id,
            &request.idempotency_key,
            &request_digest,
            &result_json,
            crate::store::now_unix(),
        ),
    )?;
    Ok(result)
}

pub fn get_verification(store: &Store, project_id: &str, verification_id: &str) -> Result<Value> {
    validate::require_id("project_id", project_id)?;
    validate::require_id("verification_id", verification_id)?;
    verification_payload(store, project_id, verification_id)
}

pub fn list_verifications(
    store: &Store,
    project_id: &str,
    changeset_id: Option<&str>,
) -> Result<Vec<Value>> {
    validate::require_id("project_id", project_id)?;
    let mut statement = if let Some(changeset_id) = changeset_id {
        validate::require_id("changeset_id", changeset_id)?;
        let mut statement = store.conn.prepare(
            "SELECT verification_id FROM verification_runs
             WHERE project_id=?1 AND changeset_id=?2 ORDER BY created_at DESC",
        )?;
        let rows =
            statement.query_map((project_id, changeset_id), |row| row.get::<_, String>(0))?;
        rows.collect::<rusqlite::Result<Vec<String>>>()?
    } else {
        let mut statement = store.conn.prepare(
            "SELECT verification_id FROM verification_runs
             WHERE project_id=?1 ORDER BY created_at DESC",
        )?;
        let rows = statement.query_map((project_id,), |row| row.get::<_, String>(0))?;
        rows.collect::<rusqlite::Result<Vec<String>>>()?
    };
    let mut summaries = Vec::new();
    for verification_id in statement.drain(..) {
        let payload = verification_payload(store, project_id, &verification_id)?;
        summaries.push(json!({
            "verification_id": payload["verification_id"],
            "changeset_id": payload["changeset_id"],
            "snapshot_id": payload["snapshot_id"],
            "state": payload["state"],
            "acceptance": payload["verdict"]["acceptance"],
            "required_not_passed": payload["verdict"]["required_not_passed"],
            "started_at": payload["started_at"],
            "finished_at": payload["finished_at"],
        }));
    }
    Ok(summaries)
}

/// Record acceptance only when every required frozen check passed on the
/// resulting snapshot, and that snapshot still matches the workspace.
pub fn accept_changeset(store: &mut Store, raw: &Value) -> Result<Value> {
    let request: AcceptRequest = serde_json::from_value(raw.clone())
        .map_err(|err| CoreError::invalid(format!("accept request: {err}")))?;
    validate::require_id("project_id", &request.project_id)?;
    validate::require_id("changeset_id", &request.changeset_id)?;
    validate::require_id("verification_id", &request.verification_id)?;
    validate::require_id("workspace_handle", &request.workspace_handle)?;
    validate::require_non_empty("actor", &request.actor)?;
    validate::require_id("idempotency_key", &request.idempotency_key)?;
    let request_digest = hash::digest_value(raw)
        .map_err(|err| CoreError::invalid(format!("accept request: {err}")))?;
    if let Some((recorded, _, result)) = store.idempotency_get(
        &request.project_id,
        "acceptChangeSet",
        &request.idempotency_key,
    )? {
        if recorded != request_digest {
            return Err(CoreError::conflict(
                "scope_conflict: idempotency_key was used with a different acceptance request",
            ));
        }
        return serde_json::from_str(&result)
            .map_err(|err| CoreError::Storage(format!("stored acceptance corrupt: {err}")));
    }
    let (revision, changeset) =
        latest_changeset(store, &request.project_id, &request.changeset_id)?.ok_or_else(|| {
            CoreError::not_found(format!("changeset {} not found", request.changeset_id))
        })?;
    if request.expected_revision != revision {
        return Err(CoreError::conflict(format!(
            "stale_snapshot: expected changeset revision {} does not match {revision}",
            request.expected_revision
        )));
    }
    if changeset["state"] != "verifying" {
        return Err(CoreError::conflict(format!(
            "changeset {} is {}; acceptance requires a completed verification after apply",
            request.changeset_id, changeset["state"]
        )));
    }
    let verification = verification_payload(store, &request.project_id, &request.verification_id)?;
    if verification["state"] != "accepted" || verification["verdict"]["acceptance"] != true {
        return Err(CoreError::conflict(
            "verification_not_accepted: not every required frozen check passed",
        ));
    }
    let snapshot_id = changeset["resulting_snapshot_id"]
        .as_str()
        .unwrap_or("")
        .to_string();
    if verification["snapshot_id"].as_str() != Some(snapshot_id.as_str())
        || verification["contract_digest"].as_str() != changeset["contract_digest"].as_str()
        || verification["toolchain_digest"].as_str() != changeset["toolchain_digest"].as_str()
    {
        return Err(CoreError::conflict(
            "stale_snapshot: the verification is not bound to this resulting snapshot/contract/toolchain",
        ));
    }
    let root = resolve_workspace_root(store, &request.workspace_handle, &request.project_id)?;
    let drift = snapshot_drift(store, &request.project_id, &snapshot_id, &root)?;
    if !drift.is_empty() {
        let failed = append_state(
            store,
            changeset,
            "failed",
            json!({
                "error": "evidence_invalid: source changed after verification",
                "verification_id": request.verification_id,
                "drift": drift,
            }),
        )?;
        let _ = failed;
        return Err(CoreError::conflict(
            "evidence_invalid: the workspace no longer matches the verified snapshot",
        ));
    }
    let accepted = append_state(
        store,
        changeset,
        "accepted",
        json!({
            "accepted_at": crate::store::now_unix(),
            "accepted_by": request.actor,
            "verification_id": request.verification_id,
            "resulting_snapshot_id": snapshot_id.clone(),
        }),
    )?;
    let result =
        crate::changeset::get_changeset(store, &request.project_id, &request.changeset_id)?;
    let result_json = serde_json::to_string(&result)
        .map_err(|err| CoreError::Storage(format!("acceptance encode: {err}")))?;
    store.conn.execute(
        "INSERT INTO idempotency
         (project_id,operation,idempotency_key,request_digest,status_code,result_json,created_at)
         VALUES (?1,'acceptChangeSet',?2,?3,200,?4,?5)",
        (
            &request.project_id,
            &request.idempotency_key,
            &request_digest,
            &result_json,
            crate::store::now_unix(),
        ),
    )?;
    let _ = accepted;
    Ok(result)
}

fn run_check(
    store: &mut Store,
    request: &VerifyRequest,
    _changeset: &Value,
    verification_id: &str,
    check: &ContractCheck,
    root: &Path,
    snapshot_id: &str,
) -> CheckOutcome {
    if let Some(command) = &check.command {
        return run_command_check(
            store,
            request,
            verification_id,
            check,
            command,
            root,
            snapshot_id,
        );
    }
    if check.checker_id.is_some() {
        return run_checker_check(store, request, check, root, snapshot_id);
    }
    CheckOutcome {
        state: "not_run".to_string(),
        exit_code: None,
        timed_out: false,
        stdout_digest: None,
        stderr_digest: None,
        started_ms: None,
        finished_ms: None,
        artifacts: vec![],
        detail: json!({"reason": "the check declares neither a command nor a checker"}),
        command_json: None,
    }
}

fn run_command_check(
    store: &mut Store,
    request: &VerifyRequest,
    verification_id: &str,
    check: &ContractCheck,
    command_value: &Value,
    root: &Path,
    snapshot_id: &str,
) -> CheckOutcome {
    let command: CommandSpec = match serde_json::from_value(command_value.clone()) {
        Ok(command) => command,
        Err(error) => {
            return error_outcome(
                &format!("invalid command object: {error}"),
                Some(command_value),
            );
        }
    };
    if command.argv.is_empty() || command.argv[0].is_empty() {
        return error_outcome("command argv must not be empty", Some(command_value));
    }
    let timeout_ms = command.timeout_ms.unwrap_or(60_000);
    if timeout_ms == 0 || timeout_ms > MAX_TIMEOUT_MS {
        return error_outcome(
            &format!("timeout_ms must be 1..={MAX_TIMEOUT_MS}"),
            Some(command_value),
        );
    }
    let env: Vec<(String, String)> = command
        .env
        .clone()
        .unwrap_or_default()
        .into_iter()
        .collect();
    let command_json = hash::canonical_json(command_value).ok();
    let mut exec_argv = command.argv.clone();
    // A declared workspace_copy sandbox is materialized from the resulting
    // snapshot before the command starts, so the check reads exactly the
    // verified bytes and writes only inside the ledger-owned copy.
    let mut sandbox = match &check.sandbox {
        Some(spec) if spec.mode == "workspace_copy" => {
            match materialize_check_sandbox(
                store,
                &request.project_id,
                verification_id,
                &check.id,
                snapshot_id,
                spec,
                command.cwd.as_deref(),
            ) {
                Ok(sandbox) => sandbox,
                Err(error) => {
                    let mut outcome = error_outcome(&error, Some(command_value));
                    outcome.detail["sandbox"] = json!({
                        "mode": "workspace_copy",
                        "declared": true,
                        "isolated": true,
                        "materialized": false,
                        "reason": error,
                    });
                    return outcome;
                }
            }
        }
        Some(spec) if spec.mode == "container" => {
            match prepare_container_run(
                store,
                &request.project_id,
                verification_id,
                &check.id,
                snapshot_id,
                spec,
                command.cwd.as_deref(),
                &command.argv,
            ) {
                Ok((sandbox, argv)) => {
                    exec_argv = argv;
                    sandbox
                }
                Err(error) => {
                    let mut outcome = error_outcome(&error, Some(command_value));
                    outcome.detail["sandbox"] = json!({
                        "mode": "container",
                        "declared": true,
                        "isolated": true,
                        "container_started": false,
                        "reason": error,
                    });
                    return outcome;
                }
            }
        }
        Some(spec) if spec.mode == "workspace" => {
            match SandboxRun::workspace(root, Some(spec), command.cwd.as_deref()) {
                Ok(sandbox) => sandbox,
                Err(error) => {
                    let mut outcome = error_outcome(&error.to_string(), Some(command_value));
                    outcome.detail["sandbox"] = json!({
                        "mode": spec.mode,
                        "declared": true,
                        "isolated": false,
                        "enforcement": "none",
                    });
                    return outcome;
                }
            }
        }
        Some(spec) => {
            let mut outcome = error_outcome(
                &format!("unsupported sandbox mode {}", spec.mode),
                Some(command_value),
            );
            outcome.detail["sandbox"] = json!({
                "mode": spec.mode,
                "declared": true,
                "isolated": false,
                "materialized": false,
            });
            return outcome;
        }
        None => match SandboxRun::workspace(root, None, command.cwd.as_deref()) {
            Ok(sandbox) => sandbox,
            Err(error) => return error_outcome(&error.to_string(), Some(command_value)),
        },
    };
    let capture = match run_command_capture(
        &exec_argv,
        &sandbox.cwd,
        &env,
        Duration::from_millis(timeout_ms),
        None,
    ) {
        Ok(capture) => capture,
        Err(error) => {
            let state = if error.to_string().contains("not found") {
                "unsupported"
            } else {
                "error"
            };
            sandbox.finish_container();
            let sandbox_detail = sandbox.settle(state);
            return CheckOutcome {
                state: state.to_string(),
                exit_code: None,
                timed_out: false,
                stdout_digest: None,
                stderr_digest: None,
                started_ms: None,
                finished_ms: None,
                artifacts: vec![],
                detail: json!({
                    "reason": error.to_string(),
                    "grant_id": request.execution_grant.grant_id,
                    "sandbox": sandbox_detail,
                }),
                command_json,
            };
        }
    };
    let stdout_digest = store.put_blob(&capture.stdout, "text/plain").ok();
    let stderr_digest = store.put_blob(&capture.stderr, "text/plain").ok();
    let (state, predicate_detail, artifacts) = if capture.timed_out {
        (
            "timed_out".to_string(),
            json!({
                "reason": "the check exceeded its declared timeout",
                "timeout_ms": timeout_ms,
                "process_audit": capture.process_audit.clone(),
            }),
            vec![],
        )
    } else if capture.cancelled {
        (
            "cancelled".to_string(),
            json!({"reason": "cancelled"}),
            vec![],
        )
    } else {
        sandbox.finish_container();
        evaluate_predicate(
            store,
            check,
            &check.predicate,
            capture.exit_code,
            &sandbox.cwd,
            &sandbox.eval_root,
            snapshot_id,
        )
    };
    let sandbox_detail = sandbox.settle(&state);
    CheckOutcome {
        state,
        exit_code: capture.exit_code,
        timed_out: capture.timed_out,
        stdout_digest,
        stderr_digest,
        started_ms: Some(capture.started_ms),
        finished_ms: Some(capture.finished_ms),
        artifacts,
        detail: json!({
            "predicate": predicate_detail,
            "completion": capture.completion,
            "stdout_truncated": capture.stdout_truncated,
            "stderr_truncated": capture.stderr_truncated,
            "process_audit": capture.process_audit,
            "grant_id": request.execution_grant.grant_id,
            "grant_reason": request.execution_grant.reason,
            "env_names": capture.env_names,
            "wall_ms": capture.wall_ms,
            "sandbox": sandbox_detail,
        }),
        command_json,
    }
}

/// Execution root for one check. `root` is `Some` only for a materialized
/// workspace copy; a failed check keeps the copy for inspection.
struct SandboxRun {
    root: Option<PathBuf>,
    cwd: PathBuf,
    eval_root: PathBuf,
    detail: Value,
    container: Option<crate::sandbox::ContainerRun>,
}

impl SandboxRun {
    fn workspace(
        workspace_root: &Path,
        spec: Option<&ContractSandbox>,
        command_cwd: Option<&str>,
    ) -> Result<Self> {
        let cwd = resolve_cwd(workspace_root, command_cwd)?;
        Ok(Self {
            root: None,
            cwd,
            eval_root: workspace_root.to_path_buf(),
            detail: json!({
                "mode": spec.map(|value| value.mode.as_str()).unwrap_or("workspace"),
                "declared": spec.is_some(),
                "isolated": false,
                "enforcement": "none",
            }),
            container: None,
        })
    }

    /// Stop and remove a container if it survived the command, and record the
    /// verification result in the sandbox detail.
    fn finish_container(&mut self) {
        let Some(container) = &self.container else {
            return;
        };
        let removed = container.removed();
        self.detail["container"] = container.detail(removed);
    }

    /// Remove a passing sandbox. Non-passed states retain it and say so.
    fn settle(&mut self, state: &str) -> Value {
        let retained = self.root.is_some() && state != "passed";
        if let Some(root) = &self.root {
            if !retained {
                let _ = fs::remove_dir_all(root);
            }
        }
        let mut detail = self.detail.clone();
        detail["retained"] = Value::Bool(retained);
        detail
    }
}

fn prepare_container_run(
    store: &mut Store,
    project_id: &str,
    verification_id: &str,
    check_id: &str,
    snapshot_id: &str,
    spec: &ContractSandbox,
    command_cwd: Option<&str>,
    argv: &[String],
) -> std::result::Result<(SandboxRun, Vec<String>), String> {
    let request = crate::sandbox::SandboxRequest {
        include: spec.include.clone(),
        exclude: spec.exclude.clone(),
        image: spec.image.clone(),
        network: spec.network.clone(),
        cwd: command_cwd.map(str::to_string),
    };
    let scope = ["verify-sandboxes", verification_id, check_id];
    let (materialized, container, exec_argv) = crate::sandbox::prepare_container(
        store,
        project_id,
        snapshot_id,
        &scope,
        &request,
        &[],
        argv,
    )
    .map_err(|error| error.to_string())?;
    let mut detail = materialized.detail.clone();
    detail["mode"] = json!("container");
    detail["declared"] = json!(true);
    detail["isolated"] = json!(true);
    detail["enforcement"] = json!("oci_container");
    detail["image"] = json!(container.image);
    detail["image_id"] = json!(container.image_id);
    detail["repo_digests"] = json!(container.repo_digests);
    detail["platform"] = json!(container.platform);
    detail["network"] = json!("none");
    detail["read_only"] = json!(true);
    detail["tmpfs"] = json!("/tmp:rw,size=64m");
    detail["workdir"] = json!(container.workdir);
    detail["container_name"] = json!(container.name);
    detail["limitations"] = json!([
        "Docker/Linux containers only; the host must provide a local pinned image (no pull)",
        "the process runs as the image default user and can write the bind-mounted snapshot copy",
        "a timeout triggers docker rm -f and the result records whether the container was removed"
    ]);
    Ok((
        SandboxRun {
            root: Some(materialized.root.clone()),
            cwd: materialized.cwd,
            eval_root: materialized.root,
            detail,
            container: Some(container),
        },
        exec_argv,
    ))
}

fn materialize_check_sandbox(
    store: &mut Store,
    project_id: &str,
    verification_id: &str,
    check_id: &str,
    snapshot_id: &str,
    spec: &ContractSandbox,
    command_cwd: Option<&str>,
) -> std::result::Result<SandboxRun, String> {
    let request = crate::sandbox::SandboxRequest {
        include: spec.include.clone(),
        exclude: spec.exclude.clone(),
        image: None,
        network: None,
        cwd: command_cwd.map(str::to_string),
    };
    let scope = ["verify-sandboxes", verification_id, check_id];
    let materialized = crate::sandbox::materialize(store, project_id, snapshot_id, &scope, &request)
        .map_err(|error| error.to_string())?;
    let mut detail = materialized.detail.clone();
    detail["mode"] = json!("workspace_copy");
    detail["declared"] = json!(true);
    detail["isolated"] = json!(true);
    detail["enforcement"] = json!("materialized_snapshot_copy");
    detail["limitations"] = json!([
        "the process can still address absolute paths outside the sandbox; OS-level confinement is not applied"
    ]);
    Ok(SandboxRun {
        root: Some(materialized.root.clone()),
        cwd: materialized.cwd,
        eval_root: materialized.root,
        container: None,
        detail,
    })
}

fn run_checker_check(
    store: &mut Store,
    request: &VerifyRequest,
    check: &ContractCheck,
    root: &Path,
    snapshot_id: &str,
) -> CheckOutcome {
    let started = now_ms();
    let checker = check.checker_id.as_deref().unwrap_or("");
    let predicate = &check.predicate;
    let kind = predicate["kind"].as_str().unwrap_or("");
    let (state, detail, artifacts) = match (checker, kind) {
        ("architecture.rules.v1", "architecture_rules")
        | ("architecture.rules.v1", "architecture_rules_v1") => {
            architecture_check(store, request, predicate, snapshot_id)
        }
        ("file.digest.v1", "file_digest") => {
            file_digest_check(store, &request.project_id, snapshot_id, predicate, root)
        }
        ("numeric.tolerance.v1", "numeric_tolerance") => predicate::evaluate_numeric_tolerance(
            store,
            predicate,
            root,
            Some(checker),
            None,
        ),
        ("numeric.invariants.v1", "invariant") => {
            predicate::evaluate_invariant(store, predicate, root, Some(checker), None)
        }
        ("performance.budget.v1", "performance_budget") => {
            predicate::evaluate_performance_budget(store, predicate, root, Some(checker), None)
        }
        (_, "numeric_tolerance") | (_, "invariant") | (_, "performance_budget") => (
            "unsupported".to_string(),
            json!({
                "reason": format!("no executable checker {checker} is registered for predicate {kind}"),
                "checker_id": checker,
            }),
            vec![],
        ),
        _ => (
            "unsupported".to_string(),
            json!({"reason": format!("unregistered checker {checker}"), "checker_id": checker}),
            vec![],
        ),
    };
    CheckOutcome {
        state,
        exit_code: None,
        timed_out: false,
        stdout_digest: None,
        stderr_digest: None,
        started_ms: Some(started),
        finished_ms: Some(now_ms()),
        artifacts,
        detail,
        command_json: None,
    }
}

fn evaluate_predicate(
    store: &mut Store,
    _check: &ContractCheck,
    predicate: &Value,
    exit_code: Option<i32>,
    cwd: &Path,
    root: &Path,
    _snapshot_id: &str,
) -> (String, Value, Vec<Value>) {
    let kind = predicate["kind"].as_str().unwrap_or("");
    match kind {
        "exit_code" => {
            let expected: Vec<i64> = predicate["expected_codes"]
                .as_array()
                .map(|codes| codes.iter().filter_map(Value::as_i64).collect())
                .unwrap_or_default();
            if expected.is_empty() {
                return (
                    "error".to_string(),
                    json!({"reason": "exit_code predicate has no expected_codes"}),
                    vec![],
                );
            }
            match exit_code {
                Some(code) if expected.contains(&(code as i64)) => (
                    "passed".to_string(),
                    json!({"kind": kind, "expected_codes": expected, "actual": code}),
                    vec![],
                ),
                Some(code) => (
                    "failed".to_string(),
                    json!({"kind": kind, "expected_codes": expected, "actual": code}),
                    vec![],
                ),
                None => (
                    "error".to_string(),
                    json!({"reason": "the process did not report an exit code", "expected_codes": expected}),
                    vec![],
                ),
            }
        }
        "test_report" => test_report_check(store, predicate, exit_code, cwd, root),
        "numeric_tolerance" => predicate::evaluate_numeric_tolerance(
            store,
            predicate,
            root,
            None,
            exit_code,
        ),
        "invariant" => predicate::evaluate_invariant(store, predicate, root, None, exit_code),
        "performance_budget" => {
            predicate::evaluate_performance_budget(store, predicate, root, None, exit_code)
        }
        _ => (
            "unsupported".to_string(),
            json!({"reason": format!("predicate {kind} is not executable by a command check")}),
            vec![],
        ),
    }
}

fn test_report_check(
    store: &mut Store,
    predicate: &Value,
    exit_code: Option<i32>,
    cwd: &Path,
    root: &Path,
) -> (String, Value, Vec<Value>) {
    let format = predicate["format"].as_str().unwrap_or("");
    if !matches!(format, "junit" | "json") {
        return (
            "error".to_string(),
            json!({"reason": "test_report requires format=junit or json"}),
            vec![],
        );
    }
    let Some(report_path) = predicate["report_path"].as_str() else {
        return (
            "error".to_string(),
            json!({"reason": "test_report requires report_path"}),
            vec![],
        );
    };
    let min_tests = predicate["min_tests"].as_i64().unwrap_or(1);
    let max_failures = predicate["max_failures"].as_i64().unwrap_or(0);
    if min_tests < 1 || max_failures < 0 {
        return (
            "error".to_string(),
            json!({"reason": "min_tests must be >= 1 and max_failures >= 0"}),
            vec![],
        );
    }
    if exit_code != Some(0) {
        return (
            "failed".to_string(),
            json!({"reason": "the test command exited non-zero", "exit_code": exit_code}),
            vec![],
        );
    }
    let report = match read_report(cwd, root, report_path) {
        Ok(report) => report,
        Err(error) => {
            return (
                "failed".to_string(),
                json!({"reason": error.to_string(), "report_path": report_path}),
                vec![],
            );
        }
    };
    let counts = match format {
        "junit" => parse_junit(&String::from_utf8_lossy(&report)),
        _ => parse_json_report(&report),
    };
    let Some(counts) = counts else {
        return (
            "failed".to_string(),
            json!({"reason": "test report is corrupt or unreadable", "report_path": report_path}),
            vec![],
        );
    };
    let artifact = store
        .put_blob(&report, "application/octet-stream")
        .ok()
        .map(|digest| json!({"role": "test_report", "digest": digest, "path": report_path}));
    let artifact_json = artifact.into_iter().collect::<Vec<_>>();
    let failures = counts.failures + counts.errors;
    if counts.tests < min_tests {
        return (
            "failed".to_string(),
            json!({
                "reason": "the report contains fewer tests than required",
                "tests": counts.tests,
                "min_tests": min_tests,
                "failures": counts.failures,
                "errors": counts.errors,
                "skipped": counts.skipped,
            }),
            artifact_json,
        );
    }
    if failures > max_failures {
        return (
            "failed".to_string(),
            json!({
                "reason": "the report contains more failures than allowed",
                "tests": counts.tests,
                "failures": counts.failures,
                "errors": counts.errors,
                "max_failures": max_failures,
            }),
            artifact_json,
        );
    }
    (
        "passed".to_string(),
        json!({
            "kind": "test_report",
            "format": format,
            "tests": counts.tests,
            "failures": counts.failures,
            "errors": counts.errors,
            "skipped": counts.skipped,
            "min_tests": min_tests,
            "max_failures": max_failures,
        }),
        artifact_json,
    )
}

fn file_digest_check(
    store: &Store,
    project_id: &str,
    snapshot_id: &str,
    predicate: &Value,
    root: &Path,
) -> (String, Value, Vec<Value>) {
    let path = predicate["path"].as_str().unwrap_or("");
    let expected = predicate["expected_digest"].as_str().unwrap_or("");
    if path.is_empty() || !expected.starts_with("sha256:") {
        return (
            "error".to_string(),
            json!({"reason": "file_digest requires path and expected_digest"}),
            vec![],
        );
    }
    if let Err(error) = validate::validate_path("path", path) {
        return (
            "error".to_string(),
            json!({"reason": error.to_string()}),
            vec![],
        );
    }
    let snapshot_digest: Option<String> = store
        .conn
        .query_row(
            "SELECT blob_digest FROM files WHERE project_id=?1 AND snapshot_id=?2 AND path=?3",
            (project_id, snapshot_id, path),
            |row| row.get(0),
        )
        .ok();
    let Some(_recorded) = snapshot_digest else {
        return (
            "failed".to_string(),
            json!({"reason": "path is not part of the verified snapshot", "path": path}),
            vec![],
        );
    };
    match safe_resolve(root, path).and_then(|target| Ok(hash::digest_bytes(&fs::read(&target)?))) {
        Ok(actual) if actual == expected => (
            "passed".to_string(),
            json!({"kind": "file_digest", "path": path, "digest": actual}),
            vec![],
        ),
        Ok(actual) => (
            "failed".to_string(),
            json!({"kind": "file_digest", "path": path, "expected": expected, "actual": actual}),
            vec![],
        ),
        Err(error) => (
            "failed".to_string(),
            json!({"kind": "file_digest", "path": path, "reason": error.to_string()}),
            vec![],
        ),
    }
}

fn architecture_check(
    store: &Store,
    request: &VerifyRequest,
    predicate: &Value,
    snapshot_id: &str,
) -> (String, Value, Vec<Value>) {
    let architecture_id = predicate["architecture_id"].as_str().unwrap_or("");
    if architecture_id.is_empty() {
        return (
            "error".to_string(),
            json!({"reason": "architecture_rules requires architecture_id"}),
            vec![],
        );
    }
    let architecture = match get_architecture(store, &request.project_id, architecture_id) {
        Ok(architecture) => architecture,
        Err(error) => {
            return (
                "failed".to_string(),
                json!({"reason": error.to_string(), "architecture_id": architecture_id}),
                vec![],
            )
        }
    };
    if architecture["snapshot_id"].as_str() != Some(snapshot_id) {
        return (
            "failed".to_string(),
            json!({
                "reason": "the architecture is bound to a different snapshot",
                "architecture_snapshot_id": architecture["snapshot_id"],
                "verification_snapshot_id": snapshot_id,
            }),
            vec![],
        );
    }
    if let Some(revision) = predicate["revision"].as_i64() {
        if architecture["revision"].as_i64() != Some(revision) {
            return (
                "failed".to_string(),
                json!({"reason": "the architecture revision is not the requested one"}),
                vec![],
            );
        }
    }
    let rule_ids: Vec<String> = predicate["rule_ids"]
        .as_array()
        .map(|ids| {
            ids.iter()
                .filter_map(Value::as_str)
                .map(str::to_string)
                .collect()
        })
        .unwrap_or_default();
    let result = match check_architecture_rules(
        store,
        &request.project_id,
        architecture_id,
        (!rule_ids.is_empty()).then_some(rule_ids.as_slice()),
    ) {
        Ok(result) => result,
        Err(error) => {
            return (
                "failed".to_string(),
                json!({"reason": error.to_string()}),
                vec![],
            )
        }
    };
    let state = match result["state"].as_str().unwrap_or("unverified") {
        "passed" => "passed",
        "failed" | "blocked" => "failed",
        "incomplete_coverage" | "unverified" => "inconclusive",
        _ => "inconclusive",
    };
    (
        state.to_string(),
        json!({
            "kind": "architecture_rules",
            "architecture_id": architecture_id,
            "rule_ids": rule_ids,
            "architecture_state": result["state"],
            "blocking_rules": result["blocking_rules"],
            "results": result["results"],
        }),
        vec![],
    )
}

fn evaluate(
    changeset: &Value,
    contract: &Value,
    contract_checks: &[ContractCheck],
    outcomes: &[(ContractCheck, CheckOutcome)],
    drift: &[Value],
    snapshot_id: &str,
    toolchain_digest: &str,
) -> Value {
    let outcome_by_id: BTreeMap<&str, &CheckOutcome> = outcomes
        .iter()
        .map(|(check, outcome)| (check.id.as_str(), outcome))
        .collect();
    let mut required_not_passed = Vec::new();
    let mut optional_not_passed = Vec::new();
    let mut states = BTreeMap::new();
    for check in contract_checks {
        let outcome = outcome_by_id.get(check.id.as_str());
        let state = outcome
            .map(|outcome| outcome.state.as_str())
            .unwrap_or("not_run");
        states.insert(check.id.clone(), state.to_string());
        if state != "passed" {
            let entry = json!({"check_id": check.id, "state": state, "required": check.required});
            if check.required {
                required_not_passed.push(entry);
            } else {
                optional_not_passed.push(entry);
            }
        }
    }
    let missing: Vec<String> = contract_checks
        .iter()
        .filter(|check| !outcome_by_id.contains_key(check.id.as_str()))
        .map(|check| check.id.clone())
        .collect();
    let result_ids: BTreeSet<&str> = outcomes
        .iter()
        .map(|(check, _)| check.id.as_str())
        .collect();
    let extra: Vec<String> = result_ids
        .iter()
        .filter(|id| !contract_checks.iter().any(|check| check.id == **id))
        .map(|id| id.to_string())
        .collect();
    let binding_matches = changeset["resulting_snapshot_id"].as_str() == Some(snapshot_id)
        && changeset["contract_digest"].as_str() == contract["digest"].as_str()
        && changeset["toolchain_digest"].as_str() == Some(toolchain_digest);
    let accepted = required_not_passed.is_empty()
        && missing.is_empty()
        && extra.is_empty()
        && drift.is_empty()
        && binding_matches;
    let state = if accepted { "accepted" } else { "failed" };
    let reasons = if accepted {
        vec![]
    } else {
        let mut reasons = Vec::new();
        if !drift.is_empty() {
            reasons.push(
                "evidence_invalid: the workspace changed relative to the resulting snapshot"
                    .to_string(),
            );
        }
        if !missing.is_empty() {
            reasons.push(format!("missing check results: {}", missing.join(", ")));
        }
        if !extra.is_empty() {
            reasons.push(format!(
                "results outside the frozen contract: {}",
                extra.join(", ")
            ));
        }
        if !binding_matches {
            reasons.push("snapshot/contract/toolchain binding mismatch".to_string());
        }
        if !required_not_passed.is_empty() {
            reasons.push(format!(
                "{} required check(s) did not pass",
                required_not_passed.len()
            ));
        }
        reasons
    };
    json!({
        "state": state,
        "acceptance": accepted,
        "snapshot_id": snapshot_id,
        "contract_digest": contract["digest"],
        "toolchain_digest": toolchain_digest,
        "checks_total": contract_checks.len(),
        "required_total": contract_checks.iter().filter(|check| check.required).count(),
        "states": states,
        "required_not_passed": required_not_passed,
        "optional_not_passed": optional_not_passed,
        "missing_results": missing,
        "extra_results": extra,
        "drift": drift,
        "reasons": reasons,
        "coverage": {
            "state": if accepted { "complete" } else { "partial" },
            "scope": "frozen contract checks against the resulting applied snapshot",
            "limitations": if accepted { vec![] } else { reasons },
        },
    })
}

fn verification_payload(store: &Store, project_id: &str, verification_id: &str) -> Result<Value> {
    let row: Option<(
        String,
        String,
        i64,
        String,
        String,
        String,
        String,
        String,
        String,
        String,
        String,
        i64,
        Option<i64>,
    )> = store
        .conn
        .query_row(
            "SELECT changeset_id,state,changeset_revision,snapshot_id,base_snapshot_id,
                        contract_id,contract_digest,toolchain_digest,environment_json,state,
                        verdict_json,started_at,finished_at
                 FROM verification_runs WHERE project_id=?1 AND verification_id=?2",
            (project_id, verification_id),
            |row| {
                Ok((
                    row.get(0)?,
                    row.get(1)?,
                    row.get(2)?,
                    row.get(3)?,
                    row.get(4)?,
                    row.get(5)?,
                    row.get(6)?,
                    row.get(7)?,
                    row.get(8)?,
                    row.get(9)?,
                    row.get(10)?,
                    row.get(11)?,
                    row.get(12)?,
                ))
            },
        )
        .ok();
    let Some((
        changeset_id,
        _state,
        revision,
        snapshot_id,
        base_snapshot_id,
        contract_id,
        contract_digest,
        toolchain_digest,
        environment,
        state,
        verdict,
        started_at,
        finished_at,
    )) = row
    else {
        return Err(CoreError::not_found(format!(
            "verification {verification_id} not found"
        )));
    };
    let verdict: Value = serde_json::from_str(&verdict)
        .map_err(|err| CoreError::Storage(format!("verdict corrupt: {err}")))?;
    let environment: Value = serde_json::from_str(&environment)
        .map_err(|err| CoreError::Storage(format!("environment corrupt: {err}")))?;
    let mut statement = store.conn.prepare(
        "SELECT check_id,kind,required,state,command_json,exit_code,timed_out,
                stdout_digest,stderr_digest,started_ms,finished_ms,artifacts_json,detail_json
         FROM check_results WHERE project_id=?1 AND verification_id=?2 ORDER BY check_id",
    )?;
    let rows = statement.query_map((project_id, verification_id), |row| {
        Ok((
            row.get::<_, String>(0)?,
            row.get::<_, String>(1)?,
            row.get::<_, i64>(2)?,
            row.get::<_, String>(3)?,
            row.get::<_, Option<String>>(4)?,
            row.get::<_, Option<i64>>(5)?,
            row.get::<_, i64>(6)?,
            row.get::<_, Option<String>>(7)?,
            row.get::<_, Option<String>>(8)?,
            row.get::<_, Option<i64>>(9)?,
            row.get::<_, Option<i64>>(10)?,
            row.get::<_, String>(11)?,
            row.get::<_, String>(12)?,
        ))
    })?;
    let mut checks = Vec::new();
    for row in rows {
        let (
            check_id,
            kind,
            required,
            state,
            command_json,
            exit_code,
            timed_out,
            stdout_digest,
            stderr_digest,
            started_ms,
            finished_ms,
            artifacts,
            detail,
        ) = row?;
        checks.push(json!({
            "check_id": check_id,
            "kind": kind,
            "required": required == 1,
            "state": state,
            "command": command_json
                .and_then(|value| serde_json::from_str::<Value>(&value).ok()),
            "exit_code": exit_code,
            "timed_out": timed_out == 1,
            "stdout_digest": stdout_digest,
            "stderr_digest": stderr_digest,
            "started_ms": started_ms,
            "finished_ms": finished_ms,
            "artifacts": serde_json::from_str::<Value>(&artifacts).unwrap_or(Value::Null),
            "detail": serde_json::from_str::<Value>(&detail).unwrap_or(Value::Null),
        }));
    }
    Ok(json!({
        "verification_id": verification_id,
        "project_id": project_id,
        "changeset_id": changeset_id,
        "changeset_revision": revision,
        "snapshot_id": snapshot_id,
        "base_snapshot_id": base_snapshot_id,
        "contract_id": contract_id,
        "contract_digest": contract_digest,
        "toolchain_digest": toolchain_digest,
        "environment": environment,
        "state": state,
        "verdict": verdict,
        "checks": checks,
        "started_at": started_at,
        "finished_at": finished_at,
    }))
}

fn snapshot_drift(
    store: &Store,
    project_id: &str,
    snapshot_id: &str,
    root: &Path,
) -> Result<Vec<Value>> {
    let mut statement = store.conn.prepare(
        "SELECT path, blob_digest FROM files WHERE project_id=?1 AND snapshot_id=?2 ORDER BY path",
    )?;
    let rows = statement.query_map((project_id, snapshot_id), |row| {
        Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
    })?;
    let mut drift = Vec::new();
    for row in rows {
        let (path, expected) = row?;
        match safe_resolve(root, &path) {
            Ok(target) => match fs::read(&target) {
                Ok(bytes) => {
                    let actual = hash::digest_bytes(&bytes);
                    if actual != expected {
                        drift.push(json!({
                            "path": path,
                            "state": "changed",
                            "expected_digest": expected,
                            "actual_digest": actual,
                        }));
                    }
                }
                Err(error) => drift.push(json!({
                    "path": path,
                    "state": "unreadable",
                    "expected_digest": expected,
                    "reason": error.to_string(),
                })),
            },
            Err(error) => drift.push(json!({
                "path": path,
                "state": "unsafe_or_missing",
                "expected_digest": expected,
                "reason": error.to_string(),
            })),
        }
    }
    Ok(drift)
}

fn resolve_cwd(root: &Path, cwd: Option<&str>) -> Result<PathBuf> {
    let canonical_root = fs::canonicalize(root)?;
    let candidate = match cwd {
        None | Some(".") | Some("") => canonical_root.clone(),
        Some(relative) => {
            validate::validate_path("command.cwd", relative)?;
            canonical_root.join(relative)
        }
    };
    let canonical = fs::canonicalize(&candidate).map_err(|err| {
        CoreError::invalid(format!("command cwd is missing or unreadable: {err}"))
    })?;
    if !canonical.starts_with(&canonical_root) {
        return Err(CoreError::invalid("command cwd escapes the workspace"));
    }
    if !canonical.is_dir() {
        return Err(CoreError::invalid("command cwd is not a directory"));
    }
    Ok(canonical)
}

fn read_report(cwd: &Path, root: &Path, report_path: &str) -> Result<Vec<u8>> {
    validate::validate_path("report_path", report_path)?;
    let canonical_root = fs::canonicalize(root)?;
    let target = cwd.join(report_path);
    let parent = target
        .parent()
        .ok_or_else(|| CoreError::invalid("report_path has no parent"))?;
    let canonical_parent = fs::canonicalize(parent)
        .map_err(|err| CoreError::invalid(format!("report directory is missing: {err}")))?;
    if !canonical_parent.starts_with(&canonical_root) {
        return Err(CoreError::invalid("report_path escapes the workspace"));
    }
    let metadata = fs::symlink_metadata(&target)
        .map_err(|err| CoreError::invalid(format!("report file is missing: {err}")))?;
    if metadata.file_type().is_symlink() {
        return Err(CoreError::invalid("report_path is a symlink"));
    }
    if !metadata.file_type().is_file() {
        return Err(CoreError::invalid("report_path is not a regular file"));
    }
    fs::read(&target).map_err(CoreError::from)
}

fn parse_junit(text: &str) -> Option<ReportCounts> {
    let tests =
        junit_attr(text, "tests").unwrap_or_else(|| text.matches("<testcase").count() as i64);
    let failures =
        junit_attr(text, "failures").unwrap_or_else(|| text.matches("<failure").count() as i64);
    let errors =
        junit_attr(text, "errors").unwrap_or_else(|| text.matches("<error").count() as i64);
    let skipped =
        junit_attr(text, "skipped").unwrap_or_else(|| text.matches("<skipped").count() as i64);
    Some(ReportCounts {
        tests,
        failures,
        errors,
        skipped,
    })
}

fn junit_attr(text: &str, name: &str) -> Option<i64> {
    let pattern = format!("{name}=\"");
    let start = text.find(&pattern)? + pattern.len();
    let rest = &text[start..];
    let end = rest.find('"')?;
    rest[..end].parse().ok()
}

fn parse_json_report(bytes: &[u8]) -> Option<ReportCounts> {
    let value: Value = serde_json::from_slice(bytes).ok()?;
    let tests = value["tests"].as_i64()?;
    Some(ReportCounts {
        tests,
        failures: value["failures"].as_i64().unwrap_or(0),
        errors: value["errors"].as_i64().unwrap_or(0),
        skipped: value["skipped"].as_i64().unwrap_or(0),
    })
}

fn error_outcome(reason: &str, command: Option<&Value>) -> CheckOutcome {
    CheckOutcome {
        state: "error".to_string(),
        exit_code: None,
        timed_out: false,
        stdout_digest: None,
        stderr_digest: None,
        started_ms: None,
        finished_ms: None,
        artifacts: vec![],
        detail: json!({"reason": reason}),
        command_json: command.and_then(|command| hash::canonical_json(command).ok()),
    }
}

fn now_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as i64
}


