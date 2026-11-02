// Copyright 2026 ARCHMELD contributors
// SPDX-License-Identifier: Apache-2.0

//! Bounded graph queries (AT015).
//!
//! Every traversal has a hard node cap, an explicit depth budget and a
//! deterministic order. Pagination cursors are bound to the snapshot and to the
//! canonical query digest, so a cursor can never be replayed against a different
//! snapshot or a modified query.

use crate::error::{CoreError, Result};
use crate::hash;
use crate::source::LineIndex;
use crate::store::{validate, Coverage, CoverageState, Store};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet, VecDeque};

const HARD_NODE_CAP: usize = 2000;
const HARD_EDGE_CAP: usize = 200_000;
const BOUNDARY_EDGE_CAP: usize = 2000;
const MAX_SOURCE_SCAN_BYTES: u64 = 256 * 1024 * 1024;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum QueryKind {
    Symbols,
    Text,
    /// Literal, snapshot-bound search over file bytes.
    Source,
    Neighbors,
    Path,
    Impact,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct QueryRequest {
    pub project_id: String,
    pub snapshot_id: String,
    pub kind: QueryKind,
    pub query: String,
    pub entity_ids: Vec<String>,
    pub relation_kinds: Vec<super::store::RelationKind>,
    pub max_nodes: u32,
    pub max_depth: u32,
    pub cursor: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct CursorPayload {
    version: u32,
    snapshot_id: String,
    query_digest: String,
    offset: u64,
    /// Source search resumes at this snapshot path and byte offset. Other
    /// query kinds leave both unset.
    #[serde(default)]
    resume_path: Option<String>,
    #[serde(default)]
    resume_byte: u64,
}

struct EdgeRow {
    relation_id: String,
    source: String,
    target: String,
    kind: String,
    payload: Value,
}

struct EdgeScan {
    edges: Vec<EdgeRow>,
    truncated: bool,
}

/// Execute a query and return an object matching `QueryResult`.
pub fn run_query(store: &Store, raw: &Value) -> Result<Value> {
    let request: QueryRequest = serde_json::from_value(raw.clone())
        .map_err(|err| CoreError::invalid(format!("query does not match QueryRequest: {err}")))?;
    validate::require_id("project_id", &request.project_id)?;
    validate::require_digest("snapshot_id", &request.snapshot_id)?;
    if !(1..=2000).contains(&request.max_nodes) {
        return Err(CoreError::invalid("max_nodes must be 1..=2000"));
    }
    if request.max_depth > 8 {
        return Err(CoreError::invalid("max_depth must be 0..=8"));
    }
    for entity_id in &request.entity_ids {
        validate::require_id("entity_ids[]", entity_id)?;
    }
    store.check_snapshot_scope(&request.project_id, &request.snapshot_id)?;

    let query_digest = digest_request(raw)?;
    let (offset, resume_path, resume_byte) = match &request.cursor {
        None => (0, None, 0),
        Some(token) => {
            let cursor = decode_cursor(token)?;
            if cursor.snapshot_id != request.snapshot_id {
                return Err(CoreError::conflict(
                    "scope_conflict: cursor belongs to a different snapshot",
                ));
            }
            if cursor.query_digest != query_digest {
                return Err(CoreError::conflict(
                    "scope_conflict: cursor belongs to a different query",
                ));
            }
            (cursor.offset, cursor.resume_path, cursor.resume_byte)
        }
    };

    match request.kind {
        QueryKind::Symbols | QueryKind::Text => search(store, &request, &query_digest, offset),
        QueryKind::Source => {
            source_search(store, &request, &query_digest, resume_path.as_deref(), resume_byte)
        }
        QueryKind::Neighbors => traverse(store, &request, &query_digest, offset, false),
        QueryKind::Impact => traverse(store, &request, &query_digest, offset, true),
        QueryKind::Path => shortest_path(store, &request),
    }
}

fn search(
    store: &Store,
    request: &QueryRequest,
    query_digest: &str,
    offset: u64,
) -> Result<Value> {
    let pattern = format!("%{}%", escape_like(&request.query));
    let columns = if request.kind == QueryKind::Text {
        "qualified_name LIKE ?3 ESCAPE '\\' OR payload_json LIKE ?3 ESCAPE '\\'"
    } else {
        "qualified_name LIKE ?3 ESCAPE '\\'"
    };
    let total: i64 = store.conn.query_row(
        &format!(
            "SELECT COUNT(*) FROM entities WHERE project_id=?1 AND snapshot_id=?2 AND ({columns})"
        ),
        (&request.project_id, &request.snapshot_id, &pattern),
        |row| row.get(0),
    )?;
    let limit = request.max_nodes as i64;
    let offset_i = offset as i64;
    let sql = format!(
        "SELECT payload_json FROM entities
         WHERE project_id=?1 AND snapshot_id=?2 AND ({columns})
         ORDER BY entity_id LIMIT ?4 OFFSET ?5"
    );
    let mut statement = store.conn.prepare(&sql)?;
    let rows = statement.query_map(
        (
            &request.project_id,
            &request.snapshot_id,
            &pattern,
            limit,
            offset_i,
        ),
        |row| row.get::<_, String>(0),
    )?;
    let mut entities = Vec::new();
    for row in rows {
        let payload: Value = serde_json::from_str(&row?)
            .map_err(|err| CoreError::Storage(format!("entity payload corrupt: {err}")))?;
        entities.push(payload);
    }
    let total = total.max(0) as u64;
    let returned = entities.len() as u64;
    let has_more = offset + returned < total;
    let mut warnings = vec![if request.kind == QueryKind::Text {
        "substring search over qualified names and stored payloads; FTS is not enabled in this build"
            .to_string()
    } else {
        "substring search over qualified names; FTS is not enabled in this build".to_string()
    }];
    let limitations = if has_more {
        vec!["result paged by max_nodes; more matching entities exist".to_string()]
    } else {
        Vec::new()
    };
    let coverage = coverage(
        &request.snapshot_id,
        format!("query:{}", if request.kind == QueryKind::Text { "text" } else { "symbols" }),
        Some(total),
        returned,
        total.saturating_sub(returned),
        0,
        has_more,
        limitations,
        &mut warnings,
    );
    let next_cursor = if has_more {
        Some(encode_cursor(&CursorPayload {
            version: 1,
            snapshot_id: request.snapshot_id.clone(),
            query_digest: query_digest.to_string(),
            offset: offset + returned,
            resume_path: None,
            resume_byte: 0,
        })?)
    } else {
        None
    };
    Ok(query_result(
        &request.snapshot_id,
        entities,
        vec![],
        vec![],
        coverage,
        warnings,
        next_cursor,
        json!({
            "boundary_edges": [],
            "boundary_entities": [],
            "pagination": {
                "offset": offset,
                "returned": returned,
                "total": total,
                "has_more": has_more,
            },
        }),
    ))
}

fn source_search(
    store: &Store,
    request: &QueryRequest,
    query_digest: &str,
    resume_path: Option<&str>,
    resume_byte: u64,
) -> Result<Value> {
    if request.query.is_empty() {
        return Err(CoreError::invalid(
            "source queries require a non-empty literal",
        ));
    }
    let needle = request.query.as_bytes();
    let limit = request.max_nodes as usize;
    let mut statement = store.conn.prepare(
        "SELECT path, blob_digest FROM files
         WHERE project_id=?1 AND snapshot_id=?2 ORDER BY path",
    )?;
    let rows = statement.query_map(
        (&request.project_id, &request.snapshot_id),
        |row| Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?)),
    )?;
    let mut matches: Vec<Value> = Vec::new();
    let mut next_resume: Option<(String, u64)> = None;
    let mut scanned_files = 0u64;
    let mut bytes_scanned = 0u64;
    let mut non_utf8_files = 0u64;
    let mut span_cache: BTreeMap<String, Vec<(String, u64, u64)>> = BTreeMap::new();
    'files: for row in rows {
        let (path, digest) = row?;
        if resume_path.is_some_and(|resume| path.as_str() < resume) {
            continue;
        }
        if bytes_scanned >= MAX_SOURCE_SCAN_BYTES {
            next_resume = Some((path, 0));
            break;
        }
        let Some(blob) = store.read_blob_bytes(&digest)? else {
            continue;
        };
        bytes_scanned += blob.len() as u64;
        scanned_files += 1;
        let Ok(text) = std::str::from_utf8(&blob) else {
            non_utf8_files += 1;
            continue;
        };
        let index = LineIndex::new(&blob);
        let mut from = if resume_path == Some(path.as_str()) {
            resume_byte.min(blob.len() as u64) as usize
        } else {
            0
        };
        let mut file_spans: Option<Vec<(String, u64, u64)>> = None;
        while let Some(relative) = find_bytes(&blob[from..], needle) {
            let start = from + relative;
            let end = start + needle.len();
            if !text.is_char_boundary(start) || !text.is_char_boundary(end) {
                from = end;
                continue;
            }
            if matches.len() == limit {
                let last = matches.last().expect("limit is at least one");
                next_resume = Some((
                    last["path"].as_str().unwrap_or("").to_string(),
                    last["byte_end"].as_u64().unwrap_or(0),
                ));
                break 'files;
            }
            if file_spans.is_none() {
                let loaded = match span_cache.get(&path) {
                    Some(spans) => spans.clone(),
                    None => {
                        let spans = entity_spans_for_path(
                            store,
                            &request.project_id,
                            &request.snapshot_id,
                            &path,
                        )?;
                        span_cache.insert(path.clone(), spans.clone());
                        spans
                    }
                };
                file_spans = Some(loaded);
            }
            let spans = file_spans.as_ref().expect("spans loaded above");
            let (line0, column0) = index.line_col(start as u64).unwrap_or((0, 0));
            let line_start = index.line_start(line0).unwrap_or(0) as usize;
            let line_end = index
                .line_start(line0 + 1)
                .map(|value| value as usize)
                .unwrap_or(text.len());
            let mut window_start = start.saturating_sub(120).max(line_start);
            while window_start < start && !text.is_char_boundary(window_start) {
                window_start += 1;
            }
            let mut window_end = (end + 300).min(line_end);
            while window_end > end && !text.is_char_boundary(window_end) {
                window_end -= 1;
            }
            let line = line0 + 1;
            let entity_ids: Vec<&str> = spans
                .iter()
                .filter(|(_, first, last)| *first <= line && line <= *last)
                .map(|(id, _, _)| id.as_str())
                .collect();
            matches.push(json!({
                "path": path,
                "blob_digest": digest,
                "line": line,
                "column": column0 + 1,
                "byte_start": start,
                "byte_end": end,
                "text": &text[window_start..window_end],
                "text_truncated": window_start > line_start || window_end < line_end,
                "entity_ids": entity_ids,
            }));
            from = end;
        }
    }
    let has_more = next_resume.is_some();
    let returned = matches.len() as u64;
    let mut warnings = vec![
        "literal, case-sensitive search over snapshot bytes; this is not semantic analysis"
            .to_string(),
    ];
    if non_utf8_files > 0 {
        warnings.push(format!(
            "{non_utf8_files} non-UTF-8 file(s) were skipped"
        ));
    }
    if has_more {
        warnings.push(
            "source search stopped at the page or scan byte limit; next_cursor resumes"
                .to_string(),
        );
    }
    let mut limitations = vec![
        "literal byte search; no tokenization or semantic matching".to_string(),
        if has_more {
            "the remaining snapshot was not scanned on this page".to_string()
        } else {
            "scan completed within the page and byte limits".to_string()
        },
    ];
    if non_utf8_files > 0 {
        limitations.push(format!(
            "{non_utf8_files} non-UTF-8 file(s) were skipped and are not searchable"
        ));
    }
    let incomplete = has_more || non_utf8_files > 0;
    let coverage = coverage(
        &request.snapshot_id,
        "query:source".to_string(),
        if incomplete { None } else { Some(returned) },
        returned,
        0,
        if incomplete { 1 } else { 0 },
        incomplete,
        limitations,
        &mut warnings,
    );
    let next_cursor = match next_resume {
        Some((path, byte)) => Some(encode_cursor(&CursorPayload {
            version: 1,
            snapshot_id: request.snapshot_id.clone(),
            query_digest: query_digest.to_string(),
            offset: 0,
            resume_path: Some(path),
            resume_byte: byte,
        })?),
        None => None,
    };
    Ok(query_result(
        &request.snapshot_id,
        vec![],
        vec![],
        vec![],
        coverage,
        warnings,
        next_cursor,
        json!({
            "source_matches": matches,
            "pagination": {
                "returned": returned,
                "has_more": has_more,
                "total": null,
                "resumed": resume_path.is_some(),
                "scan_complete": !has_more,
                "files_scanned": scanned_files,
                "bytes_scanned": bytes_scanned,
                "non_utf8_files": non_utf8_files,
                "scan_byte_limit": MAX_SOURCE_SCAN_BYTES,
            },
        }),
    ))
}

