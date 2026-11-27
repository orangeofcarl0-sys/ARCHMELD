// Copyright 2026 ARCHMELD contributors
// SPDX-License-Identifier: Apache-2.0

//! Safe application, writer leases and crash recovery (S09 §4–§6, S19 §3–§4).
//!
//! Source replacement is never described as atomic across files. Every target
//! is checked against the prepared preimage immediately before it is replaced,
//! the intent is journaled durably first, and an interrupted apply is detected
//! from that journal rather than inferred from the ChangeSet state.

use crate::changeset::{
    append_state, append_state_with_journal, ensure_plan_current, get_changeset, latest_changeset,
    SnapshotCaptureOptions,
};
use crate::error::{CoreError, Result};
use crate::hash;
use crate::snapshot::{capture_snapshot, SnapshotInputs};
use crate::store::{validate, Store};
use rusqlite::TransactionBehavior;
use serde::Deserialize;
use serde_json::{json, Value};
use std::fs;
use std::io::Write;
use std::path::{Component, Path, PathBuf};

const MAX_LEASE_SECONDS: u64 = 86_400;
const TEMP_PREFIX: &str = ".archmeld-apply-";

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
struct ApplyRequest {
    project_id: String,
    changeset_id: String,
    expected_revision: i64,
    prepared_digest: String,
    approval_id: String,
    actor: String,
    workspace_handle: String,
    idempotency_key: String,
    lease_ttl_secs: Option<u64>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
struct RecoverRequest {
    project_id: String,
    changeset_id: String,
    expected_revision: i64,
    actor: String,
    workspace_handle: String,
    idempotency_key: String,
    lease_ttl_secs: Option<u64>,
}

#[derive(Debug, Clone)]
pub enum ApplyEvent {
    LeaseAcquired {
        owner: String,
        fencing_token: i64,
        expires_at: i64,
    },
    PreflightPassed,
    BeforeReplace {
        sequence: i64,
        op: String,
        path: String,
    },
    AfterReplace {
        sequence: i64,
        op: String,
        path: String,
        observed_digest: Option<String>,
    },
    SnapshotCaptured {
        snapshot_id: String,
    },
    RecoveryBefore {
        sequence: i64,
        path: String,
    },
    RecoveryAfter {
        sequence: i64,
        path: String,
        conflict: bool,
    },
}

#[derive(Debug, Clone)]
struct ResolvedOperation {
    sequence: i64,
    op: String,
    path: String,
    destination: Option<String>,
    before_digest: Option<String>,
    after_digest: Option<String>,
    case_only: bool,
    executable: bool,
    source: PathBuf,
    target: PathBuf,
    temp: PathBuf,
}

#[derive(Debug, Clone)]
struct JournalRow {
    sequence: i64,
    op: String,
    path: String,
    destination: Option<String>,
    preimage_digest: Option<String>,
    postimage_digest: Option<String>,
    detail: Value,
}

enum PreflightError {
    Stale(String),
    Invalid(String),
}

/// Acquire the single-writer lease. A valid lease owned by somebody else is a
/// conflict; an expired lease is replaced with a strictly larger fencing
/// token, so the previous holder can no longer write.
pub fn acquire_writer_lease(
    store: &mut Store,
    project_id: &str,
    owner: &str,
    ttl_secs: u64,
) -> Result<Value> {
    validate::require_id("project_id", project_id)?;
    validate::require_non_empty("lease owner", owner)?;
    if ttl_secs == 0 || ttl_secs > MAX_LEASE_SECONDS {
        return Err(CoreError::invalid(format!(
            "lease ttl must be 1..={MAX_LEASE_SECONDS} seconds"
        )));
    }
    store.ensure_project_exists(project_id)?;
    let now = crate::store::now_unix();
    let tx = store
        .conn
        .transaction_with_behavior(TransactionBehavior::Immediate)?;
    let existing: Option<(String, i64, i64, Option<i64>)> = tx
        .query_row(
            "SELECT owner, fencing_token, expires_at, released_at FROM writer_leases
             WHERE project_id=?1",
            (project_id,),
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?)),
        )
        .ok();
    if let Some((_held_by, _, expires_at, released_at)) = &existing {
        if released_at.is_none() && *expires_at > now {
            return Err(CoreError::conflict(format!(
                "lease_held: project {project_id} is owned by another writer until {expires_at}"
            )));
        }
    }
    let token = existing
        .as_ref()
        .map(|(_, token, _, _)| token + 1)
        .unwrap_or(1);
    let expires_at = now + ttl_secs as i64;
    tx.execute(
        "INSERT INTO writer_leases
         (project_id, owner, fencing_token, expires_at, released_at, updated_at)
         VALUES (?1,?2,?3,?4,NULL,?5)
         ON CONFLICT(project_id) DO UPDATE SET
           owner=excluded.owner,
           fencing_token=excluded.fencing_token,
           expires_at=excluded.expires_at,
           released_at=NULL,
           updated_at=excluded.updated_at",
        (project_id, owner, token, expires_at, now),
    )?;
    tx.commit()?;
    Ok(json!({
        "project_id": project_id,
        "owner": owner,
        "fencing_token": token,
        "expires_at": expires_at,
    }))
}

pub fn release_writer_lease(
    store: &mut Store,
    project_id: &str,
    owner: &str,
    fencing_token: i64,
) -> Result<bool> {
    validate::require_id("project_id", project_id)?;
    let updated = store.conn.execute(
        "UPDATE writer_leases SET released_at=?1, updated_at=?1
         WHERE project_id=?2 AND owner=?3 AND fencing_token=?4 AND released_at IS NULL",
        (crate::store::now_unix(), project_id, owner, fencing_token),
    )?;
    Ok(updated == 1)
}

pub fn writer_lease(store: &Store, project_id: &str) -> Result<Option<Value>> {
    validate::require_id("project_id", project_id)?;
    let row: Option<(String, i64, i64, Option<i64>, i64)> = store
        .conn
        .query_row(
            "SELECT owner, fencing_token, expires_at, released_at, updated_at
             FROM writer_leases WHERE project_id=?1",
            (project_id,),
            |row| {
                Ok((
                    row.get(0)?,
                    row.get(1)?,
                    row.get(2)?,
                    row.get(3)?,
                    row.get(4)?,
                ))
            },
        )
        .ok();
    Ok(
        row.map(|(owner, token, expires_at, released_at, updated_at)| {
            json!({
                "owner": owner,
                "fencing_token": token,
                "expires_at": expires_at,
                "released_at": released_at,
                "updated_at": updated_at,
            })
        }),
    )
}

pub fn apply_changeset(store: &mut Store, raw: &Value) -> Result<Value> {
    apply_changeset_with_hook(store, raw, None)
}

