// Copyright 2026 ARCHMELD contributors
// SPDX-License-Identifier: Apache-2.0

//! Manager / Expedition lifecycle (S01, S14).
//!
//! The project id is the identity; a workspace path is only a current
//! location. Opening never writes into the workspace, and every external state
//! root is rejected if it lives inside the workspace. Expedition sessions are
//! distinguishable from Manager sessions, survive restarts as explicit
//! `interrupted` records, and can be promoted without losing evidence.

use crate::error::{CoreError, Result};
use crate::hash;
use crate::store::{validate, Store};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

pub struct OpenRequest<'a> {
    pub project_id: &'a str,
    pub mode: &'a str,
    pub workspace_root: &'a Path,
    pub state_root: &'a Path,
    pub config: Value,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RootRecord {
    pub root_path: String,
    pub active: bool,
}

pub struct SessionRecord {
    pub session_id: String,
    pub mode: String,
    pub state: String,
    pub workspace_root: String,
    pub state_root: String,
}

pub fn open_project(store: &mut Store, request: OpenRequest<'_>) -> Result<Value> {
    validate::require_id("project_id", request.project_id)?;
    if !matches!(request.mode, "manager" | "expedition") {
        return Err(CoreError::invalid("mode must be manager or expedition"));
    }
    if !request.config.is_object() {
        return Err(CoreError::invalid("project config must be an object"));
    }
    let workspace = canonical_dir(request.workspace_root)?;
    std::fs::create_dir_all(request.state_root)?;
    let state_root = canonical_dir(request.state_root)?;
    if inside(&state_root, &workspace) {
        return Err(CoreError::invalid(
            "state_root_inside_workspace: the external state root must not be inside the analyzed repository",
        ));
    }
    if inside(&workspace, &state_root) {
        return Err(CoreError::invalid(
            "workspace_inside_state_root: refusing a workspace nested under the ledger root",
        ));
    }
    let config_json = hash::canonical_json(&request.config)
        .map_err(|err| CoreError::invalid(format!("project config: {err}")))?;
    let workspace_text = path_text(&workspace);
    let state_text = path_text(&state_root);
    let now = crate::store::now_unix();

    let tx = store.conn.transaction()?;
    let existing: Option<(String, i64)> = tx
        .query_row(
            "SELECT mode, revision FROM projects WHERE project_id=?1",
            (request.project_id,),
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .ok();
    let revision = match existing {
        Some((mode, revision)) => {
            if mode != request.mode {
                return Err(CoreError::conflict(format!(
                    "project {} is registered as {mode}; use promote for expedition→manager",
                    request.project_id
                )));
            }
            let revision = revision + 1;
            tx.execute(
                "UPDATE projects SET revision=?2, state='open', config_json=?3 WHERE project_id=?1",
                (request.project_id, revision, &config_json),
            )?;
            revision
        }
        None => {
            tx.execute(
                "INSERT INTO projects
                 (project_id, mode, revision, current_snapshot_id, config_digest, state, created_at, config_json)
                 VALUES (?1, ?2, 1, NULL, NULL, 'open', ?3, ?4)",
                (request.project_id, request.mode, now, &config_json),
            )?;
            1
        }
    };
    tx.execute(
        "UPDATE project_roots SET active=0 WHERE project_id=?1",
        (request.project_id,),
    )?;
    tx.execute(
        "INSERT INTO project_roots (project_id, root_path, first_seen, last_seen, active)
         VALUES (?1, ?2, ?3, ?3, 1)
         ON CONFLICT(project_id, root_path)
         DO UPDATE SET last_seen=excluded.last_seen, active=1",
        (request.project_id, &workspace_text, now),
    )?;
    let session_id = format!(
        "session.{}",
        short_hash(&format!("{}:{}:{}:{}", request.project_id, now, std::process::id(), revision))
    );
    let owner_token = short_hash(&format!("owner:{}:{}:{}", session_id, std::process::id(), now));
    tx.execute(
        "INSERT INTO sessions
         (session_id, project_id, mode, state, workspace_root, state_root, owner_pid, owner_token, created_at)
         VALUES (?1, ?2, ?3, 'open', ?4, ?5, ?6, ?7, ?8)",
        (
            &session_id,
            request.project_id,
            request.mode,
            &workspace_text,
            &state_text,
            std::process::id() as i64,
            &owner_token,
            now,
        ),
    )?;
    tx.commit()?;
    get_project(store, request.project_id)?.ok_or_else(|| {
        CoreError::Storage("opened project cannot be read back".to_string())
    })
}

/// Open through a trusted workspace handle. The handle->project mapping is
/// persisted, so reopening the same handle after a server restart keeps the
/// project id even though a new session is created.
pub fn open_project_for_handle(
    store: &mut Store,
    handle: &str,
    workspace_root: &Path,
    mode: &str,
    state_root: &Path,
    config: Value,
) -> Result<Value> {
    validate::require_id("workspace_handle", handle)?;
    let existing: Option<String> = store
        .conn
        .query_row(
            "SELECT project_id FROM workspace_handles WHERE handle=?1",
            (handle,),
            |row| row.get(0),
        )
        .ok();
    let project_id = match existing {
        Some(id) => id,
        None => format!(
            "project.{}",
            short_hash(&format!("{handle}:{}", crate::store::now_unix()))
        ),
    };
    let project = open_project(
        store,
        OpenRequest {
            project_id: &project_id,
            mode,
            workspace_root,
            state_root,
            config,
        },
    )?;
    store.conn.execute(
        "INSERT INTO workspace_handles (handle, project_id, root_path, created_at)
         VALUES (?1, ?2, ?3, ?4)
         ON CONFLICT(handle) DO UPDATE SET project_id=excluded.project_id, root_path=excluded.root_path",
        (
            handle,
            &project_id,
            workspace_root.to_string_lossy().to_string(),
            crate::store::now_unix(),
        ),
    )?;
    Ok(project)
}

pub fn close_project_checked(
    store: &mut Store,
    project_id: &str,
    expected_revision: i64,
    purge_ephemeral: bool,
) -> Result<Value> {
    let current = get_project(store, project_id)?
        .ok_or_else(|| CoreError::not_found(format!("project {project_id} not found")))?;
    let revision = current["revision"].as_i64().unwrap_or(0);
    if revision != expected_revision {
        return Err(CoreError::conflict(format!(
            "stale_snapshot: expected_revision {expected_revision} does not match project revision {revision}"
        )));
    }
    close_project(store, project_id, purge_ephemeral)
}

pub fn promote_project_checked(
    store: &mut Store,
    project_id: &str,
    expected_revision: i64,
) -> Result<Value> {
    let current = get_project(store, project_id)?
        .ok_or_else(|| CoreError::not_found(format!("project {project_id} not found")))?;
    let revision = current["revision"].as_i64().unwrap_or(0);
    if revision != expected_revision {
        return Err(CoreError::conflict(format!(
            "stale_snapshot: expected_revision {expected_revision} does not match project revision {revision}"
        )));
    }
    promote_project(store, project_id)
}

pub fn close_project(
    store: &mut Store,
    project_id: &str,
    purge_ephemeral: bool,
) -> Result<Value> {
    let tx = store.conn.transaction()?;
    let session: Option<(String, String, String, String)> = tx
        .query_row(
            "SELECT session_id, mode, workspace_root, state_root FROM sessions
             WHERE project_id=?1 AND state='open' ORDER BY created_at DESC, rowid DESC LIMIT 1",
            (project_id,),
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?)),
        )
        .ok();
    let Some((session_id, mode, workspace_root, state_root)) = session else {
        return Err(CoreError::conflict(format!(
            "project {project_id} has no open session"
        )));
    };
    let now = crate::store::now_unix();
    tx.execute(
        "UPDATE sessions SET state='closed', closed_at=?2 WHERE session_id=?1",
        (&session_id, now),
    )?;
    tx.execute(
        "UPDATE projects SET state='closed', revision=revision+1 WHERE project_id=?1",
        (project_id,),
    )?;
    tx.commit()?;

    if purge_ephemeral && mode == "expedition" {
        let workspace = PathBuf::from(&workspace_root);
        let root = PathBuf::from(&state_root);
        if !inside(&root, &workspace) {
            // Only rebuildable scratch; the evidence ledger and CAS stay.
            for name in ["cache", "scratch"] {
                let target = root.join(name);
                if target.exists() {
                    let _ = std::fs::remove_dir_all(target);
                }
            }
        }
    }
    get_project(store, project_id)?.ok_or_else(|| CoreError::Storage("project disappeared".into()))
}

