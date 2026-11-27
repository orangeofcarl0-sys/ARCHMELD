// Copyright 2026 ARCHMELD contributors
// SPDX-License-Identifier: Apache-2.0

//! Offline review bundles (S01 §3, S14). A bundle names one explicit snapshot
//! and contains a static HTML report, machine-readable model, evidence index and
//! a checksummed manifest. Source snippets and raw traces are excluded unless a
//! future explicit export path adds them; this exporter never copies them.

use crate::error::{CoreError, Result};
use crate::hash;
use crate::manager::get_project;
use crate::store::{validate, Store};
use serde::Deserialize;
use serde_json::{json, Value};
use std::fs;
use std::path::{Path, PathBuf};

const REPORT_VERSION: &str = "1.0";
const DEFAULT_MAX_ENTITIES: u32 = 5_000;
const DEFAULT_MAX_RELATIONS: u32 = 20_000;
const MAX_ENTITIES_LIMIT: u32 = 200_000;
const MAX_RELATIONS_LIMIT: u32 = 200_000;
/// Opt-in source bytes are bounded so a report never becomes an accidental
/// full-repository copy.
const MAX_SOURCE_BYTES: u64 = 64 * 1024 * 1024;
/// Native and normalized trace artifacts have their own independent budget.
const MAX_TRACE_BYTES: u64 = 256 * 1024 * 1024;
const VIEWER_MAX_NODES: usize = 240;
const VIEWER_MAX_EDGES: usize = 480;

/// One additional file placed in the bundle. Source and trace payloads are
/// only present when the caller opted in; every one is listed in the manifest.
#[derive(Debug, Clone)]
struct ExtraFile {
    path: String,
    bytes: Vec<u8>,
    media_type: String,
    kind: &'static str,
    source_path: Option<String>,
    owners: Option<String>,
}

#[derive(Debug, Default)]
struct Collection {
    files: Vec<ExtraFile>,
    index: Vec<Value>,
    bytes: u64,
    omitted: usize,
}

impl Collection {
    fn state(&self, requested: bool) -> &'static str {
        if !requested {
            "excluded"
        } else if self.files.is_empty() {
            "none"
        } else if self.omitted > 0 {
            "partial"
        } else {
            "included"
        }
    }
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
struct ReportRequest {
    project_id: String,
    snapshot_id: String,
    output_dir: String,
    #[serde(default)]
    include_source: bool,
    #[serde(default)]
    include_raw_traces: bool,
    max_entities: Option<u32>,
    max_relations: Option<u32>,
}

