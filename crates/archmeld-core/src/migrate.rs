// Copyright 2026 ARCHMELD contributors
// SPDX-License-Identifier: Apache-2.0

//! Cross-ledger migration (B027).
//!
//! The supported protocol copies one complete ledger (SQLite database plus the
//! content-addressed object store) into a new, empty state directory. It
//! verifies the source and the copy before writing a receipt; merging two
//! populated ledgers is refused instead of approximated.
//!
//! Source and target never overlap. The source is opened read-only and is left
//! untouched. Exports, verification sandboxes and other transient state are
//! not copied.

use crate::error::{CoreError, Result};
use crate::hash;
use crate::store::validate;
use rusqlite::{Connection, OpenFlags};
use serde_json::{json, Value};
use std::fs;
use std::path::{Path, PathBuf};

const RECEIPT: &str = "migration.json";

pub fn migrate_ledger(source: &Path, target: &Path, reason: &str) -> Result<Value> {
    validate::require_non_empty("migration reason", reason)?;
    let source = fs::canonicalize(source)
        .map_err(|error| CoreError::invalid(format!("source ledger: {error}")))?;
    let source_db = source.join("state.sqlite3");
    if !source_db.is_file() {
        return Err(CoreError::invalid(format!(
            "source {} has no state.sqlite3",
            source.display()
        )));
    }
    let target_absolute = absolute(target)?;
    ensure_outside("target", &target_absolute, &source)?;
    ensure_outside("source", &source, &target_absolute)?;
    if target_absolute.exists() {
        return Err(CoreError::conflict(format!(
            "migration target {} already exists; merging into a populated or existing ledger is not supported",
            target_absolute.display()
        )));
    }
    let parent = target_absolute
        .parent()
        .ok_or_else(|| CoreError::invalid("migration target has no parent"))?;
    fs::create_dir_all(parent)?;

    let outcome = copy_ledger(&source, &source_db, &target_absolute, reason);
    if outcome.is_err() {
        let _ = fs::remove_dir_all(&target_absolute);
    }
    outcome
}

fn absolute(path: &Path) -> Result<PathBuf> {
    let absolute = std::path::absolute(path)?;
    if let Ok(canonical) = fs::canonicalize(&absolute) {
        return Ok(canonical);
    }
    // A target does not exist yet; canonicalize the nearest existing ancestor
    // and keep the remaining components so overlap checks compare the same
    // (possibly verbatim `\\?\`) form.
    let mut suffix: Vec<std::ffi::OsString> = Vec::new();
    let mut ancestor = absolute.as_path();
    while let Some(parent) = ancestor.parent() {
        if let Some(name) = ancestor.file_name() {
            suffix.push(name.to_os_string());
        }
        if let Ok(canonical) = fs::canonicalize(parent) {
            let mut resolved = canonical;
            for part in suffix.iter().rev() {
                resolved.push(part);
            }
            return Ok(resolved);
        }
        ancestor = parent;
    }
    Ok(absolute)
}

fn ensure_outside(label: &str, candidate: &Path, other: &Path) -> Result<()> {
    let normalize = |path: &Path| {
        path.to_string_lossy()
            .replace('\\', "/")
            .trim_end_matches('/')
            .to_lowercase()
    };
    let candidate_text = normalize(candidate);
    let other_text = normalize(other);
    if candidate_text == other_text
        || candidate_text.starts_with(&format!("{other_text}/"))
        || other_text.starts_with(&format!("{candidate_text}/"))
    {
        return Err(CoreError::invalid(format!(
            "{label} {} overlaps the other ledger path",
            candidate.display()
        )));
    }
    Ok(())
}