pub(crate) fn entity_spans_for_path(
    store: &Store,
    project_id: &str,
    snapshot_id: &str,
    path: &str,
) -> Result<Vec<(String, u64, u64)>> {
    let marker = format!(
        "\"path\":{}",
        serde_json::to_string(path)
            .map_err(|error| CoreError::Storage(format!("source path marker: {error}")))?
    );
    let pattern = format!("%{}%", escape_like(&marker));
    let mut statement = store.conn.prepare(
        "SELECT payload_json FROM entities
         WHERE project_id=?1 AND snapshot_id=?2 AND payload_json LIKE ?3 ESCAPE '\\'",
    )?;
    let rows = statement.query_map((project_id, snapshot_id, &pattern), |row| {
        row.get::<_, String>(0)
    })?;
    let mut spans = BTreeSet::new();
    for row in rows {
        let payload: Value = serde_json::from_str(&row?)
            .map_err(|error| CoreError::Storage(format!("entity payload corrupt: {error}")))?;
        let Some(entity_id) = payload["id"].as_str() else {
            continue;
        };
        for anchor in payload["anchors"].as_array().into_iter().flatten() {
            if anchor["path"].as_str() != Some(path) {
                continue;
            }
            if let (Some(first), Some(last)) =
                (anchor["line_start"].as_u64(), anchor["line_end"].as_u64())
            {
                spans.insert((entity_id.to_string(), first, last));
            }
        }
    }
    Ok(spans.into_iter().collect())
}

