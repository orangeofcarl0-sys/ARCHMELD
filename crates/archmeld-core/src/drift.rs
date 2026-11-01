// Copyright 2026 ARCHMELD contributors
// SPDX-License-Identifier: Apache-2.0

//! Drift reporting, consistent backups and mark-and-sweep GC (S14).
//!
//! Drift is a report: it names stale plans/changesets/claims/architectures but
//! never rewrites history or deletes snapshots. Backups copy a consistent
//! SQLite snapshot plus reachable CAS objects. GC only removes unreferenced,
//! unpinned objects after an explicit non-dry-run call.

use crate::apply::resolve_workspace_root;
use crate::error::{CoreError, Result};
use crate::hash;
use crate::store::{validate, Store};
use rusqlite::Connection;
use serde::Deserialize;
use serde_json::{json, Value};
use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
struct DriftRequest {
    project_id: String,
    base_snapshot_id: String,
    workspace_handle: String,
    new_snapshot_id: Option<String>,
    idempotency_key: String,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
struct PinRequest {
    project_id: String,
    pin_id: String,
    kind: String,
    target: String,
}

pub fn analyze_drift(
    store: &Store,
    project_id: &str,
    base_snapshot_id: &str,
    root: &Path,
    new_snapshot_id: Option<&str>,
) -> Result<Value> {
    validate::require_id("project_id", project_id)?;
    validate::require_digest("base_snapshot_id", base_snapshot_id)?;
    store.check_snapshot_scope(project_id, base_snapshot_id)?;
    if let Some(new_snapshot_id) = new_snapshot_id {
        validate::require_digest("new_snapshot_id", new_snapshot_id)?;
        store.check_snapshot_scope(project_id, new_snapshot_id)?;
        if new_snapshot_id == base_snapshot_id {
            return Err(CoreError::invalid(
                "new_snapshot_id must differ from base_snapshot_id",
            ));
        }
    }
    let base_files = snapshot_files(store, project_id, base_snapshot_id)?;
    let disk_files = walk_disk(root)?;
    let mut changed = Vec::new();
    let mut deleted = Vec::new();
    for (path, digest) in &base_files {
        match disk_files.get(path) {
            Some(actual) if actual == digest => {}
            Some(actual) => changed.push(json!({
                "path": path,
                "state": "changed",
                "base_digest": digest,
                "current_digest": actual,
            })),
            None => deleted.push(json!({"path": path, "state": "deleted", "base_digest": digest})),
        }
    }
    let mut added = Vec::new();
    for (path, digest) in &disk_files {
        if !base_files.contains_key(path) {
            added.push(json!({"path": path, "state": "added", "current_digest": digest}));
        }
    }
    changed.sort_by(|left, right| left["path"].as_str().cmp(&right["path"].as_str()));
    deleted.sort_by(|left, right| left["path"].as_str().cmp(&right["path"].as_str()));
    added.sort_by(|left, right| left["path"].as_str().cmp(&right["path"].as_str()));
    let changed_paths: BTreeSet<String> = changed
        .iter()
        .chain(deleted.iter())
        .filter_map(|item| item["path"].as_str().map(str::to_string))
        .collect();

    let mut affected_entities = Vec::new();
    for entity_id in store.entity_ids(project_id, base_snapshot_id)? {
        let Some(entity) = store.get_entity(project_id, base_snapshot_id, &entity_id)? else {
            continue;
        };
        let hit = entity["anchors"]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(|anchor| anchor["path"].as_str())
            .any(|path| changed_paths.contains(path));
        if hit {
            affected_entities.push(json!({
                "entity_id": entity_id,
                "qualified_name": entity["qualified_name"],
                "kind": entity["kind"],
            }));
        }
    }
    let affected_ids: BTreeSet<String> = affected_entities
        .iter()
        .filter_map(|item| item["entity_id"].as_str().map(str::to_string))
        .collect();
    let mut affected_relations = Vec::new();
    for relation in store.relation_payloads(project_id, base_snapshot_id)? {
        let source = relation["source_id"].as_str().unwrap_or("");
        let target = relation["target_id"].as_str().unwrap_or("");
        if affected_ids.contains(source) || affected_ids.contains(target) {
            affected_relations.push(json!({
                "relation_id": relation["id"],
                "source_id": source,
                "target_id": target,
                "kind": relation["kind"],
                "resolution": relation["resolution"],
            }));
        }
    }

    let mut affected_claims = Vec::new();
    for claim in latest_rows(store, "claims", "claim_id", project_id)? {
        if claim["snapshot_id"].as_str() != Some(base_snapshot_id) {
            continue;
        }
        let scope = claim["scope"].as_str().unwrap_or("");
        let hit = scope.is_empty() || changed_paths.iter().any(|path| scope.contains(path));
        if hit {
            affected_claims.push(json!({
                "claim_id": claim["id"],
                "status": claim["status"],
                "scope": claim["scope"],
                "reason": "scope overlaps changed paths",
            }));
        }
    }
    let mut affected_architectures = Vec::new();
    for architecture in latest_rows(store, "architectures", "architecture_id", project_id)? {
        if architecture["snapshot_id"].as_str() != Some(base_snapshot_id) {
            continue;
        }
        let hit = architecture["components"]
            .as_array()
            .into_iter()
            .flatten()
            .flat_map(|component| {
                component["member_entity_ids"]
                    .as_array()
                    .into_iter()
                    .flatten()
                    .filter_map(|id| id.as_str())
            })
            .any(|id| affected_ids.contains(id));
        if hit {
            affected_architectures.push(json!({
                "architecture_id": architecture["id"],
                "status": architecture["status"],
                "reason": "member entities changed",
            }));
        }
    }
    let stale_plans = plans_on_snapshot(store, project_id, base_snapshot_id)?;
    let stale_changesets = changesets_on_snapshot(store, project_id, base_snapshot_id)?;
    let dirty = !(changed.is_empty() && deleted.is_empty() && added.is_empty());
    Ok(json!({
        "schema_version": "1.0",
        "project_id": project_id,
        "base_snapshot_id": base_snapshot_id,
        "new_snapshot_id": new_snapshot_id,
        "state": if dirty { "changed" } else { "clean" },
        "changed": changed,
        "deleted": deleted,
        "added": added,
        "affected_entities": affected_entities,
        "affected_relations": affected_relations,
        "affected_claims": affected_claims,
        "affected_architectures": affected_architectures,
        "stale_plans": stale_plans,
        "stale_changesets": stale_changesets,
        "needs_review": dirty
            && !(affected_entities.is_empty()
                && affected_relations.is_empty()
                && affected_claims.is_empty()
                && affected_architectures.is_empty()
                && stale_plans.is_empty()
                && stale_changesets.is_empty()),
        "source_mutation": "none",
        "history_mutation": "none",
        "coverage": {
            "state": "complete",
            "scope": "listed base-snapshot files versus the current workspace",
            "limitations": ["files added outside the captured snapshot policy are listed as added"],
        },
    }))
}

pub fn record_drift(store: &mut Store, raw: &Value) -> Result<Value> {
    let request: DriftRequest = serde_json::from_value(raw.clone())
        .map_err(|err| CoreError::invalid(format!("drift request: {err}")))?;
    validate::require_id("project_id", &request.project_id)?;
    validate::require_id("workspace_handle", &request.workspace_handle)?;
    validate::require_id("idempotency_key", &request.idempotency_key)?;
    let digest = hash::digest_value(raw)
        .map_err(|err| CoreError::invalid(format!("drift request: {err}")))?;
    if let Some((recorded, _, result)) =
        store.idempotency_get(&request.project_id, "recordDrift", &request.idempotency_key)?
    {
        if recorded != digest {
            return Err(CoreError::conflict(
                "scope_conflict: idempotency_key was used with a different drift request",
            ));
        }
        return serde_json::from_str(&result)
            .map_err(|err| CoreError::Storage(format!("stored drift corrupt: {err}")));
    }
    let root = resolve_workspace_root(store, &request.workspace_handle, &request.project_id)?;
    let report = analyze_drift(
        store,
        &request.project_id,
        &request.base_snapshot_id,
        &root,
        request.new_snapshot_id.as_deref(),
    )?;
    let summary_json = hash::canonical_json(&report)
        .map_err(|err| CoreError::invalid(format!("drift report: {err}")))?;
    let report_id = format!(
        "drift.{}",
        short_hash(&format!(
            "{}:{}:{}:{}",
            request.project_id,
            request.base_snapshot_id,
            request.new_snapshot_id.as_deref().unwrap_or(""),
            report["state"]
        ))
    );
    let stored = json!({
        "report_id": report_id,
        "project_id": request.project_id,
        "base_snapshot_id": request.base_snapshot_id,
        "new_snapshot_id": request.new_snapshot_id,
        "summary": report,
        "created_at": crate::store::now_unix(),
    });
    store.conn.execute(
        "INSERT INTO drift_reports
         (project_id,report_id,base_snapshot_id,new_snapshot_id,summary_json,created_at)
         VALUES (?1,?2,?3,?4,?5,?6)
         ON CONFLICT(project_id,report_id) DO UPDATE SET summary_json=excluded.summary_json",
        (
            &stored["project_id"].as_str().unwrap_or(""),
            &report_id,
            &stored["base_snapshot_id"].as_str().unwrap_or(""),
            stored["new_snapshot_id"].as_str(),
            &summary_json,
            crate::store::now_unix(),
        ),
    )?;
    let result_json = serde_json::to_string(&stored)
        .map_err(|err| CoreError::Storage(format!("drift encode: {err}")))?;
    store.conn.execute(
        "INSERT INTO idempotency
         (project_id,operation,idempotency_key,request_digest,status_code,result_json,created_at)
         VALUES (?1,'recordDrift',?2,?3,200,?4,?5)",
        (
            &request.project_id,
            &request.idempotency_key,
            &digest,
            result_json,
            crate::store::now_unix(),
        ),
    )?;
    Ok(stored)
}

pub fn list_drift_reports(store: &Store, project_id: &str) -> Result<Vec<Value>> {
    validate::require_id("project_id", project_id)?;
    let mut statement = store.conn.prepare(
        "SELECT report_id,summary_json FROM drift_reports
         WHERE project_id=?1 ORDER BY created_at DESC, report_id",
    )?;
    let rows = statement.query_map((project_id,), |row| {
        Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
    })?;
    let mut reports = Vec::new();
    for row in rows {
        let (report_id, summary) = row?;
        let summary: Value = serde_json::from_str(&summary)
            .map_err(|err| CoreError::Storage(format!("drift summary corrupt: {err}")))?;
        reports.push(json!({"report_id": report_id, "summary": summary}));
    }
    Ok(reports)
}

/// Consistent SQLite snapshot plus every CAS object. The backup directory can
/// be opened with `Store::open` to inspect the historical decision state.
pub fn create_backup(store: &mut Store, project_id: &str, destination: &Path) -> Result<Value> {
    validate::require_id("project_id", project_id)?;
    store.ensure_project_exists(project_id)?;
    if destination.exists() && fs::read_dir(destination)?.next().is_some() {
        return Err(CoreError::conflict(
            "backup destination already exists and is not empty",
        ));
    }
    fs::create_dir_all(destination)?;
    let db_path = destination.join("state.sqlite3");
    if db_path.exists() {
        return Err(CoreError::conflict("backup database already exists"));
    }
    let escaped = db_path.to_string_lossy().replace('\'', "''");
    store
        .conn
        .execute_batch(&format!("VACUUM INTO '{escaped}'"))?;
    let objects_root = destination.join("objects");
    fs::create_dir_all(objects_root.join("sha256"))?;
    fs::create_dir_all(objects_root.join("tmp"))?;
    let mut objects = Vec::new();
    let source_objects = store.root().join("objects").join("sha256");
    copy_objects(&source_objects, &objects_root.join("sha256"), &mut objects)?;
    let db_bytes = fs::read(&db_path)?;
    let db_digest = hash::digest_bytes(&db_bytes);
    let manifest = json!({
        "schema_version": "1.0",
        "project_id": project_id,
        "database": {"path": "state.sqlite3", "digest": db_digest, "size": db_bytes.len()},
        "objects": objects,
        "object_count": objects.len(),
        "consistency": "VACUUM INTO snapshot of the live transaction log plus copied CAS objects",
        "contains_source": true,
        "created_at": crate::store::now_unix(),
    });
    let manifest_bytes = hash::canonical_json(&manifest)
        .map_err(|err| CoreError::invalid(format!("backup manifest: {err}")))?;
    fs::write(destination.join("backup.json"), &manifest_bytes)?;
    let manifest_digest = hash::digest_bytes(manifest_bytes.as_bytes());
    let backup_id = format!(
        "backup.{}",
        short_hash(&format!("{}:{}:{}", project_id, db_digest, manifest_digest))
    );
    store.conn.execute(
        "INSERT INTO backup_runs
         (project_id,backup_id,directory,db_digest,manifest_digest,object_count,created_at)
         VALUES (?1,?2,?3,?4,?5,?6,?7)",
        (
            project_id,
            &backup_id,
            destination.to_string_lossy(),
            &db_digest,
            &manifest_digest,
            objects.len() as i64,
            crate::store::now_unix(),
        ),
    )?;
    Ok(json!({
        "backup_id": backup_id,
        "directory": destination.to_string_lossy(),
        "database_digest": db_digest,
        "manifest_digest": manifest_digest,
        "object_count": objects.len(),
        "verified": verify_backup(destination).is_ok(),
    }))
}

pub fn pin(store: &mut Store, raw: &Value) -> Result<Value> {
    let request: PinRequest = serde_json::from_value(raw.clone())
        .map_err(|err| CoreError::invalid(format!("pin request: {err}")))?;
    validate::require_id("project_id", &request.project_id)?;
    validate::require_id("pin_id", &request.pin_id)?;
    validate::require_non_empty("target", &request.target)?;
    if !matches!(request.kind.as_str(), "object" | "snapshot" | "decision") {
        return Err(CoreError::invalid(
            "pin kind must be object, snapshot or decision",
        ));
    }
    store.ensure_project_exists(&request.project_id)?;
    store.conn.execute(
        "INSERT INTO retention_pins (project_id,pin_id,kind,target,created_at)
         VALUES (?1,?2,?3,?4,?5)
         ON CONFLICT(project_id,pin_id) DO UPDATE SET kind=excluded.kind,target=excluded.target",
        (
            &request.project_id,
            &request.pin_id,
            &request.kind,
            &request.target,
            crate::store::now_unix(),
        ),
    )?;
    Ok(json!({"pin_id": request.pin_id, "kind": request.kind, "target": request.target}))
}

pub fn unpin(store: &mut Store, project_id: &str, pin_id: &str) -> Result<Value> {
    validate::require_id("project_id", project_id)?;
    validate::require_id("pin_id", pin_id)?;
    let deleted = store.conn.execute(
        "DELETE FROM retention_pins WHERE project_id=?1 AND pin_id=?2",
        (project_id, pin_id),
    )?;
    Ok(json!({"pin_id": pin_id, "removed": deleted == 1}))
}

/// Mark-and-sweep over the global CAS. `dry_run=true` only lists candidates.
pub fn gc(store: &mut Store, dry_run: bool) -> Result<Value> {
    let referenced = referenced_digests(store)?;
    let pinned: BTreeSet<String> = {
        let mut statement = store
            .conn
            .prepare("SELECT target FROM retention_pins WHERE kind='object'")?;
        let rows = statement.query_map([], |row| row.get::<_, String>(0))?;
        rows.collect::<rusqlite::Result<BTreeSet<String>>>()?
    };
    let mut candidates = Vec::new();
    let objects = store.root().join("objects").join("sha256");
    if objects.is_dir() {
        for prefix in fs::read_dir(&objects)? {
            let prefix = prefix?;
            if !prefix.file_type()?.is_dir() {
                continue;
            }
            for object in fs::read_dir(prefix.path())? {
                let object = object?;
                if !object.file_type()?.is_file() {
                    continue;
                }
                let name = object.file_name().to_string_lossy().to_string();
                let prefix_name = prefix.file_name().to_string_lossy().to_string();
                let digest = format!("sha256:{prefix_name}{name}");
                if referenced.contains(&digest) || pinned.contains(&digest) {
                    continue;
                }
                candidates.push(json!({
                    "digest": digest,
                    "size": object.metadata()?.len(),
                    "path": object.path().to_string_lossy(),
                }));
            }
        }
    }
    candidates.sort_by(|left, right| left["digest"].as_str().cmp(&right["digest"].as_str()));
    if !dry_run {
        for candidate in &candidates {
            let digest = candidate["digest"].as_str().unwrap_or("");
            let hex = digest.strip_prefix("sha256:").unwrap_or(digest);
            if hex.len() >= 3 {
                let path = objects.join(&hex[..2]).join(&hex[2..]);
                let _ = fs::remove_file(path);
            }
            store
                .conn
                .execute("DELETE FROM artifacts WHERE digest=?1", (digest,))?;
        }
    }
    Ok(json!({
        "dry_run": dry_run,
        "referenced_objects": referenced.len(),
        "pinned_objects": pinned.len(),
        "candidate_count": candidates.len(),
        "candidates": candidates,
        "deleted_count": if dry_run { 0 } else { candidates.len() },
        "owned_root": store.root().to_string_lossy(),
        "whole_tree_reset": false,
    }))
}

fn referenced_digests(store: &Store) -> Result<BTreeSet<String>> {
    let mut set = BTreeSet::new();
    collect_column(store, "SELECT manifest_artifact FROM snapshots", &mut set)?;
    collect_column(store, "SELECT blob_digest FROM files", &mut set)?;
    collect_column(
        store,
        "SELECT artifact_digest FROM evidence WHERE artifact_digest IS NOT NULL",
        &mut set,
    )?;
    collect_column(
        store,
        "SELECT artifact_digest FROM runtime_runs WHERE artifact_digest IS NOT NULL",
        &mut set,
    )?;
    collect_column(
        store,
        "SELECT stdout_digest FROM scenario_runs WHERE stdout_digest IS NOT NULL",
        &mut set,
    )?;
    collect_column(
        store,
        "SELECT stderr_digest FROM scenario_runs WHERE stderr_digest IS NOT NULL",
        &mut set,
    )?;
    collect_column(
        store,
        "SELECT stdout_digest FROM check_results WHERE stdout_digest IS NOT NULL",
        &mut set,
    )?;
    collect_column(
        store,
        "SELECT stderr_digest FROM check_results WHERE stderr_digest IS NOT NULL",
        &mut set,
    )?;
    collect_json_digests(store, "SELECT artifacts_json FROM check_results", &mut set)?;
    collect_json_digests(store, "SELECT payload_json FROM changesets", &mut set)?;
    collect_json_digests(store, "SELECT artifact_digests_json FROM jobs", &mut set)?;
    collect_json_digests(store, "SELECT result_json FROM idempotency", &mut set)?;
    Ok(set)
}

fn collect_column(store: &Store, sql: &str, set: &mut BTreeSet<String>) -> Result<()> {
    let mut statement = store.conn.prepare(sql)?;
    let rows = statement.query_map([], |row| row.get::<_, String>(0))?;
    for row in rows {
        let value = row?;
        if value.starts_with("sha256:") {
            set.insert(value);
        }
    }
    Ok(())
}

fn collect_json_digests(store: &Store, sql: &str, set: &mut BTreeSet<String>) -> Result<()> {
    let mut statement = store.conn.prepare(sql)?;
    let rows = statement.query_map([], |row| row.get::<_, String>(0))?;
    for row in rows {
        let value: Value = match serde_json::from_str(&row?) {
            Ok(value) => value,
            Err(_) => continue,
        };
        collect_digest_values(&value, set);
    }
    Ok(())
}

fn collect_digest_values(value: &Value, set: &mut BTreeSet<String>) {
    match value {
        Value::String(text) if text.starts_with("sha256:") => {
            set.insert(text.clone());
        }
        Value::Array(items) => {
            for item in items {
                collect_digest_values(item, set);
            }
        }
        Value::Object(object) => {
            for item in object.values() {
                collect_digest_values(item, set);
            }
        }
        _ => {}
    }
}

fn copy_objects(source: &Path, destination: &Path, manifest: &mut Vec<Value>) -> Result<()> {
    if !source.is_dir() {
        return Ok(());
    }
    for prefix in fs::read_dir(source)? {
        let prefix = prefix?;
        if !prefix.file_type()?.is_dir() {
            continue;
        }
        let prefix_name = prefix.file_name().to_string_lossy().to_string();
        let target_prefix = destination.join(&prefix_name);
        fs::create_dir_all(&target_prefix)?;
        for object in fs::read_dir(prefix.path())? {
            let object = object?;
            if !object.file_type()?.is_file() {
                continue;
            }
            let name = object.file_name().to_string_lossy().to_string();
            let bytes = fs::read(object.path())?;
            let digest = format!("sha256:{prefix_name}{name}");
            let actual = hash::digest_bytes(&bytes);
            if actual != digest {
                return Err(CoreError::Storage(format!(
                    "CAS object {digest} does not hash to its path"
                )));
            }
            fs::write(target_prefix.join(&name), &bytes)?;
            manifest.push(json!({"digest": digest, "size": bytes.len()}));
        }
    }
    manifest.sort_by(|left, right| left["digest"].as_str().cmp(&right["digest"].as_str()));
    Ok(())
}

fn verify_backup(destination: &Path) -> Result<()> {
    let manifest_text = fs::read_to_string(destination.join("backup.json"))?;
    let manifest: Value = serde_json::from_str(&manifest_text)
        .map_err(|err| CoreError::Storage(format!("backup manifest corrupt: {err}")))?;
    let db_bytes = fs::read(destination.join("state.sqlite3"))?;
    if hash::digest_bytes(&db_bytes) != manifest["database"]["digest"].as_str().unwrap_or("") {
        return Err(CoreError::Storage("backup database digest mismatch".into()));
    }
    let connection = Connection::open(destination.join("state.sqlite3"))?;
    let integrity: String = connection.query_row("PRAGMA integrity_check", [], |row| row.get(0))?;
    if integrity != "ok" {
        return Err(CoreError::Storage(format!(
            "backup database integrity: {integrity}"
        )));
    }
    for object in manifest["objects"].as_array().into_iter().flatten() {
        let digest = object["digest"].as_str().unwrap_or("");
        let hex = digest.strip_prefix("sha256:").unwrap_or(digest);
        let path = destination
            .join("objects")
            .join("sha256")
            .join(&hex[..2])
            .join(&hex[2..]);
        let bytes = fs::read(&path)?;
        if hash::digest_bytes(&bytes) != digest {
            return Err(CoreError::Storage(format!(
                "backup object {digest} is corrupt"
            )));
        }
    }
    Ok(())
}

fn snapshot_files(
    store: &Store,
    project_id: &str,
    snapshot_id: &str,
) -> Result<BTreeMap<String, String>> {
    let mut statement = store.conn.prepare(
        "SELECT path,blob_digest FROM files WHERE project_id=?1 AND snapshot_id=?2 ORDER BY path",
    )?;
    let rows = statement.query_map((project_id, snapshot_id), |row| {
        Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
    })?;
    Ok(rows.collect::<rusqlite::Result<BTreeMap<_, _>>>()?)
}

fn walk_disk(root: &Path) -> Result<BTreeMap<String, String>> {
    let excluded = [".git", "node_modules", "target", ".venv", "__pycache__"];
    let mut files = BTreeMap::new();
    fn walk(
        root: &Path,
        dir: &Path,
        excluded: &[&str],
        files: &mut BTreeMap<String, String>,
    ) -> Result<()> {
        for entry in fs::read_dir(dir)? {
            let entry = entry?;
            let name = entry.file_name().to_string_lossy().to_string();
            if excluded.contains(&name.as_str()) {
                continue;
            }
            let path = entry.path();
            let metadata = fs::symlink_metadata(&path)?;
            if metadata.is_dir() {
                walk(root, &path, excluded, files)?;
            } else if metadata.is_file() {
                let relative = path
                    .strip_prefix(root)
                    .map_err(|_| CoreError::Storage("drift path escaped root".into()))?
                    .to_string_lossy()
                    .replace('\\', "/");
                files.insert(relative, hash::digest_bytes(&fs::read(&path)?));
            }
        }
        Ok(())
    }
    walk(root, root, &excluded, &mut files)?;
    Ok(files)
}

fn latest_rows(
    store: &Store,
    table: &str,
    id_column: &str,
    project_id: &str,
) -> Result<Vec<Value>> {
    let sql = format!(
        "SELECT payload_json FROM {table} t
         WHERE project_id=?1 AND revision=(SELECT MAX(revision) FROM {table} u
             WHERE u.project_id=t.project_id AND u.{id_column}=t.{id_column})
         ORDER BY {id_column}"
    );
    let mut statement = store.conn.prepare(&sql)?;
    let rows = statement.query_map((project_id,), |row| row.get::<_, String>(0))?;
    let mut values = Vec::new();
    for row in rows {
        values.push(
            serde_json::from_str(&row?)
                .map_err(|err| CoreError::Storage(format!("{table} payload corrupt: {err}")))?,
        );
    }
    Ok(values)
}

fn plans_on_snapshot(store: &Store, project_id: &str, snapshot_id: &str) -> Result<Vec<Value>> {
    let mut statement = store.conn.prepare(
        "SELECT plan_id,MAX(revision) FROM plans WHERE project_id=?1
         GROUP BY plan_id ORDER BY plan_id",
    )?;
    let rows = statement.query_map((project_id,), |row| {
        Ok((row.get::<_, String>(0)?, row.get::<_, i64>(1)?))
    })?;
    let mut result = Vec::new();
    for row in rows {
        let (plan_id, revision) = row?;
        let (payload, base, status): (String, String, String) = store.conn.query_row(
            "SELECT payload_json,base_snapshot_id,status FROM plans
             WHERE project_id=?1 AND plan_id=?2 AND revision=?3",
            (project_id, &plan_id, revision),
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
        )?;
        if base == snapshot_id && status == "approved" {
            let payload: Value = serde_json::from_str(&payload)
                .map_err(|err| CoreError::Storage(format!("plan corrupt: {err}")))?;
            result.push(json!({
                "plan_id": plan_id,
                "revision": revision,
                "contract_digest": payload["contract_digest"],
                "status": "needs_review",
                "reason": "base snapshot drifted",
            }));
        }
    }
    Ok(result)
}

fn changesets_on_snapshot(
    store: &Store,
    project_id: &str,
    snapshot_id: &str,
) -> Result<Vec<Value>> {
    let mut statement = store.conn.prepare(
        "SELECT changeset_id,MAX(revision) FROM changesets WHERE project_id=?1
         GROUP BY changeset_id ORDER BY changeset_id",
    )?;
    let rows = statement.query_map((project_id,), |row| {
        Ok((row.get::<_, String>(0)?, row.get::<_, i64>(1)?))
    })?;
    let mut result = Vec::new();
    for row in rows {
        let (changeset_id, revision) = row?;
        let (base, state): (String, String) = store.conn.query_row(
            "SELECT base_snapshot_id,state FROM changesets
             WHERE project_id=?1 AND changeset_id=?2 AND revision=?3",
            (project_id, &changeset_id, revision),
            |row| Ok((row.get(0)?, row.get(1)?)),
        )?;
        if base == snapshot_id
            && matches!(
                state.as_str(),
                "prepared" | "applied" | "verifying" | "accepted"
            )
        {
            result.push(json!({
                "changeset_id": changeset_id,
                "revision": revision,
                "state": "stale",
                "previous_state": state,
                "reason": "base snapshot drifted",
            }));
        }
    }
    Ok(result)
}

fn short_hash(identity: &str) -> String {
    let digest = hash::digest_bytes(identity.as_bytes());
    let hex = digest.strip_prefix("sha256:").unwrap_or(&digest);
    hex[..32].to_string()
}