pub fn export_report(store: &mut Store, raw: &Value) -> Result<Value> {
    let request: ReportRequest = serde_json::from_value(raw.clone())
        .map_err(|err| CoreError::invalid(format!("report request: {err}")))?;
    validate::require_id("project_id", &request.project_id)?;
    validate::require_digest("snapshot_id", &request.snapshot_id)?;
    validate::require_non_empty("output_dir", &request.output_dir)?;
    let max_entities = request
        .max_entities
        .unwrap_or(DEFAULT_MAX_ENTITIES)
        .clamp(1, MAX_ENTITIES_LIMIT);
    let max_relations = request
        .max_relations
        .unwrap_or(DEFAULT_MAX_RELATIONS)
        .clamp(1, MAX_RELATIONS_LIMIT);
    let project = get_project(store, &request.project_id)?
        .ok_or_else(|| CoreError::not_found("project is not registered"))?;
    let snapshot = store
        .snapshot_payload(&request.project_id, &request.snapshot_id)?
        .ok_or_else(|| CoreError::not_found("snapshot is not registered for this project"))?;
    let manifest_artifact = store
        .snapshot_manifest_digest(&request.project_id, &request.snapshot_id)?
        .ok_or_else(|| CoreError::Storage("snapshot manifest digest is missing".into()))?;

    let all_entities = store.entity_ids(&request.project_id, &request.snapshot_id)?;
    let entity_total = all_entities.len();
    let mut entities = Vec::new();
    for entity_id in all_entities.iter().take(max_entities as usize) {
        if let Some(entity) = store.get_entity(&request.project_id, &request.snapshot_id, entity_id)? {
            entities.push(entity);
        }
    }
    let all_relations = store.relation_payloads(&request.project_id, &request.snapshot_id)?;
    let relation_total = all_relations.len();
    let relations: Vec<Value> = all_relations
        .iter()
        .take(max_relations as usize)
        .cloned()
        .collect();

    let claims = latest_payloads(
        store,
        "SELECT payload_json FROM claims WHERE project_id=?1 AND revision=(
             SELECT MAX(revision) FROM claims c2
             WHERE c2.project_id=claims.project_id AND c2.claim_id=claims.claim_id)
         ORDER BY claim_id",
        &request.project_id,
    )?;
    let architectures = latest_payloads(
        store,
        "SELECT payload_json FROM architectures WHERE project_id=?1 AND revision=(
             SELECT MAX(revision) FROM architectures a2
             WHERE a2.project_id=architectures.project_id AND a2.architecture_id=architectures.architecture_id)
         ORDER BY architecture_id",
        &request.project_id,
    )?;
    let plans = latest_payloads(
        store,
        "SELECT payload_json FROM plans WHERE project_id=?1 AND revision=(
             SELECT MAX(revision) FROM plans p2
             WHERE p2.project_id=plans.project_id AND p2.plan_id=plans.plan_id)
         ORDER BY plan_id",
        &request.project_id,
    )?;
    let changesets = latest_payloads(
        store,
        "SELECT payload_json FROM changesets WHERE project_id=?1 AND revision=(
             SELECT MAX(revision) FROM changesets c2
             WHERE c2.project_id=changesets.project_id AND c2.changeset_id=changesets.changeset_id)
         ORDER BY changeset_id",
        &request.project_id,
    )?;
    let contracts = latest_payloads(
        store,
        "SELECT payload_json FROM behavior_contracts WHERE project_id=?1 AND revision=(
             SELECT MAX(revision) FROM behavior_contracts b2
             WHERE b2.project_id=behavior_contracts.project_id AND b2.contract_id=behavior_contracts.contract_id)
         ORDER BY contract_id",
        &request.project_id,
    )?;
    let evidence_index = evidence_index(store, &request.project_id, &request.snapshot_id)?;
    let verifications = verification_summaries(store, &request.project_id)?;
    let counts = object_counts(store, &request.project_id)?;
    let source = if request.include_source {
        collect_source(store, &snapshot)?
    } else {
        Collection::default()
    };
    let traces = if request.include_raw_traces {
        collect_traces(store, &request.project_id)?
    } else {
        Collection::default()
    };
    let source_state = source.state(request.include_source);
    let trace_state = traces.state(request.include_raw_traces);

    let entity_omitted = entity_total.saturating_sub(entities.len());
    let relation_omitted = relation_total.saturating_sub(relations.len());
    let mut omissions = Vec::new();
    if entity_omitted > 0 {
        omissions.push(json!({
            "what": "entities",
            "omitted": entity_omitted,
            "reason": format!("bounded report limit {max_entities}; this is not complete coverage"),
        }));
    }
    if relation_omitted > 0 {
        omissions.push(json!({
            "what": "relations",
            "omitted": relation_omitted,
            "reason": format!("bounded report limit {max_relations}; this is not complete coverage"),
        }));
    }
    if source.omitted > 0 {
        omissions.push(json!({
            "what": "source_files",
            "omitted": source.omitted,
            "reason": format!("bundle source byte budget {MAX_SOURCE_BYTES}; source_state is partial"),
        }));
    }
    if traces.omitted > 0 {
        omissions.push(json!({
            "what": "raw_traces",
            "omitted": traces.omitted,
            "reason": format!("bundle trace byte budget {MAX_TRACE_BYTES}; raw_trace_state is partial"),
        }));
    }
    let mut unknowns = Vec::new();
    for claim in &claims {
        if let Some(questions) = claim["unresolved_questions"].as_array() {
            for question in questions {
                unknowns.push(json!({"source": "claim", "claim_id": claim["id"], "question": question}));
            }
        }
    }
    for architecture in &architectures {
        if let Some(components) = architecture["components"].as_array() {
            for component in components {
                if let Some(questions) = component["open_questions"].as_array() {
                    for question in questions {
                        unknowns.push(json!({
                            "source": "architecture",
                            "architecture_id": architecture["id"],
                            "component_id": component["id"],
                            "question": question,
                        }));
                    }
                }
            }
        }
    }
    let sensitive = source_state != "excluded" || trace_state != "excluded";
    let risk_labels: Vec<&str> = if sensitive {
        vec![
            "this bundle contains opted-in source or trace bytes and is sensitive",
            "source bytes may contain credentials, personal data or proprietary code",
            "raw traces may contain process names, paths, environment values or user identifiers",
            "no secret scanner ran; secrets_included is unknown",
            "do not upload, publish or share this bundle without a manual review",
        ]
    } else {
        vec![]
    };
    let mut limitations = vec![
        "static candidate relations are not resolved calls".to_string(),
        "bounded entity/relation lists are flagged as omissions, not silent completeness".to_string(),
        "verification states are copied from the frozen contract runs bound to their own snapshots".to_string(),
        "the HTML report executes no project code and loads no remote assets".to_string(),
    ];
    if sensitive {
        limitations.push(
            "source/trace bytes are included only because the caller opted in; they are not redacted".to_string(),
        );
    } else {
        limitations.push("source bytes and raw trace payloads are excluded from this bundle".to_string());
    }
    let report = json!({
        "schema_version": REPORT_VERSION,
        "report_kind": "archmeld.offline_review",
        "generated_at": crate::store::now_unix(),
        "producer": {"name": "archmeld-core", "version": env!("CARGO_PKG_VERSION")},
        "project": {
            "id": project["id"],
            "mode": project["mode"],
            "revision": project["revision"],
        },
        "snapshot": {
            "id": request.snapshot_id,
            "config_digest": snapshot["config_digest"],
            "environment_digest": snapshot["environment_digest"],
            "exclusions_digest": snapshot["exclusions_digest"],
            "manifest_artifact": manifest_artifact,
            "coverage": snapshot["coverage"],
            "files": snapshot["files"],
        },
        "counts": counts,
        "graph": {
            "entities_total": entity_total,
            "entities_included": entities.len(),
            "relations_total": relation_total,
            "relations_included": relations.len(),
            "entities": entities,
            "relations": relations,
        },
        "viewer": {
            "kind": "inline_svg_anchor_navigation",
            "max_nodes": VIEWER_MAX_NODES,
            "max_edges": VIEWER_MAX_EDGES,
            "scripts": 0,
            "external_urls": 0,
            "interaction": "anchor links to entity detail cards and CSS :target highlighting; no JavaScript",
        },
        "claims": claims,
        "architectures": architectures,
        "contracts": contracts,
        "plans": plans,
        "changesets": changesets,
        "verifications": verifications,
        "evidence": evidence_index,
        "unknowns": unknowns,
        "omissions": omissions,
        "limitations": limitations,
        "source_files": source.index,
        "trace_files": traces.index,
        "source_policy": {
            "source_included": source_state == "included",
            "source_state": source_state,
            "raw_traces_included": trace_state == "included",
            "raw_trace_state": trace_state,
            "source_file_count": source.files.len(),
            "source_bytes": source.bytes,
            "trace_file_count": traces.files.len(),
            "trace_bytes": traces.bytes,
            "source_max_bytes": MAX_SOURCE_BYTES,
            "raw_trace_max_bytes": MAX_TRACE_BYTES,
            "secrets_included": if sensitive { "unknown: no secret scanner ran" } else { "no source or trace bytes" },
            "reason": if sensitive {
                "explicit opt-in; the bundle now carries source/trace bytes and must be handled as sensitive"
            } else {
                "default exclusion; report contains metadata and analysis, not source bytes"
            },
        },
        "risk": {
            "level": if sensitive { "high" } else { "low" },
            "labels": risk_labels,
        },
    });
    let model = json!({
        "schema_version": REPORT_VERSION,
        "project_id": project["id"],
        "snapshot_id": request.snapshot_id,
        "config_digest": snapshot["config_digest"],
        "entities": report["graph"]["entities"],
        "relations": report["graph"]["relations"],
        "coverage": snapshot["coverage"],
        "omissions": omissions,
        "limitations": limitations,
    });
    let evidence_document = json!({
        "schema_version": REPORT_VERSION,
        "snapshot_id": request.snapshot_id,
        "entries": evidence_index,
        "note": "digests identify CAS objects; raw source and trace bytes are not included",
    });
    let html = render_html(&report)?;
    let output_dir = PathBuf::from(&request.output_dir);
    let mut extras = source.files;
    extras.extend(traces.files);
    write_bundle(
        &output_dir,
        &report,
        &model,
        &evidence_document,
        &html,
        &extras,
        &request,
        source_state,
        trace_state,
    )
}

