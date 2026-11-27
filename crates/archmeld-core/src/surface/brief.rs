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
use std::fs;
use std::path::{Path, PathBuf};
use std::time::{Instant, SystemTime, UNIX_EPOCH};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BriefRecipe {
    Auto,
    Project,
    Change,
    Map,
}

impl BriefRecipe {
    pub fn parse(value: &str) -> Result<Self> {
        match value {
            "auto" => Ok(Self::Auto),
            "project" => Ok(Self::Project),
            "change" => Ok(Self::Change),
            "map" => Ok(Self::Map),
            other => Err(CoreError::invalid(format!(
                "recipe must be auto, project, change or map, got {other}"
            ))),
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Self::Auto => "auto",
            Self::Project => "project",
            Self::Change => "change",
            Self::Map => "map",
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
                    "counts": {"declared": 0, "candidate": 0, "unknown": 1, "stale": 0, "findings": 0},
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

    let mut auto_review: Option<Value> = None;
    let mut clean_freshness: Option<Value> = None;
    let mut review_calls = 0u64;
    let mut review_ms = 0u64;
    let recipe = match request.recipe {
        BriefRecipe::Auto => {
            if request.workspace.is_none() {
                BriefRecipe::Project
            } else {
                let started = Instant::now();
                let review = review_workspace_diff(
                    store,
                    request.project_id,
                    &snapshot_id,
                    request.workspace.expect("checked"),
                )?;
                review_calls = 1;
                review_ms = started.elapsed().as_millis() as u64;
                if review["state"] == "changed" {
                    auto_review = Some(review);
                    BriefRecipe::Change
                } else {
                    clean_freshness = Some(json!({
                        "state": "clean",
                        "needs_review": false,
                        "changed": [],
                        "added": [],
                        "deleted": [],
                    }));
                    BriefRecipe::Project
                }
            }
        }
        other => other,
    };

    let mut brief = match recipe {
        BriefRecipe::Change => {
            change_brief(store, request, &snapshot_id, auto_review.as_ref())
        }
        BriefRecipe::Map => map_brief(store, request, &snapshot_id),
        _ => project_brief(store, request, &snapshot_id, clean_freshness.as_ref()),
    }?;
    if let Some(object) = brief.as_object_mut() {
        object.insert(
            "timings".to_string(),
            json!({"review_calls": review_calls, "review_ms": review_ms}),
        );
    }
    Ok(brief)
}

fn map_brief(store: &Store, request: &BriefRequest<'_>, snapshot_id: &str) -> Result<Value> {
    let Some(workspace) = request.workspace else {
        return Ok(json!({
            "schema_version": "1.0",
            "state": "needs_workspace",
            "map_state": "needs_workspace",
            "level": "map",
            "semantic_claims": "none",
            "verification": "not_run",
            "summary": "map status needs an authorized workspace root",
            "scope": {"project_id": request.project_id, "snapshot_id": snapshot_id, "coverage": "unknown"},
            "counts": {"declared": 0, "candidate": 0, "unknown": 1, "stale": 0, "findings": 0},
            "findings": [],
            "unknowns": [{
                "question": "Which workspace contains archmeld.map.txt?",
                "why": "the request did not include a workspace root",
                "how_to_resolve": "start archmeld-mcp with a registered project root, or pass --workspace to the CLI"
            }],
            "next_actions": [{"tool": "archmeld_brief", "args": {"recipe": "project"}, "why": "get the project summary first"}]
        }));
    };
    let report = crate::cognition_map::check_map(
        store,
        request.project_id,
        workspace,
        Some(snapshot_id),
        None,
    )?;
    let state = report["state"].as_str().unwrap_or("unknown");
    let entries = report["entries"]["total"].as_u64().unwrap_or(0);
    let refs = &report["refs"];
    let confirmed = refs["confirmed"].as_u64().unwrap_or(0);
    let candidate = refs["candidate"].as_u64().unwrap_or(0);
    let unknown = refs["unknown"].as_u64().unwrap_or(0);
    let missing = report["coverage"]["missing"]
        .as_array()
        .map(Vec::len)
        .unwrap_or(0);
    let errors = report["errors"].as_array().map(Vec::len).unwrap_or(0);
    let summary = match state {
        "absent" => "no archmeld.map.txt in the workspace".to_string(),
        "invalid" => format!("map invalid: {entries} entries, {errors} error(s)"),
        "stale" => format!("map stale: {entries} entries, {missing} missing file(s)"),
        _ => format!(
            "map ok: {entries} entries, {confirmed} confirmed refs, {candidate} candidate, {unknown} unknown"
        ),
    };
    Ok(json!({
        "schema_version": "1.0",
        "state": if state == "absent" { "no_map" } else { "ok" },
        "map_state": state,
        "level": "map",
        "semantic_claims": if state == "absent" { "none" } else { "unverified" },
        "verification": "not_run",
        "summary": summary,
        "scope": {
            "project_id": request.project_id,
            "snapshot_id": snapshot_id,
            "coverage": if missing == 0 && errors == 0 { "complete" } else { "partial" },
        },
        "counts": {
            "confirmed": confirmed,
            "candidate": candidate,
            "unknown": unknown,
            "stale": if state == "stale" || state == "invalid" { 1 } else { 0 },
            "findings": errors,
        },
        "map": report,
        "findings": [],
        "unknowns": [],
        "next_actions": [
            {"tool": "archmeld_brief", "args": {"recipe": "project"}, "why": "read the project summary"},
            {"tool": "archmeld_review", "args": {}, "why": "compare the working tree with the snapshot"}
        ],
        "artifacts": {}
    }))
}

fn project_brief(
    store: &Store,
    request: &BriefRequest<'_>,
    snapshot_id: &str,
    freshness_override: Option<&Value>,
) -> Result<Value> {
    // Auto already ran the review once; do not hash the workspace again just
    // to prove it is clean.
    let status_workspace = if freshness_override.is_some() {
        None
    } else {
        request.workspace
    };
    let status = status_report(store, Some(request.project_id), status_workspace)?;
    let mut entry = status["projects"]
        .as_array()
        .and_then(|projects| projects.first())
        .cloned()
        .unwrap_or_else(|| json!({}));
    if let Some(freshness) = freshness_override {
        entry["freshness"] = freshness.clone();
    }
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
    let (declared, candidate) = resolution_counts(store, request.project_id, snapshot_id)?;
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
            "eligible workspace bytes differ from the indexed snapshot; the graph describes older bytes",
            "archmeld_review",
            json!({}),
        ));
    }
    let unknowns = coverage_unknowns(&coverage);
    let parsed_files = entry["index"]["parse"]["files_parsed"].as_u64();
    let summary = match parsed_files {
        Some(parsed) => format!(
            "{} files ({} parsed), {} entities, {} relations, {} language(s)",
            files.len(),
            parsed,
            entities,
            relations,
            languages.len()
        ),
        None => format!(
            "{} files, {} entities, {} relations, {} language(s)",
            files.len(),
            entities,
            relations,
            languages.len()
        ),
    };
    let mut brief = json!({
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
            "declared": declared,
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
    });
    if entry["index"].is_object() {
        brief["index"] = entry["index"].clone();
    }
    let cognition = crate::cognition::summarize(request.workspace);
    if cognition["present"] == true {
        brief["cognition"] = cognition;
    }
    let cognition_map = crate::cognition_map::summarize(request.workspace, Some(snapshot_id));
    if cognition_map["present"] == true {
        brief["cognition_map"] = cognition_map;
    }
    Ok(brief)
}

