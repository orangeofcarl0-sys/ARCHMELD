// Copyright 2026 ARCHMELD contributors
// SPDX-License-Identifier: Apache-2.0

//! Atomic multi-batch evidence import.
//!
//! Every batch is validated before a transaction starts, then staged in
//! `staging_*` tables. The final batch performs cross-batch reference checks and
//! publishes the whole generation inside a single SQLite transaction. A crash
//! or injected fault rolls back both the staging rows and the generation row,
//! so no half graph and no dangling relation can be observed.

use super::types::*;
use super::validate::{parse_sequence, validate_batch, ValidatedBatch};
use super::{now_unix, Store};
use crate::error::{CoreError, Result};
use serde_json::Value;
use std::collections::{HashMap, HashSet};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FaultPoint {
    None,
    /// Roll back after staging but before the final publish checks.
    AfterStaging,
    /// Roll back immediately before commit, simulating process death.
    BeforeCommit,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ImportStatus {
    Applied,
    AlreadyApplied,
}

#[derive(Debug, Clone)]
pub struct ImportOutcome {
    pub sequence: u64,
    pub status: ImportStatus,
    pub published: bool,
    pub entities: u64,
    pub evidence: u64,
    pub relations: u64,
    pub diagnostics: u64,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct PublishedCounts {
    pub entities: u64,
    pub evidence: u64,
    pub relations: u64,
    pub diagnostics: u64,
    pub relation_evidence: u64,
    pub backend_mappings: u64,
}

impl Store {
    /// Import one NDJSON batch. Duplicate keys and non-canonical numbers are
    /// rejected before schema validation.
    pub fn import_batch_json(&mut self, text: &str) -> Result<ImportOutcome> {
        let raw = crate::hash::parse_strict(text)
            .map_err(|err| CoreError::invalid(format!("batch JSON rejected: {err}")))?;
        self.import_batch(&raw)
    }

    pub fn import_batch(&mut self, raw: &Value) -> Result<ImportOutcome> {
        self.import_batch_with_fault(raw, FaultPoint::None)
    }

    /// Fault-injection entry point used by crash/rollback tests and by the
    /// apply journal later. Faults are deterministic and roll back cleanly.
    pub fn import_batch_with_fault(
        &mut self,
        raw: &Value,
        fault: FaultPoint,
    ) -> Result<ImportOutcome> {
        let batch = validate_batch(self, raw)?;
        self.check_project_and_snapshot(&batch)?;

        let tx = self.conn.transaction()?;
        let idempotent = apply_batch_transaction(&tx, &batch, fault)?;
        if let Some(outcome) = idempotent {
            tx.commit()?;
            return Ok(outcome);
        }
        if fault == FaultPoint::AfterStaging {
            tx.rollback()?;
            return Err(CoreError::Injected(
                "fault injected after staging; transaction rolled back".to_string(),
            ));
        }
        if fault == FaultPoint::BeforeCommit {
            tx.rollback()?;
            return Err(CoreError::Injected(
                "fault injected before commit; transaction rolled back".to_string(),
            ));
        }
        tx.commit()?;
        Ok(ImportOutcome {
            sequence: batch.sequence,
            status: ImportStatus::Applied,
            published: batch.header.is_final,
            entities: batch.entities.len() as u64,
            evidence: batch.evidence.len() as u64,
            relations: batch.relations.len() as u64,
            diagnostics: batch.diagnostics.len() as u64,
        })
    }

    /// Mark a staged generation as failed and drop its staging rows. Published
    /// generations cannot be aborted.
    pub fn abort_generation(
        &mut self,
        project_id: &str,
        snapshot_id: &str,
        adapter_id: &str,
        run_id: &str,
    ) -> Result<()> {
        let tx = self.conn.transaction()?;
        let status: Option<String> = tx
            .query_row(
                "SELECT status FROM generations WHERE project_id=?1 AND snapshot_id=?2 AND adapter_id=?3 AND run_id=?4",
                (project_id, snapshot_id, adapter_id, run_id),
                |row| row.get(0),
            )
            .ok();
        match status.as_deref() {
            None => return Err(CoreError::not_found("generation does not exist")),
            Some("published") => {
                return Err(CoreError::conflict(
                    "published generation cannot be aborted",
                ))
            }
            Some(_) => {}
        }
        delete_staging(&tx, project_id, snapshot_id, adapter_id, run_id)?;
        tx.execute(
            "UPDATE generations SET status='failed', finished_at=?5
             WHERE project_id=?1 AND snapshot_id=?2 AND adapter_id=?3 AND run_id=?4",
            (project_id, snapshot_id, adapter_id, run_id, now_unix()),
        )?;
        tx.commit()?;
        Ok(())
    }

    pub fn generation_status(
        &self,
        project_id: &str,
        snapshot_id: &str,
        adapter_id: &str,
        run_id: &str,
    ) -> Result<Option<String>> {
        Ok(self
            .conn
            .query_row(
                "SELECT status FROM generations WHERE project_id=?1 AND snapshot_id=?2 AND adapter_id=?3 AND run_id=?4",
                (project_id, snapshot_id, adapter_id, run_id),
                |row| row.get(0),
            )
            .ok())
    }

    pub fn published_counts(&self, project_id: &str, snapshot_id: &str) -> Result<PublishedCounts> {
        fn count(conn: &rusqlite::Connection, table: &str, project: &str, snapshot: &str) -> Result<u64> {
            let sql = format!("SELECT COUNT(*) FROM {table} WHERE project_id=?1 AND snapshot_id=?2");
            let value: i64 = conn.query_row(&sql, (project, snapshot), |row| row.get(0))?;
            Ok(value.max(0) as u64)
        }
        Ok(PublishedCounts {
            entities: count(&self.conn, "entities", project_id, snapshot_id)?,
            evidence: count(&self.conn, "evidence", project_id, snapshot_id)?,
            relations: count(&self.conn, "relations", project_id, snapshot_id)?,
            diagnostics: count(&self.conn, "diagnostics", project_id, snapshot_id)?,
            relation_evidence: count(&self.conn, "relation_evidence", project_id, snapshot_id)?,
            backend_mappings: count(&self.conn, "backend_mappings", project_id, snapshot_id)?,
        })
    }

    /// `PRAGMA integrity_check` + `PRAGMA foreign_key_check`; both must be clean.
    pub fn integrity_check(&self) -> Result<Vec<String>> {
        let mut problems = Vec::new();
        let integrity: String = self
            .conn
            .query_row("PRAGMA integrity_check", [], |row| row.get(0))?;
        if integrity != "ok" {
            problems.push(format!("integrity_check: {integrity}"));
        }
        let mut statement = self.conn.prepare("PRAGMA foreign_key_check")?;
        let mut rows = statement.query([])?;
        while let Some(row) = rows.next()? {
            let table: String = row.get(0)?;
            let parent: String = row.get(2)?;
            problems.push(format!("foreign_key_check: {table} -> {parent}"));
        }
        Ok(problems)
    }

    fn check_project_and_snapshot(&self, batch: &ValidatedBatch) -> Result<()> {
        let header = &batch.header;
        let project_exists: bool = self
            .conn
            .query_row(
                "SELECT 1 FROM projects WHERE project_id=?1",
                (&header.project_id,),
                |_| Ok(true),
            )
            .unwrap_or(false);
        if !project_exists {
            return Err(CoreError::not_found(format!(
                "project {} is not registered",
                header.project_id
            )));
        }
        let snapshot: Option<(String, String)> = self
            .conn
            .query_row(
                "SELECT project_id, config_digest FROM snapshots WHERE snapshot_id=?1",
                (&header.snapshot_id,),
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .ok();
        let Some((snapshot_project, snapshot_config)) = snapshot else {
            return Err(CoreError::not_found(format!(
                "snapshot {} is not registered",
                header.snapshot_id
            )));
        };
        if snapshot_project != header.project_id {
            return Err(CoreError::invalid(format!(
                "snapshot {} belongs to project {}, batch is {}",
                header.snapshot_id, snapshot_project, header.project_id
            )));
        }
        if snapshot_config != header.config_digest {
            return Err(CoreError::invalid(format!(
                "batch config_digest {} does not match snapshot config {}",
                header.config_digest, snapshot_config
            )));
        }
        Ok(())
    }
}

/// Returns `Some(outcome)` when the exact same batch was already applied and no
/// further work is needed.
fn apply_batch_transaction(
    tx: &rusqlite::Transaction<'_>,
    batch: &ValidatedBatch,
    _fault: FaultPoint,
) -> Result<Option<ImportOutcome>> {
    let header = &batch.header;
    let generation = load_generation(tx, header)?;
    if let Some(generation) = &generation {
        if generation.status == "published" || generation.status == "failed" {
            match load_batch_digest(tx, header, batch.sequence)? {
                Some(existing) if existing == batch.payload_digest => {
                    return Ok(Some(ImportOutcome {
                        sequence: batch.sequence,
                        status: ImportStatus::AlreadyApplied,
                        published: generation.status == "published",
                        entities: batch.entities.len() as u64,
                        evidence: batch.evidence.len() as u64,
                        relations: batch.relations.len() as u64,
                        diagnostics: batch.diagnostics.len() as u64,
                    }));
                }
                Some(_) => {
                    return Err(CoreError::conflict(format!(
                        "batch sequence {} was already received with a different payload digest",
                        batch.sequence
                    )))
                }
                None => {
                    return Err(CoreError::conflict(format!(
                        "generation {}/{} is {}; batch {} cannot be appended",
                        header.adapter_id, header.run_id, generation.status, batch.sequence
                    )))
                }
            }
        }
        if generation.adapter_version != header.adapter_version {
            return Err(CoreError::conflict(format!(
                "adapter_version changed mid-run: {} -> {}",
                generation.adapter_version, header.adapter_version
            )));
        }
        if let Some(existing) = load_batch_digest(tx, header, batch.sequence)? {
            if existing == batch.payload_digest {
                return Ok(Some(ImportOutcome {
                    sequence: batch.sequence,
                    status: ImportStatus::AlreadyApplied,
                    published: false,
                    entities: batch.entities.len() as u64,
                    evidence: batch.evidence.len() as u64,
                    relations: batch.relations.len() as u64,
                    diagnostics: batch.diagnostics.len() as u64,
                }));
            }
            return Err(CoreError::conflict(format!(
                "batch sequence {} was already received with a different payload digest",
                batch.sequence
            )));
        }
        let expected = next_expected_sequence(tx, header)?;
        if batch.sequence != expected {
            return Err(CoreError::invalid(format!(
                "sequence_gap: expected batch {expected}, received {}",
                batch.sequence
            )));
        }
        let final_seen: bool = tx.query_row(
            "SELECT EXISTS(SELECT 1 FROM run_batches WHERE project_id=?1 AND snapshot_id=?2 AND adapter_id=?3 AND run_id=?4 AND is_final=1)",
            (&header.project_id, &header.snapshot_id, &header.adapter_id, &header.run_id),
            |row| row.get(0),
        )?;
        if final_seen {
            return Err(CoreError::conflict("generation already received a final batch"));
        }
    } else {
        if batch.sequence != 0 {
            return Err(CoreError::invalid(format!(
                "sequence_gap: first batch must be sequence 0, received {}",
                batch.sequence
            )));
        }
        tx.execute(
            "INSERT INTO generations
             (project_id, snapshot_id, adapter_id, run_id, adapter_version, config_digest, status, started_at)
             VALUES (?1,?2,?3,?4,?5,?6,'staging',?7)",
            (
                &header.project_id,
                &header.snapshot_id,
                &header.adapter_id,
                &header.run_id,
                &header.adapter_version,
                &header.config_digest,
                now_unix(),
            ),
        )?;
    }

    stage_batch(tx, batch)?;

    tx.execute(
        "INSERT INTO run_batches
         (project_id, snapshot_id, adapter_id, run_id, sequence, payload_digest,
          entity_count, evidence_count, relation_count, diagnostic_count, is_final, coverage_json, received_at)
         VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12,?13)",
        (
            &header.project_id,
            &header.snapshot_id,
            &header.adapter_id,
            &header.run_id,
            batch.sequence as i64,
            &batch.payload_digest,
            batch.entities.len() as i64,
            batch.evidence.len() as i64,
            batch.relations.len() as i64,
            batch.diagnostics.len() as i64,
            if header.is_final { 1 } else { 0 },
            serde_json::to_string(&header.coverage).expect("coverage serializes"),
            now_unix(),
        ),
    )?;

    if header.is_final {
        publish_generation(tx, batch)?;
    }
    Ok(None)
}

struct GenerationRow {
    adapter_version: String,
    status: String,
}

fn load_generation(
    tx: &rusqlite::Transaction<'_>,
    header: &EvidenceBatch,
) -> Result<Option<GenerationRow>> {
    let row = tx
        .query_row(
            "SELECT adapter_version, status FROM generations
             WHERE project_id=?1 AND snapshot_id=?2 AND adapter_id=?3 AND run_id=?4",
            (&header.project_id, &header.snapshot_id, &header.adapter_id, &header.run_id),
            |row| {
                Ok(GenerationRow {
                    adapter_version: row.get(0)?,
                    status: row.get(1)?,
                })
            },
        )
        .ok();
    Ok(row)
}

fn load_batch_digest(
    tx: &rusqlite::Transaction<'_>,
    header: &EvidenceBatch,
    sequence: u64,
) -> Result<Option<String>> {
    Ok(tx
        .query_row(
            "SELECT payload_digest FROM run_batches
             WHERE project_id=?1 AND snapshot_id=?2 AND adapter_id=?3 AND run_id=?4 AND sequence=?5",
            (
                &header.project_id,
                &header.snapshot_id,
                &header.adapter_id,
                &header.run_id,
                sequence as i64,
            ),
            |row| row.get(0),
        )
        .ok())
}

fn next_expected_sequence(
    tx: &rusqlite::Transaction<'_>,
    header: &EvidenceBatch,
) -> Result<u64> {
    let max: Option<i64> = tx.query_row(
        "SELECT MAX(sequence) FROM run_batches
         WHERE project_id=?1 AND snapshot_id=?2 AND adapter_id=?3 AND run_id=?4",
        (&header.project_id, &header.snapshot_id, &header.adapter_id, &header.run_id),
        |row| row.get(0),
    )?;
    Ok(max.map(|value| value as u64 + 1).unwrap_or(0))
}

fn stage_batch(tx: &rusqlite::Transaction<'_>, batch: &ValidatedBatch) -> Result<()> {
    let header = &batch.header;
    let scope = (
        &header.project_id,
        &header.snapshot_id,
        &header.adapter_id,
        &header.run_id,
    );

    for item in &batch.entities {
        let entity = &item.typed;
        let existing: Option<String> = tx
            .query_row(
                "SELECT payload_digest FROM staging_entities
                 WHERE project_id=?1 AND snapshot_id=?2 AND adapter_id=?3 AND run_id=?4 AND entity_id=?5",
                (&scope.0, &scope.1, &scope.2, &scope.3, &entity.id),
                |row| row.get(0),
            )
            .ok();
        if let Some(existing) = existing {
            if existing != item.payload_digest {
                return Err(CoreError::conflict(format!(
                    "entity {} was staged with a different payload",
                    entity.id
                )));
            }
            continue;
        }
        tx.execute(
            "INSERT INTO staging_entities
             (project_id, snapshot_id, adapter_id, run_id, sequence, entity_id, payload_digest, payload_json)
             VALUES (?1,?2,?3,?4,?5,?6,?7,?8)",
            (
                &scope.0, &scope.1, &scope.2, &scope.3,
                batch.sequence as i64, &entity.id, &item.payload_digest, &item.payload_json,
            ),
        )?;
    }

    for item in &batch.evidence {
        let evidence = &item.typed;
        let existing: Option<String> = tx
            .query_row(
                "SELECT payload_digest FROM staging_evidence
                 WHERE project_id=?1 AND snapshot_id=?2 AND adapter_id=?3 AND run_id=?4 AND evidence_id=?5",
                (&scope.0, &scope.1, &scope.2, &scope.3, &evidence.id),
                |row| row.get(0),
            )
            .ok();
        if let Some(existing) = existing {
            if existing != item.payload_digest {
                return Err(CoreError::conflict(format!(
                    "evidence {} was staged with a different payload",
                    evidence.id
                )));
            }
            continue;
        }
        tx.execute(
            "INSERT INTO staging_evidence
             (project_id, snapshot_id, adapter_id, run_id, sequence, evidence_id, payload_digest, payload_json)
             VALUES (?1,?2,?3,?4,?5,?6,?7,?8)",
            (
                &scope.0, &scope.1, &scope.2, &scope.3,
                batch.sequence as i64, &evidence.id, &item.payload_digest, &item.payload_json,
            ),
        )?;
    }

    for item in &batch.relations {
        let relation = &item.typed;
        let existing: Option<String> = tx
            .query_row(
                "SELECT payload_digest FROM staging_relations
                 WHERE project_id=?1 AND snapshot_id=?2 AND adapter_id=?3 AND run_id=?4 AND relation_id=?5",
                (&scope.0, &scope.1, &scope.2, &scope.3, &relation.id),
                |row| row.get(0),
            )
            .ok();
        if let Some(existing) = existing {
            if existing != item.payload_digest {
                return Err(CoreError::conflict(format!(
                    "relation {} was staged with a different payload",
                    relation.id
                )));
            }
            continue;
        }
        tx.execute(
            "INSERT INTO staging_relations
             (project_id, snapshot_id, adapter_id, run_id, sequence, relation_id, source_id, target_id,
              kind, basis, resolution, payload_digest, payload_json)
             VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12,?13)",
            (
                &scope.0, &scope.1, &scope.2, &scope.3,
                batch.sequence as i64, &relation.id, &relation.source_id, &relation.target_id,
                relation.kind.as_str(), relation.basis.as_str(), resolution_str(relation.resolution),
                &item.payload_digest, &item.payload_json,
            ),
        )?;
        for evidence_id in &relation.evidence_ids {
            tx.execute(
                "INSERT INTO staging_relation_evidence
                 (project_id, snapshot_id, adapter_id, run_id, relation_id, evidence_id)
                 VALUES (?1,?2,?3,?4,?5,?6)",
                (&scope.0, &scope.1, &scope.2, &scope.3, &relation.id, evidence_id),
            )?;
        }
    }

    for (ordinal, diagnostic) in batch.diagnostics.iter().enumerate() {
        tx.execute(
            "INSERT INTO staging_diagnostics
             (project_id, snapshot_id, adapter_id, run_id, sequence, ordinal, code, severity, message, entity_id, path)
             VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11)",
            (
                &scope.0, &scope.1, &scope.2, &scope.3,
                batch.sequence as i64, ordinal as i64, &diagnostic.code,
                severity_str(diagnostic.severity), &diagnostic.message,
                &diagnostic.entity_id, &diagnostic.path,
            ),
        )?;
    }
    Ok(())
}

