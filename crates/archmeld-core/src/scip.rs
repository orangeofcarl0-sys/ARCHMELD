// Copyright 2026 ARCHMELD contributors
// SPDX-License-Identifier: Apache-2.0

//! SCIP semantic import (S03, S05).
//!
//! Reads the official SCIP protobuf wire format (`third_party/scip/scip.proto`)
//! without protoc. Definitions become entities with byte/line anchors; a
//! symbol occurrence becomes a semantic `references` relation. A plain SCIP
//! reference is never promoted to `resolved_call` — call semantics need a
//! callsite rule that this adapter does not claim. Symbols not defined in the
//! index are external entities with no source anchor.

use crate::error::{CoreError, Result};
use crate::hash;
use crate::identity;
use crate::source::LineIndex;
use crate::store::{validate, Coverage, CoverageState, FileEntry, Store};
use serde_json::{json, Value};
use std::collections::HashMap;

pub const PROVIDER: &str = "scip";
pub const FORMAT: &str = "scip-protobuf-v0.3";

const ROLE_DEFINITION: u32 = 0x1;
const ENC_UTF8: i32 = 1;
const ENC_UTF16: i32 = 2;
const ENC_UTF32: i32 = 3;

pub struct ScipConfig<'a> {
    pub project_id: &'a str,
    pub snapshot_id: &'a str,
    pub config_digest: &'a str,
    pub run_id: &'a str,
    pub max_entities_per_batch: usize,
    pub max_relations_per_batch: usize,
}

#[derive(Debug, Clone)]
pub struct ScipReport {
    pub batches: u64,
    pub documents: u64,
    pub entities: u64,
    pub relations: u64,
    pub external_entities: u64,
    pub diagnostics: u64,
    pub omitted_documents: u64,
    pub unresolved: u64,
    pub coverage: Value,
}

struct ScipIndex {
    tool_name: String,
    tool_version: String,
    encoding: i32,
    documents: Vec<ScipDocument>,
    external_kinds: HashMap<String, i64>,
}

struct ScipDocument {
    path: String,
    language: String,
    encoding: i32,
    occurrences: Vec<ScipOccurrence>,
    kinds: HashMap<String, i64>,
}

#[derive(Clone)]
struct ScipOccurrence {
    range: Option<(u32, u32, u32, u32)>,
    symbol: String,
    roles: u32,
}