pub fn apply_changeset_with_hook(
    store: &mut Store,
    raw: &Value,
    mut hook: Option<&mut dyn FnMut(&mut Store, &ApplyEvent) -> Result<()>>,
) -> Result<Value> {
    let request: ApplyRequest = serde_json::from_value(raw.clone())
        .map_err(|err| CoreError::invalid(format!("apply request: {err}")))?;
    validate::require_id("project_id", &request.project_id)?;
    validate::require_id("changeset_id", &request.changeset_id)?;
    validate::require_digest("prepared_digest", &request.prepared_digest)?;
    validate::require_id("approval_id", &request.approval_id)?;
    validate::require_non_empty("actor", &request.actor)?;
    validate::require_id("workspace_handle", &request.workspace_handle)?;
    validate::require_id("idempotency_key", &request.idempotency_key)?;
    let request_digest = hash::digest_value(raw)
        .map_err(|err| CoreError::invalid(format!("apply request: {err}")))?;
    if let Some((recorded, _, result)) = store.idempotency_get(
        &request.project_id,
        "applyChangeSet",
        &request.idempotency_key,
    )? {
        if recorded != request_digest {
            return Err(CoreError::conflict(
                "scope_conflict: idempotency_key was used with a different apply request",
            ));
        }
        return serde_json::from_str(&result)
            .map_err(|err| CoreError::Storage(format!("stored apply result corrupt: {err}")));
    }
    let (revision, payload) = latest_changeset(store, &request.project_id, &request.changeset_id)?
        .ok_or_else(|| {
            CoreError::not_found(format!("changeset {} not found", request.changeset_id))
        })?;
    if request.expected_revision != revision {
        return Err(CoreError::conflict(format!(
            "stale_snapshot: expected changeset revision {} does not match {revision}",
            request.expected_revision
        )));
    }
    if payload["state"] != "prepared" {
        return Err(CoreError::conflict(format!(
            "changeset {} is {}; only a prepared revision can be applied",
            request.changeset_id, payload["state"]
        )));
    }
    if payload["prepared_digest"].as_str() != Some(request.prepared_digest.as_str()) {
        return Err(CoreError::conflict(
            "stale_plan: prepared_digest does not match the current prepared revision",
        ));
    }
    if let Err(error) = ensure_plan_current(store, &payload) {
        mark_stale(store, &payload, &error.to_string())?;
        return Err(error);
    }
    require_prepared_approval(store, &request, &payload)?;
    let root = resolve_workspace_root(store, &request.workspace_handle, &request.project_id)?;
    let ttl = request
        .lease_ttl_secs
        .unwrap_or(300)
        .min(MAX_LEASE_SECONDS)
        .max(1);
    let owner = format!(
        "apply:{}",
        short_hash(&format!("{}:{}", request.actor, request.idempotency_key))
    );
    let lease = acquire_writer_lease(store, &request.project_id, &owner, ttl)?;
    let token = lease["fencing_token"].as_i64().unwrap_or(0);
    if let Some(callback) = hook.as_mut() {
        callback(
            store,
            &ApplyEvent::LeaseAcquired {
                owner: owner.clone(),
                fencing_token: token,
                expires_at: lease["expires_at"].as_i64().unwrap_or(0),
            },
        )?;
    }
    let resolved = match preflight(store, &payload, &root) {
        Ok(resolved) => resolved,
        Err(PreflightError::Stale(reason)) => {
            mark_stale(store, &payload, &reason)?;
            let _ = release_writer_lease(store, &request.project_id, &owner, token);
            return Err(CoreError::conflict(format!("stale_inputs: {reason}")));
        }
        Err(PreflightError::Invalid(reason)) => {
            mark_stale(store, &payload, &format!("preflight_rejected: {reason}"))?;
            let _ = release_writer_lease(store, &request.project_id, &owner, token);
            return Err(CoreError::conflict(format!("unsafe_change: {reason}")));
        }
    };
    if let Some(callback) = hook.as_mut() {
        callback(store, &ApplyEvent::PreflightPassed)?;
    }
    let journal: Vec<Value> = resolved.iter().map(intent_row).collect();
    let mut applying = append_state_with_journal(
        store,
        payload.clone(),
        "applying",
        json!({
            "apply_started_at": crate::store::now_unix(),
            "apply_actor": request.actor,
            "apply_workspace_handle": request.workspace_handle,
            "lease_owner": owner,
            "lease_fencing_token": token,
        }),
        Some(&journal),
    )?;
    let journal_revision = applying["revision"].as_i64().unwrap_or(revision + 1);

    let mut completed: Vec<i64> = Vec::new();
    for resolved_operation in &resolved {
        if let Err(error) = check_lease(store, &request.project_id, &owner, token) {
            journal_phase(
                store,
                &request.project_id,
                &request.changeset_id,
                journal_revision,
                resolved_operation.sequence,
                "conflict",
                None,
                json!({"reason": error.to_string()}),
            )?;
            let mut failed_payload = applying;
            for sequence in &completed {
                set_operation_state(&mut failed_payload, *sequence, "applied");
            }
            append_state(
                store,
                failed_payload,
                "failed",
                json!({"error": error.to_string(), "failed_sequence": resolved_operation.sequence}),
            )?;
            return Err(error);
        }
        if let Some(callback) = hook.as_mut() {
            callback(
                store,
                &ApplyEvent::BeforeReplace {
                    sequence: resolved_operation.sequence,
                    op: resolved_operation.op.clone(),
                    path: resolved_operation.path.clone(),
                },
            )?;
        }
        if let Err(reason) = check_operation(&root, resolved_operation) {
            journal_phase(
                store,
                &request.project_id,
                &request.changeset_id,
                journal_revision,
                resolved_operation.sequence,
                "conflict",
                None,
                json!({"reason": reason}),
            )?;
            let mut conflicted_payload = applying;
            for sequence in &completed {
                set_operation_state(&mut conflicted_payload, *sequence, "applied");
            }
            set_operation_state(
                &mut conflicted_payload,
                resolved_operation.sequence,
                "conflict",
            );
            append_state(
                store,
                conflicted_payload,
                "conflicted",
                json!({
                    "error": reason,
                    "conflicts": [{
                        "sequence": resolved_operation.sequence,
                        "path": resolved_operation.path,
                        "reason": reason,
                    }],
                }),
            )?;
            let _ = release_writer_lease(store, &request.project_id, &owner, token);
            return Err(CoreError::conflict(format!(
                "external_conflict: {reason}; recovery is required"
            )));
        }
        journal_phase(
            store,
            &request.project_id,
            &request.changeset_id,
            journal_revision,
            resolved_operation.sequence,
            "replace_started",
            None,
            json!({}),
        )?;
        match perform_operation(store, resolved_operation) {
            Ok(observed) => {
                journal_phase(
                    store,
                    &request.project_id,
                    &request.changeset_id,
                    journal_revision,
                    resolved_operation.sequence,
                    "replaced",
                    observed.as_deref(),
                    json!({}),
                )?;
                if let Some(callback) = hook.as_mut() {
                    callback(
                        store,
                        &ApplyEvent::AfterReplace {
                            sequence: resolved_operation.sequence,
                            op: resolved_operation.op.clone(),
                            path: resolved_operation.path.clone(),
                            observed_digest: observed,
                        },
                    )?;
                }
                completed.push(resolved_operation.sequence);
            }
            Err(error) => {
                journal_phase(
                    store,
                    &request.project_id,
                    &request.changeset_id,
                    journal_revision,
                    resolved_operation.sequence,
                    "conflict",
                    None,
                    json!({"error": error.to_string()}),
                )?;
                let mut failed_payload = applying;
                for sequence in &completed {
                    set_operation_state(&mut failed_payload, *sequence, "applied");
                }
                set_operation_state(&mut failed_payload, resolved_operation.sequence, "failed");
                append_state(
                    store,
                    failed_payload,
                    "failed",
                    json!({
                        "error": error.to_string(),
                        "failed_sequence": resolved_operation.sequence,
                    }),
                )?;
                let _ = release_writer_lease(store, &request.project_id, &owner, token);
                return Err(error);
            }
        }
    }
    if let Err(reason) = verify_workspace_after(&resolved) {
        let mut conflicted_payload = applying;
        for operation in &resolved {
            set_operation_state(&mut conflicted_payload, operation.sequence, "applied");
        }
        append_state(
            store,
            conflicted_payload,
            "conflicted",
            json!({"error": reason, "conflicts": [{"path": null, "reason": reason}]}),
        )?;
        let _ = release_writer_lease(store, &request.project_id, &owner, token);
        return Err(CoreError::conflict(format!(
            "post_apply_mismatch: {reason}; recovery is required"
        )));
    }
    let captured = match capture_resulting_snapshot(store, &payload, &root) {
        Ok(captured) => captured,
        Err(error) => {
            let failed = append_state(
                store,
                applying,
                "failed",
                json!({"error": format!("resulting snapshot failed: {error}")}),
            )?;
            let _ = failed;
            let _ = release_writer_lease(store, &request.project_id, &owner, token);
            return Err(CoreError::Storage(format!(
                "files were replaced but the resulting snapshot failed: {error}; recovery is required"
            )));
        }
    };
    if let Err(error) = verify_resulting_coverage(store, &captured, &resolved) {
        let failed = append_state(
            store,
            applying,
            "failed",
            json!({"error": error.to_string(), "resulting_snapshot_id": captured}),
        )?;
        let _ = failed;
        let _ = release_writer_lease(store, &request.project_id, &owner, token);
        return Err(error);
    }
    if let Some(callback) = hook.as_mut() {
        callback(
            store,
            &ApplyEvent::SnapshotCaptured {
                snapshot_id: captured.clone(),
            },
        )?;
    }
    let mut applied_payload = applying;
    for operation in &resolved {
        set_operation_state(&mut applied_payload, operation.sequence, "applied");
    }
    let applied = append_state(
        store,
        applied_payload,
        "applied",
        json!({
            "applied_at": crate::store::now_unix(),
            "resulting_snapshot_id": captured,
        }),
    )?;
    let _ = release_writer_lease(store, &request.project_id, &owner, token);
    let result = get_changeset(store, &request.project_id, &request.changeset_id)?;
    let result_json = serde_json::to_string(&result)
        .map_err(|err| CoreError::Storage(format!("apply result encode: {err}")))?;
    store.conn.execute(
        "INSERT INTO idempotency
         (project_id,operation,idempotency_key,request_digest,status_code,result_json,created_at)
         VALUES (?1,'applyChangeSet',?2,?3,200,?4,?5)",
        (
            &request.project_id,
            &request.idempotency_key,
            &request_digest,
            &result_json,
            crate::store::now_unix(),
        ),
    )?;
    let _ = applied;
    Ok(result)
}