fn change_brief(
    store: &Store,
    request: &BriefRequest<'_>,
    snapshot_id: &str,
    prepared_review: Option<&Value>,
) -> Result<Value> {
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
            "counts": {"declared": 0, "candidate": 0, "unknown": 1, "stale": 0, "findings": 0},
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
    let computed_review;
    let review = if let Some(review) = prepared_review {
        review
    } else {
        computed_review = review_workspace_diff(store, request.project_id, snapshot_id, workspace)?;
        &computed_review
    };
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

    let declared = relations
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
    let changed_paths = files
        .iter()
        .filter_map(|file| file["path"].as_str().map(str::to_string))
        .collect::<Vec<_>>();
    let cognition = crate::cognition::affected(request.workspace, &changed_paths);
    let mut brief = json!({
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
            "declared": declared,
            "candidate": candidate,
            "unknown": unknown,
            "stale": if review["state"] == "changed" { 1 } else { 0 },
            "findings": findings.len(),
        },
        "findings": findings,
        "unknowns": unknowns,
        "next_actions": next_actions,
        "artifacts": {}
    });
    if cognition["present"] == true {
        brief["cognition"] = cognition;
    }
    let cognition_map =
        crate::cognition_map::affected(workspace, Some(snapshot_id), &changed_paths);
    if cognition_map["present"] == true {
        brief["cognition_map"] = cognition_map;
    }
    Ok(brief)
}

