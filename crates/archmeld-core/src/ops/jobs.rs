// Copyright 2026 ARCHMELD contributors
// SPDX-License-Identifier: Apache-2.0

//! Job records and the guarded job state machine.
//!
//! Long operations return a Job instead of holding an HTTP connection open. A
//! job transition and its `job.state_changed` event are written in the same
//! transaction; cancel is idempotent per `idempotency_key`.

use crate::error::{CoreError, Result};
use crate::events;
use crate::store::{validate, Store};
use serde_json::{json, Value};

const JOB_STATES: &[&str] = &[
    "queued",
    "running",
    "succeeded",
    "failed",
    "cancelled",
    "timed_out",
    "blocked",
];

pub fn create_job(store: &mut Store, project_id: &str, operation: &str) -> Result<Value> {
    validate::require_id("project_id", project_id)?;
    if operation.is_empty() {
        return Err(CoreError::invalid("job operation must not be empty"));
    }
    store.ensure_project_exists(project_id)?;
    let job_id = format!(
        "job.{}.{}",
        crate::store::now_unix(),
        next_job_counter(store, project_id)?
    );
    let now = crate::store::now_unix();
    store.conn.execute(
        "INSERT INTO jobs
         (project_id, job_id, operation, state, progress, result_ids_json, artifact_digests_json,
          error_json, revision, created_at, updated_at)
         VALUES (?1,?2,?3,'queued',0,'[]','[]',NULL,1,?4,?4)",
        (project_id, &job_id, operation, now),
    )?;
    get_job(store, project_id, &job_id)?
        .ok_or_else(|| CoreError::Storage("created job cannot be read back".to_string()))
}

pub fn get_job(store: &Store, project_id: &str, job_id: &str) -> Result<Option<Value>> {
    let row = store
        .conn
        .query_row(
            "SELECT job_id, operation, state, progress, result_ids_json, artifact_digests_json,
                    error_json, revision
             FROM jobs WHERE project_id=?1 AND job_id=?2",
            (project_id, job_id),
            |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, String>(2)?,
                    row.get::<_, f64>(3)?,
                    row.get::<_, String>(4)?,
                    row.get::<_, String>(5)?,
                    row.get::<_, Option<String>>(6)?,
                    row.get::<_, i64>(7)?,
                ))
            },
        )
        .ok();
    Ok(row.map(|row| job_json(project_id, row)))
}

