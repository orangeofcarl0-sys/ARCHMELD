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
use crate::store::{validate, Coverage, CoverageState, Store};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::collections::{BTreeSet, HashMap, HashSet, VecDeque};

const HARD_NODE_CAP: usize = 2000;
const HARD_EDGE_CAP: usize = 200_000;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum QueryKind {
    Symbols,
    Text,
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
}

struct EdgeRow {
    relation_id: String,
    source: String,
    target: String,
    kind: String,
    payload: Value,
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
    let offset = match &request.cursor {
        None => 0,
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
            cursor.offset
        }
    };

    match request.kind {
        QueryKind::Symbols | QueryKind::Text => search(store, &request, &query_digest, offset),
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
    let coverage = coverage(
        &request.snapshot_id,
        format!("query:{}", if request.kind == QueryKind::Text { "text" } else { "symbols" }),
        total,
        returned,
        has_more,
        &mut warnings,
    );
    let next_cursor = if has_more {
        Some(encode_cursor(&CursorPayload {
            version: 1,
            snapshot_id: request.snapshot_id.clone(),
            query_digest: query_digest.to_string(),
            offset: offset + returned,
        })?)
    } else {
        None
    };
    Ok(query_result(&request.snapshot_id, entities, vec![], vec![], coverage, warnings, next_cursor))
}

fn traverse(
    store: &Store,
    request: &QueryRequest,
    query_digest: &str,
    offset: u64,
    reverse: bool,
) -> Result<Value> {
    if request.entity_ids.is_empty() {
        return Err(CoreError::invalid(
            "neighbors/impact queries require at least one entity_id",
        ));
    }
    let edges = load_edges(store, request)?;
    let mut warnings = Vec::new();
    let (order, depths, visited, depth_cut, cap_cut) =
        breadth_first(&edges, request, reverse, &mut warnings);
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
    for edge in &edges {
        if page_ids.contains(&edge.source) && page_ids.contains(&edge.target) {
            relations.push(edge.payload.clone());
        }
    }
    let evidence = load_evidence_for(store, &request.project_id, &request.snapshot_id, &relations)?;
    let has_more = end < entities.len();
    let truncated = has_more || depth_cut || cap_cut;
    if has_more {
        warnings.push("result paged by max_nodes; next_cursor continues this query".to_string());
    }
    if cap_cut {
        warnings.push(format!(
            "traversal capped at {HARD_NODE_CAP} nodes; result is partial"
        ));
    }
    if depth_cut {
        warnings.push("neighbors remain beyond max_depth; result is partial".to_string());
    }
    let _ = &visited;
    let _ = &depths;
    let coverage = coverage(
        &request.snapshot_id,
        format!("query:{}", if reverse { "impact" } else { "neighbors" }),
        total,
        page.len() as u64,
        truncated,
        &mut warnings,
    );
    let next_cursor = if has_more {
        Some(encode_cursor(&CursorPayload {
            version: 1,
            snapshot_id: request.snapshot_id.clone(),
            query_digest: query_digest.to_string(),
            offset: end as u64,
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
    let edges = load_edges(store, request)?;
    let mut forward: HashMap<&str, Vec<&EdgeRow>> = HashMap::new();
    let mut reverse: HashMap<&str, Vec<&EdgeRow>> = HashMap::new();
    for edge in &edges {
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
    let mut truncated = false;
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
            truncated = true;
        }
    }
    let evidence = load_evidence_for(store, &request.project_id, &request.snapshot_id, &relations)?;
    let total = if found { entities.len() as u64 } else { 0 };
    let coverage = coverage(
        &request.snapshot_id,
        "query:path".to_string(),
        total,
        entities.len() as u64,
        truncated,
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

fn load_edges(store: &Store, request: &QueryRequest) -> Result<Vec<EdgeRow>> {
    let kinds: Vec<String> = request
        .relation_kinds
        .iter()
        .map(|kind| kind.as_str().to_string())
        .collect();
    let (sql, params): (String, Vec<String>) = if kinds.is_empty() {
        (
            "SELECT relation_id, source_id, target_id, kind, payload_json FROM relations
             WHERE project_id=?1 AND snapshot_id=?2 ORDER BY relation_id LIMIT 200000"
                .to_string(),
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
                 ORDER BY relation_id LIMIT 200000",
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
    for row in rows {
        let (relation_id, source, target, kind, payload_json) = row?;
        let payload: Value = serde_json::from_str(&payload_json)
            .map_err(|err| CoreError::Storage(format!("relation payload corrupt: {err}")))?;
        edges.push(EdgeRow {
            relation_id,
            source,
            target,
            kind,
            payload,
        });
        if edges.len() > HARD_EDGE_CAP {
            return Err(CoreError::invalid(
                "resource_exhausted: relation scan exceeded the hard cap; narrow relation_kinds",
            ));
        }
    }
    Ok(edges)
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
    expected: u64,
    observed: u64,
    truncated: bool,
    warnings: &mut Vec<String>,
) -> Coverage {
    let state = if truncated {
        CoverageState::Partial
    } else {
        CoverageState::Complete
    };
    if truncated {
        warnings.push("coverage is partial: the requested budget was reached".to_string());
    }
    Coverage {
        state,
        scope: format!("{scope} @ {snapshot_id}"),
        expected_count: Some(expected),
        observed_count: observed,
        omitted_count: 0,
        unresolved_count: 0,
        limitations: if truncated {
            vec!["budget or depth limit reached".to_string()]
        } else {
            vec![]
        },
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
) -> Value {
    json!({
        "snapshot_id": snapshot_id,
        "entities": entities,
        "relations": relations,
        "evidence": evidence,
        "coverage": coverage.to_json(),
        "warnings": warnings,
        "next_cursor": next_cursor,
    })
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