fn find_bytes(haystack: &[u8], needle: &[u8]) -> Option<usize> {
    if needle.is_empty() || needle.len() > haystack.len() {
        return None;
    }
    haystack.windows(needle.len()).position(|window| window == needle)
}

fn traverse(
    store: &Store,
    request: &QueryRequest,
    query_digest: &str,
    offset: u64,
    reverse: bool,
) -> Result<Value> {
    traverse_with_cap(
        store,
        request,
        query_digest,
        offset,
        reverse,
        HARD_EDGE_CAP,
    )
}

fn traverse_with_cap(
    store: &Store,
    request: &QueryRequest,
    query_digest: &str,
    offset: u64,
    reverse: bool,
    edge_cap: usize,
) -> Result<Value> {
    if request.entity_ids.is_empty() {
        return Err(CoreError::invalid(
            "neighbors/impact queries require at least one entity_id",
        ));
    }
    let scan = load_edges_with_cap(store, request, edge_cap)?;
    let edges = &scan.edges;
    let mut warnings = Vec::new();
    let (order, depths, visited, depth_cut, cap_cut) =
        breadth_first(edges, request, reverse, &mut warnings);
    let mut entities = Vec::with_capacity(order.len());
    for (entity_id, _depth) in &order {
        match load_entity(store, &request.project_id, &request.snapshot_id, entity_id)? {
            Some(entity) => entities.push(entity),
            None => warnings.push(format!("entity {entity_id} disappeared during the query")),
        }
    }
    let total = entities.len() as u64;
    let start = (offset as usize).min(entities.len());
    let end = (start + request.max_nodes as usize).min(entities.len());
    let page: Vec<Value> = entities[start..end].to_vec();
    let page_ids: HashSet<String> = page
        .iter()
        .filter_map(|entity| entity["id"].as_str().map(str::to_string))
        .collect();
    let mut relations = Vec::new();
    let mut boundary_edges = Vec::new();
    let mut boundary_entities: BTreeMap<String, Value> = BTreeMap::new();
    let mut boundary_truncated = false;
    for edge in edges {
        let source_in = page_ids.contains(&edge.source);
        let target_in = page_ids.contains(&edge.target);
        if source_in && target_in {
            relations.push(edge.payload.clone());
            continue;
        }
        if source_in == target_in {
            continue;
        }
        if boundary_edges.len() >= BOUNDARY_EDGE_CAP {
            boundary_truncated = true;
            break;
        }
        let (included, missing, direction) = if source_in {
            (&edge.source, &edge.target, "outgoing")
        } else {
            (&edge.target, &edge.source, "incoming")
        };
        let stub = if let Some(stub) = boundary_entities.get(missing) {
            stub.clone()
        } else {
            let stub = match load_entity(
                store,
                &request.project_id,
                &request.snapshot_id,
                missing,
            )? {
                Some(entity) => json!({
                    "entity_id": missing,
                    "resolved": true,
                    "kind": entity["kind"],
                    "language": entity["language"],
                    "qualified_name": entity["qualified_name"],
                }),
                None => json!({"entity_id": missing, "resolved": false}),
            };
            boundary_entities.insert(missing.clone(), stub.clone());
            stub
        };
        boundary_edges.push(json!({
            "relation": edge.payload,
            "relation_kind": edge.kind,
            "included_entity_id": included,
            "missing_entity_id": missing,
            "direction": direction,
            "missing_entity": stub,
        }));
    }
    let boundary_entities: Vec<Value> = boundary_entities.into_values().collect();
    let evidence = load_evidence_for(store, &request.project_id, &request.snapshot_id, &relations)?;
    let has_more = end < entities.len();
    let mut limitations = Vec::new();
    if has_more {
        warnings.push("result paged by max_nodes; next_cursor continues this query".to_string());
        limitations.push("result paged by max_nodes; more entities exist".to_string());
    }
    if cap_cut {
        warnings.push(format!(
            "traversal capped at {HARD_NODE_CAP} nodes; result is partial"
        ));
        limitations.push(format!("traversal node cap ({HARD_NODE_CAP}) reached"));
    }
    if depth_cut {
        warnings.push("neighbors remain beyond max_depth; result is partial".to_string());
        limitations.push("neighbors remain beyond max_depth".to_string());
    }
    if scan.truncated {
        warnings.push(format!(
            "relation scan capped at {edge_cap} edges; unseen edges may change this result"
        ));
        limitations.push(format!(
            "relation scan stopped at {edge_cap} edges; at least one relation was not examined"
        ));
    }
    if boundary_truncated {
        warnings.push(format!(
            "boundary edge list capped at {BOUNDARY_EDGE_CAP}; later pages still carry their own boundary edges"
        ));
        limitations.push("boundary edge list was capped for this page".to_string());
    }
    let truncated = has_more || depth_cut || cap_cut || scan.truncated || boundary_truncated;
    let _ = &visited;
    let _ = &depths;
    let coverage = coverage(
        &request.snapshot_id,
        format!("query:{}", if reverse { "impact" } else { "neighbors" }),
        Some(total),
        page.len() as u64,
        total.saturating_sub(page.len() as u64),
        if scan.truncated || boundary_truncated { 1 } else { 0 },
        truncated,
        limitations,
        &mut warnings,
    );
    let next_cursor = if has_more {
        Some(encode_cursor(&CursorPayload {
            version: 1,
            snapshot_id: request.snapshot_id.clone(),
            query_digest: query_digest.to_string(),
            offset: end as u64,
            resume_path: None,
            resume_byte: 0,
        })?)
    } else {
        None
    };
    Ok(query_result(
        &request.snapshot_id,
        page,
        relations,
        evidence,
        coverage,
        warnings,
        next_cursor,
        json!({
            "boundary_edges": boundary_edges,
            "boundary_entities": boundary_entities,
            "pagination": {
                "offset": start,
                "page_size": request.max_nodes,
                "returned": end.saturating_sub(start),
                "total": total,
                "has_more": has_more,
                "boundary_edges_truncated": boundary_truncated,
                "edge_scan_truncated": scan.truncated,
            },
        }),
    ))
}