/// Parse and import a SCIP index. The raw index is stored as the semantic
/// evidence artifact and never rewritten.
pub fn import_scip(
    store: &mut Store,
    config: &ScipConfig<'_>,
    files: &[FileEntry],
    index_bytes: &[u8],
) -> Result<ScipReport> {
    validate::require_id("project_id", config.project_id)?;
    validate::require_digest("snapshot_id", config.snapshot_id)?;
    validate::require_digest("config_digest", config.config_digest)?;
    validate::require_id("run_id", config.run_id)?;
    store.check_snapshot_scope(config.project_id, config.snapshot_id)?;
    let index = parse_index(index_bytes)?;
    let artifact = store.put_blob(index_bytes, "application/x-scip")?;
    let evidence_id = format!("evidence.scip.{}", config.run_id);

    let file_by_path: HashMap<&str, &FileEntry> =
        files.iter().map(|file| (file.path.as_str(), file)).collect();
    let mut diagnostics: Vec<Value> = Vec::new();
    let mut entities: Vec<Value> = Vec::new();
    let mut relations: Vec<Value> = Vec::new();
    let mut entity_ids: HashMap<String, String> = HashMap::new();
    let mut external_entities = 0u64;
    let mut omitted_documents = 0u64;
    let mut unresolved = 0u64;
    let mut range_fallbacks = 0u64;

    for document in &index.documents {
        // rust-analyzer on Windows emits backslash paths. SCIP requires POSIX
        // separators; normalize before validating so absolute and '..' paths
        // are still rejected.
        let raw_path = &document.path;
        let path = raw_path.replace('\\', "/");
        validate::validate_path("scip.document", &path).map_err(|err| {
            CoreError::invalid(format!("SCIP document path {}: {err}", path))
        })?;
        let Some(file) = file_by_path.get(path.as_str()) else {
            omitted_documents += 1;
            diagnostics.push(diagnostic(
                "scip_document_not_in_snapshot",
                "warning",
                &format!(
                    "SCIP document {} is not part of snapshot {}",
                    path, config.snapshot_id
                ),
            ));
            continue;
        };
        let blob = store
            .read_blob(&file.blob_digest)?
            .ok_or_else(|| CoreError::Storage(format!("blob {} missing", file.blob_digest)))?;
        let text = std::str::from_utf8(&blob).ok();
        let index_lines = LineIndex::new(&blob);
        let encoding = if document.encoding != 0 {
            document.encoding
        } else {
            index.encoding
        };

        // One file entity hosts file-scope references when no enclosing
        // definition can be identified.
        let file_entity_id = identity::semantic_entity_id(
            PROVIDER,
            &format!("file:{}", path),
            config.config_digest,
            "file",
        )?;
        if !entity_ids.contains_key(&format!("file:{}", path)) {
            entity_ids.insert(format!("file:{}", path), file_entity_id.clone());
            entities.push(json!({
                "id": file_entity_id,
                "project_id": config.project_id,
                "snapshot_id": config.snapshot_id,
                "config_digest": config.config_digest,
                "kind": "file",
                "language": language_or_unknown(&document.language),
                "qualified_name": path,
                "anchors": [line_anchor(&path, &file.blob_digest, 1, index_lines.line_count())],
                "external": false,
                "backend_refs": [],
            }));
        }

        // Definition occurrences first so references can resolve locally.
        let mut definition_ranges: Vec<(u32, u32, u32, u32, String)> = Vec::new();
        for occurrence in &document.occurrences {
            if occurrence.roles & ROLE_DEFINITION == 0 || occurrence.symbol.is_empty() {
                if occurrence.roles & ROLE_DEFINITION != 0 {
                    unresolved += 1;
                }
                continue;
            }
            let Some(range) = occurrence.range else {
                unresolved += 1;
                diagnostics.push(diagnostic(
                    "scip_definition_without_range",
                    "warning",
                    &format!("definition {} in {} has no range", occurrence.symbol, path),
                ));
                continue;
            };
            if let Some(existing) = entity_ids.get(&occurrence.symbol) {
                if existing != &entity_id_for(config, &occurrence.symbol, &path, "definition")? {
                    diagnostics.push(diagnostic(
                        "scip_duplicate_definition",
                        "info",
                        &format!("symbol {} is defined more than once", occurrence.symbol),
                    ));
                }
                continue;
            }
            let kind_code = document.kinds.get(&occurrence.symbol).copied();
            let kind = kind_for(kind_code, &occurrence.symbol);
            let (anchor, fallback) = anchor_for(
                &path,
                &file.blob_digest,
                range,
                encoding,
                text,
                &index_lines,
            );
            if fallback {
                range_fallbacks += 1;
            }
            let entity_id = identity::semantic_entity_id(
                PROVIDER,
                &occurrence.symbol,
                config.config_digest,
                &format!("definition:{}", path),
            )?;
            entity_ids.insert(occurrence.symbol.clone(), entity_id.clone());
            definition_ranges.push((range.0, range.1, range.2, range.3, entity_id.clone()));
            entities.push(json!({
                "id": entity_id,
                "project_id": config.project_id,
                "snapshot_id": config.snapshot_id,
                "config_digest": config.config_digest,
                "kind": kind,
                "language": language_or_unknown(&document.language),
                "qualified_name": occurrence.symbol,
                "anchors": [anchor],
                "external": false,
                "backend_refs": [],
            }));
        }

        for occurrence in &document.occurrences {
            if occurrence.roles & ROLE_DEFINITION != 0 {
                continue;
            }
            if occurrence.symbol.is_empty() {
                unresolved += 1;
                continue;
            }
            let (target_id, target_external) = match entity_ids.get(&occurrence.symbol) {
                Some(id) => (id.clone(), false),
                None => {
                    let external_kind = index
                        .external_kinds
                        .get(&occurrence.symbol)
                        .copied();
                    let external_id = identity::semantic_entity_id(
                        PROVIDER,
                        &occurrence.symbol,
                        config.config_digest,
                        "external",
                    )?;
                    if !entity_ids.contains_key(&occurrence.symbol) {
                        entity_ids.insert(occurrence.symbol.clone(), external_id.clone());
                        external_entities += 1;
                        entities.push(json!({
                            "id": external_id,
                            "project_id": config.project_id,
                            "snapshot_id": config.snapshot_id,
                            "config_digest": config.config_digest,
                            "kind": kind_for(external_kind, &occurrence.symbol),
                            "language": "unknown",
                            "qualified_name": occurrence.symbol,
                            "anchors": [],
                            "external": true,
                            "backend_refs": [],
                        }));
                    }
                    (external_id, true)
                }
            };
            let Some(range) = occurrence.range else {
                unresolved += 1;
                continue;
            };
            let source_id = enclosing_definition(&definition_ranges, range)
                .unwrap_or_else(|| file_entity_id.clone());
            let conditions = if target_external {
                vec![
                    "scip:reference".to_string(),
                    "resolved symbol reference; call semantics not asserted".to_string(),
                    "external symbol: no local source anchor".to_string(),
                ]
            } else {
                vec![
                    "scip:reference".to_string(),
                    "resolved symbol reference; call semantics not asserted".to_string(),
                ]
            };
            let relation_identity = json!({
                "version": "1.0",
                "provider": PROVIDER,
                "source": source_id,
                "target": target_id,
                "kind": "references",
                "path": path,
                "range": [range.0, range.1, range.2, range.3],
            });
            let digest = hash::digest_value(&relation_identity)
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
                "kind": "references",
                "basis": "semantic",
                "evidence_ids": [evidence_id],
                "conditions": conditions,
                "resolution": "resolved",
            }));
        }
    }

    let completeness = omitted_documents == 0 && unresolved == 0;
    let mut limitations = vec![
        "SCIP symbol references are resolved; call semantics are not asserted".to_string(),
        "external symbols have no source anchor".to_string(),
    ];
    if omitted_documents > 0 {
        limitations.push(format!(
            "{omitted_documents} document(s) were not part of the snapshot"
        ));
    }
    if unresolved > 0 {
        limitations.push(format!("{unresolved} occurrence(s) lacked a range or symbol"));
    }
    if range_fallbacks > 0 {
        limitations.push(format!(
            "{range_fallbacks} occurrence(s) fell back to line-only anchors"
        ));
    }
    let coverage = Coverage {
        state: if completeness {
            CoverageState::Complete
        } else {
            CoverageState::Partial
        },
        scope: "SCIP index over the declared snapshot".to_string(),
        expected_count: Some(index.documents.len() as u64),
        observed_count: index.documents.len().saturating_sub(omitted_documents as usize) as u64,
        omitted_count: omitted_documents,
        unresolved_count: unresolved,
        limitations: limitations.clone(),
    };
    let evidence = json!({
        "id": evidence_id,
        "project_id": config.project_id,
        "snapshot_id": config.snapshot_id,
        "config_digest": config.config_digest,
        "kind": "semantic",
        "producer": {
            "name": format!("scip:{}", index.tool_name),
            "version": index.tool_version,
            "source_ref": FORMAT,
            "run_id": config.run_id,
        },
        "artifact_digest": artifact,
        "locator": FORMAT,
        "source_anchor": null,
        "scenario_id": null,
        "runtime_run_id": null,
        "coverage": coverage.to_json(),
        "freshness": "current",
    });

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
    let tool_version = index.tool_version.clone();
    for batch_index in 0..batches {
        let entity_slice = entity_chunks.get(batch_index).copied().unwrap_or(&[]);
        let relation_slice = relation_chunks.get(batch_index).copied().unwrap_or(&[]);
        let is_final = batch_index + 1 == batches;
        let evidence_slice: &[Value] = if batch_index == 0 {
            std::slice::from_ref(&evidence)
        } else {
            &[]
        };
        let diagnostic_slice: &[Value] = if is_final { &diagnostics } else { &[] };
        let batch = json!({
            "schema_version": "1.0",
            "project_id": config.project_id,
            "snapshot_id": config.snapshot_id,
            "config_digest": config.config_digest,
            "run_id": config.run_id,
            "adapter_id": PROVIDER,
            "adapter_version": &tool_version,
            "sequence": batch_index.to_string(),
            "final": is_final,
            "coverage": coverage.to_json(),
            "entities": entity_slice,
            "evidence": evidence_slice,
            "relations": relation_slice,
            "diagnostics": diagnostic_slice,
        });
        store.import_batch(&batch)?;
    }

    Ok(ScipReport {
        batches: batches as u64,
        documents: index.documents.len() as u64,
        entities: entities.len() as u64,
        relations: relations.len() as u64,
        external_entities,
        diagnostics: diagnostics.len() as u64,
        omitted_documents,
        unresolved,
        coverage: coverage.to_json(),
    })
}