fn resolution_counts(store: &Store, project_id: &str, snapshot_id: &str) -> Result<(u64, u64)> {
    let mut statement = store
        .conn
        .prepare("SELECT payload_json FROM relations WHERE project_id=?1 AND snapshot_id=?2")?;
    let rows = statement.query_map((project_id, snapshot_id), |row| row.get::<_, String>(0))?;
    let mut declared = 0u64;
    let mut candidate = 0u64;
    for row in rows {
        let text = row?;
        let payload: Value = serde_json::from_str(&text)
            .map_err(|error| CoreError::Storage(format!("relation payload corrupt: {error}")))?;
        match payload["resolution"].as_str() {
            Some("declared") | Some("resolved") | Some("observed") => declared += 1,
            Some("candidate") => candidate += 1,
            _ => {}
        }
    }
    Ok((declared, candidate))
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
        "- declared: {} · candidate: {} · unknown: {} · stale: {} · findings: {}\n",
        counts["declared"].as_u64().unwrap_or(0),
        counts["candidate"].as_u64().unwrap_or(0),
        counts["unknown"].as_u64().unwrap_or(0),
        counts["stale"].as_u64().unwrap_or(0),
        counts["findings"].as_u64().unwrap_or(0),
    ));
    if let Some(cognition) = brief.get("cognition") {
        if cognition["present"] == true {
            out.push_str("\n## Cognition map (AOCI, optional)\n\n");
            out.push_str(&format!(
                "- entries: {} (model-authored; not a parsed call graph)\n",
                cognition["entries_total"].as_u64().unwrap_or(0)
            ));
            let affected = cognition["affected"]
                .as_array()
                .cloned()
                .unwrap_or_default();
            if !affected.is_empty() {
                out.push_str(&format!("- affected entries: {}\n", affected.len()));
                for entry in affected.iter().take(5) {
                    out.push_str(&format!(
                        "  - `{}` [{}] {}\n",
                        entry["path"].as_str().unwrap_or("?"),
                        entry["tag"].as_str().unwrap_or(""),
                        entry["function"].as_str().unwrap_or(""),
                    ));
                }
            }
            let unmapped = cognition["unmapped_changed_files"]
                .as_array()
                .cloned()
                .unwrap_or_default();
            if !unmapped.is_empty() {
                out.push_str(&format!(
                    "- changed files without an entry: {}\n",
                    unmapped.len()
                ));
            }
            for warning in cognition["warnings"]
                .as_array()
                .cloned()
                .unwrap_or_default()
            {
                if let Some(warning) = warning.as_str() {
                    out.push_str(&format!("- warning: {warning}\n"));
                }
            }
        }
    }
    if let Some(map) = brief.get("cognition_map") {
        if map["present"] == true {
            out.push_str("\n## Cognition map (ARCHMELD, optional)\n\n");
            out.push_str(&format!(
                "- state: {} · entries: {}\n",
                map["state"].as_str().unwrap_or("unknown"),
                map["entries"]
                    .as_u64()
                    .or_else(|| map["entries_total"].as_u64())
                    .unwrap_or(0),
            ));
            if let Some(matched) = map["snapshot_match"].as_bool() {
                out.push_str(&format!("- snapshot match: {matched}\n"));
            }
            if let Some(freshness) = map["freshness"].as_str() {
                out.push_str(&format!("- freshness: {freshness}\n"));
            }
            if let Some(digest) = map["map_digest"].as_str() {
                out.push_str(&format!("- map digest: {digest}\n"));
            }
            out.push_str("- semantic claims: unverified · validation: not_run\n");
            let affected = map["affected"].as_array().cloned().unwrap_or_default();
            for entry in affected.iter().take(5) {
                out.push_str(&format!(
                    "  - `{}` [{}] {}\n",
                    entry["path"].as_str().unwrap_or("?"),
                    entry["tags"].as_str().unwrap_or(""),
                    entry["function"].as_str().unwrap_or(""),
                ));
            }
            let unmapped = map["unmapped_changed_files"]
                .as_array()
                .map(Vec::len)
                .unwrap_or(0);
            if unmapped > 0 {
                out.push_str(&format!("- changed files without an entry: {unmapped}\n"));
            }
        }
    }
    if let Some(map) = brief.get("map") {
        out.push_str(&crate::cognition_map::render_markdown(map));
    }
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

/// Marker that identifies a generated session card. A file without it is a
/// user file and is never overwritten without `force`.
pub const SESSION_CARD_MARKER: &str = "<!-- ARCHMELD SESSION CARD -->";

/// Default location of the optional session card, relative to the workspace.
pub fn session_card_path(workspace: &Path) -> PathBuf {
    workspace.join(".archmeld").join("SESSION.md")
}

