// Copyright 2026 ARCHMELD contributors
// SPDX-License-Identifier: Apache-2.0

//! One-screen, read-only briefs composed from existing status and review
//! facts. A brief never adds evidence, infers new calls, or executes a
//! repository command; it only summarizes what the snapshot and the static
//! workspace diff already say.

use crate::error::{CoreError, Result};
use crate::drift::analyze_drift;
use crate::review::review_workspace_diff;
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

/// Output budget for a change brief. The default is deliberately small: a
/// brief is a one-screen decision aid, not the full review payload. Callers
/// that need the complete list can raise the limits or call `archmeld_review`
/// with `detail=full`.
#[derive(Debug, Clone, Copy)]
pub struct BriefBudget {
    pub max_findings: usize,
    pub max_context: usize,
    pub max_gaps: usize,
    pub max_relation_items: usize,
}

impl Default for BriefBudget {
    fn default() -> Self {
        Self {
            max_findings: 20,
            max_context: 10,
            max_gaps: 10,
            max_relation_items: 10,
        }
    }
}

impl BriefBudget {
    fn normalized(self) -> Self {
        Self {
            max_findings: self.max_findings.clamp(1, 200),
            max_context: self.max_context.clamp(1, 200),
            max_gaps: self.max_gaps.clamp(1, 200),
            max_relation_items: self.max_relation_items.clamp(0, 100),
        }
    }
}

/// Build a brief with the default one-screen budget.
pub fn build_brief(store: &Store, request: &BriefRequest<'_>) -> Result<Value> {
    build_brief_with_budget(store, request, BriefBudget::default())
}

/// Build a brief for one project and snapshot. Missing snapshots and missing
/// workspaces are returned as actionable states, not as hard errors, so an
/// MCP client can present the next step instead of a dead end.
pub fn build_brief_with_budget(
    store: &Store,
    request: &BriefRequest<'_>,
    budget: BriefBudget,
) -> Result<Value> {
    let budget = budget.normalized();
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
                    "counts": {"confirmed": 0, "declared": 0, "candidate": 0, "unknown": 1, "stale": 0, "findings": 0, "context": 0, "gaps": 0},
                    "findings": [],
                    "context": [],
                    "gaps": [],
                    "coverage": {"state": "unknown"},
                    "budget": map_budget(budget, 0, 0, 0, 0),
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

    if recipe == BriefRecipe::Change && review_calls == 0 {
        review_calls = 1;
    }
    let mut brief = match recipe {
        BriefRecipe::Change => {
            change_brief(store, request, &snapshot_id, auto_review.as_ref(), budget)
        }
        BriefRecipe::Map => map_brief(store, request, &snapshot_id, budget),
        _ => project_brief(
            store,
            request,
            &snapshot_id,
            clean_freshness.as_ref(),
            budget,
        ),
    }?;
    if let Some(object) = brief.as_object_mut() {
        object.insert(
            "timings".to_string(),
            json!({"review_calls": review_calls, "review_ms": review_ms}),
        );
    }
    Ok(brief)
}

