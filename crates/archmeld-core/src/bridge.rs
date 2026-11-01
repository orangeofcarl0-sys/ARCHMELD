// Copyright 2026 ARCHMELD contributors
// SPDX-License-Identifier: Apache-2.0

//! CodeGraph structural export → `EvidenceBatch` bridge (S04).
//!
//! The consumer is deliberately independent of the Rust graph types: it reads
//! the versioned NDJSON produced by `--archmeld-export-v1`. Backend integer
//! IDs appear only in `backend_refs`; public IDs are content-derived. Every
//! relation keeps the upstream edge type as a condition and stays a syntax
//! candidate unless a semantic tool resolves it later.

use crate::error::{CoreError, Result};
use crate::hash;
use crate::identity;
use crate::store::{Coverage, FileEntry, Store};
use serde_json::{json, Map, Value};
use std::collections::{BTreeSet, HashMap};
use std::path::Path;

pub const EXPORT_FORMAT: &str = "archmeld-export-v1";
pub const ADAPTER_ID: &str = "codegraph";

pub struct BridgeConfig<'a> {
    pub project_id: &'a str,
    pub snapshot_id: &'a str,
    pub config_digest: &'a str,
    pub run_id: &'a str,
    /// Absolute workspace root used to strip upstream absolute paths.
    pub workspace_root: Option<&'a Path>,
    pub max_entities_per_batch: usize,
    pub max_relations_per_batch: usize,
}

#[derive(Debug, Clone)]
pub struct BridgeReport {
    pub batches: u64,
    pub entities: u64,
    pub relations: u64,
    pub diagnostics: u64,
    pub skipped_nodes: u64,
    pub merged_nodes: u64,
    pub unresolved: u64,
    pub coverage: Value,
}

struct NodeRec {
    backend_id: u64,
    node_type: String,
    path: Option<String>,
    name: Option<String>,
    line_start: Option<u64>,
    line_end: Option<u64>,
    raw_properties: Map<String, Value>,
}

struct EdgeRec {
    backend_id: u64,
    source: u64,
    target: u64,
    edge_type: String,
    line: Option<u64>,
}

