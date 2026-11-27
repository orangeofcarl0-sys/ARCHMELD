// Copyright 2026 ARCHMELD contributors
// SPDX-License-Identifier: Apache-2.0

//! Candidate architecture proposals, reviews and persistence.

use super::*;
use super::codeboarding::verify_architecture_references;

pub fn propose_architecture(store: &mut Store, raw: &Value) -> Result<Value> {
    let proposal: ArchitectureProposal = serde_json::from_value(raw.clone()).map_err(|err| {
        CoreError::invalid(format!("ArchitectureProposal does not match the contract: {err}"))
    })?;
    validate::require_id("idempotency_key", &proposal.idempotency_key)?;
    let mut architecture = proposal.model;
    validate_architecture(store, &proposal.project_id, &architecture)?;
    if architecture.status != ArchitectureStatus::Candidate
        && architecture.status != ArchitectureStatus::NeedsReview
    {
        return Err(CoreError::invalid(
            "proposal-only: architecture status must be candidate or needs_review",
        ));
    }
    let revision = latest_architecture(store, &architecture.project_id, &architecture.id)?
        .map(|(revision, _)| revision + 1)
        .unwrap_or(1);
    architecture.revision = revision;
    let project_revision = project_revision(store, &architecture.project_id)?;
    if proposal.expected_revision != project_revision {
        return Err(CoreError::conflict(format!(
            "stale_snapshot: expected_revision {} does not match project revision {project_revision}",
            proposal.expected_revision
        )));
    }
    let request = json!({
        "project_id": proposal.project_id,
        "model": raw["model"].clone(),
        "expected_revision": proposal.expected_revision,
    });
    let digest = hash::digest_value(&request)
        .map_err(|err| CoreError::invalid(format!("proposal is outside the hash domain: {err}")))?;
    if let Some((recorded, _, result)) =
        store.idempotency_get(&proposal.project_id, "proposeArchitecture", &proposal.idempotency_key)?
    {
        if recorded != digest {
            return Err(CoreError::conflict(
                "scope_conflict: idempotency_key was used with a different architecture",
            ));
        }
        return serde_json::from_str(&result)
            .map_err(|err| CoreError::Storage(format!("stored architecture corrupt: {err}")));
    }
    let mut verification = verify_architecture_references(store, &architecture);
    verification["commands_ignored"] = json!(0);
    verification["commands_executed"] = json!(0);
    insert_architecture(
        store,
        &architecture,
        &json!({
            "name": "human",
            "version": "manual-1",
            "source_ref": "ArchitectureProposal",
            "run_id": format!("manual.{}", &digest[7..23]),
        }),
        None,
        0,
        &verification,
        "proposeArchitecture",
        &proposal.idempotency_key,
        &digest,
    )?;
    Ok(serde_json::to_value(&architecture)
        .map_err(|err| CoreError::Storage(format!("architecture encode: {err}")))?)
}

/// Validate an architecture document without storing it (used by target-graph
/// diffing, which must never mutate the ledger or source tree).
pub fn validate_architecture_document(
    store: &Store,
    project_id: &str,
    raw: &Value,
) -> Result<Value> {
    let architecture: Architecture = serde_json::from_value(raw.clone())
        .map_err(|err| CoreError::invalid(format!("Architecture does not match the contract: {err}")))?;
    validate_architecture(store, project_id, &architecture)?;
    serde_json::to_value(&architecture)
        .map_err(|err| CoreError::Storage(format!("architecture encode: {err}")))
}

pub fn get_architecture(store: &Store, project_id: &str, architecture_id: &str) -> Result<Value> {
    validate::require_id("project_id", project_id)?;
    validate::require_id("architecture_id", architecture_id)?;
    let (_, payload) = latest_architecture(store, project_id, architecture_id)?
        .ok_or_else(|| CoreError::not_found(format!("architecture {architecture_id} not found")))?;
    serde_json::from_str(&payload)
        .map_err(|err| CoreError::Storage(format!("architecture payload corrupt: {err}")))
}

/// Producer metadata for the latest architecture revision. The payload stays
/// separate from the producer record; this exposes the recorded model
/// provenance without inventing UI fields.
pub fn architecture_producer(
    store: &Store,
    project_id: &str,
    architecture_id: &str,
) -> Result<Value> {
    validate::require_id("project_id", project_id)?;
    validate::require_id("architecture_id", architecture_id)?;
    let producer: Option<String> = store
        .conn
        .query_row(
            "SELECT producer_json FROM architectures
             WHERE project_id=?1 AND architecture_id=?2 ORDER BY revision DESC LIMIT 1",
            (project_id, architecture_id),
            |row| row.get(0),
        )
        .ok();
    let Some(producer) = producer else {
        return Err(CoreError::not_found(format!(
            "architecture {architecture_id} not found"
        )));
    };
    serde_json::from_str(&producer)
        .map_err(|err| CoreError::Storage(format!("architecture producer corrupt: {err}")))
}