fn map_brief(
    store: &Store,
    request: &BriefRequest<'_>,
    snapshot_id: &str,
    budget: BriefBudget,
) -> Result<Value> {
    let Some(workspace) = request.workspace else {
        return Ok(json!({
            "schema_version": "1.0",
            "state": "needs_workspace",
            "map_state": "needs_workspace",
            "level": "map",
            "semantic_claims": "none",
            "verification": {"state": "not_run", "reason": "no map was read"},
            "summary": "map status needs an authorized workspace root",
            "scope": {"project_id": request.project_id, "snapshot_id": snapshot_id, "coverage": "unknown"},
            "coverage": {"state": "unknown"},
            "counts": {"confirmed": 0, "declared": 0, "candidate": 0, "unknown": 1, "stale": 0, "findings": 0, "gaps": 0, "context": 0},
            "findings": [],
            "gaps": [],
            "context": [],
            "budget": map_budget(budget, 0, 0, 0, 0),
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
    let unknown_refs = refs["unknown"].as_u64().unwrap_or(0);
    let errors = report["errors"].as_array().cloned().unwrap_or_default();
    let error_total = errors.len();
    let missing: Vec<String> = report["coverage"]["missing"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|item| item.as_str().map(str::to_string))
        .collect();
    let missing_total = missing.len();
    let summary = match state {
        "absent" => "no archmeld.map.txt in the workspace".to_string(),
        "invalid" => format!("map invalid: {entries} entries, {error_total} error(s)"),
        "stale" => format!(
            "map stale: {entries} entries, {missing_total} snapshot file(s) without an Entry"
        ),
        _ => format!(
            "map ok: {entries} entries, {confirmed} confirmed refs, {candidate} candidate, {unknown_refs} unknown"
        ),
    };
    let mut findings = Vec::new();
    for (index, error) in errors.iter().take(budget.max_findings).enumerate() {
        findings.push(finding(
            &format!("M{}", index + 1),
            "high",
            "map-invalid",
            error["message"]
                .as_str()
                .unwrap_or("map validation error"),
            json!({
                "path": error["path"],
                "line": error["line"],
                "map_digest": report["map_digest"],
            }),
            vec![error.clone()],
            "the map validation rejected this entry or header; semantic claims in the map are unverified",
            "confirmed",
            "archmeld_brief",
            json!({"recipe": "map", "snapshot_id": snapshot_id}),
        ));
    }
    if state == "invalid" && findings.is_empty() {
        findings.push(finding(
            "M1",
            "high",
            "map-invalid",
            "map validation failed without a structured error",
            json!({"map_digest": report["map_digest"]}),
            Vec::new(),
            "the map is marked invalid; treat its claims as unverified",
            "confirmed",
            "archmeld_brief",
            json!({"recipe": "map", "snapshot_id": snapshot_id}),
        ));
    }
    let findings_total = if state == "invalid" {
        error_total.max(findings.len())
    } else {
        error_total
    };
    let mut gaps = Vec::new();
    for path in missing.iter().take(budget.max_gaps) {
        gaps.push(json!({
            "kind": "map_missing",
            "severity": "medium",
            "path": path,
            "reason": "snapshot file has no map Entry and is not declared observed/excluded",
            "side": "base",
            "next_action": {
                "tool": "archmeld_brief",
                "args": {"recipe": "map", "snapshot_id": snapshot_id}
            }
        }));
    }
    let mut unknowns = Vec::new();
    if unknown_refs > 0 {
        unknowns.push(json!({
            "question": format!("{unknown_refs} map reference(s) could not be resolved"),
            "why": "the map names a ref suffix that is neither confirmed nor candidate in this snapshot",
            "how_to_resolve": "inspect the map report refs and fix or annotate the entry"
        }));
    }
    if error_total > findings.len() {
        unknowns.push(json!({
            "question": "Are there more map errors?",
            "why": format!("the brief returned {} of {} map error(s)", findings.len(), error_total),
            "how_to_resolve": "raise max_findings"
        }));
    }
    if missing_total > gaps.len() {
        unknowns.push(json!({
            "question": "Are there more uncovered snapshot files?",
            "why": format!("the brief returned {} of {} uncovered file(s)", gaps.len(), missing_total),
            "how_to_resolve": "raise max_gaps"
        }));
    }
    let coverage_state = if state == "absent" {
        "unknown"
    } else if error_total == 0 && missing_total == 0 {
        "complete"
    } else {
        "partial"
    };
    let mut map_report = report.clone();
    if let Some(items) = map_report["errors"].as_array() {
        map_report["errors_total"] = json!(items.len());
    }
    map_report["errors"] = json!(errors.iter().take(budget.max_findings).collect::<Vec<_>>());
    if let Some(items) = map_report["coverage"]["missing"].as_array() {
        map_report["coverage"]["missing_total"] = json!(items.len());
    }
    map_report["coverage"]["missing"] = json!(missing.iter().take(budget.max_gaps).collect::<Vec<_>>());
    map_report["truncated"] =
        json!(error_total > budget.max_findings || missing_total > budget.max_gaps);
    Ok(json!({
        "schema_version": "1.0",
        "state": if state == "absent" { "no_map" } else { "ok" },
        "map_state": state,
        "level": "map",
        "semantic_claims": if state == "absent" { "none" } else { "unverified" },
        "verification": {
            "state": "not_run",
            "reason": "map validation checks file/ref shape; it executes no build, test or runtime command"
        },
        "summary": summary,
        "scope": {
            "project_id": request.project_id,
            "snapshot_id": snapshot_id,
            "coverage": coverage_state,
        },
        "coverage": {
            "state": coverage_state,
            "entries": entries,
            "errors": error_total,
            "missing_files": missing_total,
            "unknown_refs": unknown_refs,
            "limitations": report["warnings"],
        },
        "counts": {
            "confirmed": confirmed,
            "declared": confirmed,
            "candidate": candidate,
            "unknown": unknowns.len(),
            "stale": if state == "stale" || state == "invalid" { 1 } else { 0 },
            "findings": findings_total,
            "gaps": missing_total,
            "context": 0,
        },
        "findings": findings,
        "gaps": gaps,
        "context": [],
        "map": map_report,
        "unknowns": unknowns,
        "budget": map_budget(budget, findings_total, findings.len(), missing_total, gaps.len()),
        "next_actions": [
            {"tool": "archmeld_brief", "args": {"recipe": "project"}, "why": "read the project summary"},
            {"tool": "archmeld_review", "args": {"base_snapshot_id": snapshot_id}, "why": "compare the working tree with the snapshot"}
        ],
        "artifacts": {}
    }))
}

fn map_budget(
    budget: BriefBudget,
    findings_total: usize,
    findings_returned: usize,
    gaps_total: usize,
    gaps_returned: usize,
) -> Value {
    json!({
        "max_findings": budget.max_findings,
        "max_context": budget.max_context,
        "max_gaps": budget.max_gaps,
        "max_relation_items": budget.max_relation_items,
        "returned_findings": findings_returned,
        "returned_gaps": gaps_returned,
        "omitted_findings": findings_total.saturating_sub(findings_returned),
        "omitted_context": 0,
        "omitted_gaps": gaps_total.saturating_sub(gaps_returned),
        "omitted_relations": 0,
        "truncated": findings_total > findings_returned || gaps_total > gaps_returned,
    })
}

fn project_brief(
    store: &Store,
    request: &BriefRequest<'_>,
    snapshot_id: &str,
    freshness_override: Option<&Value>,
    budget: BriefBudget,
) -> Result<Value> {
    // Auto already ran the review once; do not hash the workspace again just
    // to prove it is clean.
    store.check_snapshot_scope(request.project_id, snapshot_id)?;
    let payload = store
        .snapshot_payload(request.project_id, snapshot_id)?
        .ok_or_else(|| CoreError::not_found(format!("snapshot {snapshot_id} is not registered")))?;
    let counts = store.published_counts(request.project_id, snapshot_id)?;
    let languages = store
        .language_counts(request.project_id, snapshot_id)?
        .into_iter()
        .map(|(language, entities)| json!({"language": language, "entities": entities}))
        .collect::<Vec<_>>();
    let index = store.latest_index_run(request.project_id, snapshot_id)?;
    let freshness = match freshness_override {
        Some(freshness) => freshness.clone(),
        None => match request.workspace {
            Some(workspace) => {
                analyze_drift(store, request.project_id, snapshot_id, workspace, None)?
            }
            None => json!({"state": "unknown", "needs_review": false}),
        },
    };
    let coverage = payload["coverage"].clone();
    let files = payload["files"].as_array().cloned().unwrap_or_default();
    let modules = top_modules(&files);

    let entities = counts.entities;
    let relations = counts.relations;
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
            "confirmed",
            "archmeld_status",
            json!({"snapshot_id": snapshot_id}),
        ));
    }
    if freshness["state"] == "changed" || freshness["state"] == "partial" {
        findings.push(finding(
            "F2",
            "medium",
            "change-impact",
            "workspace differs from the indexed snapshot",
            json!({}),
            Vec::new(),
            "eligible workspace bytes differ from the indexed snapshot; the graph describes older bytes",
            "confirmed",
            "archmeld_review",
            json!({"base_snapshot_id": snapshot_id}),
        ));
    }
    let unknowns = coverage_unknowns(&coverage);
    let parsed_files = index
        .as_ref()
        .and_then(|index| index["parse"]["files_parsed"].as_u64());
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
    let stale = if freshness["state"] == "changed" || freshness["state"] == "partial" {
        1
    } else {
        0
    };
    let mut brief = json!({
        "schema_version": "1.0",
        "state": "ok",
        "level": "module",
        "summary": summary,
        "scope": {
            "project_id": request.project_id,
            "snapshot_id": snapshot_id,
            "commit": payload["git_commit"],
            "coverage": coverage["state"],
        },
        "counts": {
            "confirmed": declared,
            "declared": declared,
            "candidate": candidate,
            "unknown": unknowns.len(),
            "stale": stale,
            "findings": findings.len(),
            "context": 0,
            "gaps": 0,
        },
        "languages": languages,
        "modules": modules,
        "findings": findings,
        "context": [],
        "gaps": [],
        "coverage": coverage,
        "verification": {
            "state": "not_run",
            "reason": "project summary is read-only; no build, test or runtime command was executed"
        },
        "budget": map_budget(budget, findings.len(), findings.len(), 0, 0),
        "unknowns": unknowns,
        "next_actions": [
            {
                "tool": "archmeld_query",
                "args": {"kind": "symbols", "query": "", "snapshot_id": snapshot_id},
                "why": "list indexed symbols in the requested snapshot"
            },
            {
                "tool": "archmeld_context",
                "args": {"objective": "summarize this project", "snapshot_id": snapshot_id},
                "why": "build a bounded, evidence-backed project context"
            },
            {
                "tool": "archmeld_review",
                "args": {"base_snapshot_id": snapshot_id},
                "why": "compare the working tree with this snapshot"
            }
        ],
        "artifacts": {}
    });
    if let Some(index) = index {
        brief["index"] = index;
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

fn load_entity(
    store: &Store,
    project_id: &str,
    snapshot_id: &str,
    entity_id: &str,
    cache: &mut BTreeMap<String, Option<Value>>,
) -> Result<Option<Value>> {
    if let Some(cached) = cache.get(entity_id) {
        return Ok(cached.clone());
    }
    let value = store.get_entity(project_id, snapshot_id, entity_id)?;
    cache.insert(entity_id.to_string(), value.clone());
    Ok(value)
}

fn entity_anchor(entity: &Value) -> (Option<String>, Option<u64>) {
    let anchor = entity["anchors"]
        .as_array()
        .and_then(|anchors| anchors.first());
    let path = anchor
        .and_then(|anchor| anchor["path"].as_str())
        .map(str::to_string);
    let line = anchor.and_then(|anchor| anchor["line_start"].as_u64());
    (path, line)
}

fn change_brief(
    store: &Store,
    request: &BriefRequest<'_>,
    snapshot_id: &str,
    prepared_review: Option<&Value>,
    budget: BriefBudget,
) -> Result<Value> {
    let Some(workspace) = request.workspace else {
        return Ok(change_brief_without_workspace(request, snapshot_id, budget));
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

    // Build the set of changed entities first. Context entries must never
    // duplicate a changed entity, and relation-derived context must exclude
    // callers that are themselves in the change.
    let mut changed_ids = std::collections::BTreeSet::new();
    for file in &files {
        for entity in file["entities"].as_array().into_iter().flatten() {
            if matches!(
                entity["state"].as_str(),
                Some("changed") | Some("added") | Some("deleted")
            ) {
                if let Some(entity_id) = entity["entity_id"].as_str() {
                    changed_ids.insert(entity_id.to_string());
                }
            }
        }
    }

    let mut incoming_by_target: BTreeMap<&str, Vec<&Value>> = BTreeMap::new();
    for relation in &relations {
        if let Some(target) = relation["target_id"].as_str() {
            incoming_by_target.entry(target).or_default().push(relation);
        }
    }
    let mut state = ChangeBriefState::default();
    collect_change_entities(
        store,
        request,
        snapshot_id,
        &files,
        &changed_ids,
        &incoming_by_target,
        budget,
        &mut state,
    )?;
    let ChangeBriefState {
        findings,
        findings_total,
        context,
        context_total,
        relation_counts,
        ..
    } = state;
    let (gaps, gap_total) = build_change_gaps(&files, &coverage, snapshot_id, budget);
    let (declared, candidate, relation_items, relation_total) =
        summarize_relations(&relations, relation_counts, budget);
    let omitted_findings = findings_total.saturating_sub(findings.len());
    let omitted_context = context_total.saturating_sub(context.len());
    let omitted_gaps = gap_total.saturating_sub(gaps.len());
    let omitted_relations = relation_total.saturating_sub(relation_items.len());
    let truncated = omitted_findings + omitted_context + omitted_gaps > 0;
    let mut unknowns = coverage_unknowns(&coverage);
    if omitted_findings > 0 {
        unknowns.push(json!({
            "question": "Are there more changed entities?",
            "why": format!("the brief returned {} of {} changed findings", findings.len(), findings_total),
            "how_to_resolve": "call archmeld_review with detail=full, or raise max_findings"
        }));
    }
    if omitted_context > 0 {
        unknowns.push(json!({
            "question": "Are there more unchanged context entries?",
            "why": format!("the brief returned {} of {} context entries", context.len(), context_total),
            "how_to_resolve": "raise max_context or call archmeld_review with detail=full"
        }));
    }
    if omitted_gaps > 0 {
        unknowns.push(json!({
            "question": "Are there more files without a complete review?",
            "why": format!("the brief returned {} of {} gap entries", gaps.len(), gap_total),
            "how_to_resolve": "raise max_gaps or call archmeld_review with detail=full"
        }));
    }
    if omitted_relations > 0 {
        unknowns.push(json!({
            "question": "Are there more relation rows?",
            "why": format!("the brief returned {} of {} relation groups", relation_items.len(), relation_total),
            "how_to_resolve": "raise max_relation_items or call archmeld_review with detail=full"
        }));
    }
    unknowns.push(json!({
        "question": "Does this change pass tests?",
        "why": "review is static and executed no repository command",
        "how_to_resolve": "run the repository's own tests or a verification contract"
    }));
    let unknown = unknowns.len();
    let truncated = truncated || omitted_relations > 0;
    let moved: Vec<Value> = files
        .iter()
        .filter(|file| file["state"] == "moved")
        .map(|file| {
            json!({
                "from": file["from"],
                "to": file["path"],
                "base_digest": file["base_digest"],
                "current_digest": file["current_digest"],
                "state": "moved",
            })
        })
        .collect();
    let summary = format!(
        "{} changed file(s); {} moved; {} changed entity finding(s); {} unchanged context; {} gap(s); {} candidate relation(s); verification not run",
        files.len(),
        moved.len(),
        findings_total,
        context_total,
        gap_total,
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
            "args": {"entity_id": entity_id, "snapshot_id": snapshot_id},
            "why": "read the exact changed source range from this snapshot"
        }));
        next_actions.push(json!({
            "tool": "archmeld_query",
            "args": {
                "kind": "impact",
                "entity_ids": [entity_id],
                "snapshot_id": snapshot_id,
                "max_depth": 1
            },
            "why": "reverse-traverse incoming relations for this entity"
        }));
    }
    next_actions.push(json!({
        "tool": "archmeld_review",
        "args": {"base_snapshot_id": snapshot_id, "detail": "full", "max_files": 500},
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
            "confirmed": declared,
            "declared": declared,
            "candidate": candidate,
            "unknown": unknown,
            "stale": if review["state"] == "changed" { 1 } else { 0 },
            "findings": findings_total,
            "context": context_total,
            "gaps": gap_total,
        },
        "findings": findings,
        "context": context,
        "gaps": gaps,
        "moved": moved,
        "relations": {
            "confirmed": declared,
            "candidate": candidate,
            "items": relation_items,
            "total": relation_total,
        },
        "budget": {
            "max_findings": budget.max_findings,
            "max_context": budget.max_context,
            "max_gaps": budget.max_gaps,
            "max_relation_items": budget.max_relation_items,
            "returned_findings": findings.len(),
            "returned_context": context.len(),
            "returned_gaps": gaps.len(),
            "omitted_findings": omitted_findings,
            "omitted_context": omitted_context,
            "omitted_gaps": omitted_gaps,
            "omitted_relations": omitted_relations,
            "truncated": truncated,
        },
        "coverage": coverage,
        "verification": {
            "state": "not_run",
            "reason": "static diff review; no build, test or runtime command was executed"
        },
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

#[derive(Default)]
struct ChangeBriefState {
    findings: Vec<Value>,
    findings_total: usize,
    context: Vec<Value>,
    context_total: usize,
    context_seen: std::collections::BTreeSet<String>,
    entity_cache: BTreeMap<String, Option<Value>>,
    relation_counts: BTreeMap<(String, String), u64>,
}

fn change_brief_without_workspace(
    request: &BriefRequest<'_>,
    snapshot_id: &str,
    budget: BriefBudget,
) -> Value {
    json!({
            "schema_version": "1.0",
            "state": "needs_workspace",
            "level": "change",
            "summary": "change review needs an authorized workspace root",
            "scope": {
                "project_id": request.project_id,
                "snapshot_id": snapshot_id,
                "coverage": "unknown"
            },
            "coverage": {"state": "unknown"},
            "verification": {
                "state": "not_run",
                "reason": "no workspace was available; no diff was computed"
            },
            "counts": {"confirmed": 0, "declared": 0, "candidate": 0, "unknown": 1, "stale": 0, "findings": 0, "context": 0, "gaps": 0},
            "findings": [],
            "context": [],
            "gaps": [],
            "budget": map_budget(budget, 0, 0, 0, 0),
            "unknowns": [{
                "question": "Which working tree should be compared with the snapshot?",
                "why": "the request did not include a workspace root",
                "how_to_resolve": "start archmeld-mcp with a registered project root, or pass --workspace to the CLI"
            }],
            "next_actions": [{
                "tool": "archmeld_review",
                "args": {"base_snapshot_id": snapshot_id},
                "why": "review uses the project's registered active workspace root"
            }]
        })
}

fn collect_change_entities(
    store: &Store,
    request: &BriefRequest<'_>,
    snapshot_id: &str,
    files: &[Value],
    changed_ids: &std::collections::BTreeSet<String>,
    incoming_by_target: &BTreeMap<&str, Vec<&Value>>,
    budget: BriefBudget,
    out: &mut ChangeBriefState,
) -> Result<()> {
    for file in files {
        let path = file["path"].as_str().unwrap_or("");
        let context_path = file["from"].as_str().unwrap_or(path);
        for entity in file["entities"].as_array().into_iter().flatten() {
            let entity_id = entity["entity_id"].as_str().unwrap_or("");
            if entity_id.is_empty() {
                continue;
            }
            let state = entity["state"].as_str().unwrap_or("changed");
            if state == "context_only" {
                if changed_ids.contains(entity_id) || !out.context_seen.insert(entity_id.to_string()) {
                    continue;
                }
                out.context_total += 1;
                if out.context.len() < budget.max_context {
                    let qualified = entity["qualified_name"].as_str().unwrap_or(entity_id);
                    out.context.push(json!({
                        "state": "unchanged_context",
                        "reason": "file_changed_entity_anchor_untouched",
                        "entity": {
                            "entity_id": entity_id,
                            "qualified_name": qualified,
                            "path": context_path,
                            "line": entity["line_start"],
                            "line_end": entity["line_end"],
                        },
                        "side": "base",
                    }));
                }
                continue;
            }
            if !matches!(state, "changed" | "added" | "deleted") {
                continue;
            }
            out.findings_total += 1;

            let incoming = incoming_by_target
                .get(entity_id)
                .cloned()
                .unwrap_or_default();
            let mut confirmed = 0usize;
            let mut candidate = 0usize;
            for relation in &incoming {
                let resolution = relation["resolution"].as_str().unwrap_or("unknown");
                let relation_kind = relation["kind"].as_str().unwrap_or("unknown");
                *out.relation_counts
                    .entry((relation_kind.to_string(), resolution.to_string()))
                    .or_default() += 1;
                if resolution == "candidate" {
                    candidate += 1;
                } else if matches!(resolution, "declared" | "resolved" | "observed") {
                    confirmed += 1;
                }
                if let Some(source_id) = relation["source_id"].as_str() {
                    if !changed_ids.contains(source_id)
                        && !out.context_seen.contains(source_id)
                    {
                        if let Some(source) = load_entity(
                            store,
                            request.project_id,
                            snapshot_id,
                            source_id,
                            &mut out.entity_cache,
                        )? {
                            out.context_seen.insert(source_id.to_string());
                            let (source_path, source_line) = entity_anchor(&source);
                            out.context_total += 1;
                            if out.context.len() < budget.max_context {
                                out.context.push(json!({
                                    "state": "unchanged_context",
                                    "reason": "relation_target_changed",
                                    "entity": {
                                        "entity_id": source_id,
                                        "qualified_name": source["qualified_name"],
                                        "path": source_path,
                                        "line": source_line,
                                    },
                                    "relation": {
                                        "relation_id": relation["relation_id"],
                                        "kind": relation_kind,
                                        "resolution": resolution,
                                    },
                                    "side": "base",
                                }));
                            }
                        }
                    }
                }
            }
            let mut evidence = vec![json!({
                "kind": "file_line",
                "path": path,
                "line": entity["line_start"],
                "side": "base",
            })];
            for relation in incoming.iter().take(budget.max_relation_items) {
                let resolution = relation["resolution"].as_str().unwrap_or("unknown");
                let relation_kind = relation["kind"].as_str().unwrap_or("unknown");
                let mut evidence_item = json!({
                    "kind": "relation",
                    "ref": relation["relation_id"],
                    "relation_kind": relation_kind,
                    "resolution": resolution,
                    "direction": "incoming",
                });
                if let Some(source_id) = relation["source_id"].as_str() {
                    evidence_item["other_entity_id"] = json!(source_id);
                    if let Some(source) = load_entity(
                        store,
                        request.project_id,
                        snapshot_id,
                        source_id,
                        &mut out.entity_cache,
                    )? {
                        let (source_path, source_line) = entity_anchor(&source);
                        evidence_item["other_qualified_name"] = source["qualified_name"].clone();
                        if source_path.is_some() {
                            evidence_item["path"] = json!(source_path);
                        }
                        if source_line.is_some() {
                            evidence_item["line"] = json!(source_line);
                        }
                    }
                }
                evidence.push(evidence_item);
            }

            let severity = if confirmed > 0 {
                "high"
            } else if candidate > 0 {
                "info"
            } else if state == "added" {
                "medium"
            } else if state == "deleted" {
                "low"
            } else {
                "info"
            };
            let why = match (state, confirmed, candidate) {
                (_, _, _) if confirmed > 0 => {
                    "this entity changed and has confirmed incoming relation(s) in the base snapshot"
                }
                (_, _, _) if candidate > 0 => {
                    "this entity changed; incoming relations are static candidates, not resolved calls"
                }
                ("deleted", _, _) => "this entity was deleted from the base snapshot",
                ("added", _, _) => "this entity was added; it has no base-snapshot relation",
                _ => "this entity changed; no incoming static relation was found in the base snapshot",
            };
            if out.findings.len() < budget.max_findings {
                let confidence = if confirmed > 0 {
                    "confirmed"
                } else if candidate > 0 {
                    "candidate"
                } else {
                    "confirmed"
                };
                out.findings.push(finding(
                    &format!("F{}", out.findings.len() + 1),
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
                        "snapshot_id": snapshot_id,
                        "side": "base",
                    }),
                    evidence,
                    why,
                    confidence,
                    "archmeld_source",
                    json!({"entity_id": entity_id, "snapshot_id": snapshot_id}),
                ));
            }
        }
    }
    Ok(())
}

fn build_change_gaps(
    files: &[Value],
    coverage: &Value,
    snapshot_id: &str,
    budget: BriefBudget,
) -> (Vec<Value>, usize) {
    // Gaps are per file and keep their path. Do not collapse an unmapped file
    // into one anonymous finding: the path is the actionable fact.
    let mut gap_map: BTreeMap<String, (String, String)> = BTreeMap::new();
    if let Some(unmapped) = coverage["unmapped_files"].as_array() {
        for path in unmapped.iter().filter_map(Value::as_str) {
            gap_map.insert(
                path.to_string(),
                ("unmapped".to_string(), "no base entity mapping".to_string()),
            );
        }
    }
    if let Some(unsupported) = coverage["unsupported_files"].as_array() {
        for path in unsupported.iter().filter_map(Value::as_str) {
            gap_map.insert(
                path.to_string(),
                (
                    "unsupported".to_string(),
                    "file could not be reviewed as text".to_string(),
                ),
            );
        }
    }
    if let Some(omitted) = coverage["omitted_base_files"].as_array() {
        for item in omitted {
            if let Some(path) = item["path"].as_str() {
                gap_map.insert(
                    path.to_string(),
                    (
                        "omitted".to_string(),
                        item["reason"]
                            .as_str()
                            .unwrap_or("base file omitted from the eligible scan")
                            .to_string(),
                    ),
                );
            }
        }
    }
    for file in files {
        if file["coverage"]["state"] == "partial" {
            if let Some(path) = file["path"].as_str() {
                gap_map.entry(path.to_string()).or_insert_with(|| {
                    (
                        "partial".to_string(),
                        file["coverage"]["reason"]
                            .as_str()
                            .unwrap_or("file review is partial")
                            .to_string(),
                    )
                });
            }
        }
    }
    let gap_total = gap_map.len();
    let mut gaps = Vec::new();
    for (path, (kind, reason)) in gap_map.into_iter().take(budget.max_gaps) {
        let severity = if kind == "unsupported" { "low" } else { "medium" };
        gaps.push(json!({
            "kind": kind,
            "severity": severity,
            "path": path,
            "reason": reason,
            "side": "both",
            "next_action": {
                "tool": "archmeld_review",
                "args": {
                    "base_snapshot_id": snapshot_id,
                    "detail": "full",
                    "max_files": 500
                }
            }
        }));
    }
    (gaps, gap_total)
}

fn summarize_relations(
    relations: &[Value],
    relation_counts: BTreeMap<(String, String), u64>,
    budget: BriefBudget,
) -> (usize, usize, Vec<Value>, usize) {
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
    let mut relation_items: Vec<Value> = relation_counts
        .into_iter()
        .map(|((kind, resolution), count)| {
            json!({"kind": kind, "resolution": resolution, "count": count})
        })
        .collect();
    relation_items.sort_by(|left, right| {
        right["count"]
            .as_u64()
            .cmp(&left["count"].as_u64())
            .then_with(|| left["kind"].as_str().cmp(&right["kind"].as_str()))
            .then_with(|| left["resolution"].as_str().cmp(&right["resolution"].as_str()))
    });
    let relation_total = relation_items.len();
    relation_items.truncate(budget.max_relation_items);
    (declared, candidate, relation_items, relation_total)
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
    confidence: &str,
    next_tool: &str,
    next_args: Value,
) -> Value {
    json!({
        "id": id,
        "severity": severity,
        "category": category,
        "confidence": confidence,
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
        "- confirmed: {} · candidate: {} · unknown: {} · stale: {} · findings: {} · context: {} · gaps: {}\n",
        counts["confirmed"]
            .as_u64()
            .or_else(|| counts["declared"].as_u64())
            .unwrap_or(0),
        counts["candidate"].as_u64().unwrap_or(0),
        counts["unknown"].as_u64().unwrap_or(0),
        counts["stale"].as_u64().unwrap_or(0),
        counts["findings"].as_u64().unwrap_or(0),
        counts["context"].as_u64().unwrap_or(0),
        counts["gaps"].as_u64().unwrap_or(0),
    ));
    if let Some(budget) = brief.get("budget") {
        out.push_str(&format!(
            "- budget: findings {}/{} · context {}/{} · gaps {}/{} · relations {} (truncated={})\n",
            budget["returned_findings"].as_u64().unwrap_or(0),
            budget["max_findings"].as_u64().unwrap_or(0),
            budget["returned_context"].as_u64().unwrap_or(0),
            budget["max_context"].as_u64().unwrap_or(0),
            budget["returned_gaps"].as_u64().unwrap_or(0),
            budget["max_gaps"].as_u64().unwrap_or(0),
            budget["max_relation_items"].as_u64().unwrap_or(0),
            budget["truncated"].as_bool().unwrap_or(false),
        ));
    }
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
    let context = brief["context"].as_array().cloned().unwrap_or_default();
    if !context.is_empty() {
        out.push_str("\n## Unchanged context\n\n");
        for entry in context {
            let entity = &entry["entity"];
            out.push_str(&format!(
                "- `{}` — {}{}\n",
                entity["qualified_name"].as_str().unwrap_or("?"),
                entry["reason"].as_str().unwrap_or("context"),
                entity["path"]
                    .as_str()
                    .map(|path| format!(" at `{path}`"))
                    .unwrap_or_default(),
            ));
            if let Some(relation) = entry.get("relation") {
                out.push_str(&format!(
                    "  - relation `{}` ({}, {})\n",
                    relation["relation_id"].as_str().unwrap_or("?"),
                    relation["kind"].as_str().unwrap_or("?"),
                    relation["resolution"].as_str().unwrap_or("?"),
                ));
            }
        }
    }
    let gaps = brief["gaps"].as_array().cloned().unwrap_or_default();
    if !gaps.is_empty() {
        out.push_str("\n## Gaps\n\n");
        for gap in gaps {
            out.push_str(&format!(
                "- `{}` — {} ({})\n",
                gap["path"].as_str().unwrap_or("?"),
                gap["reason"].as_str().unwrap_or("incomplete review"),
                gap["kind"].as_str().unwrap_or("gap"),
            ));
        }
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