fn entity_id_for(
    config: &ScipConfig<'_>,
    symbol: &str,
    path: &str,
    scope: &str,
) -> Result<String> {
    identity::semantic_entity_id(
        PROVIDER,
        symbol,
        config.config_digest,
        &format!("{scope}:{path}"),
    )
}

fn anchor_for(
    path: &str,
    blob: &str,
    range: (u32, u32, u32, u32),
    encoding: i32,
    text: Option<&str>,
    index: &LineIndex,
) -> (Value, bool) {
    let (sl, sc, el, ec) = range;
    let line_anchor = line_anchor(path, blob, sl as u64 + 1, el as u64 + 1);
    let conversion = text.and_then(|text| {
        let bytes = text.as_bytes();
        let start = match encoding {
            ENC_UTF8 => Some(index.line_start(sl as u64)?.checked_add(sc as u64)?),
            ENC_UTF16 => index.utf16_col_to_byte(bytes, sl as u64, sc as u64),
            ENC_UTF32 => index.utf32_col_to_byte(bytes, sl as u64, sc as u64),
            _ => None,
        }?;
        let end = match encoding {
            ENC_UTF8 => Some(index.line_start(el as u64)?.checked_add(ec as u64)?),
            ENC_UTF16 => index.utf16_col_to_byte(bytes, el as u64, ec as u64),
            ENC_UTF32 => index.utf32_col_to_byte(bytes, el as u64, ec as u64),
            _ => None,
        }?;
        if end <= bytes.len() as u64
            && start <= end
            && crate::source::is_char_boundary(bytes, start)
            && crate::source::is_char_boundary(bytes, end)
        {
            Some((start, end))
        } else {
            None
        }
    });
    match conversion {
        Some((start, end)) => (
            json!({
                "path": path,
                "blob_digest": blob,
                "precision": "bytes",
                "byte_start": start,
                "byte_end": end,
                "line_start": sl as u64 + 1,
                "line_end": el as u64 + 1,
                "encoding": "utf-8",
            }),
            false,
        ),
        None => (line_anchor, encoding != 0),
    }
}

