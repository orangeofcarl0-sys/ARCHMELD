// Copyright 2026 ARCHMELD contributors
// SPDX-License-Identifier: Apache-2.0

//! Claims, review and lookup.

use super::*;

pub fn propose_claim(store: &mut Store, raw: &Value) -> Result<Value> {
    let proposal: ClaimProposal = serde_json::from_value(raw.clone())
        .map_err(|err| CoreError::invalid(format!("ClaimProposal does not match the contract: {err}")))?;
    validate::require_id("project_id", &proposal.project_id)?;
    validate::require_id("idempotency_key", &proposal.idempotency_key)?;
    let claim = &proposal.claim;
    if claim.project_id != proposal.project_id {
        return Err(CoreError::invalid(
            "claim.project_id does not match the proposal project_id",
        ));
    }
    validate_claim(store, claim)?;
    let project_revision = project_revision(store, &proposal.project_id)?;
    if proposal.expected_revision != project_revision {
        return Err(CoreError::conflict(format!(
            "stale_snapshot: expected_revision {} does not match project revision {project_revision}",
            proposal.expected_revision
        )));
    }
    let digest = hash::digest_value(raw)
        .map_err(|err| CoreError::invalid(format!("proposal is outside the hash domain: {err}")))?;
    if let Some((recorded, _, result)) =
        store.idempotency_get(&proposal.project_id, "proposeClaim", &proposal.idempotency_key)?
    {
        if recorded != digest {
            return Err(CoreError::conflict(
                "scope_conflict: idempotency_key was used with a different claim",
            ));
        }
        return serde_json::from_str(&result)
            .map_err(|err| CoreError::Storage(format!("stored claim result corrupt: {err}")));
    }
    if latest_claim(store, &claim.project_id, &claim.id)?.is_some() {
        return Err(CoreError::conflict(format!(
            "claim {} already exists; propose a review instead of a new claim",
            claim.id
        )));
    }
    let config_digest = snapshot_config(store, &claim.project_id, &claim.snapshot_id)?;
    let payload = serde_json::to_value(claim)
        .map_err(|err| CoreError::Storage(format!("claim encode: {err}")))?;
    let canonical = hash::canonical_json(&payload)
        .map_err(|err| CoreError::invalid(format!("claim is outside the hash domain: {err}")))?;
    let result = raw_claim_json(claim)?;
    let tx = store.conn.transaction()?;
    tx.execute(
        "INSERT INTO claims
         (project_id,claim_id,snapshot_id,config_digest,revision,claim_type,text,status,scope,
          producer_json,support_ids_json,counter_ids_json,questions_json,payload_json,created_at)
         VALUES (?1,?2,?3,?4,1,?5,?6,?7,?8,?9,?10,?11,?12,?13,?14)",
        (
            &claim.project_id,
            &claim.id,
            &claim.snapshot_id,
            &config_digest,
            claim.claim_type.as_str(),
            &claim.text,
            claim.status.as_str(),
            &claim.scope,
            serde_json::to_string(&claim.producer).unwrap(),
            serde_json::to_string(&claim.support_ids).unwrap(),
            serde_json::to_string(&claim.counterevidence_ids).unwrap(),
            serde_json::to_string(&claim.unresolved_questions).unwrap(),
            &canonical,
            crate::store::now_unix(),
        ),
    )?;
    append_event(
        &tx,
        &claim.project_id,
        None,
        Some(&claim.snapshot_id),
        "claim.proposed",
        json!({"claim_id": claim.id, "status": claim.status.as_str(), "revision": 1}),
    )?;
    tx.execute(
        "INSERT INTO idempotency
         (project_id,operation,idempotency_key,request_digest,status_code,result_json,created_at)
         VALUES (?1,'proposeClaim',?2,?3,200,?4,?5)",
        (
            &claim.project_id,
            &proposal.idempotency_key,
            &digest,
            &result.to_string(),
            crate::store::now_unix(),
        ),
    )?;
    tx.commit()?;
    Ok(result)
}

