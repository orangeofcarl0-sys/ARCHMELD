// Copyright 2026 ARCHMELD contributors
// SPDX-License-Identifier: Apache-2.0

//! Interpretation layer: claims, investigations, and candidate architectures
//! (S07/S12). Nothing in this module writes entities or relations into the
//! evidence ledger. A model/CodeBoarding output is always a candidate; a claim
//! may be rejected by counterevidence and an accepted claim means "used for the
//! current design", not "formally proven".

use crate::error::{CoreError, Result};
use crate::events::append_event;
use crate::hash;
use crate::store::{validate, Store};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::collections::BTreeSet;

const MAX_CLAIM_TEXT: usize = 20_000;
const MAX_COMPONENTS: usize = 5_000;
const MAX_DEPENDENCIES: usize = 20_000;
const MAX_SOURCE_REFERENCES: usize = 10_000;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
enum ClaimType {
    Explanation,
    Hypothesis,
    Requirement,
    Decision,
}

impl ClaimType {
    fn as_str(self) -> &'static str {
        match self {
            Self::Explanation => "explanation",
            Self::Hypothesis => "hypothesis",
            Self::Requirement => "requirement",
            Self::Decision => "decision",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
enum ClaimStatus {
    Proposed,
    Accepted,
    Rejected,
    NeedsReview,
}

impl ClaimStatus {
    fn as_str(self) -> &'static str {
        match self {
            Self::Proposed => "proposed",
            Self::Accepted => "accepted",
            Self::Rejected => "rejected",
            Self::NeedsReview => "needs_review",
        }
    }
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct Claim {
    id: String,
    project_id: String,
    snapshot_id: String,
    claim_type: ClaimType,
    text: String,
    status: ClaimStatus,
    support_ids: Vec<String>,
    counterevidence_ids: Vec<String>,
    scope: String,
    producer: crate::store::Producer,
    unresolved_questions: Vec<String>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
struct ClaimProposal {
    project_id: String,
    claim: Claim,
    expected_revision: i64,
    idempotency_key: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
enum Layer {
    AsIs,
    ToBe,
    Transition,
}

impl Layer {
    fn as_str(self) -> &'static str {
        match self {
            Self::AsIs => "as_is",
            Self::ToBe => "to_be",
            Self::Transition => "transition",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
enum ArchitectureStatus {
    Candidate,
    Reviewed,
    Accepted,
    NeedsReview,
}

impl ArchitectureStatus {
    fn as_str(self) -> &'static str {
        match self {
            Self::Candidate => "candidate",
            Self::Reviewed => "reviewed",
            Self::Accepted => "accepted",
            Self::NeedsReview => "needs_review",
        }
    }
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct ArchitectureComponent {
    id: String,
    name: String,
    purpose: String,
    responsibilities: Vec<String>,
    non_responsibilities: Vec<String>,
    member_entity_ids: Vec<String>,
    owned_state: Vec<String>,
    owned_resources: Vec<String>,
    runtime_placement: Vec<String>,
    evidence_ids: Vec<String>,
    open_questions: Vec<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
enum DependencyStatus {
    Observed,
    Candidate,
    Required,
    Temporary,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct ArchitectureDependency {
    source_id: String,
    target_id: String,
    reason: String,
    evidence_ids: Vec<String>,
    status: DependencyStatus,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct ArchitectureRule {
    id: String,
    kind: String,
    description: String,
    scope_ids: Vec<String>,
    required: bool,
    checker_id: Option<String>,
    checkability: String,
    /// Checker-specific declarative parameters. Optional so existing rule
    /// documents keep their canonical bytes.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    parameters: Option<Value>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct Architecture {
    schema_version: String,
    id: String,
    project_id: String,
    snapshot_id: String,
    revision: i64,
    layer: Layer,
    status: ArchitectureStatus,
    components: Vec<ArchitectureComponent>,
    dependencies: Vec<ArchitectureDependency>,
    rules: Vec<ArchitectureRule>,
    claim_ids: Vec<String>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
struct ArchitectureProposal {
    project_id: String,
    model: Architecture,
    expected_revision: i64,
    idempotency_key: String,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct Investigation {
    investigation_id: String,
    project_id: String,
    snapshot_id: String,
    question: String,
    scope: String,
    status: String,
    facts: Vec<String>,
    hypotheses: Vec<String>,
    missing_evidence: Vec<String>,
    experiments: Vec<String>,
    conclusions: Vec<String>,
    residual_risks: Vec<String>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
struct InvestigationProposal {
    project_id: String,
    investigation: Investigation,
    expected_revision: i64,
    idempotency_key: String,
}

/// The model provider is deliberately optional. This process is fully usable
/// offline; an unconfigured model is reported instead of being simulated.
pub fn provider_capabilities() -> Value {
    json!({
        "manual_modeling": {"state": "available", "requires_model": false},
        "codeboarding_adapter": {"state": "available", "executes_network": false},
        "model": {
            "state": "unconfigured",
            "reason": "no model provider is configured; navigation, manual architecture and existing checks work offline",
        },
        "model_output_policy": "claims and candidate architectures only; commands are stored as data and never executed",
    })
}

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
struct ArchitectureReviewRequest {
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

pub struct CodeBoardingRequest<'a> {
    pub project_id: &'a str,
    pub snapshot_id: &'a str,
    pub expected_revision: i64,
    pub idempotency_key: &'a str,
    pub provider_version: &'a str,
    pub run_id: &'a str,
    pub commands: Vec<String>,
    /// Model interpretation provenance. Never carries an API key.
    pub model: Option<ModelProvenance<'a>>,
}

#[derive(Debug, Clone)]
pub struct ModelProvenance<'a> {
    pub provider: &'a str,
    pub model: &'a str,
    /// Host only, for example `opencode.ai`; no credentials or query string.
    pub base_url_host: &'a str,
    pub calls: u64,
    pub input_tokens: Option<u64>,
    pub output_tokens: Option<u64>,
}

impl ModelProvenance<'_> {
    fn to_json(&self) -> Value {
        json!({
            "provider": self.provider,
            "model": self.model,
            "base_url_host": self.base_url_host,
            "calls": self.calls,
            "input_tokens": self.input_tokens,
            "output_tokens": self.output_tokens,
        })
    }
}

/// Import a real CodeBoarding `analysis.json`. Components, dependencies and
/// source references are mapped to candidates; missing references are recorded
/// as unverified and no ledger entity/relation is created.
pub fn import_codeboarding(
    store: &mut Store,
    request: &CodeBoardingRequest<'_>,
    analysis: &Value,
) -> Result<Value> {
    validate::require_id("project_id", request.project_id)?;
    validate::require_id("idempotency_key", request.idempotency_key)?;
    validate::require_id("run_id", request.run_id)?;
    validate::require_non_empty("provider_version", request.provider_version)?;
    if let Some(model) = &request.model {
        validate::require_non_empty("model.provider", model.provider)?;
        validate::require_non_empty("model.model", model.model)?;
        validate::require_non_empty("model.base_url_host", model.base_url_host)?;
        if model.calls == 0 {
            return Err(CoreError::invalid("model.calls must be >= 1"));
        }
        if model.base_url_host.contains('@')
            || model.base_url_host.contains('/')
            || model.base_url_host.contains('?')
        {
            return Err(CoreError::invalid(
                "model.base_url_host must be a host name without credentials or a path",
            ));
        }
    }
    snapshot_config(store, request.project_id, request.snapshot_id)?;
    let project_revision = project_revision(store, request.project_id)?;
    if request.expected_revision != project_revision {
        return Err(CoreError::conflict(format!(
            "stale_snapshot: expected_revision {} does not match project revision {project_revision}",
            request.expected_revision
        )));
    }
    if analysis["components"].as_array().is_none() {
        return Err(CoreError::invalid(
            "CodeBoarding analysis is missing the components array",
        ));
    }
    if request.commands.len() > 100
        || request.commands.iter().any(|command| command.len() > 10_000)
    {
        return Err(CoreError::invalid(
            "resource_exhausted: provider command list exceeds limits",
        ));
    }
    let envelope = json!({"analysis": analysis, "commands": request.commands});
    let artifact_bytes = serde_json::to_vec(&envelope)
        .map_err(|err| CoreError::Storage(format!("provider envelope encode: {err}")))?;
    let artifact_digest = store.put_blob(&artifact_bytes, "application/json")?;
    let model_json = request.model.as_ref().map(ModelProvenance::to_json);
    let digest = hash::digest_value(&json!({
        "project_id": request.project_id,
        "snapshot_id": request.snapshot_id,
        "analysis": analysis,
        "commands": request.commands,
        "expected_revision": request.expected_revision,
        "model": model_json,
    }))
    .map_err(|err| CoreError::invalid(format!("provider output outside hash domain: {err}")))?;
    if let Some((recorded, _, result)) =
        store.idempotency_get(request.project_id, "importCodeBoarding", request.idempotency_key)?
    {
        if recorded != digest {
            return Err(CoreError::conflict(
                "scope_conflict: idempotency_key was used with different provider output",
            ));
        }
        return serde_json::from_str(&result)
            .map_err(|err| CoreError::Storage(format!("stored architecture corrupt: {err}")));
    }

    let mut components: Vec<ArchitectureComponent> = Vec::new();
    let mut dependencies: Vec<ArchitectureDependency> = Vec::new();
    let mut references: Vec<Value> = Vec::new();
    let mut component_ids: BTreeSet<String> = BTreeSet::new();
    collect_codeboarding_components(
        store,
        request,
        analysis["components"].as_array().unwrap(),
        &mut components,
        &mut dependencies,
        &mut references,
        &mut component_ids,
    )?;
    if components.is_empty() {
        return Err(CoreError::invalid(
            "CodeBoarding analysis produced no candidate components",
        ));
    }
    if components.len() > MAX_COMPONENTS || dependencies.len() > MAX_DEPENDENCIES {
        return Err(CoreError::invalid(
            "resource_exhausted: provider output exceeds component/dependency limits",
        ));
    }
    if let Some(relations) = analysis["components_relations"].as_array() {
        push_codeboarding_relations(relations, &mut dependencies);
    }
    let architecture_id = format!("arch.codeboarding.{}", &digest[7..23]);
    let mut architecture = Architecture {
        schema_version: "1.0".to_string(),
        id: architecture_id,
        project_id: request.project_id.to_string(),
        snapshot_id: request.snapshot_id.to_string(),
        revision: 1,
        layer: Layer::AsIs,
        status: ArchitectureStatus::Candidate,
        components,
        dependencies,
        rules: Vec::new(),
        claim_ids: Vec::new(),
    };
    validate_architecture(store, request.project_id, &architecture)?;
    let mut verification = verify_architecture_references_with_references(
        store,
        &architecture,
        references,
    );
    verification["commands_ignored"] = json!(request.commands.len());
    verification["commands_executed"] = json!(0);
    let mut producer = json!({
        "name": "CodeBoarding",
        "version": request.provider_version,
        "source_ref": "analysis.json (isolated CLI output)",
        "run_id": request.run_id,
    });
    if let Some(model) = &request.model {
        producer["model"] = model.to_json();
        producer["source_ref"] =
            json!("analysis.json (model interpretation; provider and model recorded)");
    }
    insert_architecture(
        store,
        &architecture,
        &producer,
        Some(&artifact_digest),
        request.commands.len(),
        &verification,
        "importCodeBoarding",
        request.idempotency_key,
        &digest,
    )?;
    architecture.revision = 1;
    Ok(serde_json::to_value(&architecture)
        .map_err(|err| CoreError::Storage(format!("architecture encode: {err}")))?)
}

fn collect_codeboarding_components(
    store: &Store,
    request: &CodeBoardingRequest<'_>,
    raw_components: &[Value],
    components: &mut Vec<ArchitectureComponent>,
    dependencies: &mut Vec<ArchitectureDependency>,
    references: &mut Vec<Value>,
    component_ids: &mut BTreeSet<String>,
) -> Result<()> {
    for (index, raw) in raw_components.iter().enumerate() {
        let component_id = raw["component_id"]
            .as_str()
            .filter(|value| !value.is_empty())
            .map(str::to_string)
            .unwrap_or_else(|| format!("generated.{index}"));
        let name = raw["name"]
            .as_str()
            .filter(|value| !value.is_empty())
            .ok_or_else(|| CoreError::invalid("CodeBoarding component has no name"))?;
        let description = raw["description"]
            .as_str()
            .filter(|value| !value.is_empty())
            .unwrap_or(name);
        let candidate_id = format!("cb.{component_id}");
        validate::require_id("architecture.components[].id", &candidate_id)?;
        component_ids.insert(candidate_id.clone());
        let mut member_ids = BTreeSet::new();
        if let Some(entities) = raw["key_entities"].as_array() {
            for entity in entities {
                let Some(file) = entity["reference_file"].as_str() else {
                    references.push(json!({
                        "component_id": candidate_id,
                        "status": "unverified",
                        "reason": "missing reference_file",
                    }));
                    continue;
                };
                let line_start = entity["reference_start_line"].as_u64().unwrap_or(1);
                let line_end = entity["reference_end_line"]
                    .as_u64()
                    .unwrap_or(line_start);
                let qualified = entity["qualified_name"].as_str().unwrap_or("");
                match verify_source_reference(
                    store,
                    request.project_id,
                    request.snapshot_id,
                    &candidate_id,
                    file,
                    line_start,
                    line_end,
                    qualified,
                ) {
                    Ok((status, entity_id)) => {
                        if let Some(entity_id) = entity_id {
                            member_ids.insert(entity_id);
                        }
                        references.push(json!({
                            "component_id": candidate_id,
                            "path": file,
                            "line_start": line_start,
                            "line_end": line_end,
                            "qualified_name": qualified,
                            "status": status,
                        }));
                    }
                    Err(error) => references.push(json!({
                        "component_id": candidate_id,
                        "path": file,
                        "line_start": line_start,
                        "line_end": line_end,
                        "qualified_name": qualified,
                        "status": "unverified",
                        "reason": error.to_string(),
                    })),
                }
            }
        }
        if references.len() > MAX_SOURCE_REFERENCES {
            return Err(CoreError::invalid(
                "resource_exhausted: too many provider source references",
            ));
        }
        components.push(ArchitectureComponent {
            id: candidate_id,
            name: name.to_string(),
            purpose: description.to_string(),
            responsibilities: vec![description.to_string()],
            non_responsibilities: Vec::new(),
            member_entity_ids: member_ids.into_iter().collect(),
            owned_state: Vec::new(),
            owned_resources: Vec::new(),
            runtime_placement: Vec::new(),
            evidence_ids: Vec::new(),
            open_questions: vec![
                "CodeBoarding proposed this boundary; verify ownership and runtime coupling".to_string(),
            ],
        });

        for relation in raw["components_relations"]
            .as_array()
            .map(Vec::as_slice)
            .unwrap_or(&[])
        {
            let source_raw = relation["src_id"].as_str().unwrap_or("");
            let target_raw = relation["dst_id"].as_str().unwrap_or("");
            if source_raw.is_empty() || target_raw.is_empty() {
                continue;
            }
            let reason = relation["relation"]
                .as_str()
                .filter(|value| !value.is_empty())
                .unwrap_or("CodeBoarding candidate relation");
            dependencies.push(ArchitectureDependency {
                source_id: format!("cb.{source_raw}"),
                target_id: format!("cb.{target_raw}"),
                reason: reason.to_string(),
                evidence_ids: Vec::new(),
                status: DependencyStatus::Candidate,
            });
        }
        if let Some(nested) = raw["components"].as_array() {
            collect_codeboarding_components(
                store,
                request,
                nested,
                components,
                dependencies,
                references,
                component_ids,
            )?;
        }
    }
    Ok(())
}

fn push_codeboarding_relations(
    relations: &[Value],
    dependencies: &mut Vec<ArchitectureDependency>,
) {
    for relation in relations {
        let source_raw = relation["src_id"].as_str().unwrap_or("");
        let target_raw = relation["dst_id"].as_str().unwrap_or("");
        if source_raw.is_empty() || target_raw.is_empty() {
            continue;
        }
        let reason = relation["relation"]
            .as_str()
            .filter(|value| !value.is_empty())
            .unwrap_or("CodeBoarding candidate relation");
        dependencies.push(ArchitectureDependency {
            source_id: format!("cb.{source_raw}"),
            target_id: format!("cb.{target_raw}"),
            reason: reason.to_string(),
            evidence_ids: Vec::new(),
            status: DependencyStatus::Candidate,
        });
    }
}

#[allow(clippy::too_many_arguments)]
fn verify_source_reference(
    store: &Store,
    project_id: &str,
    snapshot_id: &str,
    _component_id: &str,
    path: &str,
    line_start: u64,
    line_end: u64,
    qualified_name: &str,
) -> Result<(String, Option<String>)> {
    validate::validate_path("provider.reference_file", path)?;
    let blob: Option<String> = store
        .conn
        .query_row(
            "SELECT blob_digest FROM files WHERE project_id=?1 AND snapshot_id=?2 AND path=?3",
            (project_id, snapshot_id, path),
            |row| row.get(0),
        )
        .ok();
    let Some(blob) = blob else {
        return Ok(("unverified".to_string(), None));
    };
    let bytes = store
        .read_blob(&blob)?
        .ok_or_else(|| CoreError::Storage(format!("snapshot blob {blob} is missing")))?;
    let text = String::from_utf8(bytes)
        .map_err(|_| CoreError::unsupported(format!("{path} is not valid UTF-8")))?;
    let lines = text.lines().count().max(1) as u64;
    if line_start == 0 || line_end < line_start || line_end > lines {
        return Ok(("unverified".to_string(), None));
    }
    let entity_id: Option<String> = store
        .conn
        .query_row(
            "SELECT entity_id FROM entities
             WHERE project_id=?1 AND snapshot_id=?2 AND qualified_name=?3
             ORDER BY entity_id LIMIT 1",
            (project_id, snapshot_id, qualified_name),
            |row| row.get(0),
        )
        .ok();
    Ok((
        if entity_id.is_some() {
            "resolved_with_entity".to_string()
        } else {
            "resolved_source_only".to_string()
        },
        entity_id,
    ))
}

fn verify_architecture_references(store: &Store, architecture: &Architecture) -> Value {
    verify_architecture_references_with_references(store, architecture, Vec::new())
}

fn verify_architecture_references_with_references(
    store: &Store,
    architecture: &Architecture,
    source_references: Vec<Value>,
) -> Value {
    let component_ids: BTreeSet<&str> = architecture
        .components
        .iter()
        .map(|component| component.id.as_str())
        .collect();
    let mut missing_members: Vec<Value> = Vec::new();
    let mut matched_members = 0usize;
    for component in &architecture.components {
        for entity_id in &component.member_entity_ids {
            let exists: Option<String> = store
                .conn
                .query_row(
                    "SELECT entity_id FROM entities
                     WHERE project_id=?1 AND snapshot_id=?2 AND entity_id=?3",
                    (&architecture.project_id, &architecture.snapshot_id, entity_id),
                    |row| row.get(0),
                )
                .ok();
            if exists.is_some() {
                matched_members += 1;
            } else {
                missing_members.push(json!({
                    "component_id": component.id,
                    "entity_id": entity_id,
                    "status": "unverified",
                }));
            }
        }
    }
    let mut missing_evidence: Vec<Value> = Vec::new();
    for component in &architecture.components {
        for evidence_id in &component.evidence_ids {
            let exists: Option<String> = store
                .conn
                .query_row(
                    "SELECT evidence_id FROM evidence
                     WHERE project_id=?1 AND snapshot_id=?2 AND evidence_id=?3",
                    (&architecture.project_id, &architecture.snapshot_id, evidence_id),
                    |row| row.get(0),
                )
                .ok();
            if exists.is_none() {
                missing_evidence.push(json!({
                    "component_id": component.id,
                    "evidence_id": evidence_id,
                    "status": "unverified",
                }));
            }
        }
    }
    for dependency in &architecture.dependencies {
        for evidence_id in &dependency.evidence_ids {
            let exists: Option<String> = store
                .conn
                .query_row(
                    "SELECT evidence_id FROM evidence
                     WHERE project_id=?1 AND snapshot_id=?2 AND evidence_id=?3",
                    (&architecture.project_id, &architecture.snapshot_id, evidence_id),
                    |row| row.get(0),
                )
                .ok();
            if exists.is_none() {
                missing_evidence.push(json!({
                    "source_id": dependency.source_id,
                    "target_id": dependency.target_id,
                    "evidence_id": evidence_id,
                    "status": "unverified",
                }));
            }
        }
    }
    let mut unresolved_dependencies: Vec<Value> = Vec::new();
    for dependency in &architecture.dependencies {
        if !component_ids.contains(dependency.source_id.as_str())
            || !component_ids.contains(dependency.target_id.as_str())
        {
            unresolved_dependencies.push(json!({
                "source_id": dependency.source_id,
                "target_id": dependency.target_id,
                "reason": dependency.reason,
                "status": "unverified",
            }));
        }
    }
    let known_claims: BTreeSet<String> = architecture
        .claim_ids
        .iter()
        .filter(|claim_id| {
            latest_claim(store, &architecture.project_id, claim_id)
                .ok()
                .flatten()
                .is_some()
        })
        .cloned()
        .collect();
    let missing_claims: Vec<&String> = architecture
        .claim_ids
        .iter()
        .filter(|claim_id| !known_claims.contains(*claim_id))
        .collect();
    let declared_only_required: Vec<String> = architecture
        .rules
        .iter()
        .filter(|rule| rule.required && rule.checkability == "declared_only")
        .map(|rule| rule.id.clone())
        .collect();
    let unverified_references = source_references
        .iter()
        .filter(|reference| reference["status"] == "unverified")
        .count();
    let resolved_references = source_references
        .iter()
        .filter(|reference| reference["status"] != "unverified")
        .count();
    json!({
        "candidate_only": true,
        "certain_calls_added": 0,
        "components": architecture.components.len(),
        "dependencies": architecture.dependencies.len(),
        "source_references": {
            "total": source_references.len(),
            "resolved": resolved_references,
            "unverified": unverified_references,
            "details": source_references,
        },
        "member_entities": {
            "matched": matched_members,
            "missing": missing_members,
        },
        "unresolved_dependency_endpoints": unresolved_dependencies,
        "missing_evidence": missing_evidence,
        "missing_claim_ids": missing_claims,
        "blocked_by_declared_only_required_rules": declared_only_required,
        "state": if missing_members.is_empty()
            && unresolved_dependencies.is_empty()
            && missing_claims.is_empty()
            && missing_evidence.is_empty()
            && unverified_references == 0
            && declared_only_required.is_empty()
        {
            "complete"
        } else {
            "partial"
        },
    })
}

#[allow(clippy::too_many_arguments)]
fn insert_architecture(
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

fn validate_claim(store: &Store, claim: &Claim) -> Result<()> {
    validate::require_id("claim.id", &claim.id)?;
    validate::require_id("claim.project_id", &claim.project_id)?;
    validate::require_digest("claim.snapshot_id", &claim.snapshot_id)?;
    validate::require_non_empty("claim.text", &claim.text)?;
    if claim.text.len() > MAX_CLAIM_TEXT {
        return Err(CoreError::invalid("claim.text exceeds 20000 bytes"));
    }
    validate::require_non_empty("claim.scope", &claim.scope)?;
    validate::require_non_empty("claim.producer.name", &claim.producer.name)?;
    validate::require_non_empty("claim.producer.version", &claim.producer.version)?;
    validate::require_non_empty("claim.producer.source_ref", &claim.producer.source_ref)?;
    validate::require_id("claim.producer.run_id", &claim.producer.run_id)?;
    for question in &claim.unresolved_questions {
        validate::require_non_empty("claim.unresolved_questions[]", question)?;
    }
    snapshot_config(store, &claim.project_id, &claim.snapshot_id)?;
    for id in claim.support_ids.iter().chain(claim.counterevidence_ids.iter()) {
        validate::require_id("claim.support_ids[]", id)?;
        if id == &claim.id {
            return Err(CoreError::invalid(
                "a claim cannot support or counter itself",
            ));
        }
        evidence_or_claim(store, &claim.project_id, &claim.snapshot_id, id)?;
    }
    Ok(())
}

fn evidence_or_claim(store: &Store, project_id: &str, snapshot_id: &str, id: &str) -> Result<()> {
    let evidence: Option<String> = store
        .conn
        .query_row(
            "SELECT evidence_id FROM evidence
             WHERE project_id=?1 AND snapshot_id=?2 AND evidence_id=?3",
            (project_id, snapshot_id, id),
            |row| row.get(0),
        )
        .ok();
    if evidence.is_some() {
        return Ok(());
    }
    let claim: Option<String> = store
        .conn
        .query_row(
            "SELECT claim_id FROM claims
             WHERE project_id=?1 AND snapshot_id=?2 AND claim_id=?3",
            (project_id, snapshot_id, id),
            |row| row.get(0),
        )
        .ok();
    if claim.is_some() {
        return Ok(());
    }
    Err(CoreError::invalid(format!(
        "claim reference {id} does not exist as evidence or a claim in snapshot {snapshot_id}"
    )))
}

fn validate_investigation(store: &Store, investigation: &Investigation) -> Result<()> {
    validate::require_id("investigation_id", &investigation.investigation_id)?;
    validate::require_id("project_id", &investigation.project_id)?;
    validate::require_digest("investigation.snapshot_id", &investigation.snapshot_id)?;
    validate::require_non_empty("investigation.question", &investigation.question)?;
    validate::require_non_empty("investigation.scope", &investigation.scope)?;
    match investigation.status.as_str() {
        "open" | "resolved" | "abandoned" => {}
        other => {
            return Err(CoreError::invalid(format!(
                "unsupported investigation status {other}"
            )))
        }
    }
    snapshot_config(store, &investigation.project_id, &investigation.snapshot_id)?;
    for fact in &investigation.facts {
        validate::require_id("investigation.facts[]", fact)?;
        evidence_or_claim(store, &investigation.project_id, &investigation.snapshot_id, fact)?;
    }
    for claim in investigation
        .hypotheses
        .iter()
        .chain(investigation.conclusions.iter())
    {
        validate::require_id("investigation.claim_ids[]", claim)?;
        evidence_or_claim(store, &investigation.project_id, &investigation.snapshot_id, claim)?;
    }
    for text in investigation
        .missing_evidence
        .iter()
        .chain(investigation.experiments.iter())
        .chain(investigation.residual_risks.iter())
    {
        validate::require_non_empty("investigation.text[]", text)?;
    }
    Ok(())
}

fn validate_architecture(store: &Store, project_id: &str, architecture: &Architecture) -> Result<()> {
    if architecture.schema_version != "1.0" {
        return Err(CoreError::invalid("architecture schema_version must be 1.0"));
    }
    validate::require_id("architecture.id", &architecture.id)?;
    validate::require_id("architecture.project_id", &architecture.project_id)?;
    validate::require_digest("architecture.snapshot_id", &architecture.snapshot_id)?;
    if architecture.project_id != project_id {
        return Err(CoreError::invalid(
            "architecture.project_id does not match the proposal project_id",
        ));
    }
    snapshot_config(store, project_id, &architecture.snapshot_id)?;
    if architecture.revision < 1 {
        return Err(CoreError::invalid("architecture.revision must be >= 1"));
    }
    if architecture.components.is_empty() || architecture.components.len() > MAX_COMPONENTS {
        return Err(CoreError::invalid(
            "architecture.components must contain 1..=5000 components",
        ));
    }
    if architecture.dependencies.len() > MAX_DEPENDENCIES {
        return Err(CoreError::invalid(
            "architecture.dependencies exceeds 20000 entries",
        ));
    }
    let mut ids = BTreeSet::new();
    for component in &architecture.components {
        validate::require_id("architecture.components[].id", &component.id)?;
        if !ids.insert(component.id.clone()) {
            return Err(CoreError::invalid(format!(
                "duplicate architecture component id {}",
                component.id
            )));
        }
        validate::require_non_empty("component.name", &component.name)?;
        validate::require_non_empty("component.purpose", &component.purpose)?;
        if component.responsibilities.is_empty() {
            return Err(CoreError::invalid(
                "component.responsibilities must not be empty",
            ));
        }
        for entity_id in &component.member_entity_ids {
            validate::require_id("component.member_entity_ids[]", entity_id)?;
        }
        for evidence_id in &component.evidence_ids {
            validate::require_id("component.evidence_ids[]", evidence_id)?;
        }
    }
    for dependency in &architecture.dependencies {
        validate::require_id("dependency.source_id", &dependency.source_id)?;
        validate::require_id("dependency.target_id", &dependency.target_id)?;
        validate::require_non_empty("dependency.reason", &dependency.reason)?;
        if dependency.status == DependencyStatus::Observed {
            return Err(CoreError::invalid(
                "proposal-only: a candidate architecture cannot declare observed dependencies",
            ));
        }
        if dependency.status != DependencyStatus::Candidate {
            return Err(CoreError::invalid(
                "proposal-only: imported dependencies must remain candidate",
            ));
        }
        for evidence_id in &dependency.evidence_ids {
            validate::require_id("dependency.evidence_ids[]", evidence_id)?;
        }
    }
    for rule in &architecture.rules {
        validate::require_id("rule.id", &rule.id)?;
        validate::require_non_empty("rule.description", &rule.description)?;
        if !matches!(
            rule.kind.as_str(),
            "forbid_dependency"
                | "allowed_layers"
                | "acyclic"
                | "interface_only"
                | "ownership"
                | "custom_contract"
        ) {
            return Err(CoreError::invalid(format!(
                "unsupported architecture rule kind {}",
                rule.kind
            )));
        }
        if !matches!(rule.checkability.as_str(), "executable" | "declared_only") {
            return Err(CoreError::invalid(format!(
                "unsupported rule checkability {}",
                rule.checkability
            )));
        }
        if rule.checkability == "executable" && rule.checker_id.as_deref().unwrap_or("").is_empty()
        {
            return Err(CoreError::invalid(
                "an executable rule requires a checker_id",
            ));
        }
        if rule.scope_ids.is_empty() {
            return Err(CoreError::invalid("rule.scope_ids must not be empty"));
        }
        if let Some(parameters) = &rule.parameters {
            let object = parameters
                .as_object()
                .ok_or_else(|| CoreError::invalid("rule.parameters must be an object"))?;
            if object.len() > 64 {
                return Err(CoreError::invalid(
                    "resource_exhausted: rule.parameters has more than 64 keys",
                ));
            }
            let encoded = serde_json::to_vec(parameters)
                .map_err(|err| CoreError::invalid(format!("rule.parameters encode: {err}")))?;
            if encoded.len() > 64 * 1024 {
                return Err(CoreError::invalid(
                    "resource_exhausted: rule.parameters exceeds 64 KiB",
                ));
            }
        }
    }
    for claim_id in &architecture.claim_ids {
        validate::require_id("architecture.claim_ids[]", claim_id)?;
    }
    Ok(())
}

fn raw_claim_json(claim: &Claim) -> Result<Value> {
    serde_json::to_value(claim).map_err(|err| CoreError::Storage(format!("claim encode: {err}")))
}

fn snapshot_config(store: &Store, project_id: &str, snapshot_id: &str) -> Result<String> {
    store
        .conn
        .query_row(
            "SELECT config_digest FROM snapshots WHERE project_id=?1 AND snapshot_id=?2",
            (project_id, snapshot_id),
            |row| row.get(0),
        )
        .map_err(|_| CoreError::not_found(format!("snapshot {snapshot_id} not found in project")))
}

fn project_revision(store: &Store, project_id: &str) -> Result<i64> {
    let revision: i64 = store
        .conn
        .query_row(
            "SELECT revision FROM projects WHERE project_id=?1",
            (project_id,),
            |row| row.get(0),
        )
        .map_err(|_| CoreError::not_found(format!("project {project_id} not found")))?;
    Ok(revision.max(1))
}

fn latest_claim(store: &Store, project_id: &str, claim_id: &str) -> Result<Option<(i64, String)>> {
    Ok(store
        .conn
        .query_row(
            "SELECT revision, payload_json FROM claims
             WHERE project_id=?1 AND claim_id=?2 ORDER BY revision DESC LIMIT 1",
            (project_id, claim_id),
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .ok())
}

fn latest_investigation(
    store: &Store,
    project_id: &str,
    investigation_id: &str,
) -> Result<Option<(i64, String)>> {
    Ok(store
        .conn
        .query_row(
            "SELECT revision, payload_json FROM investigations
             WHERE project_id=?1 AND investigation_id=?2 ORDER BY revision DESC LIMIT 1",
            (project_id, investigation_id),
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .ok())
}

fn latest_architecture(
    store: &Store,
    project_id: &str,
    architecture_id: &str,
) -> Result<Option<(i64, String)>> {
    Ok(store
        .conn
        .query_row(
            "SELECT revision, payload_json FROM architectures
             WHERE project_id=?1 AND architecture_id=?2 ORDER BY revision DESC LIMIT 1",
            (project_id, architecture_id),
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .ok())
}
