// Copyright 2026 ARCHMELD contributors
// SPDX-License-Identifier: Apache-2.0

//! Task-level diff review.
//!
//! The review is deliberately static: it compares the current workspace bytes
//! with a frozen snapshot, reports line overlap and graph impact, and never
//! executes a build or test. A clean result is not a correctness claim.

use crate::drift::analyze_drift;
use crate::error::{CoreError, Result};
use crate::hash;
use crate::snapshot::git_workspace_tree;
use crate::store::{validate, Store};
use serde_json::{json, Value};
use similar::{DiffOp, TextDiff};
use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::Path;
use std::time::Instant;

const MAX_TEXT_BYTES: u64 = 8 * 1024 * 1024;
const MAX_LIST: usize = 500;
/// Freeze stops before this many candidate bytes would be written. The limit
/// keeps one review inside the host's memory and disk budget; reaching it is
/// reported as `truncated`, never as a durable freeze.
const MAX_FREEZE_BYTES: u64 = 64 * 1024 * 1024;
const MAX_STRUCTURAL_PATHS: usize = 20;
const MAX_PATH_DEPTH: usize = 4;
const MAX_PATH_EXPANSIONS: usize = 5_000;

#[derive(Debug, Clone)]
struct EntitySpan {
    entity_id: String,
    qualified_name: String,
    kind: String,
    start: u64,
    end: u64,
}

#[derive(Debug, Clone, Copy)]
struct LineChange {
    kind: &'static str,
    old_start: u64,
    old_len: u64,
    new_start: u64,
    new_len: u64,
}

impl LineChange {
    fn to_json(self) -> Value {
        json!({
            "kind": self.kind,
            "old_start": self.old_start,
            "old_len": self.old_len,
            "new_start": self.new_start,
            "new_len": self.new_len,
            "side": "both",
        })
    }

    fn overlaps(self, span: &EntitySpan) -> bool {
        if self.old_len == 0 {
            return span.start <= self.old_start && self.old_start <= span.end.saturating_add(1);
        }
        let change_end = self.old_start.saturating_add(self.old_len - 1);
        span.start <= change_end && self.old_start <= span.end
    }
}

fn line_changes(base: &str, current: &str) -> Vec<LineChange> {
    let diff = TextDiff::from_lines(base, current);
    let mut changes = Vec::new();
    for op in diff.ops() {
        match *op {
            DiffOp::Equal { .. } => {}
            DiffOp::Delete {
                old_index,
                old_len,
                new_index,
            } => changes.push(LineChange {
                kind: "delete",
                old_start: old_index as u64 + 1,
                old_len: old_len as u64,
                new_start: new_index as u64 + 1,
                new_len: 0,
            }),
            DiffOp::Insert {
                old_index,
                new_index,
                new_len,
            } => changes.push(LineChange {
                kind: "insert",
                old_start: old_index as u64 + 1,
                old_len: 0,
                new_start: new_index as u64 + 1,
                new_len: new_len as u64,
            }),
            DiffOp::Replace {
                old_index,
                old_len,
                new_index,
                new_len,
            } => changes.push(LineChange {
                kind: "replace",
                old_start: old_index as u64 + 1,
                old_len: old_len as u64,
                new_start: new_index as u64 + 1,
                new_len: new_len as u64,
            }),
        }
    }
    changes
}

fn read_current(root: &Path, path: &str) -> Result<Vec<u8>> {
    validate::validate_path("review path", path)?;
    let target = root.join(path);
    let canonical_root = fs::canonicalize(root)?;
    let canonical = fs::canonicalize(&target)
        .map_err(|error| CoreError::not_found(format!("{path} is not readable: {error}")))?;
    if !canonical.starts_with(&canonical_root) {
        return Err(CoreError::invalid(format!(
            "review path {path} resolves outside the workspace"
        )));
    }
    let metadata = fs::metadata(&canonical)?;
    if metadata.len() > MAX_TEXT_BYTES {
        return Err(CoreError::unsupported(format!(
            "{path} is larger than the {MAX_TEXT_BYTES}-byte review limit"
        )));
    }
    Ok(fs::read(&canonical)?)
}

fn snapshot_files(store: &Store, project_id: &str, snapshot_id: &str) -> Result<BTreeMap<String, String>> {
    let payload = store
        .snapshot_payload(project_id, snapshot_id)?
        .ok_or_else(|| CoreError::not_found(format!("snapshot {snapshot_id} is missing")))?;
    let mut files = BTreeMap::new();
    for file in payload["files"].as_array().into_iter().flatten() {
        if let (Some(path), Some(digest)) = (file["path"].as_str(), file["blob_digest"].as_str()) {
            files.insert(path.to_string(), digest.to_string());
        }
    }
    Ok(files)
}

fn entity_spans(
    store: &Store,
    project_id: &str,
    snapshot_id: &str,
) -> Result<(BTreeMap<String, Vec<EntitySpan>>, BTreeMap<String, String>)> {
    let mut by_path: BTreeMap<String, Vec<EntitySpan>> = BTreeMap::new();
    let mut names = BTreeMap::new();
    for entity_id in store.entity_ids(project_id, snapshot_id)? {
        let Some(entity) = store.get_entity(project_id, snapshot_id, &entity_id)? else {
            continue;
        };
        let name = entity["qualified_name"].as_str().unwrap_or("").to_string();
        let kind = entity["kind"].as_str().unwrap_or("").to_string();
        names.insert(entity_id.clone(), name.clone());
        for anchor in entity["anchors"].as_array().into_iter().flatten() {
            let Some(path) = anchor["path"].as_str() else {
                continue;
            };
            let (Some(start), Some(end)) = (
                anchor["line_start"].as_u64(),
                anchor["line_end"].as_u64(),
            ) else {
                continue;
            };
            by_path.entry(path.to_string()).or_default().push(EntitySpan {
                entity_id: entity_id.clone(),
                qualified_name: name.clone(),
                kind: kind.clone(),
                start,
                end,
            });
        }
    }
    Ok((by_path, names))
}

fn relation_impact(
    relations: &[Value],
    entity_id: &str,
    names: &BTreeMap<String, String>,
) -> Vec<Value> {
    let mut impact = Vec::new();
    for relation in relations {
        let source = relation["source_id"].as_str().unwrap_or("");
        let target = relation["target_id"].as_str().unwrap_or("");
        let (direction, other) = if source == entity_id {
            ("outgoing", target)
        } else if target == entity_id {
            ("incoming", source)
        } else {
            continue;
        };
        impact.push(json!({
            "relation_id": relation["id"],
            "kind": relation["kind"],
            "basis": relation["basis"],
            "resolution": relation["resolution"],
            "direction": direction,
            "other_entity_id": other,
            "other_qualified_name": names.get(other),
        }));
        if impact.len() >= 50 {
            break;
        }
    }
    impact
}