pub fn architecture_review(
    store: &Store,
    project_id: &str,
    architecture_id: &str,
) -> Result<Value> {
    validate::require_id("project_id", project_id)?;
    validate::require_id("architecture_id", architecture_id)?;
    let verification: Option<String> = store
        .conn
        .query_row(
            "SELECT verification_json FROM architectures
             WHERE project_id=?1 AND architecture_id=?2 ORDER BY revision DESC LIMIT 1",
            (project_id, architecture_id),
            |row| row.get(0),
        )
        .ok();
    let Some(verification) = verification else {
        return Err(CoreError::not_found(format!(
            "architecture {architecture_id} not found"
        )));
    };
    serde_json::from_str(&verification)
        .map_err(|err| CoreError::Storage(format!("verification payload corrupt: {err}")))
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ArchitectureReviewRequest {
    project_id: String,
    architecture_id: String,
    expected_revision: i64,
    action: String,
    reason: String,
    actor: String,
    idempotency_key: String,
}

/// Append a human review decision to an architecture's history. Accepting a
/// candidate records that the interpretation is used for the current design;
/// it is not a formal proof, and it never edits the target graph or source.
pub fn review_architecture(store: &mut Store, raw: &Value) -> Result<Value> {
    let request: ArchitectureReviewRequest = serde_json::from_value(raw.clone())
        .map_err(|err| CoreError::invalid(format!("architecture review: {err}")))?;
    validate::require_id("project_id", &request.project_id)?;
    validate::require_id("architecture_id", &request.architecture_id)?;
    validate::require_non_empty("reason", &request.reason)?;
    validate::require_non_empty("actor", &request.actor)?;
    if request.reason.len() > 10_000 || request.actor.len() > 200 {
        return Err(CoreError::invalid(
            "resource_exhausted: reason or actor exceeds the review limit",
        ));
    }
    validate::require_id("idempotency_key", &request.idempotency_key)?;
    let status = match request.action.as_str() {
        "accept" => ArchitectureStatus::Accepted,
        "reject" => ArchitectureStatus::NeedsReview,
        "request_changes" => ArchitectureStatus::NeedsReview,
        "mark_reviewed" => ArchitectureStatus::Reviewed,
        other => {
            return Err(CoreError::invalid(format!(
                "unknown architecture review action {other:?}"
            )))
        }
    };
    let request_digest = hash::digest_value(raw)
        .map_err(|err| CoreError::invalid(format!("architecture review: {err}")))?;
    if let Some((recorded, _, result)) = store.idempotency_get(
        &request.project_id,
        "reviewArchitecture",
        &request.idempotency_key,
    )? {
        if recorded != request_digest {
            return Err(CoreError::conflict(
                "scope_conflict: idempotency_key was used with a different architecture review",
            ));
        }
        return serde_json::from_str(&result)
            .map_err(|err| CoreError::Storage(format!("stored review corrupt: {err}")));
    }
    let row: Option<(i64, String, String, String, Option<String>, i64)> = store
        .conn
        .query_row(
            "SELECT revision,payload_json,verification_json,producer_json,
                    artifact_digest,commands_ignored
             FROM architectures WHERE project_id=?1 AND architecture_id=?2
             ORDER BY revision DESC LIMIT 1",
            (&request.project_id, &request.architecture_id),
            |row| {
                Ok((
                    row.get(0)?,
                    row.get(1)?,
                    row.get(2)?,
                    row.get(3)?,
                    row.get(4)?,
                    row.get(5)?,
                ))
            },
        )
        .ok();
    let Some((latest, payload_json, verification_json, producer_json, artifact_digest, commands)) =
        row
    else {
        return Err(CoreError::not_found(format!(
            "architecture {} not found",
            request.architecture_id
        )));
    };
    if request.expected_revision != latest {
        return Err(CoreError::conflict(format!(
            "stale_snapshot: expected architecture revision {} does not match {latest}",
            request.expected_revision
        )));
    }
    let mut architecture: Architecture = serde_json::from_str(&payload_json)
        .map_err(|err| CoreError::Storage(format!("architecture payload corrupt: {err}")))?;
    if architecture.id != request.architecture_id {
        return Err(CoreError::Storage(
            "architecture payload id disagrees with the review target".to_string(),
        ));
    }
    architecture.revision = latest + 1;
    architecture.status = status;
    validate_architecture(store, &request.project_id, &architecture)?;
    let payload = serde_json::to_value(&architecture)
        .map_err(|err| CoreError::Storage(format!("architecture encode: {err}")))?;
    let canonical = hash::canonical_json(&payload)
        .map_err(|err| CoreError::invalid(format!("architecture is outside the hash domain: {err}")))?;
    let config_digest =
        snapshot_config(store, &request.project_id, &architecture.snapshot_id)?;
    let result = serde_json::to_string(&payload)
        .map_err(|err| CoreError::Storage(format!("architecture encode: {err}")))?;
    let actor_json = json!({"name": request.actor, "source": "local_api"});
    let tx = store.conn.transaction()?;
    tx.execute(
        "INSERT INTO architectures
         (project_id,architecture_id,snapshot_id,config_digest,revision,layer,status,
          producer_json,artifact_digest,commands_ignored,payload_json,verification_json,created_at)
         VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12,?13)",
        (
            &request.project_id,
            &architecture.id,
            &architecture.snapshot_id,
            &config_digest,
            architecture.revision,
            architecture.layer.as_str(),
            architecture.status.as_str(),
            &producer_json,
            artifact_digest.as_deref(),
            commands,
            &canonical,
            &verification_json,
            crate::store::now_unix(),
        ),
    )?;
    tx.execute(
        "INSERT INTO architecture_reviews
         (project_id,architecture_id,revision,action,reason,actor_json,created_at)
         VALUES (?1,?2,?3,?4,?5,?6,?7)",
        (
            &request.project_id,
            &architecture.id,
            architecture.revision,
            &request.action,
            &request.reason,
            hash::canonical_json(&actor_json)
                .map_err(|err| CoreError::invalid(format!("review actor: {err}")))?,
            crate::store::now_unix(),
        ),
    )?;
    append_event(
        &tx,
        &request.project_id,
        None,
        Some(&architecture.snapshot_id),
        "architecture.reviewed",
        json!({
            "architecture_id": architecture.id,
            "previous_revision": latest,
            "revision": architecture.revision,
            "action": request.action,
            "status": architecture.status.as_str(),
            "candidate_only": false,
        }),
    )?;
    tx.execute(
        "INSERT INTO idempotency
         (project_id,operation,idempotency_key,request_digest,status_code,result_json,created_at)
         VALUES (?1,'reviewArchitecture',?2,?3,200,?4,?5)",
        (
            &request.project_id,
            &request.idempotency_key,
            &request_digest,
            &result,
            crate::store::now_unix(),
        ),
    )?;
    tx.commit()?;
    Ok(payload)
}