pub fn recover_changeset(store: &mut Store, raw: &Value) -> Result<Value> {
    recover_changeset_with_hook(store, raw, None)
}

pub fn recover_changeset_with_hook(
    store: &mut Store,
    raw: &Value,
    mut hook: Option<&mut dyn FnMut(&mut Store, &ApplyEvent) -> Result<()>>,
) -> Result<Value> {
    let request: RecoverRequest = serde_json::from_value(raw.clone())
        .map_err(|err| CoreError::invalid(format!("recover request: {err}")))?;
    validate::require_id("project_id", &request.project_id)?;
    validate::require_id("changeset_id", &request.changeset_id)?;
    validate::require_non_empty("actor", &request.actor)?;
    validate::require_id("workspace_handle", &request.workspace_handle)?;
    validate::require_id("idempotency_key", &request.idempotency_key)?;
    let request_digest = hash::digest_value(raw)
        .map_err(|err| CoreError::invalid(format!("recover request: {err}")))?;
    if let Some((recorded, _, result)) = store.idempotency_get(
        &request.project_id,
        "recoverChangeSet",
        &request.idempotency_key,
    )? {
        if recorded != request_digest {
            return Err(CoreError::conflict(
                "scope_conflict: idempotency_key was used with a different recovery request",
            ));
        }
        return serde_json::from_str(&result)
            .map_err(|err| CoreError::Storage(format!("stored recovery result corrupt: {err}")));
    }
    let (revision, payload) = latest_changeset(store, &request.project_id, &request.changeset_id)?
        .ok_or_else(|| {
            CoreError::not_found(format!("changeset {} not found", request.changeset_id))
        })?;
    if request.expected_revision != revision {
        return Err(CoreError::conflict(format!(
            "stale_snapshot: expected changeset revision {} does not match {revision}",
            request.expected_revision
        )));
    }
    if !matches!(
        payload["state"].as_str().unwrap_or(""),
        "applying" | "failed" | "conflicted" | "applied"
    ) {
        return Err(CoreError::conflict(format!(
            "changeset {} is {}; recovery is not allowed",
            request.changeset_id, payload["state"]
        )));
    }
    let root = resolve_workspace_root(store, &request.workspace_handle, &request.project_id)?;
    let ttl = request
        .lease_ttl_secs
        .unwrap_or(300)
        .min(MAX_LEASE_SECONDS)
        .max(1);
    let owner = format!(
        "recover:{}",
        short_hash(&format!("{}:{}", request.actor, request.idempotency_key))
    );
    let lease = acquire_writer_lease(store, &request.project_id, &owner, ttl)?;
    let token = lease["fencing_token"].as_i64().unwrap_or(0);
    if let Some(callback) = hook.as_mut() {
        callback(
            store,
            &ApplyEvent::LeaseAcquired {
                owner: owner.clone(),
                fencing_token: token,
                expires_at: lease["expires_at"].as_i64().unwrap_or(0),
            },
        )?;
    }
    let journal_revision: Option<i64> = store
        .conn
        .query_row(
            "SELECT MAX(revision) FROM apply_journal
             WHERE project_id=?1 AND changeset_id=?2",
            (&request.project_id, &request.changeset_id),
            |row| row.get::<_, Option<i64>>(0),
        )
        .ok()
        .flatten();
    let rows = match journal_revision {
        Some(journal_revision) => load_journal(
            store,
            &request.project_id,
            &request.changeset_id,
            journal_revision,
        )?,
        None => Vec::new(),
    };
    let mut payload = payload;
    if payload["state"] == "applying" {
        // An interrupted process cannot claim "failed" from the outside. The
        // journal proves the intermediate state; record the failure before
        // entering recovery.
        payload = append_state(
            store,
            payload,
            "failed",
            json!({"error": "apply_interrupted: the apply process ended before completion"}),
        )?;
    }
    let recovering = append_state(
        store,
        payload.clone(),
        "recovering",
        json!({
            "recovery_started_at": crate::store::now_unix(),
            "recovery_actor": request.actor,
            "journal_revision": journal_revision,
        }),
    )?;
    let mut conflicts = Vec::new();
    let mut restored_sequences = std::collections::BTreeSet::new();
    let mut conflict_sequences = std::collections::BTreeSet::new();
    for row in rows.iter().rev() {
        if let Err(error) = check_lease(store, &request.project_id, &owner, token) {
            let failed = append_state(
                store,
                recovering,
                "failed",
                json!({"error": error.to_string(), "recovery_incomplete": true}),
            )?;
            let _ = failed;
            return Err(error);
        }
        if let Some(callback) = hook.as_mut() {
            callback(
                store,
                &ApplyEvent::RecoveryBefore {
                    sequence: row.sequence,
                    path: row.path.clone(),
                },
            )?;
        }
        match restore_row(store, &root, row) {
            Ok(RestoreOutcome::Restored) => {
                restored_sequences.insert(row.sequence);
                if let Some(journal_revision) = journal_revision {
                    journal_phase(
                        store,
                        &request.project_id,
                        &request.changeset_id,
                        journal_revision,
                        row.sequence,
                        "restored",
                        row.preimage_digest.as_deref(),
                        json!({}),
                    )?;
                }
                if let Some(callback) = hook.as_mut() {
                    callback(
                        store,
                        &ApplyEvent::RecoveryAfter {
                            sequence: row.sequence,
                            path: row.path.clone(),
                            conflict: false,
                        },
                    )?;
                }
            }
            Ok(RestoreOutcome::Conflict {
                current_digest,
                user_artifact,
                reason,
            }) => {
                conflict_sequences.insert(row.sequence);
                if let Some(journal_revision) = journal_revision {
                    journal_phase(
                        store,
                        &request.project_id,
                        &request.changeset_id,
                        journal_revision,
                        row.sequence,
                        "conflict",
                        current_digest.as_deref(),
                        json!({
                            "reason": reason,
                            "user_version_artifact": user_artifact,
                        }),
                    )?;
                }
                conflicts.push(json!({
                    "sequence": row.sequence,
                    "op": row.op,
                    "path": row.path,
                    "destination": row.destination,
                    "reason": reason,
                    "current_digest": current_digest,
                    "user_version_artifact": user_artifact,
                }));
                if let Some(callback) = hook.as_mut() {
                    callback(
                        store,
                        &ApplyEvent::RecoveryAfter {
                            sequence: row.sequence,
                            path: row.path.clone(),
                            conflict: true,
                        },
                    )?;
                }
            }
            Err(error) => {
                let failed = append_state(
                    store,
                    recovering,
                    "failed",
                    json!({"error": error.to_string(), "recovery_incomplete": true}),
                )?;
                let _ = failed;
                let _ = release_writer_lease(store, &request.project_id, &owner, token);
                return Err(error);
            }
        }
    }
    let mut final_payload = recovering;
    for row in &rows {
        let state = if conflict_sequences.contains(&row.sequence) {
            "conflict"
        } else if restored_sequences.contains(&row.sequence) {
            "restored"
        } else {
            "pending"
        };
        set_operation_state(&mut final_payload, row.sequence, state);
    }
    if conflicts.is_empty() {
        let snapshot = capture_resulting_snapshot(store, &payload, &root).ok();
        append_state(
            store,
            final_payload,
            "recovered",
            json!({
                "recovered_at": crate::store::now_unix(),
                "conflicts": [],
                "resulting_snapshot_id": snapshot,
            }),
        )?;
    } else {
        append_state(
            store,
            final_payload,
            "conflicted",
            json!({
                "conflicts": conflicts,
                "recovery_conflicted_at": crate::store::now_unix(),
            }),
        )?;
    }
    let _ = release_writer_lease(store, &request.project_id, &owner, token);
    let result = get_changeset(store, &request.project_id, &request.changeset_id)?;
    let result_json = serde_json::to_string(&result)
        .map_err(|err| CoreError::Storage(format!("recovery result encode: {err}")))?;
    store.conn.execute(
        "INSERT INTO idempotency
         (project_id,operation,idempotency_key,request_digest,status_code,result_json,created_at)
         VALUES (?1,'recoverChangeSet',?2,?3,200,?4,?5)",
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

/// Explicit intermediate-state report for doctor/restart. A ChangeSet state
/// alone never proves that files are consistent.
pub fn interrupted_changesets(store: &Store, project_id: &str) -> Result<Vec<Value>> {
    validate::require_id("project_id", project_id)?;
    let mut statement = store.conn.prepare(
        "SELECT changeset_id, MAX(revision) FROM changesets
         WHERE project_id=?1 GROUP BY changeset_id",
    )?;
    let rows = statement.query_map((project_id,), |row| {
        Ok((row.get::<_, String>(0)?, row.get::<_, i64>(1)?))
    })?;
    let mut interrupted = Vec::new();
    for row in rows {
        let (changeset_id, revision) = row?;
        let (_, payload) = match latest_changeset(store, project_id, &changeset_id)? {
            Some(latest) => latest,
            None => continue,
        };
        let state = payload["state"].as_str().unwrap_or("");
        let journal_revision: Option<i64> = store
            .conn
            .query_row(
                "SELECT MAX(revision) FROM apply_journal
                 WHERE project_id=?1 AND changeset_id=?2",
                (project_id, &changeset_id),
                |row| row.get::<_, Option<i64>>(0),
            )
            .ok()
            .flatten();
        if !matches!(state, "applying" | "recovering" | "failed" | "conflicted") {
            continue;
        }
        let mut pending = Vec::new();
        if let Some(journal_revision) = journal_revision {
            let mut statement = store.conn.prepare(
                "SELECT sequence, phase FROM apply_journal
                 WHERE project_id=?1 AND changeset_id=?2 AND revision=?3
                   AND phase NOT IN ('restored') ORDER BY sequence",
            )?;
            let journal = statement
                .query_map((project_id, &changeset_id, journal_revision), |row| {
                    Ok((row.get::<_, i64>(0)?, row.get::<_, String>(1)?))
                })?;
            for item in journal {
                let (sequence, phase) = item?;
                pending.push(json!({"sequence": sequence, "phase": phase}));
            }
        }
        // A preparation failure has no journal; it is failed, not interrupted.
        // Only an apply that started and still has unrecovered journal work is
        // reported for recovery.
        if !matches!(state, "applying" | "recovering")
            && (journal_revision.is_none() || pending.is_empty())
        {
            continue;
        }
        interrupted.push(json!({
            "changeset_id": changeset_id,
            "revision": revision,
            "state": state,
            "journal_revision": journal_revision,
            "pending_journal": pending,
            "resulting_snapshot_id": payload["resulting_snapshot_id"],
        }));
    }
    Ok(interrupted)
}

fn preflight(
    store: &Store,
    payload: &Value,
    root: &Path,
) -> std::result::Result<Vec<ResolvedOperation>, PreflightError> {
    let project_id = payload["project_id"].as_str().unwrap_or("");
    let base_snapshot_id = payload["base_snapshot_id"].as_str().unwrap_or("");
    for path in payload["read_set"].as_array().into_iter().flatten() {
        let Some(path) = path.as_str() else {
            return Err(PreflightError::Invalid(
                "read_set contains a non-string".into(),
            ));
        };
        let expected: Option<String> = store
            .conn
            .query_row(
                "SELECT blob_digest FROM files WHERE project_id=?1 AND snapshot_id=?2 AND path=?3",
                (project_id, base_snapshot_id, path),
                |row| row.get(0),
            )
            .ok();
        let Some(expected) = expected else {
            return Err(PreflightError::Invalid(format!(
                "read_set path {path} is not in the base snapshot"
            )));
        };
        let target =
            safe_resolve(root, path).map_err(|err| PreflightError::Invalid(err.to_string()))?;
        match read_digest(&target) {
            Ok(digest) if digest == expected => {}
            Ok(_) => {
                return Err(PreflightError::Stale(format!(
                    "read_set file {path} changed since preparation"
                )))
            }
            Err(_) => {
                return Err(PreflightError::Stale(format!(
                    "read_set file {path} is missing or unreadable"
                )))
            }
        }
    }
    let mut resolved = Vec::new();
    for (sequence, operation) in payload["operations"]
        .as_array()
        .into_iter()
        .flatten()
        .enumerate()
    {
        let op = operation["op"].as_str().unwrap_or("").to_string();
        let path = operation["path"].as_str().unwrap_or("").to_string();
        if !matches!(op.as_str(), "create" | "update" | "delete" | "move") {
            return Err(PreflightError::Invalid(format!("unknown operation {op}")));
        }
        let destination = operation["destination"].as_str().map(str::to_string);
        let before_digest = operation["before"]["digest"].as_str().map(str::to_string);
        let after_digest = operation["after"]["digest"].as_str().map(str::to_string);
        if let Some(digest) = &after_digest {
            if store
                .blob_info(digest)
                .map_err(|err| PreflightError::Invalid(err.to_string()))?
                .is_none()
            {
                return Err(PreflightError::Invalid(format!(
                    "postimage {digest} is missing from CAS"
                )));
            }
        }
        let source =
            safe_resolve(root, &path).map_err(|err| PreflightError::Invalid(err.to_string()))?;
        let target = match &destination {
            Some(destination) if op == "move" => safe_resolve(root, destination)
                .map_err(|err| PreflightError::Invalid(err.to_string()))?,
            Some(_) => {
                return Err(PreflightError::Invalid(
                    "only move operations may have a destination".into(),
                ))
            }
            None => source.clone(),
        };
        let case_only = operation["case_only"].as_bool().unwrap_or(false);
        let changeset_id = payload["id"].as_str().unwrap_or("");
        let temp = deterministic_temp_path(&target, changeset_id, sequence as i64)
            .map_err(|err| PreflightError::Invalid(err.to_string()))?;
        let resolved_operation = ResolvedOperation {
            sequence: sequence as i64,
            op,
            path,
            destination,
            before_digest,
            after_digest,
            case_only,
            executable: operation["before"]["executable"].as_bool().unwrap_or(false),
            source,
            target,
            temp,
        };
        check_operation(root, &resolved_operation).map_err(PreflightError::Stale)?;
        resolved.push(resolved_operation);
    }
    Ok(resolved)
}

fn check_operation(root: &Path, operation: &ResolvedOperation) -> std::result::Result<(), String> {
    let _ = root;
    match operation.op.as_str() {
        "create" => {
            if operation.target.symlink_metadata().is_ok() {
                return Err(format!("create target {} already exists", operation.path));
            }
            require_parent_dir(&operation.target)?;
        }
        "update" | "delete" => {
            let metadata = operation
                .source
                .symlink_metadata()
                .map_err(|_| format!("{} is missing", operation.path))?;
            if !metadata.file_type().is_file() {
                return Err(format!("{} is not a regular file", operation.path));
            }
            let digest = read_digest(&operation.source)
                .map_err(|err| format!("{} is unreadable: {err}", operation.path))?;
            if operation.before_digest.as_deref() != Some(digest.as_str()) {
                return Err(format!(
                    "{} changed since preparation (expected {})",
                    operation.path,
                    operation.before_digest.as_deref().unwrap_or("no preimage")
                ));
            }
        }
        "move" => {
            let metadata = operation
                .source
                .symlink_metadata()
                .map_err(|_| format!("{} is missing", operation.path))?;
            if !metadata.file_type().is_file() {
                return Err(format!("{} is not a regular file", operation.path));
            }
            let digest = read_digest(&operation.source)
                .map_err(|err| format!("{} is unreadable: {err}", operation.path))?;
            if operation.before_digest.as_deref() != Some(digest.as_str()) {
                return Err(format!("{} changed since preparation", operation.path));
            }
            require_parent_dir(&operation.target)?;
            if operation.target.symlink_metadata().is_ok() {
                let same = same_file(&operation.source, &operation.target);
                let case_only = operation.case_only
                    && operation
                        .destination
                        .as_deref()
                        .map(|destination| {
                            destination.to_lowercase() == operation.path.to_lowercase()
                        })
                        .unwrap_or(false);
                if !(same && case_only) {
                    return Err(format!(
                        "move destination {} already exists",
                        operation.destination.as_deref().unwrap_or("")
                    ));
                }
            }
        }
        _ => return Err("unknown operation".to_string()),
    }
    Ok(())
}

fn perform_operation(store: &Store, operation: &ResolvedOperation) -> Result<Option<String>> {
    match operation.op.as_str() {
        "create" => {
            let bytes = postimage(store, operation)?;
            write_new_file(
                &operation.target,
                &operation.temp,
                &bytes,
                operation.executable,
            )?;
        }
        "update" => {
            let bytes = postimage(store, operation)?;
            write_replace_file(
                &operation.target,
                &operation.temp,
                &bytes,
                operation.executable,
            )?;
        }
        "delete" => {
            fs::remove_file(&operation.source)?;
        }
        "move" => {
            if operation.case_only {
                cleanup_temp(&operation.temp);
                fs::rename(&operation.source, &operation.temp)?;
                fs::rename(&operation.temp, &operation.target)?;
            } else {
                fs::rename(&operation.source, &operation.target)?;
            }
        }
        _ => return Err(CoreError::Storage("unknown operation".to_string())),
    }
    match operation.op.as_str() {
        "delete" => Ok(None),
        _ => {
            let digest = read_digest(&operation.target)?;
            if operation.after_digest.as_deref() != Some(digest.as_str()) {
                return Err(CoreError::Storage(format!(
                    "post-replacement digest mismatch for {}",
                    operation.path
                )));
            }
            Ok(Some(digest))
        }
    }
}

fn postimage(store: &Store, operation: &ResolvedOperation) -> Result<Vec<u8>> {
    let digest = operation
        .after_digest
        .as_deref()
        .ok_or_else(|| CoreError::Storage("operation has no postimage".to_string()))?;
    let bytes = store
        .read_blob(digest)?
        .ok_or_else(|| CoreError::Storage(format!("postimage {digest} is missing")))?;
    if hash::digest_bytes(&bytes) != digest {
        return Err(CoreError::Storage(format!(
            "postimage {digest} content does not match its digest"
        )));
    }
    Ok(bytes)
}

fn verify_workspace_after(resolved: &[ResolvedOperation]) -> std::result::Result<(), String> {
    for operation in resolved {
        if operation.op == "delete" {
            if operation.source.symlink_metadata().is_ok() {
                return Err(format!("{} still exists after delete", operation.path));
            }
            continue;
        }
        let digest = read_digest(&operation.target)
            .map_err(|err| format!("{} is unreadable after apply: {err}", operation.path))?;
        if operation.after_digest.as_deref() != Some(digest.as_str()) {
            return Err(format!(
                "{} does not match its postimage after apply",
                operation.path
            ));
        }
    }
    Ok(())
}

fn capture_resulting_snapshot(store: &mut Store, payload: &Value, root: &Path) -> Result<String> {
    let options: SnapshotCaptureOptions =
        serde_json::from_value(payload["snapshot_options"].clone())
            .map_err(|err| CoreError::Storage(format!("snapshot options corrupt: {err}")))?;
    let inputs = SnapshotInputs {
        build_config: payload["build_config"].clone(),
        environment: payload["environment"].clone(),
    };
    let captured = capture_snapshot(
        store,
        payload["project_id"].as_str().unwrap_or(""),
        root,
        &inputs,
        &options.to_options(),
    )?;
    Ok(captured.snapshot_id)
}

fn verify_resulting_coverage(
    store: &Store,
    snapshot_id: &str,
    resolved: &[ResolvedOperation],
) -> Result<()> {
    for operation in resolved {
        if operation.op == "delete" {
            let exists: Option<String> = store
                .conn
                .query_row(
                    "SELECT path FROM files WHERE snapshot_id=?1 AND path=?2",
                    (snapshot_id, &operation.path),
                    |row| row.get(0),
                )
                .ok();
            if exists.is_some() {
                return Err(CoreError::Storage(format!(
                    "resulting snapshot still lists deleted path {}",
                    operation.path
                )));
            }
            continue;
        }
        let recorded: Option<String> = store
            .conn
            .query_row(
                "SELECT blob_digest FROM files WHERE snapshot_id=?1 AND path=?2",
                (
                    snapshot_id,
                    operation.destination.as_deref().unwrap_or(&operation.path),
                ),
                |row| row.get(0),
            )
            .ok();
        let expected = operation
            .after_digest
            .as_deref()
            .ok_or_else(|| CoreError::Storage("operation has no postimage".into()))?;
        if recorded.as_deref() != Some(expected) {
            return Err(CoreError::conflict(format!(
                "resulting_snapshot_incomplete: {} is not captured with its postimage; check snapshot_options",
                operation.destination.as_deref().unwrap_or(&operation.path)
            )));
        }
    }
    Ok(())
}

fn require_prepared_approval(store: &Store, request: &ApplyRequest, payload: &Value) -> Result<()> {
    let now = crate::store::now_unix();
    let row: Option<(String, String, i64, String, String)> = store
        .conn
        .query_row(
            "SELECT target_kind, target_id, target_revision, target_digest, decision
             FROM approvals
             WHERE project_id=?1 AND approval_id=?2
               AND (expires_at IS NULL OR expires_at > ?3)",
            (&request.project_id, &request.approval_id, now),
            |row| {
                Ok((
                    row.get(0)?,
                    row.get(1)?,
                    row.get(2)?,
                    row.get(3)?,
                    row.get(4)?,
                ))
            },
        )
        .ok();
    let Some((target_kind, target_id, target_revision, target_digest, decision)) = row else {
        return Err(CoreError::conflict(
            "missing_prepared_approval: no unexpired approval matches this approval_id",
        ));
    };
    if target_kind != "prepared_change"
        || target_id != request.changeset_id
        || target_revision != request.expected_revision
        || target_digest != request.prepared_digest
        || decision != "approve"
    {
        return Err(CoreError::conflict(
            "missing_prepared_approval: the approval does not bind this exact prepared_digest",
        ));
    }
    if payload["prepared_digest"].as_str() != Some(target_digest.as_str()) {
        return Err(CoreError::conflict(
            "missing_prepared_approval: prepared content changed after approval",
        ));
    }
    Ok(())
}

fn check_lease(store: &Store, project_id: &str, owner: &str, token: i64) -> Result<()> {
    let row: Option<(String, i64, i64, Option<i64>)> = store
        .conn
        .query_row(
            "SELECT owner, fencing_token, expires_at, released_at FROM writer_leases
             WHERE project_id=?1",
            (project_id,),
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?)),
        )
        .ok();
    let now = crate::store::now_unix();
    match row {
        Some((held_by, held_token, expires_at, released_at))
            if held_by == owner
                && held_token == token
                && released_at.is_none()
                && expires_at > now =>
        {
            Ok(())
        }
        _ => Err(CoreError::conflict(
            "lease_lost: the fencing token is no longer valid; no write was performed",
        )),
    }
}