fn file_review(
    store: &Store,
    base_snapshot_id: &str,
    root: &Path,
    file: &Value,
    base_files: &BTreeMap<String, String>,
    spans: &BTreeMap<String, Vec<EntitySpan>>,
    names: &BTreeMap<String, String>,
    relations: &[Value],
) -> Result<Value> {
    let path = file["path"].as_str().unwrap_or("").to_string();
    let state = file["state"].as_str().unwrap_or("changed");
    let base_digest = base_files.get(&path).cloned();
    let current_digest = file["current_digest"].as_str().map(str::to_string);
    let mut entry = json!({
        "path": path,
        "state": state,
        "base_snapshot_id": base_snapshot_id,
        "base_digest": base_digest,
        "current_digest": current_digest,
        "entities": [],
        "line_changes": [],
        "base_side": "base",
        "current_side": "candidate",
        "diff_basis": "snapshot_bytes_vs_workspace_text_lines",
        "verification": {
            "state": "not_run",
            "reason": "static diff review; no build, test or runtime command was executed",
        },
    });
    if state == "added" {
        entry["entities"] = json!([]);
        entry["coverage"] = json!({
            "state": "partial",
            "reason": "the file has no base-snapshot entity anchors",
        });
        entry["confidence"] = json!("no_base_entity_mapping");
        entry["trigger"] = json!("file_added_no_base_anchor");
        entry["suggested_verification"] = json!(
            "run the repository's build/tests for this new path; no command was executed"
        );
        return Ok(entry);
    }
    let Some(base_digest) = base_digest else {
        entry["coverage"] = json!({"state":"partial","reason":"base digest is missing"});
        entry["confidence"] = json!("unknown");
        return Ok(entry);
    };
    let Some(base_bytes) = store.read_blob_bytes(&base_digest)? else {
        entry["coverage"] = json!({"state":"partial","reason":"base blob is missing from the CAS"});
        entry["confidence"] = json!("unknown");
        return Ok(entry);
    };
    let base_text = match std::str::from_utf8(&base_bytes) {
        Ok(text) => text,
        Err(_) => {
            entry["coverage"] = json!({"state":"unsupported","reason":"base blob is not UTF-8"});
            entry["confidence"] = json!("unsupported_text");
            return Ok(entry);
        }
    };
    let current_bytes = if state == "deleted" {
        Vec::new()
    } else {
        match read_current(root, &path) {
            Ok(bytes) => bytes,
            Err(error) => {
                entry["coverage"] = json!({"state":"unsupported","reason":error.to_string()});
                entry["confidence"] = json!("unsupported_text");
                return Ok(entry);
            }
        }
    };
    let changes = if state == "deleted" {
        vec![LineChange {
            kind: "delete",
            old_start: 1,
            old_len: base_text.lines().count().max(1) as u64,
            new_start: 1,
            new_len: 0,
        }]
    } else {
        let current_text = match std::str::from_utf8(&current_bytes) {
            Ok(text) => text,
            Err(_) => {
                entry["coverage"] = json!({"state":"unsupported","reason":"current file is not UTF-8"});
                entry["confidence"] = json!("unsupported_text");
                return Ok(entry);
            }
        };
        line_changes(base_text, current_text)
    };
    entry["line_changes"] = json!(changes.iter().copied().map(LineChange::to_json).collect::<Vec<_>>());
    if state != "deleted" {
        entry["current_digest"] = json!(hash::digest_bytes(&current_bytes));
    }
    let mut entities = Vec::new();
    let mut changed_ids = BTreeSet::new();
    for span in spans.get(&path).into_iter().flatten() {
        let overlapping: Vec<Value> = changes
            .iter()
            .filter(|change| change.overlaps(span))
            .map(|change| change.to_json())
            .collect();
        let entity_state = if state == "deleted" {
            "deleted"
        } else if !overlapping.is_empty() {
            "changed"
        } else {
            "context_only"
        };
        if entity_state == "changed" {
            changed_ids.insert(span.entity_id.clone());
        }
        entities.push(json!({
            "entity_id": span.entity_id,
            "qualified_name": span.qualified_name,
            "kind": span.kind,
            "line_start": span.start,
            "line_end": span.end,
            "side": "base",
            "state": entity_state,
            "changed_lines": overlapping,
            "trigger": if state == "deleted" {
                "file_deleted"
            } else if entity_state == "changed" {
                "changed_lines_overlap_entity_anchor"
            } else {
                "file_changed_but_entity_anchor_untouched"
            },
            "confidence": if entity_state == "changed" {
                "static_line_overlap"
            } else {
                "file_changed_entity_range_not_touched"
            },
            "impact": relation_impact(relations, &span.entity_id, names),
            "suggested_verification": "run the repository's build/tests for this path; no command was executed",
            "verification": {
                "state": "not_run",
                "reason": "impact is static graph adjacency; run the repository's own checks",
            },
        }));
    }
    entry["entities"] = json!(entities);
    entry["coverage"] = json!({
        "state": if entities.is_empty() { "partial" } else { "complete" },
        "reason": if entities.is_empty() {
            "no base-snapshot entity anchor matched this file"
        } else {
            "all base entities with anchors in this file were evaluated"
        },
    });
    entry["confidence"] = json!(if entities.is_empty() {
        "file_changed_no_entity_mapping"
    } else if changed_ids.is_empty() {
        "file_changed_entity_ranges_not_touched"
    } else {
        "static_line_overlap"
    });
    Ok(entry)
}

fn manifest_tree_digest(files: &[Value]) -> Option<String> {
    let entries: Vec<(String, String)> = files
        .iter()
        .filter_map(|file| {
            let path = file["path"].as_str()?.to_string();
            let digest = file["blob_digest"].as_str()?.to_string();
            Some((path, digest))
        })
        .collect();
    crate::snapshot::content_tree_digest(&entries).ok()
}

fn snapshot_provenance(
    store: &Store,
    project_id: &str,
    snapshot_id: &str,
) -> Result<Value> {
    let payload = store
        .snapshot_payload(project_id, snapshot_id)?
        .ok_or_else(|| CoreError::not_found(format!("snapshot {snapshot_id} is missing")))?;
    let files = payload["files"].as_array().cloned().unwrap_or_default();
    let analyser = store
        .latest_index_run(project_id, snapshot_id)?
        .map(|run| {
            json!({
                "cli_commit": run["cli_commit"],
                "cli_dirty": run["cli_dirty"],
                "helper_commit": run["helper_commit"],
                "helper_dirty": run["helper_dirty"],
                "export_schema": run["export_schema"],
                "index_config": run["index_config"],
                "index_mode": run["index_mode"],
                "files_reused": run["files_reused"],
                "files_deleted": run["files_deleted"],
            })
        });
    Ok(json!({
        "snapshot_id": snapshot_id,
        "file_count": files.len(),
        "git_commit": payload["git_commit"],
        "head_tree_oid": Value::Null,
        "head_tree_scope": Value::Null,
        "head_tree_basis": "unavailable_no_workspace",
        "workspace_subdir": Value::Null,
        "git_tree_digest": payload["git_tree_digest"],
        "content_tree_digest": manifest_tree_digest(&files),
        "config_digest": payload["config_digest"],
        "environment_digest": payload["environment_digest"],
        "exclusions_digest": payload["exclusions_digest"],
        "workspace_policy": payload["workspace_policy"],
        "analyser": analyser,
    }))
}

