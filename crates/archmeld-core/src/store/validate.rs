// Copyright 2026 ARCHMELD contributors
// SPDX-License-Identifier: Apache-2.0

//! Structural and semantic validation of an `EvidenceBatch`.
//!
//! Schema validity is necessary but not sufficient (S19 §3). This module also
//! enforces identity/scope consistency, anchor correspondence with stored
//! blobs, evidence-class rules for call relations, and the batch size limits.

use super::types::*;
use crate::error::{CoreError, Result};
use crate::hash;
use crate::store::{BlobInfo, Store};
use serde_json::Value;

pub(crate) const MAX_ENTITIES_PER_BATCH: usize = 500;
pub(crate) const MAX_RELATIONS_PER_BATCH: usize = 2000;
pub(crate) const MAX_EVIDENCE_PER_BATCH: usize = 2000;
pub(crate) const MAX_DIAGNOSTICS_PER_BATCH: usize = 2000;

#[derive(Debug, Clone)]
pub(crate) struct BatchItem<T> {
    pub typed: T,
    pub payload_digest: String,
    pub payload_json: String,
}

#[derive(Debug, Clone)]
pub(crate) struct ValidatedBatch {
    pub header: EvidenceBatch,
    pub sequence: u64,
    pub payload_digest: String,
    pub entities: Vec<BatchItem<Entity>>,
    pub evidence: Vec<BatchItem<Evidence>>,
    pub relations: Vec<BatchItem<Relation>>,
    pub diagnostics: Vec<Diagnostic>,
}

pub(crate) fn validate_batch(store: &Store, raw: &Value) -> Result<ValidatedBatch> {
    let header: EvidenceBatch = serde_json::from_value(raw.clone())
        .map_err(|err| CoreError::invalid(format!("batch does not match the EvidenceBatch contract: {err}")))?;

    if header.schema_version != "1.0" {
        return Err(CoreError::invalid(format!(
            "unsupported schema_version: {}",
            header.schema_version
        )));
    }
    require_id("project_id", &header.project_id)?;
    require_digest("snapshot_id", &header.snapshot_id)?;
    require_digest("config_digest", &header.config_digest)?;
    require_id("run_id", &header.run_id)?;
    require_non_empty("adapter_id", &header.adapter_id)?;
    require_non_empty("adapter_version", &header.adapter_version)?;
    let sequence = parse_sequence(&header.sequence)?;
    validate_coverage_shape("coverage", &header.coverage)?;

    if header.entities.len() > MAX_ENTITIES_PER_BATCH {
        return Err(CoreError::invalid(format!(
            "resource_exhausted: {} entities exceed the batch limit of {MAX_ENTITIES_PER_BATCH}",
            header.entities.len()
        )));
    }
    if header.relations.len() > MAX_RELATIONS_PER_BATCH {
        return Err(CoreError::invalid(format!(
            "resource_exhausted: {} relations exceed the batch limit of {MAX_RELATIONS_PER_BATCH}",
            header.relations.len()
        )));
    }
    if header.evidence.len() > MAX_EVIDENCE_PER_BATCH {
        return Err(CoreError::invalid(format!(
            "resource_exhausted: {} evidence records exceed the batch limit of {MAX_EVIDENCE_PER_BATCH}",
            header.evidence.len()
        )));
    }
    if header.diagnostics.len() > MAX_DIAGNOSTICS_PER_BATCH {
        return Err(CoreError::invalid(format!(
            "resource_exhausted: {} diagnostics exceed the batch limit of {MAX_DIAGNOSTICS_PER_BATCH}",
            header.diagnostics.len()
        )));
    }

    let payload_digest = hash::digest_value(raw)
        .map_err(|err| CoreError::invalid(format!("batch is outside the hash domain: {err}")))?;

    let mut entities: Vec<BatchItem<Entity>> = Vec::with_capacity(header.entities.len());
    let mut seen_entities: std::collections::HashMap<String, usize> = std::collections::HashMap::new();
    for (index, entity) in header.entities.iter().enumerate() {
        let item = item(store, &raw["entities"][index], entity.clone())?;
        if let Some(previous) = seen_entities.get(&entity.id) {
            if entities[*previous].payload_digest != item.payload_digest {
                return Err(CoreError::conflict(format!(
                    "entity {} appears twice in one batch with different payloads",
                    entity.id
                )));
            }
            continue;
        }
        validate_entity(store, &header, entity)?;
        seen_entities.insert(entity.id.clone(), entities.len());
        entities.push(item);
    }

    let mut evidence: Vec<BatchItem<Evidence>> = Vec::with_capacity(header.evidence.len());
    let mut seen_evidence: std::collections::HashMap<String, usize> = std::collections::HashMap::new();
    for (index, item_value) in header.evidence.iter().enumerate() {
        let item = item(store, &raw["evidence"][index], item_value.clone())?;
        if let Some(previous) = seen_evidence.get(&item_value.id) {
            if evidence[*previous].payload_digest != item.payload_digest {
                return Err(CoreError::conflict(format!(
                    "evidence {} appears twice in one batch with different payloads",
                    item_value.id
                )));
            }
            continue;
        }
        validate_evidence(store, &header, item_value)?;
        seen_evidence.insert(item_value.id.clone(), evidence.len());
        evidence.push(item);
    }

    let mut relations: Vec<BatchItem<Relation>> = Vec::with_capacity(header.relations.len());
    let mut seen_relations: std::collections::HashMap<String, usize> = std::collections::HashMap::new();
    for (index, relation) in header.relations.iter().enumerate() {
        let item = item(store, &raw["relations"][index], relation.clone())?;
        if let Some(previous) = seen_relations.get(&relation.id) {
            if relations[*previous].payload_digest != item.payload_digest {
                return Err(CoreError::conflict(format!(
                    "relation {} appears twice in one batch with different payloads",
                    relation.id
                )));
            }
            continue;
        }
        validate_relation(&header, relation)?;
        seen_relations.insert(relation.id.clone(), relations.len());
        relations.push(item);
    }

    let mut diagnostics = Vec::with_capacity(header.diagnostics.len());
    for diagnostic in &header.diagnostics {
        validate_diagnostic(diagnostic)?;
        diagnostics.push(diagnostic.clone());
    }

    Ok(ValidatedBatch {
        header,
        sequence,
        payload_digest,
        entities,
        evidence,
        relations,
        diagnostics,
    })
}