fn line_anchor(path: &str, blob: &str, line_start: u64, line_end: u64) -> Value {
    json!({
        "path": path,
        "blob_digest": blob,
        "precision": "line_only",
        "byte_start": null,
        "byte_end": null,
        "line_start": line_start,
        "line_end": line_end,
        "encoding": "utf-8",
    })
}

fn enclosing_definition(
    definitions: &[(u32, u32, u32, u32, String)],
    range: (u32, u32, u32, u32),
) -> Option<String> {
    let (sl, sc, el, ec) = range;
    definitions
        .iter()
        .filter(|(dsl, dsc, del, dec, _)| {
            (*dsl, *dsc) <= (sl, sc)
                && (el, ec) <= (*del, *dec)
                && ((*dsl, *dsc) != (sl, sc) || (*del, *dec) != (el, ec))
        })
        .min_by_key(|(dsl, dsc, del, dec, _)| {
            (del.saturating_sub(*dsl), dec.saturating_sub(*dsc))
        })
        .map(|(_, _, _, _, id)| id.clone())
}

fn kind_for(code: Option<i64>, symbol: &str) -> &'static str {
    match code {
        Some(7) | Some(11) | Some(21) | Some(46) => "type",
        Some(29) | Some(30) | Some(35) | Some(36) => "module",
        Some(17) | Some(26) | Some(9) | Some(25) | Some(15) | Some(8) | Some(37) => "symbol",
        _ => {
            if symbol.ends_with('#') {
                "type"
            } else if symbol.ends_with('/') || symbol.ends_with("::") {
                "module"
            } else {
                "symbol"
            }
        }
    }
}

fn language_or_unknown(language: &str) -> String {
    if language.is_empty() {
        "unknown".to_string()
    } else {
        language.to_string()
    }
}

fn diagnostic(code: &str, severity: &str, message: &str) -> Value {
    json!({
        "code": code,
        "severity": severity,
        "message": message,
        "entity_id": null,
        "path": null,
    })
}