fn collect_source(store: &Store, snapshot: &Value) -> Result<Collection> {
    let mut files: Vec<&Value> = snapshot["files"]
        .as_array()
        .map(|files| files.iter().collect())
        .unwrap_or_default();
    files.sort_by(|left, right| {
        left["path"]
            .as_str()
            .unwrap_or("")
            .cmp(right["path"].as_str().unwrap_or(""))
    });
    let mut collection = Collection::default();
    for (position, file) in files.iter().enumerate() {
        let path = file["path"]
            .as_str()
            .ok_or_else(|| CoreError::Storage("snapshot file has no path".to_string()))?;
        validate::validate_path("snapshot file", path)?;
        let digest = file["blob_digest"]
            .as_str()
            .ok_or_else(|| CoreError::Storage(format!("snapshot file {path} has no digest")))?;
        let size = file["size_bytes"].as_u64().unwrap_or(0);
        if collection.bytes.saturating_add(size) > MAX_SOURCE_BYTES {
            collection.omitted = files.len() - position;
            break;
        }
        let bytes = store
            .read_blob_bytes(digest)?
            .ok_or_else(|| CoreError::Storage(format!("source blob {digest} is missing")))?;
        if hash::digest_bytes(&bytes) != digest {
            return Err(CoreError::Storage(format!(
                "source blob {digest} does not match its stored digest"
            )));
        }
        collection.bytes += bytes.len() as u64;
        let bundle_path = format!("source/{path}");
        collection.index.push(json!({
            "path": bundle_path,
            "snapshot_path": path,
            "digest": digest,
            "size": bytes.len(),
            "media_type": source_media_type(path),
        }));
        collection.files.push(ExtraFile {
            path: bundle_path,
            bytes,
            media_type: source_media_type(path).to_string(),
            kind: "source",
            source_path: Some(path.to_string()),
            owners: None,
        });
    }
    Ok(collection)
}

fn collect_traces(store: &Store, project_id: &str) -> Result<Collection> {
    let mut statement = store.conn.prepare(
        "SELECT a.digest, a.size_bytes, a.media_type,
                (SELECT group_concat(r.owner_type || ':' || r.owner_id, ',')
                   FROM artifact_refs r
                  WHERE r.digest = a.digest AND r.owner_type = 'runtime_native'
                    AND r.owner_id IN (SELECT run_id FROM runtime_runs WHERE project_id = ?1))
                ||
                CASE WHEN EXISTS (
                    SELECT 1 FROM runtime_runs rr
                     WHERE rr.project_id = ?1 AND rr.artifact_digest = a.digest)
                     THEN ',' ||
                       (SELECT group_concat(rr2.run_id, ',') FROM runtime_runs rr2
                         WHERE rr2.project_id = ?1 AND rr2.artifact_digest = a.digest)
                     ELSE '' END AS owners
           FROM artifacts a
          WHERE EXISTS (SELECT 1 FROM runtime_runs rr3
                         WHERE rr3.project_id = ?1 AND rr3.artifact_digest = a.digest)
             OR EXISTS (SELECT 1 FROM artifact_refs r2
                         JOIN runtime_runs rr4 ON rr4.run_id = r2.owner_id
                        WHERE r2.digest = a.digest AND r2.owner_type = 'runtime_native'
                          AND rr4.project_id = ?1)
          ORDER BY a.digest",
    )?;
    let rows = statement.query_map((project_id,), |row| {
        Ok((
            row.get::<_, String>(0)?,
            row.get::<_, i64>(1)?,
            row.get::<_, String>(2)?,
            row.get::<_, Option<String>>(3)?,
        ))
    })?;
    let mut entries = Vec::new();
    for row in rows {
        entries.push(row?);
    }
    let mut collection = Collection::default();
    for (position, (digest, size, media_type, owners)) in entries.iter().enumerate() {
        if collection.bytes.saturating_add(*size as u64) > MAX_TRACE_BYTES {
            collection.omitted = entries.len() - position;
            break;
        }
        let bytes = store
            .read_blob_bytes(digest)?
            .ok_or_else(|| CoreError::Storage(format!("trace blob {digest} is missing")))?;
        if hash::digest_bytes(&bytes) != *digest {
            return Err(CoreError::Storage(format!(
                "trace blob {digest} does not match its stored digest"
            )));
        }
        collection.bytes += bytes.len() as u64;
        let hex = digest.strip_prefix("sha256:").unwrap_or(digest);
        let bundle_path = format!("traces/sha256/{hex}{}", trace_extension(media_type));
        let owner_text = owners.clone().unwrap_or_default();
        collection.index.push(json!({
            "path": bundle_path,
            "digest": digest,
            "size": bytes.len(),
            "media_type": media_type,
            "owners": owner_text,
        }));
        collection.files.push(ExtraFile {
            path: bundle_path,
            bytes,
            media_type: media_type.clone(),
            kind: "raw_trace",
            source_path: None,
            owners: Some(owner_text),
        });
    }
    Ok(collection)
}

