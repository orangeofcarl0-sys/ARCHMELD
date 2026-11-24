// Copyright 2026 ARCHMELD contributors
// SPDX-License-Identifier: Apache-2.0

//! One-screen, read-only briefs composed from existing status and review
//! facts. A brief never adds evidence, infers new calls, or executes a
//! repository command; it only summarizes what the snapshot and the static
//! workspace diff already say.

use crate::error::{CoreError, Result};
use crate::review::review_workspace_diff;
use crate::status::status_report;
use crate::store::Store;
use serde_json::{json, Value};
use std::collections::BTreeMap;
use std::path::Path;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BriefRecipe {
    Auto,
    Project,
    Change,
}

impl BriefRecipe {
    pub fn parse(value: &str) -> Result<Self> {
        match value {
            "auto" => Ok(Self::Auto),
            "project" => Ok(Self::Project),
            "change" => Ok(Self::Change),
            other => Err(CoreError::invalid(format!(
                "recipe must be auto, project or change, got {other}"
            ))),
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Self::Auto => "auto",
            Self::Project => "project",
            Self::Change => "change",
        }
    }
}

pub struct BriefRequest<'a> {
    pub project_id: &'a str,
    pub snapshot_id: Option<&'a str>,
    pub workspace: Option<&'a Path>,
    pub recipe: BriefRecipe,
}

/// Build a brief for one project and snapshot. Missing snapshots and missing
/// workspaces are returned as actionable states, not as hard errors, so an
/// MCP client can present the next step instead of a dead end.
pub fn build_brief(store: &Store, request: &BriefRequest<'_>) -> Result<Value> {
    let snapshot_id = match request.snapshot_id {
        Some(snapshot_id) => snapshot_id.to_string(),
        None => match store.latest_snapshot_id(request.project_id)? {
            Some(snapshot_id) => snapshot_id,
            None => {
                return Ok(json!({
                    "schema_version": "1.0",
                    "state": "needs_init",
                    "level": "module",
                    "summary": format!(
                        "project {} has no snapshot yet",
                        request.project_id
                    ),
                    "scope": {
                        "project_id": request.project_id,
                        "snapshot_id": null,
                        "coverage": "unknown"
                    },
                    "counts": {"confirmed": 0, "candidate": 0, "unknown": 1, "stale": 0, "findings": 0},
                    "findings": [],
                    "unknowns": [{
                        "question": "Which bytes does the project graph describe?",
                        "why": "no snapshot is registered for this project",
                        "how_to_resolve": "run archmeld init (or archmeld index) before querying"
                    }],
                    "next_actions": [{
                        "tool": "archmeld_init",
                        "args": {"workspace": request.workspace.map(|path| path.to_string_lossy().to_string())},
                        "why": "capture and index a snapshot first"
                    }]
                }));
            }
        },
    };

    let recipe = match request.recipe {
        BriefRecipe::Auto => {
            if request.workspace.is_none() {
                BriefRecipe::Project
            } else {
                let review = review_workspace_diff(
                    store,
                    request.project_id,
                    &snapshot_id,
                    request.workspace.expect("checked"),
                )?;
                if review["state"] == "changed" {
                    BriefRecipe::Change
                } else {
                    BriefRecipe::Project
                }
            }
        }
        other => other,
    };

    match recipe {
        BriefRecipe::Change => change_brief(store, request, &snapshot_id),
        _ => project_brief(store, request, &snapshot_id),
    }
}