fn set_operation_state(payload: &mut Value, sequence: i64, state: &str) {
    if let Some(operations) = payload["operations"].as_array_mut() {
        if let Some(operation) = operations.get_mut(sequence as usize) {
            operation["state"] = json!(state);
        }
    }
}

fn mark_stale(store: &mut Store, payload: &Value, reason: &str) -> Result<()> {
    if payload["state"] == "prepared" {
        append_state(
            store,
            payload.clone(),
            "stale",
            json!({"stale_reason": reason}),
        )?;
    }
    Ok(())
}

pub(crate) fn resolve_workspace_root(store: &Store, handle: &str, project_id: &str) -> Result<PathBuf> {
    let root: Option<String> = store
        .conn
        .query_row(
            "SELECT root_path FROM workspace_handles WHERE handle=?1 AND project_id=?2",
            (handle, project_id),
            |row| row.get(0),
        )
        .ok();
    let Some(root) = root else {
        return Err(CoreError::unsupported(
            "permission_denied: workspace_handle is not registered for this project",
        ));
    };
    let canonical = fs::canonicalize(&root).map_err(|err| {
        CoreError::not_found(format!("workspace root {root} is unreadable: {err}"))
    })?;
    if !canonical.is_dir() {
        return Err(CoreError::invalid("workspace root is not a directory"));
    }
    Ok(canonical)
}