/// Expedition → Manager without losing evidence. Before changing the mode we
/// verify the ledger and every referenced content object; the transaction only
/// transfers ownership, it never rewrites entities, claims, contracts or plans.
pub fn promote_project(store: &mut Store, project_id: &str) -> Result<Value> {
    let validation = verify_project_objects(store, project_id)?;
    if !validation["missing"]
        .as_array()
        .map(Vec::is_empty)
        .unwrap_or(false)
        || !validation["hash_mismatches"]
            .as_array()
            .map(Vec::is_empty)
            .unwrap_or(false)
        || !validation["database_problems"]
            .as_array()
            .map(Vec::is_empty)
            .unwrap_or(false)
    {
        return Err(CoreError::conflict(format!(
            "object_validation_failed: promotion refused; missing={}, hash_mismatches={}, database_problems={}",
            validation["missing"].as_array().map(Vec::len).unwrap_or(0),
            validation["hash_mismatches"]
                .as_array()
                .map(Vec::len)
                .unwrap_or(0),
            validation["database_problems"]
                .as_array()
                .map(Vec::len)
                .unwrap_or(0)
        )));
    }
    let counts_before = project_counts(store, project_id)?;
    let tx = store.conn.transaction()?;
    let (mode, state): (String, String) = tx
        .query_row(
            "SELECT mode, state FROM projects WHERE project_id=?1",
            (project_id,),
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .map_err(|_| CoreError::not_found(format!("project {project_id} is not registered")))?;
    if mode == "manager" {
        return Err(CoreError::conflict("project is already a Manager project"));
    }
    if state != "open" && state != "closed" {
        return Err(CoreError::conflict(format!(
            "project state {state} cannot be promoted"
        )));
    }
    let session: Option<(String, String, String)> = tx
        .query_row(
            "SELECT session_id, workspace_root, state_root FROM sessions
             WHERE project_id=?1 ORDER BY created_at DESC, rowid DESC LIMIT 1",
            (project_id,),
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
        )
        .ok();
    let Some((session_id, workspace_root, state_root)) = session else {
        return Err(CoreError::conflict("project has no session to promote"));
    };
    let now = crate::store::now_unix();
    tx.execute(
        "UPDATE sessions SET state='promoted', promoted_to='manager', closed_at=COALESCE(closed_at, ?2)
         WHERE session_id=?1",
        (&session_id, now),
    )?;
    tx.execute(
        "UPDATE projects SET mode='manager', state='open', revision=revision+1 WHERE project_id=?1",
        (project_id,),
    )?;
    let new_session = format!("session.{}", short_hash(&format!("promote:{}:{now}", project_id)));
    tx.execute(
        "INSERT INTO sessions
         (session_id, project_id, mode, state, workspace_root, state_root, owner_pid, owner_token, created_at)
         VALUES (?1, ?2, 'manager', 'open', ?3, ?4, ?5, ?6, ?7)",
        (
            &new_session,
            project_id,
            workspace_root,
            state_root,
            std::process::id() as i64,
            short_hash(&format!("owner:{new_session}")),
            now,
        ),
    )?;
    let counts_after = project_counts_tx(&tx, project_id)?;
    if counts_before != counts_after {
        return Err(CoreError::Storage(
            "promotion changed project object counts; transaction rolled back".to_string(),
        ));
    }
    tx.commit()?;
    let mut project = get_project(store, project_id)?
        .ok_or_else(|| CoreError::Storage("project disappeared".into()))?;
    project["promotion"] = json!({
        "source_session_id": session_id,
        "manager_session_id": new_session,
        "mode_before": "expedition",
        "mode_after": "manager",
        "object_validation": validation,
        "counts_before": counts_before,
        "counts_after": counts_after,
        "records_rewritten": 0,
        "model_calls": 0,
        "source_session_preserved": true,
    });
    Ok(project)
}

fn project_counts(store: &Store, project_id: &str) -> Result<Value> {
    project_counts_conn(&store.conn, project_id)
}

fn project_counts_tx(tx: &rusqlite::Transaction<'_>, project_id: &str) -> Result<Value> {
    project_counts_conn(tx, project_id)
}

fn project_counts_conn(conn: &rusqlite::Connection, project_id: &str) -> Result<Value> {
    let count = |table: &str| -> Result<i64> {
        Ok(conn.query_row(
            &format!("SELECT COUNT(*) FROM {table} WHERE project_id=?1"),
            (project_id,),
            |row| row.get(0),
        )?)
    };
    Ok(json!({
        "snapshots": count("snapshots")?,
        "files": count("files")?,
        "entities": count("entities")?,
        "relations": count("relations")?,
        "evidence": count("evidence")?,
        "claims": count("claims")?,
        "architectures": count("architectures")?,
        "contracts": count("behavior_contracts")?,
        "plans": count("plans")?,
        "changesets": count("changesets")?,
        "verifications": count("verification_runs")?,
    }))
}

/// Validate every project-referenced CAS object before promotion. Missing or
/// corrupted objects are reported with their digests; promotion does not make
/// a best-effort copy or silently drop the affected history.
fn verify_project_objects(store: &Store, project_id: &str) -> Result<Value> {
    let problems = store.integrity_check()?;
    let mut digests = BTreeSet::new();
    collect_column(
        store,
        "SELECT manifest_artifact FROM snapshots WHERE project_id=?1",
        project_id,
        &mut digests,
    )?;
    collect_column(
        store,
        "SELECT blob_digest FROM files WHERE project_id=?1",
        project_id,
        &mut digests,
    )?;
    collect_column(
        store,
        "SELECT artifact_digest FROM evidence WHERE project_id=?1",
        project_id,
        &mut digests,
    )?;
    collect_column(
        store,
        "SELECT artifact_digest FROM runtime_runs WHERE project_id=?1 AND artifact_digest IS NOT NULL",
        project_id,
        &mut digests,
    )?;
    collect_column(
        store,
        "SELECT stdout_digest FROM scenario_runs WHERE project_id=?1 AND stdout_digest IS NOT NULL",
        project_id,
        &mut digests,
    )?;
    collect_column(
        store,
        "SELECT stderr_digest FROM scenario_runs WHERE project_id=?1 AND stderr_digest IS NOT NULL",
        project_id,
        &mut digests,
    )?;
    collect_column(
        store,
        "SELECT stdout_digest FROM check_results WHERE project_id=?1 AND stdout_digest IS NOT NULL",
        project_id,
        &mut digests,
    )?;
    collect_column(
        store,
        "SELECT stderr_digest FROM check_results WHERE project_id=?1 AND stderr_digest IS NOT NULL",
        project_id,
        &mut digests,
    )?;
    collect_json_column(
        store,
        "SELECT artifacts_json FROM check_results WHERE project_id=?1",
        project_id,
        &mut digests,
    )?;
    collect_changeset_artifacts(store, project_id, &mut digests)?;
    collect_json_column(
        store,
        "SELECT artifact_digests_json FROM jobs WHERE project_id=?1",
        project_id,
        &mut digests,
    )?;
    let mut missing = Vec::new();
    let mut mismatches = Vec::new();
    for digest in &digests {
        match store.read_blob_bytes(digest) {
            Ok(Some(bytes)) => {
                let actual = hash::digest_bytes(&bytes);
                if &actual != digest {
                    mismatches.push(json!({"expected": digest, "actual": actual}));
                }
            }
            Ok(None) => missing.push(json!({"digest": digest, "reason": "object missing from CAS"})),
            Err(error) => missing.push(json!({"digest": digest, "reason": error.to_string()})),
        }
    }
    Ok(json!({
        "checked_objects": digests.len(),
        "missing": missing,
        "hash_mismatches": mismatches,
        "database_problems": problems,
        "store_root": store.root().to_string_lossy(),
    }))
}

fn collect_column(
    store: &Store,
    sql: &str,
    project_id: &str,
    digests: &mut BTreeSet<String>,
) -> Result<()> {
    let mut statement = store.conn.prepare(sql)?;
    let rows = statement.query_map((project_id,), |row| row.get::<_, String>(0))?;
    for row in rows {
        let digest = row?;
        if digest.starts_with("sha256:") {
            digests.insert(digest);
        }
    }
    Ok(())
}

fn collect_json_column(
    store: &Store,
    sql: &str,
    project_id: &str,
    digests: &mut BTreeSet<String>,
) -> Result<()> {
    let mut statement = store.conn.prepare(sql)?;
    let rows = statement.query_map((project_id,), |row| row.get::<_, String>(0))?;
    for row in rows {
        let value: Value = match serde_json::from_str(&row?) {
            Ok(value) => value,
            Err(_) => continue,
        };
        collect_digest_values(&value, digests);
    }
    Ok(())
}

fn collect_changeset_artifacts(
    store: &Store,
    project_id: &str,
    digests: &mut BTreeSet<String>,
) -> Result<()> {
    let mut statement = store
        .conn
        .prepare("SELECT payload_json FROM changesets WHERE project_id=?1")?;
    let rows = statement.query_map((project_id,), |row| row.get::<_, String>(0))?;
    for row in rows {
        let value: Value = match serde_json::from_str(&row?) {
            Ok(value) => value,
            Err(_) => continue,
        };
        for key in ["prepared_artifact", "patch_artifact", "prompt_artifact", "toolchain_digest"] {
            if let Some(digest) = value.get(key).and_then(Value::as_str) {
                if digest.starts_with("sha256:") {
                    digests.insert(digest.to_string());
                }
            }
        }
    }
    Ok(())
}

fn collect_digest_values(value: &Value, digests: &mut BTreeSet<String>) {
    match value {
        Value::String(text) if text.starts_with("sha256:") => {
            digests.insert(text.clone());
        }
        Value::Array(items) => {
            for item in items {
                collect_digest_values(item, digests);
            }
        }
        Value::Object(map) => {
            for item in map.values() {
                collect_digest_values(item, digests);
            }
        }
        _ => {}
    }
}

pub fn get_project(store: &Store, project_id: &str) -> Result<Option<Value>> {
    let row = store
        .conn
        .query_row(
            "SELECT project_id, mode, revision, current_snapshot_id, state, config_json
             FROM projects WHERE project_id=?1",
            (project_id,),
            |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, i64>(2)?,
                    row.get::<_, Option<String>>(3)?,
                    row.get::<_, String>(4)?,
                    row.get::<_, Option<String>>(5)?,
                ))
            },
        )
        .ok();
    Ok(row.map(
        |(id, mode, revision, current_snapshot_id, state, config_json)| {
            json!({
                "id": id,
                "mode": mode,
                "revision": revision.max(1),
                "current_snapshot_id": current_snapshot_id,
                "config": config_json
                    .and_then(|text| serde_json::from_str::<Value>(&text).ok())
                    .unwrap_or_else(|| json!({})),
                "state": state,
                "workspace_security": {
                    "externally_writable": true,
                    "isolation": "none",
                    "not_a_sandbox": true,
                    "reason": "the analyzed workspace is the user's directory; other local agents with shell access can edit it",
                    "detection": "content-addressed snapshots plus read_set/preflight hash checks mark stale changesets and refuse old patches",
                },
            })
        },
    ))
}