// ---- protobuf wire decoding (schema: third_party/scip/scip.proto) ----

struct Reader<'a> {
    data: &'a [u8],
    pos: usize,
}

impl<'a> Reader<'a> {
    fn new(data: &'a [u8]) -> Self {
        Self { data, pos: 0 }
    }

    fn eof(&self) -> bool {
        self.pos >= self.data.len()
    }

    fn varint(&mut self) -> Result<u64> {
        let mut value = 0u64;
        let mut shift = 0;
        loop {
            if self.pos >= self.data.len() || shift > 63 {
                return Err(CoreError::invalid("truncated SCIP varint"));
            }
            let byte = self.data[self.pos];
            self.pos += 1;
            value |= ((byte & 0x7f) as u64) << shift;
            if byte & 0x80 == 0 {
                return Ok(value);
            }
            shift += 7;
        }
    }

    fn tag(&mut self) -> Result<(u32, u8)> {
        let key = self.varint()?;
        Ok(((key >> 3) as u32, (key & 0x7) as u8))
    }

    fn length_delimited(&mut self) -> Result<&'a [u8]> {
        let length = self.varint()? as usize;
        if self.pos + length > self.data.len() {
            return Err(CoreError::invalid("truncated SCIP length-delimited field"));
        }
        let slice = &self.data[self.pos..self.pos + length];
        self.pos += length;
        Ok(slice)
    }

    fn skip(&mut self, wire: u8) -> Result<()> {
        match wire {
            0 => {
                self.varint()?;
            }
            1 => self.pos = self.pos.saturating_add(8),
            2 => {
                self.length_delimited()?;
            }
            5 => self.pos = self.pos.saturating_add(4),
            other => {
                return Err(CoreError::invalid(format!(
                    "unsupported protobuf wire type {other} in SCIP index"
                )))
            }
        }
        if self.pos > self.data.len() {
            return Err(CoreError::invalid("truncated SCIP field"));
        }
        Ok(())
    }
}

fn parse_index(bytes: &[u8]) -> Result<ScipIndex> {
    let mut index = ScipIndex {
        tool_name: "unknown".into(),
        tool_version: "unknown".into(),
        encoding: 0,
        documents: Vec::new(),
        external_kinds: HashMap::new(),
    };
    let mut reader = Reader::new(bytes);
    while !reader.eof() {
        let (field, wire) = reader.tag()?;
        match (field, wire) {
            (1, 2) => parse_metadata(reader.length_delimited()?, &mut index)?,
            (2, 2) => index
                .documents
                .push(parse_document(reader.length_delimited()?)?),
            (3, 2) => {
                let (symbol, kind) = parse_symbol_information(reader.length_delimited()?)?;
                if !symbol.is_empty() {
                    index.external_kinds.insert(symbol, kind);
                }
            }
            (_, wire) => reader.skip(wire)?,
        }
    }
    Ok(index)
}

fn parse_metadata(bytes: &[u8], index: &mut ScipIndex) -> Result<()> {
    let mut reader = Reader::new(bytes);
    while !reader.eof() {
        let (field, wire) = reader.tag()?;
        match (field, wire) {
            (2, 2) => {
                let (name, version) = parse_tool_info(reader.length_delimited()?)?;
                if !name.is_empty() {
                    index.tool_name = name;
                }
                if !version.is_empty() {
                    index.tool_version = version;
                }
            }
            (4, 0) => index.encoding = reader.varint()? as i32,
            (_, wire) => reader.skip(wire)?,
        }
    }
    Ok(())
}

fn parse_tool_info(bytes: &[u8]) -> Result<(String, String)> {
    let mut name = String::new();
    let mut version = String::new();
    let mut reader = Reader::new(bytes);
    while !reader.eof() {
        let (field, wire) = reader.tag()?;
        match (field, wire) {
            (1, 2) => name = str_field(reader.length_delimited()?)?,
            (2, 2) => version = str_field(reader.length_delimited()?)?,
            (_, wire) => reader.skip(wire)?,
        }
    }
    Ok((name, version))
}