/// Resolve a snapshot-relative path under the canonical workspace root. Every
/// existing ancestor is rejected if it is a symlink, and the canonical parent
/// must still be inside the root, which also catches Windows junctions.
pub(crate) fn safe_resolve(root: &Path, relative: &str) -> Result<PathBuf> {
    validate::validate_path("path", relative)?;
    let canonical_root = fs::canonicalize(root)?;
    let target = canonical_root.join(relative);
    let mut current = canonical_root.clone();
    for component in Path::new(relative).components() {
        let Component::Normal(name) = component else {
            return Err(CoreError::invalid(format!(
                "path {relative} is not a plain relative path"
            )));
        };
        current.push(name);
        match fs::symlink_metadata(&current) {
            Ok(metadata) if is_link_like(&metadata) => {
                return Err(CoreError::invalid(format!(
                    "path {relative} traverses a symlink or reparse point"
                )))
            }
            Ok(_) => {}
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => break,
            Err(err) => return Err(CoreError::from(err)),
        }
    }
    let parent = target
        .parent()
        .ok_or_else(|| CoreError::invalid("path has no parent"))?;
    let canonical_parent = fs::canonicalize(parent).map_err(|err| {
        CoreError::invalid(format!(
            "parent of {relative} is missing or unreadable: {err}"
        ))
    })?;
    if !canonical_parent.starts_with(&canonical_root) {
        return Err(CoreError::invalid(format!(
            "path {relative} escapes the workspace root"
        )));
    }
    Ok(target)
}