pub fn list_roots(store: &Store, project_id: &str) -> Result<Vec<RootRecord>> {
    let mut statement = store.conn.prepare(
        "SELECT root_path, active FROM project_roots WHERE project_id=?1 ORDER BY first_seen, root_path",
    )?;
    let rows = statement.query_map((project_id,), |row| {
        Ok(RootRecord {
            root_path: row.get(0)?,
            active: row.get::<_, i64>(1)? == 1,
        })
    })?;
    Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
}

pub fn list_sessions(store: &Store, project_id: &str) -> Result<Vec<SessionRecord>> {
    let mut statement = store.conn.prepare(
        "SELECT session_id, mode, state, workspace_root, state_root FROM sessions
         WHERE project_id=?1 ORDER BY created_at, rowid",
    )?;
    let rows = statement.query_map((project_id,), |row| {
        Ok(SessionRecord {
            session_id: row.get(0)?,
            mode: row.get(1)?,
            state: row.get(2)?,
            workspace_root: row.get(3)?,
            state_root: row.get(4)?,
        })
    })?;
    Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
}

/// Startup recovery: sessions left open by a dead process become explicit
/// `interrupted` records; their project moves to `recovering`. Nothing is
/// deleted and no decision is made on the user's behalf.
pub fn recover_interrupted_sessions(store: &mut Store) -> Result<u64> {
    let tx = store.conn.transaction()?;
    let affected: i64 = tx.query_row(
        "SELECT COUNT(*) FROM sessions WHERE state='open'",
        [],
        |row| row.get(0),
    )?;
    tx.execute(
        "UPDATE sessions SET state='interrupted', closed_at=COALESCE(closed_at, ?1)
         WHERE state='open'",
        (crate::store::now_unix(),),
    )?;
    tx.execute(
        "UPDATE projects SET state='recovering'
         WHERE state='open' AND project_id IN (SELECT project_id FROM sessions WHERE state='interrupted')",
        [],
    )?;
    tx.commit()?;
    Ok(affected.max(0) as u64)
}

fn canonical_dir(path: &Path) -> Result<PathBuf> {
    let canonical = std::fs::canonicalize(path).map_err(|err| {
        CoreError::not_found(format!("path {} is not accessible: {err}", path.display()))
    })?;
    if !canonical.is_dir() {
        return Err(CoreError::invalid(format!(
            "path {} is not a directory",
            canonical.display()
        )));
    }
    Ok(canonical)
}

fn path_text(path: &Path) -> String {
    path.to_string_lossy().replace('\\', "/")
}

/// Case-insensitive on Windows, component-aware on every platform.
fn inside(child: &Path, parent: &Path) -> bool {
    let mut child = path_text(child).to_lowercase();
    let mut parent = path_text(parent).to_lowercase();
    while child.ends_with('/') {
        child.pop();
    }
    while parent.ends_with('/') {
        parent.pop();
    }
    child == parent || child.starts_with(&format!("{parent}/"))
}

fn short_hash(text: &str) -> String {
    let digest = hash::digest_bytes(text.as_bytes());
    digest
        .trim_start_matches("sha256:")
        .chars()
        .take(32)
        .collect()
}