fn item<T: serde::Serialize>(
    _store: &Store,
    raw: &Value,
    typed: T,
) -> Result<BatchItem<T>> {
    let payload_digest = hash::digest_value(raw)
        .map_err(|err| CoreError::invalid(format!("batch item is outside the hash domain: {err}")))?;
    let payload_json = hash::canonical_json(raw)
        .map_err(|err| CoreError::invalid(format!("batch item is outside the hash domain: {err}")))?;
    Ok(BatchItem {
        typed,
        payload_digest,
        payload_json,
    })
}

fn validate_entity(store: &Store, header: &EvidenceBatch, entity: &Entity) -> Result<()> {
    require_id("entity.id", &entity.id)?;
    require_scope("entity", &entity.id, header, &entity.project_id, &entity.snapshot_id, &entity.config_digest)?;
    require_non_empty("entity.language", &entity.language)?;
    require_non_empty("entity.qualified_name", &entity.qualified_name)?;
    if !entity.external && entity.anchors.is_empty() {
        return Err(CoreError::invalid(format!(
            "entity {} is not external but has no source anchor",
            entity.id
        )));
    }
    for anchor in &entity.anchors {
        validate_anchor(store, "entity.anchors", anchor)?;
    }
    for backend in &entity.backend_refs {
        require_non_empty("backend_ref.adapter_id", &backend.adapter_id)?;
        require_id("backend_ref.run_id", &backend.run_id)?;
        require_non_empty("backend_ref.backend_id", &backend.backend_id)?;
        if backend.adapter_id != header.adapter_id {
            return Err(CoreError::invalid(format!(
                "entity {} has a backend ref for adapter {} but the batch adapter is {}",
                entity.id, backend.adapter_id, header.adapter_id
            )));
        }
        if backend.run_id != header.run_id {
            return Err(CoreError::invalid(format!(
                "entity {} has a backend ref for run {} but the batch run is {}",
                entity.id, backend.run_id, header.run_id
            )));
        }
    }
    Ok(())
}

fn validate_evidence(store: &Store, header: &EvidenceBatch, evidence: &Evidence) -> Result<()> {
    require_id("evidence.id", &evidence.id)?;
    require_scope(
        "evidence",
        &evidence.id,
        header,
        &evidence.project_id,
        &evidence.snapshot_id,
        &evidence.config_digest,
    )?;
    require_non_empty("evidence.producer.name", &evidence.producer.name)?;
    require_non_empty("evidence.producer.version", &evidence.producer.version)?;
    require_non_empty("evidence.producer.source_ref", &evidence.producer.source_ref)?;
    require_id("evidence.producer.run_id", &evidence.producer.run_id)?;
    require_non_empty("evidence.locator", &evidence.locator)?;
    require_digest("evidence.artifact_digest", &evidence.artifact_digest)?;
    match store.blob_info(&evidence.artifact_digest)? {
        Some(_) => {}
        None => {
            return Err(CoreError::invalid(format!(
                "evidence {} references artifact {} that is not in the object store",
                evidence.id, evidence.artifact_digest
            )))
        }
    }
    if let Some(anchor) = &evidence.source_anchor {
        validate_anchor(store, "evidence.source_anchor", anchor)?;
    }
    if evidence.kind == EvidenceKind::Runtime
        && evidence.runtime_run_id.is_none()
        && evidence.scenario_id.is_none()
    {
        return Err(CoreError::invalid(format!(
            "runtime evidence {} must bind a scenario or runtime run",
            evidence.id
        )));
    }
    validate_coverage_shape("evidence.coverage", &evidence.coverage)?;
    Ok(())
}