/// Parse and import a structural export. Returns counts and the published
/// coverage; the caller decides whether partial coverage is acceptable for the
/// current task.
pub fn import_codegraph_export(
    store: &mut Store,
    config: &BridgeConfig<'_>,
    files: &[FileEntry],
    export_text: &str,
) -> Result<BridgeReport> {
    let parsed = parse_export(export_text)?;
    let export_digest = store.put_blob(export_text.as_bytes(), "application/x-ndjson")?;
    let evidence_id = format!("evidence.codegraph.export.{}", config.run_id);

    let by_path: HashMap<&str, &FileEntry> =
        files.iter().map(|file| (file.path.as_str(), file)).collect();
    let by_basename: HashMap<&str, Vec<&FileEntry>> = {
        let mut map: HashMap<&str, Vec<&FileEntry>> = HashMap::new();
        for file in files {
            let base = file
                .path
                .rsplit_once('/')
                .map(|(_, base)| base)
                .unwrap_or(file.path.as_str());
            map.entry(base).or_default().push(file);
        }
        map
    };

    let mut diagnostics: Vec<Value> = Vec::new();
    let mut entities: Vec<Value> = Vec::new();
    let mut entity_index: HashMap<String, usize> = HashMap::new();
    let mut entity_id_by_backend: HashMap<u64, String> = HashMap::new();
    let mut skipped_nodes = 0u64;

    for node in &parsed.nodes {
        let Some((kind, is_file)) = map_node_kind(&node.node_type) else {
            skipped_nodes += 1;
            diagnostics.push(diagnostic(
                "unsupported_node_type",
                "warning",
                &format!(
                    "node {} has unsupported upstream type {}",
                    node.backend_id, node.node_type
                ),
                None,
            ));
            continue;
        };
        let resolved = resolve_path(node, config.workspace_root, &by_path, &by_basename);
        let Some((path, file)) = resolved else {
            // Upstream module nodes are import/use declarations: they have no
            // file path but are real endpoints of Imports/References edges.
            // Keep them as external entities instead of dropping the edges.
            if node.node_type == "Module" {
                let name = node
                    .name
                    .clone()
                    .filter(|name| !name.is_empty())
                    .unwrap_or_else(|| format!("module@{}", node.backend_id));
                let external_kind = if property_bool(&node.raw_properties, "is_external") {
                    "external"
                } else {
                    "module"
                };
                let entity_id = identity::external_entity_id(
                    config.snapshot_id,
                    ADAPTER_ID,
                    &name,
                    external_kind,
                )?;
                let entity = json!({
                    "id": entity_id,
                    "project_id": config.project_id,
                    "snapshot_id": config.snapshot_id,
                    "config_digest": config.config_digest,
                    "kind": external_kind,
                    "language": "unknown",
                    "qualified_name": name,
                    "anchors": [],
                    "external": true,
                    "backend_refs": [{
                        "adapter_id": ADAPTER_ID,
                        "run_id": config.run_id,
                        "backend_id": node.backend_id.to_string(),
                    }],
                });
                insert_or_merge_entity(&mut entities, &mut entity_index, entity_id.clone(), entity);
                entity_id_by_backend.insert(node.backend_id, entity_id);
                continue;
            }
            skipped_nodes += 1;
            diagnostics.push(diagnostic(
                "unmapped_source_path",
                "warning",
                &format!(
                    "node {} ({}) has no repository-relative path in the snapshot",
                    node.backend_id, node.node_type
                ),
                None,
            ));
            continue;
        };
        let name = node
            .name
            .clone()
            .filter(|name| !name.is_empty())
            .unwrap_or_else(|| {
                path.rsplit_once('/')
                    .map(|(_, base)| base.to_string())
                    .unwrap_or_else(|| path.clone())
            });
        let (line_start, line_end) = if is_file {
            (None, None)
        } else {
            match (node.line_start, node.line_end) {
                (Some(start), Some(end)) if start <= end => (Some(start), Some(end)),
                (Some(start), None) => (Some(start), Some(start)),
                _ => (None, None),
            }
        };
        let discriminator = format!("{path}::{}::{name}", node.node_type);
        let entity_id = identity::line_entity_id(
            &file.blob_digest,
            kind,
            line_start.unwrap_or(0),
            line_end.unwrap_or(0),
            &discriminator,
        )?;
        let qualified_name = if is_file {
            path.clone()
        } else {
            format!("{path}::{name}")
        };
        let anchor = json!({
            "path": path,
            "blob_digest": file.blob_digest,
            "precision": "line_only",
            "byte_start": null,
            "byte_end": null,
            "line_start": line_start,
            "line_end": line_end,
            "encoding": "utf-8",
        });
        let entity = json!({
            "id": entity_id,
            "project_id": config.project_id,
            "snapshot_id": config.snapshot_id,
            "config_digest": config.config_digest,
            "kind": kind,
            "language": language_for(&path),
            "qualified_name": qualified_name,
            "anchors": [anchor],
            "external": false,
            "backend_refs": [{
                "adapter_id": ADAPTER_ID,
                "run_id": config.run_id,
                "backend_id": node.backend_id.to_string(),
            }],
        });
        insert_or_merge_entity(&mut entities, &mut entity_index, entity_id.clone(), entity);
        entity_id_by_backend.insert(node.backend_id, entity_id);
    }

    let evidence = json!({
        "id": evidence_id,
        "project_id": config.project_id,
        "snapshot_id": config.snapshot_id,
        "config_digest": config.config_digest,
        "kind": "syntax",
        "producer": {
            "name": "codegraph",
            "version": parsed.version,
            "source_ref": EXPORT_FORMAT,
            "run_id": config.run_id,
        },
        "artifact_digest": export_digest,
        "locator": EXPORT_FORMAT,
        "source_anchor": null,
        "scenario_id": null,
        "runtime_run_id": null,
        "coverage": {
            "state": "partial",
            "scope": "codegraph structural export",
            "expected_count": parsed.manifest_node_count,
            "observed_count": parsed.nodes.len() as u64,
            "omitted_count": skipped_nodes,
            "unresolved_count": 0,
            "limitations": ["structural export; call edges are syntax candidates"]
        },
        "freshness": "current",
    });

    let mut relations: Vec<Value> = Vec::new();
    let mut unresolved = 0u64;
    for edge in &parsed.edges {
        let Some(kind) = map_edge_kind(&edge.edge_type) else {
            unresolved += 1;
            diagnostics.push(diagnostic(
                "unmapped_edge_type",
                "warning",
                &format!(
                    "edge {} has upstream type {} which has no v1 mapping; it was not imported",
                    edge.backend_id, edge.edge_type
                ),
                None,
            ));
            continue;
        };
        let (Some(source_id), Some(target_id)) = (
            entity_id_by_backend.get(&edge.source),
            entity_id_by_backend.get(&edge.target),
        ) else {
            unresolved += 1;
            diagnostics.push(diagnostic(
                "unresolved_endpoint",
                "warning",
                &format!(
                    "edge {} references a node that was not imported",
                    edge.backend_id
                ),
                None,
            ));
            continue;
        };
        let resolution = if matches!(
            kind,
            "static_call_candidate" | "references"
        ) {
            "candidate"
        } else {
            "declared"
        };
        let identity_value = json!({
            "version": "1.0",
            "source": source_id,
            "target": target_id,
            "kind": kind,
            "line": edge.line,
        });
        let digest = hash::digest_value(&identity_value)
            .map_err(|err| CoreError::invalid(format!("relation identity: {err}")))?;
        let relation_id = format!(
            "rel.{}",
            digest.strip_prefix("sha256:").expect("sha256 prefix")
        );
        relations.push(json!({
            "id": relation_id,
            "project_id": config.project_id,
            "snapshot_id": config.snapshot_id,
            "config_digest": config.config_digest,
            "source_id": source_id,
            "target_id": target_id,
            "kind": kind,
            "basis": "syntax",
            "evidence_ids": [evidence_id],
            "conditions": [
                format!("codegraph-edge:{}", edge.edge_type),
                "syntax-only; not a resolved call".to_string()
            ],
            "resolution": resolution,
        }));
    }

    let merged_nodes = parsed.nodes.len() as u64 - entities.len() as u64;
    let completeness = skipped_nodes == 0
        && unresolved == 0
        && parsed.nodes.len() as u64 == parsed.manifest_node_count;
    let mut limitations = vec![
        "line-only anchors; upstream exposes no byte ranges".to_string(),
        "call/reference edges are syntax candidates, not resolved semantics".to_string(),
    ];
    if skipped_nodes > 0 {
        limitations.push(format!("{skipped_nodes} node(s) had no mapped source path"));
    }
    if unresolved > 0 {
        limitations.push(format!("{unresolved} edge(s) were unresolved or unmapped"));
    }
    if merged_nodes > 0 {
        limitations.push(format!(
            "{merged_nodes} upstream node(s) merged by content identity"
        ));
    }
    let coverage = Coverage {
        state: if completeness {
            crate::store::CoverageState::Complete
        } else {
            crate::store::CoverageState::Partial
        },
        scope: "codegraph structural export over the declared snapshot".to_string(),
        expected_count: Some(parsed.manifest_node_count),
        observed_count: parsed.nodes.len() as u64,
        omitted_count: skipped_nodes,
        unresolved_count: unresolved,
        limitations,
    };

    let adapter_version = parsed.version.clone();
    let max_entities = config.max_entities_per_batch.max(1);
    let max_relations = config.max_relations_per_batch.max(1);
    let entity_chunks: Vec<&[Value]> = if entities.is_empty() {
        vec![&[]]
    } else {
        entities.chunks(max_entities).collect()
    };
    let relation_chunks: Vec<&[Value]> = if relations.is_empty() {
        vec![&[]]
    } else {
        relations.chunks(max_relations).collect()
    };
    let batches = entity_chunks.len().max(relation_chunks.len());
    for index in 0..batches {
        let entity_slice = entity_chunks.get(index).copied().unwrap_or(&[]);
        let relation_slice = relation_chunks.get(index).copied().unwrap_or(&[]);
        let is_final = index + 1 == batches;
        let mut evidence_slice: &[Value] = &[];
        if index == 0 {
            evidence_slice = std::slice::from_ref(&evidence);
        }
        let diagnostics_slice: &[Value] = if is_final { &diagnostics } else { &[] };
        let batch = json!({
            "schema_version": "1.0",
            "project_id": config.project_id,
            "snapshot_id": config.snapshot_id,
            "config_digest": config.config_digest,
            "run_id": config.run_id,
            "adapter_id": ADAPTER_ID,
            "adapter_version": adapter_version,
            "sequence": index.to_string(),
            "final": is_final,
            "coverage": coverage.to_json(),
            "entities": entity_slice,
            "evidence": evidence_slice,
            "relations": relation_slice,
            "diagnostics": diagnostics_slice,
        });
        store.import_batch(&batch)?;
    }

    Ok(BridgeReport {
        batches: batches as u64,
        entities: entities.len() as u64,
        relations: relations.len() as u64,
        diagnostics: diagnostics.len() as u64,
        skipped_nodes,
        merged_nodes,
        unresolved,
        coverage: coverage.to_json(),
    })
}