fn shortest_path(store: &Store, request: &QueryRequest) -> Result<Value> {
    if request.entity_ids.len() < 2 {
        return Err(CoreError::invalid(
            "path queries require source and target entity_ids",
        ));
    }
    let source = &request.entity_ids[0];
    let target = &request.entity_ids[1];
    let scan = load_edges(store, request)?;
    let edges = &scan.edges;
    let mut forward: HashMap<&str, Vec<&EdgeRow>> = HashMap::new();
    let mut reverse: HashMap<&str, Vec<&EdgeRow>> = HashMap::new();
    for edge in edges {
        forward.entry(edge.source.as_str()).or_default().push(edge);
        reverse.entry(edge.target.as_str()).or_default().push(edge);
    }
    for list in forward.values_mut() {
        list.sort_by(|a, b| a.relation_id.cmp(&b.relation_id));
    }
    for list in reverse.values_mut() {
        list.sort_by(|a, b| a.relation_id.cmp(&b.relation_id));
    }

    // Bidirectional BFS bounded by max_depth; parents record the edge used.
    let mut parents: HashMap<String, (String, String)> = HashMap::new(); // node -> (prev, edge_id)
    let mut depth: HashMap<String, u32> = HashMap::new();
    let mut queue = VecDeque::new();
    depth.insert(source.clone(), 0);
    queue.push_back(source.clone());
    let mut found = false;
    let mut depth_cut = false;
    while let Some(node) = queue.pop_front() {
        let d = depth[&node];
        if node == *target {
            found = true;
            break;
        }
        if d >= request.max_depth {
            if forward.get(node.as_str()).is_some_and(|list| !list.is_empty()) {
                depth_cut = true;
            }
            continue;
        }
        if let Some(list) = forward.get(node.as_str()) {
            for edge in list {
                if depth.contains_key(&edge.target) {
                    continue;
                }
                depth.insert(edge.target.clone(), d + 1);
                parents.insert(edge.target.clone(), (node.clone(), edge.relation_id.clone()));
                queue.push_back(edge.target.clone());
            }
        }
    }
    let mut warnings = Vec::new();
    let mut entities = Vec::new();
    let mut relations = Vec::new();
    let mut truncated = scan.truncated;
    let mut limitations = Vec::new();
    if scan.truncated {
        warnings.push(
            "relation scan capped before every edge was examined; a path may be longer or absent"
                .to_string(),
        );
        limitations.push("relation scan cap reached".to_string());
    }
    if found {
        let mut chain = Vec::new();
        let mut current = target.clone();
        while current != *source {
            let (prev, edge_id) = parents
                .get(&current)
                .ok_or_else(|| CoreError::Storage("path reconstruction lost a parent".to_string()))?
                .clone();
            chain.push((current.clone(), Some(edge_id)));
            current = prev;
        }
        chain.push((source.clone(), None));
        chain.reverse();
        for (index, (entity_id, edge_id)) in chain.iter().enumerate() {
            if index >= request.max_nodes as usize {
                truncated = true;
                warnings.push("path longer than max_nodes; result truncated".to_string());
                limitations.push("path result truncated by max_nodes".to_string());
                break;
            }
            if let Some(entity) =
                load_entity(store, &request.project_id, &request.snapshot_id, entity_id)?
            {
                entities.push(entity);
            }
            if let Some(edge_id) = edge_id {
                if let Some(edge) = edges.iter().find(|edge| &edge.relation_id == edge_id) {
                    relations.push(edge.payload.clone());
                }
            }
        }
    } else {
        warnings.push(format!(
            "no path from {source} to {target} within max_depth={}",
            request.max_depth
        ));
        if depth_cut {
            warnings.push("search stopped at max_depth; a longer path may exist".to_string());
            limitations.push("search stopped at max_depth".to_string());
            truncated = true;
        }
        if scan.truncated {
            warnings.push(
                "the relation scan was truncated, so the absence of a path is not established"
                    .to_string(),
            );
        }
    }
    let evidence = load_evidence_for(store, &request.project_id, &request.snapshot_id, &relations)?;
    let total = if found { entities.len() as u64 } else { 0 };
    let returned = entities.len() as u64;
    let coverage = coverage(
        &request.snapshot_id,
        "query:path".to_string(),
        Some(total),
        returned,
        0,
        if truncated { 1 } else { 0 },
        truncated,
        limitations,
        &mut warnings,
    );
    Ok(query_result(
        &request.snapshot_id,
        entities,
        relations,
        evidence,
        coverage,
        warnings,
        None,
        json!({"boundary_edges": [], "boundary_entities": [], "pagination": {"offset": 0, "returned": returned, "total": total, "has_more": false}}),
    ))
}