/// Write a session card. The card is a projection of the brief, not a new
/// source of truth; it carries the snapshot id so `status` can report whether
/// it is stale. A missing workspace is an error only when a card was requested.
pub fn write_session_card(
    workspace: &Path,
    brief: &Value,
    out: Option<&Path>,
    force: bool,
) -> Result<Value> {
    let path = out
        .map(Path::to_path_buf)
        .unwrap_or_else(|| session_card_path(workspace));
    if let Ok(existing) = fs::read_to_string(&path) {
        if !force && !existing.starts_with(SESSION_CARD_MARKER) {
            return Err(CoreError::invalid(format!(
                "refusing to overwrite {}; it is not an ARCHMELD session card (use --force)",
                path.display()
            )));
        }
    }
    if let Some(parent) = path.parent() {
        if !parent.as_os_str().is_empty() {
            fs::create_dir_all(parent).map_err(|error| {
                CoreError::Storage(format!("cannot create {}: {error}", parent.display()))
            })?;
        }
    }
    let scope = &brief["scope"];
    let counts = &brief["counts"];
    let snapshot = scope["snapshot_id"].as_str().unwrap_or("none");
    let commit = scope["commit"].as_str().unwrap_or("unknown");
    let coverage = scope["coverage"].as_str().unwrap_or("unknown");
    let stale = counts["stale"].as_u64().unwrap_or(0) > 0;
    let generated_at = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_secs())
        .unwrap_or(0);
    let mut content = String::new();
    content.push_str(SESSION_CARD_MARKER);
    content.push_str("\n# ARCHMELD session card\n\n");
    content.push_str(&format!("- snapshot: {snapshot}\n"));
    content.push_str(&format!("- commit: {commit}\n"));
    content.push_str(&format!("- coverage: {coverage}\n"));
    content.push_str(&format!(
        "- workspace: {}\n",
        if stale { "dirty-or-stale" } else { "clean" }
    ));
    content.push_str(&format!(
        "- counts: declared={} candidate={} unknown={} stale={} findings={}\n",
        counts["declared"].as_u64().unwrap_or(0),
        counts["candidate"].as_u64().unwrap_or(0),
        counts["unknown"].as_u64().unwrap_or(0),
        counts["stale"].as_u64().unwrap_or(0),
        counts["findings"].as_u64().unwrap_or(0),
    ));
    content.push_str(&format!("- generated_at_unix: {generated_at}\n"));
    content.push_str(
        "- note: generated from the evidence snapshot; regenerate after changes. \
         This card is a projection, not a source of truth and not a call graph.\n\n",
    );
    content.push_str(&render_markdown(brief));
    fs::write(&path, content.as_bytes())
        .map_err(|error| CoreError::Storage(format!("cannot write {}: {error}", path.display())))?;
    Ok(json!({
        "path": display_path(&path),
        "written": true,
        "bytes": content.len(),
        "marker": true,
    }))
}

/// Report whether a session card exists and whether it matches the current
/// snapshot and working tree. This is cheap: it reads only the card header.
pub fn session_card_status(workspace: &Path, current_snapshot: Option<&str>, dirty: bool) -> Value {
    let path = session_card_path(workspace);
    let text = match fs::read_to_string(&path) {
        Ok(text) => text,
        Err(_) => {
            return json!({
                "present": false,
                "path": display_path(&path),
                "stale": false,
            })
        }
    };
    let marker = text.starts_with(SESSION_CARD_MARKER);
    let mut card_snapshot: Option<String> = None;
    let mut generated_at: Option<u64> = None;
    for line in text.lines().take(20) {
        if let Some(value) = line.strip_prefix("- snapshot: ") {
            card_snapshot = Some(value.trim().to_string());
        }
        if let Some(value) = line.strip_prefix("- generated_at_unix: ") {
            generated_at = value.trim().parse().ok();
        }
    }
    let snapshot_match = match (card_snapshot.as_deref(), current_snapshot) {
        (Some(card), Some(current)) => card == current,
        _ => false,
    };
    json!({
        "present": true,
        "path": display_path(&path),
        "marker": marker,
        "snapshot": card_snapshot,
        "snapshot_match": snapshot_match,
        "generated_at_unix": generated_at,
        "stale": !snapshot_match || dirty,
    })
}

fn display_path(path: &Path) -> String {
    let text = path.to_string_lossy();
    text.strip_prefix(r"\\?\")
        .or_else(|| text.strip_prefix("//?/"))
        .unwrap_or(&text)
        .to_string()
}