fn is_link_like(metadata: &fs::Metadata) -> bool {
    #[cfg(windows)]
    {
        use std::os::windows::fs::MetadataExt;
        const FILE_ATTRIBUTE_REPARSE_POINT: u32 = 0x0400;
        metadata.file_attributes() & FILE_ATTRIBUTE_REPARSE_POINT != 0
    }
    #[cfg(not(windows))]
    {
        metadata.file_type().is_symlink()
    }
}

fn require_parent_dir(target: &Path) -> std::result::Result<(), String> {
    let parent = target
        .parent()
        .ok_or_else(|| "target has no parent directory".to_string())?;
    match fs::metadata(parent) {
        Ok(metadata) if metadata.is_dir() => Ok(()),
        _ => Err("target parent is not an existing directory".to_string()),
    }
}

fn same_file(left: &Path, right: &Path) -> bool {
    match (fs::canonicalize(left), fs::canonicalize(right)) {
        (Ok(left), Ok(right)) => left == right,
        _ => false,
    }
}

fn read_digest(path: &Path) -> Result<String> {
    let bytes = fs::read(path)?;
    Ok(hash::digest_bytes(&bytes))
}

fn postimage_bytes(store: &Store, digest: &str) -> Result<Vec<u8>> {
    let bytes = store
        .read_blob(digest)?
        .ok_or_else(|| CoreError::Storage(format!("preimage {digest} is missing")))?;
    if hash::digest_bytes(&bytes) != digest {
        return Err(CoreError::Storage(format!(
            "preimage {digest} content does not match its digest"
        )));
    }
    Ok(bytes)
}

