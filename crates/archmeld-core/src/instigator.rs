// Copyright 2026 ARCHMELD contributors
// SPDX-License-Identifier: Apache-2.0

//! Optional asynchronous Instigator (S12 §4).
//!
//! It can only propose `NUDGE` or `SILENT` for an existing task revision. It
//! cannot create tasks/goals, change task state, approve, apply or block
//! completion. Delivery is a separate, non-blocking action; stale/expired/
//! finished messages are discarded at delivery time.

use crate::error::{CoreError, Result};
use crate::events::append_event;
use crate::hash;
use crate::store::{validate, Store};
use serde::Deserialize;
use serde_json::{json, Value};

const MAX_COOLDOWN_SECONDS: i64 = 86_400;
const MAX_NUDGES_PER_TASK: i64 = 10;
const ACTIVE_TASK_STATES: &[&str] = &["queued", "running", "blocked"];

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
struct ConfigureRequest {
    project_id: String,
    enabled: bool,
    cooldown_seconds: Option<i64>,
    max_nudges_per_task: Option<i64>,
    actor: String,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
struct NudgeRequest {
    project_id: String,
    task_id: String,
    task_revision: i64,
    reason: String,
    scope_ref: String,
    expires_at: i64,
    idempotency_key: String,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
struct DeliverRequest {
    project_id: String,
    idempotency_key: String,
}

pub fn configure(store: &mut Store, raw: &Value) -> Result<Value> {
    let request: ConfigureRequest = serde_json::from_value(raw.clone())
        .map_err(|err| CoreError::invalid(format!("instigator configure: {err}")))?;
    validate::require_id("project_id", &request.project_id)?;
    validate::require_non_empty("actor", &request.actor)?;
    let cooldown = request.cooldown_seconds.unwrap_or(300);
    let max_nudges = request.max_nudges_per_task.unwrap_or(2);
    if !(0..=MAX_COOLDOWN_SECONDS).contains(&cooldown) {
        return Err(CoreError::invalid("cooldown_seconds is out of range"));
    }
    if !(0..=MAX_NUDGES_PER_TASK).contains(&max_nudges) {
        return Err(CoreError::invalid("max_nudges_per_task is out of range"));
    }
    store.ensure_project_exists(&request.project_id)?;
    store.conn.execute(
        "INSERT INTO instigator_config
         (project_id,enabled,cooldown_seconds,max_nudges_per_task,updated_at)
         VALUES (?1,?2,?3,?4,?5)
         ON CONFLICT(project_id) DO UPDATE SET
           enabled=excluded.enabled,
           cooldown_seconds=excluded.cooldown_seconds,
           max_nudges_per_task=excluded.max_nudges_per_task,
           updated_at=excluded.updated_at",
        (
            &request.project_id,
            if request.enabled { 1 } else { 0 },
            cooldown,
            max_nudges,
            crate::store::now_unix(),
        ),
    )?;
    status(store, &request.project_id)
}

/// Propose one bounded nudge. The function only reads the task row; it never
/// transitions it and never waits for a consumer.
pub fn propose_nudge(store: &mut Store, raw: &Value) -> Result<Value> {
    let request: NudgeRequest = serde_json::from_value(raw.clone())
        .map_err(|err| CoreError::invalid(format!("instigator nudge: {err}")))?;
    validate::require_id("project_id", &request.project_id)?;
    validate::require_id("idempotency_key", &request.idempotency_key)?;
    validate::require_non_empty("reason", &request.reason)?;
    validate::require_non_empty("scope_ref", &request.scope_ref)?;
    let request_digest = hash::digest_value(raw)
        .map_err(|err| CoreError::invalid(format!("instigator nudge: {err}")))?;
    if let Some((recorded, _, result)) = store.idempotency_get(
        &request.project_id,
        "proposeNudge",
        &request.idempotency_key,
    )? {
        if recorded != request_digest {
            return Err(CoreError::conflict(
                "scope_conflict: idempotency_key was used with a different nudge",
            ));
        }
        return serde_json::from_str(&result)
            .map_err(|err| CoreError::Storage(format!("stored nudge corrupt: {err}")));
    }
    let config = config_for(store, &request.project_id)?;
    let Some((enabled, cooldown, max_nudges)) = config else {
        return silent(store, &request, "instigator_disabled", &request_digest);
    };
    if !enabled {
        return silent(store, &request, "instigator_disabled", &request_digest);
    }
    // The scope is the existing task, never a new goal.
    if request.scope_ref != request.task_id {
        return silent(
            store,
            &request,
            "scope_not_an_existing_task",
            &request_digest,
        );
    }
    let Some(job) = crate::jobs::get_job(store, &request.project_id, &request.task_id)? else {
        return Err(CoreError::not_found(format!(
            "task {} does not exist; the Instigator cannot create tasks",
            request.task_id
        )));
    };
    let state = job["state"].as_str().unwrap_or("");
    let revision = job["revision"].as_i64().unwrap_or(0);
    if !ACTIVE_TASK_STATES.contains(&state) {
        return silent(store, &request, "task_finished", &request_digest);
    }
    if revision != request.task_revision {
        return silent(store, &request, "stale_task_revision", &request_digest);
    }
    let now = crate::store::now_unix();
    if request.expires_at <= now {
        return silent(store, &request, "expired", &request_digest);
    }
    let delivered_or_pending: i64 = store.conn.query_row(
        "SELECT COUNT(*) FROM instigator_messages
         WHERE project_id=?1 AND task_id=?2 AND decision='NUDGE' AND state IN ('pending','delivered')",
        (&request.project_id, &request.task_id),
        |row| row.get(0),
    )?;
    if delivered_or_pending >= max_nudges {
        return silent(store, &request, "nudge_budget_exhausted", &request_digest);
    }
    let recent: i64 = store.conn.query_row(
        "SELECT COUNT(*) FROM instigator_messages
         WHERE project_id=?1 AND task_id=?2 AND reason=?3 AND decision='NUDGE'
           AND created_at > ?4",
        (
            &request.project_id,
            &request.task_id,
            &request.reason,
            now - cooldown,
        ),
        |row| row.get(0),
    )?;
    if recent > 0 {
        return silent(store, &request, "cooldown", &request_digest);
    }
    let message_id = format!(
        "nudge.{}",
        short_hash(&format!(
            "{}:{}:{}:{}:{}",
            request.project_id,
            request.task_id,
            request.task_revision,
            request.reason,
            request.idempotency_key
        ))
    );
    store.conn.execute(
        "INSERT INTO instigator_messages
         (project_id,message_id,task_id,task_revision,reason,scope_ref,decision,state,
          expires_at,created_at,delivered_at,discard_reason)
         VALUES (?1,?2,?3,?4,?5,?6,'NUDGE','pending',?7,?8,NULL,NULL)",
        (
            &request.project_id,
            &message_id,
            &request.task_id,
            request.task_revision,
            &request.reason,
            &request.scope_ref,
            request.expires_at,
            now,
        ),
    )?;
    let result = json!({
        "decision": "NUDGE",
        "message_id": message_id,
        "task_id": request.task_id,
        "task_revision": request.task_revision,
        "reason": request.reason,
        "scope_ref": request.scope_ref,
        "expires_at": request.expires_at,
        "state": "pending",
        "may_change_task_state": false,
        "may_create_tasks": false,
        "blocking": false,
    });
    store_idempotency(
        store,
        &request.project_id,
        &request.idempotency_key,
        &request_digest,
        &result,
    )?;
    Ok(result)
}

/// Deliver due nudges. Delivery checks the task revision/expiry again. A
/// missing or finished task discards the message instead of waking it.
pub fn deliver_due(store: &mut Store, raw: &Value) -> Result<Value> {
    let request: DeliverRequest = serde_json::from_value(raw.clone())
        .map_err(|err| CoreError::invalid(format!("instigator deliver: {err}")))?;
    validate::require_id("project_id", &request.project_id)?;
    validate::require_id("idempotency_key", &request.idempotency_key)?;
    let request_digest = hash::digest_value(raw)
        .map_err(|err| CoreError::invalid(format!("instigator deliver: {err}")))?;
    if let Some((recorded, _, result)) = store.idempotency_get(
        &request.project_id,
        "deliverNudges",
        &request.idempotency_key,
    )? {
        if recorded != request_digest {
            return Err(CoreError::conflict(
                "scope_conflict: idempotency_key was used with a different delivery request",
            ));
        }
        return serde_json::from_str(&result)
            .map_err(|err| CoreError::Storage(format!("stored delivery corrupt: {err}")));
    }
    let config = config_for(store, &request.project_id)?;
    let enabled = config.map(|(enabled, _, _)| enabled).unwrap_or(false);
    let now = crate::store::now_unix();
    let mut statement = store.conn.prepare(
        "SELECT message_id,task_id,task_revision,reason,scope_ref,expires_at
         FROM instigator_messages
         WHERE project_id=?1 AND state='pending' ORDER BY created_at,message_id",
    )?;
    let rows: Vec<(String, String, i64, String, String, i64)> = statement
        .query_map((&request.project_id,), |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, i64>(2)?,
                row.get::<_, String>(3)?,
                row.get::<_, String>(4)?,
                row.get::<_, i64>(5)?,
            ))
        })?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    drop(statement);
    let mut delivered = Vec::new();
    let mut discarded = Vec::new();
    for row in rows {
        let (message_id, task_id, task_revision, reason, scope_ref, expires_at) = row;
        if !enabled {
            discard(
                store,
                &request.project_id,
                &message_id,
                "instigator_disabled",
            )?;
            discarded.push(json!({"message_id": message_id, "reason": "instigator_disabled"}));
            continue;
        }
        if expires_at <= now {
            discard(store, &request.project_id, &message_id, "expired")?;
            discarded.push(json!({"message_id": message_id, "reason": "expired"}));
            continue;
        }
        let job = crate::jobs::get_job(store, &request.project_id, &task_id)?;
        let Some(job) = job else {
            discard(store, &request.project_id, &message_id, "task_missing")?;
            discarded.push(json!({"message_id": message_id, "reason": "task_missing"}));
            continue;
        };
        let state = job["state"].as_str().unwrap_or("");
        let revision = job["revision"].as_i64().unwrap_or(0);
        if !ACTIVE_TASK_STATES.contains(&state) {
            discard(store, &request.project_id, &message_id, "task_finished")?;
            discarded.push(json!({"message_id": message_id, "reason": "task_finished"}));
            continue;
        }
        if revision != task_revision {
            discard(
                store,
                &request.project_id,
                &message_id,
                "stale_task_revision",
            )?;
            discarded.push(json!({"message_id": message_id, "reason": "stale_task_revision"}));
            continue;
        }
        let tx = store.conn.transaction()?;
        tx.execute(
            "UPDATE instigator_messages SET state='delivered', delivered_at=?1
             WHERE project_id=?2 AND message_id=?3",
            (now, &request.project_id, &message_id),
        )?;
        append_event(
            &tx,
            &request.project_id,
            None,
            None,
            "instigator.nudge",
            json!({
                "message_id": message_id,
                "task_id": task_id,
                "task_revision": task_revision,
                "expires_at": expires_at,
                "reason": reason,
                "existing_scope_ref": scope_ref,
            }),
        )?;
        tx.commit()?;
        delivered.push(json!({
            "message_id": message_id,
            "task_id": task_id,
            "task_revision": task_revision,
            "reason": reason,
            "scope_ref": scope_ref,
            "expires_at": expires_at,
        }));
    }
    let result = json!({
        "enabled": enabled,
        "delivered": delivered,
        "discarded": discarded,
        "blocking": false,
        "waited_for_approval": false,
    });
    store_idempotency(
        store,
        &request.project_id,
        &request.idempotency_key,
        &request_digest,
        &result,
    )?;
    Ok(result)
}