fn validate_relation(header: &EvidenceBatch, relation: &Relation) -> Result<()> {
    require_id("relation.id", &relation.id)?;
    require_scope(
        "relation",
        &relation.id,
        header,
        &relation.project_id,
        &relation.snapshot_id,
        &relation.config_digest,
    )?;
    require_id("relation.source_id", &relation.source_id)?;
    require_id("relation.target_id", &relation.target_id)?;
    if relation.evidence_ids.is_empty() {
        return Err(CoreError::invalid(format!(
            "relation {} has no evidence",
            relation.id
        )));
    }
    for evidence_id in &relation.evidence_ids {
        require_id("relation.evidence_ids[]", evidence_id)?;
    }
    for condition in &relation.conditions {
        require_non_empty("relation.conditions[]", condition)?;
    }

    match relation.kind {
        RelationKind::ResolvedCall => {
            if relation.basis != Basis::Semantic || relation.resolution != Resolution::Resolved {
                return Err(CoreError::invalid(format!(
                    "relation {}: resolved_call requires basis=semantic and resolution=resolved, got basis={} resolution={:?}",
                    relation.id,
                    relation.basis.as_str(),
                    relation.resolution
                )));
            }
        }
        RelationKind::ObservedCall => {
            if relation.basis != Basis::Runtime || relation.resolution != Resolution::Observed {
                return Err(CoreError::invalid(format!(
                    "relation {}: observed_call requires basis=runtime and resolution=observed, got basis={} resolution={:?}",
                    relation.id,
                    relation.basis.as_str(),
                    relation.resolution
                )));
            }
        }
        RelationKind::StaticCallCandidate => {
            if relation.resolution != Resolution::Candidate {
                return Err(CoreError::invalid(format!(
                    "relation {}: static_call_candidate must stay resolution=candidate",
                    relation.id
                )));
            }
            if matches!(relation.basis, Basis::Model | Basis::Runtime) {
                return Err(CoreError::invalid(format!(
                    "relation {}: static_call_candidate cannot be built on {} evidence",
                    relation.id,
                    relation.basis.as_str()
                )));
            }
        }
        _ => {}
    }

    if relation.basis == Basis::Model
        && matches!(
            relation.kind,
            RelationKind::ResolvedCall | RelationKind::ObservedCall
        )
    {
        return Err(CoreError::invalid(format!(
            "relation {}: model evidence cannot support {:?}",
            relation.id,
            relation.kind
        )));
    }
    Ok(())
}

fn validate_diagnostic(diagnostic: &Diagnostic) -> Result<()> {
    require_non_empty("diagnostic.code", &diagnostic.code)?;
    require_non_empty("diagnostic.message", &diagnostic.message)?;
    if let Some(entity_id) = &diagnostic.entity_id {
        require_id("diagnostic.entity_id", entity_id)?;
    }
    if let Some(path) = &diagnostic.path {
        validate_path("diagnostic.path", path)?;
    }
    Ok(())
}

pub(crate) fn validate_anchor(store: &Store, field: &str, anchor: &Anchor) -> Result<()> {
    validate_path(field, &anchor.path)?;
    require_digest(field, &anchor.blob_digest)?;
    if anchor.encoding != "utf-8" {
        return Err(CoreError::unsupported(format!(
            "{field}: unsupported encoding {} (only utf-8 anchors are accepted in v1)",
            anchor.encoding
        )));
    }
    match anchor.precision {
        Precision::Bytes => {
            let (Some(start), Some(end)) = (anchor.byte_start, anchor.byte_end) else {
                return Err(CoreError::invalid(format!(
                    "{field}: byte precision requires byte_start and byte_end"
                )));
            };
            if start > end {
                return Err(CoreError::invalid(format!(
                    "{field}: byte_start {start} is after byte_end {end}"
                )));
            }
            let info = store.blob_info(&anchor.blob_digest)?.ok_or_else(|| {
                CoreError::invalid(format!(
                    "{field}: blob {} is not in the object store",
                    anchor.blob_digest
                ))
            })?;
            validate_anchor_bounds(field, store, &info, start, end)?;
        }
        Precision::LineOnly | Precision::Candidate => {
            if anchor.byte_start.is_some() || anchor.byte_end.is_some() {
                return Err(CoreError::invalid(format!(
                    "{field}: {precision:?} precision must not claim byte offsets",
                    precision = anchor.precision
                )));
            }
        }
    }
    Ok(())
}