struct ParsedExport {
    version: String,
    manifest_node_count: u64,
    nodes: Vec<NodeRec>,
    edges: Vec<EdgeRec>,
}

fn parse_export(text: &str) -> Result<ParsedExport> {
    let mut version: Option<String> = None;
    let mut manifest_node_count: Option<u64> = None;
    let mut nodes = Vec::new();
    let mut edges = Vec::new();
    let mut seen_manifest = false;
    for (line_number, line) in text.lines().enumerate() {
        if line.trim().is_empty() {
            continue;
        }
        let value: Value = serde_json::from_str(line).map_err(|err| {
            CoreError::invalid(format!("export line {} is not JSON: {err}", line_number + 1))
        })?;
        match value["type"].as_str().unwrap_or("") {
            "manifest" => {
                if seen_manifest {
                    return Err(CoreError::invalid("export has more than one manifest line"));
                }
                seen_manifest = true;
                if value["format"].as_str() != Some(EXPORT_FORMAT) {
                    return Err(CoreError::unsupported(format!(
                        "export format {} is not {EXPORT_FORMAT}",
                        value["format"]
                    )));
                }
                version = value["version"].as_str().map(str::to_string);
                manifest_node_count = value["node_count"].as_u64();
            }
            "node" => {
                let backend_id = value["id"]
                    .as_u64()
                    .ok_or_else(|| CoreError::invalid("node line without numeric id"))?;
                let properties = value["properties"]
                    .as_object()
                    .cloned()
                    .unwrap_or_default();
                nodes.push(NodeRec {
                    backend_id,
                    node_type: value["node_type"].as_str().unwrap_or("Generic").to_string(),
                    path: property_string(Some(&properties), "path"),
                    name: property_string(Some(&properties), "name"),
                    line_start: property_u64(Some(&properties), "line_start"),
                    line_end: property_u64(Some(&properties), "line_end"),
                    raw_properties: properties,
                });
            }
            "edge" => {
                let backend_id = value["id"]
                    .as_u64()
                    .ok_or_else(|| CoreError::invalid("edge line without numeric id"))?;
                edges.push(EdgeRec {
                    backend_id,
                    source: value["source"]
                        .as_u64()
                        .ok_or_else(|| CoreError::invalid("edge without source"))?,
                    target: value["target"]
                        .as_u64()
                        .ok_or_else(|| CoreError::invalid("edge without target"))?,
                    edge_type: value["edge_type"].as_str().unwrap_or("").to_string(),
                    line: value["properties"]
                        .as_object()
                        .and_then(|props| property_u64(Some(props), "line")),
                });
            }
            other => {
                return Err(CoreError::invalid(format!(
                    "export line {} has unknown type {other:?}",
                    line_number + 1
                )))
            }
        }
    }
    if !seen_manifest {
        return Err(CoreError::invalid("export has no manifest line"));
    }
    Ok(ParsedExport {
        version: version.ok_or_else(|| CoreError::invalid("manifest has no version"))?,
        manifest_node_count: manifest_node_count
            .ok_or_else(|| CoreError::invalid("manifest has no node_count"))?,
        nodes,
        edges,
    })
}