fn project_brief(store: &Store, request: &BriefRequest<'_>, snapshot_id: &str) -> Result<Value> {
    let status = status_report(store, Some(request.project_id), request.workspace)?;
    let entry = status["projects"]
        .as_array()
        .and_then(|projects| projects.first())
        .cloned()
        .unwrap_or_else(|| json!({}));
    let counts = &entry["counts"];
    let snapshot = &entry["snapshot"];
    let coverage = snapshot["coverage"].clone();
    let languages = entry["languages"].as_array().cloned().unwrap_or_default();
    let files = store
        .snapshot_payload(request.project_id, snapshot_id)?
        .and_then(|payload| payload["files"].as_array().cloned())
        .unwrap_or_default();
    let modules = top_modules(&files);

    let entities = counts["entities"].as_u64().unwrap_or(0);
    let relations = counts["relations"].as_u64().unwrap_or(0);
    let (confirmed, candidate) = resolution_counts(store, request.project_id, snapshot_id)?;
    let mut findings = Vec::new();
    if coverage["state"] == "partial"
        || coverage["omitted_count"].as_u64().unwrap_or(0) > 0
        || coverage["unresolved_count"].as_u64().unwrap_or(0) > 0
    {
        findings.push(finding(
            "F1",
            "medium",
            "evidence-gap",
            "snapshot coverage is partial",
            json!({}),
            Vec::new(),
            "some files or symbols were omitted or unresolved during indexing",
            "archmeld_status",
            json!({}),
        ));
    }
    if entry["freshness"]["state"] == "changed" {
        findings.push(finding(
            "F2",
            "medium",
            "change-impact",
            "workspace differs from the indexed snapshot",
            json!({}),
            Vec::new(),
            "the working tree has uncommitted changes; the graph describes older bytes",
            "archmeld_review",
            json!({}),
        ));
    }
    let unknowns = coverage_unknowns(&coverage);
    let summary = format!(
        "{} files, {} entities, {} relations, {} language(s)",
        files.len(),
        entities,
        relations,
        languages.len()
    );
    Ok(json!({
        "schema_version": "1.0",
        "state": "ok",
        "level": "module",
        "summary": summary,
        "scope": {
            "project_id": request.project_id,
            "snapshot_id": snapshot_id,
            "commit": snapshot["git_commit"],
            "coverage": coverage["state"],
        },
        "counts": {
            "confirmed": confirmed,
            "candidate": candidate,
            "unknown": unknowns.len(),
            "stale": if entry["freshness"]["state"] == "changed" { 1 } else { 0 },
            "findings": findings.len(),
        },
        "languages": languages,
        "modules": modules,
        "findings": findings,
        "unknowns": unknowns,
        "next_actions": [
            {
                "tool": "archmeld_query",
                "args": {"kind": "symbols", "query": ""},
                "why": "list indexed symbols in the current snapshot"
            },
            {
                "tool": "archmeld_context",
                "args": {"objective": "summarize this project"},
                "why": "build a bounded, evidence-backed project context"
            },
            {
                "tool": "archmeld_review",
                "args": {},
                "why": "compare the working tree with this snapshot"
            }
        ],
        "artifacts": {}
    }))
}