fn write_new_file(target: &Path, temp: &Path, bytes: &[u8], executable: bool) -> Result<()> {
    write_temp(target, temp, bytes, executable)?;
    match fs::hard_link(temp, target) {
        Ok(()) => {
            cleanup_temp(temp);
            Ok(())
        }
        Err(err) => {
            cleanup_temp(temp);
            Err(CoreError::Storage(format!(
                "create {} failed because the target appeared or sharing was denied: {err}",
                target.display()
            )))
        }
    }
}

fn write_replace_file(target: &Path, temp: &Path, bytes: &[u8], executable: bool) -> Result<()> {
    write_temp(target, temp, bytes, executable)?;
    if let Err(err) = fs::rename(temp, target) {
        cleanup_temp(temp);
        return Err(CoreError::Storage(format!(
            "replace {} failed: {err}",
            target.display()
        )));
    }
    Ok(())
}

fn write_temp(target: &Path, temp: &Path, bytes: &[u8], executable: bool) -> Result<()> {
    let parent = target
        .parent()
        .ok_or_else(|| CoreError::invalid("target has no parent directory"))?;
    cleanup_temp(temp);
    let mut file = fs::OpenOptions::new()
        .create_new(true)
        .write(true)
        .open(temp)?;
    file.write_all(bytes)?;
    file.sync_all()?;
    drop(file);
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mode = if executable { 0o755 } else { 0o644 };
        fs::set_permissions(temp, fs::Permissions::from_mode(mode))?;
    }
    #[cfg(not(unix))]
    let _ = (parent, executable);
    Ok(())
}

fn deterministic_temp_path(
    target: &Path,
    changeset_id: &str,
    sequence: i64,
) -> std::result::Result<PathBuf, String> {
    let parent = target
        .parent()
        .ok_or_else(|| "target has no parent directory".to_string())?;
    let key = format!("{changeset_id}:{sequence}");
    Ok(parent.join(format!("{TEMP_PREFIX}{}.tmp", short_hash(&key))))
}

fn cleanup_temp(path: &Path) {
    let _ = fs::remove_file(path);
}

fn restore_temp(root: &Path, row: &JournalRow, target: &Path) -> Result<PathBuf> {
    if let Some(text) = row.detail["temp"].as_str() {
        let path = PathBuf::from(text);
        if path.starts_with(fs::canonicalize(root)?) {
            return Ok(path);
        }
    }
    deterministic_temp_path(target, "recovery", row.sequence).map_err(CoreError::invalid)
}

fn intent_row(operation: &ResolvedOperation) -> Value {
    json!({
        "sequence": operation.sequence,
        "op": operation.op,
        "path": operation.path,
        "destination": operation.destination,
        "preimage_digest": operation.before_digest,
        "postimage_digest": operation.after_digest,
        "phase": "intent",
        "observed_digest": null,
        "detail": {
            "case_only": operation.case_only,
            "executable": operation.executable,
            "temp": operation.temp.to_string_lossy(),
        },
    })
}

fn journal_phase(
    store: &mut Store,
    project_id: &str,
    changeset_id: &str,
    revision: i64,
    sequence: i64,
    phase: &str,
    observed_digest: Option<&str>,
    detail: Value,
) -> Result<()> {
    store.conn.execute(
        "UPDATE apply_journal
         SET phase=?1, observed_digest=?2, detail_json=?3, recorded_at=?4
         WHERE project_id=?5 AND changeset_id=?6 AND revision=?7 AND sequence=?8",
        (
            phase,
            observed_digest,
            hash::canonical_json(&detail)
                .map_err(|err| CoreError::invalid(format!("journal detail: {err}")))?,
            crate::store::now_unix(),
            project_id,
            changeset_id,
            revision,
            sequence,
        ),
    )?;
    Ok(())
}

