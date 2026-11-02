// Copyright 2026 ARCHMELD contributors
// SPDX-License-Identifier: Apache-2.0

//! Read-only session status. This is the shared projection used by the CLI and
//! HTTP entry; it never creates a snapshot or records drift history.

use crate::drift::analyze_drift;
use crate::error::{CoreError, Result};
use crate::manager::get_project;
use crate::store::Store;
use serde_json::{json, Value};
use std::path::Path;

pub fn status_report(
    store: &Store,
    project_filter: Option<&str>,
    workspace: Option<&Path>,
) -> Result<Value> {
    if let Some(project) = project_filter {
        if get_project(store, project)?.is_none() {
            return Err(CoreError::not_found(format!(
                "project {project} is not registered"
            )));
        }
    }
    if let Some(root) = workspace {
        if !root.is_dir() {
            return Err(CoreError::invalid(format!(
                "workspace {} is not a directory",
                root.display()
            )));
        }
    }
    let project_ids = match project_filter {
        Some(project) => vec![project.to_string()],
        None => store.list_projects()?,
    };
    let mut projects = Vec::new();
    for project in project_ids {
        let record = get_project(store, &project)?;
        let snapshot_count = store.snapshot_count(&project)?;
        let latest_snapshot_id = store.latest_snapshot_id(&project)?;
        let mut entry = json!({
            "project_id": project,
            "project": record,
            "snapshot_count": snapshot_count,
            "latest_snapshot_id": latest_snapshot_id,
        });
        if let Some(snapshot_id) = latest_snapshot_id.as_deref() {
            if let Some(payload) = store.snapshot_payload(&project, snapshot_id)? {
                entry["snapshot"] = json!({
                    "created_at": payload["created_at"],
                    "git_commit": payload["git_commit"],
                    "config_digest": payload["config_digest"],
                    "coverage": payload["coverage"],
                    "file_count": payload["files"].as_array().map(Vec::len),
                    "stable": payload["stable"],
                });
            }
            let counts = store.published_counts(&project, snapshot_id)?;
            entry["counts"] = json!({
                "entities": counts.entities,
                "evidence": counts.evidence,
                "relations": counts.relations,
                "relation_evidence": counts.relation_evidence,
                "backend_mappings": counts.backend_mappings,
                "diagnostics": counts.diagnostics,
            });
            let languages = store.language_counts(&project, snapshot_id)?;
            entry["languages"] = json!(
                languages
                    .into_iter()
                    .map(|(language, entities)| json!({
                        "language": language,
                        "entities": entities,
                    }))
                    .collect::<Vec<_>>()
            );
            if let Some(root) = workspace {
                match analyze_drift(store, &project, snapshot_id, root, None) {
                    Ok(report) => {
                        entry["freshness"] = json!({
                            "state": report["state"],
                            "needs_review": report["needs_review"],
                            "base_snapshot_id": report["base_snapshot_id"],
                            "changed": report["changed"],
                            "added": report["added"],
                            "deleted": report["deleted"],
                            "affected_entities": report["affected_entities"],
                            "affected_relations": report["affected_relations"],
                            "affected_claims": report["affected_claims"],
                            "affected_architectures": report["affected_architectures"],
                            "stale_plans": report["stale_plans"],
                            "stale_changesets": report["stale_changesets"],
                            "source_mutation": report["source_mutation"],
                            "history_mutation": report["history_mutation"],
                        });
                    }
                    Err(error) => {
                        entry["freshness"] = json!({
                            "state": "unknown",
                            "error": error.to_string(),
                        });
                    }
                }
            } else {
                entry["freshness"] = json!({
                    "state": "unknown",
                    "reason": "no workspace supplied; compare with --workspace or workspace_handle",
                });
            }
        } else {
            entry["freshness"] = json!({
                "state": "unknown",
                "reason": "no snapshot registered for this project",
            });
        }
        projects.push(entry);
    }
    Ok(json!({
        "schema_version": "1.0",
        "state": store.state_root().to_string_lossy(),
        "workspace": workspace.map(|path| path.to_string_lossy().to_string()),
        "actions": {
            "read_only": ["status", "review-diff", "query", "context", "source", "inspect"],
            "requires_grant": ["scenario", "changeset/apply", "verification", "report"],
            "not_exposed_over_mcp": ["scenario", "changeset/apply", "verification", "report"],
            "note": "reads never accept an arbitrary filesystem path; writes require the HTTP/CLI grant and exact prepared digest",
        },
        "projects": projects,
    }))
}
