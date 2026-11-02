// Copyright 2026 ARCHMELD contributors
// SPDX-License-Identifier: Apache-2.0

//! Bounded, traceable ContextPacks (S12 §2).
//!
//! A pack is built only from a frozen snapshot. Every selected source range
//! carries its relative path, blob digest and range. The builder reserves space
//! for objective, constraints, claims and counterexamples first; whatever does
//! not fit is listed as an omission with a resume cursor instead of being
//! silently dropped or described as complete.

use crate::error::{CoreError, Result};
use crate::hash;
use crate::source::LineIndex;
use crate::store::{validate, Store};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::collections::{BTreeMap, BTreeSet};

const CONTEXT_VERSION: &str = "1.0";
const SNIPPET_BYTES: usize = 1200;
const MAX_ENTITY_METADATA: usize = 80;
const MAX_RELATIONS: usize = 300;
const RESUME_BYTES: usize = 200;
const MAX_SEEDS: usize = 8;
const MAX_CANDIDATES: usize = 200;
const MAX_OBJECTIVE_TOKENS: usize = 16;
const MAX_OBJECTIVE_SCAN_BYTES: u64 = 64 * 1024 * 1024;
const CLAIM_BUDGET_PERCENT: usize = 35;

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
struct ContextRequest {
    project_id: String,
    snapshot_id: String,
    objective: String,
    non_goals: Option<Vec<String>>,
    entity_ids: Vec<String>,
    budget_tokens: u64,
    changeset_id: Option<String>,
    #[serde(default)]
    cursor: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct ContextCursor {
    /// The request digest includes the snapshot id, so the cursor is bound to
    /// both without repeating a 64-character digest in every small pack.
    request_digest: String,
    offset: u64,
}

#[derive(Debug, Clone)]
struct CandidateSelection {
    ids: Vec<String>,
    scores: BTreeMap<String, i64>,
    matched_by: BTreeMap<String, BTreeSet<String>>,
    names: BTreeMap<String, String>,
    seeds: BTreeSet<String>,
    missing_requested: Vec<String>,
    seed_source: &'static str,
    fallback: bool,
    objective_terms: Vec<String>,
    source_scanned_bytes: u64,
    source_scan_truncated: bool,
}

fn context_request_digest(raw: &Value) -> Result<String> {
    let mut normalized = raw.clone();
    if let Some(object) = normalized.as_object_mut() {
        object.remove("cursor");
    }
    hash::digest_value(&normalized)
        .map_err(|error| CoreError::invalid(format!("context request digest: {error}")))
}

fn encode_context_cursor(cursor: &ContextCursor) -> Result<String> {
    let digest = cursor
        .request_digest
        .strip_prefix("sha256:")
        .unwrap_or(&cursor.request_digest);
    if digest.len() != 64 || !digest.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        return Err(CoreError::Storage(
            "context cursor request digest is not sha256".to_string(),
        ));
    }
    Ok(format!("k1:{}:{digest}", cursor.offset))
}

fn decode_context_cursor(token: &str) -> Result<ContextCursor> {
    let rest = token
        .strip_prefix("k1:")
        .ok_or_else(|| CoreError::invalid("context cursor has an unknown format"))?;
    let (offset, digest) = rest
        .split_once(':')
        .ok_or_else(|| CoreError::invalid("context cursor payload is corrupt"))?;
    let offset = offset
        .parse::<u64>()
        .map_err(|_| CoreError::invalid("context cursor offset is invalid"))?;
    if digest.len() != 64 || !digest.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        return Err(CoreError::invalid("context cursor digest is invalid"));
    }
    Ok(ContextCursor {
        request_digest: format!("sha256:{digest}"),
        offset,
    })
}

fn objective_tokens(objective: &str) -> Vec<String> {
    const STOPWORDS: &[&str] = &[
        "the", "and", "for", "with", "this", "that", "from", "into", "without", "whether",
        "what", "when", "where", "which", "while", "should", "would", "could", "have", "has",
        "are", "was", "were", "its", "our", "your", "their", "about", "after", "before",
        "between", "code", "changes", "files", "project", "snapshot", "using", "make",
    ];
    let mut tokens = Vec::new();
    for word in objective
        .split(|character: char| !character.is_alphanumeric() && character != '_')
    {
        let word = word.to_lowercase();
        if word.len() < 3 || STOPWORDS.contains(&word.as_str()) {
            continue;
        }
        if !tokens.contains(&word) {
            tokens.push(word);
            if tokens.len() >= MAX_OBJECTIVE_TOKENS {
                break;
            }
        }
    }
    tokens
}

fn escape_like(text: &str) -> String {
    text.replace('\\', "\\\\")
        .replace('%', "\\%")
        .replace('_', "\\_")
}

fn add_candidate(
    scores: &mut BTreeMap<String, i64>,
    matched_by: &mut BTreeMap<String, BTreeSet<String>>,
    id: &str,
    score: i64,
    reason: &str,
) {
    scores
        .entry(id.to_string())
        .and_modify(|current| *current = current.saturating_add(score))
        .or_insert(score);
    matched_by
        .entry(id.to_string())
        .or_default()
        .insert(reason.to_string());
}