fn breadth_first(
    edges: &[EdgeRow],
    request: &QueryRequest,
    reverse: bool,
    warnings: &mut Vec<String>,
) -> (Vec<(String, u32)>, HashMap<String, u32>, HashSet<String>, bool, bool) {
    let mut adjacency: HashMap<&str, Vec<&EdgeRow>> = HashMap::new();
    for edge in edges {
        let (from, to) = if reverse {
            (edge.target.as_str(), edge.source.as_str())
        } else {
            (edge.source.as_str(), edge.target.as_str())
        };
        let _ = to;
        adjacency.entry(from).or_default().push(edge);
    }
    for list in adjacency.values_mut() {
        list.sort_by(|a, b| a.relation_id.cmp(&b.relation_id));
    }
    let mut depths: HashMap<String, u32> = HashMap::new();
    let mut queue = VecDeque::new();
    for seed in &request.entity_ids {
        if depths.insert(seed.clone(), 0).is_none() {
            queue.push_back(seed.clone());
        }
    }
    let mut visited: HashSet<String> = depths.keys().cloned().collect();
    let mut depth_cut = false;
    let mut cap_cut = false;
    while let Some(node) = queue.pop_front() {
        let d = depths[&node];
        if let Some(list) = adjacency.get(node.as_str()) {
            if d >= request.max_depth {
                if list.iter().any(|edge| {
                    let next = if reverse { &edge.source } else { &edge.target };
                    !visited.contains(next)
                }) {
                    depth_cut = true;
                }
                continue;
            }
            for edge in list {
                let next = if reverse {
                    edge.source.clone()
                } else {
                    edge.target.clone()
                };
                if visited.insert(next.clone()) {
                    if visited.len() > HARD_NODE_CAP {
                        cap_cut = true;
                        break;
                    }
                    depths.insert(next.clone(), d + 1);
                    queue.push_back(next);
                }
            }
        }
        if cap_cut {
            break;
        }
    }
    let mut order: Vec<(String, u32)> = depths
        .iter()
        .filter(|(id, _)| visited.contains(*id))
        .map(|(id, depth)| (id.clone(), *depth))
        .collect();
    order.sort_by(|a, b| a.1.cmp(&b.1).then(a.0.cmp(&b.0)));
    if cap_cut {
        order.truncate(HARD_NODE_CAP);
        warnings.push("traversal node cap reached".to_string());
    }
    (order, depths, visited, depth_cut, cap_cut)
}

