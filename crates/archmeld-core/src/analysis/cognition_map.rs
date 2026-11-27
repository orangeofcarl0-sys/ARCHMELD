// Copyright 2026 ARCHMELD contributors
// SPDX-License-Identifier: Apache-2.0

//! Read-only E1 support for an evidence-bound repository cognition map.
//!
//! A map is a committed text file with one file-level Entry per managed file:
//!
//! ```text
//! #ARCHMELD-MAP: 1
//! #snapshot: sha256:...
//! #scope: code
//! ===src/===
//! a.py[L1D1C5M]: F:Does one thing. | R:file:src/b.py | A:- | S:-
//! ```
//!
//! `F`/`A`/`S` are model-authored prose. `R` is a list of logical references
//! (`file:` or `entity:`; `relation:` is not implemented because E3 was not
//! promoted after the E4a no-go). This module only parses and validates; it
//! never writes a map, never runs a repository command and never promotes a
//! candidate edge.

use crate::error::{CoreError, Result};
use crate::hash;
use crate::store::Store;
use globset::{Glob, GlobSet, GlobSetBuilder};
use serde_json::{json, Value};
use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::{Path, PathBuf};

const MAP_FILE: &str = "archmeld.map.txt";
const MAP_VERSION: &str = "1";
const DEFAULT_BUDGET_TOKENS: u64 = 40_000;
const CHARS_PER_TOKEN: u64 = 4;
const F_MAX: usize = 240;
const A_MAX: usize = 240;
const S_MAX: usize = 400;
const REFS_MAX: usize = 16;
const LIST_LIMIT: usize = 50;