fn relation_weight(kind: &str) -> i64 {
    match kind {
        "implements" | "declares" | "contains" | "resolved_call" | "observed_call" => 40,
        "static_call_candidate" | "references" | "imports" => 25,
        "reads" | "writes" | "publishes" | "consumes" | "awaits" | "schedules" => 15,
        _ => 5,
    }
}

fn select_candidates(
    store: &Store,
    request: &ContextRequest,
    relations: &[Value],
) -> Result<CandidateSelection> {
    let objective_terms = objective_tokens(&request.objective);
    let mut scores: BTreeMap<String, i64> = BTreeMap::new();
    let mut matched_by: BTreeMap<String, BTreeSet<String>> = BTreeMap::new();
    let mut names: BTreeMap<String, String> = BTreeMap::new();
    let mut missing_requested = Vec::new();
    let mut requested = request.entity_ids.clone();
    requested.sort();
    requested.dedup();
    for id in &requested {
        match store.get_entity(&request.project_id, &request.snapshot_id, id)? {
            Some(entity) => {
                let name = entity["qualified_name"].as_str().unwrap_or("").to_string();
                add_candidate(&mut scores, &mut matched_by, id, 1000, "requested");
                names.insert(id.clone(), name);
            }
            None => missing_requested.push(id.clone()),
        }
    }
    let mut seed_source = "explicit";
    let mut fallback = false;
    let mut source_scanned_bytes = 0u64;
    let mut source_scan_truncated = false;
    if request.entity_ids.is_empty() {
        for term in &objective_terms {
            let pattern = format!("%{}%", escape_like(term));
            let mut statement = store.conn.prepare(
                "SELECT payload_json FROM entities
                 WHERE project_id=?1 AND snapshot_id=?2
                   AND LOWER(qualified_name) LIKE ?3 ESCAPE '\\'
                 LIMIT 500",
            )?;
            let rows = statement.query_map(
                (&request.project_id, &request.snapshot_id, &pattern),
                |row| row.get::<_, String>(0),
            )?;
            for row in rows {
                let payload: Value = serde_json::from_str(&row?).map_err(|error| {
                    CoreError::Storage(format!("entity payload corrupt: {error}"))
                })?;
                let Some(id) = payload["id"].as_str() else {
                    continue;
                };
                let name = payload["qualified_name"].as_str().unwrap_or("");
                let lower = name.to_lowercase();
                let short = lower.rsplit("::").next().unwrap_or(&lower);
                let score = if lower == *term {
                    140
                } else if short == term {
                    110
                } else {
                    40 + (term.len() as i64).min(10)
                };
                add_candidate(&mut scores, &mut matched_by, id, score, term);
                names
                    .entry(id.to_string())
                    .or_insert_with(|| name.to_string());
            }
        }
        let (source_scores, scanned, truncated) =
            objective_source_scores(store, request, &objective_terms)?;
        source_scanned_bytes = scanned;
        source_scan_truncated = truncated;
        for (id, (score, terms)) in source_scores {
            for term in terms {
                add_candidate(&mut scores, &mut matched_by, &id, score, &term);
            }
            if !names.contains_key(&id) {
                if let Some(entity) =
                    store.get_entity(&request.project_id, &request.snapshot_id, &id)?
                {
                    names.insert(
                        id.clone(),
                        entity["qualified_name"].as_str().unwrap_or("").to_string(),
                    );
                }
            }
        }
        if scores.is_empty() {
            let mut ids = store.entity_ids(&request.project_id, &request.snapshot_id)?;
            ids.truncate(MAX_ENTITY_METADATA);
            for id in ids {
                if let Some(entity) =
                    store.get_entity(&request.project_id, &request.snapshot_id, &id)?
                {
                    add_candidate(&mut scores, &mut matched_by, &id, 1, "fallback");
                    names.insert(
                        id.clone(),
                        entity["qualified_name"].as_str().unwrap_or("").to_string(),
                    );
                }
            }
            seed_source = "fallback";
            fallback = true;
        } else {
            seed_source = "objective";
        }
    }

    let mut ranked: Vec<(String, i64)> = scores
        .iter()
        .map(|(id, score)| (id.clone(), *score))
        .collect();
    ranked.sort_by(|left, right| right.1.cmp(&left.1).then(left.0.cmp(&right.0)));
    let seeds: BTreeSet<String> = ranked
        .iter()
        .take(MAX_SEEDS)
        .map(|(id, _)| id.clone())
        .collect();

    // One bounded hop from the seeds. A neighbor of several seeds accumulates
    // relevance; a candidate that is itself a seed is never replaced.
    let mut neighbor_scores: BTreeMap<String, i64> = BTreeMap::new();
    for relation in relations {
        let Some(source) = relation["source_id"].as_str() else {
            continue;
        };
        let Some(target) = relation["target_id"].as_str() else {
            continue;
        };
        let source_seed = seeds.contains(source);
        let target_seed = seeds.contains(target);
        if source_seed == target_seed {
            continue;
        }
        let (neighbor, reason) = if source_seed {
            (target, "outgoing_neighbor")
        } else {
            (source, "incoming_neighbor")
        };
        let weight = relation_weight(relation["kind"].as_str().unwrap_or(""));
        let entry = neighbor_scores.entry(neighbor.to_string()).or_insert(0);
        *entry = entry.saturating_add(weight);
        matched_by
            .entry(neighbor.to_string())
            .or_default()
            .insert(reason.to_string());
    }
    for (id, score) in neighbor_scores {
        scores
            .entry(id.clone())
            .and_modify(|current| *current = current.saturating_add(score))
            .or_insert(score);
        if !names.contains_key(&id) {
            if let Some(entity) = store.get_entity(&request.project_id, &request.snapshot_id, &id)? {
                names.insert(id.clone(), entity["qualified_name"].as_str().unwrap_or("").to_string());
            }
        }
    }
    let mut ranked: Vec<(String, i64)> = scores
        .iter()
        .map(|(id, score)| (id.clone(), *score))
        .collect();
    ranked.sort_by(|left, right| right.1.cmp(&left.1).then(left.0.cmp(&right.0)));
    ranked.truncate(MAX_CANDIDATES);
    let ids = ranked.iter().map(|(id, _)| id.clone()).collect();
    Ok(CandidateSelection {
        ids,
        scores,
        matched_by,
        names,
        seeds,
        missing_requested,
        seed_source,
        fallback,
        objective_terms,
        source_scanned_bytes,
        source_scan_truncated,
    })
}