fn invalidation_reasons(base: &Value, candidate: &Value) -> Vec<Value> {
    let mut reasons = Vec::new();
    for (field, reason) in [
        ("config_digest", "config_changed"),
        ("environment_digest", "environment_changed"),
        ("exclusions_digest", "scope_changed"),
        ("workspace_policy", "scope_changed"),
        ("analyser", "analyser_changed"),
        ("content_tree_digest", "tree_changed"),
    ] {
        if base.get(field) != candidate.get(field) {
            reasons.push(json!({
                "reason": reason,
                "field": field,
                "base": base.get(field).cloned().unwrap_or(Value::Null),
                "candidate": candidate.get(field).cloned().unwrap_or(Value::Null),
            }));
        }
    }
    reasons
}

fn identity_changes(base: &Value, candidate: &Value) -> Vec<Value> {
    let mut changes = Vec::new();
    for field in [
        "config_digest",
        "environment_digest",
        "exclusions_digest",
        "workspace_policy",
        "git_tree_digest",
        "content_tree_digest",
        "analyser",
    ] {
        if base.get(field) != candidate.get(field) {
            changes.push(json!({
                "field": field,
                "base": base.get(field).cloned().unwrap_or(Value::Null),
                "candidate": candidate.get(field).cloned().unwrap_or(Value::Null),
            }));
        }
    }
    changes
}

fn moved_review(
    base_snapshot_id: &str,
    root: &Path,
    moved: &Value,
    spans: &BTreeMap<String, Vec<EntitySpan>>,
    names: &BTreeMap<String, String>,
    relations: &[Value],
) -> Result<Value> {
    let from = moved["from"].as_str().unwrap_or("");
    let to = moved["to"].as_str().unwrap_or("");
    let base_digest = moved["base_digest"].as_str();
    let current_digest = moved["current_digest"].as_str();
    let mut entry = json!({
        "path": to,
        "from": from,
        "state": "moved",
        "base_snapshot_id": base_snapshot_id,
        "base_digest": base_digest,
        "current_digest": current_digest,
        "entities": [],
        "line_changes": [],
        "base_side": "base",
        "current_side": "candidate",
        "diff_basis": "content_identical_move",
        "trigger": "file_moved_same_blob",
        "confidence": "content_identical_move",
        "verification": {
            "state": "not_run",
            "reason": "static move pairing; no build, test or runtime command was executed",
        },
    });
    let bytes = match read_current(root, to) {
        Ok(bytes) => bytes,
        Err(error) => {
            entry["coverage"] = json!({"state":"unsupported","reason":error.to_string()});
            entry["confidence"] = json!("unsupported_text");
            return Ok(entry);
        }
    };
    if current_digest.is_some_and(|digest| hash::digest_bytes(&bytes) != digest) {
        entry["coverage"] = json!({
            "state": "partial",
            "reason": "moved file changed after the drift scan",
        });
        entry["confidence"] = json!("move_not_verified");
        return Ok(entry);
    }
    let mut entities = Vec::new();
    for span in spans.get(from).into_iter().flatten() {
        entities.push(json!({
            "entity_id": span.entity_id,
            "qualified_name": span.qualified_name,
            "kind": span.kind,
            "line_start": span.start,
            "line_end": span.end,
            "side": "base",
            "state": "context_only",
            "changed_lines": [],
            "trigger": "file_moved_same_blob",
            "confidence": "content_identical_move",
            "impact": relation_impact(relations, &span.entity_id, names),
            "suggested_verification": "verify that callers use the moved path; no command was executed",
            "verification": {
                "state": "not_run",
                "reason": "static move pairing; no repository command was executed",
            },
        }));
    }
    entry["entities"] = json!(entities);
    entry["coverage"] = json!({
        "state": if entities.is_empty() { "partial" } else { "complete" },
        "reason": if entities.is_empty() {
            "moved file has no base entity anchor"
        } else {
            "base anchors were carried across the move as unchanged context"
        },
    });
    Ok(entry)
}