pub fn status(store: &Store, project_id: &str) -> Result<Value> {
    validate::require_id("project_id", project_id)?;
    let config = config_for(store, project_id)?;
    let (enabled, cooldown, max_nudges) = config.unwrap_or((false, 300, 2));
    let mut statement = store.conn.prepare(
        "SELECT message_id,task_id,task_revision,reason,scope_ref,decision,state,expires_at,
                created_at,delivered_at,discard_reason
         FROM instigator_messages WHERE project_id=?1 ORDER BY created_at DESC, message_id DESC
         LIMIT 100",
    )?;
    let rows = statement.query_map((project_id,), |row| {
        Ok(json!({
            "message_id": row.get::<_, String>(0)?,
            "task_id": row.get::<_, String>(1)?,
            "task_revision": row.get::<_, i64>(2)?,
            "reason": row.get::<_, String>(3)?,
            "scope_ref": row.get::<_, String>(4)?,
            "decision": row.get::<_, String>(5)?,
            "state": row.get::<_, String>(6)?,
            "expires_at": row.get::<_, i64>(7)?,
            "created_at": row.get::<_, i64>(8)?,
            "delivered_at": row.get::<_, Option<i64>>(9)?,
            "discard_reason": row.get::<_, Option<String>>(10)?,
        }))
    })?;
    let messages = rows.collect::<rusqlite::Result<Vec<Value>>>()?;
    Ok(json!({
        "project_id": project_id,
        "enabled": enabled,
        "cooldown_seconds": cooldown,
        "max_nudges_per_task": max_nudges,
        "allowed_decisions": ["NUDGE", "SILENT"],
        "may_change_task_state": false,
        "may_create_tasks": false,
        "may_block_delivery": false,
        "messages": messages,
    }))
}