#[derive(Default)]
struct Header {
    declared: bool,
    snapshot: Option<String>,
    commit: Option<String>,
    scope: Option<String>,
    budget_tokens: u64,
    observe: Vec<String>,
    exclude: Vec<String>,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Suffix {
    None,
    Candidate,
    Unknown,
}

struct Reference {
    raw: String,
    kind: RefKind,
    path: String,
    entity_kind: Option<String>,
    name: Option<String>,
    suffix: Suffix,
    line: usize,
}

enum RefKind {
    File,
    Entity,
}

struct Entry {
    path: String,
    tags: Option<String>,
    function: String,
    api: String,
    constraint: String,
    references: Vec<Reference>,
    line: usize,
}

struct Parsed {
    header: Header,
    entries: Vec<Entry>,
    errors: Vec<Value>,
    warnings: Vec<Value>,
    chars: u64,
}

/// Default path of the map inside one workspace.
pub fn map_path(workspace: &Path) -> PathBuf {
    workspace.join(MAP_FILE)
}

/// Parse, resolve and report one map. The map is read-only input; this function
/// never writes it or the ledger.
pub fn check_map(
    store: &Store,
    project_id: &str,
    workspace: &Path,
    snapshot: Option<&str>,
    map: Option<&Path>,
) -> Result<Value> {
    let path = map
        .map(Path::to_path_buf)
        .unwrap_or_else(|| map_path(workspace));
    let latest = store.latest_snapshot_id(project_id)?;
    if !path.is_file() {
        return Ok(json!({
            "schema_version": "1.0",
            "state": "absent",
            "present": false,
            "map_path": display_path(&path),
            "snapshot": {"requested": snapshot, "latest": latest},
            "map_digest": null,
            "freshness": "unknown",
            "semantic_claims": "none",
            "verification": "not_run",
        }));
    }
    let text = fs::read_to_string(&path)
        .map_err(|error| CoreError::Storage(format!("cannot read {}: {error}", path.display())))?;
    let map_digest = hash::digest_bytes(text.as_bytes());
    let parsed = parse_map(&text);
    let mut errors = parsed.errors;
    let mut warnings = parsed.warnings;
    let snapshot_id = match snapshot.map(str::to_string).or_else(|| latest.clone()) {
        Some(snapshot_id) => snapshot_id,
        None => {
            return Ok(json!({
                "schema_version": "1.0",
                "state": "needs_snapshot",
                "present": true,
                "map_path": display_path(&path),
                "map_digest": map_digest,
                "freshness": "unknown",
                "entries": {"total": parsed.entries.len(), "chars": parsed.chars},
                "semantic_claims": "unverified",
                "verification": "not_run",
                "validation": "not_run",
                "errors": errors,
                "warnings": warnings,
            }))
        }
    };
    store.check_snapshot_scope(project_id, &snapshot_id)?;
    let payload = store
        .snapshot_payload(project_id, &snapshot_id)?
        .ok_or_else(|| CoreError::not_found(format!("snapshot {snapshot_id} is not registered")))?;
    let files: BTreeSet<String> = payload["files"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|file| file["path"].as_str().map(normalize_path))
        .collect();

    let header = parsed.header;
    let header_matches = header.snapshot.as_deref() == Some(snapshot_id.as_str());
    if !header.declared {
        errors.push(issue(
            "map_header",
            "missing #ARCHMELD-MAP: 1 header",
            None,
            None,
            None,
        ));
    }
    if header.snapshot.is_none() {
        errors.push(issue(
            "map_unbaselined",
            "the map declares no #snapshot",
            None,
            None,
            None,
        ));
    } else if !header_matches {
        warnings.push(issue(
            "map_stale",
            "the map was authored against a different snapshot",
            None,
            None,
            None,
        ));
    }
    if let Some(latest_id) = &latest {
        if latest_id != &snapshot_id {
            warnings.push(issue(
                "map_not_latest",
                "the map was checked against an older snapshot",
                None,
                None,
                None,
            ));
        }
    }

    let observe = match glob_set(&header.observe) {
        Ok(set) => Some(set),
        Err(message) => {
            errors.push(issue("map_scope", &message, None, None, None));
            None
        }
    };
    let exclude = match glob_set(&header.exclude) {
        Ok(set) => Some(set),
        Err(message) => {
            errors.push(issue("map_scope", &message, None, None, None));
            None
        }
    };

    let mut covered = BTreeSet::new();
    let mut ref_counts = json!({
        "confirmed": 0,
        "candidate": 0,
        "unknown": 0,
        "orphan": 0,
        "ambiguous": 0,
    });
    for entry in &parsed.entries {
        if !files.contains(&entry.path) {
            errors.push(issue(
                "entry_orphan",
                "Entry path is not present in the snapshot",
                Some(&entry.path),
                None,
                Some(entry.line),
            ));
            continue;
        }
        covered.insert(entry.path.clone());
        if let Some(pattern) = find_secret(&entry.function, &entry.api, &entry.constraint) {
            errors.push(issue(
                "secret_literal",
                &format!("Entry contains a {pattern} literal; remove it and reference an environment variable name"),
                Some(&entry.path),
                None,
                Some(entry.line),
            ));
        }
        for reference in &entry.references {
            match reference.suffix {
                Suffix::Unknown => {
                    bump(&mut ref_counts, "unknown");
                    continue;
                }
                Suffix::Candidate => {
                    bump(&mut ref_counts, "candidate");
                    warnings.push(issue(
                        "candidate_suffix",
                        "candidate suffixes are not relation-backed in E1; relation refs are deferred",
                        Some(&entry.path),
                        Some(&reference.raw),
                        Some(reference.line),
                    ));
                }
                Suffix::None => {}
            }
            match &reference.kind {
                RefKind::File => {
                    if files.contains(&reference.path) {
                        if reference.suffix == Suffix::None {
                            bump(&mut ref_counts, "confirmed");
                        }
                    } else {
                        bump(&mut ref_counts, "orphan");
                        errors.push(issue(
                            "ref_orphan",
                            "file reference is not present in the snapshot",
                            Some(&entry.path),
                            Some(&reference.raw),
                            Some(reference.line),
                        ));
                    }
                }
                RefKind::Entity => {
                    let kind = reference.entity_kind.as_deref().unwrap_or("");
                    let name = reference.name.as_deref().unwrap_or("");
                    match resolve_entity(
                        store,
                        project_id,
                        &snapshot_id,
                        kind,
                        &reference.path,
                        name,
                    ) {
                        Ok(0) => {
                            bump(&mut ref_counts, "orphan");
                            errors.push(issue(
                                "ref_orphan",
                                "entity reference is not present in the snapshot",
                                Some(&entry.path),
                                Some(&reference.raw),
                                Some(reference.line),
                            ));
                        }
                        Ok(1) => {
                            if reference.suffix == Suffix::None {
                                bump(&mut ref_counts, "confirmed");
                            }
                        }
                        Ok(_) => {
                            bump(&mut ref_counts, "ambiguous");
                            errors.push(issue(
                                "ref_ambiguous",
                                "entity reference matches more than one entity",
                                Some(&entry.path),
                                Some(&reference.raw),
                                Some(reference.line),
                            ));
                        }
                        Err(error) => {
                            errors.push(issue(
                                "ref_query",
                                &error.to_string(),
                                Some(&entry.path),
                                Some(&reference.raw),
                                Some(reference.line),
                            ));
                        }
                    }
                }
            }
        }
    }

    let mut missing = Vec::new();
    let mut observed = Vec::new();
    let mut excluded = Vec::new();
    for file in &files {
        if is_map_metadata(file) || covered.contains(file) {
            continue;
        }
        let observed_match = observe.as_ref().is_some_and(|set| set.is_match(file));
        let excluded_match = exclude.as_ref().is_some_and(|set| set.is_match(file));
        if excluded_match {
            if excluded.len() < LIST_LIMIT {
                excluded.push(file.clone());
            }
        } else if observed_match {
            if observed.len() < LIST_LIMIT {
                observed.push(file.clone());
            }
        } else if missing.len() < LIST_LIMIT {
            missing.push(file.clone());
        }
    }
    if !missing.is_empty() {
        warnings.push(issue(
            "map_missing",
            &format!(
                "{} snapshot file(s) have no Entry and are not observed/excluded",
                files
                    .iter()
                    .filter(|file| {
                        !is_map_metadata(file)
                            && !covered.contains(*file)
                            && !observe.as_ref().is_some_and(|set| set.is_match(file))
                            && !exclude.as_ref().is_some_and(|set| set.is_match(file))
                    })
                    .count()
            ),
            None,
            None,
            None,
        ));
    }

    let tokens = parsed.chars / CHARS_PER_TOKEN;
    let budget = if header.budget_tokens > 0 {
        header.budget_tokens
    } else {
        DEFAULT_BUDGET_TOKENS
    };
    if tokens > budget {
        warnings.push(issue(
            "map_budget",
            &format!("estimated {tokens} tokens exceed the {budget} token budget"),
            None,
            None,
            None,
        ));
    }

    let state = if !errors.is_empty() {
        "invalid"
    } else if header.snapshot.as_deref() != Some(snapshot_id.as_str()) {
        "stale"
    } else {
        "ok"
    };
    let freshness = match &latest {
        Some(latest_id) if header.snapshot.as_deref() == Some(latest_id.as_str()) => "latest",
        Some(_) => "behind",
        None => "unknown",
    };
    Ok(json!({
        "schema_version": "1.0",
        "state": state,
        "present": true,
        "map_path": display_path(&path),
        "map_digest": map_digest,
        "freshness": freshness,
        "header": {
            "snapshot": header.snapshot,
            "commit": header.commit,
            "scope": header.scope,
            "budget_tokens": budget,
            "observe": header.observe,
            "exclude": header.exclude,
        },
        "snapshot": {
            "requested": snapshot_id,
            "latest": latest,
            "match": header_matches,
        },
        "entries": {
            "total": parsed.entries.len(),
            "files_covered": covered.len(),
            "chars": parsed.chars,
            "tokens_estimate": tokens,
            "budget_tokens": budget,
            "over_budget": tokens > budget,
        },
        "refs": ref_counts,
        "semantic_claims": "unverified",
        "verification": "not_run",
        "validation": "refs_checked",
        "coverage": {
            "snapshot_files": files.len(),
            "covered": covered.len(),
            "missing": missing,
            "observed": observed,
            "excluded": excluded,
            "truncated": files.len() > LIST_LIMIT && missing.len() == LIST_LIMIT,
        },
        "errors": errors,
        "warnings": warnings,
    }))
}

/// Cheap header/entry summary for status and project brief integration. It
/// does not resolve refs; `check_map` is the validating surface.
pub fn summarize(workspace: Option<&Path>, snapshot: Option<&str>) -> Value {
    let Some(workspace) = workspace else {
        return json!({"provider": "archmeld-map", "present": false});
    };
    let path = map_path(workspace);
    let Ok(text) = fs::read_to_string(&path) else {
        return json!({
            "provider": "archmeld-map",
            "present": false,
            "map_digest": null,
            "freshness": "unknown",
        });
    };
    let map_digest = hash::digest_bytes(text.as_bytes());
    let parsed = parse_map(&text);
    let header = parsed.header;
    let tokens = parsed.chars / CHARS_PER_TOKEN;
    let budget = if header.budget_tokens > 0 {
        header.budget_tokens
    } else {
        DEFAULT_BUDGET_TOKENS
    };
    let snapshot_match = header.snapshot.as_deref() == snapshot;
    let freshness = if snapshot_match {
        "latest"
    } else if snapshot.is_some() {
        "behind"
    } else {
        "unknown"
    };
    json!({
        "provider": "archmeld-map",
        "present": true,
        "map_path": display_path(&path),
        "map_digest": map_digest,
        "freshness": freshness,
        "snapshot": header.snapshot,
        "snapshot_match": snapshot_match,
        "entries": parsed.entries.len(),
        "tokens_estimate": tokens,
        "budget_tokens": budget,
        "parse_errors": parsed.errors.len(),
        "semantic_claims": "unverified",
        "verification": "not_run",
        "validation": "not_run",
        "state": if !parsed.errors.is_empty() {
            "invalid"
        } else if snapshot_match {
            "ok"
        } else {
            "stale"
        },
    })
}

/// Map entries whose object path is in the changed set. This is a navigation
/// aid for change briefs; it does not claim that an Entry's prose is current.
pub fn affected(workspace: &Path, snapshot: Option<&str>, changed_paths: &[String]) -> Value {
    let path = map_path(workspace);
    let Ok(text) = fs::read_to_string(&path) else {
        return json!({"provider": "archmeld-map", "present": false});
    };
    let digest = hash::digest_bytes(text.as_bytes());
    let parsed = parse_map(&text);
    let snapshot_match =
        snapshot.map(|snapshot| parsed.header.snapshot.as_deref() == Some(snapshot));
    let state = if !parsed.errors.is_empty() {
        "invalid"
    } else if snapshot.is_none() {
        "unknown"
    } else if snapshot_match == Some(true) {
        "ok"
    } else {
        "stale"
    };
    let changed: BTreeSet<String> = changed_paths
        .iter()
        .map(|path| normalize_path(path))
        .collect();
    let mut affected = Vec::new();
    let mut matched = BTreeSet::new();
    let mut binding = json!({"file": 0, "entity": 0});
    for entry in &parsed.entries {
        if changed.contains(&entry.path) {
            matched.insert(entry.path.clone());
            for reference in &entry.references {
                bump(
                    &mut binding,
                    match reference.kind {
                        RefKind::File => "file",
                        RefKind::Entity => "entity",
                    },
                );
            }
            affected.push(entry_json(entry));
        }
    }
    let unmapped = changed
        .into_iter()
        .filter(|path| !matched.contains(path))
        .collect::<Vec<_>>();
    json!({
        "provider": "archmeld-map",
        "present": true,
        "map_path": display_path(&path),
        "map_snapshot": parsed.header.snapshot,
        "snapshot_match": snapshot_match,
        "state": state,
        "map_digest": digest,
        "entries_total": parsed.entries.len(),
        "affected": affected,
        "unmapped_changed_files": unmapped,
        "binding": binding,
        "parse_errors": parsed.errors.len(),
        "semantic_claims": "unverified",
        "verification": "not_run",
        "validation": "not_run",
        "warnings": parsed.warnings,
    })
}

/// Compare the map's snapshot with the requested snapshot. Read-only: it
/// reports stale/orphan/missing entries and never rewrites the map.
pub fn maintain(
    store: &Store,
    project_id: &str,
    workspace: &Path,
    snapshot: Option<&str>,
    map: Option<&Path>,
) -> Result<Value> {
    let report = check_map(store, project_id, workspace, snapshot, map)?;
    if report["state"] == "absent" || report["state"] == "needs_snapshot" {
        let mut report = report;
        report["command"] = json!("maintain");
        return Ok(report);
    }
    let current_id = report["snapshot"]["requested"]
        .as_str()
        .unwrap_or_default()
        .to_string();
    let map_id = report["header"]["snapshot"].as_str().map(str::to_string);
    let path = map
        .map(Path::to_path_buf)
        .unwrap_or_else(|| map_path(workspace));
    let text = fs::read_to_string(&path)
        .map_err(|error| CoreError::Storage(format!("cannot read {}: {error}", path.display())))?;
    let parsed = parse_map(&text);
    let current_files = file_digests(store, project_id, &current_id)?;
    let old_files = match map_id.as_deref() {
        Some(id) if id != current_id => file_digests_optional(store, project_id, id)?,
        _ => Some(current_files.clone()),
    };
    let mut stale_entries = Vec::new();
    let mut unbaselined_entries = Vec::new();
    let mut unchanged = 0usize;
    for entry in &parsed.entries {
        match (
            old_files.as_ref().and_then(|files| files.get(&entry.path)),
            current_files.get(&entry.path),
        ) {
            (Some(old), Some(new)) if old != new => stale_entries.push(json!({
                "path": &entry.path,
                "line": entry.line,
                "old_blob": old,
                "new_blob": new,
            })),
            (Some(_), Some(_)) => unchanged += 1,
            (None, Some(_)) => unbaselined_entries.push(json!({
                "path": &entry.path,
                "line": entry.line,
                "reason": "the map snapshot does not contain this path",
            })),
            (_, None) => {}
        }
    }
    let mut warnings = report["warnings"].as_array().cloned().unwrap_or_default();
    if map_id.as_deref().is_some_and(|id| id != current_id) && old_files.is_none() {
        warnings.push(issue(
            "map_snapshot_unknown",
            "the map snapshot is not registered; entry freshness cannot be compared",
            None,
            None,
            None,
        ));
    }
    let map_relative = path
        .strip_prefix(workspace)
        .ok()
        .map(|path| normalize_path(&path.to_string_lossy()));
    let map_digest = hash::digest_bytes(text.as_bytes());
    let map_tracked = map_relative
        .as_ref()
        .and_then(|relative| current_files.get(relative));
    let map_dirty = map_tracked.is_some_and(|tracked| tracked != &map_digest);
    let orphan_entries = report["errors"]
        .as_array()
        .cloned()
        .unwrap_or_default()
        .into_iter()
        .filter(|error| {
            matches!(
                error["code"].as_str(),
                Some("entry_orphan") | Some("ref_orphan") | Some("ref_ambiguous")
            )
        })
        .collect::<Vec<_>>();
    let state = if report["state"] == "invalid" {
        "invalid"
    } else if !stale_entries.is_empty()
        || !unbaselined_entries.is_empty()
        || map_id.as_deref() != Some(current_id.as_str())
    {
        "stale"
    } else {
        "ok"
    };
    let freshness = match map_id.as_deref() {
        Some(id) if id == current_id => "latest",
        Some(_) => "behind",
        None => "unknown",
    };
    Ok(json!({
        "schema_version": "1.0",
        "state": state,
        "command": "maintain",
        "map_path": display_path(&path),
        "map_digest": map_digest,
        "freshness": freshness,
        "map_snapshot": map_id,
        "current_snapshot": current_id,
        "entries": {
            "total": parsed.entries.len(),
            "unchanged": unchanged,
            "stale": stale_entries.len(),
            "unbaselined": unbaselined_entries.len(),
            "missing": report["coverage"]["missing"].as_array().map(Vec::len).unwrap_or(0),
            "orphan": orphan_entries.len(),
        },
        "stale_entries": stale_entries,
        "unbaselined_entries": unbaselined_entries,
        "missing_entries": report["coverage"]["missing"],
        "orphan_entries": orphan_entries,
        "map_dirty": map_dirty,
        "map_tracked": map_tracked.is_some(),
        "check_state": report["state"],
        "refs": report["refs"],
        "semantic_claims": "unverified",
        "verification": "not_run",
        "validation": "refs_checked",
        "errors": report["errors"],
        "warnings": warnings,
    }))
}

/// One Entry with its check issues. Used by `inspect(kind=map_entry)`.
pub fn entry_report(
    store: &Store,
    project_id: &str,
    workspace: &Path,
    snapshot: Option<&str>,
    map: Option<&Path>,
    entry_path: &str,
) -> Result<Value> {
    let report = check_map(store, project_id, workspace, snapshot, map)?;
    if report["state"] == "absent" {
        return Ok(json!({
            "schema_version": "1.0",
            "state": "absent",
            "present": false,
            "map_path": report["map_path"],
            "entry": null,
            "next_action": {
                "tool": "archmeld_brief",
                "args": {"recipe": "project"},
                "why": "no archmeld.map.txt in the workspace; author one before reading an Entry"
            }
        }));
    }
    if report["state"] == "needs_snapshot" {
        let mut value = report.clone();
        value["present"] = json!(true);
        value["entry"] = json!(null);
        value["next_action"] = json!({
            "tool": "archmeld_status",
            "args": {},
            "why": "the map exists but no snapshot is registered"
        });
        return Ok(value);
    }
    let path = map
        .map(Path::to_path_buf)
        .unwrap_or_else(|| map_path(workspace));
    let text = fs::read_to_string(&path)
        .map_err(|error| CoreError::Storage(format!("cannot read {}: {error}", path.display())))?;
    let parsed = parse_map(&text);
    let wanted = normalize_path(entry_path);
    let entry = parsed
        .entries
        .iter()
        .find(|entry| entry.path == wanted)
        .ok_or_else(|| CoreError::not_found(format!("map entry {wanted} not found")))?;
    let filter = |items: &Value| -> Vec<Value> {
        items
            .as_array()
            .cloned()
            .unwrap_or_default()
            .into_iter()
            .filter(|issue| issue["path"].as_str() == Some(wanted.as_str()))
            .collect()
    };
    Ok(json!({
        "schema_version": "1.0",
        "map_path": display_path(&path),
        "state": report["state"],
        "map_digest": report["map_digest"],
        "freshness": report["freshness"],
        "snapshot_match": report["snapshot"]["match"],
        "entry": entry_json(entry),
        "refs": report["refs"],
        "semantic_claims": "unverified",
        "verification": "not_run",
        "validation": "refs_checked",
        "errors": filter(&report["errors"]),
        "warnings": filter(&report["warnings"]),
    }))
}

fn entry_json(entry: &Entry) -> Value {
    json!({
        "path": &entry.path,
        "tags": &entry.tags,
        "function": &entry.function,
        "api": &entry.api,
        "constraint": &entry.constraint,
        "line": entry.line,
        "refs": entry.references.iter().map(|reference| json!({
            "raw": &reference.raw,
            "kind": match &reference.kind {
                RefKind::File => "file",
                RefKind::Entity => "entity",
            },
            "suffix": match reference.suffix {
                Suffix::None => "none",
                Suffix::Candidate => "candidate",
                Suffix::Unknown => "unknown",
            },
        })).collect::<Vec<_>>(),
    })
}

fn file_digests(
    store: &Store,
    project_id: &str,
    snapshot_id: &str,
) -> Result<BTreeMap<String, String>> {
    file_digests_optional(store, project_id, snapshot_id)?
        .ok_or_else(|| CoreError::not_found(format!("snapshot {snapshot_id} is not registered")))
}

fn file_digests_optional(
    store: &Store,
    project_id: &str,
    snapshot_id: &str,
) -> Result<Option<BTreeMap<String, String>>> {
    let Some(payload) = store.snapshot_payload(project_id, snapshot_id)? else {
        return Ok(None);
    };
    let mut files = BTreeMap::new();
    for file in payload["files"].as_array().into_iter().flatten() {
        if let (Some(path), Some(digest)) = (file["path"].as_str(), file["blob_digest"].as_str()) {
            files.insert(normalize_path(path), digest.to_string());
        }
    }
    Ok(Some(files))
}

fn parse_map(text: &str) -> Parsed {
    let mut header = Header {
        budget_tokens: DEFAULT_BUDGET_TOKENS,
        ..Header::default()
    };
    let mut entries = Vec::new();
    let mut errors = Vec::new();
    let mut warnings = Vec::new();
    let mut section = String::new();
    let mut seen = BTreeSet::new();
    for (index, raw) in text.lines().enumerate() {
        let line_no = index + 1;
        let line = raw.trim_end_matches('\r').trim();
        if line.is_empty() {
            continue;
        }
        if line.starts_with('#') {
            if let Some(rest) = line.strip_prefix("#ARCHMELD-MAP:") {
                header.declared = rest.trim() == MAP_VERSION;
                if !header.declared {
                    errors.push(issue(
                        "map_version",
                        "unsupported map version",
                        None,
                        None,
                        Some(line_no),
                    ));
                }
                continue;
            }
            if let Some(value) = line.strip_prefix("#snapshot:") {
                header.snapshot = Some(value.trim().to_string());
                continue;
            }
            if let Some(value) = line.strip_prefix("#commit:") {
                header.commit = Some(value.trim().to_string());
                continue;
            }
            if let Some(value) = line.strip_prefix("#scope:") {
                header.scope = Some(value.trim().to_string());
                continue;
            }
            if let Some(value) = line.strip_prefix("#budget_tokens:") {
                match value.trim().parse::<u64>() {
                    Ok(value) if value > 0 => header.budget_tokens = value,
                    _ => warnings.push(issue(
                        "map_budget",
                        "budget_tokens is not a positive integer; using the default",
                        None,
                        None,
                        Some(line_no),
                    )),
                }
                continue;
            }
            if let Some(value) = line.strip_prefix("#observe:") {
                header.observe.extend(
                    value
                        .split(',')
                        .map(str::trim)
                        .filter(|item| !item.is_empty())
                        .map(str::to_string),
                );
                continue;
            }
            if let Some(value) = line.strip_prefix("#exclude:") {
                header.exclude.extend(
                    value
                        .split(',')
                        .map(str::trim)
                        .filter(|item| !item.is_empty())
                        .map(str::to_string),
                );
                continue;
            }
            continue;
        }
        if line.starts_with("===") && line.ends_with("===") {
            let inner = line.trim_matches('=').trim();
            match section_path(inner) {
                Ok(value) => section = value,
                Err(message) => {
                    errors.push(issue("section_path", &message, None, None, Some(line_no)));
                    section.clear();
                }
            }
            continue;
        }
        match parse_entry(line, line_no, &section) {
            Ok(entry) => {
                if !seen.insert(entry.path.clone()) {
                    errors.push(issue(
                        "entry_duplicate",
                        "duplicate Entry for the same path",
                        Some(&entry.path),
                        None,
                        Some(line_no),
                    ));
                } else {
                    entries.push(entry);
                }
            }
            Err(mut issues) => errors.append(&mut issues),
        }
    }
    Parsed {
        header,
        entries,
        errors,
        warnings,
        chars: text.chars().count() as u64,
    }
}

fn parse_entry(
    line: &str,
    line_no: usize,
    section: &str,
) -> std::result::Result<Entry, Vec<Value>> {
    let Some((head, body)) = line.split_once(": ") else {
        return Err(vec![issue(
            "entry_grammar",
            "Entry must look like name[tags]: F:... | R:... | A:... | S:...",
            None,
            None,
            Some(line_no),
        )]);
    };
    let (name, tags) = if let Some(open) = head.find('[') {
        if !head.ends_with(']') || open == 0 {
            return Err(vec![issue(
                "entry_tags",
                "malformed Entry tag",
                None,
                None,
                Some(line_no),
            )]);
        }
        (&head[..open], Some(&head[open + 1..head.len() - 1]))
    } else {
        (head, None)
    };
    if name.is_empty() || name.contains('/') || name.contains('\\') || name == "." || name == ".." {
        return Err(vec![issue(
            "entry_name",
            "Entry name must be a bare file name inside the section",
            None,
            None,
            Some(line_no),
        )]);
    }
    if let Some(tags) = tags {
        if tags.is_empty() || !tags.chars().all(|ch| ch.is_ascii_alphanumeric()) {
            return Err(vec![issue(
                "entry_tags",
                "tags must be non-empty ASCII letters/digits",
                None,
                None,
                Some(line_no),
            )]);
        }
    }
    let mut function = None;
    let mut api = None;
    let mut constraint = None;
    let mut refs_raw = None;
    for field in body.split(" | ") {
        let Some((key, value)) = field.split_once(':') else {
            return Err(vec![issue(
                "entry_grammar",
                "Entry fields must be F/R/A/S prefixed",
                None,
                None,
                Some(line_no),
            )]);
        };
        let value = value.trim();
        match key.trim() {
            "F" => function = Some(value.to_string()),
            "R" => refs_raw = Some(value.to_string()),
            "A" => api = Some(value.to_string()),
            "S" => constraint = Some(value.to_string()),
            other => {
                return Err(vec![issue(
                    "entry_field",
                    &format!("unknown Entry field {other}"),
                    None,
                    None,
                    Some(line_no),
                )])
            }
        }
    }
    let (Some(function), Some(api), Some(constraint), Some(refs_raw)) =
        (function, api, constraint, refs_raw)
    else {
        return Err(vec![issue(
            "entry_field",
            "Entry requires F, R, A and S exactly once",
            None,
            None,
            Some(line_no),
        )]);
    };
    if function.is_empty() || function == "-" {
        return Err(vec![issue(
            "entry_function",
            "F must be one non-empty responsibility proposition",
            None,
            None,
            Some(line_no),
        )]);
    }
    let mut issues = Vec::new();
    for (field, value, max) in [
        ("F", &function, F_MAX),
        ("A", &api, A_MAX),
        ("S", &constraint, S_MAX),
    ] {
        if value.chars().count() > max {
            issues.push(issue(
                "entry_length",
                &format!("{field} exceeds its {max}-character limit"),
                None,
                None,
                Some(line_no),
            ));
        }
    }
    let mut references = Vec::new();
    if refs_raw != "-" && !refs_raw.is_empty() {
        let raw_refs: Vec<&str> = refs_raw.split(',').map(str::trim).collect();
        if raw_refs.len() > REFS_MAX {
            issues.push(issue(
                "entry_refs",
                &format!("R exceeds the {REFS_MAX}-reference limit"),
                None,
                None,
                Some(line_no),
            ));
        }
        for raw in raw_refs {
            match parse_reference(raw, line_no) {
                Ok(reference) => references.push(reference),
                Err(mut found) => issues.append(&mut found),
            }
        }
    }
    if !issues.is_empty() {
        return Err(issues);
    }
    let path = if section.is_empty() {
        name.to_string()
    } else {
        format!("{}/{}", section.trim_end_matches('/'), name)
    };
    Ok(Entry {
        path: normalize_path(&path),
        tags: tags.map(str::to_string),
        function,
        api,
        constraint,
        references,
        line: line_no,
    })
}

fn parse_reference(raw: &str, line_no: usize) -> std::result::Result<Reference, Vec<Value>> {
    let (core, suffix) = if let Some(start) = raw.find("@unknown(") {
        if !raw.ends_with(')') {
            return Err(vec![issue(
                "ref_suffix",
                "malformed @unknown(reason) suffix",
                None,
                Some(raw),
                Some(line_no),
            )]);
        }
        let reason = &raw[start + "@unknown(".len()..raw.len() - 1];
        if reason.trim().is_empty() {
            return Err(vec![issue(
                "ref_suffix",
                "@unknown requires a reason",
                None,
                Some(raw),
                Some(line_no),
            )]);
        }
        (&raw[..start], Suffix::Unknown)
    } else if let Some(core) = raw.strip_suffix("@candidate") {
        (core, Suffix::Candidate)
    } else {
        (raw, Suffix::None)
    };
    if let Some(rest) = core.strip_prefix("file:") {
        validate_ref_path(rest, raw, line_no)?;
        return Ok(Reference {
            raw: raw.to_string(),
            kind: RefKind::File,
            path: normalize_path(rest),
            entity_kind: None,
            name: None,
            suffix,
            line: line_no,
        });
    }
    if let Some(rest) = core.strip_prefix("entity:") {
        let Some((kind, remainder)) = rest.split_once(':') else {
            return Err(vec![issue(
                "ref_entity",
                "entity reference needs entity:<kind>:<path>::<name>",
                None,
                Some(raw),
                Some(line_no),
            )]);
        };
        let Some((path, name)) = remainder.split_once("::") else {
            return Err(vec![issue(
                "ref_entity",
                "entity reference needs a path::name",
                None,
                Some(raw),
                Some(line_no),
            )]);
        };
        validate_ref_path(path, raw, line_no)?;
        if kind.is_empty() || name.is_empty() {
            return Err(vec![issue(
                "ref_entity",
                "entity kind and name must be non-empty",
                None,
                Some(raw),
                Some(line_no),
            )]);
        }
        return Ok(Reference {
            raw: raw.to_string(),
            kind: RefKind::Entity,
            path: normalize_path(path),
            entity_kind: Some(kind.to_string()),
            name: Some(name.to_string()),
            suffix,
            line: line_no,
        });
    }
    if core.starts_with("relation:") {
        return Err(vec![issue(
            "ref_relation_deferred",
            "relation references are not implemented; E3 was not promoted after the E4a no-go",
            None,
            Some(raw),
            Some(line_no),
        )]);
    }
    Err(vec![issue(
        "ref_prefix",
        "reference must start with file: or entity:",
        None,
        Some(raw),
        Some(line_no),
    )])
}

fn validate_ref_path(path: &str, raw: &str, line_no: usize) -> std::result::Result<(), Vec<Value>> {
    let path = path.trim();
    if path.is_empty()
        || path.starts_with('/')
        || path.contains('\\')
        || path.split('/').any(|part| part.is_empty() || part == "..")
        || (path.len() > 1 && path.as_bytes().get(1) == Some(&b':'))
    {
        return Err(vec![issue(
            "ref_path",
            "reference path must be repository-relative and must not contain ..",
            None,
            Some(raw),
            Some(line_no),
        )]);
    }
    Ok(())
}

fn section_path(inner: &str) -> std::result::Result<String, String> {
    let inner = inner.trim();
    if inner.is_empty() || inner == "." || inner == "./" || inner == "/" {
        return Ok(String::new());
    }
    if inner.starts_with('/')
        || inner.contains('\\')
        || inner.split('/').any(|part| part == "..")
        || (inner.len() > 1 && inner.as_bytes().get(1) == Some(&b':'))
    {
        return Err("section path must be repository-relative".to_string());
    }
    Ok(inner
        .strip_prefix("./")
        .unwrap_or(inner)
        .trim_end_matches('/')
        .to_string())
}

fn normalize_path(path: &str) -> String {
    let path = path.replace('\\', "/");
    let path = path.strip_prefix("./").unwrap_or(&path);
    path.trim_start_matches('/').to_string()
}

fn resolve_entity(
    store: &Store,
    project_id: &str,
    snapshot_id: &str,
    kind: &str,
    path: &str,
    name: &str,
) -> Result<usize> {
    let kind = canonical_entity_kind(kind);
    let qualified_name = if kind == "file" {
        path.to_string()
    } else {
        format!("{path}::{name}")
    };
    let mut statement = store.conn.prepare(
        "SELECT payload_json FROM entities
         WHERE project_id=?1 AND snapshot_id=?2 AND qualified_name=?3",
    )?;
    let rows = statement.query_map((project_id, snapshot_id, &qualified_name), |row| {
        row.get::<_, String>(0)
    })?;
    let mut count = 0usize;
    for row in rows {
        let text = row?;
        let payload: Value = serde_json::from_str(&text)
            .map_err(|error| CoreError::Storage(format!("entity payload corrupt: {error}")))?;
        let same_kind = payload["kind"].as_str() == Some(kind);
        let same_path = payload["anchors"]
            .as_array()
            .and_then(|anchors| anchors.first())
            .and_then(|anchor| anchor["path"].as_str())
            .map(normalize_path)
            .is_some_and(|value| value == path);
        if same_kind && same_path {
            count += 1;
        }
    }
    Ok(count)
}

/// Map the vocabulary a model naturally writes onto the ledger's entity kinds.
/// `symbol` is canonical; `function`/`method`/`variable` are accepted aliases.
fn canonical_entity_kind(kind: &str) -> &str {
    match kind {
        "function" | "method" | "variable" | "var" => "symbol",
        "class" | "interface" => "type",
        other => other,
    }
}

fn glob_set(patterns: &[String]) -> std::result::Result<GlobSet, String> {
    let mut builder = GlobSetBuilder::new();
    for pattern in patterns {
        let glob =
            Glob::new(pattern).map_err(|error| format!("invalid glob {pattern}: {error}"))?;
        builder.add(glob);
    }
    builder
        .build()
        .map_err(|error| format!("cannot build scope globs: {error}"))
}

fn is_map_metadata(path: &str) -> bool {
    path == MAP_FILE || path == "archmeld.map.candidate.txt" || path.starts_with(".archmeld/")
}

fn find_secret<'a>(function: &'a str, api: &'a str, constraint: &'a str) -> Option<&'a str> {
    const PATTERNS: [&str; 7] = [
        "sk-",
        "ghp_",
        "github_pat_",
        "xoxb-",
        "xoxp-",
        "BEGIN RSA PRIVATE KEY",
        "BEGIN OPENSSH PRIVATE KEY",
    ];
    for text in [function, api, constraint] {
        for pattern in PATTERNS {
            if text.contains(pattern) {
                return Some(pattern);
            }
        }
    }
    None
}