/// Review the current workspace against a frozen snapshot without executing
/// any repository command.
pub fn review_workspace_diff(
    store: &Store,
    project_id: &str,
    base_snapshot_id: &str,
    root: &Path,
) -> Result<Value> {
    let started = Instant::now();
    validate::require_id("project_id", project_id)?;
    validate::require_digest("snapshot_id", base_snapshot_id)?;
    if !root.is_dir() {
        return Err(CoreError::invalid(format!(
            "workspace {} is not a directory",
            root.display()
        )));
    }
    store.check_snapshot_scope(project_id, base_snapshot_id)?;
    let drift = analyze_drift(store, project_id, base_snapshot_id, root, None)?;
    let base_files = snapshot_files(store, project_id, base_snapshot_id)?;
    let (spans, names) = entity_spans(store, project_id, base_snapshot_id)?;
    let relations = store.relation_payloads(project_id, base_snapshot_id)?;
    let mut files = Vec::new();
    let mut summary_files = BTreeMap::new();
    for key in ["changed", "added", "deleted"] {
        let count = drift[key].as_array().map(Vec::len).unwrap_or(0);
        summary_files.insert(key, count);
        for file in drift[key].as_array().into_iter().flatten() {
            files.push(file_review(
                store,
                base_snapshot_id,
                root,
                file,
                &base_files,
                &spans,
                &names,
                &relations,
            )?);
        }
    }
    summary_files.insert(
        "moved",
        drift["moved"].as_array().map(Vec::len).unwrap_or(0),
    );
    for moved in drift["moved"].as_array().into_iter().flatten() {
        files.push(moved_review(
            base_snapshot_id,
            root,
            moved,
            &spans,
            &names,
            &relations,
        )?);
    }
    let mut affected = BTreeSet::new();
    let mut unmapped = Vec::new();
    let mut unsupported = Vec::new();
    let mut partial = Vec::new();
    let omitted_base: Vec<Value> = drift["omitted_base"]
        .as_array()
        .cloned()
        .unwrap_or_default();
    for path in &omitted_base {
        if let Some(path) = path["path"].as_str() {
            partial.push(json!(path));
        }
    }
    for file in &files {
        for entity in file["entities"].as_array().into_iter().flatten() {
            if entity["state"] == "changed" || entity["state"] == "deleted" {
                if let Some(id) = entity["entity_id"].as_str() {
                    affected.insert(id.to_string());
                }
            }
        }
        if matches!(
            file["confidence"].as_str(),
            Some("file_changed_no_entity_mapping") | Some("no_base_entity_mapping")
        ) {
            unmapped.push(file["path"].clone());
        }
        if file["coverage"]["state"] == "unsupported" {
            unsupported.push(file["path"].clone());
        }
        if file["coverage"]["state"] == "partial" {
            partial.push(file["path"].clone());
        }
    }
    partial.sort_by(|left, right| {
        left.as_str()
            .unwrap_or("")
            .cmp(right.as_str().unwrap_or(""))
    });
    partial.dedup();

    let mut base = snapshot_provenance(store, project_id, base_snapshot_id)?;
    if let Some(commit) = base["git_commit"].as_str() {
        match git_workspace_tree(root, commit) {
            Some((oid, prefix)) => {
                base["head_tree_oid"] = json!(oid);
                base["head_tree_scope"] = json!(if prefix.is_empty() {
                    "repository"
                } else {
                    "workspace_subtree"
                });
                base["workspace_subdir"] = json!(prefix);
                base["head_tree_basis"] = json!("workspace_git");
            }
            None => {
                base["head_tree_basis"] = json!("unavailable_not_git");
            }
        }
    } else {
        base["head_tree_basis"] = json!("unavailable_not_git");
    }
    let git = &drift["git"];
    let git_available = git["is_repository"].as_bool() == Some(true);
    let candidate = json!({
        "kind": "workspace",
        "git_state": if git_available { "repository" } else { "unavailable" },
        "git_commit": if git_available { git["commit"].clone() } else { Value::Null },
        "head_tree_oid": if git_available { git["head_tree_oid"].clone() } else { Value::Null },
        "commit_tree_oid": if git_available { git["head_tree_oid"].clone() } else { Value::Null },
        "head_tree_scope": if git_available { git["head_tree_scope"].clone() } else { json!("unavailable") },
        "head_tree_basis": if git_available { json!("workspace_git") } else { json!("unavailable_not_git") },
        "workspace_subdir": if git_available { git["workspace_subdir"].clone() } else { Value::Null },
        "dirty": if git_available { json!(git["dirty"].as_bool().unwrap_or(false)) } else { Value::Null },
        "dirty_paths": if git_available { git["dirty_paths_count"].clone() } else { Value::Null },
        "content_tree_digest": drift["coverage"]["content_tree_digest"],
        "durable": false,
    });
    let tree_equivalent = match (
        base["content_tree_digest"].as_str(),
        candidate["content_tree_digest"].as_str(),
    ) {
        (Some(base), Some(candidate)) => base == candidate,
        _ => false,
    };
    let mut limitations = vec![
        "line overlap is static text evidence, not semantic equivalence".to_string(),
        "entity impact is adjacency in the base snapshot; new calls are not inferred".to_string(),
        "added files have no base entity mapping".to_string(),
        "base files present but omitted from the eligible scan are reported as omitted_base, not deleted".to_string(),
        "no repository command was executed".to_string(),
    ];
    if base["analyser"].is_null() {
        limitations.push(
            "base snapshot has no index-run analyser provenance; analyser=null, not the current build"
                .to_string(),
        );
    }
    if !git_available {
        limitations.push(
            "workspace is not a git repository; git commit/tree/dirty provenance is unavailable"
                .to_string(),
        );
    }
    if let Some(subdir) = candidate["workspace_subdir"].as_str() {
        if !subdir.is_empty() {
            limitations.push(format!(
                "workspace is a repository subdirectory ({subdir}); head_tree_oid is the workspace subtree, not the repository root tree"
            ));
        }
    }
    Ok(json!({
        "schema_version": "1.0",
        "project_id": project_id,
        "base_snapshot_id": base_snapshot_id,
        "workspace": root.to_string_lossy(),
        "state": drift["state"],
        "provenance": {
            "base": base,
            "candidate": candidate,
            "tree_equivalent": tree_equivalent,
            "durability": "base_snapshot_immutable; candidate_workspace_not_frozen",
        },
        "timings": {"total_ms": started.elapsed().as_millis() as u64},
        "summary": {
            "files": {
                "changed": summary_files.get("changed").copied().unwrap_or(0),
                "added": summary_files.get("added").copied().unwrap_or(0),
                "deleted": summary_files.get("deleted").copied().unwrap_or(0),
                "moved": summary_files.get("moved").copied().unwrap_or(0),
            },
            "affected_entities": affected.len(),
            "unmapped_files": unmapped.len(),
            "unsupported_files": unsupported.len(),
            "omitted_base_files": omitted_base.len(),
            "needs_review": drift["needs_review"].as_bool().unwrap_or(false),
        },
        "files": files,
        "affected_entities": drift["affected_entities"],
        "affected_relations": drift["affected_relations"],
        "claims_at_risk": drift["affected_claims"],
        "architectures_at_risk": drift["affected_architectures"],
        "plans_at_risk": drift["stale_plans"],
        "changesets_at_risk": drift["stale_changesets"],
        "verification": {
            "state": "not_run",
            "reason": "this review executed no build, test, scenario or runtime command",
        },
        "coverage": {
            "state": if unsupported.is_empty() && unmapped.is_empty() && partial.is_empty() { "complete" } else { "partial" },
            "scope": "workspace bytes vs base snapshot bytes, with base-snapshot entity anchors",
            "unmapped_files": unmapped,
            "unsupported_files": unsupported,
            "omitted_base_files": omitted_base,
            "partial_files": partial,
            "limitations": limitations,
        },
        "source_mutation": "none",
        "history_mutation": "none",
    }))
}

/// Freeze the changed/added candidate bytes into the CAS and pin each object.
/// The base snapshot is already immutable; this makes the candidate citations
/// replayable without creating a new snapshot or advancing `latest_snapshot_id`.
pub fn freeze_review_candidate(
    store: &mut Store,
    root: &Path,
    review: &mut Value,
) -> Result<Value> {
    let project_id = review["project_id"]
        .as_str()
        .ok_or_else(|| CoreError::invalid("review has no project_id"))?
        .to_string();
    let expected_files = review["files"]
        .as_array()
        .into_iter()
        .flatten()
        .filter(|file| matches!(file["state"].as_str(), Some("changed") | Some("added")))
        .count();
    let mut files = Vec::new();
    let mut limitations = Vec::new();
    let mut bytes_frozen = 0u64;
    let mut truncated = false;
    for file in review["files"].as_array().into_iter().flatten() {
        let state = file["state"].as_str().unwrap_or("");
        if !matches!(state, "changed" | "added") {
            continue;
        }
        if files.len() >= MAX_LIST {
            truncated = true;
            limitations.push("freeze stopped at the file cap".to_string());
            break;
        }
        let path = file["path"].as_str().unwrap_or("");
        let bytes = match read_current(root, path) {
            Ok(bytes) => bytes,
            Err(error) => {
                limitations.push(format!("{path}: {error}"));
                continue;
            }
        };
        let actual_digest = hash::digest_bytes(&bytes);
        if file["current_digest"].as_str() != Some(actual_digest.as_str()) {
            limitations.push(format!(
                "{path}: bytes changed after the review; not frozen"
            ));
            continue;
        }
        if bytes_frozen.saturating_add(bytes.len() as u64) > MAX_FREEZE_BYTES {
            truncated = true;
            limitations.push(format!(
                "freeze stopped at the {MAX_FREEZE_BYTES} byte cap"
            ));
            break;
        }
        let media_type = "text/plain";
        let digest = store.put_blob(&bytes, media_type)?;
        bytes_frozen += bytes.len() as u64;
        let pin_id = crate::store::freeze_pin_id(&digest);
        crate::drift::pin(
            store,
            &json!({
                "project_id": project_id,
                "pin_id": pin_id,
                "kind": "object",
                "target": digest,
            }),
        )?;
        files.push(json!({
            "path": path,
            "blob_digest": digest,
            "size_bytes": bytes.len(),
            "state": state,
        }));
    }
    let durable = expected_files > 0
        && !truncated
        && limitations.is_empty()
        && files.len() == expected_files;
    let frozen = json!({
        "state": if durable { "frozen" } else { "partial" },
        "files": files,
        "expected_files": expected_files,
        "bytes": bytes_frozen,
        "byte_cap": MAX_FREEZE_BYTES,
        "truncated": truncated,
        "durable": durable,
        "limitations": limitations,
    });
    if let Some(candidate) = review
        .get_mut("provenance")
        .and_then(|provenance| provenance.get_mut("candidate"))
    {
        candidate["durable"] = json!(durable);
        candidate["frozen"] = frozen.clone();
        candidate["durability"] = if durable {
            json!("candidate_bytes_frozen_in_cas")
        } else {
            json!("candidate_bytes_partially_frozen")
        };
    }
    if let Some(provenance) = review.get_mut("provenance") {
        provenance["durability"] = json!("base_snapshot_immutable; candidate_bytes_frozen");
    }
    Ok(frozen)
}