fn source_media_type(path: &str) -> &'static str {
    let extension = path
        .rsplit_once('.')
        .map(|(_, extension)| extension.to_ascii_lowercase())
        .unwrap_or_default();
    match extension.as_str() {
        "py" => "text/x-python",
        "rs" => "text/x-rust",
        "ts" | "tsx" => "text/typescript",
        "js" | "jsx" => "text/javascript",
        "java" => "text/x-java",
        "cs" => "text/x-csharp",
        "go" => "text/x-go",
        "cpp" | "cc" | "cxx" | "h" | "hpp" => "text/x-c++",
        "c" => "text/x-c",
        "json" => "application/json",
        "toml" => "application/toml",
        "yaml" | "yml" => "application/yaml",
        "md" => "text/markdown",
        "html" | "htm" => "text/html",
        "css" => "text/css",
        "sql" => "application/sql",
        "txt" | "cfg" | "ini" | "sh" | "ps1" => "text/plain",
        _ => "application/octet-stream",
    }
}

fn trace_extension(media_type: &str) -> &'static str {
    match media_type {
        "application/vnd.tracy" => ".tracy",
        "application/json" => ".json",
        "application/x-perfetto" => ".pftrace",
        _ => ".bin",
    }
}

fn latest_payloads(store: &Store, sql: &str, project_id: &str) -> Result<Vec<Value>> {
    let mut statement = store.conn.prepare(sql)?;
    let rows = statement.query_map((project_id,), |row| row.get::<_, String>(0))?;
    let mut values = Vec::new();
    for row in rows {
        let payload: Value = serde_json::from_str(&row?)
            .map_err(|err| CoreError::Storage(format!("report payload corrupt: {err}")))?;
        values.push(payload);
    }
    Ok(values)
}

fn evidence_index(store: &Store, project_id: &str, snapshot_id: &str) -> Result<Vec<Value>> {
    let mut statement = store.conn.prepare(
        "SELECT payload_json,artifact_digest FROM evidence
         WHERE project_id=?1 AND snapshot_id=?2 ORDER BY evidence_id",
    )?;
    let rows = statement.query_map((project_id, snapshot_id), |row| {
        Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
    })?;
    let mut entries = Vec::new();
    for row in rows {
        let (payload, artifact_digest) = row?;
        let payload: Value = serde_json::from_str(&payload)
            .map_err(|err| CoreError::Storage(format!("evidence payload corrupt: {err}")))?;
        entries.push(json!({
            "id": payload["id"],
            "kind": payload["kind"],
            "producer": payload["producer"],
            "artifact_digest": artifact_digest,
            "artifact_available": store.blob_exists(&artifact_digest)?,
            "source_anchor": payload["source_anchor"],
            "coverage": payload["coverage"],
            "freshness": payload["freshness"],
            "raw_artifact_included": false,
        }));
    }
    Ok(entries)
}

fn verification_summaries(store: &Store, project_id: &str) -> Result<Vec<Value>> {
    let mut statement = store.conn.prepare(
        "SELECT verification_id,changeset_id,snapshot_id,state,verdict_json,contract_digest,
                toolchain_digest,started_at,finished_at
         FROM verification_runs WHERE project_id=?1 ORDER BY created_at, verification_id",
    )?;
    let rows = statement.query_map((project_id,), |row| {
        Ok((
            row.get::<_, String>(0)?,
            row.get::<_, String>(1)?,
            row.get::<_, String>(2)?,
            row.get::<_, String>(3)?,
            row.get::<_, String>(4)?,
            row.get::<_, String>(5)?,
            row.get::<_, String>(6)?,
            row.get::<_, i64>(7)?,
            row.get::<_, Option<i64>>(8)?,
        ))
    })?;
    let mut verifications = Vec::new();
    for row in rows {
        let (
            verification_id,
            changeset_id,
            snapshot_id,
            state,
            verdict_json,
            contract_digest,
            toolchain_digest,
            started_at,
            finished_at,
        ) = row?;
        let verdict: Value = serde_json::from_str(&verdict_json)
            .map_err(|err| CoreError::Storage(format!("verification verdict corrupt: {err}")))?;
        let checks = check_states(store, project_id, &verification_id)?;
        verifications.push(json!({
            "verification_id": verification_id,
            "changeset_id": changeset_id,
            "snapshot_id": snapshot_id,
            "state": state,
            "contract_digest": contract_digest,
            "toolchain_digest": toolchain_digest,
            "started_at": started_at,
            "finished_at": finished_at,
            "verdict": {
                "state": verdict["state"],
                "acceptance": verdict["acceptance"],
                "required_not_passed": verdict["required_not_passed"],
                "optional_not_passed": verdict["optional_not_passed"],
                "drift": verdict["drift"],
                "reasons": verdict["reasons"],
            },
            "checks": checks,
        }));
    }
    Ok(verifications)
}

fn check_states(store: &Store, project_id: &str, verification_id: &str) -> Result<Vec<Value>> {
    let mut statement = store.conn.prepare(
        "SELECT check_id,kind,required,state,timed_out FROM check_results
         WHERE project_id=?1 AND verification_id=?2 ORDER BY check_id",
    )?;
    let rows = statement.query_map((project_id, verification_id), |row| {
        Ok(json!({
            "check_id": row.get::<_, String>(0)?,
            "kind": row.get::<_, String>(1)?,
            "required": row.get::<_, i64>(2)? == 1,
            "state": row.get::<_, String>(3)?,
            "timed_out": row.get::<_, i64>(4)? == 1,
        }))
    })?;
    Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
}

fn object_counts(store: &Store, project_id: &str) -> Result<Value> {
    let count = |table: &str| -> Result<i64> {
        Ok(store.conn.query_row(
            &format!("SELECT COUNT(*) FROM {table} WHERE project_id=?1"),
            (project_id,),
            |row| row.get(0),
        )?)
    };
    Ok(json!({
        "snapshots": count("snapshots")?,
        "entities": count("entities")?,
        "relations": count("relations")?,
        "evidence": count("evidence")?,
        "claims": count("claims")?,
        "architectures": count("architectures")?,
        "contracts": count("behavior_contracts")?,
        "plans": count("plans")?,
        "changesets": count("changesets")?,
        "verifications": count("verification_runs")?,
    }))
}

fn escape(value: &str) -> String {
    value
        .replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&#39;")
}

fn text(value: &Value) -> String {
    match value {
        Value::Null => "—".to_string(),
        Value::String(text) => escape(text),
        other => escape(&other.to_string()),
    }
}