pub fn review_claim(
    store: &mut Store,
    project_id: &str,
    claim_id: &str,
    expected_revision: i64,
    status: &str,
    reason: &str,
    actor: &Value,
    additional_support_ids: &[String],
    additional_counterevidence_ids: &[String],
    idempotency_key: &str,
) -> Result<Value> {
    validate::require_id("project_id", project_id)?;
    validate::require_id("claim_id", claim_id)?;
    validate::require_id("idempotency_key", idempotency_key)?;
    if reason.is_empty() {
        return Err(CoreError::invalid("review reason must not be empty"));
    }
    let new_status = match status {
        "accepted" => ClaimStatus::Accepted,
        "rejected" => ClaimStatus::Rejected,
        "needs_review" => ClaimStatus::NeedsReview,
        other => {
            return Err(CoreError::invalid(format!(
                "unsupported claim review status {other}"
            )))
        }
    };
    let (latest_revision, latest_json) = latest_claim(store, project_id, claim_id)?
        .ok_or_else(|| CoreError::not_found(format!("claim {claim_id} not found")))?;
    if expected_revision != latest_revision {
        return Err(CoreError::conflict(format!(
            "stale_snapshot: expected claim revision {expected_revision} does not match {latest_revision}"
        )));
    }
    let mut claim: Claim = serde_json::from_str(&latest_json)
        .map_err(|err| CoreError::Storage(format!("claim payload corrupt: {err}")))?;
    for id in additional_support_ids
        .iter()
        .chain(additional_counterevidence_ids.iter())
    {
        validate::require_id("claim.review_ids[]", id)?;
        evidence_or_claim(store, project_id, &claim.snapshot_id, id)?;
    }
    for id in additional_support_ids {
        if !claim.support_ids.contains(id) {
            claim.support_ids.push(id.clone());
        }
    }
    for id in additional_counterevidence_ids {
        if !claim.counterevidence_ids.contains(id) {
            claim.counterevidence_ids.push(id.clone());
        }
    }
    match new_status {
        ClaimStatus::Rejected if claim.counterevidence_ids.is_empty() => {
            return Err(CoreError::invalid(
                "a rejected claim requires at least one counterevidence id",
            ))
        }
        ClaimStatus::Accepted if claim.support_ids.is_empty() => {
            return Err(CoreError::invalid(
                "an accepted claim requires at least one support id",
            ))
        }
        _ => {}
    }
    claim.status = new_status;
    let config_digest = snapshot_config(store, project_id, &claim.snapshot_id)?;
    let payload = serde_json::to_value(&claim)
        .map_err(|err| CoreError::Storage(format!("claim encode: {err}")))?;
    let canonical = hash::canonical_json(&payload)
        .map_err(|err| CoreError::invalid(format!("claim is outside the hash domain: {err}")))?;
    let request = json!({
        "project_id": project_id,
        "claim_id": claim_id,
        "expected_revision": expected_revision,
        "status": status,
        "reason": reason,
        "actor": actor,
    });
    let digest = hash::digest_value(&request)
        .map_err(|err| CoreError::invalid(format!("review is outside the hash domain: {err}")))?;
    if let Some((recorded, _, result)) =
        store.idempotency_get(project_id, "reviewClaim", idempotency_key)?
    {
        if recorded != digest {
            return Err(CoreError::conflict(
                "scope_conflict: idempotency_key was used with a different review",
            ));
        }
        return serde_json::from_str(&result)
            .map_err(|err| CoreError::Storage(format!("stored claim result corrupt: {err}")));
    }
    let result = raw_claim_json(&claim)?;
    let revision = latest_revision + 1;
    let tx = store.conn.transaction()?;
    tx.execute(
        "INSERT INTO claims
         (project_id,claim_id,snapshot_id,config_digest,revision,claim_type,text,status,scope,
          producer_json,support_ids_json,counter_ids_json,questions_json,payload_json,created_at)
         VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12,?13,?14,?15)",
        (
            project_id,
            claim_id,
            &claim.snapshot_id,
            &config_digest,
            revision,
            claim.claim_type.as_str(),
            &claim.text,
            claim.status.as_str(),
            &claim.scope,
            serde_json::to_string(&claim.producer).unwrap(),
            serde_json::to_string(&claim.support_ids).unwrap(),
            serde_json::to_string(&claim.counterevidence_ids).unwrap(),
            serde_json::to_string(&claim.unresolved_questions).unwrap(),
            &canonical,
            crate::store::now_unix(),
        ),
    )?;
    tx.execute(
        "INSERT INTO claim_reviews (project_id,claim_id,revision,action,reason,actor_json,created_at)
         VALUES (?1,?2,?3,?4,?5,?6,?7)",
        (
            project_id,
            claim_id,
            revision,
            new_status.as_str(),
            reason,
            actor.to_string(),
            crate::store::now_unix(),
        ),
    )?;
    append_event(
        &tx,
        project_id,
        None,
        Some(&claim.snapshot_id),
        "claim.reviewed",
        json!({"claim_id": claim_id, "status": new_status.as_str(), "revision": revision}),
    )?;
    tx.execute(
        "INSERT INTO idempotency
         (project_id,operation,idempotency_key,request_digest,status_code,result_json,created_at)
         VALUES (?1,'reviewClaim',?2,?3,200,?4,?5)",
        (
            project_id,
            idempotency_key,
            &digest,
            &result.to_string(),
            crate::store::now_unix(),
        ),
    )?;
    tx.commit()?;
    Ok(result)
}

pub fn get_claim(store: &Store, project_id: &str, claim_id: &str) -> Result<Value> {
    validate::require_id("project_id", project_id)?;
    validate::require_id("claim_id", claim_id)?;
    let (_, payload) = latest_claim(store, project_id, claim_id)?
        .ok_or_else(|| CoreError::not_found(format!("claim {claim_id} not found")))?;
    serde_json::from_str(&payload)
        .map_err(|err| CoreError::Storage(format!("claim payload corrupt: {err}")))
}