fn objective_source_scores(
    store: &Store,
    request: &ContextRequest,
    terms: &[String],
) -> Result<(BTreeMap<String, (i64, BTreeSet<String>)>, u64, bool)> {
    if terms.is_empty() {
        return Ok((BTreeMap::new(), 0, false));
    }
    let mut statement = store.conn.prepare(
        "SELECT path, blob_digest FROM files
         WHERE project_id=?1 AND snapshot_id=?2 ORDER BY path",
    )?;
    let rows = statement.query_map(
        (&request.project_id, &request.snapshot_id),
        |row| Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?)),
    )?;
    let mut found: BTreeMap<String, (i64, BTreeSet<String>)> = BTreeMap::new();
    let mut scanned = 0u64;
    let mut truncated = false;
    let mut span_cache: BTreeMap<String, Vec<(String, u64, u64)>> = BTreeMap::new();
    for row in rows {
        let (path, digest) = row?;
        if scanned >= MAX_OBJECTIVE_SCAN_BYTES {
            truncated = true;
            break;
        }
        let Some(blob) = store.read_blob_bytes(&digest)? else {
            continue;
        };
        scanned += blob.len() as u64;
        let Ok(text) = std::str::from_utf8(&blob) else {
            continue;
        };
        let index = LineIndex::new(&blob);
        let mut spans: Option<Vec<(String, u64, u64)>> = None;
        for term in terms {
            let mut from = 0usize;
            let mut hits = 0usize;
            while let Some(relative) = text[from..].find(term) {
                let start = from + relative;
                let (line0, _) = index.line_col(start as u64).unwrap_or((0, 0));
                let line = line0 + 1;
                if spans.is_none() {
                    let loaded = match span_cache.get(&path) {
                        Some(spans) => spans.clone(),
                        None => {
                            let spans = crate::query::entity_spans_for_path(
                                store,
                                &request.project_id,
                                &request.snapshot_id,
                                &path,
                            )?;
                            span_cache.insert(path.clone(), spans.clone());
                            spans
                        }
                    };
                    spans = Some(loaded);
                }
                for (id, first, last) in spans.as_ref().expect("spans loaded above") {
                    if *first <= line && line <= *last {
                        let entry = found.entry(id.clone()).or_insert_with(|| (0, BTreeSet::new()));
                        entry.0 += 30 + (term.len() as i64).min(20) * 2;
                        entry.1.insert(term.clone());
                    }
                }
                hits += 1;
                if hits >= 100 {
                    break;
                }
                from = start + term.len().max(1);
            }
        }
    }
    Ok((found, scanned, truncated))
}

fn claim_is_relevant(
    claim: &Value,
    terms: &[String],
    candidate_ids: &BTreeSet<String>,
    names: &BTreeMap<String, String>,
) -> bool {
    let text = format!(
        "{} {}",
        claim["text"].as_str().unwrap_or(""),
        claim["scope"].as_str().unwrap_or("")
    )
    .to_lowercase();
    if terms.iter().any(|term| text.contains(term)) {
        return true;
    }
    for id in claim["support_ids"]
        .as_array()
        .into_iter()
        .flatten()
        .chain(
            claim["counterevidence_ids"]
                .as_array()
                .into_iter()
                .flatten(),
        )
    {
        if let Some(id) = id.as_str() {
            if candidate_ids.contains(id) {
                return true;
            }
        }
    }
    for name in names.values() {
        if let Some(short) = name.rsplit("::").next() {
            if short.len() >= 3 && text.contains(&short.to_lowercase()) {
                return true;
            }
        }
    }
    false
}