fn bump(counts: &mut Value, key: &str) {
    let value = counts[key].as_u64().unwrap_or(0);
    counts[key] = json!(value + 1);
}

fn issue(
    code: &str,
    message: &str,
    path: Option<&str>,
    reference: Option<&str>,
    line: Option<usize>,
) -> Value {
    json!({
        "code": code,
        "message": message,
        "path": path,
        "ref": reference,
        "line": line,
    })
}

fn display_path(path: &Path) -> String {
    let text = path.to_string_lossy();
    text.strip_prefix(r"\\?\")
        .or_else(|| text.strip_prefix("//?/"))
        .unwrap_or(&text)
        .to_string()
}

/// Human-readable rendering for `map check --markdown`.
pub fn render_markdown(report: &Value) -> String {
    let command = report["command"].as_str().unwrap_or("check");
    let mut out = String::new();
    out.push_str(&format!("# ARCHMELD map {command}\n\n"));
    let snapshot = report["snapshot"]["requested"]
        .as_str()
        .or_else(|| report["current_snapshot"].as_str())
        .or_else(|| report["map_snapshot"].as_str())
        .unwrap_or("none");
    out.push_str(&format!(
        "- state: {}\n- map: {}\n- snapshot: {}\n",
        report["state"].as_str().unwrap_or("unknown"),
        report["map_path"].as_str().unwrap_or("(missing)"),
        snapshot,
    ));
    if let Some(digest) = report["map_digest"].as_str() {
        out.push_str(&format!("- map digest: {digest}\n"));
    }
    if let Some(freshness) = report["freshness"].as_str() {
        out.push_str(&format!("- freshness: {freshness}\n"));
    }
    out.push_str(&format!(
        "- semantic claims: {} · verification: {} · validation: {}\n",
        report["semantic_claims"].as_str().unwrap_or("unverified"),
        report["verification"].as_str().unwrap_or("not_run"),
        report["validation"].as_str().unwrap_or("not_run"),
    ));
    if command == "maintain" {
        if let Some(entries) = report.get("entries") {
            out.push_str(&format!(
                "- entries: {} total · {} unchanged · {} stale · {} unbaselined · {} missing · {} orphan\n",
                entries["total"].as_u64().unwrap_or(0),
                entries["unchanged"].as_u64().unwrap_or(0),
                entries["stale"].as_u64().unwrap_or(0),
                entries["unbaselined"].as_u64().unwrap_or(0),
                entries["missing"].as_u64().unwrap_or(0),
                entries["orphan"].as_u64().unwrap_or(0),
            ));
        }
    } else if let Some(entries) = report.get("entries") {
        out.push_str(&format!(
            "- entries: {} · files covered: {} · tokens: {} / {}\n",
            entries["total"].as_u64().unwrap_or(0),
            entries["files_covered"].as_u64().unwrap_or(0),
            entries["tokens_estimate"].as_u64().unwrap_or(0),
            entries["budget_tokens"].as_u64().unwrap_or(0),
        ));
    }
    if let Some(refs) = report.get("refs") {
        out.push_str(&format!(
            "- refs: confirmed {} · candidate {} · unknown {} · orphan {} · ambiguous {}\n",
            refs["confirmed"].as_u64().unwrap_or(0),
            refs["candidate"].as_u64().unwrap_or(0),
            refs["unknown"].as_u64().unwrap_or(0),
            refs["orphan"].as_u64().unwrap_or(0),
            refs["ambiguous"].as_u64().unwrap_or(0),
        ));
    }
    if let Some(coverage) = report.get("coverage") {
        out.push_str(&format!(
            "- coverage: {}/{} files · missing {}\n",
            coverage["covered"].as_u64().unwrap_or(0),
            coverage["snapshot_files"].as_u64().unwrap_or(0),
            coverage["missing"].as_array().map(Vec::len).unwrap_or(0),
        ));
    }
    for section in ["errors", "warnings"] {
        let items = report[section].as_array().cloned().unwrap_or_default();
        if items.is_empty() {
            continue;
        }
        out.push_str(&format!("\n## {section}\n\n"));
        for item in items.iter().take(LIST_LIMIT) {
            out.push_str(&format!(
                "- [{}] {}{}\n",
                item["code"].as_str().unwrap_or("issue"),
                item["message"].as_str().unwrap_or(""),
                item["path"]
                    .as_str()
                    .map(|path| format!(" ({path})"))
                    .unwrap_or_default(),
            ));
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_a_minimal_entry() {
        let parsed = parse_map(
            "#ARCHMELD-MAP: 1\n#snapshot: sha256:abc\n===src/===\n\
             a.py[L1D2C5M]: F:Does A. | R:file:src/b.py | A:- | S:-\n",
        );
        assert!(parsed.errors.is_empty(), "{:?}", parsed.errors);
        assert_eq!(parsed.entries.len(), 1);
        assert_eq!(parsed.entries[0].path, "src/a.py");
        assert_eq!(parsed.entries[0].references.len(), 1);
    }

    #[test]
    fn rejects_a_relation_reference_in_e1() {
        let parsed = parse_map(
            "#ARCHMELD-MAP: 1\n#snapshot: sha256:abc\n===src/===\n\
             a.py: F:Does A. | R:relation:call:src/a.py::a->src/b.py::b | A:- | S:-\n",
        );
        assert!(parsed
            .errors
            .iter()
            .any(|issue| issue["code"] == "ref_relation_deferred"));
    }

    #[test]
    fn rejects_an_absolute_section_and_path() {
        let parsed = parse_map(
            "#ARCHMELD-MAP: 1\n#snapshot: sha256:abc\n===/abs/===\n\
             a.py: F:Does A. | R:file:/etc/passwd | A:- | S:-\n",
        );
        assert!(parsed
            .errors
            .iter()
            .any(|issue| issue["code"] == "section_path"));
        assert!(parsed
            .errors
            .iter()
            .any(|issue| issue["code"] == "ref_path"));
    }

    #[test]
    fn accepts_unknown_and_candidate_suffixes() {
        let parsed = parse_map(
            "#ARCHMELD-MAP: 1\n#snapshot: sha256:abc\n===src/===\n\
             a.py: F:Does A. | R:entity:function:src/a.py::a@unknown(dynamic dispatch),file:src/b.py@candidate | A:- | S:-\n",
        );
        assert!(parsed.errors.is_empty(), "{:?}", parsed.errors);
        assert_eq!(parsed.entries[0].references.len(), 2);
    }
}