pub fn list_architecture_reviews(
    store: &Store,
    project_id: &str,
    architecture_id: &str,
) -> Result<Vec<Value>> {
    validate::require_id("project_id", project_id)?;
    validate::require_id("architecture_id", architecture_id)?;
    let mut statement = store.conn.prepare(
        "SELECT revision,action,reason,actor_json,created_at FROM architecture_reviews
         WHERE project_id=?1 AND architecture_id=?2 ORDER BY revision ASC",
    )?;
    let rows = statement.query_map((project_id, architecture_id), |row| {
        Ok((
            row.get::<_, i64>(0)?,
            row.get::<_, String>(1)?,
            row.get::<_, String>(2)?,
            row.get::<_, String>(3)?,
            row.get::<_, i64>(4)?,
        ))
    })?;
    let mut reviews = Vec::new();
    for row in rows {
        let (revision, action, reason, actor_json, created_at) = row?;
        let actor: Value = serde_json::from_str(&actor_json)
            .map_err(|err| CoreError::Storage(format!("review actor corrupt: {err}")))?;
        reviews.push(json!({
            "revision": revision,
            "action": action,
            "reason": reason,
            "actor": actor,
            "created_at": created_at,
        }));
    }
    Ok(reviews)
}


#[allow(clippy::too_many_arguments)]
pub(super) fn insert_architecture(
    store: &mut Store,
    architecture: &Architecture,
    producer: &Value,
    artifact_digest: Option<&str>,
    commands_ignored: usize,
    verification: &Value,
    operation: &str,
    idempotency_key: &str,
    request_digest: &str,
) -> Result<()> {
    let config_digest = snapshot_config(store, &architecture.project_id, &architecture.snapshot_id)?;
    let payload = serde_json::to_value(architecture)
        .map_err(|err| CoreError::Storage(format!("architecture encode: {err}")))?;
    let payload_json = hash::canonical_json(&payload)
        .map_err(|err| CoreError::invalid(format!("architecture is outside the hash domain: {err}")))?;
    let result = serde_json::to_string(&payload)
        .map_err(|err| CoreError::Storage(format!("architecture encode: {err}")))?;
    let tx = store.conn.transaction()?;
    tx.execute(
        "INSERT INTO architectures
         (project_id,architecture_id,snapshot_id,config_digest,revision,layer,status,producer_json,
          artifact_digest,commands_ignored,payload_json,verification_json,created_at)
         VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12,?13)",
        (
            &architecture.project_id,
            &architecture.id,
            &architecture.snapshot_id,
            &config_digest,
            architecture.revision,
            architecture.layer.as_str(),
            architecture.status.as_str(),
            producer.to_string(),
            artifact_digest,
            commands_ignored as i64,
            &payload_json,
            verification.to_string(),
            crate::store::now_unix(),
        ),
    )?;
    append_event(
        &tx,
        &architecture.project_id,
        None,
        Some(&architecture.snapshot_id),
        "architecture.proposed",
        json!({
            "architecture_id": architecture.id,
            "revision": architecture.revision,
            "status": architecture.status.as_str(),
            "candidate_only": true,
        }),
    )?;
    tx.execute(
        "INSERT INTO idempotency
         (project_id,operation,idempotency_key,request_digest,status_code,result_json,created_at)
         VALUES (?1,?2,?3,?4,200,?5,?6)",
        (
            &architecture.project_id,
            operation,
            idempotency_key,
            request_digest,
            &result,
            crate::store::now_unix(),
        ),
    )?;
    tx.commit()?;
    Ok(())
}