fn render_rows(items: &[Value], columns: &[(&str, &str)]) -> String {
    let mut html = String::new();
    for item in items {
        html.push_str("<tr>");
        for (key, _fallback) in columns {
            let value = if key.contains('.') {
                fallback_value(item, key).unwrap_or(&Value::Null)
            } else {
                item.get(*key).unwrap_or(&Value::Null)
            };
            html.push_str("<td>");
            html.push_str(&text(value));
            html.push_str("</td>");
        }
        html.push_str("</tr>");
    }
    if html.is_empty() {
        html.push_str("<tr><td colspan=\"");
        html.push_str(&columns.len().to_string());
        html.push_str("\">none</td></tr>");
    }
    html
}

fn fallback_value<'a>(item: &'a Value, path: &str) -> Option<&'a Value> {
    let mut current = item;
    for part in path.split('.') {
        current = current.get(part)?;
    }
    Some(current)
}

fn table(headers: &[&str], rows: &str) -> String {
    let mut html = String::from("<table><thead><tr>");
    for header in headers {
        html.push_str("<th>");
        html.push_str(&escape(header));
        html.push_str("</th>");
    }
    html.push_str("</tr></thead><tbody>");
    html.push_str(rows);
    html.push_str("</tbody></table>");
    html
}

fn entity_anchor(entity_id: &str) -> String {
    let digest = hash::digest_bytes(entity_id.as_bytes());
    format!("entity-{}", &digest.strip_prefix("sha256:").unwrap_or(&digest)[..12])
}

/// A deterministic inline SVG with anchor links and CSS `:target` only. No
/// scripts, no remote namespace URL, no layout library.
fn render_graph_view(report: &Value) -> String {
    use std::fmt::Write as _;
    let empty: Vec<Value> = Vec::new();
    let entities = report["graph"]["entities"]
        .as_array()
        .unwrap_or(&empty)
        .iter()
        .take(VIEWER_MAX_NODES)
        .collect::<Vec<_>>();
    let relations = report["graph"]["relations"]
        .as_array()
        .unwrap_or(&empty)
        .iter()
        .take(VIEWER_MAX_EDGES)
        .collect::<Vec<_>>();
    if entities.is_empty() {
        return "<p class=\"muted\">No entities were available for the graph view.</p>".to_string();
    }
    let anchors: std::collections::BTreeMap<&str, String> = entities
        .iter()
        .filter_map(|entity| entity["id"].as_str().map(|id| (id, entity_anchor(id))))
        .collect();
    let columns = (entities.len() as f64).sqrt().ceil() as usize;
    let cell = 150usize;
    let width = columns.max(1) * cell;
    let rows = entities.len().div_ceil(columns.max(1));
    let height = rows.max(1) * cell;
    let mut svg = String::new();
    let _ = write!(
        svg,
        "<div class=\"graph\"><svg role=\"img\" aria-label=\"entity relation graph\" width=\"{width}\" height=\"{height}\" viewBox=\"0 0 {width} {height}\">"
    );
    for relation in &relations {
        let (Some(source), Some(target)) = (
            relation["source_id"].as_str().and_then(|id| anchors.get(id)),
            relation["target_id"].as_str().and_then(|id| anchors.get(id)),
        ) else {
            continue;
        };
        let position = |anchor: &str| {
            entities
                .iter()
                .position(|entity| entity["id"].as_str().map(entity_anchor).as_deref() == Some(anchor))
                .unwrap_or(0)
        };
        let (sx, sy) = node_center(position(source), columns, cell);
        let (tx, ty) = node_center(position(target), columns, cell);
        let kind = escape(relation["kind"].as_str().unwrap_or("relation"));
        let resolution = escape(
            relation["resolution"]
                .as_str()
                .or_else(|| relation["status"].as_str())
                .unwrap_or("candidate"),
        );
        let _ = write!(
            svg,
            "<line x1=\"{sx}\" y1=\"{sy}\" x2=\"{tx}\" y2=\"{ty}\" stroke=\"#94a3b8\" stroke-width=\"1\" opacity=\"0.45\"><title>{kind} ({resolution})</title></line>"
        );
    }
    for (index, entity) in entities.iter().enumerate() {
        let Some(id) = entity["id"].as_str() else {
            continue;
        };
        let anchor = entity_anchor(id);
        let (x, y) = node_center(index, columns, cell);
        let label = entity["qualified_name"]
            .as_str()
            .or_else(|| entity["name"].as_str())
            .unwrap_or(id);
        let label: String = label.chars().take(18).collect();
        let _ = write!(
            svg,
            "<a href=\"#{anchor}\"><circle cx=\"{x}\" cy=\"{y}\" r=\"7\"><title>{}</title></circle><text x=\"{}\" y=\"{}\">{}</text></a>",
            escape(id),
            x.saturating_sub(52),
            y + 22,
            escape(&label)
        );
    }
    let _ = write!(svg, "</svg></div>");
    if report["graph"]["entities_total"].as_u64().unwrap_or(0) > entities.len() as u64
        || report["graph"]["relations_total"].as_u64().unwrap_or(0) > relations.len() as u64
    {
        let _ = write!(
            svg,
            "<p class=\"muted\">Viewer shows {}/{} entities and {}/{} relations; the remaining graph is in model.json.</p>",
            entities.len(),
            report["graph"]["entities_total"],
            relations.len(),
            report["graph"]["relations_total"]
        );
    }
    svg.push_str("<p class=\"muted\">Click a node to jump to its detail card. Native anchor navigation and CSS :target only; no scripts or remote assets.</p>");
    svg.push_str("<details><summary>Entity details</summary>");
    for entity in &entities {
        let Some(id) = entity["id"].as_str() else {
            continue;
        };
        let anchor = entity_anchor(id);
        let qualified = entity["qualified_name"].as_str().unwrap_or(id);
        let _ = write!(
            svg,
            "<div class=\"entity\" id=\"{anchor}\"><strong>{}</strong><br><span class=\"muted\">{} · {}</span><br><code>{}</code>",
            escape(qualified),
            text(&entity["kind"]),
            text(&entity["language"]),
            escape(id)
        );
        if let Some(anchor_value) = entity["anchors"].as_array().and_then(|anchors| anchors.first()) {
            let _ = write!(
                svg,
                "<br><code>{}:{}</code>",
                text(&anchor_value["path"]),
                text(&anchor_value["line_start"])
            );
        }
        svg.push_str("</div>");
    }
    svg.push_str("</details>");
    svg
}