fn change_brief(store: &Store, request: &BriefRequest<'_>, snapshot_id: &str) -> Result<Value> {
    let Some(workspace) = request.workspace else {
        return Ok(json!({
            "schema_version": "1.0",
            "state": "needs_workspace",
            "level": "change",
            "summary": "change review needs an authorized workspace root",
            "scope": {
                "project_id": request.project_id,
                "snapshot_id": snapshot_id,
                "coverage": "unknown"
            },
            "counts": {"confirmed": 0, "candidate": 0, "unknown": 1, "stale": 0, "findings": 0},
            "findings": [],
            "unknowns": [{
                "question": "Which working tree should be compared with the snapshot?",
                "why": "the request did not include a workspace root",
                "how_to_resolve": "start archmeld-mcp with a registered project root, or pass --workspace to the CLI"
            }],
            "next_actions": [{
                "tool": "archmeld_review",
                "args": {},
                "why": "review uses the project's registered active workspace root"
            }]
        }));
    };
    let review = review_workspace_diff(store, request.project_id, snapshot_id, workspace)?;
    let relations = review["affected_relations"]
        .as_array()
        .cloned()
        .unwrap_or_default();
    let files = review["files"].as_array().cloned().unwrap_or_default();
    let coverage = review["coverage"].clone();

    let mut findings = Vec::new();
    let mut finding_number = 0usize;
    let mut truncated = false;
    for file in &files {
        let path = file["path"].as_str().unwrap_or("");
        for entity in file["entities"].as_array().cloned().unwrap_or_default() {
            if findings.len() >= 50 {
                truncated = true;
                break;
            }
            let entity_id = entity["entity_id"].as_str().unwrap_or("");
            if entity_id.is_empty() {
                continue;
            }
            let incoming: Vec<&Value> = relations
                .iter()
                .filter(|relation| relation["target_id"] == entity_id)
                .collect();
            let state = entity["state"].as_str().unwrap_or("changed");
            let severity = if !incoming.is_empty() {
                "high"
            } else if state == "added" {
                "medium"
            } else if state == "deleted" {
                "low"
            } else {
                "info"
            };
            finding_number += 1;
            let mut evidence = vec![json!({
                "kind": "file_line",
                "path": path,
                "line": entity["line_start"],
            })];
            for relation in &incoming {
                evidence.push(json!({
                    "kind": "relation",
                    "ref": relation["relation_id"],
                    "resolution": relation["resolution"],
                }));
            }
            let why = if incoming.is_empty() {
                "this entity changed; no incoming static relation was found in the base snapshot"
            } else {
                "this entity changed and has incoming static relation(s) in the base snapshot"
            };
            findings.push(finding(
                &format!("F{finding_number}"),
                severity,
                "change-impact",
                &format!(
                    "{} {}",
                    entity["qualified_name"].as_str().unwrap_or(entity_id),
                    state
                ),
                json!({
                    "path": path,
                    "line": entity["line_start"],
                    "entity_id": entity_id,
                }),
                evidence,
                why,
                "archmeld_source",
                json!({"entity_id": entity_id}),
            ));
        }
        if truncated {
            break;
        }
    }

    let unmapped = coverage["unmapped_files"]
        .as_array()
        .cloned()
        .unwrap_or_default();
    let unsupported = coverage["unsupported_files"]
        .as_array()
        .cloned()
        .unwrap_or_default();
    if !unmapped.is_empty() {
        findings.push(finding(
            &format!("F{}", findings.len() + 1),
            "medium",
            "evidence-gap",
            "changed files without a base entity mapping",
            json!({}),
            Vec::new(),
            &format!("{} file(s) have no base anchors", unmapped.len()),
            "archmeld_review",
            json!({}),
        ));
    }
    if !unsupported.is_empty() {
        findings.push(finding(
            &format!("F{}", findings.len() + 1),
            "low",
            "evidence-gap",
            "files skipped or unsupported",
            json!({}),
            Vec::new(),
            &format!("{} file(s) could not be fully reviewed", unsupported.len()),
            "archmeld_review",
            json!({}),
        ));
    }

    let confirmed = relations
        .iter()
        .filter(|relation| {
            matches!(
                relation["resolution"].as_str(),
                Some("declared") | Some("resolved") | Some("observed")
            )
        })
        .count();
    let candidate = relations
        .iter()
        .filter(|relation| relation["resolution"] == "candidate")
        .count();
    let unknown = unmapped.len() + unsupported.len() + if truncated { 1 } else { 0 };
    let mut unknowns = coverage_unknowns(&coverage);
    if truncated {
        unknowns.push(json!({
            "question": "Are there more changed entities?",
            "why": "the brief stopped after 50 findings",
            "how_to_resolve": "call archmeld_review for the full list"
        }));
    }
    unknowns.push(json!({
        "question": "Does this change pass tests?",
        "why": "review is static and executed no repository command",
        "how_to_resolve": "run the repository's own tests or a verification contract"
    }));
    let summary = format!(
        "{} file(s) changed; {} affected entities; {} candidate relation(s); verification not run",
        files.len(),
        review["summary"]["affected_entities"].as_u64().unwrap_or(0),
        candidate
    );
    let top_entity = findings
        .iter()
        .find_map(|finding| finding["location"]["entity_id"].as_str())
        .map(str::to_string);
    let mut next_actions = Vec::new();
    if let Some(entity_id) = top_entity {
        next_actions.push(json!({
            "tool": "archmeld_source",
            "args": {"entity_id": entity_id},
            "why": "read the exact changed source range"
        }));
        next_actions.push(json!({
            "tool": "archmeld_query",
            "args": {"kind": "impact", "entity_ids": [entity_id], "max_depth": 1},
            "why": "reverse-traverse incoming relations for this entity"
        }));
    }
    next_actions.push(json!({
        "tool": "archmeld_review",
        "args": {},
        "why": "get the full per-file review and coverage"
    }));
    Ok(json!({
        "schema_version": "1.0",
        "state": "ok",
        "level": "change",
        "summary": summary,
        "scope": {
            "project_id": request.project_id,
            "snapshot_id": snapshot_id,
            "workspace": workspace.to_string_lossy(),
            "coverage": coverage["state"],
        },
        "counts": {
            "confirmed": confirmed,
            "candidate": candidate,
            "unknown": unknown,
            "stale": if review["state"] == "changed" { 1 } else { 0 },
            "findings": findings.len(),
        },
        "findings": findings,
        "unknowns": unknowns,
        "next_actions": next_actions,
        "artifacts": {}
    }))
}

