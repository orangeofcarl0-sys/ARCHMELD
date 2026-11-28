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
struct RuleSource {
    path: String,
    line: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    digest: Option<String>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct ArchitectureException {
    id: String,
    violation_key: String,
    reason: String,
    introduced_revision: i64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    expires_revision: Option<i64>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    scope_ids: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    source: Option<RuleSource>,
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
    /// Optional rule provenance. Omitted rules keep their previous canonical
    /// bytes.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    source: Option<RuleSource>,
    /// Accepted exceptions. Omitted rules keep their previous canonical bytes.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    exceptions: Vec<ArchitectureException>,
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


mod claims;
mod investigations;
mod architecture;
mod codeboarding;

pub use claims::{get_claim, propose_claim, review_claim};
pub use investigations::{get_investigation, propose_investigation};
pub(crate) use architecture::architecture_at_revision;
pub use architecture::{
    architecture_producer, architecture_review, get_architecture, list_architecture_reviews,
    propose_architecture, review_architecture, validate_architecture_document,
    ArchitectureReviewRequest,
};
pub use codeboarding::{import_codeboarding, CodeBoardingRequest, ModelProvenance};


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

fn validate_rule_source(label: &str, source: &RuleSource) -> Result<()> {
    validate::validate_path(label, &source.path)?;
    if source.line == 0 {
        return Err(CoreError::invalid(format!("{label}.line must be >= 1")));
    }
    if let Some(digest) = &source.digest {
        validate::require_digest(&format!("{label}.digest"), digest)?;
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
    let mut rule_ids = std::collections::BTreeSet::new();
    for rule in &architecture.rules {
        validate::require_id("rule.id", &rule.id)?;
        if !rule_ids.insert(rule.id.as_str()) {
            return Err(CoreError::invalid(format!(
                "duplicate rule id {}",
                rule.id
            )));
        }
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
        if let Some(source) = &rule.source {
            validate_rule_source("rule.source", source)?;
        }
        if rule.exceptions.len() > 100 {
            return Err(CoreError::invalid(
                "resource_exhausted: a rule has more than 100 exceptions",
            ));
        }
        let mut exception_ids = std::collections::BTreeSet::new();
        let mut exception_keys = std::collections::BTreeSet::new();
        for exception in &rule.exceptions {
            validate::require_id("rule.exception.id", &exception.id)?;
            if !exception_ids.insert(exception.id.as_str()) {
                return Err(CoreError::invalid(format!(
                    "duplicate rule exception id {}",
                    exception.id
                )));
            }
            validate::require_non_empty("rule.exception.violation_key", &exception.violation_key)?;
            if !exception_keys.insert(exception.violation_key.as_str()) {
                return Err(CoreError::invalid(format!(
                    "duplicate rule exception violation_key {}",
                    exception.violation_key
                )));
            }
            validate::require_non_empty("rule.exception.reason", &exception.reason)?;
            if exception.introduced_revision < 1 {
                return Err(CoreError::invalid(
                    "rule.exception.introduced_revision must be >= 1",
                ));
            }
            if exception
                .expires_revision
                .is_some_and(|expires| expires < exception.introduced_revision)
            {
                return Err(CoreError::invalid(
                    "rule.exception.expires_revision must be >= introduced_revision",
                ));
            }
            for scope_id in &exception.scope_ids {
                validate::require_id("rule.exception.scope_ids[]", scope_id)?;
            }
            if let Some(source) = &exception.source {
                validate_rule_source("rule.exception.source", source)?;
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
