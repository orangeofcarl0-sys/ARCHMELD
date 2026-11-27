// Copyright 2026 ARCHMELD contributors
// SPDX-License-Identifier: Apache-2.0

//! Count-based snapshot retention (B025/S14).
//!
//! Retention is deliberately narrower than a general purge: a snapshot beyond
//! the newest N is deleted only when no plan, ChangeSet, verification, claim,
//! architecture, drift report, runtime/scenario evidence, job, project pointer
//! or explicit pin references it. Event rows are notification history and are
//! pruned through the separate event-retention watermark, so they never make a
//! snapshot immortal and retention never rewrites the event log.

use crate::error::{CoreError, Result};
use crate::hash;
use crate::store::{validate, Store};
use serde::Deserialize;
use serde_json::{json, Value};

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
struct RetentionConfigRequest {
    project_id: String,
    keep_snapshots: i64,
    #[serde(default)]
    auto_prune: bool,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
struct PruneRequest {
    project_id: String,
    keep_snapshots: Option<i64>,
    #[serde(default = "default_true")]
    dry_run: bool,
    #[serde(default)]
    idempotency_key: Option<String>,
}

fn default_true() -> bool {
    true
}

fn short_hash(identity: &str) -> String {
    let digest = hash::digest_bytes(identity.as_bytes());
    let hex = digest.strip_prefix("sha256:").unwrap_or(&digest);
    hex[..32].to_string()
}

/// Persist a project's count-based snapshot retention policy. The policy never
/// deletes by itself; capture-time pruning only runs when `auto_prune` is on.
pub fn configure_retention(store: &mut Store, raw: &Value) -> Result<Value> {
    let request: RetentionConfigRequest = serde_json::from_value(raw.clone())
        .map_err(|err| CoreError::invalid(format!("retention config: {err}")))?;
    validate::require_id("project_id", &request.project_id)?;
    if !(1..=100_000).contains(&request.keep_snapshots) {
        return Err(CoreError::invalid("keep_snapshots must be 1..=100000"));
    }
    store.ensure_project_exists(&request.project_id)?;
    store.conn.execute(
        "INSERT INTO retention_config (project_id,keep_snapshots,auto_prune,updated_at)
         VALUES (?1,?2,?3,?4)
         ON CONFLICT(project_id) DO UPDATE SET
           keep_snapshots=excluded.keep_snapshots,
           auto_prune=excluded.auto_prune,
           updated_at=excluded.updated_at",
        (
            &request.project_id,
            request.keep_snapshots,
            if request.auto_prune { 1 } else { 0 },
            crate::store::now_unix(),
        ),
    )?;
    retention_status(store, &request.project_id)
}

pub fn retention_status(store: &Store, project_id: &str) -> Result<Value> {
    validate::require_id("project_id", project_id)?;
    store.ensure_project_exists(project_id)?;
    let config: Option<(i64, i64)> = store
        .conn
        .query_row(
            "SELECT keep_snapshots,auto_prune FROM retention_config WHERE project_id=?1",
            (project_id,),
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .ok();
    let snapshots: i64 = store.conn.query_row(
        "SELECT COUNT(*) FROM snapshots WHERE project_id=?1",
        (project_id,),
        |row| row.get(0),
    )?;
    let pins: i64 = store.conn.query_row(
        "SELECT COUNT(*) FROM retention_pins WHERE project_id=?1 AND kind='snapshot'",
        (project_id,),
        |row| row.get(0),
    )?;
    Ok(json!({
        "project_id": project_id,
        "configured": config.is_some(),
        "keep_snapshots": config.map(|value| value.0),
        "auto_prune": config.map(|value| value.1 == 1),
        "snapshot_count": snapshots,
        "snapshot_pins": pins,
    }))
}

/// Delete snapshots outside the newest N only when no higher-level object
/// references them. Referenced snapshots stay visible with explicit reasons.
pub fn prune_snapshots(store: &mut Store, raw: &Value) -> Result<Value> {
    let request: PruneRequest = serde_json::from_value(raw.clone())
        .map_err(|err| CoreError::invalid(format!("retention prune request: {err}")))?;
    validate::require_id("project_id", &request.project_id)?;
    if let Some(key) = &request.idempotency_key {
        validate::require_id("idempotency_key", key)?;
    }
    store.ensure_project_exists(&request.project_id)?;
    let configured: Option<i64> = store
        .conn
        .query_row(
            "SELECT keep_snapshots FROM retention_config WHERE project_id=?1",
            (&request.project_id,),
            |row| row.get(0),
        )
        .ok();
    let keep = request.keep_snapshots.or(configured).unwrap_or(10);
    if !(1..=100_000).contains(&keep) {
        return Err(CoreError::invalid("keep_snapshots must be 1..=100000"));
    }
    prune_with(store, &request.project_id, keep, request.dry_run, "manual")
}

/// Capture-time hook: only runs when the project explicitly enabled
/// `auto_prune`. A retention failure is reported in the capture result instead
/// of being hidden.
pub(crate) fn auto_prune(store: &mut Store, project_id: &str) -> Result<Option<Value>> {
    let config: Option<(i64, i64)> = store
        .conn
        .query_row(
            "SELECT keep_snapshots,auto_prune FROM retention_config WHERE project_id=?1",
            (project_id,),
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .ok();
    let Some((keep, auto)) = config else {
        return Ok(None);
    };
    if auto != 1 {
        return Ok(None);
    }
    prune_with(store, project_id, keep, false, "automatic").map(Some)
}

pub fn list_retention_runs(store: &Store, project_id: &str) -> Result<Vec<Value>> {
    validate::require_id("project_id", project_id)?;
    let mut statement = store.conn.prepare(
        "SELECT run_id,mode,keep_snapshots,kept_count,candidate_count,deleted_count,
                details_json,created_at
         FROM retention_runs WHERE project_id=?1 ORDER BY created_at DESC, run_id DESC",
    )?;
    let rows = statement.query_map((project_id,), |row| {
        Ok((
            row.get::<_, String>(0)?,
            row.get::<_, String>(1)?,
            row.get::<_, i64>(2)?,
            row.get::<_, i64>(3)?,
            row.get::<_, i64>(4)?,
            row.get::<_, i64>(5)?,
            row.get::<_, String>(6)?,
            row.get::<_, i64>(7)?,
        ))
    })?;
    let mut runs = Vec::new();
    for row in rows {
        let (run_id, mode, keep, kept, candidates, deleted, details, created_at) = row?;
        let details: Value = serde_json::from_str(&details)
            .map_err(|err| CoreError::Storage(format!("retention details corrupt: {err}")))?;
        runs.push(json!({
            "run_id": run_id,
            "mode": mode,
            "keep_snapshots": keep,
            "kept_count": kept,
            "candidate_count": candidates,
            "deleted_count": deleted,
            "details": details,
            "created_at": created_at,
        }));
    }
    Ok(runs)
}

fn prune_with(
    store: &mut Store,
    project_id: &str,
    keep: i64,
    dry_run: bool,
    mode: &str,
) -> Result<Value> {
    store.ensure_project_exists(project_id)?;
    let mut statement = store.conn.prepare(
        "SELECT snapshot_id,created_at,manifest_artifact FROM snapshots
         WHERE project_id=?1 ORDER BY created_at DESC, snapshot_id DESC",
    )?;
    let rows = statement.query_map((project_id,), |row| {
        Ok((
            row.get::<_, String>(0)?,
            row.get::<_, i64>(1)?,
            row.get::<_, String>(2)?,
        ))
    })?;
    let snapshots = rows.collect::<rusqlite::Result<Vec<_>>>()?;
    drop(statement);
    let kept: Vec<Value> = snapshots
        .iter()
        .take(keep as usize)
        .map(|(snapshot_id, created_at, manifest)| {
            json!({
                "snapshot_id": snapshot_id,
                "created_at": created_at,
                "manifest_artifact": manifest,
                "reason": "within_keep_count",
            })
        })
        .collect();
    let mut candidates = Vec::new();
    let mut retained = Vec::new();
    for (snapshot_id, created_at, manifest) in snapshots.iter().skip(keep as usize) {
        let reasons = retention_references(store, project_id, snapshot_id)?;
        let counts = snapshot_row_counts(store, project_id, snapshot_id);
        if reasons.is_empty() {
            candidates.push(json!({
                "snapshot_id": snapshot_id,
                "created_at": created_at,
                "manifest_artifact": manifest,
                "counts": counts,
            }));
        } else {
            retained.push(json!({
                "snapshot_id": snapshot_id,
                "created_at": created_at,
                "manifest_artifact": manifest,
                "reasons": reasons,
                "counts": counts,
            }));
        }
    }
    let mut deleted = Vec::new();
    if !dry_run {
        let tx = store.conn.transaction()?;
        for candidate in &candidates {
            let snapshot_id = candidate["snapshot_id"].as_str().unwrap_or("");
            let counts = &candidate["counts"];
            // Child rows first, snapshot row last. The snapshot manifest
            // artifact stays in the CAS until a separate GC pass proves it is
            // unreferenced.
            tx.execute(
                "DELETE FROM relation_evidence WHERE project_id=?1 AND snapshot_id=?2",
                (project_id, snapshot_id),
            )?;
            tx.execute(
                "DELETE FROM relations WHERE project_id=?1 AND snapshot_id=?2",
                (project_id, snapshot_id),
            )?;
            tx.execute(
                "DELETE FROM backend_mappings WHERE project_id=?1 AND snapshot_id=?2",
                (project_id, snapshot_id),
            )?;
            tx.execute(
                "DELETE FROM diagnostics WHERE project_id=?1 AND snapshot_id=?2",
                (project_id, snapshot_id),
            )?;
            tx.execute(
                "DELETE FROM evidence WHERE project_id=?1 AND snapshot_id=?2",
                (project_id, snapshot_id),
            )?;
            tx.execute(
                "DELETE FROM staging_relation_evidence WHERE project_id=?1 AND snapshot_id=?2",
                (project_id, snapshot_id),
            )?;
            tx.execute(
                "DELETE FROM staging_relations WHERE project_id=?1 AND snapshot_id=?2",
                (project_id, snapshot_id),
            )?;
            tx.execute(
                "DELETE FROM staging_evidence WHERE project_id=?1 AND snapshot_id=?2",
                (project_id, snapshot_id),
            )?;
            tx.execute(
                "DELETE FROM staging_entities WHERE project_id=?1 AND snapshot_id=?2",
                (project_id, snapshot_id),
            )?;
            tx.execute(
                "DELETE FROM staging_diagnostics WHERE project_id=?1 AND snapshot_id=?2",
                (project_id, snapshot_id),
            )?;
            tx.execute(
                "DELETE FROM run_batches WHERE project_id=?1 AND snapshot_id=?2",
                (project_id, snapshot_id),
            )?;
            tx.execute(
                "DELETE FROM generations WHERE project_id=?1 AND snapshot_id=?2",
                (project_id, snapshot_id),
            )?;
            tx.execute(
                "DELETE FROM files WHERE project_id=?1 AND snapshot_id=?2",
                (project_id, snapshot_id),
            )?;
            tx.execute(
                "DELETE FROM snapshots WHERE project_id=?1 AND snapshot_id=?2",
                (project_id, snapshot_id),
            )?;
            deleted.push(json!({
                "snapshot_id": snapshot_id,
                "counts": counts,
            }));
        }
        let run_id = format!(
            "retention.{}",
            short_hash(&format!(
                "{}:{}:{}:{}:{}",
                project_id,
                keep,
                mode,
                dry_run,
                crate::store::now_unix()
            ))
        );
        let details = json!({
            "kept": kept,
            "retained": retained,
            "candidates": candidates,
            "deleted": deleted,
            "event_log_pruned": false,
            "note": "events are notifications; they are pruned through the separate event-retention watermark",
        });
        tx.execute(
            "INSERT INTO retention_runs
             (project_id,run_id,mode,keep_snapshots,kept_count,candidate_count,
              deleted_count,details_json,created_at)
             VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9)",
            (
                project_id,
                &run_id,
                mode,
                keep,
                kept.len() as i64,
                candidates.len() as i64,
                deleted.len() as i64,
                hash::canonical_json(&details)
                    .map_err(|err| CoreError::invalid(format!("retention details: {err}")))?,
                crate::store::now_unix(),
            ),
        )?;
        tx.commit()?;
    }
    Ok(json!({
        "project_id": project_id,
        "keep_snapshots": keep,
        "dry_run": dry_run,
        "mode": mode,
        "snapshot_count": snapshots.len(),
        "kept_count": kept.len(),
        "candidate_count": candidates.len(),
        "deleted_count": deleted.len(),
        "kept": kept,
        "retained": retained,
        "candidates": candidates,
        "deleted": deleted,
        "whole_tree_reset": false,
        "event_log_pruned": false,
    }))
}

fn retention_references(
    store: &Store,
    project_id: &str,
    snapshot_id: &str,
) -> Result<Vec<String>> {
    let probe = |sql: &str| -> Result<bool> {
        let mut statement = store.conn.prepare(sql)?;
        let mut rows = statement.query((project_id, snapshot_id))?;
        Ok(rows.next()?.is_some())
    };
    let mut reasons = Vec::new();
    if probe("SELECT 1 FROM retention_pins WHERE project_id=?1 AND kind='snapshot' AND target=?2")? {
        reasons.push("pinned".to_string());
    }
    if probe("SELECT 1 FROM projects WHERE project_id=?1 AND current_snapshot_id=?2")? {
        reasons.push("project_current".to_string());
    }
    for (sql, reason) in [
        (
            "SELECT 1 FROM plans WHERE project_id=?1 AND base_snapshot_id=?2",
            "plan",
        ),
        (
            "SELECT 1 FROM changesets WHERE project_id=?1 AND (base_snapshot_id=?2 OR payload_json LIKE '%'||?2||'%')",
            "changeset",
        ),
        (
            "SELECT 1 FROM verification_runs WHERE project_id=?1 AND (snapshot_id=?2 OR base_snapshot_id=?2)",
            "verification",
        ),
        (
            "SELECT 1 FROM claims WHERE project_id=?1 AND snapshot_id=?2",
            "claim",
        ),
        (
            "SELECT 1 FROM architectures WHERE project_id=?1 AND snapshot_id=?2",
            "architecture",
        ),
        (
            "SELECT 1 FROM investigations WHERE project_id=?1 AND snapshot_id=?2",
            "investigation",
        ),
        (
            "SELECT 1 FROM drift_reports WHERE project_id=?1 AND (base_snapshot_id=?2 OR new_snapshot_id=?2)",
            "drift_report",
        ),
        (
            "SELECT 1 FROM evidence WHERE project_id=?1 AND snapshot_id=?2",
            "evidence",
        ),
        (
            "SELECT 1 FROM runtime_runs WHERE project_id=?1 AND snapshot_id=?2",
            "runtime_evidence",
        ),
        (
            "SELECT 1 FROM scenario_runs WHERE project_id=?1 AND snapshot_id=?2",
            "scenario_evidence",
        ),
        (
            "SELECT 1 FROM jobs WHERE project_id=?1 AND result_ids_json LIKE '%'||?2||'%'",
            "job",
        ),
    ] {
        if probe(sql)? {
            reasons.push(reason.to_string());
        }
    }
    if probe("SELECT 1 FROM generations WHERE project_id=?1 AND snapshot_id=?2 AND status='staging'")?
    {
        reasons.push("active_import".to_string());
    } else if probe(
        "SELECT 1 FROM run_batches WHERE project_id=?1 AND snapshot_id=?2 AND is_final=0",
    )? {
        reasons.push("active_import".to_string());
    } else {
        for table in [
            "staging_entities",
            "staging_evidence",
            "staging_relations",
            "staging_relation_evidence",
            "staging_diagnostics",
        ] {
            let sql = format!(
                "SELECT 1 FROM {table} WHERE project_id=?1 AND snapshot_id=?2"
            );
            if probe(&sql)? {
                reasons.push("staged_import_rows".to_string());
                break;
            }
        }
    }
    reasons.sort();
    reasons.dedup();
    Ok(reasons)
}

fn snapshot_row_counts(store: &Store, project_id: &str, snapshot_id: &str) -> Value {
    let count = |table: &str| -> i64 {
        store
            .conn
            .query_row(
                &format!("SELECT COUNT(*) FROM {table} WHERE project_id=?1 AND snapshot_id=?2"),
                (project_id, snapshot_id),
                |row| row.get(0),
            )
            .unwrap_or(0)
    };
    json!({
        "files": count("files"),
        "entities": count("entities"),
        "relations": count("relations"),
        "evidence": count("evidence"),
        "diagnostics": count("diagnostics"),
        "generations": count("generations"),
        "run_batches": count("run_batches"),
    })
}