fn resolution_counts(store: &Store, project_id: &str, snapshot_id: &str) -> Result<(u64, u64)> {
    let mut statement = store
        .conn
        .prepare("SELECT payload_json FROM relations WHERE project_id=?1 AND snapshot_id=?2")?;
    let rows = statement.query_map((project_id, snapshot_id), |row| row.get::<_, String>(0))?;
    let mut confirmed = 0u64;
    let mut candidate = 0u64;
    for row in rows {
        let text = row?;
        let payload: Value = serde_json::from_str(&text)
            .map_err(|error| CoreError::Storage(format!("relation payload corrupt: {error}")))?;
        match payload["resolution"].as_str() {
            Some("declared") | Some("resolved") | Some("observed") => confirmed += 1,
            Some("candidate") => candidate += 1,
            _ => {}
        }
    }
    Ok((confirmed, candidate))
}

fn top_modules(files: &[Value]) -> Vec<Value> {
    let mut counts: BTreeMap<String, u64> = BTreeMap::new();
    for file in files {
        let path = file["path"].as_str().unwrap_or("");
        let module = path.split_once('/').map(|(first, _)| first).unwrap_or(".");
        if module.is_empty() {
            continue;
        }
        *counts.entry(module.to_string()).or_default() += 1;
    }
    let mut modules: Vec<(String, u64)> = counts.into_iter().collect();
    modules.sort_by(|a, b| b.1.cmp(&a.1).then(a.0.cmp(&b.0)));
    modules
        .into_iter()
        .take(8)
        .map(|(name, files)| json!({"module": name, "files": files}))
        .collect()
}

fn coverage_unknowns(coverage: &Value) -> Vec<Value> {
    let mut unknowns = Vec::new();
    if coverage["state"] == "partial" {
        unknowns.push(json!({
            "question": "Which files or symbols are missing from this view?",
            "why": "coverage is partial",
            "how_to_resolve": "inspect coverage.limitations or re-index with a larger budget"
        }));
    }
    if let Some(limitations) = coverage["limitations"].as_array() {
        for limitation in limitations {
            if let Some(text) = limitation.as_str() {
                if is_generic_static_limitation(text) {
                    continue;
                }
                unknowns.push(json!({
                    "question": text,
                    "why": "declared coverage limitation",
                    "how_to_resolve": "treat the result as bounded evidence, not as absence"
                }));
            }
        }
    }
    unknowns
}

fn is_generic_static_limitation(text: &str) -> bool {
    text.contains("line overlap is static")
        || text.contains("entity impact is adjacency")
        || text.contains("added files have no base entity mapping")
        || text.contains("no repository command was executed")
}

