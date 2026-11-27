// Copyright 2026 ARCHMELD contributors
// SPDX-License-Identifier: Apache-2.0

//! Investigation proposals and lookup.

use super::*;

pub fn propose_investigation(store: &mut Store, raw: &Value) -> Result<Value> {
    let proposal: InvestigationProposal = serde_json::from_value(raw.clone()).map_err(|err| {
        CoreError::invalid(format!("InvestigationProposal does not match: {err}"))
    })?;
    validate::require_id("project_id", &proposal.project_id)?;
    validate::require_id("idempotency_key", &proposal.idempotency_key)?;
    let investigation = &proposal.investigation;
    if investigation.project_id != proposal.project_id {
        return Err(CoreError::invalid(
            "investigation.project_id does not match the proposal project_id",
        ));
    }
    validate_investigation(store, investigation)?;
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
        store.idempotency_get(&proposal.project_id, "proposeInvestigation", &proposal.idempotency_key)?
    {
        if recorded != digest {
            return Err(CoreError::conflict(
                "scope_conflict: idempotency_key was used with a different investigation",
            ));
        }
        return serde_json::from_str(&result)
            .map_err(|err| CoreError::Storage(format!("stored investigation corrupt: {err}")));
    }
    let latest = latest_investigation(store, &investigation.project_id, &investigation.investigation_id)?;
    if latest.is_some() {
        return Err(CoreError::conflict(format!(
            "investigation {} already exists; revise it instead",
            investigation.investigation_id
        )));
    }
    let config_digest = snapshot_config(store, &investigation.project_id, &investigation.snapshot_id)?;
    let result = serde_json::to_value(investigation)
        .map_err(|err| CoreError::Storage(format!("investigation encode: {err}")))?;
    let payload_json = hash::canonical_json(&result)
        .map_err(|err| CoreError::invalid(format!("investigation is outside the hash domain: {err}")))?;
    let tx = store.conn.transaction()?;
    tx.execute(
        "INSERT INTO investigations
         (project_id,investigation_id,snapshot_id,config_digest,revision,question,scope,status,
          facts_json,hypotheses_json,missing_json,experiments_json,conclusions_json,residual_json,
          payload_json,created_at)
         VALUES (?1,?2,?3,?4,1,?5,?6,?7,?8,?9,?10,?11,?12,?13,?14,?15)",
        (
            &investigation.project_id,
            &investigation.investigation_id,
            &investigation.snapshot_id,
            &config_digest,
            &investigation.question,
            &investigation.scope,
            &investigation.status,
            serde_json::to_string(&investigation.facts).unwrap(),
            serde_json::to_string(&investigation.hypotheses).unwrap(),
            serde_json::to_string(&investigation.missing_evidence).unwrap(),
            serde_json::to_string(&investigation.experiments).unwrap(),
            serde_json::to_string(&investigation.conclusions).unwrap(),
            serde_json::to_string(&investigation.residual_risks).unwrap(),
            &payload_json,
            crate::store::now_unix(),
        ),
    )?;
    append_event(
        &tx,
        &investigation.project_id,
        None,
        Some(&investigation.snapshot_id),
        "investigation.proposed",
        json!({"investigation_id": investigation.investigation_id}),
    )?;
    tx.execute(
        "INSERT INTO idempotency
         (project_id,operation,idempotency_key,request_digest,status_code,result_json,created_at)
         VALUES (?1,'proposeInvestigation',?2,?3,200,?4,?5)",
        (
            &investigation.project_id,
            &proposal.idempotency_key,
            &digest,
            &result.to_string(),
            crate::store::now_unix(),
        ),
    )?;
    tx.commit()?;
    Ok(result)
}

pub fn get_investigation(
    store: &Store,
    project_id: &str,
    investigation_id: &str,
) -> Result<Value> {
    validate::require_id("project_id", project_id)?;
    validate::require_id("investigation_id", investigation_id)?;
    let (_, payload) = latest_investigation(store, project_id, investigation_id)?
        .ok_or_else(|| {
            CoreError::not_found(format!("investigation {investigation_id} not found"))
        })?;
    serde_json::from_str(&payload)
        .map_err(|err| CoreError::Storage(format!("investigation payload corrupt: {err}")))
}