/// Persist a review/structural-diff result and pin every snapshot it cites in
/// the same transaction. A failed pin rolls the report back instead of leaving
/// readable evidence that retention cannot keep alive.
pub fn persist_review_report(
    store: &mut Store,
    project_id: &str,
    base_snapshot_id: &str,
    candidate_snapshot_id: Option<&str>,
    mode: &str,
    report: &Value,
) -> Result<Value> {
    let mut pins = vec![("base".to_string(), base_snapshot_id.to_string())];
    if let Some(candidate) = candidate_snapshot_id {
        pins.push(("candidate".to_string(), candidate.to_string()));
    }
    let stored = store.insert_review_report(
        project_id,
        base_snapshot_id,
        candidate_snapshot_id,
        mode,
        report,
        &pins,
    )?;
    let report_id = stored["report_id"]
        .as_str()
        .ok_or_else(|| CoreError::Storage("review report id missing".into()))?
        .to_string();
    Ok(json!({
        "stored": stored,
        "pins": pins
            .iter()
            .map(|(suffix, target)| json!({
                "pin_id": format!("{report_id}.{suffix}"),
                "kind": "snapshot",
                "target": target,
            }))
            .collect::<Vec<_>>(),
        "read_with": {"cli": format!("archmeld review-diff --report {report_id}")},
    }))
}

fn capped<T: Clone>(items: &[T], value: impl Fn(&T) -> Value) -> Value {
    json!({
        "items": items.iter().take(MAX_LIST).map(value).collect::<Vec<_>>(),
        "count": items.len(),
        "truncated": items.len() > MAX_LIST,
    })
}

fn empty_capped() -> Value {
    capped(&Vec::<Value>::new(), |value| value.clone())
}

fn empty_structural() -> Value {
    json!({
        "dependencies": {"added": empty_capped(), "removed": empty_capped()},
        "ownership": {
            "added": empty_capped(),
            "removed": empty_capped(),
            "basis": "relation_edges",
            "coverage": "none",
        },
        "entry_points": {"added": [], "removed": [], "basis": "unavailable"},
        "paths": {
            "convergence": [],
            "coverage": "not_run",
            "limitations": ["the two snapshot ids are identical; no structural comparison was run"],
        },
    })
}

type StructuralKey = (String, String, String, String);

fn entity_names(entities: &BTreeMap<String, Value>) -> BTreeMap<String, String> {
    entities
        .iter()
        .map(|(id, entity)| {
            (
                id.clone(),
                entity["qualified_name"]
                    .as_str()
                    .unwrap_or(id)
                    .to_string(),
            )
        })
        .collect()
}

fn structural_relations(
    relations: &[Value],
    names: &BTreeMap<String, String>,
) -> BTreeMap<StructuralKey, Vec<Value>> {
    let mut grouped: BTreeMap<StructuralKey, Vec<Value>> = BTreeMap::new();
    for relation in relations {
        let (Some(source_id), Some(target_id), Some(kind), Some(resolution)) = (
            relation["source_id"].as_str(),
            relation["target_id"].as_str(),
            relation["kind"].as_str(),
            relation["resolution"].as_str(),
        ) else {
            continue;
        };
        let source = names
            .get(source_id)
            .cloned()
            .unwrap_or_else(|| source_id.to_string());
        let target = names
            .get(target_id)
            .cloned()
            .unwrap_or_else(|| target_id.to_string());
        grouped
            .entry((
                source.clone(),
                target.clone(),
                kind.to_string(),
                resolution.to_string(),
            ))
            .or_default()
            .push(json!({
                "relation_id": relation["id"],
                "source_id": source_id,
                "target_id": target_id,
                "source": source,
                "target": target,
                "kind": kind,
                "resolution": resolution,
                "basis": relation["basis"],
                "evidence_ids": relation["evidence_ids"],
                "call_site": relation["call_site"],
            }));
    }
    grouped
}

fn path_relation_allowed(kind: &str) -> bool {
    matches!(
        kind,
        "static_call_candidate"
            | "resolved_call"
            | "observed_call"
            | "imports"
            | "references"
            | "implements"
            | "owns"
            | "reads"
            | "writes"
            | "publishes"
            | "consumes"
            | "awaits"
            | "schedules"
            | "communicates_with"
    )
}

fn structural_paths(
    relations: &[Value],
    seeds: &BTreeSet<String>,
    names: &BTreeMap<String, String>,
) -> (Vec<Value>, bool) {
    if seeds.is_empty() {
        return (Vec::new(), false);
    }
    let mut adjacency: BTreeMap<String, Vec<&Value>> = BTreeMap::new();
    for relation in relations {
        let (Some(source), Some(_target), Some(kind)) = (
            relation["source_id"].as_str(),
            relation["target_id"].as_str(),
            relation["kind"].as_str(),
        ) else {
            continue;
        };
        if path_relation_allowed(kind) {
            adjacency.entry(source.to_string()).or_default().push(relation);
        }
    }

    let mut by_target: BTreeMap<String, Vec<(Vec<String>, Vec<String>, Vec<String>)>> =
        BTreeMap::new();
    let mut seen_paths: BTreeSet<String> = BTreeSet::new();
    let mut stack: Vec<(String, Vec<String>, Vec<String>, Vec<String>)> = seeds
        .iter()
        .map(|seed| (seed.clone(), vec![seed.clone()], Vec::new(), Vec::new()))
        .collect();
    let mut expansions = 0usize;
    let mut capped = false;
    while let Some((node, nodes, relation_ids, resolutions)) = stack.pop() {
        if nodes.len() >= MAX_PATH_DEPTH {
            continue;
        }
        for relation in adjacency.get(&node).into_iter().flatten() {
            expansions += 1;
            if expansions > MAX_PATH_EXPANSIONS {
                capped = true;
                break;
            }
            let target = relation["target_id"].as_str().unwrap_or("");
            if target.is_empty() || nodes.iter().any(|node| node == target) {
                continue;
            }
            let relation_id = relation["id"].as_str().unwrap_or("").to_string();
            let resolution = relation["resolution"].as_str().unwrap_or("unknown").to_string();
            let mut next_nodes = nodes.clone();
            next_nodes.push(target.to_string());
            let mut next_relations = relation_ids.clone();
            next_relations.push(relation_id);
            let mut next_resolutions = resolutions.clone();
            next_resolutions.push(resolution);
            if next_nodes.len() >= 2 {
                let signature = next_nodes.join("->");
                if seen_paths.insert(signature) {
                    by_target
                        .entry(target.to_string())
                        .or_default()
                        .push((
                            next_nodes.clone(),
                            next_relations.clone(),
                            next_resolutions.clone(),
                        ));
                }
            }
            if next_nodes.len() < MAX_PATH_DEPTH {
                stack.push((target.to_string(), next_nodes, next_relations, next_resolutions));
            }
        }
        if capped {
            break;
        }
    }

    let mut convergence = Vec::new();
    for (target, mut paths) in by_target {
        if paths.len() < 2 {
            continue;
        }
        paths.sort_by(|left, right| left.0.len().cmp(&right.0.len()));
        paths.truncate(3);
        let hypothesis = paths
            .iter()
            .any(|(_, _, resolutions)| resolutions.iter().any(|value| value == "candidate"));
        let path_items: Vec<Value> = paths
            .into_iter()
            .map(|(nodes, relation_ids, resolutions)| {
                json!({
                    "nodes": nodes
                        .iter()
                        .map(|id| names.get(id).cloned().unwrap_or_else(|| id.clone()))
                        .collect::<Vec<_>>(),
                    "node_ids": nodes,
                    "relation_ids": relation_ids,
                    "resolutions": resolutions,
                })
            })
            .collect();
        convergence.push(json!({
            "target": names.get(&target).cloned().unwrap_or_else(|| target.clone()),
            "target_id": target,
            "paths": path_items,
            "hypothesis": hypothesis,
            "basis": "bounded_static_relation_paths",
        }));
        if convergence.len() >= MAX_STRUCTURAL_PATHS {
            capped = true;
            break;
        }
    }
    (convergence, capped)
}