#[allow(clippy::too_many_arguments)]
fn finding(
    id: &str,
    severity: &str,
    category: &str,
    title: &str,
    location: Value,
    evidence: Vec<Value>,
    why: &str,
    next_tool: &str,
    next_args: Value,
) -> Value {
    json!({
        "id": id,
        "severity": severity,
        "category": category,
        "title": title,
        "location": location,
        "evidence": evidence,
        "why": why,
        "verification": "not_run",
        "next_action": {"tool": next_tool, "args": next_args}
    })
}

/// Render a brief as a compact markdown page for a terminal or an MCP
/// `format=human` response. No ANSI color is used, so the text stays readable
/// in logs and non-color terminals.
pub fn render_markdown(brief: &Value) -> String {
    let mut out = String::new();
    out.push_str("# ARCHMELD brief\n\n");
    out.push_str(brief["summary"].as_str().unwrap_or("no summary"));
    out.push_str("\n\n");
    let scope = &brief["scope"];
    out.push_str(&format!(
        "- level: {}\n- project: {}\n- snapshot: {}\n- coverage: {}\n",
        brief["level"].as_str().unwrap_or("unknown"),
        scope["project_id"].as_str().unwrap_or("unknown"),
        scope["snapshot_id"].as_str().unwrap_or("none"),
        scope["coverage"].as_str().unwrap_or("unknown"),
    ));
    let counts = &brief["counts"];
    out.push_str(&format!(
        "- confirmed: {} · candidate: {} · unknown: {} · stale: {} · findings: {}\n",
        counts["confirmed"].as_u64().unwrap_or(0),
        counts["candidate"].as_u64().unwrap_or(0),
        counts["unknown"].as_u64().unwrap_or(0),
        counts["stale"].as_u64().unwrap_or(0),
        counts["findings"].as_u64().unwrap_or(0),
    ));
    out.push_str("\n## Findings\n\n");
    let findings = brief["findings"].as_array().cloned().unwrap_or_default();
    if findings.is_empty() {
        out.push_str("No findings in this scope.\n");
    }
    for finding in findings {
        out.push_str(&format!(
            "- **{}** {} — {}\n",
            finding["severity"]
                .as_str()
                .unwrap_or("info")
                .to_uppercase(),
            finding["title"].as_str().unwrap_or("finding"),
            finding["why"].as_str().unwrap_or(""),
        ));
        let location = &finding["location"];
        if let Some(path) = location["path"].as_str() {
            out.push_str(&format!(
                "  - at `{}:{}`\n",
                path,
                location["line"].as_u64().unwrap_or(0)
            ));
        }
        if let Some(entity) = location["entity_id"].as_str() {
            out.push_str(&format!("  - entity `{entity}`\n"));
        }
        out.push_str(&format!(
            "  - next: `{}`\n",
            finding["next_action"]["tool"]
                .as_str()
                .unwrap_or("archmeld_review")
        ));
    }
    out.push_str("\n## Unknowns\n\n");
    let unknowns = brief["unknowns"].as_array().cloned().unwrap_or_default();
    if unknowns.is_empty() {
        out.push_str("None declared.\n");
    }
    for unknown in unknowns {
        out.push_str(&format!(
            "- {}\n  - why: {}\n  - resolve: {}\n",
            unknown["question"].as_str().unwrap_or("unknown"),
            unknown["why"].as_str().unwrap_or(""),
            unknown["how_to_resolve"].as_str().unwrap_or(""),
        ));
    }
    out.push_str("\n## Next\n\n");
    for action in brief["next_actions"]
        .as_array()
        .cloned()
        .unwrap_or_default()
    {
        out.push_str(&format!(
            "- `{}` — {}\n",
            action["tool"].as_str().unwrap_or("archmeld_review"),
            action["why"].as_str().unwrap_or("")
        ));
    }
    out
}