fn load_edges(store: &Store, request: &QueryRequest) -> Result<EdgeScan> {
    load_edges_with_cap(store, request, HARD_EDGE_CAP)
}

fn load_edges_with_cap(
    store: &Store,
    request: &QueryRequest,
    cap: usize,
) -> Result<EdgeScan> {
    let kinds: Vec<String> = request
        .relation_kinds
        .iter()
        .map(|kind| kind.as_str().to_string())
        .collect();
    let limit = cap.saturating_add(1);
    let (sql, params) = if kinds.is_empty() {
        (
            format!(
                "SELECT relation_id, source_id, target_id, kind, payload_json FROM relations
                 WHERE project_id=?1 AND snapshot_id=?2 ORDER BY relation_id LIMIT {limit}"
            ),
            vec![request.project_id.clone(), request.snapshot_id.clone()],
        )
    } else {
        let placeholders: Vec<String> = (3..3 + kinds.len()).map(|i| format!("?{i}")).collect();
        let mut params = vec![request.project_id.clone(), request.snapshot_id.clone()];
        params.extend(kinds.iter().cloned());
        (
            format!(
                "SELECT relation_id, source_id, target_id, kind, payload_json FROM relations
                 WHERE project_id=?1 AND snapshot_id=?2 AND kind IN ({})
                 ORDER BY relation_id LIMIT {limit}",
                placeholders.join(",")
            ),
            params,
        )
    };
    let mut statement = store.conn.prepare(&sql)?;
    let rows = statement.query_map(rusqlite::params_from_iter(params.iter()), |row| {
        Ok((
            row.get::<_, String>(0)?,
            row.get::<_, String>(1)?,
            row.get::<_, String>(2)?,
            row.get::<_, String>(3)?,
            row.get::<_, String>(4)?,
        ))
    })?;
    let mut edges = Vec::new();
    let mut truncated = false;
    for row in rows {
        let (relation_id, source, target, kind, payload_json) = row?;
        if edges.len() >= cap {
            truncated = true;
            break;
        }
        let payload: Value = serde_json::from_str(&payload_json)
            .map_err(|err| CoreError::Storage(format!("relation payload corrupt: {err}")))?;
        edges.push(EdgeRow {
            relation_id,
            source,
            target,
            kind,
            payload,
        });
    }
    Ok(EdgeScan { edges, truncated })
}

fn load_entity(
    store: &Store,
    project_id: &str,
    snapshot_id: &str,
    entity_id: &str,
) -> Result<Option<Value>> {
    let payload: Option<String> = store
        .conn
        .query_row(
            "SELECT payload_json FROM entities WHERE project_id=?1 AND snapshot_id=?2 AND entity_id=?3",
            (project_id, snapshot_id, entity_id),
            |row| row.get(0),
        )
        .ok();
    payload
        .map(|text| {
            serde_json::from_str(&text)
                .map_err(|err| CoreError::Storage(format!("entity payload corrupt: {err}")))
        })
        .transpose()
}

fn load_evidence_for(
    store: &Store,
    project_id: &str,
    snapshot_id: &str,
    relations: &[Value],
) -> Result<Vec<Value>> {
    let mut ids: BTreeSet<String> = BTreeSet::new();
    for relation in relations {
        if let Some(list) = relation["evidence_ids"].as_array() {
            for id in list {
                if let Some(id) = id.as_str() {
                    ids.insert(id.to_string());
                }
            }
        }
    }
    let mut evidence = Vec::new();
    for id in ids {
        let payload: Option<String> = store
            .conn
            .query_row(
                "SELECT payload_json FROM evidence WHERE project_id=?1 AND snapshot_id=?2 AND evidence_id=?3",
                (project_id, snapshot_id, &id),
                |row| row.get(0),
            )
            .ok();
        if let Some(text) = payload {
            evidence.push(
                serde_json::from_str(&text)
                    .map_err(|err| CoreError::Storage(format!("evidence payload corrupt: {err}")))?,
            );
        }
    }
    Ok(evidence)
}