/// Exact snapshot-to-snapshot difference. This is used by the refresh command;
/// it does not read the workspace and does not execute anything.
pub fn diff_snapshots(
    store: &Store,
    project_id: &str,
    base_snapshot_id: &str,
    new_snapshot_id: &str,
) -> Result<Value> {
    let started = Instant::now();
    validate::require_id("project_id", project_id)?;
    validate::require_digest("base_snapshot_id", base_snapshot_id)?;
    validate::require_digest("new_snapshot_id", new_snapshot_id)?;
    store.check_snapshot_scope(project_id, base_snapshot_id)?;
    store.check_snapshot_scope(project_id, new_snapshot_id)?;
    let base_provenance = snapshot_provenance(store, project_id, base_snapshot_id)?;
    let new_provenance = snapshot_provenance(store, project_id, new_snapshot_id)?;
    let tree_equivalent = match (
        base_provenance["content_tree_digest"].as_str(),
        new_provenance["content_tree_digest"].as_str(),
    ) {
        (Some(base), Some(new)) => base == new,
        _ => false,
    };
    let identity_changes = identity_changes(&base_provenance, &new_provenance);
    let base_file_count = base_provenance["file_count"].clone();
    let base_index_mode = if base_provenance["analyser"].is_object() {
        base_provenance["analyser"]["index_mode"]
            .as_str()
            .unwrap_or("full_parse")
            .to_string()
    } else {
        "not_indexed".to_string()
    };
    if base_snapshot_id == new_snapshot_id {
        return Ok(identical_diff(
            base_snapshot_id,
            new_snapshot_id,
            base_provenance,
            new_provenance,
            tree_equivalent,
            identity_changes,
            base_file_count,
            &base_index_mode,
            started,
        ));
    }
    let diff = file_diff(
        store,
        project_id,
        base_snapshot_id,
        new_snapshot_id,
        &base_provenance,
        &new_provenance,
    )?;
    let base_entities = entity_payloads(store, project_id, base_snapshot_id)?;
    let new_entities = entity_payloads(store, project_id, new_snapshot_id)?;
    let (entities_changed, entities_added, entities_deleted) =
        entity_diff(&base_entities, &new_entities);
    let base_relations = store.relation_payloads(project_id, base_snapshot_id)?;
    let new_relations = store.relation_payloads(project_id, new_snapshot_id)?;
    let (relations_added, relations_deleted) = relation_diff(&base_relations, &new_relations);
    let structural = structural_diff(
        &base_entities,
        &new_entities,
        &base_relations,
        &new_relations,
        &entities_changed,
        &entities_added,
    );
    let files_changed = diff.files_changed;
    let files_added = diff.files_added;
    let files_deleted = diff.files_deleted;
    let moved = diff.moved;
    let reuse = diff.reuse;
    let invalidation = invalidation_reasons(&base_provenance, &new_provenance);
    let dependencies_added = structural.dependencies_added;
    let dependencies_removed = structural.dependencies_removed;
    let ownership_added = structural.ownership_added;
    let ownership_removed = structural.ownership_removed;
    let convergence = structural.convergence;
    let structural_limitations = structural.structural_limitations;
    Ok(build_snapshot_diff(SnapshotDiff {
        base_snapshot_id,
        new_snapshot_id,
        base_provenance,
        new_provenance,
        tree_equivalent,
        identity_changes,
        invalidation,
        reuse,
        files_changed,
        files_added,
        files_deleted,
        moved,
        entities_changed,
        entities_added,
        entities_deleted,
        relations_added,
        relations_deleted,
        dependencies_added,
        dependencies_removed,
        ownership_added,
        ownership_removed,
        convergence,
        structural_limitations,
        started,
    }))
}

struct FileDiff {
    files_changed: Vec<Value>,
    files_added: Vec<Value>,
    files_deleted: Vec<Value>,
    moved: Vec<Value>,
    reuse: Value,
}

fn identical_diff(
    base_snapshot_id: &str,
    new_snapshot_id: &str,
    base_provenance: Value,
    new_provenance: Value,
    tree_equivalent: bool,
    identity_changes: Vec<Value>,
    base_file_count: Value,
    base_index_mode: &str,
    started: Instant,
) -> Value {
        json!({
            "schema_version": "1.0",
            "base_snapshot_id": base_snapshot_id,
            "new_snapshot_id": new_snapshot_id,
            "state": "unchanged",
            "provenance": {
                "base": base_provenance,
                "candidate": new_provenance,
                "tree_equivalent": tree_equivalent,
                "durability": "both snapshots are immutable ledger records",
            },
            "identity_changes": identity_changes,
            "invalidation": invalidation_reasons(&base_provenance, &new_provenance),
            "reuse": {
                "files_unchanged": base_file_count,
                "files_changed": 0,
                "files_added": 0,
                "files_deleted": 0,
                "files_moved": 0,
                "blobs_reused": base_file_count,
                "blobs_new": 0,
                "index_mode": base_index_mode,
            },
            "timings": {"total_ms": started.elapsed().as_millis() as u64},
            "summary": {
                "files": {"changed": 0, "added": 0, "deleted": 0, "moved": 0},
                "entities": {"changed": 0, "added": 0, "deleted": 0},
                "relations": {"added": 0, "deleted": 0},
            },
            "files": {
                "changed": empty_capped(),
                "added": empty_capped(),
                "deleted": empty_capped(),
                "moved": empty_capped(),
            },
            "structural": empty_structural(),
            "entities": {
                "changed": empty_capped(),
                "added": empty_capped(),
                "deleted": empty_capped(),
            },
            "relations": {
                "added": empty_capped(),
                "deleted": empty_capped(),
            },
            "verification": {
                "state": "not_run",
                "reason": "snapshot comparison; no build, test or runtime command was executed",
            },
            "coverage": {
                "state": "complete",
                "scope": "two registered snapshots; content-addressed files, entities and relations",
                "limitations": [
                    "entity changed means same kind+qualified name but a different content-derived id",
                    "relations are compared by relation id; a line-only change can appear as delete+add",
                ],
            },
        })
}