fn node_center(index: usize, columns: usize, cell: usize) -> (usize, usize) {
    let column = index % columns;
    let row = index / columns;
    (column * cell + cell / 2, row * cell + cell / 2)
}

fn render_html(report: &Value) -> Result<String> {
    let mut components = Vec::new();
    for architecture in report["architectures"].as_array().into_iter().flatten() {
        for component in architecture["components"].as_array().into_iter().flatten() {
            components.push(json!({
                "architecture_id": architecture["id"],
                "id": component["id"],
                "name": component["name"],
                "purpose": component["purpose"],
                "member_count": component["member_entity_ids"].as_array().map(Vec::len).unwrap_or(0),
            }));
        }
    }
    let dependencies: Vec<Value> = report["architectures"]
        .as_array()
        .into_iter()
        .flatten()
        .flat_map(|architecture| {
            architecture["dependencies"]
                .as_array()
                .cloned()
                .unwrap_or_default()
                .into_iter()
                .map(move |dependency| {
                    json!({
                        "architecture_id": architecture["id"],
                        "source_id": dependency["source_id"],
                        "target_id": dependency["target_id"],
                        "status": dependency["status"],
                        "reason": dependency["reason"],
                    })
                })
        })
        .collect();
    let claims_rows = render_rows(
        report["claims"].as_array().map(Vec::as_slice).unwrap_or(&[]),
        &[("id", "id"), ("claim_type", "claim_type"), ("status", "status"), ("text", "text")],
    );
    let check_rows: Vec<Value> = report["verifications"]
        .as_array()
        .into_iter()
        .flatten()
        .flat_map(|verification| {
            verification["checks"]
                .as_array()
                .cloned()
                .unwrap_or_default()
                .into_iter()
                .map(move |check| {
                    json!({
                        "verification_id": verification["verification_id"],
                        "check_id": check["check_id"],
                        "kind": check["kind"],
                        "required": check["required"],
                        "state": check["state"],
                    })
                })
        })
        .collect();
    let evidence_rows = render_rows(
        report["evidence"].as_array().map(Vec::as_slice).unwrap_or(&[]),
        &[
            ("id", "id"),
            ("kind", "kind"),
            ("artifact_digest", "artifact_digest"),
            ("artifact_available", "artifact_available"),
            ("freshness", "freshness"),
        ],
    );
    let unknown_rows = render_rows(
        report["unknowns"].as_array().map(Vec::as_slice).unwrap_or(&[]),
        &[("source", "source"), ("question", "question")],
    );
    let omission_rows = render_rows(
        report["omissions"].as_array().map(Vec::as_slice).unwrap_or(&[]),
        &[("what", "what"), ("omitted", "omitted"), ("reason", "reason")],
    );
    let limitations = report["limitations"]
        .as_array()
        .into_iter()
        .flatten()
        .map(|item| format!("<li>{}</li>", text(item)))
        .collect::<String>();
    let title = format!(
        "ARCHMELD offline review · {} · {}",
        text(&report["project"]["id"]),
        text(&report["snapshot"]["id"])
    );
    let mut html = String::new();
    html.push_str("<!doctype html><html lang=\"en\"><head><meta charset=\"utf-8\">");
    html.push_str("<meta name=\"viewport\" content=\"width=device-width, initial-scale=1\">");
    html.push_str("<title>");
    html.push_str(&title);
    html.push_str("</title><style>");
    html.push_str(":root{color-scheme:light}body{font:14px/1.5 system-ui,sans-serif;margin:0;color:#17202a;background:#f7f9fb}main{max-width:1100px;margin:0 auto;padding:28px}header{border-bottom:3px solid #0f766e;padding-bottom:16px;margin-bottom:24px}h1{font-size:24px;margin:0 0 8px}h2{font-size:18px;margin-top:28px;border-bottom:1px solid #cbd5e1;padding-bottom:6px}code{background:#e2e8f0;padding:1px 4px;border-radius:3px;word-break:break-all}table{width:100%;border-collapse:collapse;background:#fff;margin:8px 0 16px}th,td{border:1px solid #cbd5e1;padding:6px 8px;text-align:left;vertical-align:top}th{background:#e2e8f0}.grid{display:grid;grid-template-columns:repeat(auto-fit,minmax(220px,1fr));gap:12px}.card{background:#fff;border:1px solid #cbd5e1;border-radius:6px;padding:12px}.muted{color:#475569}.warn{background:#fff7ed;border-left:4px solid #c2410c;padding:10px 12px}footer{margin-top:36px;color:#475569;font-size:12px}.graph{overflow:auto;max-height:70vh;border:1px solid #cbd5e1;background:#fff}.graph svg{display:block}.graph circle{fill:#0f766e;stroke:#0b4f4a;stroke-width:1}.graph a:hover circle{fill:#c2410c}.graph text{font:10px system-ui,sans-serif;fill:#17202a}.entity{border:1px solid #cbd5e1;background:#fff;padding:8px 10px;margin:6px 0}.entity:target{background:#fef3c7;outline:2px solid #b45309}</style></head><body><main>");
    let sensitive = report["risk"]["level"].as_str() == Some("high");
    html.push_str("<header><h1>ARCHMELD offline review</h1><p class=\"muted\">");
    if sensitive {
        html.push_str("One explicit snapshot with opted-in source or trace bytes. No scripts, no remote assets. Treat this bundle as sensitive.");
    } else {
        html.push_str("One explicit snapshot, generated for offline inspection. No scripts, no remote assets, no source bytes.");
    }
    html.push_str("</p><div class=\"grid\">");
    for (label, value) in [
        ("Project", &report["project"]["id"]),
        ("Mode", &report["project"]["mode"]),
        ("Snapshot", &report["snapshot"]["id"]),
        ("Config digest", &report["snapshot"]["config_digest"]),
        ("Generated", &report["generated_at"]),
    ] {
        html.push_str(&format!(
            "<div class=\"card\"><strong>{}</strong><br><code>{}</code></div>",
            escape(label),
            text(value)
        ));
    }
    html.push_str("</div></header>");
    if sensitive {
        let mut labels = String::new();
        for label in report["risk"]["labels"].as_array().into_iter().flatten() {
            labels.push_str(&format!("<li>{}</li>", text(label)));
        }
        html.push_str(&format!(
            "<div class=\"warn\"><strong>Sensitive bundle.</strong> Source or raw trace bytes were explicitly included.<ul>{labels}</ul></div>"
        ));
    }
    html.push_str("<section><h2>Coverage and limitations</h2>");
    html.push_str(&format!("<p>Snapshot coverage: <code>{}</code></p>", text(&report["snapshot"]["coverage"])));
    html.push_str(&format!("<div class=\"warn\"><ul>{}{}</ul></div>", limitations, if report["limitations"].as_array().map(Vec::is_empty).unwrap_or(true) { "<li>none</li>" } else { "" }));
    html.push_str("<h3>Omissions</h3>");
    html.push_str(&table(&["What", "Omitted", "Reason"], &omission_rows));
    html.push_str("</section>");
    html.push_str("<section><h2>Graph view</h2>");
    html.push_str(&render_graph_view(report));
    html.push_str("<section><h2>Architecture</h2>");
    html.push_str(&table(
        &["Architecture", "Component", "Name", "Purpose", "Members"],
        &render_rows(
            &components,
            &[("architecture_id", "architecture_id"), ("id", "id"), ("name", "name"), ("purpose", "purpose"), ("member_count", "member_count")],
        ),
    ));
    html.push_str("<h3>Dependencies (candidate unless otherwise marked)</h3>");
    html.push_str(&table(
        &["Architecture", "Source", "Target", "Status", "Reason"],
        &render_rows(
            &dependencies,
            &[("architecture_id", "architecture_id"), ("source_id", "source_id"), ("target_id", "target_id"), ("status", "status"), ("reason", "reason")],
        ),
    ));
    html.push_str("</section>");
    html.push_str("<section><h2>Claims</h2>");
    html.push_str(&table(&["ID", "Type", "Status", "Text"], &claims_rows));
    html.push_str("<h3>Unknowns</h3>");
    html.push_str(&table(&["Source", "Question"], &unknown_rows));
    html.push_str("</section>");
    html.push_str("<section><h2>Verification</h2>");
    html.push_str(&table(
        &["Verification", "Check", "Kind", "Required", "State"],
        &render_rows(
            &check_rows,
            &[
                ("verification_id", ""),
                ("check_id", ""),
                ("kind", ""),
                ("required", ""),
                ("state", ""),
            ],
        ),
    ));
    html.push_str("</section>");
    html.push_str("<section><h2>Evidence index</h2>");
    html.push_str(&table(
        &["ID", "Kind", "Artifact digest", "Available", "Freshness"],
        &evidence_rows,
    ));
    html.push_str("<p class=\"muted\">Artifact bytes are not embedded. A reader can compare the digest with an independently held copy.</p>");
    html.push_str("</section>");
    if !report["source_files"].as_array().map(Vec::is_empty).unwrap_or(true) {
        html.push_str("<section><h2>Source files (opt-in)</h2>");
        html.push_str("<p class=\"warn\">These bytes are part of the bundle and may contain secrets or personal data. They are listed by path and digest, not displayed here.</p>");
        html.push_str(&table(
            &["Bundle path", "Snapshot path", "Digest", "Size"],
            &render_rows(
                report["source_files"].as_array().map(Vec::as_slice).unwrap_or(&[]),
                &[("path", "path"), ("snapshot_path", "snapshot_path"), ("digest", "digest"), ("size", "size")],
            ),
        ));
        html.push_str("</section>");
    }
    if !report["trace_files"].as_array().map(Vec::is_empty).unwrap_or(true) {
        html.push_str("<section><h2>Raw traces (opt-in)</h2>");
        html.push_str("<p class=\"warn\">Raw trace payloads can contain process names, paths and user identifiers.</p>");
        html.push_str(&table(
            &["Bundle path", "Digest", "Size", "Media type", "Owners"],
            &render_rows(
                report["trace_files"].as_array().map(Vec::as_slice).unwrap_or(&[]),
                &[("path", "path"), ("digest", "digest"), ("size", "size"), ("media_type", "media_type"), ("owners", "owners")],
            ),
        ));
        html.push_str("</section>");
    }
    html.push_str("<footer>Generated by ARCHMELD. Static report: opening it does not execute repository code or contact a network service. Verify manifest.json before trusting copied files.</footer>");
    html.push_str("</main></body></html>");
    Ok(html)
}