fn config_for(store: &Store, project_id: &str) -> Result<Option<(bool, i64, i64)>> {
    Ok(store
        .conn
        .query_row(
            "SELECT enabled,cooldown_seconds,max_nudges_per_task FROM instigator_config
             WHERE project_id=?1",
            (project_id,),
            |row| {
                Ok((
                    row.get::<_, i64>(0)? == 1,
                    row.get::<_, i64>(1)?,
                    row.get::<_, i64>(2)?,
                ))
            },
        )
        .ok())
}

fn silent(
    store: &mut Store,
    request: &NudgeRequest,
    reason: &str,
    request_digest: &str,
) -> Result<Value> {
    let result = json!({
        "decision": "SILENT",
        "task_id": request.task_id,
        "task_revision": request.task_revision,
        "reason": reason,
        "blocking": false,
        "may_change_task_state": false,
    });
    store_idempotency(
        store,
        &request.project_id,
        &request.idempotency_key,
        request_digest,
        &result,
    )?;
    Ok(result)
}

fn store_idempotency(
    store: &mut Store,
    project_id: &str,
    key: &str,
    digest: &str,
    result: &Value,
) -> Result<()> {
    let result_json = serde_json::to_string(result)
        .map_err(|err| CoreError::Storage(format!("nudge encode: {err}")))?;
    store.conn.execute(
        "INSERT INTO idempotency
         (project_id,operation,idempotency_key,request_digest,status_code,result_json,created_at)
         VALUES (?1,?2,?3,?4,200,?5,?6)",
        (
            project_id,
            "proposeNudge",
            key,
            digest,
            result_json,
            crate::store::now_unix(),
        ),
    )?;
    Ok(())
}

fn discard(store: &mut Store, project_id: &str, message_id: &str, reason: &str) -> Result<()> {
    store.conn.execute(
        "UPDATE instigator_messages SET state='discarded', discard_reason=?1
         WHERE project_id=?2 AND message_id=?3 AND state='pending'",
        (reason, project_id, message_id),
    )?;
    Ok(())
}

fn short_hash(identity: &str) -> String {
    let digest = hash::digest_bytes(identity.as_bytes());
    let hex = digest.strip_prefix("sha256:").unwrap_or(&digest);
    hex[..32].to_string()
}