fn property_bool(properties: &Map<String, Value>, key: &str) -> bool {
    properties.get(key).and_then(Value::as_bool).unwrap_or(false)
}

/// Merge duplicate backend references for one content identity instead of
/// emitting two conflicting entities (upstream reuses module names).
fn insert_or_merge_entity(
    entities: &mut Vec<Value>,
    index: &mut HashMap<String, usize>,
    entity_id: String,
    entity: Value,
) {
    if let Some(existing) = index.get(&entity_id) {
        let reference = entity["backend_refs"][0].clone();
        let references = entities[*existing]["backend_refs"]
            .as_array_mut()
            .expect("backend_refs array");
        if !references.iter().any(|item| item == &reference) {
            references.push(reference);
        }
        return;
    }
    index.insert(entity_id, entities.len());
    entities.push(entity);
}

fn property_string(properties: Option<&Map<String, Value>>, key: &str) -> Option<String> {
    properties?
        .get(key)?
        .as_str()
        .map(str::to_string)
        .filter(|value| !value.is_empty())
}

fn property_u64(properties: Option<&Map<String, Value>>, key: &str) -> Option<u64> {
    let value = properties?.get(key)?;
    value.as_u64().or_else(|| {
        value
            .as_i64()
            .and_then(|number| u64::try_from(number).ok())
    })
}