fn write_bundle(
    output_dir: &Path,
    report: &Value,
    model: &Value,
    evidence: &Value,
    html: &str,
    extras: &[ExtraFile],
    request: &ReportRequest,
    source_state: &str,
    trace_state: &str,
) -> Result<Value> {
    if output_dir.exists() {
        return Err(CoreError::conflict(format!(
            "report destination {} already exists",
            output_dir.display()
        )));
    }
    let parent = output_dir
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
        .unwrap_or_else(|| Path::new("."));
    fs::create_dir_all(parent)?;
    let tmp = parent.join(format!(
        ".archmeld-report-{}",
        &hash::digest_bytes(format!("{}:{}", request.project_id, request.snapshot_id).as_bytes())
            [7..23]
    ));
    if tmp.exists() {
        fs::remove_dir_all(&tmp)?;
    }
    fs::create_dir_all(tmp.join("evidence"))?;
    let mut files: Vec<(String, Vec<u8>, String, &'static str, Option<String>, Option<String>)> = vec![
        (
            "report.html".to_string(),
            html.as_bytes().to_vec(),
            "text/html".to_string(),
            "report",
            None,
            None,
        ),
        (
            "report.json".to_string(),
            serde_json::to_vec_pretty(report)
                .map_err(|err| CoreError::Storage(format!("report encode: {err}")))?,
            "application/json".to_string(),
            "report",
            None,
            None,
        ),
        (
            "model.json".to_string(),
            serde_json::to_vec_pretty(model)
                .map_err(|err| CoreError::Storage(format!("model encode: {err}")))?,
            "application/json".to_string(),
            "report",
            None,
            None,
        ),
        (
            "evidence/index.json".to_string(),
            serde_json::to_vec_pretty(evidence)
                .map_err(|err| CoreError::Storage(format!("evidence encode: {err}")))?,
            "application/json".to_string(),
            "report",
            None,
            None,
        ),
    ];
    for extra in extras {
        files.push((
            extra.path.clone(),
            extra.bytes.clone(),
            extra.media_type.clone(),
            extra.kind,
            extra.source_path.clone(),
            extra.owners.clone(),
        ));
    }
    let mut manifest_files = Vec::new();
    for (path, bytes, media_type, kind, source_path, owners) in &files {
        let target = tmp.join(path);
        if let Some(parent) = target.parent() {
            fs::create_dir_all(parent)?;
        }
        fs::write(&target, bytes)?;
        let digest = hash::digest_bytes(bytes);
        let actual = fs::read(&target)?;
        if hash::digest_bytes(&actual) != digest {
            return Err(CoreError::Storage(format!(
                "report file {path} changed while writing"
            )));
        }
        let mut entry = json!({
            "path": path,
            "digest": digest,
            "size": bytes.len(),
            "media_type": media_type,
            "kind": kind,
        });
        if let Some(source_path) = source_path {
            entry["source_path"] = json!(source_path);
        }
        if let Some(owners) = owners {
            entry["owners"] = json!(owners);
        }
        manifest_files.push(entry);
    }
    manifest_files.sort_by(|left, right| left["path"].as_str().cmp(&right["path"].as_str()));
    let sensitive = source_state != "excluded" || trace_state != "excluded";
    let source_count = extras.iter().filter(|file| file.kind == "source").count();
    let source_bytes: usize = extras
        .iter()
        .filter(|file| file.kind == "source")
        .map(|file| file.bytes.len())
        .sum();
    let trace_count = extras.iter().filter(|file| file.kind == "raw_trace").count();
    let trace_bytes: usize = extras
        .iter()
        .filter(|file| file.kind == "raw_trace")
        .map(|file| file.bytes.len())
        .sum();
    let mut manifest = json!({
        "schema_version": REPORT_VERSION,
        "report_kind": "archmeld.offline_review",
        "project_id": request.project_id,
        "snapshot_id": request.snapshot_id,
        "generated_at": crate::store::now_unix(),
        "files": manifest_files,
        "attachments": manifest_files,
        "source_included": source_state == "included",
        "source_state": source_state,
        "raw_traces_included": trace_state == "included",
        "raw_trace_state": trace_state,
        "source_file_count": source_count,
        "source_bytes": source_bytes,
        "trace_file_count": trace_count,
        "trace_bytes": trace_bytes,
        "secrets_included": if sensitive {
            "unknown: no secret scanner ran"
        } else {
            "no source or trace bytes"
        },
        "risk_level": if sensitive { "high" } else { "low" },
        "risk_labels": report["risk"]["labels"],
        "offline": true,
        "external_urls": 0,
        "scripts": 0,
        "verification": "each file digest is checked again before publication",
    });
    let bundle_digest = hash::digest_value(&manifest)
        .map_err(|err| CoreError::invalid(format!("report manifest: {err}")))?;
    manifest["bundle_digest"] = json!(bundle_digest);
    let manifest_bytes = serde_json::to_vec_pretty(&manifest)
        .map_err(|err| CoreError::Storage(format!("manifest encode: {err}")))?;
    fs::write(tmp.join("manifest.json"), &manifest_bytes)?;
    verify_bundle(&tmp, &manifest)?;
    fs::rename(&tmp, output_dir)?;
    let report_id = format!(
        "report.{}",
        &hash::digest_bytes(format!("{}:{}:{}", request.project_id, request.snapshot_id, bundle_digest).as_bytes())
            [7..23]
    );
    Ok(json!({
        "report_id": report_id,
        "output_dir": output_dir.to_string_lossy(),
        "snapshot_id": request.snapshot_id,
        "bundle_digest": bundle_digest,
        "manifest": manifest,
        "source_included": source_state == "included",
        "source_state": source_state,
        "raw_traces_included": trace_state == "included",
        "raw_trace_state": trace_state,
        "risk_level": if sensitive { "high" } else { "low" },
        "offline": true,
    }))
}