/// Transition a job with the state-machine guard. Used by tools/workers; HTTP
/// exposes cancel, which goes through the idempotent wrapper below.
pub fn transition_job(
    store: &mut Store,
    project_id: &str,
    job_id: &str,
    to_state: &str,
    progress: f64,
) -> Result<Value> {
    if !JOB_STATES.contains(&to_state) {
        return Err(CoreError::invalid(format!("unknown job state {to_state}")));
    }
    if !(0.0..=1.0).contains(&progress) {
        return Err(CoreError::invalid("job progress must be 0..=1"));
    }
    let tx = store.conn.transaction()?;
    let (current, revision): (String, i64) = tx
        .query_row(
            "SELECT state, revision FROM jobs WHERE project_id=?1 AND job_id=?2",
            (project_id, job_id),
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .map_err(|_| CoreError::not_found(format!("job {job_id} not found")))?;
    guard_transition(&current, to_state)?;
    let new_revision = revision + 1;
    tx.execute(
        "UPDATE jobs SET state=?3, progress=?4, revision=?5, updated_at=?6
         WHERE project_id=?1 AND job_id=?2",
        (project_id, job_id, to_state, progress, new_revision, crate::store::now_unix()),
    )?;
    events::append_event(
        &tx,
        project_id,
        Some(job_id),
        None,
        "job.state_changed",
        json!({
            "job_id": job_id,
            "previous_state": current,
            "state": to_state,
            "revision": new_revision,
        }),
    )?;
    tx.commit()?;
    get_job(store, project_id, job_id)?
        .ok_or_else(|| CoreError::Storage("transitioned job cannot be read back".to_string()))
}

/// AT073: same key + same request digest replays the stored result; same key
/// with a different digest conflicts; a stale expected_revision conflicts.
pub fn cancel_job_idempotent(
    store: &mut Store,
    project_id: &str,
    job_id: &str,
    expected_revision: i64,
    idempotency_key: &str,
    request_digest: &str,
) -> Result<(u16, Value)> {
    validate::require_id("project_id", project_id)?;
    if idempotency_key.is_empty() {
        return Err(CoreError::invalid("idempotency_key must not be empty"));
    }
    let tx = store.conn.transaction()?;
    let existing: Option<(String, i64, String)> = tx
        .query_row(
            "SELECT request_digest, status_code, result_json FROM idempotency
             WHERE project_id=?1 AND operation='cancelJob' AND idempotency_key=?2",
            (project_id, idempotency_key),
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
        )
        .ok();
    if let Some((digest, status, result)) = existing {
        if digest != request_digest {
            return Err(CoreError::conflict(
                "scope_conflict: idempotency_key was used with a different request body",
            ));
        }
        let result: Value = serde_json::from_str(&result)
            .map_err(|err| CoreError::Storage(format!("stored idempotent result corrupt: {err}")))?;
        tx.commit()?;
        return Ok((status.max(0) as u16, result));
    }

    let (state, revision): (String, i64) = tx
        .query_row(
            "SELECT state, revision FROM jobs WHERE project_id=?1 AND job_id=?2",
            (project_id, job_id),
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .map_err(|_| CoreError::not_found(format!("job {job_id} not found")))?;
    if revision != expected_revision {
        return Err(CoreError::conflict(format!(
            "stale_snapshot: expected_revision {expected_revision} does not match job revision {revision}"
        )));
    }
    guard_transition(&state, "cancelled")?;
    let new_revision = revision + 1;
    tx.execute(
        "UPDATE jobs SET state='cancelled', revision=?3, updated_at=?4
         WHERE project_id=?1 AND job_id=?2",
        (project_id, job_id, new_revision, crate::store::now_unix()),
    )?;
    events::append_event(
        &tx,
        project_id,
        Some(job_id),
        None,
        "job.state_changed",
        json!({
            "job_id": job_id,
            "previous_state": state,
            "state": "cancelled",
            "revision": new_revision,
        }),
    )?;
    let job = get_job_in_tx(&tx, project_id, job_id)?
        .ok_or_else(|| CoreError::Storage("cancelled job cannot be read back".to_string()))?;
    tx.execute(
        "INSERT INTO idempotency
         (project_id, operation, idempotency_key, request_digest, status_code, result_json, created_at)
         VALUES (?1,'cancelJob',?2,?3,202,?4,?5)",
        (
            project_id,
            idempotency_key,
            request_digest,
            job.to_string(),
            crate::store::now_unix(),
        ),
    )?;
    tx.commit()?;
    Ok((202, job))
}

fn get_job_in_tx(
    tx: &rusqlite::Transaction<'_>,
    project_id: &str,
    job_id: &str,
) -> Result<Option<Value>> {
    let row = tx
        .query_row(
            "SELECT job_id, operation, state, progress, result_ids_json, artifact_digests_json,
                    error_json, revision
             FROM jobs WHERE project_id=?1 AND job_id=?2",
            (project_id, job_id),
            |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, String>(2)?,
                    row.get::<_, f64>(3)?,
                    row.get::<_, String>(4)?,
                    row.get::<_, String>(5)?,
                    row.get::<_, Option<String>>(6)?,
                    row.get::<_, i64>(7)?,
                ))
            },
        )
        .ok();
    Ok(row.map(|row| job_json(project_id, row)))
}

fn job_json(
    project_id: &str,
    (job_id, operation, state, progress, result_ids, artifacts, error, revision): (
        String,
        String,
        String,
        f64,
        String,
        String,
        Option<String>,
        i64,
    ),
) -> Value {
    json!({
        "id": job_id,
        "project_id": project_id,
        "operation": operation,
        "state": state,
        "progress": progress,
        "result_ids": serde_json::from_str::<Value>(&result_ids).unwrap_or_else(|_| json!([])),
        "artifact_digests": serde_json::from_str::<Value>(&artifacts).unwrap_or_else(|_| json!([])),
        "error": error
            .and_then(|text| serde_json::from_str::<Value>(&text).ok())
            .unwrap_or(Value::Null),
        "revision": revision,
    })
}

fn guard_transition(from: &str, to: &str) -> Result<()> {
    let allowed = match from {
        "queued" => matches!(to, "running" | "cancelled" | "failed" | "blocked"),
        "running" => matches!(to, "succeeded" | "failed" | "cancelled" | "timed_out"),
        "blocked" => matches!(to, "queued" | "cancelled" | "failed"),
        _ => false,
    };
    if !allowed {
        return Err(CoreError::conflict(format!(
            "scope_conflict: job cannot move from {from} to {to}"
        )));
    }
    Ok(())
}

fn next_job_counter(store: &Store, project_id: &str) -> Result<u64> {
    let count: i64 = store.conn.query_row(
        "SELECT COUNT(*) FROM jobs WHERE project_id=?1",
        (project_id,),
        |row| row.get(0),
    )?;
    Ok(count.max(0) as u64)
}