fn validate_anchor_bounds(
    field: &str,
    store: &Store,
    info: &BlobInfo,
    start: u64,
    end: u64,
) -> Result<()> {
    if !info.is_utf8 {
        return Err(CoreError::unsupported(format!(
            "{field}: blob is not valid UTF-8; byte anchors are unsupported"
        )));
    }
    if end > info.size {
        return Err(CoreError::invalid(format!(
            "{field}: byte_end {end} exceeds blob size {}",
            info.size
        )));
    }
    for offset in [start, end] {
        if !store.blob_is_char_boundary(info, offset)? {
            return Err(CoreError::invalid(format!(
                "{field}: byte offset {offset} is not a UTF-8 character boundary"
            )));
        }
    }
    Ok(())
}

fn require_scope(
    kind: &str,
    id: &str,
    header: &EvidenceBatch,
    project_id: &str,
    snapshot_id: &str,
    config_digest: &str,
) -> Result<()> {
    if project_id != header.project_id {
        return Err(CoreError::invalid(format!(
            "{kind} {id} belongs to project {project_id}, batch is {}",
            header.project_id
        )));
    }
    if snapshot_id != header.snapshot_id {
        return Err(CoreError::invalid(format!(
            "{kind} {id} belongs to snapshot {snapshot_id}, batch is {}",
            header.snapshot_id
        )));
    }
    if config_digest != header.config_digest {
        return Err(CoreError::invalid(format!(
            "{kind} {id} was produced under config {config_digest}, batch is {}",
            header.config_digest
        )));
    }
    Ok(())
}

pub(crate) fn validate_path(field: &str, path: &str) -> Result<()> {
    if path.is_empty() || path.len() > 4096 {
        return Err(CoreError::invalid(format!(
            "{field}: path length must be 1..=4096"
        )));
    }
    if path.starts_with('/') || path.contains('\\') || path.contains(':') || path.contains('\0') {
        return Err(CoreError::invalid(format!(
            "{field}: path {path:?} must be a relative POSIX path"
        )));
    }
    if path.split('/').any(|segment| segment == "..") {
        return Err(CoreError::invalid(format!(
            "{field}: path {path:?} must not contain '..' segments"
        )));
    }
    if path.split('/').any(|segment| segment.is_empty()) {
        return Err(CoreError::invalid(format!(
            "{field}: path {path:?} must not contain empty segments"
        )));
    }
    Ok(())
}

pub(crate) fn require_id(field: &str, value: &str) -> Result<()> {
    let bytes = value.as_bytes();
    let ok = (3..=160).contains(&bytes.len())
        && bytes[0].is_ascii_alphabetic()
        && bytes
            .iter()
            .all(|b| b.is_ascii_alphanumeric() || matches!(*b, b'_' | b'.' | b':' | b'-'));
    if !ok {
        return Err(CoreError::invalid(format!(
            "{field}: {value:?} is not a valid Id"
        )));
    }
    Ok(())
}

pub(crate) fn require_digest(field: &str, value: &str) -> Result<()> {
    let ok = value.len() == 71
        && value.starts_with("sha256:")
        && value[7..]
            .bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b));
    if !ok {
        return Err(CoreError::invalid(format!(
            "{field}: {value:?} is not a lowercase sha256 digest"
        )));
    }
    Ok(())
}

pub(crate) fn require_non_empty(field: &str, value: &str) -> Result<()> {
    if value.is_empty() {
        return Err(CoreError::invalid(format!("{field} must not be empty")));
    }
    Ok(())
}

pub(crate) fn parse_sequence(value: &str) -> Result<u64> {
    if value.is_empty()
        || !value.bytes().all(|b| b.is_ascii_digit())
        || (value.len() > 1 && value.starts_with('0'))
    {
        return Err(CoreError::invalid(format!(
            "sequence {value:?} is not a non-negative DecimalInteger without leading zeros"
        )));
    }
    value
        .parse::<u64>()
        .map_err(|_| CoreError::invalid(format!("sequence {value:?} is out of range")))
}

pub(crate) fn validate_coverage_shape(field: &str, coverage: &Coverage) -> Result<()> {
    require_non_empty(&format!("{field}.scope"), &coverage.scope)?;
    if coverage.state == CoverageState::Complete && !coverage.is_well_formed_complete() {
        return Err(CoreError::invalid(format!(
            "{field}: state=complete requires a known expected_count equal to observed_count, omitted_count=0 and unresolved_count=0"
        )));
    }
    Ok(())
}