fn coverage(
    snapshot_id: &str,
    scope: String,
    expected: Option<u64>,
    observed: u64,
    omitted: u64,
    unresolved: u64,
    truncated: bool,
    limitations: Vec<String>,
    warnings: &mut Vec<String>,
) -> Coverage {
    let state = if truncated || omitted > 0 || unresolved > 0 {
        CoverageState::Partial
    } else {
        CoverageState::Complete
    };
    if state == CoverageState::Partial {
        warnings.push("coverage is partial: the requested budget was reached".to_string());
    }
    Coverage {
        state,
        scope: format!("{scope} @ {snapshot_id}"),
        expected_count: expected,
        observed_count: observed,
        omitted_count: omitted,
        unresolved_count: unresolved,
        limitations,
    }
}

fn query_result(
    snapshot_id: &str,
    entities: Vec<Value>,
    relations: Vec<Value>,
    evidence: Vec<Value>,
    coverage: Coverage,
    warnings: Vec<String>,
    next_cursor: Option<String>,
    extra: Value,
) -> Value {
    let mut result = json!({
        "snapshot_id": snapshot_id,
        "entities": entities,
        "relations": relations,
        "evidence": evidence,
        "coverage": coverage.to_json(),
        "warnings": warnings,
        "next_cursor": next_cursor,
    });
    if let (Some(object), Value::Object(extra)) = (result.as_object_mut(), extra) {
        for (key, value) in extra {
            object.insert(key, value);
        }
    }
    result
}

fn digest_request(raw: &Value) -> Result<String> {
    let mut normalized = raw.clone();
    if let Some(object) = normalized.as_object_mut() {
        object.remove("cursor");
    }
    hash::digest_value(&normalized)
        .map_err(|err| CoreError::invalid(format!("query is outside the hash domain: {err}")))
}

fn encode_cursor(cursor: &CursorPayload) -> Result<String> {
    let bytes = serde_json::to_vec(cursor)
        .map_err(|err| CoreError::Storage(format!("cursor encode: {err}")))?;
    Ok(format!("c1:{}", hex_encode(&bytes)))
}

fn decode_cursor(token: &str) -> Result<CursorPayload> {
    let hex = token
        .strip_prefix("c1:")
        .ok_or_else(|| CoreError::invalid("cursor has an unknown format"))?;
    let bytes = hex_decode(hex).ok_or_else(|| CoreError::invalid("cursor is not valid hex"))?;
    serde_json::from_slice(&bytes).map_err(|_| CoreError::invalid("cursor payload is corrupt"))
}

fn escape_like(text: &str) -> String {
    text.replace('\\', "\\\\")
        .replace('%', "\\%")
        .replace('_', "\\_")
}

fn hex_encode(bytes: &[u8]) -> String {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut out = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        out.push(HEX[(byte >> 4) as usize] as char);
        out.push(HEX[(byte & 0x0f) as usize] as char);
    }
    out
}

fn hex_decode(text: &str) -> Option<Vec<u8>> {
    if text.len() % 2 != 0 {
        return None;
    }
    let bytes = text.as_bytes();
    let mut out = Vec::with_capacity(bytes.len() / 2);
    for pair in bytes.chunks(2) {
        let high = (pair[0] as char).to_digit(16)?;
        let low = (pair[1] as char).to_digit(16)?;
        out.push((high * 16 + low) as u8);
    }
    Some(out)
}

impl Store {
    pub(crate) fn check_snapshot_scope(&self, project_id: &str, snapshot_id: &str) -> Result<()> {
        let exists: Option<String> = self
            .conn
            .query_row(
                "SELECT project_id FROM snapshots WHERE snapshot_id=?1",
                (snapshot_id,),
                |row| row.get(0),
            )
            .ok();
        match exists {
            None => Err(CoreError::not_found(format!(
                "snapshot {snapshot_id} is not registered"
            ))),
            Some(owner) if owner != project_id => Err(CoreError::invalid(format!(
                "snapshot {snapshot_id} belongs to project {owner}"
            ))),
            Some(_) => Ok(()),
        }
    }