fn resolve_path<'a>(
    node: &NodeRec,
    workspace_root: Option<&Path>,
    by_path: &HashMap<&'a str, &'a FileEntry>,
    by_basename: &HashMap<&'a str, Vec<&'a FileEntry>>,
) -> Option<(String, &'a FileEntry)> {
    let raw = node.path.as_ref()?.replace('\\', "/");
    let root_prefix = workspace_root.map(|root| {
        let mut text = root.to_string_lossy().replace('\\', "/");
        if !text.ends_with('/') {
            text.push('/');
        }
        text
    });
    let relative = if let Some(prefix) = &root_prefix {
        if let Some(stripped) = raw.strip_prefix(prefix.as_str()) {
            stripped.to_string()
        } else if raw.starts_with('/') || raw.contains(':') {
            raw.clone()
        } else {
            raw.clone()
        }
    } else {
        raw.clone()
    };
    let normalized = relative.trim_start_matches("./").to_string();
    if let Some(file) = by_path.get(normalized.as_str()) {
        return Some((normalized, file));
    }
    // Fallback: upstream sometimes passes a canonicalized path outside the
    // declared root (symlinked temp dirs). Match on the basename suffix.
    if let Some((_, base)) = normalized.rsplit_once('/') {
        if let Some(candidates) = by_basename.get(base) {
            for candidate in candidates {
                if normalized.ends_with(&candidate.path) {
                    return Some((candidate.path.clone(), candidate));
                }
            }
            if candidates.len() == 1 {
                return Some((candidates[0].path.clone(), candidates[0]));
            }
        }
    }
    None
}

fn map_node_kind(node_type: &str) -> Option<(&'static str, bool)> {
    match node_type {
        "CodeFile" => Some(("file", true)),
        "Function" => Some(("symbol", false)),
        "Class" | "Interface" | "Type" => Some(("type", false)),
        "Module" => Some(("module", false)),
        "Variable" | "Generic" => Some(("symbol", false)),
        _ => None,
    }
}

fn map_edge_kind(edge_type: &str) -> Option<&'static str> {
    match edge_type {
        "Calls" | "Invokes" | "Instantiates" | "Uses" => Some("static_call_candidate"),
        "Imports" | "ImportsFrom" => Some("imports"),
        "Contains" => Some("contains"),
        "Defines" => Some("declares"),
        "Extends" | "Implements" => Some("implements"),
        "References" => Some("references"),
        // RuntimeCalls is a heuristic static correlation in upstream; claiming
        // observed_call would fabricate runtime evidence.
        _ => None,
    }
}

fn language_for(path: &str) -> String {
    match path.rsplit_once('.').map(|(_, ext)| ext) {
        Some("py") | Some("pyi") => "python",
        Some("rs") => "rust",
        Some("ts") | Some("tsx") => "typescript",
        Some("js") | Some("jsx") | Some("mjs") | Some("cjs") => "javascript",
        Some("c") | Some("h") => "c",
        Some("cc") | Some("cpp") | Some("cxx") | Some("hpp") | Some("hh") | Some("hxx") => "cpp",
        Some("java") => "java",
        Some("cs") => "csharp",
        Some("go") => "go",
        _ => "unknown",
    }
    .to_string()
}

fn diagnostic(code: &str, severity: &str, message: &str, entity_id: Option<&str>) -> Value {
    json!({
        "code": code,
        "severity": severity,
        "message": message,
        "entity_id": entity_id,
        "path": null,
    })
}

/// Ordered, deduplicated list of upstream edge types seen in an export. Used by
/// capability reporting, not for importing.
pub fn upstream_edge_types(text: &str) -> Result<BTreeSet<String>> {
    let mut types = BTreeSet::new();
    for line in text.lines() {
        if line.trim().is_empty() {
            continue;
        }
        let value: Value = serde_json::from_str(line)
            .map_err(|err| CoreError::invalid(format!("export line is not JSON: {err}")))?;
        if value["type"].as_str() == Some("edge") {
            if let Some(edge_type) = value["edge_type"].as_str() {
                types.insert(edge_type.to_string());
            }
        }
    }
    Ok(types)
}