fn parse_document(bytes: &[u8]) -> Result<ScipDocument> {
    let mut document = ScipDocument {
        path: String::new(),
        language: String::new(),
        encoding: 0,
        occurrences: Vec::new(),
        kinds: HashMap::new(),
    };
    let mut reader = Reader::new(bytes);
    while !reader.eof() {
        let (field, wire) = reader.tag()?;
        match (field, wire) {
            (1, 2) => document.path = str_field(reader.length_delimited()?)?,
            (2, 2) => document
                .occurrences
                .push(parse_occurrence(reader.length_delimited()?)?),
            (3, 2) => {
                let (symbol, kind) = parse_symbol_information(reader.length_delimited()?)?;
                if !symbol.is_empty() {
                    document.kinds.insert(symbol, kind);
                }
            }
            (4, 2) => document.language = str_field(reader.length_delimited()?)?,
            (6, 0) => document.encoding = reader.varint()? as i32,
            (_, wire) => reader.skip(wire)?,
        }
    }
    Ok(document)
}

fn parse_symbol_information(bytes: &[u8]) -> Result<(String, i64)> {
    let mut symbol = String::new();
    let mut kind = 0i64;
    let mut reader = Reader::new(bytes);
    while !reader.eof() {
        let (field, wire) = reader.tag()?;
        match (field, wire) {
            (1, 2) => symbol = str_field(reader.length_delimited()?)?,
            (5, 0) => kind = reader.varint()? as i64,
            (_, wire) => reader.skip(wire)?,
        }
    }
    Ok((symbol, kind))
}

fn parse_occurrence(bytes: &[u8]) -> Result<ScipOccurrence> {
    let mut occurrence = ScipOccurrence {
        range: None,
        symbol: String::new(),
        roles: 0,
    };
    let mut reader = Reader::new(bytes);
    while !reader.eof() {
        let (field, wire) = reader.tag()?;
        match (field, wire) {
            (1, 2) => occurrence.range = parse_packed_range(reader.length_delimited()?)?,
            (1, 0) => occurrence.range = Some((reader.varint()? as u32, 0, 0, 0)),
            (2, 2) => occurrence.symbol = str_field(reader.length_delimited()?)?,
            (3, 0) => occurrence.roles = reader.varint()? as u32,
            (8, 2) => {
                let (line, start, end) = parse_single_line_range(reader.length_delimited()?)?;
                occurrence.range = Some((line, start, line, end));
            }
            (9, 2) => occurrence.range = Some(parse_multi_line_range(reader.length_delimited()?)?),
            (_, wire) => reader.skip(wire)?,
        }
    }
    Ok(occurrence)
}

fn parse_packed_range(bytes: &[u8]) -> Result<Option<(u32, u32, u32, u32)>> {
    let mut reader = Reader::new(bytes);
    let mut values = Vec::new();
    while !reader.eof() {
        values.push(reader.varint()? as u32);
    }
    match values.len() {
        3 => Ok(Some((values[0], values[1], values[0], values[2]))),
        4 => Ok(Some((values[0], values[1], values[2], values[3]))),
        _ => Err(CoreError::invalid("SCIP range must have 3 or 4 elements")),
    }
}

fn parse_single_line_range(bytes: &[u8]) -> Result<(u32, u32, u32)> {
    let mut line = 0;
    let mut start = 0;
    let mut end = 0;
    let mut reader = Reader::new(bytes);
    while !reader.eof() {
        let (field, wire) = reader.tag()?;
        match (field, wire) {
            (1, 0) => line = reader.varint()? as u32,
            (2, 0) => start = reader.varint()? as u32,
            (3, 0) => end = reader.varint()? as u32,
            (_, wire) => reader.skip(wire)?,
        }
    }
    Ok((line, start, end))
}

fn parse_multi_line_range(bytes: &[u8]) -> Result<(u32, u32, u32, u32)> {
    let mut values = [0u32; 4];
    let mut reader = Reader::new(bytes);
    while !reader.eof() {
        let (field, wire) = reader.tag()?;
        match (field, wire) {
            (1, 0) => values[0] = reader.varint()? as u32,
            (2, 0) => values[1] = reader.varint()? as u32,
            (3, 0) => values[2] = reader.varint()? as u32,
            (4, 0) => values[3] = reader.varint()? as u32,
            (_, wire) => reader.skip(wire)?,
        }
    }
    Ok((values[0], values[1], values[2], values[3]))
}

fn str_field(bytes: &[u8]) -> Result<String> {
    std::str::from_utf8(bytes)
        .map(str::to_string)
        .map_err(|_| CoreError::invalid("SCIP string field is not UTF-8"))
}