    pub fn get_entity(
        &self,
        project_id: &str,
        snapshot_id: &str,
        entity_id: &str,
    ) -> Result<Option<Value>> {
        validate::require_id("entity_id", entity_id)?;
        self.check_snapshot_scope(project_id, snapshot_id)?;
        load_entity(self, project_id, snapshot_id, entity_id)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::snapshot::{capture_snapshot, SnapshotInputs, SnapshotOptions};
    use serde_json::json;
    use std::fs;

    fn seed() -> (tempfile::TempDir, Store, String, String) {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join("repo");
        fs::create_dir_all(&root).unwrap();
        fs::write(root.join("a.py"), "x = 1\n").unwrap();
        let mut store = Store::open(dir.path().join("state")).unwrap();
        store.ensure_project("project.cap", "expedition").unwrap();
        let captured = capture_snapshot(
            &mut store,
            "project.cap",
            &root,
            &SnapshotInputs {
                build_config: json!({"variant": "cap"}),
                environment: json!({"os": "test"}),
            },
            &SnapshotOptions {
                include_untracked: true,
                ..SnapshotOptions::default()
            },
        )
        .unwrap();
        let config = captured.snapshot["config_digest"]
            .as_str()
            .unwrap()
            .to_string();
        (dir, store, captured.snapshot_id, config)
    }

    fn add_entity(store: &Store, snapshot: &str, config: &str, id: &str, name: &str) {
        let payload = json!({
            "id": id,
            "kind": "symbol",
            "language": "python",
            "qualified_name": name,
            "config_digest": config,
            "anchors": [],
        });
        store
            .conn
            .execute(
                "INSERT INTO entities (project_id,snapshot_id,entity_id,config_digest,kind,language,qualified_name,external,adapter_id,run_id,payload_json)
                 VALUES (?1,?2,?3,?4,'symbol','python',?5,0,'test','run.cap',?6)",
                (
                    "project.cap",
                    snapshot,
                    id,
                    config,
                    name,
                    payload.to_string(),
                ),
            )
            .unwrap();
    }

    fn add_relation(store: &Store, snapshot: &str, config: &str, id: &str) {
        let payload = json!({
            "id": id,
            "project_id": "project.cap",
            "snapshot_id": snapshot,
            "config_digest": config,
            "source_id": "entity.a",
            "target_id": "entity.b",
            "kind": "contains",
            "basis": "syntax",
            "evidence_ids": [],
            "resolution": "candidate",
        });
        store
            .conn
            .execute(
                "INSERT INTO relations (project_id,snapshot_id,relation_id,config_digest,source_id,target_id,kind,basis,resolution,payload_json)
                 VALUES ('project.cap',?1,?2,?3,'entity.a','entity.b','contains','syntax','candidate',?4)",
                (snapshot, id, config, payload.to_string()),
            )
            .unwrap();
    }

    fn add_many_relations(
        store: &mut Store,
        snapshot: &str,
        config: &str,
        count: usize,
    ) {
        let tx = store.conn.transaction().unwrap();
        {
            let mut statement = tx
                .prepare(
                    "INSERT INTO relations
                     (project_id,snapshot_id,relation_id,config_digest,source_id,target_id,kind,basis,resolution,payload_json)
                     VALUES ('project.cap',?1,?2,?3,'entity.a','entity.b','contains','syntax','candidate',?4)",
                )
                .unwrap();
            for index in 0..count {
                let id = format!("rel.{index:06}");
                let payload = json!({
                    "id": id,
                    "project_id": "project.cap",
                    "snapshot_id": snapshot,
                    "config_digest": config,
                    "source_id": "entity.a",
                    "target_id": "entity.b",
                    "kind": "contains",
                    "basis": "syntax",
                    "resolution": "candidate",
                    "evidence_ids": [],
                });
                statement
                    .execute((snapshot, &id, config, payload.to_string()))
                    .unwrap();
            }
        }
        tx.commit().unwrap();
    }

    #[test]
    fn production_edge_cap_detects_200001_rows_without_claiming_completeness() {
        let (_dir, mut store, snapshot, config) = seed();
        add_entity(&store, &snapshot, &config, "entity.a", "a");
        add_entity(&store, &snapshot, &config, "entity.b", "b");
        add_many_relations(&mut store, &snapshot, &config, HARD_EDGE_CAP + 1);
        let request = QueryRequest {
            project_id: "project.cap".to_string(),
            snapshot_id: snapshot,
            kind: QueryKind::Neighbors,
            query: String::new(),
            entity_ids: vec!["entity.a".to_string()],
            relation_kinds: vec![crate::store::RelationKind::Contains],
            max_nodes: 1,
            max_depth: 1,
            cursor: None,
        };
        let result = traverse(&store, &request, "digest", 0, false).unwrap();
        assert_eq!(result["pagination"]["edge_scan_truncated"], true);
        assert_eq!(result["pagination"]["boundary_edges_truncated"], true);
        assert_eq!(result["coverage"]["state"], "partial");
        assert_eq!(result["coverage"]["unresolved_count"], 1);
        assert_eq!(
            result["boundary_edges"].as_array().unwrap().len(),
            BOUNDARY_EDGE_CAP
        );
        assert!(result["warnings"]
            .as_array()
            .unwrap()
            .iter()
            .any(|warning| warning.as_str().unwrap().contains("relation scan capped at 200000")));
    }

    #[test]
    fn edge_cap_is_detected_and_reported_as_partial() {
        let (_dir, store, snapshot, config) = seed();
        add_entity(&store, &snapshot, &config, "entity.a", "a");
        add_entity(&store, &snapshot, &config, "entity.b", "b");
        for index in 0..3 {
            add_relation(&store, &snapshot, &config, &format!("rel.{index}"));
        }
        let request = QueryRequest {
            project_id: "project.cap".to_string(),
            snapshot_id: snapshot,
            kind: QueryKind::Neighbors,
            query: String::new(),
            entity_ids: vec!["entity.a".to_string()],
            relation_kinds: vec![crate::store::RelationKind::Contains],
            max_nodes: 10,
            max_depth: 2,
            cursor: None,
        };
        let result = traverse_with_cap(&store, &request, "digest", 0, false, 2).unwrap();
        assert_eq!(result["pagination"]["edge_scan_truncated"], true);
        assert_eq!(result["coverage"]["state"], "partial");
        assert_eq!(result["coverage"]["unresolved_count"], 1);
        assert!(result["warnings"]
            .as_array()
            .unwrap()
            .iter()
            .any(|warning| warning.as_str().unwrap().contains("relation scan capped")));
    }
}