pub fn verify_bundle(directory: &Path, manifest: &Value) -> Result<Value> {
    let files = manifest["files"]
        .as_array()
        .ok_or_else(|| CoreError::Storage("report manifest has no files".into()))?;
    let mut checked = 0usize;
    for file in files {
        let path = file["path"]
            .as_str()
            .ok_or_else(|| CoreError::Storage("manifest file has no path".into()))?;
        if path.contains("..") || path.starts_with('/') || path.contains('\\') {
            return Err(CoreError::invalid(format!(
                "manifest path {path:?} is not bundle-relative"
            )));
        }
        let bytes = fs::read(directory.join(path))
            .map_err(|err| CoreError::Storage(format!("bundle file {path} missing: {err}")))?;
        if hash::digest_bytes(&bytes) != file["digest"].as_str().unwrap_or("") {
            return Err(CoreError::Storage(format!(
                "bundle file {path} does not match its manifest digest"
            )));
        }
        checked += 1;
    }
    let mut without_digest = manifest.clone();
    without_digest
        .as_object_mut()
        .ok_or_else(|| CoreError::Storage("manifest is not an object".into()))?
        .remove("bundle_digest");
    let actual = hash::digest_value(&without_digest)
        .map_err(|err| CoreError::invalid(format!("manifest digest: {err}")))?;
    if actual != manifest["bundle_digest"].as_str().unwrap_or("") {
        return Err(CoreError::Storage(
            "report bundle_digest does not match the manifest".into(),
        ));
    }
    Ok(json!({"checked_files": checked, "bundle_digest": actual}))
}