fn publish_generation(tx: &rusqlite::Transaction<'_>, batch: &ValidatedBatch) -> Result<()> {
    let header = &batch.header;
    let scope = (&header.project_id, &header.snapshot_id);

    let staged_entities: Vec<(String, String, String)> = {
        let mut statement = tx.prepare(
            "SELECT entity_id, payload_digest, payload_json FROM staging_entities
             WHERE project_id=?1 AND snapshot_id=?2 AND adapter_id=?3 AND run_id=?4",
        )?;
        let rows = statement.query_map(
            (&header.project_id, &header.snapshot_id, &header.adapter_id, &header.run_id),
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
        )?;
        rows.collect::<rusqlite::Result<Vec<_>>>()?
    };
    let staged_evidence: Vec<(String, String)> = {
        let mut statement = tx.prepare(
            "SELECT evidence_id, payload_json FROM staging_evidence
             WHERE project_id=?1 AND snapshot_id=?2 AND adapter_id=?3 AND run_id=?4",
        )?;
        let rows = statement.query_map(
            (&header.project_id, &header.snapshot_id, &header.adapter_id, &header.run_id),
            |row| Ok((row.get(0)?, row.get(1)?)),
        )?;
        rows.collect::<rusqlite::Result<Vec<_>>>()?
    };
    let staged_relations: Vec<(String, String, String, String, String)> = {
        let mut statement = tx.prepare(
            "SELECT relation_id, source_id, target_id, basis, payload_json FROM staging_relations
             WHERE project_id=?1 AND snapshot_id=?2 AND adapter_id=?3 AND run_id=?4",
        )?;
        let rows = statement.query_map(
            (&header.project_id, &header.snapshot_id, &header.adapter_id, &header.run_id),
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?, row.get(4)?)),
        )?;
        rows.collect::<rusqlite::Result<Vec<_>>>()?
    };

    let mut known_entities: HashSet<String> = HashSet::new();
    {
        let mut statement = tx.prepare(
            "SELECT entity_id FROM entities WHERE project_id=?1 AND snapshot_id=?2",
        )?;
        let rows = statement.query_map(scope, |row| row.get::<_, String>(0))?;
        for row in rows {
            known_entities.insert(row?);
        }
    }
    for (entity_id, _, _) in &staged_entities {
        known_entities.insert(entity_id.clone());
    }

    let mut evidence_kinds: HashMap<String, String> = HashMap::new();
    {
        let mut statement = tx.prepare(
            "SELECT evidence_id, kind FROM evidence WHERE project_id=?1 AND snapshot_id=?2",
        )?;
        let rows = statement.query_map(scope, |row| {
            Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
        })?;
        for row in rows {
            let (id, kind) = row?;
            evidence_kinds.insert(id, kind);
        }
    }
    for (evidence_id, payload_json) in &staged_evidence {
        let evidence: Evidence = serde_json::from_str(payload_json)
            .map_err(|err| CoreError::Storage(format!("staged evidence JSON is corrupt: {err}")))?;
        evidence_kinds.insert(evidence_id.clone(), evidence.kind.as_str().to_string());
    }

    // Cross-batch reference and evidence-class checks before writing anything
    // visible.
    for (relation_id, source_id, target_id, basis, payload_json) in &staged_relations {
        if !known_entities.contains(source_id) {
            return Err(CoreError::invalid(format!(
                "relation {relation_id}: source entity {source_id} does not exist in snapshot {}",
                header.snapshot_id
            )));
        }
        if !known_entities.contains(target_id) {
            return Err(CoreError::invalid(format!(
                "relation {relation_id}: target entity {target_id} does not exist in snapshot {}",
                header.snapshot_id
            )));
        }
        let relation: Relation = serde_json::from_str(payload_json)
            .map_err(|err| CoreError::Storage(format!("staged relation JSON is corrupt: {err}")))?;
        let mut kinds = Vec::with_capacity(relation.evidence_ids.len());
        for evidence_id in &relation.evidence_ids {
            match evidence_kinds.get(evidence_id) {
                Some(kind) => kinds.push(kind.clone()),
                None => {
                    return Err(CoreError::invalid(format!(
                        "relation {relation_id}: evidence {evidence_id} does not exist in snapshot {}",
                        header.snapshot_id
                    )))
                }
            }
        }
        if !kinds.iter().any(|kind| kind == basis) {
            return Err(CoreError::invalid(format!(
                "relation {relation_id}: no evidence of basis {basis} among its evidence ids"
            )));
        }
        if relation.kind == RelationKind::ResolvedCall && !kinds.iter().any(|k| k == "semantic") {
            return Err(CoreError::invalid(format!(
                "relation {relation_id}: resolved_call requires semantic evidence"
            )));
        }
        if relation.kind == RelationKind::ObservedCall && !kinds.iter().any(|k| k == "runtime") {
            return Err(CoreError::invalid(format!(
                "relation {relation_id}: observed_call requires runtime evidence"
            )));
        }
    }

    for (entity_id, payload_digest, payload_json) in &staged_entities {
        let entity: Entity = serde_json::from_str(payload_json)
            .map_err(|err| CoreError::Storage(format!("staged entity JSON is corrupt: {err}")))?;
        let existing: Option<String> = tx
            .query_row(
                "SELECT payload_json FROM entities WHERE project_id=?1 AND snapshot_id=?2 AND entity_id=?3",
                (&header.project_id, &header.snapshot_id, entity_id),
                |row| row.get(0),
            )
            .ok();
        if let Some(existing) = existing {
            if existing != *payload_json {
                return Err(CoreError::conflict(format!(
                    "entity {entity_id} already exists in this snapshot with a different payload"
                )));
            }
            continue;
        }
        let _ = payload_digest;
        tx.execute(
            "INSERT INTO entities
             (project_id, snapshot_id, entity_id, config_digest, kind, language, qualified_name,
              external, adapter_id, run_id, payload_json)
             VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11)",
            (
                &header.project_id,
                &header.snapshot_id,
                entity_id,
                &header.config_digest,
                entity_kind_str(entity.kind),
                &entity.language,
                &entity.qualified_name,
                if entity.external { 1 } else { 0 },
                &header.adapter_id,
                &header.run_id,
                payload_json,
            ),
        )?;
        for backend in &entity.backend_refs {
            let existing_entity: Option<String> = tx
                .query_row(
                    "SELECT entity_id FROM backend_mappings
                     WHERE project_id=?1 AND snapshot_id=?2 AND adapter_id=?3 AND run_id=?4 AND backend_id=?5",
                    (
                        &header.project_id,
                        &header.snapshot_id,
                        &backend.adapter_id,
                        &backend.run_id,
                        &backend.backend_id,
                    ),
                    |row| row.get(0),
                )
                .ok();
            if let Some(existing_entity) = existing_entity {
                if existing_entity != *entity_id {
                    return Err(CoreError::conflict(format!(
                        "backend id {} already maps to {} in this snapshot",
                        backend.backend_id, existing_entity
                    )));
                }
                continue;
            }
            tx.execute(
                "INSERT INTO backend_mappings
                 (project_id, snapshot_id, adapter_id, run_id, backend_id, entity_id)
                 VALUES (?1,?2,?3,?4,?5,?6)",
                (
                    &header.project_id,
                    &header.snapshot_id,
                    &backend.adapter_id,
                    &backend.run_id,
                    &backend.backend_id,
                    entity_id,
                ),
            )?;
        }
    }

    for (evidence_id, payload_json) in &staged_evidence {
        let evidence: Evidence = serde_json::from_str(payload_json)
            .map_err(|err| CoreError::Storage(format!("staged evidence JSON is corrupt: {err}")))?;
        let existing: Option<String> = tx
            .query_row(
                "SELECT payload_json FROM evidence WHERE project_id=?1 AND snapshot_id=?2 AND evidence_id=?3",
                (&header.project_id, &header.snapshot_id, evidence_id),
                |row| row.get(0),
            )
            .ok();
        if let Some(existing) = existing {
            if existing != *payload_json {
                return Err(CoreError::conflict(format!(
                    "evidence {evidence_id} already exists in this snapshot with a different payload"
                )));
            }
            continue;
        }
        tx.execute(
            "INSERT INTO evidence
             (project_id, snapshot_id, evidence_id, config_digest, kind, producer_name, producer_version,
              artifact_digest, coverage_json, freshness, payload_json)
             VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11)",
            (
                &header.project_id,
                &header.snapshot_id,
                evidence_id,
                &header.config_digest,
                evidence.kind.as_str(),
                &evidence.producer.name,
                &evidence.producer.version,
                &evidence.artifact_digest,
                serde_json::to_string(&evidence.coverage).expect("coverage serializes"),
                freshness_str(evidence.freshness),
                payload_json,
            ),
        )?;
        tx.execute(
            "INSERT OR IGNORE INTO artifact_refs (owner_type, owner_id, digest) VALUES ('evidence', ?1, ?2)",
            (evidence_id, &evidence.artifact_digest),
        )?;
    }

    for (relation_id, source_id, target_id, basis, payload_json) in &staged_relations {
        let relation: Relation = serde_json::from_str(payload_json)
            .map_err(|err| CoreError::Storage(format!("staged relation JSON is corrupt: {err}")))?;
        let existing: Option<String> = tx
            .query_row(
                "SELECT payload_json FROM relations WHERE project_id=?1 AND snapshot_id=?2 AND relation_id=?3",
                (&header.project_id, &header.snapshot_id, relation_id),
                |row| row.get(0),
            )
            .ok();
        if let Some(existing) = existing {
            if existing != *payload_json {
                return Err(CoreError::conflict(format!(
                    "relation {relation_id} already exists in this snapshot with a different payload"
                )));
            }
            continue;
        }
        tx.execute(
            "INSERT INTO relations
             (project_id, snapshot_id, relation_id, config_digest, source_id, target_id, kind, basis, resolution, payload_json)
             VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9,?10)",
            (
                &header.project_id,
                &header.snapshot_id,
                relation_id,
                &header.config_digest,
                source_id,
                target_id,
                relation.kind.as_str(),
                basis,
                resolution_str(relation.resolution),
                payload_json,
            ),
        )?;
        for evidence_id in &relation.evidence_ids {
            tx.execute(
                "INSERT INTO relation_evidence (project_id, snapshot_id, relation_id, evidence_id)
                 VALUES (?1,?2,?3,?4)",
                (&header.project_id, &header.snapshot_id, relation_id, evidence_id),
            )?;
        }
    }

    {
        let mut statement = tx.prepare(
            "SELECT sequence, ordinal, code, severity, message, entity_id, path
             FROM staging_diagnostics
             WHERE project_id=?1 AND snapshot_id=?2 AND adapter_id=?3 AND run_id=?4
             ORDER BY sequence, ordinal",
        )?;
        let rows = statement.query_map(
            (&header.project_id, &header.snapshot_id, &header.adapter_id, &header.run_id),
            |row| {
                Ok((
                    row.get::<_, i64>(0)?,
                    row.get::<_, i64>(1)?,
                    row.get::<_, String>(2)?,
                    row.get::<_, String>(3)?,
                    row.get::<_, String>(4)?,
                    row.get::<_, Option<String>>(5)?,
                    row.get::<_, Option<String>>(6)?,
                ))
            },
        )?;
        let diagnostics = rows.collect::<rusqlite::Result<Vec<_>>>()?;
        drop(statement);
        for (sequence, ordinal, code, severity, message, entity_id, path) in diagnostics {
            tx.execute(
                "INSERT INTO diagnostics
                 (project_id, snapshot_id, adapter_id, run_id, sequence, ordinal, code, severity, message, entity_id, path)
                 VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11)",
                (
                    &header.project_id, &header.snapshot_id, &header.adapter_id, &header.run_id,
                    sequence, ordinal, code, severity, message, entity_id, path,
                ),
            )?;
        }
    }

    delete_staging(
        tx,
        &header.project_id,
        &header.snapshot_id,
        &header.adapter_id,
        &header.run_id,
    )?;

    tx.execute(
        "UPDATE generations
         SET status='published', final_seq=?5, coverage_json=?6, finished_at=?7
         WHERE project_id=?1 AND snapshot_id=?2 AND adapter_id=?3 AND run_id=?4",
        (
            &header.project_id,
            &header.snapshot_id,
            &header.adapter_id,
            &header.run_id,
            batch.sequence as i64,
            serde_json::to_string(&header.coverage).expect("coverage serializes"),
            now_unix(),
        ),
    )?;

    let next_event = crate::events::next_sequence(tx, &header.project_id)?;
    let event_id = format!(
        "event.analysis.completed.{}.{}",
        header.run_id, batch.sequence
    );
    tx.execute(
        "INSERT INTO events
         (project_id, sequence, event_id, type, payload_json, created_at, job_id, snapshot_id)
         VALUES (?1, ?2, ?3, 'analysis.completed', ?4, ?5, NULL, ?6)",
        (
            &header.project_id,
            next_event,
            event_id,
            serde_json::json!({
                "run_id": header.run_id,
                "coverage": header.coverage,
                "diagnostic_ids": [],
            })
            .to_string(),
            now_unix(),
            &header.snapshot_id,
        ),
    )?;
    Ok(())
}

