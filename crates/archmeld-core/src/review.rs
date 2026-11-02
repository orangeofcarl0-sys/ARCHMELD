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
use crate::store::{validate, Store};
use serde_json::{json, Value};
use similar::{DiffOp, TextDiff};
use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::Path;

const MAX_TEXT_BYTES: u64 = 8 * 1024 * 1024;
const MAX_LIST: usize = 500;

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

/// Review the current workspace against a frozen snapshot without executing
/// any repository command.
pub fn review_workspace_diff(
    store: &Store,
    project_id: &str,
    base_snapshot_id: &str,
    root: &Path,
) -> Result<Value> {
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
    let mut affected = BTreeSet::new();
    let mut unmapped = Vec::new();
    let mut unsupported = Vec::new();
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
    }
    let dirty = drift["state"] == "changed";
    Ok(json!({
        "schema_version": "1.0",
        "project_id": project_id,
        "base_snapshot_id": base_snapshot_id,
        "workspace": root.to_string_lossy(),
        "state": drift["state"],
        "summary": {
            "files": {
                "changed": summary_files.get("changed").copied().unwrap_or(0),
                "added": summary_files.get("added").copied().unwrap_or(0),
                "deleted": summary_files.get("deleted").copied().unwrap_or(0),
            },
            "affected_entities": affected.len(),
            "unmapped_files": unmapped.len(),
            "unsupported_files": unsupported.len(),
            "needs_review": dirty,
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
            "state": if unsupported.is_empty() && unmapped.is_empty() { "complete" } else { "partial" },
            "scope": "workspace bytes vs base snapshot bytes, with base-snapshot entity anchors",
            "unmapped_files": unmapped,
            "unsupported_files": unsupported,
            "limitations": [
                "line overlap is static text evidence, not semantic equivalence",
                "entity impact is adjacency in the base snapshot; new calls are not inferred",
                "added files have no base entity mapping",
                "no repository command was executed"
            ],
        },
        "source_mutation": "none",
        "history_mutation": "none",
    }))
}

fn capped<T: Clone>(items: &[T], value: impl Fn(&T) -> Value) -> Value {
    json!({
        "items": items.iter().take(MAX_LIST).map(value).collect::<Vec<_>>(),
        "count": items.len(),
        "truncated": items.len() > MAX_LIST,
    })
}

/// Exact snapshot-to-snapshot difference. This is used by the refresh command;
/// it does not read the workspace and does not execute anything.
pub fn diff_snapshots(
    store: &Store,
    project_id: &str,
    base_snapshot_id: &str,
    new_snapshot_id: &str,
) -> Result<Value> {
    validate::require_id("project_id", project_id)?;
    validate::require_digest("base_snapshot_id", base_snapshot_id)?;
    validate::require_digest("new_snapshot_id", new_snapshot_id)?;
    store.check_snapshot_scope(project_id, base_snapshot_id)?;
    store.check_snapshot_scope(project_id, new_snapshot_id)?;
    if base_snapshot_id == new_snapshot_id {
        return Ok(json!({
            "base_snapshot_id": base_snapshot_id,
            "new_snapshot_id": new_snapshot_id,
            "state": "unchanged",
            "files": {"changed": [], "added": [], "deleted": []},
            "entities": {"changed": [], "added": [], "deleted": []},
            "relations": {"added": [], "deleted": []},
        }));
    }
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
    let base_entities = entity_payloads(store, project_id, base_snapshot_id)?;
    let new_entities = entity_payloads(store, project_id, new_snapshot_id)?;
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
    let base_relations = store.relation_payloads(project_id, base_snapshot_id)?;
    let new_relations = store.relation_payloads(project_id, new_snapshot_id)?;
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
    Ok(json!({
        "base_snapshot_id": base_snapshot_id,
        "new_snapshot_id": new_snapshot_id,
        "state": if files_changed.is_empty() && files_added.is_empty() && files_deleted.is_empty()
            && entities_changed.is_empty() && entities_added.is_empty() && entities_deleted.is_empty() {
            "unchanged"
        } else {
            "changed"
        },
        "files": {
            "changed": capped(&files_changed, |value| value.clone()),
            "added": capped(&files_added, |value| value.clone()),
            "deleted": capped(&files_deleted, |value| value.clone()),
        },
        "entities": {
            "changed": capped(&entities_changed, |value| value.clone()),
            "added": capped(&entities_added, |value| value.clone()),
            "deleted": capped(&entities_deleted, |value| value.clone()),
        },
        "relations": {
            "added": capped(&relations_added, |value| value.clone()),
            "deleted": capped(&relations_deleted, |value| value.clone()),
        },
        "coverage": {
            "state": "complete",
            "scope": "two registered snapshots; content-addressed files, entities and relations",
            "limitations": [
                "entity changed means same kind+qualified name but a different content-derived id",
                "relations are compared by relation id; a line-only change can appear as delete+add"
            ],
        },
    }))
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