fn file_diff(
    store: &Store,
    project_id: &str,
    base_snapshot_id: &str,
    new_snapshot_id: &str,
    base_provenance: &Value,
    new_provenance: &Value,
) -> Result<FileDiff> {
    let base_files = snapshot_files(store, project_id, base_snapshot_id)?;
    let new_files = snapshot_files(store, project_id, new_snapshot_id)?;
    let mut files_changed = Vec::new();
    let mut files_added = Vec::new();
    let mut files_deleted = Vec::new();
    for (path, digest) in &base_files {
        match new_files.get(path) {
            Some(new_digest) if new_digest == digest => {}
            Some(new_digest) => files_changed.push(json!({
                "path": path,
                "base_digest": digest,
                "new_digest": new_digest,
            })),
            None => files_deleted.push(json!({"path": path, "base_digest": digest})),
        }
    }
    for (path, digest) in &new_files {
        if !base_files.contains_key(path) {
            files_added.push(json!({"path": path, "new_digest": digest}));
        }
    }
    let mut moved = Vec::new();
    let mut remaining_deleted = Vec::new();
    let deleted_digests: BTreeMap<String, usize> = files_deleted
        .iter()
        .filter_map(|file| file["base_digest"].as_str().map(str::to_string))
        .fold(BTreeMap::new(), |mut counts, digest| {
            *counts.entry(digest).or_insert(0) += 1;
            counts
        });
    let added_digests: BTreeMap<String, usize> = files_added
        .iter()
        .filter_map(|file| file["new_digest"].as_str().map(str::to_string))
        .fold(BTreeMap::new(), |mut counts, digest| {
            *counts.entry(digest).or_insert(0) += 1;
            counts
        });
    for deleted in files_deleted.drain(..) {
        let digest = deleted["base_digest"].clone();
        let digest_text = digest.as_str().unwrap_or("");
        let unique = deleted_digests.get(digest_text) == Some(&1)
            && added_digests.get(digest_text) == Some(&1);
        if unique {
            if let Some(position) = files_added
                .iter()
                .position(|added| added["new_digest"] == digest)
            {
                let added = files_added.remove(position);
                moved.push(json!({
                    "from": deleted["path"],
                    "to": added["path"],
                    "digest": digest,
                }));
                continue;
            }
        }
        let mut file = deleted;
        if added_digests.contains_key(digest_text) {
            file["ambiguous_move"] = json!(true);
            file["ambiguous_digest"] = digest;
        }
        remaining_deleted.push(file);
    }
    for added in files_added.iter_mut() {
        let digest = added["new_digest"].as_str().unwrap_or("");
        if deleted_digests.get(digest).is_some_and(|count| *count > 1)
            || added_digests.get(digest).is_some_and(|count| *count > 1)
        {
            added["ambiguous_move"] = json!(true);
            added["ambiguous_digest"] = added["new_digest"].clone();
        }
    }
    files_deleted = remaining_deleted;
    let files_unchanged = base_files
        .iter()
        .filter(|(path, digest)| new_files.get(*path) == Some(*digest))
        .count();
    let index_mode = if new_provenance["analyser"].is_object() {
        new_provenance["analyser"]["index_mode"]
            .as_str()
            .unwrap_or("full_parse")
    } else if base_provenance["analyser"].is_object() {
        base_provenance["analyser"]["index_mode"]
            .as_str()
            .unwrap_or("full_parse")
    } else {
        "not_indexed"
    };
    let base_digests: BTreeSet<&str> = base_files.values().map(String::as_str).collect();
    let new_digests: BTreeSet<&str> = new_files.values().map(String::as_str).collect();
    let blobs_reused = base_digests.intersection(&new_digests).count();
    let reuse = json!({
        "files_unchanged": files_unchanged,
        "files_changed": files_changed.len(),
        "files_added": files_added.len(),
        "files_deleted": files_deleted.len(),
        "files_moved": moved.len(),
        "blobs_reused": blobs_reused,
        "blobs_new": new_digests.len().saturating_sub(blobs_reused),
        "index_mode": index_mode,
    });
    Ok(FileDiff {
        files_changed,
        files_added,
        files_deleted,
        moved,
        reuse,
    })
}

fn entity_diff(
    base_entities: &BTreeMap<String, Value>,
    new_entities: &BTreeMap<String, Value>,
) -> (Vec<Value>, Vec<Value>, Vec<Value>) {
    let base_by_name: BTreeMap<(String, String), &Value> = base_entities
        .values()
        .map(|entity| {
            (
                (
                    entity["kind"].as_str().unwrap_or("").to_string(),
                    entity["qualified_name"].as_str().unwrap_or("").to_string(),
                ),
                entity,
            )
        })
        .collect();
    let new_by_name: BTreeMap<(String, String), &Value> = new_entities
        .values()
        .map(|entity| {
            (
                (
                    entity["kind"].as_str().unwrap_or("").to_string(),
                    entity["qualified_name"].as_str().unwrap_or("").to_string(),
                ),
                entity,
            )
        })
        .collect();
    let mut entities_changed = Vec::new();
    let mut entities_added = Vec::new();
    let mut entities_deleted = Vec::new();
    for (key, entity) in &base_by_name {
        match new_by_name.get(key) {
            Some(new) if new["id"] == entity["id"] => {}
            Some(new) => entities_changed.push(json!({
                "kind": key.0,
                "qualified_name": key.1,
                "base_entity_id": entity["id"],
                "new_entity_id": new["id"],
            })),
            None => entities_deleted.push(json!({
                "kind": key.0,
                "qualified_name": key.1,
                "base_entity_id": entity["id"],
            })),
        }
    }
    for (key, entity) in &new_by_name {
        if !base_by_name.contains_key(key) {
            entities_added.push(json!({
                "kind": key.0,
                "qualified_name": key.1,
                "new_entity_id": entity["id"],
            }));
        }
    }
    (entities_changed, entities_added, entities_deleted)
}

fn relation_diff(
    base_relations: &[Value],
    new_relations: &[Value],
) -> (Vec<Value>, Vec<Value>) {
    let base_relation_ids: BTreeSet<&str> = base_relations
        .iter()
        .filter_map(|relation| relation["id"].as_str())
        .collect();
    let new_relation_ids: BTreeSet<&str> = new_relations
        .iter()
        .filter_map(|relation| relation["id"].as_str())
        .collect();
    let relations_added: Vec<Value> = new_relations
        .iter()
        .filter(|relation| {
            relation["id"]
                .as_str()
                .is_some_and(|id| !base_relation_ids.contains(id))
        })
        .map(|relation| {
            json!({
                "relation_id": relation["id"],
                "kind": relation["kind"],
                "source_id": relation["source_id"],
                "target_id": relation["target_id"],
            })
        })
        .collect();
    let relations_deleted: Vec<Value> = base_relations
        .iter()
        .filter(|relation| {
            relation["id"]
                .as_str()
                .is_some_and(|id| !new_relation_ids.contains(id))
        })
        .map(|relation| {
            json!({
                "relation_id": relation["id"],
                "kind": relation["kind"],
                "source_id": relation["source_id"],
                "target_id": relation["target_id"],
            })
        })
        .collect();
    (relations_added, relations_deleted)
}

