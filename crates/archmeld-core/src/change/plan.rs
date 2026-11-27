// Copyright 2026 ARCHMELD contributors
// SPDX-License-Identifier: Apache-2.0

//! Frozen behavior contracts and plan DAGs (S09/S10). A plan is not a
//! ChangeSet: it never mutates source and no staging row is created here.

use crate::error::{CoreError, Result};
use crate::events::append_event;
use crate::hash;
use crate::store::{validate, Store};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::collections::{BTreeMap, BTreeSet};

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct Check {
    id: String,
    kind: String,
    required: bool,
    description: String,
    command: Option<Value>,
    checker_id: Option<String>,
    predicate: Value,
    input_artifacts: Vec<String>,
    /// Optional per-check filesystem sandbox. Omitted for contracts frozen
    /// before the field existed; serialization skips it so their digests stay
    /// stable.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    sandbox: Option<CheckSandbox>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct CheckSandbox {
    /// `workspace_copy` materializes the verified snapshot into a temporary
    /// directory; `workspace` explicitly declares no isolation; `container`
    /// runs the command in an OCI container over that copy.
    mode: String,
    #[serde(default)]
    include: Vec<String>,
    #[serde(default)]
    exclude: Vec<String>,
    /// Pinned container image (`name@sha256:...` or a local `sha256:<id>`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    image: Option<String>,
    /// Only `none` is accepted; no network namespace access.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    network: Option<String>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct BehaviorContract {
    schema_version: String,
    id: String,
    project_id: String,
    revision: i64,
    kind: String,
    invariants: Vec<String>,
    allowed_changes: Vec<String>,
    known_defects: Vec<String>,
    checks: Vec<Check>,
    digest: String,
    frozen: bool,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
struct ContractProposal {
    project_id: String,
    contract: BehaviorContract,
    expected_revision: i64,
    idempotency_key: String,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct PlanStep {
    id: String,
    title: String,
    depends_on: Vec<String>,
    allowed_paths: Vec<String>,
    required_check_ids: Vec<String>,
    completion_criteria: Vec<String>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct Plan {
    schema_version: String,
    id: String,
    project_id: String,
    revision: i64,
    kind: String,
    objective: String,
    non_goals: Vec<String>,
    base_snapshot_id: String,
    target_architecture_id: String,
    contract_id: String,
    contract_digest: String,
    steps: Vec<PlanStep>,
    risks: Vec<String>,
    evidence_ids: Vec<String>,
    status: String,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
struct PlanProposal {
    project_id: String,
    plan: Plan,
    expected_revision: i64,
    idempotency_key: String,
}

pub fn propose_contract(store: &mut Store, raw: &Value) -> Result<Value> {
    let proposal: ContractProposal = serde_json::from_value(raw.clone()).map_err(|err| {
        CoreError::invalid(format!("ContractProposal does not match the contract: {err}"))
    })?;
    validate::require_id("project_id", &proposal.project_id)?;
    validate::require_id("idempotency_key", &proposal.idempotency_key)?;
    let contract = &proposal.contract;
    if contract.project_id != proposal.project_id {
        return Err(CoreError::invalid(
            "contract.project_id does not match the proposal project_id",
        ));
    }
    validate_contract(contract)?;
    if contract.frozen {
        return Err(CoreError::invalid(
            "a contract proposal must be unfrozen; freezing happens on acceptance",
        ));
    }
    let revision = latest_contract(store, &contract.project_id, &contract.id)?
        .map(|(revision, _)| revision + 1)
        .unwrap_or(1);
    if contract.revision != revision {
        return Err(CoreError::conflict(format!(
            "stale_snapshot: contract revision must be {revision}, got {}",
            contract.revision
        )));
    }
    let computed = contract_digest(contract)?;
    if computed != contract.digest {
        return Err(CoreError::invalid(format!(
            "contract_digest is {} but the supplied contract hashes to {computed}",
            contract.digest
        )));
    }
    let project_revision = project_revision(store, &proposal.project_id)?;
    if proposal.expected_revision != project_revision {
        return Err(CoreError::conflict(format!(
            "stale_snapshot: expected_revision {} does not match project revision {project_revision}",
            proposal.expected_revision
        )));
    }
    let digest = hash::digest_value(raw)
        .map_err(|err| CoreError::invalid(format!("contract proposal is outside the hash domain: {err}")))?;
    if let Some((recorded, _, result)) =
        store.idempotency_get(&proposal.project_id, "proposeContract", &proposal.idempotency_key)?
    {
        if recorded != digest {
            return Err(CoreError::conflict(
                "scope_conflict: idempotency_key was used with a different contract",
            ));
        }
        return serde_json::from_str(&result)
            .map_err(|err| CoreError::Storage(format!("stored contract corrupt: {err}")));
    }
    let mut stored = contract.clone();
    stored.revision = revision;
    stored.frozen = true;
    let payload = serde_json::to_value(&stored)
        .map_err(|err| CoreError::Storage(format!("contract encode: {err}")))?;
    let payload_json = hash::canonical_json(&payload)
        .map_err(|err| CoreError::invalid(format!("contract is outside the hash domain: {err}")))?;
    let result = serde_json::to_string(&payload)
        .map_err(|err| CoreError::Storage(format!("contract encode: {err}")))?;
    let tx = store.conn.transaction()?;
    tx.execute(
        "INSERT INTO behavior_contracts
         (project_id,contract_id,revision,kind,digest,frozen,payload_json,created_at)
         VALUES (?1,?2,?3,?4,?5,1,?6,?7)",
        (
            &stored.project_id,
            &stored.id,
            revision,
            &stored.kind,
            &computed,
            &payload_json,
            crate::store::now_unix(),
        ),
    )?;
    append_event(
        &tx,
        &stored.project_id,
        None,
        None,
        "contract.frozen",
        json!({"contract_id": stored.id, "revision": revision, "digest": computed}),
    )?;
    tx.execute(
        "INSERT INTO idempotency
         (project_id,operation,idempotency_key,request_digest,status_code,result_json,created_at)
         VALUES (?1,'proposeContract',?2,?3,200,?4,?5)",
        (
            &stored.project_id,
            &proposal.idempotency_key,
            &digest,
            &result,
            crate::store::now_unix(),
        ),
    )?;
    tx.commit()?;
    Ok(payload)
}

pub fn get_contract(store: &Store, project_id: &str, contract_id: &str) -> Result<Value> {
    validate::require_id("project_id", project_id)?;
    validate::require_id("contract_id", contract_id)?;
    let (_, payload) = latest_contract(store, project_id, contract_id)?
        .ok_or_else(|| CoreError::not_found(format!("contract {contract_id} not found")))?;
    serde_json::from_str(&payload)
        .map_err(|err| CoreError::Storage(format!("contract payload corrupt: {err}")))
}

pub fn propose_plan(store: &mut Store, raw: &Value) -> Result<Value> {
    let proposal: PlanProposal = serde_json::from_value(raw.clone())
        .map_err(|err| CoreError::invalid(format!("PlanProposal does not match the contract: {err}")))?;
    validate::require_id("project_id", &proposal.project_id)?;
    validate::require_id("idempotency_key", &proposal.idempotency_key)?;
    let plan = &proposal.plan;
    if plan.project_id != proposal.project_id {
        return Err(CoreError::invalid(
            "plan.project_id does not match the proposal project_id",
        ));
    }
    if plan.schema_version != "1.0" {
        return Err(CoreError::invalid("plan.schema_version must be 1.0"));
    }
    validate::require_id("plan.id", &plan.id)?;
    validate::require_digest("plan.base_snapshot_id", &plan.base_snapshot_id)?;
    validate::require_digest("plan.contract_digest", &plan.contract_digest)?;
    validate::require_id("plan.target_architecture_id", &plan.target_architecture_id)?;
    validate::require_id("plan.contract_id", &plan.contract_id)?;
    validate::require_non_empty("plan.objective", &plan.objective)?;
    for non_goal in &plan.non_goals {
        validate::require_non_empty("plan.non_goals[]", non_goal)?;
    }
    for risk in &plan.risks {
        validate::require_non_empty("plan.risks[]", risk)?;
    }
    for evidence_id in &plan.evidence_ids {
        validate::require_id("plan.evidence_ids[]", evidence_id)?;
    }
    if !matches!(
        plan.kind.as_str(),
        "structural_refactor" | "behavior_fix" | "architecture_migration"
    ) {
        return Err(CoreError::invalid(format!("unsupported plan kind {}", plan.kind)));
    }
    if plan.status != "draft" {
        return Err(CoreError::invalid(
            "a new plan must start in draft status; approvals create a new revision",
        ));
    }
    if plan.steps.is_empty() {
        return Err(CoreError::invalid("plan.steps must not be empty"));
    }
    snapshot_exists(store, &plan.project_id, &plan.base_snapshot_id)?;
    let architecture_snapshot: Option<String> = store
        .conn
        .query_row(
            "SELECT snapshot_id FROM architectures
             WHERE project_id=?1 AND architecture_id=?2 ORDER BY revision DESC LIMIT 1",
            (&plan.project_id, &plan.target_architecture_id),
            |row| row.get(0),
        )
        .ok();
    let Some(architecture_snapshot) = architecture_snapshot else {
        return Err(CoreError::not_found(format!(
            "target architecture {} does not exist",
            plan.target_architecture_id
        )));
    };
    if architecture_snapshot != plan.base_snapshot_id {
        return Err(CoreError::conflict(
            "target architecture is bound to a different snapshot than the plan base",
        ));
    }
    let contract = get_contract(store, &plan.project_id, &plan.contract_id)?;
    if contract["digest"] != plan.contract_digest {
        return Err(CoreError::conflict(
            "stale_contract: plan.contract_digest does not match the frozen contract",
        ));
    }
    let check_ids: BTreeSet<&str> = contract["checks"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|check| check["id"].as_str())
        .collect();
    let mut step_ids = BTreeSet::new();
    let mut dependencies: BTreeMap<String, Vec<String>> = BTreeMap::new();
    for step in &plan.steps {
        validate::require_id("step.id", &step.id)?;
        if !step_ids.insert(step.id.clone()) {
            return Err(CoreError::invalid(format!("duplicate plan step id {}", step.id)));
        }
        validate::require_non_empty("step.title", &step.title)?;
        if step.allowed_paths.is_empty() {
            return Err(CoreError::invalid(format!(
                "step {} has no allowed_paths",
                step.id
            )));
        }
        for path in &step.allowed_paths {
            validate::validate_path("step.allowed_paths[]", path)?;
        }
        if step.required_check_ids.is_empty() {
            return Err(CoreError::invalid(format!(
                "step {} has no required_check_ids",
                step.id
            )));
        }
        for check_id in &step.required_check_ids {
            validate::require_id("step.required_check_ids[]", check_id)?;
            if !check_ids.contains(check_id.as_str()) {
                return Err(CoreError::invalid(format!(
                    "step {} references unknown check_id {check_id}",
                    step.id
                )));
            }
        }
        if step.completion_criteria.is_empty() {
            return Err(CoreError::invalid(format!(
                "step {} has no completion_criteria",
                step.id
            )));
        }
        for criterion in &step.completion_criteria {
            validate::require_non_empty("step.completion_criteria[]", criterion)?;
        }
        dependencies.insert(step.id.clone(), step.depends_on.clone());
    }
    for (step_id, depends_on) in &dependencies {
        for dependency in depends_on {
            if dependency == step_id {
                return Err(CoreError::invalid(format!(
                    "step {step_id} depends on itself"
                )));
            }
            if !step_ids.contains(dependency) {
                return Err(CoreError::invalid(format!(
                    "step {step_id} depends on unknown step {dependency}"
                )));
            }
        }
    }
    detect_cycles(&dependencies)?;
    let project_revision = project_revision(store, &plan.project_id)?;
    if proposal.expected_revision != project_revision {
        return Err(CoreError::conflict(format!(
            "stale_snapshot: expected_revision {} does not match project revision {project_revision}",
            proposal.expected_revision
        )));
    }
    let digest = hash::digest_value(raw)
        .map_err(|err| CoreError::invalid(format!("plan proposal is outside the hash domain: {err}")))?;
    if let Some((recorded, _, result)) =
        store.idempotency_get(&plan.project_id, "proposePlan", &proposal.idempotency_key)?
    {
        if recorded != digest {
            return Err(CoreError::conflict(
                "scope_conflict: idempotency_key was used with a different plan",
            ));
        }
        return serde_json::from_str(&result)
            .map_err(|err| CoreError::Storage(format!("stored plan corrupt: {err}")));
    }
    let revision = latest_plan(store, &plan.project_id, &plan.id)?
        .map(|(revision, _)| revision + 1)
        .unwrap_or(1);
    let mut stored = plan.clone();
    stored.revision = revision;
    let payload = serde_json::to_value(&stored)
        .map_err(|err| CoreError::Storage(format!("plan encode: {err}")))?;
    let payload_json = hash::canonical_json(&payload)
        .map_err(|err| CoreError::invalid(format!("plan is outside the hash domain: {err}")))?;
    let result = serde_json::to_string(&payload)
        .map_err(|err| CoreError::Storage(format!("plan encode: {err}")))?;
    let tx = store.conn.transaction()?;
    tx.execute(
        "INSERT INTO plans
         (project_id,plan_id,revision,contract_id,contract_digest,target_architecture_id,
          base_snapshot_id,status,payload_json,created_at)
         VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9,?10)",
        (
            &stored.project_id,
            &stored.id,
            revision,
            &stored.contract_id,
            &stored.contract_digest,
            &stored.target_architecture_id,
            &stored.base_snapshot_id,
            &stored.status,
            &payload_json,
            crate::store::now_unix(),
        ),
    )?;
    append_event(
        &tx,
        &stored.project_id,
        None,
        Some(&stored.base_snapshot_id),
        "plan.proposed",
        json!({"plan_id": stored.id, "revision": revision, "status": stored.status}),
    )?;
    tx.execute(
        "INSERT INTO idempotency
         (project_id,operation,idempotency_key,request_digest,status_code,result_json,created_at)
         VALUES (?1,'proposePlan',?2,?3,200,?4,?5)",
        (
            &stored.project_id,
            &proposal.idempotency_key,
            &digest,
            &result,
            crate::store::now_unix(),
        ),
    )?;
    tx.commit()?;
    Ok(payload)
}

pub fn approve_plan(
    store: &mut Store,
    project_id: &str,
    plan_id: &str,
    expected_revision: i64,
    contract_digest: &str,
    actor: &str,
    idempotency_key: &str,
) -> Result<Value> {
    validate::require_id("project_id", project_id)?;
    validate::require_id("plan_id", plan_id)?;
    validate::require_id("idempotency_key", idempotency_key)?;
    validate::require_non_empty("actor", actor)?;
    validate::require_digest("contract_digest", contract_digest)?;
    let (revision, payload) = latest_plan(store, project_id, plan_id)?
        .ok_or_else(|| CoreError::not_found(format!("plan {plan_id} not found")))?;
    if expected_revision != revision {
        return Err(CoreError::conflict(format!(
            "stale_snapshot: expected plan revision {expected_revision} does not match {revision}"
        )));
    }
    let mut plan: Plan = serde_json::from_str(&payload)
        .map_err(|err| CoreError::Storage(format!("plan payload corrupt: {err}")))?;
    if plan.status != "draft" {
        return Err(CoreError::conflict(format!(
            "plan {plan_id} is {} and cannot be approved again",
            plan.status
        )));
    }
    let contract = get_contract(store, project_id, &plan.contract_id)?;
    if plan.contract_digest != contract_digest {
        return Err(CoreError::conflict(
            "contract_digest does not match the plan's frozen contract",
        ));
    }
    if contract["digest"] != contract_digest {
        return Err(CoreError::conflict(
            "stale_contract: the frozen contract changed; the plan must be re-proposed",
        ));
    }
    let request = json!({
        "project_id": project_id,
        "plan_id": plan_id,
        "expected_revision": expected_revision,
        "contract_digest": contract_digest,
        "actor": actor,
    });
    let digest = hash::digest_value(&request)
        .map_err(|err| CoreError::invalid(format!("approval is outside the hash domain: {err}")))?;
    if let Some((recorded, _, result)) =
        store.idempotency_get(project_id, "approvePlan", idempotency_key)?
    {
        if recorded != digest {
            return Err(CoreError::conflict(
                "scope_conflict: idempotency_key was used with a different approval",
            ));
        }
        return serde_json::from_str(&result)
            .map_err(|err| CoreError::Storage(format!("stored plan corrupt: {err}")));
    }
    plan.status = "approved".to_string();
    plan.revision = revision + 1;
    let new_revision = plan.revision;
    let payload = serde_json::to_value(&plan)
        .map_err(|err| CoreError::Storage(format!("plan encode: {err}")))?;
    let payload_json = hash::canonical_json(&payload)
        .map_err(|err| CoreError::invalid(format!("plan is outside the hash domain: {err}")))?;
    let result = serde_json::to_string(&payload)
        .map_err(|err| CoreError::Storage(format!("plan encode: {err}")))?;
    let tx = store.conn.transaction()?;
    tx.execute(
        "INSERT INTO plans
         (project_id,plan_id,revision,contract_id,contract_digest,target_architecture_id,
          base_snapshot_id,status,payload_json,created_at)
         VALUES (?1,?2,?3,?4,?5,?6,?7,'approved',?8,?9)",
        (
            project_id,
            plan_id,
            new_revision,
            &plan.contract_id,
            &plan.contract_digest,
            &plan.target_architecture_id,
            &plan.base_snapshot_id,
            &payload_json,
            crate::store::now_unix(),
        ),
    )?;
    tx.execute(
        "INSERT INTO plan_approvals
         (project_id,plan_id,revision,contract_digest,actor,decision,created_at)
         VALUES (?1,?2,?3,?4,?5,'approve',?6)",
        (
            project_id,
            plan_id,
            new_revision,
            contract_digest,
            actor,
            crate::store::now_unix(),
        ),
    )?;
    append_event(
        &tx,
        project_id,
        None,
        Some(&plan.base_snapshot_id),
        "plan.approved",
        json!({"plan_id": plan_id, "revision": new_revision, "contract_digest": contract_digest}),
    )?;
    tx.execute(
        "INSERT INTO idempotency
         (project_id,operation,idempotency_key,request_digest,status_code,result_json,created_at)
         VALUES (?1,'approvePlan',?2,?3,200,?4,?5)",
        (
            project_id,
            idempotency_key,
            &digest,
            &result,
            crate::store::now_unix(),
        ),
    )?;
    tx.commit()?;
    Ok(payload)
}

pub fn get_plan(store: &Store, project_id: &str, plan_id: &str) -> Result<Value> {
    validate::require_id("project_id", project_id)?;
    validate::require_id("plan_id", plan_id)?;
    let (_, payload) = latest_plan(store, project_id, plan_id)?
        .ok_or_else(|| CoreError::not_found(format!("plan {plan_id} not found")))?;
    let mut plan: Value = serde_json::from_str(&payload)
        .map_err(|err| CoreError::Storage(format!("plan payload corrupt: {err}")))?;
    if plan["status"] == "approved" {
        let current_digest: Option<String> = store
            .conn
            .query_row(
                "SELECT digest FROM behavior_contracts
                 WHERE project_id=?1 AND contract_id=?2 ORDER BY revision DESC LIMIT 1",
                (project_id, plan["contract_id"].as_str().unwrap_or("")),
                |row| row.get(0),
            )
            .ok();
        if current_digest.as_deref() != plan["contract_digest"].as_str() {
            plan["status"] = json!("stale");
        }
    }
    Ok(plan)
}

fn validate_contract(contract: &BehaviorContract) -> Result<()> {
    if contract.schema_version != "1.0" {
        return Err(CoreError::invalid("contract.schema_version must be 1.0"));
    }
    validate::require_id("contract.id", &contract.id)?;
    validate::require_id("contract.project_id", &contract.project_id)?;
    validate::require_digest("contract.digest", &contract.digest)?;
    if contract.revision < 1 {
        return Err(CoreError::invalid("contract.revision must be >= 1"));
    }
    if !matches!(
        contract.kind.as_str(),
        "structural_refactor" | "behavior_fix" | "architecture_migration"
    ) {
        return Err(CoreError::invalid(format!(
            "unsupported contract kind {}",
            contract.kind
        )));
    }
    if contract.invariants.is_empty() {
        return Err(CoreError::invalid("contract.invariants must not be empty"));
    }
    for text in contract
        .invariants
        .iter()
        .chain(contract.allowed_changes.iter())
        .chain(contract.known_defects.iter())
    {
        validate::require_non_empty("contract.text[]", text)?;
    }
    if contract.checks.is_empty() {
        return Err(CoreError::invalid("contract.checks must not be empty"));
    }
    let mut ids = BTreeSet::new();
    for check in &contract.checks {
        validate::require_id("check.id", &check.id)?;
        if !ids.insert(check.id.clone()) {
            return Err(CoreError::invalid(format!("duplicate check id {}", check.id)));
        }
        if !matches!(
            check.kind.as_str(),
            "build" | "behavior" | "concurrency" | "architecture" | "performance" | "domain"
        ) {
            return Err(CoreError::invalid(format!(
                "unsupported check kind {}",
                check.kind
            )));
        }
        validate::require_non_empty("check.description", &check.description)?;
        if check.command.is_some() && !check.command.as_ref().unwrap().is_object() {
            return Err(CoreError::invalid(format!(
                "check {} command must be an object or null",
                check.id
            )));
        }
        if let Some(checker_id) = &check.checker_id {
            validate::require_non_empty("check.checker_id", checker_id)?;
        }
        if check.required && check.command.is_none() && check.checker_id.is_none() {
            return Err(CoreError::invalid(format!(
                "required check {} has neither a command nor a checker_id",
                check.id
            )));
        }
        for artifact in &check.input_artifacts {
            validate::require_digest("check.input_artifacts[]", artifact)?;
        }
        if !check.predicate.is_object() {
            return Err(CoreError::invalid(format!(
                "check {} predicate must be an object",
                check.id
            )));
        }
        match (&check.command, &check.sandbox) {
            (Some(_), Some(sandbox)) => validate_sandbox(&check.id, true, sandbox)?,
            (Some(_), None) => {
                return Err(CoreError::invalid(format!(
                    "check {} has a command but no sandbox declaration; declare workspace_copy or workspace",
                    check.id
                )));
            }
            (None, Some(sandbox)) => validate_sandbox(&check.id, false, sandbox)?,
            (None, None) => {}
        }
    }
    Ok(())
}

fn validate_sandbox(check_id: &str, has_command: bool, sandbox: &CheckSandbox) -> Result<()> {
    if !has_command {
        return Err(CoreError::invalid(format!(
            "check {check_id} declares a sandbox but has no command to confine"
        )));
    }
    match sandbox.mode.as_str() {
        "workspace_copy" => {
            if sandbox.include.is_empty() {
                return Err(CoreError::invalid(format!(
                    "check {check_id} sandbox.include must not be empty for workspace_copy"
                )));
            }
            if sandbox.image.is_some() || sandbox.network.is_some() {
                return Err(CoreError::invalid(format!(
                    "check {check_id} workspace_copy does not accept image/network"
                )));
            }
        }
        "container" => {
            if sandbox.include.is_empty() {
                return Err(CoreError::invalid(format!(
                    "check {check_id} sandbox.include must not be empty for container"
                )));
            }
            let Some(image) = &sandbox.image else {
                return Err(CoreError::invalid(format!(
                    "check {check_id} container sandbox requires a pinned image"
                )));
            };
            if !image.starts_with("sha256:") && !image.contains("@sha256:") {
                return Err(CoreError::invalid(format!(
                    "check {check_id} container image must be pinned by digest"
                )));
            }
            if let Some(network) = &sandbox.network {
                if network != "none" {
                    return Err(CoreError::invalid(format!(
                        "check {check_id} container network must be none"
                    )));
                }
            }
        }
        "workspace" => {
            if !sandbox.include.is_empty() || !sandbox.exclude.is_empty() {
                return Err(CoreError::invalid(format!(
                    "check {check_id} sandbox mode workspace declares no include/exclude allowlist"
                )));
            }
            if sandbox.image.is_some() || sandbox.network.is_some() {
                return Err(CoreError::invalid(format!(
                    "check {check_id} sandbox mode workspace does not accept image/network"
                )));
            }
        }
        other => {
            return Err(CoreError::invalid(format!(
                "check {check_id} has unsupported sandbox mode {other}"
            )));
        }
    }
    for path in sandbox.include.iter().chain(sandbox.exclude.iter()) {
        validate::validate_path("check.sandbox.include[]", path)?;
    }
    Ok(())
}

fn contract_digest(contract: &BehaviorContract) -> Result<String> {
    let mut value = serde_json::to_value(contract)
        .map_err(|err| CoreError::Storage(format!("contract encode: {err}")))?;
    if let Some(object) = value.as_object_mut() {
        object.remove("digest");
        object.remove("frozen");
    }
    hash::digest_value(&value)
        .map_err(|err| CoreError::invalid(format!("contract is outside the hash domain: {err}")))
}

fn detect_cycles(dependencies: &BTreeMap<String, Vec<String>>) -> Result<()> {
    fn visit(
        node: &str,
        dependencies: &BTreeMap<String, Vec<String>>,
        visiting: &mut BTreeSet<String>,
        done: &mut BTreeSet<String>,
    ) -> Result<()> {
        if done.contains(node) {
            return Ok(());
        }
        if !visiting.insert(node.to_string()) {
            return Err(CoreError::invalid(format!(
                "plan step dependency cycle includes {node}"
            )));
        }
        if let Some(deps) = dependencies.get(node) {
            for dep in deps {
                visit(dep, dependencies, visiting, done)?;
            }
        }
        visiting.remove(node);
        done.insert(node.to_string());
        Ok(())
    }
    let mut done = BTreeSet::new();
    for node in dependencies.keys() {
        visit(node, dependencies, &mut BTreeSet::new(), &mut done)?;
    }
    Ok(())
}

fn snapshot_exists(store: &Store, project_id: &str, snapshot_id: &str) -> Result<()> {
    let exists: Option<String> = store
        .conn
        .query_row(
            "SELECT snapshot_id FROM snapshots WHERE project_id=?1 AND snapshot_id=?2",
            (project_id, snapshot_id),
            |row| row.get(0),
        )
        .ok();
    if exists.is_none() {
        return Err(CoreError::not_found(format!(
            "snapshot {snapshot_id} not found in project"
        )));
    }
    Ok(())
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

fn latest_contract(store: &Store, project_id: &str, contract_id: &str) -> Result<Option<(i64, String)>> {
    Ok(store
        .conn
        .query_row(
            "SELECT revision, payload_json FROM behavior_contracts
             WHERE project_id=?1 AND contract_id=?2 ORDER BY revision DESC LIMIT 1",
            (project_id, contract_id),
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .ok())
}

fn latest_plan(store: &Store, project_id: &str, plan_id: &str) -> Result<Option<(i64, String)>> {
    Ok(store
        .conn
        .query_row(
            "SELECT revision, payload_json FROM plans
             WHERE project_id=?1 AND plan_id=?2 ORDER BY revision DESC LIMIT 1",
            (project_id, plan_id),
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .ok())
}