fn delete_staging(
    tx: &rusqlite::Transaction<'_>,
    project_id: &str,
    snapshot_id: &str,
    adapter_id: &str,
    run_id: &str,
) -> Result<()> {
    let key = (project_id, snapshot_id, adapter_id, run_id);
    for table in [
        "staging_relation_evidence",
        "staging_relations",
        "staging_evidence",
        "staging_entities",
        "staging_diagnostics",
    ] {
        let sql = format!(
            "DELETE FROM {table} WHERE project_id=?1 AND snapshot_id=?2 AND adapter_id=?3 AND run_id=?4"
        );
        tx.execute(&sql, key)?;
    }
    Ok(())
}

fn entity_kind_str(kind: EntityKind) -> &'static str {
    match kind {
        EntityKind::File => "file",
        EntityKind::Module => "module",
        EntityKind::Symbol => "symbol",
        EntityKind::Type => "type",
        EntityKind::Callsite => "callsite",
        EntityKind::Component => "component",
        EntityKind::Process => "process",
        EntityKind::Thread => "thread",
        EntityKind::Task => "task",
        EntityKind::Queue => "queue",
        EntityKind::Resource => "resource",
        EntityKind::Boundary => "boundary",
        EntityKind::External => "external",
    }
}

fn resolution_str(resolution: Resolution) -> &'static str {
    match resolution {
        Resolution::Resolved => "resolved",
        Resolution::Candidate => "candidate",
        Resolution::Observed => "observed",
        Resolution::Declared => "declared",
        Resolution::Unresolved => "unresolved",
    }
}

fn freshness_str(freshness: Freshness) -> &'static str {
    match freshness {
        Freshness::Current => "current",
        Freshness::Historical => "historical",
        Freshness::Stale => "stale",
        Freshness::Unbound => "unbound",
    }
}

fn severity_str(severity: Severity) -> &'static str {
    match severity {
        Severity::Info => "info",
        Severity::Warning => "warning",
        Severity::Error => "error",
    }
}

#[allow(dead_code)]
fn _sequence(value: &str) -> Result<u64> {
    parse_sequence(value)
}