struct StructuralDiff {
    dependencies_added: Vec<Value>,
    dependencies_removed: Vec<Value>,
    ownership_added: Vec<Value>,
    ownership_removed: Vec<Value>,
    convergence: Vec<Value>,
    structural_limitations: Vec<&'static str>,
}

fn structural_diff(
    base_entities: &BTreeMap<String, Value>,
    new_entities: &BTreeMap<String, Value>,
    base_relations: &[Value],
    new_relations: &[Value],
    entities_changed: &[Value],
    entities_added: &[Value],
) -> StructuralDiff {
    let base_names = entity_names(&base_entities);
    let new_names = entity_names(&new_entities);
    let base_structural = structural_relations(&base_relations, &base_names);
    let new_structural = structural_relations(&new_relations, &new_names);
    let mut dependencies_added = Vec::new();
    let mut dependencies_removed = Vec::new();
    for (key, items) in &new_structural {
        if path_relation_allowed(&key.2) && !base_structural.contains_key(key) {
            dependencies_added.extend(items.iter().cloned());
        }
    }
    for (key, items) in &base_structural {
        if path_relation_allowed(&key.2) && !new_structural.contains_key(key) {
            dependencies_removed.extend(items.iter().cloned());
        }
    }
    let ownership_added: Vec<Value> = dependencies_added
        .iter()
        .filter(|item| matches!(item["kind"].as_str(), Some("owns") | Some("writes")))
        .cloned()
        .collect();
    let ownership_removed: Vec<Value> = dependencies_removed
        .iter()
        .filter(|item| matches!(item["kind"].as_str(), Some("owns") | Some("writes")))
        .cloned()
        .collect();
    let mut seeds = BTreeSet::new();
    for item in entities_changed.iter().chain(entities_added.iter()) {
        if let Some(entity_id) = item["new_entity_id"].as_str() {
            seeds.insert(entity_id.to_string());
        }
    }
    let (convergence, paths_capped) = structural_paths(&new_relations, &seeds, &new_names);
    let mut structural_limitations = vec![
        "path search is bounded to depth 4 and a fixed expansion budget",
        "paths are static relation hypotheses, not execution paths",
        "entry points require an architecture declaration and are not inferred from call degree",
    ];
    if paths_capped {
        structural_limitations.push("path search hit its expansion or result cap");
    }
    if ownership_added.is_empty() && ownership_removed.is_empty() {
        structural_limitations.push("no owns/writes relation evidence was present");
    }
    StructuralDiff {
        dependencies_added,
        dependencies_removed,
        ownership_added,
        ownership_removed,
        convergence,
        structural_limitations,
    }
}


fn entity_payloads(
    store: &Store,
    project_id: &str,
    snapshot_id: &str,
) -> Result<BTreeMap<String, Value>> {
    let mut entities = BTreeMap::new();
    for id in store.entity_ids(project_id, snapshot_id)? {
        if let Some(entity) = store.get_entity(project_id, snapshot_id, &id)? {
            entities.insert(id, entity);
        }
    }
    Ok(entities)
}

struct SnapshotDiff<'a> {
    base_snapshot_id: &'a str,
    new_snapshot_id: &'a str,
    base_provenance: Value,
    new_provenance: Value,
    tree_equivalent: bool,
    identity_changes: Vec<Value>,
    invalidation: Vec<Value>,
    reuse: Value,
    files_changed: Vec<Value>,
    files_added: Vec<Value>,
    files_deleted: Vec<Value>,
    moved: Vec<Value>,
    entities_changed: Vec<Value>,
    entities_added: Vec<Value>,
    entities_deleted: Vec<Value>,
    relations_added: Vec<Value>,
    relations_deleted: Vec<Value>,
    dependencies_added: Vec<Value>,
    dependencies_removed: Vec<Value>,
    ownership_added: Vec<Value>,
    ownership_removed: Vec<Value>,
    convergence: Vec<Value>,
    structural_limitations: Vec<&'static str>,
    started: Instant,
}

fn build_snapshot_diff(diff: SnapshotDiff<'_>) -> Value {
    json!({
        "base_snapshot_id": diff.base_snapshot_id,
        "new_snapshot_id": diff.new_snapshot_id,
        "state": if diff.files_changed.is_empty() && diff.files_added.is_empty() && diff.files_deleted.is_empty()
            && diff.moved.is_empty()
            && diff.entities_changed.is_empty() && diff.entities_added.is_empty() && diff.entities_deleted.is_empty()
            && diff.relations_added.is_empty() && diff.relations_deleted.is_empty() {
            "unchanged"
        } else {
            "changed"
        },
        "provenance": {
            "base": diff.base_provenance,
            "candidate": diff.new_provenance,
            "tree_equivalent": diff.tree_equivalent,
            "durability": "both snapshots are immutable ledger records",
        },
        "identity_changes": diff.identity_changes,
        "invalidation": diff.invalidation,
        "reuse": diff.reuse,
        "timings": {"total_ms": diff.started.elapsed().as_millis() as u64},
        "summary": {
            "files": {
                "changed": diff.files_changed.len(),
                "added": diff.files_added.len(),
                "deleted": diff.files_deleted.len(),
                "moved": diff.moved.len(),
            },
            "entities": {
                "changed": diff.entities_changed.len(),
                "added": diff.entities_added.len(),
                "deleted": diff.entities_deleted.len(),
            },
            "relations": {
                "added": diff.relations_added.len(),
                "deleted": diff.relations_deleted.len(),
            },
        },
        "verification": {
            "state": "not_run",
            "reason": "snapshot comparison; no build, test or runtime command was executed",
        },
        "files": {
            "changed": capped(&diff.files_changed, |value| value.clone()),
            "added": capped(&diff.files_added, |value| value.clone()),
            "deleted": capped(&diff.files_deleted, |value| value.clone()),
            "moved": capped(&diff.moved, |value| value.clone()),
        },
        "structural": {
            "dependencies": {
                "added": capped(&diff.dependencies_added, |value| value.clone()),
                "removed": capped(&diff.dependencies_removed, |value| value.clone()),
            },
            "ownership": {
                "added": capped(&diff.ownership_added, |value| value.clone()),
                "removed": capped(&diff.ownership_removed, |value| value.clone()),
                "basis": "relation_edges",
                "coverage": if diff.ownership_added.is_empty() && diff.ownership_removed.is_empty() {
                    "none"
                } else {
                    "partial"
                },
            },
            "entry_points": {
                "added": [],
                "removed": [],
                "basis": "unavailable",
            },
            "paths": {
                "convergence": diff.convergence,
                "coverage": "partial",
                "limitations": diff.structural_limitations,
            },
        },
        "entities": {
            "changed": capped(&diff.entities_changed, |value| value.clone()),
            "added": capped(&diff.entities_added, |value| value.clone()),
            "deleted": capped(&diff.entities_deleted, |value| value.clone()),
        },
        "relations": {
            "added": capped(&diff.relations_added, |value| value.clone()),
            "deleted": capped(&diff.relations_deleted, |value| value.clone()),
        },
        "coverage": {
            "state": "complete",
            "scope": "two registered snapshots; content-addressed files, entities and relations",
            "limitations": [
                "entity changed means same kind+qualified name but a different content-derived id",
                "relations are compared by relation id; a line-only change can appear as delete+add",
                "structural paths are bounded hypotheses; ownership and entry-point coverage can be none"
            ],
        },
    })
}