fn copy_ledger(source: &Path, source_db: &Path, target: &Path, reason: &str) -> Result<Value> {
    let source_conn = Connection::open_with_flags(
        source_db,
        OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX,
    )?;
    let source_integrity: String =
        source_conn.query_row("PRAGMA integrity_check", [], |row| row.get(0))?;
    if source_integrity != "ok" {
        return Err(CoreError::Storage(format!(
            "source sqlite integrity_check is {source_integrity}"
        )));
    }
    let source_version = schema_version(&source_conn)?;
    let source_counts = table_counts(&source_conn)?;
    let projects: Vec<String> = {
        let mut statement = source_conn.prepare("SELECT project_id FROM projects ORDER BY project_id")?;
        let rows = statement.query_map([], |row| row.get::<_, String>(0))?;
        rows.collect::<rusqlite::Result<Vec<_>>>()?
    };

    fs::create_dir_all(target.join("objects").join("sha256"))?;
    fs::create_dir_all(target.join("objects").join("tmp"))?;
    let target_db = target.join("state.sqlite3");
    let escaped = target_db.to_string_lossy().replace('\'', "''");
    source_conn.execute_batch(&format!("VACUUM INTO '{escaped}'"))?;
    drop(source_conn);

    let target_conn = Connection::open_with_flags(
        &target_db,
        OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX,
    )?;
    let target_integrity: String =
        target_conn.query_row("PRAGMA integrity_check", [], |row| row.get(0))?;
    if target_integrity != "ok" {
        return Err(CoreError::Storage(format!(
            "target sqlite integrity_check is {target_integrity}"
        )));
    }
    let violations: i64 = {
        let mut statement = target_conn.prepare("PRAGMA foreign_key_check")?;
        let rows = statement
            .query_map([], |_| Ok(()))?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        rows.len() as i64
    };
    if violations != 0 {
        return Err(CoreError::Storage(format!(
            "target sqlite has {violations} foreign key violations"
        )));
    }
    let target_version = schema_version(&target_conn)?;
    if target_version != source_version {
        return Err(CoreError::Storage(format!(
            "schema version changed during migration: {source_version} -> {target_version}"
        )));
    }
    let target_counts = table_counts(&target_conn)?;
    if target_counts != source_counts {
        return Err(CoreError::Storage(
            "table row counts changed during migration".to_string(),
        ));
    }
    let artifact_digests: Vec<String> = {
        let mut statement = target_conn.prepare("SELECT digest FROM artifacts ORDER BY digest")?;
        let rows = statement.query_map([], |row| row.get::<_, String>(0))?;
        rows.collect::<rusqlite::Result<Vec<_>>>()?
    };
    drop(target_conn);

    let source_hash = file_digest(source_db)?;
    let target_hash = file_digest(&target_db)?;
    let bytes_equal = source_hash == target_hash;

    let cas = copy_cas(source, target, &artifact_digests)?;
    let receipt = json!({
        "schema_version": "1.0",
        "kind": "archmeld.ledger_migration",
        "mode": "copy_entire_ledger",
        "merge": false,
        "reason": reason,
        "source": {
            "root": source.to_string_lossy(),
            "database_sha256": source_hash,
            "schema_version": source_version,
            "projects": projects,
        },
        "target": {
            "root": target.to_string_lossy(),
            "database_sha256": target_hash,
            "schema_version": target_version,
        },
        "cas": cas,
        "verification": {
            "sqlite_integrity": "ok",
            "foreign_key_violations": 0,
            "database_bytes_equal": bytes_equal,
            "logical_check": "schema version and every table row count match the source",
            "schema_versions_match": true,
            "table_counts_match": true,
            "tables_checked": source_counts.len(),
        },
        "limitations": [
            "copies the entire ledger; merging into an existing target is refused",
            "the source ledger is read-only and left untouched",
            "exports, verification sandboxes, scenario sandboxes and other transient state are not copied",
            "the receipt is written beside state.sqlite3 and is not covered by the database hash",
        ],
        "created_at": crate::store::now_unix(),
    });
    let bytes = serde_json::to_vec_pretty(&receipt)
        .map_err(|error| CoreError::Storage(format!("migration receipt: {error}")))?;
    fs::write(target.join(RECEIPT), bytes)?;
    Ok(receipt)
}