pub fn create_context_pack(store: &mut Store, raw: &Value) -> Result<Value> {
    let request: ContextRequest = serde_json::from_value(raw.clone())
        .map_err(|err| CoreError::invalid(format!("context request: {err}")))?;
    validate::require_id("project_id", &request.project_id)?;
    validate::require_digest("snapshot_id", &request.snapshot_id)?;
    validate::require_non_empty("objective", &request.objective)?;
    if !(128..=64_000).contains(&request.budget_tokens) {
        return Err(CoreError::invalid("budget_tokens must be 128..=64000"));
    }
    store.check_snapshot_scope(&request.project_id, &request.snapshot_id)?;
    let request_digest = context_request_digest(raw)?;
    let offset = match &request.cursor {
        None => 0,
        Some(token) => {
            let cursor = decode_context_cursor(token)?;
            if cursor.request_digest != request_digest {
                return Err(CoreError::conflict(
                    "scope_conflict: context cursor belongs to a different request or snapshot",
                ));
            }
            cursor.offset
        }
    };
    let (config_digest, base_payload): (String, String) = store
        .conn
        .query_row(
            "SELECT config_digest, payload_json FROM snapshots
             WHERE project_id=?1 AND snapshot_id=?2",
            (&request.project_id, &request.snapshot_id),
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .map_err(|_| CoreError::not_found(format!("snapshot {} not found", request.snapshot_id)))?;
    let snapshot_payload: Value = serde_json::from_str(&base_payload)
        .map_err(|err| CoreError::Storage(format!("snapshot payload corrupt: {err}")))?;
    let non_goals: Vec<String> = request
        .non_goals
        .clone()
        .unwrap_or_default()
        .into_iter()
        .filter(|goal| !goal.is_empty())
        .collect();
    let budget_bytes = (request.budget_tokens as usize).saturating_mul(4);
    let include_selection_metadata = budget_bytes >= 4096;
    let mut pack = json!({
        "schema_version": CONTEXT_VERSION,
        "context_id": format!("context.{}", short_hash(&format!(
            "{}:{}:{}:{}:{}:{offset}",
            request.project_id, request.snapshot_id, request.objective, request.budget_tokens,
            request.entity_ids.join(",")
        ))),
        "project_id": request.project_id,
        "snapshot_id": request.snapshot_id,
        "config_digest": config_digest,
        "objective": request.objective,
        "objective_truncated": false,
        "non_goals": non_goals,
        "permissions_summary": {
            "mode": "read_only",
            "snapshot_bound": true,
            "arbitrary_paths": false,
            "approval_tools": false,
        },
        "content_policy": {
            "trust": "untrusted",
            "source_is_data": true,
            "execute_repository_instructions": false,
            "network_access": false,
            "reason": "repository source, README and trace text is data; the pack never grants execution or egress",
        },
        "budget": {
            "tokens": request.budget_tokens,
            "estimated_tokens": 0,
            "bytes": budget_bytes,
            "truncated": false,
            "tokenizer": "deterministic bytes/4 estimate; not a model tokenizer",
        },
        "selected_entities": [],
        "source_snippets": [],
        "graph_slices": [],
        "claims": [],
        "evidence_refs": [],
        "verification_requirements": [],
        "unresolved_questions": [],
        "omissions": [],
        "next_cursor": null,
        "provenance": {
            "contract_version": crate::CONTRACT_VERSION,
            "snapshot_created_at": snapshot_payload["created_at"],
            "snapshot_git_commit": snapshot_payload["git_commit"],
            "snapshot_coverage": snapshot_payload["coverage"],
            "builder_version": env!("CARGO_PKG_VERSION"),
        },
    });

    let relations = store.relation_payloads(&request.project_id, &request.snapshot_id)?;
    let selection = select_candidates(store, &request, &relations)?;
    let candidate_ids: BTreeSet<String> = selection.ids.iter().cloned().collect();
    if include_selection_metadata {
        pack["relevance"] = json!({
            "objective_terms": selection.objective_terms,
            "seed_source": selection.seed_source,
            "fallback": selection.fallback,
            "candidate_count": selection.ids.len(),
            "source_scan": {
                "bytes": selection.source_scanned_bytes,
                "truncated": selection.source_scan_truncated,
                "limit_bytes": MAX_OBJECTIVE_SCAN_BYTES,
            },
        });
    }

    let mut omissions: Vec<Value> = Vec::new();
    let mut omissions_truncated = 0usize;
    let mut counterexample_preserved = false;

    // Claims and counterexamples come before large source bodies. Claims that
    // cannot be tied to the objective or the candidate slice are aggregated as
    // omissions instead of being allowed to crowd out the implementation.
    let claims = latest_claims(store, &request.project_id, &request.snapshot_id)?;
    let mut relevant_claims = Vec::new();
    let mut preserved_irrelevant = Vec::new();
    let mut irrelevant_claims = 0usize;
    for claim in claims {
        if claim_is_relevant(&claim, &selection.objective_terms, &candidate_ids, &selection.names) {
            relevant_claims.push(claim);
        } else if matches!(
            claim["status"].as_str().unwrap_or(""),
            "rejected" | "needs_review"
        ) && preserved_irrelevant.len() < 2
        {
            preserved_irrelevant.push(claim);
        } else {
            irrelevant_claims += 1;
        }
    }
    let mut ordered_claims = relevant_claims;
    ordered_claims.sort_by_key(|claim| {
        let status = claim["status"].as_str().unwrap_or("");
        match status {
            "rejected" => 0,
            "needs_review" => 1,
            "proposed" => 2,
            _ => 3,
        }
    });
    let mut preserved_irrelevant = preserved_irrelevant;
    preserved_irrelevant.sort_by_key(|claim| {
        if claim["status"] == "rejected" {
            0
        } else {
            1
        }
    });
    ordered_claims.extend(preserved_irrelevant);
    if irrelevant_claims > 0 {
        omissions.push(json!({
            "kind": "claim",
            "count": irrelevant_claims,
            "reason": "irrelevant_to_objective",
            "note": "claims whose text, scope and evidence are unrelated to the objective and candidate slice",
        }));
    }
    let claim_budget_start = serde_json::to_string(&pack)
        .map(|text| text.len())
        .unwrap_or(0);
    let claim_budget = budget_bytes.saturating_mul(CLAIM_BUDGET_PERCENT) / 100;
    let mut claim_refs: BTreeSet<String> = BTreeSet::new();
    let mut claim_budget_exhausted = false;
    for claim in &ordered_claims {
        if serde_json::to_string(&pack)
            .map(|text| text.len())
            .unwrap_or(0)
            .saturating_sub(claim_budget_start)
            > claim_budget
        {
            claim_budget_exhausted = true;
            break;
        }
        let mut summary = json!({
            "claim_id": claim["id"],
            "claim_type": claim["claim_type"],
            "status": claim["status"],
            "text": claim["text"],
            "support_ids": claim["support_ids"],
            "counterevidence_ids": claim["counterevidence_ids"],
            "unresolved_questions": claim["unresolved_questions"],
            "scope": claim["scope"],
        });
        if claim["status"] == "rejected" {
            counterexample_preserved = true;
        }
        for id in claim["support_ids"].as_array().into_iter().flatten().chain(
            claim["counterevidence_ids"]
                .as_array()
                .into_iter()
                .flatten(),
        ) {
            if let Some(id) = id.as_str() {
                claim_refs.insert(id.to_string());
            }
        }
        if append_fits(&pack, &summary, budget_bytes, RESUME_BYTES) {
            pack["claims"].as_array_mut().unwrap().push(summary);
        } else if claim["status"] == "rejected" || claim["status"] == "needs_review" {
            // Preserve the decisive counterexample metadata even at a very
            // small budget; only the prose may be clipped.
            let text = summary["text"].as_str().unwrap_or("").to_string();
            let mut clipped = String::new();
            for ch in text.chars() {
                clipped.push(ch);
                summary["text"] = json!(format!("{clipped}…"));
                if append_fits(&pack, &summary, budget_bytes, RESUME_BYTES) {
                    break;
                }
            }
            if append_fits(&pack, &summary, budget_bytes, RESUME_BYTES) {
                pack["claims"].as_array_mut().unwrap().push(summary);
            } else {
                omissions.push(json!({
                    "kind": "claim",
                    "id": claim["id"],
                    "status": claim["status"],
                    "reason": "budget",
                }));
                omissions_truncated += 1;
            }
        } else {
            omissions.push(json!({
                "kind": "claim",
                "id": claim["id"],
                "status": claim["status"],
                "reason": "budget",
            }));
            omissions_truncated += 1;
        }
    }

    if claim_budget_exhausted {
        let packed_ids: BTreeSet<String> = pack["claims"]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(|claim| claim["claim_id"].as_str().map(str::to_string))
            .collect();
        let mut other_claims = 0usize;
        for claim in &ordered_claims {
            let Some(id) = claim["id"].as_str() else {
                continue;
            };
            if packed_ids.contains(id) {
                continue;
            }
            let status = claim["status"].as_str().unwrap_or("");
            if matches!(status, "rejected" | "needs_review") {
                omissions.push(json!({
                    "kind": "claim",
                    "id": id,
                    "status": status,
                    "reason": "claim_budget",
                }));
            } else {
                other_claims += 1;
            }
        }
        if other_claims > 0 {
            omissions.push(json!({
                "kind": "claim_budget",
                "count": other_claims,
                "reason": "claim_budget",
                "note": "relevant claims beyond the claim share of the budget are omitted; they are not silently discarded",
            }));
        }
        let remaining = ordered_claims.len().saturating_sub(packed_ids.len());
        omissions_truncated += remaining;
    }

    // Entities: explicit or objective-derived seeds first, then a bounded
    // one-hop neighborhood. The cursor is an offset into this deterministic
    // candidate order, so a resumed pack continues the same sequence.
    let mut next_cursor: Option<String> = None;
    let mut selected_entities: BTreeSet<String> = BTreeSet::new();
    for (index, entity_id) in selection.ids.iter().enumerate().skip(offset as usize) {
        let Some(entity) =
            store.get_entity(&request.project_id, &request.snapshot_id, entity_id)?
        else {
            continue;
        };
        let metadata = json!({
            "entity_id": entity["id"],
            "kind": entity["kind"],
            "language": entity["language"],
            "qualified_name": entity["qualified_name"],
            "config_digest": entity["config_digest"],
            "anchor_count": entity["anchors"].as_array().map(Vec::len).unwrap_or(0),
            "relevance": {
                "score": selection.scores.get(entity_id),
                "matched_by": selection.matched_by.get(entity_id),
                "seed": selection.seeds.contains(entity_id),
            },
        });
        if !append_fits(&pack, &metadata, budget_bytes, RESUME_BYTES) {
            omissions.push(json!({
                "kind": "entity",
                "id": entity_id,
                "reason": "budget",
            }));
            omissions_truncated += 1;
            next_cursor = Some(encode_context_cursor(&ContextCursor {
                request_digest: request_digest.clone(),
                offset: index as u64,
            })?);
            break;
        }
        pack["selected_entities"]
            .as_array_mut()
            .unwrap()
            .push(metadata);
        selected_entities.insert(entity_id.clone());
        for anchor in entity["anchors"].as_array().into_iter().flatten() {
            if let Some(snippet) = source_snippet(store, anchor)? {
                if append_fits(&pack, &snippet, budget_bytes, RESUME_BYTES) {
                    pack["source_snippets"]
                        .as_array_mut()
                        .unwrap()
                        .push(snippet);
                } else {
                    omissions.push(json!({
                        "kind": "source",
                        "id": snippet["anchor"]["path"],
                        "reason": "budget",
                    }));
                    omissions_truncated += 1;
                }
            }
        }
    }

    if include_selection_metadata {
        pack["page"] = json!({
            "offset": offset,
            "candidate_count": selection.ids.len(),
            "returned_entities": selected_entities.len(),
            "has_more": next_cursor.is_some(),
            "seed_source": selection.seed_source,
            "fallback": selection.fallback,
        });
    }

    // Graph slice touching selected entities, with basis and evidence refs.
    let mut relation_refs: BTreeSet<String> = BTreeSet::new();
    let mut relation_count = 0usize;
    for relation in relations {
        if relation_count >= MAX_RELATIONS {
            omissions.push(json!({
                "kind": "relation_limit",
                "id": null,
                "reason": format!("only {MAX_RELATIONS} relations are included"),
            }));
            omissions_truncated += 1;
            break;
        }
        let source = relation["source_id"].as_str().unwrap_or("");
        let target = relation["target_id"].as_str().unwrap_or("");
        if !selected_entities.contains(source) && !selected_entities.contains(target) {
            continue;
        }
        let summary = json!({
            "relation_id": relation["id"],
            "source_id": source,
            "target_id": target,
            "kind": relation["kind"],
            "basis": relation["basis"],
            "resolution": relation["resolution"],
            "evidence_ids": relation["evidence_ids"],
        });
        if append_fits(&pack, &summary, budget_bytes, RESUME_BYTES) {
            for id in relation["evidence_ids"].as_array().into_iter().flatten() {
                if let Some(id) = id.as_str() {
                    relation_refs.insert(id.to_string());
                }
            }
            pack["graph_slices"].as_array_mut().unwrap().push(summary);
            relation_count += 1;
        } else {
            omissions.push(json!({
                "kind": "relation",
                "id": relation["id"],
                "reason": "budget",
            }));
            omissions_truncated += 1;
        }
    }

    // Evidence summaries for the claims and slices that made it into the pack.
    let evidence = store.evidence_payloads(&request.project_id, &request.snapshot_id)?;
    let mut included_evidence = BTreeSet::new();
    for item in evidence {
        let id = item["id"].as_str().unwrap_or("");
        if !claim_refs.contains(id) && !relation_refs.contains(id) {
            continue;
        }
        let summary = json!({
            "evidence_id": id,
            "kind": item["kind"],
            "producer": item["producer"],
            "coverage": item["coverage"],
            "freshness": item["freshness"],
            "artifact_digest": item["artifact_digest"],
        });
        if append_fits(&pack, &summary, budget_bytes, RESUME_BYTES) {
            pack["evidence_refs"].as_array_mut().unwrap().push(summary);
            included_evidence.insert(id.to_string());
        } else {
            omissions.push(json!({
                "kind": "evidence",
                "id": id,
                "reason": "budget",
            }));
            omissions_truncated += 1;
        }
    }

    // Optional verification requirements from a bound ChangeSet.
    if let Some(changeset_id) = &request.changeset_id {
        if let Ok(changeset) =
            crate::changeset::get_changeset(store, &request.project_id, changeset_id)
        {
            let verification = json!({
                "changeset_id": changeset_id,
                "state": changeset["effective_state"],
                "contract_id": changeset["contract_id"],
                "contract_digest": changeset["contract_digest"],
                "prepared_digest": changeset["prepared_digest"],
                "verification_required": true,
            });
            if append_fits(&pack, &verification, budget_bytes, RESUME_BYTES) {
                pack["verification_requirements"]
                    .as_array_mut()
                    .unwrap()
                    .push(verification);
            } else {
                omissions.push(json!({
                    "kind": "verification_requirements",
                    "id": changeset_id,
                    "reason": "budget",
                }));
                omissions_truncated += 1;
            }
        }
    }

    // Unresolved material is mandatory; add it before the final size check.
    let mut unresolved: Vec<String> = Vec::new();
    for claim in pack["claims"].as_array().into_iter().flatten() {
        for question in claim["unresolved_questions"]
            .as_array()
            .into_iter()
            .flatten()
        {
            if let Some(question) = question.as_str() {
                unresolved.push(question.to_string());
            }
        }
    }
    for id in &selection.missing_requested {
        unresolved.push(format!(
            "requested entity {id} is not present in this snapshot"
        ));
    }
    if selection.fallback {
        unresolved.push(
            "objective matched no symbol or source text; the first entities were selected deterministically, not by relevance"
                .to_string(),
        );
    }
    if selection.source_scan_truncated {
        unresolved.push(
            "objective source scan stopped at the byte limit; relevance may be incomplete"
                .to_string(),
        );
    }
    if next_cursor.is_some() {
        unresolved.push(
            "more candidate entities remain; call again with next_cursor to continue"
                .to_string(),
        );
    }
    if counterexample_preserved {
        unresolved.push(
            "a rejected/counterexample claim is included; do not treat its hypothesis as established"
                .to_string(),
        );
    }
    if omissions_truncated > 0 {
        unresolved.push(format!(
            "{omissions_truncated} item(s) were omitted by budget; use next_cursor or a larger budget"
        ));
    }
    let distinct_questions: BTreeSet<String> = unresolved.into_iter().collect();
    pack["unresolved_questions"] = json!(distinct_questions.into_iter().collect::<Vec<_>>());
    pack["omissions"] = json!(omissions);
    pack["next_cursor"] = next_cursor.map(Value::String).unwrap_or(Value::Null);

    let mut truncated = omissions_truncated > 0;
    while serde_json::to_string(&pack)
        .map(|text| text.len())
        .unwrap_or(usize::MAX)
        .saturating_add(256)
        > budget_bytes
    {
        // Drop the largest optional slice first; unresolved/objective stay.
        let dropped = {
            let mut popped = false;
            for key in [
                "graph_slices",
                "evidence_refs",
                "source_snippets",
                "non_goals",
                "selected_entities",
            ] {
                if let Some(array) = pack[key].as_array_mut() {
                    if array.pop().is_some() {
                        if key != "non_goals" {
                            pack["omissions"]
                                .as_array_mut()
                                .unwrap()
                                .push(json!({"kind": key, "reason": "final_budget_trim"}));
                        }
                        popped = true;
                        break;
                    }
                }
            }
            popped
        };
        let dropped = if dropped {
            dropped
        } else {
            let removed = {
                let object = pack.as_object_mut().unwrap();
                ["page", "relevance"]
                    .into_iter()
                    .find(|key| object.contains_key(*key))
                    .map(|key| {
                        object.remove(key);
                        key.to_string()
                    })
            };
            if let Some(key) = removed {
                pack["omissions"]
                    .as_array_mut()
                    .unwrap()
                    .push(json!({"kind": key, "reason": "final_budget_trim"}));
                true
            } else {
                false
            }
        };
        if !dropped {
            let compacted_omissions = pack["omissions"]
                .as_array()
                .map(|items| items.len())
                .unwrap_or(0)
                > 1;
            if compacted_omissions {
                let count = pack["omissions"].as_array().map(Vec::len).unwrap_or(0);
                pack["omissions"] = json!([{
                    "kind": "omitted",
                    "count": count,
                    "reason": "final_budget_trim",
                }]);
                truncated = true;
                continue;
            }
            let compacted_questions = pack["unresolved_questions"]
                .as_array()
                .map(|items| items.len())
                .unwrap_or(0)
                > 1;
            if compacted_questions {
                let first = pack["unresolved_questions"][0].clone();
                pack["unresolved_questions"] = json!([first]);
                truncated = true;
                continue;
            }
            if pack["provenance"]["snapshot_coverage"].is_object() {
                pack["provenance"]
                    .as_object_mut()
                    .unwrap()
                    .remove("snapshot_coverage");
                truncated = true;
                continue;
            }
            let objective = pack["objective"].as_str().unwrap_or("").to_string();
            let mut chars: Vec<char> = objective.chars().collect();
            if chars.len() > 16 {
                chars.truncate(chars.len() - (chars.len() / 5).max(1));
                pack["objective"] = json!(chars.into_iter().collect::<String>() + "…");
                pack["objective_truncated"] = json!(true);
            } else {
                return Err(CoreError::invalid(
                    "resource_exhausted: budget_tokens is too small for the mandatory context header",
                ));
            }
        }
        truncated = true;
    }
    let estimated_tokens = (serde_json::to_string(&pack)
        .map(|text| text.len())
        .unwrap_or(0)
        .saturating_add(3))
        / 4;
    pack["budget"]["estimated_tokens"] = json!(estimated_tokens);
    pack["budget"]["truncated"] = json!(truncated);
    let mut digest_preimage = pack.clone();
    if let Some(object) = digest_preimage.as_object_mut() {
        object.remove("context_digest");
        object.remove("pack_artifact");
    }
    let context_digest = hash::digest_value(&digest_preimage)
        .map_err(|err| CoreError::invalid(format!("context pack: {err}")))?;
    let mut final_pack = pack;
    final_pack["context_digest"] = json!(context_digest);
    let bytes = hash::canonical_json(&final_pack)
        .map_err(|err| CoreError::invalid(format!("context pack: {err}")))?;
    let artifact = store.put_blob(bytes.as_bytes(), "application/json")?;
    final_pack["pack_artifact"] = json!(artifact);
    Ok(final_pack)
}

fn append_fits(pack: &Value, item: &Value, budget_bytes: usize, reserve: usize) -> bool {
    let base = serde_json::to_string(pack)
        .map(|text| text.len())
        .unwrap_or(usize::MAX);
    let addition = serde_json::to_string(item)
        .map(|text| text.len())
        .unwrap_or(usize::MAX);
    base.saturating_add(addition).saturating_add(reserve) <= budget_bytes
}

fn latest_claims(store: &Store, project_id: &str, snapshot_id: &str) -> Result<Vec<Value>> {
    let mut statement = store.conn.prepare(
        "SELECT claim_id, MAX(revision) FROM claims
         WHERE project_id=?1 AND snapshot_id=?2 GROUP BY claim_id ORDER BY claim_id",
    )?;
    let rows = statement.query_map((project_id, snapshot_id), |row| {
        Ok((row.get::<_, String>(0)?, row.get::<_, i64>(1)?))
    })?;
    let mut claims = Vec::new();
    for row in rows {
        let (claim_id, revision) = row?;
        let payload: Option<String> = store
            .conn
            .query_row(
                "SELECT payload_json FROM claims
                 WHERE project_id=?1 AND claim_id=?2 AND revision=?3",
                (project_id, &claim_id, revision),
                |row| row.get(0),
            )
            .ok();
        if let Some(payload) = payload {
            claims.push(
                serde_json::from_str(&payload)
                    .map_err(|err| CoreError::Storage(format!("claim payload corrupt: {err}")))?,
            );
        }
    }
    Ok(claims)
}

fn source_snippet(store: &Store, anchor: &Value) -> Result<Option<Value>> {
    let Some(path) = anchor["path"].as_str() else {
        return Ok(None);
    };
    let Some(blob_digest) = anchor["blob_digest"].as_str() else {
        return Ok(None);
    };
    let Some(bytes) = store.read_blob_bytes(blob_digest)? else {
        return Ok(None);
    };
    let full = std::str::from_utf8(&bytes).map_err(|_| {
        CoreError::unsupported(format!("{path} is not valid UTF-8; source is unsupported"))
    })?;
    let range = crate::anchor::byte_range(anchor, &bytes)?;
    let (start, end) = (range.start, range.end);
    if !full.is_char_boundary(start as usize) {
        return Err(CoreError::invalid(
            "anchor start splits a UTF-8 character",
        ));
    }
    let slice_end = (start + SNIPPET_BYTES as u64)
        .min(end)
        .min(bytes.len() as u64);
    let mut cut = slice_end as usize;
    while cut > start as usize && !full.is_char_boundary(cut) {
        cut -= 1;
    }
    let text = &full[start as usize..cut];
    Ok(Some(json!({
        "path": path,
        "blob_digest": blob_digest,
        "anchor": {
            "path": path,
            "precision": anchor["precision"],
            "byte_start": anchor["byte_start"],
            "byte_end": anchor["byte_end"],
            "line_start": anchor["line_start"],
            "line_end": anchor["line_end"],
        },
        "range": {
            "byte_start": start,
            "byte_end": end,
            "line_start": anchor["line_start"],
            "line_end": anchor["line_end"],
        },
        "text": text,
        "trust": "untrusted",
        "truncated": cut as u64 != end,
        "blob_size": bytes.len(),
    })))
}

fn short_hash(identity: &str) -> String {
    let digest = hash::digest_bytes(identity.as_bytes());
    let hex = digest.strip_prefix("sha256:").unwrap_or(&digest);
    hex[..32].to_string()
}