fn load_journal(
    store: &Store,
    project_id: &str,
    changeset_id: &str,
    revision: i64,
) -> Result<Vec<JournalRow>> {
    let mut statement = store.conn.prepare(
        "SELECT sequence, op, path, destination, preimage_digest, postimage_digest,
                detail_json
         FROM apply_journal
         WHERE project_id=?1 AND changeset_id=?2 AND revision=?3
         ORDER BY sequence",
    )?;
    let rows = statement.query_map((project_id, changeset_id, revision), |row| {
        Ok((
            row.get::<_, i64>(0)?,
            row.get::<_, String>(1)?,
            row.get::<_, String>(2)?,
            row.get::<_, Option<String>>(3)?,
            row.get::<_, Option<String>>(4)?,
            row.get::<_, Option<String>>(5)?,
            row.get::<_, String>(6)?,
        ))
    })?;
    let mut result = Vec::new();
    for row in rows {
        let (sequence, op, path, destination, preimage, postimage, detail) = row?;
        let detail: Value = serde_json::from_str(&detail)
            .map_err(|err| CoreError::Storage(format!("journal detail corrupt: {err}")))?;
        result.push(JournalRow {
            sequence,
            op,
            path,
            destination,
            preimage_digest: preimage,
            postimage_digest: postimage,
            detail,
        });
    }
    Ok(result)
}

enum RestoreOutcome {
    Restored,
    Conflict {
        current_digest: Option<String>,
        user_artifact: Option<String>,
        reason: String,
    },
}

fn restore_row(store: &mut Store, root: &Path, row: &JournalRow) -> Result<RestoreOutcome> {
    let source = safe_resolve(root, &row.path)?;
    let canonical_root = fs::canonicalize(root)?;
    if let Some(temp) = row.detail["temp"].as_str() {
        let temp = PathBuf::from(temp);
        if temp.starts_with(&canonical_root) {
            cleanup_temp(&temp);
        }
    }
    let executable = row.detail["executable"].as_bool().unwrap_or(false);
    match row.op.as_str() {
        "update" => {
            let current = match read_digest(&source) {
                Ok(digest) => digest,
                Err(_) => {
                    return Ok(RestoreOutcome::Conflict {
                        current_digest: None,
                        user_artifact: None,
                        reason: "file is missing after apply".to_string(),
                    })
                }
            };
            if Some(current.as_str()) == row.preimage_digest.as_deref() {
                return Ok(RestoreOutcome::Restored);
            }
            if Some(current.as_str()) != row.postimage_digest.as_deref() {
                return Ok(conflict_version(
                    store,
                    &source,
                    &current,
                    "file changed after apply",
                ));
            }
            let preimage = require_preimage(store, row)?;
            let temp = restore_temp(root, row, &source)?;
            write_replace_file(&source, &temp, &preimage, executable)?;
            Ok(RestoreOutcome::Restored)
        }
        "create" => {
            let current = match read_digest(&source) {
                Ok(digest) => digest,
                Err(_) => return Ok(RestoreOutcome::Restored),
            };
            if Some(current.as_str()) != row.postimage_digest.as_deref() {
                return Ok(conflict_version(
                    store,
                    &source,
                    &current,
                    "new file was modified after apply; it is preserved",
                ));
            }
            fs::remove_file(&source)?;
            Ok(RestoreOutcome::Restored)
        }
        "delete" => {
            if let Ok(current) = read_digest(&source) {
                if Some(current.as_str()) == row.preimage_digest.as_deref() {
                    return Ok(RestoreOutcome::Restored);
                }
                return Ok(conflict_version(
                    store,
                    &source,
                    &current,
                    "deleted path was recreated with different content",
                ));
            }
            let preimage = require_preimage(store, row)?;
            let temp = restore_temp(root, row, &source)?;
            write_new_file(&source, &temp, &preimage, executable)?;
            Ok(RestoreOutcome::Restored)
        }
        "move" => {
            let destination_text = row.destination.as_deref().ok_or_else(|| {
                CoreError::Storage("move journal row has no destination".to_string())
            })?;
            let destination = safe_resolve(root, destination_text)?;
            let source_digest = read_digest(&source).ok();
            let destination_digest = read_digest(&destination).ok();
            let same = source.symlink_metadata().is_ok()
                && destination.symlink_metadata().is_ok()
                && same_file(&source, &destination);
            if same {
                // Case-only move on a case-insensitive filesystem: one file
                // currently reports the destination spelling.
                if destination_digest.as_deref() == row.postimage_digest.as_deref() {
                    let temp = restore_temp(root, row, &destination)?;
                    cleanup_temp(&temp);
                    fs::rename(&destination, &temp)?;
                    fs::rename(&temp, &source)?;
                }
                return Ok(RestoreOutcome::Restored);
            }
            if destination_digest.is_some() {
                if destination_digest.as_deref() == row.postimage_digest.as_deref()
                    && source_digest.is_none()
                {
                    fs::rename(&destination, &source)?;
                    return Ok(RestoreOutcome::Restored);
                }
                let current = destination_digest.as_deref().map(str::to_string);
                return Ok(RestoreOutcome::Conflict {
                    current_digest: current,
                    user_artifact: None,
                    reason: "move destination still exists with unexpected content".to_string(),
                });
            }
            match source_digest {
                Some(digest) if Some(digest.as_str()) == row.preimage_digest.as_deref() => {
                    Ok(RestoreOutcome::Restored)
                }
                Some(digest) => Ok(conflict_version(
                    store,
                    &source,
                    &digest,
                    "move source has unexpected content",
                )),
                None => Ok(RestoreOutcome::Conflict {
                    current_digest: None,
                    user_artifact: None,
                    reason: "neither move source nor destination exists".to_string(),
                }),
            }
        }
        other => Err(CoreError::Storage(format!(
            "unknown journal operation {other}"
        ))),
    }
}

fn conflict_version(store: &mut Store, path: &Path, digest: &str, reason: &str) -> RestoreOutcome {
    let artifact = fs::read(path)
        .ok()
        .and_then(|bytes| store.put_blob(&bytes, "application/octet-stream").ok());
    RestoreOutcome::Conflict {
        current_digest: Some(digest.to_string()),
        user_artifact: artifact,
        reason: reason.to_string(),
    }
}

fn require_preimage(store: &Store, row: &JournalRow) -> Result<Vec<u8>> {
    let digest = row
        .preimage_digest
        .as_deref()
        .ok_or_else(|| CoreError::Storage("journal row has no preimage".to_string()))?;
    postimage_bytes(store, digest)
}

fn short_hash(identity: &str) -> String {
    let digest = hash::digest_bytes(identity.as_bytes());
    let hex = digest.strip_prefix("sha256:").unwrap_or(&digest);
    hex[..16].to_string()
}