fn schema_version(connection: &Connection) -> Result<i64> {
    let version: Option<i64> = connection
        .query_row("SELECT MAX(version) FROM schema_migrations", [], |row| row.get(0))
        .unwrap_or(None);
    version.ok_or_else(|| CoreError::Storage("ledger has no schema_migrations rows".to_string()))
}

fn table_counts(connection: &Connection) -> Result<std::collections::BTreeMap<String, i64>> {
    let tables: Vec<String> = {
        let mut statement = connection.prepare(
            "SELECT name FROM sqlite_master WHERE type='table' AND name NOT LIKE 'sqlite_%' ORDER BY name",
        )?;
        let rows = statement.query_map([], |row| row.get::<_, String>(0))?;
        rows.collect::<rusqlite::Result<Vec<_>>>()?
    };
    let mut counts = std::collections::BTreeMap::new();
    for table in tables {
        let escaped = table.replace('"', "\"\"");
        let count: i64 = connection.query_row(
            &format!("SELECT COUNT(*) FROM \"{escaped}\""),
            [],
            |row| row.get(0),
        )?;
        counts.insert(table, count);
    }
    Ok(counts)
}

fn file_digest(path: &Path) -> Result<String> {
    let bytes = fs::read(path)?;
    Ok(hash::digest_bytes(&bytes))
}

fn copy_cas(source: &Path, target: &Path, referenced: &[String]) -> Result<Value> {
    let source_root = source.join("objects").join("sha256");
    let target_root = target.join("objects").join("sha256");
    let mut objects = 0u64;
    let mut bytes = 0u64;
    if source_root.is_dir() {
        for prefix in fs::read_dir(&source_root)? {
            let prefix = prefix?;
            if !prefix.file_type()?.is_dir() {
                continue;
            }
            let prefix_name = prefix.file_name().to_string_lossy().to_string();
            if prefix_name.len() != 2 {
                return Err(CoreError::Storage(format!(
                    "unexpected CAS prefix {prefix_name:?}"
                )));
            }
            for object in fs::read_dir(prefix.path())? {
                let object = object?;
                if !object.file_type()?.is_file() {
                    continue;
                }
                let name = object.file_name().to_string_lossy().to_string();
                if name.len() != 62 {
                    return Err(CoreError::Storage(format!(
                        "unexpected CAS object name {name:?}"
                    )));
                }
                let hex = format!("{prefix_name}{name}");
                let data = fs::read(object.path())?;
                if hash::digest_bytes(&data) != format!("sha256:{hex}") {
                    return Err(CoreError::Storage(format!(
                        "CAS object {prefix_name}/{name} does not match its digest"
                    )));
                }
                let destination = target_root.join(&prefix_name).join(&name);
                fs::create_dir_all(destination.parent().unwrap())?;
                fs::write(&destination, &data)?;
                if hash::digest_bytes(&fs::read(&destination)?) != format!("sha256:{hex}") {
                    return Err(CoreError::Storage(format!(
                        "copied CAS object {prefix_name}/{name} changed"
                    )));
                }
                objects += 1;
                bytes += data.len() as u64;
            }
        }
    }
    let mut missing = Vec::new();
    for digest in referenced {
        let Some(hex) = digest.strip_prefix("sha256:") else {
            return Err(CoreError::Storage(format!(
                "artifact digest {digest:?} is not a sha256 digest"
            )));
        };
        if hex.len() != 64 || !target_root.join(&hex[..2]).join(&hex[2..]).is_file() {
            missing.push(digest.clone());
        }
    }
    if !missing.is_empty() {
        return Err(CoreError::Storage(format!(
            "{} referenced CAS objects are missing: {:?}",
            missing.len(),
            &missing[..missing.len().min(5)]
        )));
    }
    Ok(json!({
        "objects": objects,
        "bytes": bytes,
        "referenced_artifacts": referenced.len(),
        "missing_referenced": 0,
    }))
}
