// Copyright 2026 ARCHMELD contributors
// SPDX-License-Identifier: Apache-2.0

//! Controlled staging and prepared ChangeSets (S09 §3, S19 §1–§2).
//!
//! A ChangeSet never writes to the analyzed repository. Preparation
//! materializes the base-snapshot bytes into a session staging directory,
//! scans what an external transform tool did there, records before/after blobs
//! in the CAS, and publishes an immutable prepared revision.
//!
//! Approvals are append-only and bind the exact prepared digest. The revision
//! history is append-only as well: a changed staging tree produces a new
//! prepared digest and therefore leaves any earlier approval unmatched.

use crate::error::{CoreError, Result};
use crate::events::append_event;
use crate::hash;
use crate::plan::get_plan;
use crate::snapshot::SnapshotOptions;
use crate::store::{validate, Store};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::{Component, Path, PathBuf};

const CHANGESET_VERSION: &str = "1.0";
const STAGING_METADATA: &str = ".archmeld-staging.json";
const SEED_MARKER: &str = ".archmeld.seeding";
const MAX_STAGED_FILES: usize = 100_000;

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SnapshotCaptureOptions {
    pub include_untracked: bool,
    pub max_file_bytes: u64,
    pub excluded_dirs: Vec<String>,
    pub max_retries: u32,
}

impl Default for SnapshotCaptureOptions {
    fn default() -> Self {
        let options = SnapshotOptions::default();
        Self {
            include_untracked: options.include_untracked,
            max_file_bytes: options.max_file_bytes,
            excluded_dirs: options.excluded_dirs,
            max_retries: options.max_retries,
        }
    }
}

impl SnapshotCaptureOptions {
    pub fn to_options(&self) -> SnapshotOptions {
        SnapshotOptions {
            include_untracked: self.include_untracked,
            max_file_bytes: self.max_file_bytes,
            excluded_dirs: self.excluded_dirs.clone(),
            max_retries: self.max_retries,
        }
    }
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
struct CreateRequest {
    schema_version: String,
    project_id: String,
    changeset_id: String,
    plan_id: String,
    plan_revision: i64,
    step_id: String,
    read_set: Vec<String>,
    build_config: Value,
    environment: Value,
    snapshot_options: Option<SnapshotCaptureOptions>,
    expected_revision: i64,
    idempotency_key: String,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
struct ToolIdentity {
    name: String,
    version: String,
    identity_digest: String,
}

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct MoveDeclaration {
    pub from: String,
    pub to: String,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
struct TransformRecord {
    method: String,
    tool: ToolIdentity,
    declared_moves: Vec<MoveDeclaration>,
    patch_artifact: Option<String>,
    prompt_artifact: Option<String>,
    workspace_edit_operations: u32,
    notes: Vec<String>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
struct PrepareRequest {
    project_id: String,
    changeset_id: String,
    expected_revision: i64,
    toolchain_digest: String,
    transform: Option<TransformRecord>,
    idempotency_key: String,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
struct ApproveRequest {
    project_id: String,
    changeset_id: String,
    expected_revision: i64,
    prepared_digest: String,
    actor: String,
    expires_at: Option<i64>,
    idempotency_key: String,
}

#[derive(Debug, Clone)]
struct BaseFile {
    digest: String,
    size: u64,
    executable: bool,
}

#[derive(Debug, Clone)]
struct StagedFile {
    digest: String,
    size: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct StagingMetadata {
    version: String,
    project_id: String,
    changeset_id: String,
    base_snapshot_id: String,
    files: Vec<StagingMetadataFile>,
    created_at: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct StagingMetadataFile {
    path: String,
    digest: String,
    size: u64,
}

#[derive(Debug, Clone)]
struct Operation {
    op: String,
    path: String,
    destination: Option<String>,
    before: Option<BaseFile>,
    after: Option<(String, u64)>,
    case_only: bool,
}

impl Operation {
    fn payload(&self) -> Value {
        json!({
            "op": self.op,
            "path": self.path,
            "destination": self.destination,
            "before": self.before.as_ref().map(|file| json!({
                "digest": file.digest,
                "size": file.size,
                "executable": file.executable,
            })),
            "after": self.after.as_ref().map(|(digest, size)| json!({
                "digest": digest,
                "size": size,
            })),
            "case_only": self.case_only,
            "state": "pending",
        })
    }

    fn digest_entry(&self) -> Value {
        json!({
            "op": self.op,
            "path": self.path,
            "destination": self.destination,
            "before": self.before.as_ref().map(|file| json!({
                "digest": file.digest,
                "size": file.size,
            })),
            "after": self.after.as_ref().map(|(digest, size)| json!({
                "digest": digest,
                "size": size,
            })),
        })
    }
}

/// Create the draft ChangeSet. This records scope and identity only; no
/// staging directory and no source write exists at this point.
pub fn create_changeset(store: &mut Store, raw: &Value) -> Result<Value> {
    let request: CreateRequest = serde_json::from_value(raw.clone())
        .map_err(|err| CoreError::invalid(format!("ChangeSet request: {err}")))?;
    if request.schema_version != CHANGESET_VERSION {
        return Err(CoreError::invalid("changeset schema_version must be 1.0"));
    }
    validate::require_id("project_id", &request.project_id)?;
    validate::require_id("changeset_id", &request.changeset_id)?;
    validate::require_id("plan_id", &request.plan_id)?;
    validate::require_id("step_id", &request.step_id)?;
    validate::require_id("idempotency_key", &request.idempotency_key)?;
    if request.expected_revision < 1 {
        return Err(CoreError::invalid("expected_revision must be >= 1"));
    }
    let mut read_set = request.read_set.clone();
    let mut seen = BTreeSet::new();
    for path in &read_set {
        validate::validate_path("read_set[]", path)?;
        if !seen.insert(path.clone()) {
            return Err(CoreError::invalid(format!(
                "read_set contains duplicate path {path}"
            )));
        }
    }
    read_set.sort();
    if !request.build_config.is_object() || !request.environment.is_object() {
        return Err(CoreError::invalid(
            "build_config and environment must be objects",
        ));
    }
    let plan = get_plan(store, &request.project_id, &request.plan_id)?;
    if plan["revision"].as_i64() != Some(request.plan_revision) {
        return Err(CoreError::conflict(
            "stale_snapshot: plan_revision is not the latest plan revision",
        ));
    }
    if plan["status"] != "approved" {
        return Err(CoreError::conflict(format!(
            "plan {} is {}; only an approved plan scope may create a ChangeSet",
            request.plan_id, plan["status"]
        )));
    }
    let step = plan["steps"]
        .as_array()
        .and_then(|steps| steps.iter().find(|step| step["id"] == request.step_id))
        .cloned()
        .ok_or_else(|| {
            CoreError::not_found(format!(
                "plan {} has no step {}",
                request.plan_id, request.step_id
            ))
        })?;
    let mut allowed_paths: Vec<String> = step["allowed_paths"]
        .as_array()
        .map(|paths| {
            paths
                .iter()
                .filter_map(|path| path.as_str().map(str::to_string))
                .collect()
        })
        .unwrap_or_default();
    allowed_paths.sort();
    if allowed_paths.is_empty() {
        return Err(CoreError::invalid("the plan step has no allowed_paths"));
    }
    let base_snapshot_id = plan["base_snapshot_id"]
        .as_str()
        .ok_or_else(|| CoreError::Storage("plan has no base_snapshot_id".to_string()))?
        .to_string();
    let contract_id = plan["contract_id"].as_str().unwrap_or("").to_string();
    let contract_digest = plan["contract_digest"].as_str().unwrap_or("").to_string();
    validate::require_digest("base_snapshot_id", &base_snapshot_id)?;

    let base_files = snapshot_files(store, &request.project_id, &base_snapshot_id)?;
    for path in &read_set {
        if !base_files.contains_key(path) {
            return Err(CoreError::not_found(format!(
                "read_set path {path} is not present in base snapshot {base_snapshot_id}"
            )));
        }
    }
    let (config_digest, environment_digest) =
        snapshot_manifest_digests(&request.build_config, &request.environment)?;
    let (recorded_config, recorded_environment): (String, String) = store
        .conn
        .query_row(
            "SELECT config_digest, environment_digest FROM snapshots
             WHERE project_id=?1 AND snapshot_id=?2",
            (&request.project_id, &base_snapshot_id),
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .map_err(|_| {
            CoreError::not_found(format!(
                "snapshot {base_snapshot_id} not found in project {}",
                request.project_id
            ))
        })?;
    if recorded_config != config_digest || recorded_environment != environment_digest {
        return Err(CoreError::conflict(
            "build_config/environment do not reproduce the base snapshot digests",
        ));
    }
    let snapshot_options = request.snapshot_options.clone().unwrap_or_default();
    validate_snapshot_options(&snapshot_options)?;

    let project_revision = project_revision(store, &request.project_id)?;
    if request.expected_revision != project_revision {
        return Err(CoreError::conflict(format!(
            "stale_snapshot: expected_revision {} does not match project revision {project_revision}",
            request.expected_revision
        )));
    }
    let digest = hash::digest_value(raw)
        .map_err(|err| CoreError::invalid(format!("changeset request: {err}")))?;
    if let Some((recorded, _, result)) = store.idempotency_get(
        &request.project_id,
        "createChangeSet",
        &request.idempotency_key,
    )? {
        if recorded != digest {
            return Err(CoreError::conflict(
                "scope_conflict: idempotency_key was used with a different ChangeSet",
            ));
        }
        return serde_json::from_str(&result)
            .map_err(|err| CoreError::Storage(format!("stored ChangeSet corrupt: {err}")));
    }
    if latest_changeset(store, &request.project_id, &request.changeset_id)?.is_some() {
        return Err(CoreError::conflict(format!(
            "changeset {} already exists",
            request.changeset_id
        )));
    }
    let payload = json!({
        "schema_version": CHANGESET_VERSION,
        "id": request.changeset_id,
        "project_id": request.project_id,
        "revision": 1,
        "plan_id": request.plan_id,
        "plan_revision": request.plan_revision,
        "step_id": request.step_id,
        "base_snapshot_id": base_snapshot_id,
        "contract_id": contract_id,
        "contract_digest": contract_digest,
        "state": "draft",
        "allowed_paths": allowed_paths,
        "read_set": read_set,
        "write_set": [],
        "operations": [],
        "toolchain_digest": null,
        "transform": null,
        "build_config": request.build_config,
        "environment": request.environment,
        "snapshot_options": serde_json::to_value(&snapshot_options)
            .map_err(|err| CoreError::Storage(format!("snapshot options: {err}")))?,
        "prepared_digest": null,
        "prepared_artifact": null,
        "staging": null,
        "created_at": crate::store::now_unix(),
    });
    persist_revision(
        store,
        payload.clone(),
        "draft",
        payload["operations"].clone(),
        None,
    )?;
    let result = serde_json::to_string(&payload)
        .map_err(|err| CoreError::Storage(format!("changeset encode: {err}")))?;
    store.conn.execute(
        "INSERT INTO idempotency
         (project_id,operation,idempotency_key,request_digest,status_code,result_json,created_at)
         VALUES (?1,'createChangeSet',?2,?3,200,?4,?5)",
        (
            &request.project_id,
            &request.idempotency_key,
            &digest,
            &result,
            crate::store::now_unix(),
        ),
    )?;
    Ok(payload)
}

/// Materialize or rescan the staging tree and publish a prepared revision.
///
/// The first call seeds the staging directory with base-snapshot bytes. Later
/// calls (after an external editor / codemod changed staging) compute a new
/// diff. The source repository is only read through CAS blobs; it is never
/// opened here.
pub fn prepare_changeset(store: &mut Store, raw: &Value) -> Result<Value> {
    let request: PrepareRequest = serde_json::from_value(raw.clone())
        .map_err(|err| CoreError::invalid(format!("prepare request: {err}")))?;
    validate::require_id("project_id", &request.project_id)?;
    validate::require_id("changeset_id", &request.changeset_id)?;
    validate::require_id("idempotency_key", &request.idempotency_key)?;
    validate::require_digest("toolchain_digest", &request.toolchain_digest)?;
    let (revision, payload) = latest_changeset(store, &request.project_id, &request.changeset_id)?
        .ok_or_else(|| {
            CoreError::not_found(format!("changeset {} not found", request.changeset_id))
        })?;
    if request.expected_revision != revision {
        return Err(CoreError::conflict(format!(
            "stale_snapshot: expected changeset revision {} does not match {revision}",
            request.expected_revision
        )));
    }
    let initial_state = payload["state"].as_str().unwrap_or("").to_string();
    if initial_state == "failed" {
        let has_journal: i64 = store.conn.query_row(
            "SELECT COUNT(*) FROM apply_journal WHERE project_id=?1 AND changeset_id=?2",
            (&request.project_id, &request.changeset_id),
            |row| row.get(0),
        )?;
        if has_journal > 0 {
            return Err(CoreError::conflict(
                "changeset failed during apply; recover it instead of re-preparing",
            ));
        }
    }
    if !matches!(
        initial_state.as_str(),
        "draft" | "approved" | "stale" | "prepared" | "failed"
    ) {
        return Err(CoreError::conflict(format!(
            "changeset {} is {initial_state} and cannot be prepared",
            request.changeset_id
        )));
    }
    require_toolchain_artifact(store, &request.toolchain_digest)?;
    if let Some(transform) = &request.transform {
        validate_transform(store, transform)?;
    }
    ensure_plan_current(store, &payload)?;
    let request_digest = hash::digest_value(raw)
        .map_err(|err| CoreError::invalid(format!("prepare request: {err}")))?;
    if let Some((recorded, _, result)) = store.idempotency_get(
        &request.project_id,
        "prepareChangeSet",
        &request.idempotency_key,
    )? {
        if recorded != request_digest {
            return Err(CoreError::conflict(
                "scope_conflict: idempotency_key was used with a different prepare request",
            ));
        }
        return serde_json::from_str(&result)
            .map_err(|err| CoreError::Storage(format!("stored ChangeSet corrupt: {err}")));
    }

    // Walk the documented state machine explicitly. A prepared ChangeSet that
    // is re-staged first becomes stale, then replans to draft, then re-enters
    // approved because the plan-scope approval still exists.
    let final_revision = match initial_state.as_str() {
        "draft" => revision + 3,
        "approved" => revision + 2,
        "stale" => revision + 4,
        "prepared" => revision + 5,
        "failed" => revision + 4,
        _ => unreachable!(),
    };
    let mut current = payload.clone();
    if initial_state == "prepared" {
        current = append_state(
            store,
            current,
            "stale",
            json!({"stale_reason": "restaged after a prepared revision"}),
        )?;
        current = append_state(store, current, "draft", json!({}))?;
    } else if initial_state == "stale" || initial_state == "failed" {
        current = append_state(
            store,
            current,
            "draft",
            json!({"replanned_from": initial_state}),
        )?;
    }
    if current["state"] == "draft" {
        current = append_state(store, current, "approved", json!({}))?;
    }
    current = append_state(store, current, "preparing", json!({}))?;
    let preparing_revision = current["revision"].as_i64().unwrap_or(final_revision);
    debug_assert!(preparing_revision < final_revision || initial_state == "approved");

    let staged = match scan_staging(store, &current, &request.transform) {
        Ok(staged) => staged,
        Err(error) => {
            let failed = append_state(
                store,
                current,
                "failed",
                json!({"error": error.to_string()}),
            )?;
            let _ = failed;
            return Err(error);
        }
    };
    let prepared = build_prepared(
        store,
        current,
        &staged,
        &request.toolchain_digest,
        &request.transform,
        final_revision,
    )?;
    let mut prepared = prepared;
    prepared["previous_state"] = json!("preparing");
    prepared["state"] = json!("prepared");
    let final_payload = prepared;
    persist_revision(
        store,
        final_payload.clone(),
        "prepared",
        final_payload["operations"].clone(),
        None,
    )?;
    let result = serde_json::to_string(&final_payload)
        .map_err(|err| CoreError::Storage(format!("changeset encode: {err}")))?;
    store.conn.execute(
        "INSERT INTO idempotency
         (project_id,operation,idempotency_key,request_digest,status_code,result_json,created_at)
         VALUES (?1,'prepareChangeSet',?2,?3,200,?4,?5)",
        (
            &request.project_id,
            &request.idempotency_key,
            &request_digest,
            &result,
            crate::store::now_unix(),
        ),
    )?;
    Ok(final_payload)
}

/// Record an approval bound to the exact prepared digest. This endpoint is
/// only reachable through the trusted local API; MCP deliberately does not
/// expose it.
pub fn approve_prepared_change(store: &mut Store, raw: &Value) -> Result<Value> {
    let request: ApproveRequest = serde_json::from_value(raw.clone())
        .map_err(|err| CoreError::invalid(format!("approval request: {err}")))?;
    validate::require_id("project_id", &request.project_id)?;
    validate::require_id("changeset_id", &request.changeset_id)?;
    validate::require_digest("prepared_digest", &request.prepared_digest)?;
    validate::require_non_empty("actor", &request.actor)?;
    validate::require_id("idempotency_key", &request.idempotency_key)?;
    let now = crate::store::now_unix();
    if let Some(expires_at) = request.expires_at {
        if expires_at <= now {
            return Err(CoreError::invalid("approval expires_at is in the past"));
        }
    }
    let (revision, payload) = latest_changeset(store, &request.project_id, &request.changeset_id)?
        .ok_or_else(|| {
            CoreError::not_found(format!("changeset {} not found", request.changeset_id))
        })?;
    if request.expected_revision != revision {
        return Err(CoreError::conflict(format!(
            "stale_snapshot: expected changeset revision {} does not match {revision}",
            request.expected_revision
        )));
    }
    if payload["state"] != "prepared" {
        return Err(CoreError::conflict(format!(
            "changeset {} is {}; only a prepared revision can be approved",
            request.changeset_id, payload["state"]
        )));
    }
    if payload["prepared_digest"].as_str() != Some(request.prepared_digest.as_str()) {
        return Err(CoreError::conflict(
            "stale_plan: prepared_digest does not match the current prepared revision",
        ));
    }
    ensure_plan_current(store, &payload)?;
    let scope = json!({
        "version": "1.0",
        "project_id": request.project_id,
        "target_kind": "prepared_change",
        "changeset_id": request.changeset_id,
        "revision": revision,
        "prepared_digest": request.prepared_digest,
        "plan_id": payload["plan_id"],
        "step_id": payload["step_id"],
        "base_snapshot_id": payload["base_snapshot_id"],
        "allowed_paths": payload["allowed_paths"],
        "read_set": payload["read_set"],
        "write_set": payload["write_set"],
    });
    let scope_json = hash::canonical_json(&scope)
        .map_err(|err| CoreError::invalid(format!("approval scope: {err}")))?;
    let scope_digest = hash::digest_value(&scope)
        .map_err(|err| CoreError::invalid(format!("approval scope: {err}")))?;
    let hex = scope_digest
        .strip_prefix("sha256:")
        .unwrap_or(&scope_digest);
    let approval_id = format!("approval.{}", &hex[..32]);
    let idempotency_digest = hash::digest_value(raw)
        .map_err(|err| CoreError::invalid(format!("approval request: {err}")))?;
    if let Some((recorded, _, result)) = store.idempotency_get(
        &request.project_id,
        "approvePreparedChange",
        &request.idempotency_key,
    )? {
        if recorded != idempotency_digest {
            return Err(CoreError::conflict(
                "scope_conflict: idempotency_key was used with a different approval",
            ));
        }
        return serde_json::from_str(&result)
            .map_err(|err| CoreError::Storage(format!("stored approval corrupt: {err}")));
    }
    let existing: Option<(String, String, String)> = store
        .conn
        .query_row(
            "SELECT target_digest, actor, decision FROM approvals
             WHERE project_id=?1 AND approval_id=?2",
            (&request.project_id, &approval_id),
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
        )
        .ok();
    if let Some((target_digest, actor, decision)) = existing {
        if target_digest != request.prepared_digest
            || actor != request.actor
            || decision != "approve"
        {
            return Err(CoreError::conflict(
                "an approval with this identity already exists for different content",
            ));
        }
        return approval_payload(
            store,
            &request.project_id,
            &approval_id,
            &payload,
            &request.prepared_digest,
        );
    }
    let approval = json!({
        "approval_id": approval_id,
        "target_kind": "prepared_change",
        "target_id": request.changeset_id,
        "target_revision": revision,
        "target_digest": request.prepared_digest,
        "actor": request.actor,
        "decision": "approve",
        "expires_at": request.expires_at,
        "created_at": now,
    });
    store.conn.execute(
        "INSERT INTO approvals
         (project_id,approval_id,target_kind,target_id,target_revision,target_digest,
          actor,decision,scope_json,expires_at,created_at)
         VALUES (?1,?2,'prepared_change',?3,?4,?5,?6,'approve',?7,?8,?9)",
        (
            &request.project_id,
            &approval_id,
            &request.changeset_id,
            revision,
            &request.prepared_digest,
            &request.actor,
            &scope_json,
            request.expires_at,
            now,
        ),
    )?;
    let result = json!({
        "changeset_id": request.changeset_id,
        "revision": revision,
        "prepared_digest": request.prepared_digest,
        "approval": approval,
        "scope_digest": scope_digest,
    });
    let result_json = serde_json::to_string(&result)
        .map_err(|err| CoreError::Storage(format!("approval encode: {err}")))?;
    store.conn.execute(
        "INSERT INTO idempotency
         (project_id,operation,idempotency_key,request_digest,status_code,result_json,created_at)
         VALUES (?1,'approvePreparedChange',?2,?3,200,?4,?5)",
        (
            &request.project_id,
            &request.idempotency_key,
            &idempotency_digest,
            &result_json,
            now,
        ),
    )?;
    Ok(result)
}

pub fn reject_changeset(
    store: &mut Store,
    project_id: &str,
    changeset_id: &str,
    expected_revision: i64,
    actor: &str,
) -> Result<Value> {
    validate::require_id("project_id", project_id)?;
    validate::require_id("changeset_id", changeset_id)?;
    validate::require_non_empty("actor", actor)?;
    let (revision, payload) = latest_changeset(store, project_id, changeset_id)?
        .ok_or_else(|| CoreError::not_found(format!("changeset {changeset_id} not found")))?;
    if expected_revision != revision {
        return Err(CoreError::conflict(
            "stale_snapshot: expected changeset revision is not latest",
        ));
    }
    if !matches!(
        payload["state"].as_str().unwrap_or(""),
        "draft" | "approved" | "prepared"
    ) {
        return Err(CoreError::conflict(format!(
            "changeset {changeset_id} is {} and cannot be rejected",
            payload["state"]
        )));
    }
    let rejected = append_state(store, payload, "rejected", json!({"rejected_by": actor}))?;
    Ok(rejected)
}

/// Latest revision plus effective (dynamically checked) status. The stored
/// state is never rewritten from a read path.
pub fn get_changeset(store: &Store, project_id: &str, changeset_id: &str) -> Result<Value> {
    validate::require_id("project_id", project_id)?;
    validate::require_id("changeset_id", changeset_id)?;
    let (revision, mut payload) = latest_changeset(store, project_id, changeset_id)?
        .ok_or_else(|| CoreError::not_found(format!("changeset {changeset_id} not found")))?;
    let mut stale_reasons = Vec::new();
    if payload["state"] == "prepared" || payload["state"] == "approved" {
        let plan = get_plan(store, project_id, payload["plan_id"].as_str().unwrap_or(""))?;
        if plan["status"] != "approved" {
            stale_reasons.push(format!("plan is {}", plan["status"]));
        } else if plan["revision"].as_i64() != payload["plan_revision"].as_i64() {
            stale_reasons.push("plan revision changed".to_string());
        }
        let current_digest: Option<String> = store
            .conn
            .query_row(
                "SELECT digest FROM behavior_contracts
                 WHERE project_id=?1 AND contract_id=?2 ORDER BY revision DESC LIMIT 1",
                (project_id, payload["contract_id"].as_str().unwrap_or("")),
                |row| row.get(0),
            )
            .ok();
        if current_digest.as_deref() != payload["contract_digest"].as_str() {
            stale_reasons.push("frozen contract changed".to_string());
        }
    }
    payload["effective_state"] = if stale_reasons.is_empty() {
        payload["state"].clone()
    } else {
        json!("stale")
    };
    payload["stale_reasons"] = json!(stale_reasons);
    payload["revision"] = json!(revision);
    if payload["prepared_digest"].as_str().is_some() {
        let now = crate::store::now_unix();
        let approval: Option<String> = store
            .conn
            .query_row(
                "SELECT approval_id FROM approvals
                 WHERE project_id=?1 AND target_kind='prepared_change' AND target_id=?2
                   AND target_digest=?3 AND decision='approve'
                   AND (expires_at IS NULL OR expires_at > ?4)
                 ORDER BY target_revision DESC, created_at DESC LIMIT 1",
                (
                    project_id,
                    changeset_id,
                    payload["prepared_digest"].as_str().unwrap_or(""),
                    now,
                ),
                |row| row.get(0),
            )
            .ok();
        payload["approval"] = approval.map(Value::String).unwrap_or(Value::Null);
    }
    Ok(payload)
}

pub fn list_changesets(store: &Store, project_id: &str, state: Option<&str>) -> Result<Vec<Value>> {
    validate::require_id("project_id", project_id)?;
    let mut statement = store.conn.prepare(
        "SELECT changeset_id, MAX(revision) FROM changesets
         WHERE project_id=?1 GROUP BY changeset_id ORDER BY changeset_id",
    )?;
    let rows = statement.query_map((project_id,), |row| {
        Ok((row.get::<_, String>(0)?, row.get::<_, i64>(1)?))
    })?;
    let mut summaries = Vec::new();
    for row in rows {
        let (changeset_id, _) = row?;
        let changeset = get_changeset(store, project_id, &changeset_id)?;
        let effective = changeset["effective_state"].as_str().unwrap_or("");
        if let Some(filter) = state {
            if effective != filter {
                continue;
            }
        }
        summaries.push(json!({
            "id": changeset["id"],
            "revision": changeset["revision"],
            "state": changeset["state"],
            "effective_state": changeset["effective_state"],
            "plan_id": changeset["plan_id"],
            "step_id": changeset["step_id"],
            "base_snapshot_id": changeset["base_snapshot_id"],
            "prepared_digest": changeset["prepared_digest"],
            "operation_count": changeset["operations"].as_array().map(Vec::len).unwrap_or(0),
            "staging": changeset["staging"]["path"],
            "approval": changeset.get("approval").cloned().unwrap_or(Value::Null),
        }));
    }
    Ok(summaries)
}

/// Reviewable diff. Preview text is read from CAS blobs and truncated
/// explicitly; the byte digests remain authoritative.
pub fn changeset_diff(
    store: &Store,
    project_id: &str,
    changeset_id: &str,
    max_bytes: u64,
) -> Result<Value> {
    if max_bytes == 0 || max_bytes > 1_048_576 {
        return Err(CoreError::invalid("max_bytes must be 1..=1048576"));
    }
    let changeset = get_changeset(store, project_id, changeset_id)?;
    let mut operations = Vec::new();
    for operation in changeset["operations"]
        .as_array()
        .cloned()
        .unwrap_or_default()
    {
        let before = preview(store, operation["before"]["digest"].as_str(), max_bytes)?;
        let after = preview(store, operation["after"]["digest"].as_str(), max_bytes)?;
        operations.push(json!({
            "op": operation["op"],
            "path": operation["path"],
            "destination": operation["destination"],
            "case_only": operation["case_only"],
            "before": before,
            "after": after,
            "state": operation["state"],
        }));
    }
    Ok(json!({
        "changeset_id": changeset_id,
        "revision": changeset["revision"],
        "state": changeset["state"],
        "effective_state": changeset["effective_state"],
        "prepared_digest": changeset["prepared_digest"],
        "operations": operations,
        "limitations": [
            "previews are truncated at max_bytes",
            "byte digests and CAS blobs are authoritative, previews are not",
        ],
        "source_mutation": "none",
    }))
}

pub(crate) fn latest_changeset(
    store: &Store,
    project_id: &str,
    changeset_id: &str,
) -> Result<Option<(i64, Value)>> {
    let row: Option<(i64, String)> = store
        .conn
        .query_row(
            "SELECT revision, payload_json FROM changesets
             WHERE project_id=?1 AND changeset_id=?2 ORDER BY revision DESC LIMIT 1",
            (project_id, changeset_id),
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .ok();
    let Some((revision, payload)) = row else {
        return Ok(None);
    };
    let mut value: Value = serde_json::from_str(&payload)
        .map_err(|err| CoreError::Storage(format!("changeset payload corrupt: {err}")))?;
    value["revision"] = json!(revision);
    Ok(Some((revision, value)))
}

pub(crate) fn append_state(
    store: &mut Store,
    payload: Value,
    state: &str,
    extra: Value,
) -> Result<Value> {
    append_state_with_journal(store, payload, state, extra, None)
}

/// Same as [`append_state`], but the revision insert, its operation rows and
/// optional apply-journal intent rows commit as one SQLite transaction. This
/// is what makes "state says applying" and "journal says intent" inseparable
/// across a crash.
pub(crate) fn append_state_with_journal(
    store: &mut Store,
    mut payload: Value,
    state: &str,
    extra: Value,
    journal: Option<&[Value]>,
) -> Result<Value> {
    let previous = payload["state"].as_str().unwrap_or("").to_string();
    if !transition_allowed(&previous, state) {
        return Err(CoreError::Storage(format!(
            "illegal ChangeSet transition {previous} -> {state}"
        )));
    }
    let revision = payload["revision"].as_i64().unwrap_or(0) + 1;
    payload["previous_state"] = json!(previous);
    payload["revision"] = json!(revision);
    payload["state"] = json!(state);
    if let Some(object) = extra.as_object() {
        for (key, value) in object {
            payload[key] = value.clone();
        }
    }
    payload["updated_at"] = json!(crate::store::now_unix());
    let operations = payload["operations"].clone();
    let row = payload.clone();
    persist_revision(store, row, state, operations, journal)?;
    Ok(payload)
}

fn persist_revision(
    store: &mut Store,
    payload: Value,
    state: &str,
    operations: Value,
    journal: Option<&[Value]>,
) -> Result<()> {
    let revision = payload["revision"]
        .as_i64()
        .ok_or_else(|| CoreError::Storage("changeset revision missing".to_string()))?;
    let payload_json = hash::canonical_json(&payload)
        .map_err(|err| CoreError::invalid(format!("changeset payload: {err}")))?;
    let tx = store.conn.transaction()?;
    tx.execute(
        "INSERT INTO changesets
         (project_id,changeset_id,revision,plan_id,plan_revision,step_id,base_snapshot_id,
          contract_digest,state,prepared_digest,payload_json,created_at)
         VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12)",
        (
            payload["project_id"].as_str().unwrap_or(""),
            payload["id"].as_str().unwrap_or(""),
            revision,
            payload["plan_id"].as_str().unwrap_or(""),
            payload["plan_revision"].as_i64().unwrap_or(0),
            payload["step_id"].as_str().unwrap_or(""),
            payload["base_snapshot_id"].as_str().unwrap_or(""),
            payload["contract_digest"].as_str().unwrap_or(""),
            state,
            payload["prepared_digest"].as_str(),
            &payload_json,
            crate::store::now_unix(),
        ),
    )?;
    let default_operation_state = match state {
        "applied" => "applied",
        "recovered" => "restored",
        "conflicted" => "conflict",
        _ => "pending",
    };
    for (sequence, operation) in operations.as_array().into_iter().flatten().enumerate() {
        let operation_state = operation["state"]
            .as_str()
            .filter(|value| {
                matches!(
                    *value,
                    "pending" | "applied" | "failed" | "restored" | "conflict"
                )
            })
            .unwrap_or(default_operation_state);
        tx.execute(
            "INSERT INTO change_operations
             (project_id,changeset_id,revision,sequence,op,path,destination,
              before_digest,before_size,after_digest,after_size,state,payload_json)
             VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12,?13)",
            (
                payload["project_id"].as_str().unwrap_or(""),
                payload["id"].as_str().unwrap_or(""),
                revision,
                sequence as i64,
                operation["op"].as_str().unwrap_or(""),
                operation["path"].as_str().unwrap_or(""),
                operation["destination"].as_str(),
                operation["before"]["digest"].as_str(),
                operation["before"]["size"].as_i64(),
                operation["after"]["digest"].as_str(),
                operation["after"]["size"].as_i64(),
                operation_state,
                hash::canonical_json(operation)
                    .map_err(|err| CoreError::invalid(format!("operation payload: {err}")))?,
            ),
        )?;
    }
    for row in journal.into_iter().flatten() {
        tx.execute(
            "INSERT OR REPLACE INTO apply_journal
             (project_id,changeset_id,revision,sequence,op,path,destination,
              preimage_digest,postimage_digest,phase,observed_digest,detail_json,recorded_at)
             VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12,?13)",
            (
                payload["project_id"].as_str().unwrap_or(""),
                payload["id"].as_str().unwrap_or(""),
                revision,
                row["sequence"].as_i64().unwrap_or(0),
                row["op"].as_str().unwrap_or(""),
                row["path"].as_str().unwrap_or(""),
                row["destination"].as_str(),
                row["preimage_digest"].as_str(),
                row["postimage_digest"].as_str(),
                row["phase"].as_str().unwrap_or("intent"),
                row["observed_digest"].as_str(),
                hash::canonical_json(&row["detail"])
                    .map_err(|err| CoreError::invalid(format!("journal detail: {err}")))?,
                crate::store::now_unix(),
            ),
        )?;
    }
    append_event(
        &tx,
        payload["project_id"].as_str().unwrap_or(""),
        None,
        Some(payload["base_snapshot_id"].as_str().unwrap_or("")),
        "change.state_changed",
        json!({
            "changeset_id": payload["id"],
            "previous_state": payload["previous_state"],
            "state": state,
            "revision": revision,
        }),
    )?;
    tx.commit()?;
    Ok(())
}

fn build_prepared(
    store: &mut Store,
    mut payload: Value,
    staged: &Staged,
    toolchain_digest: &str,
    transform: &Option<TransformRecord>,
    final_revision: i64,
) -> Result<Value> {
    let operations = diff_operations(store, &payload, staged, transform)?;
    let mut verification_ids = Vec::new();
    let checks = vec![
        (
            "scope",
            json!({
                "allowed_paths": payload["allowed_paths"],
                "operation_paths": operations.iter().map(|op| op.path.clone()).collect::<Vec<_>>(),
                "status": "within_allowed_paths",
            }),
        ),
        (
            "preimages",
            json!({
                "before_digests": operations.iter().filter_map(|op| op.before.as_ref().map(|file| file.digest.clone())).collect::<Vec<_>>(),
                "status": "all_present_in_cas",
            }),
        ),
        (
            "postimages",
            json!({
                "after_digests": operations.iter().filter_map(|op| op.after.as_ref().map(|(digest, _)| digest.clone())).collect::<Vec<_>>(),
                "status": "all_present_in_cas",
            }),
        ),
        (
            "encoding",
            json!({"status": "all_staged_files_utf8", "binary_operations": 0}),
        ),
        ("symlinks", json!({"status": "no_symlinks_in_staging"})),
        (
            "moves",
            json!({
                "declared": transform.as_ref().map(|record| record.declared_moves.clone()).unwrap_or_default(),
                "status": "all_declared_moves_match_content_identity",
            }),
        ),
        (
            "read_set",
            json!({
                "paths": payload["read_set"],
                "status": "read_set_unchanged_in_staging",
            }),
        ),
    ];
    for (kind, evidence) in &checks {
        let verification_id = verification_id(&payload, final_revision, kind);
        store.conn.execute(
            "INSERT INTO staging_verifications
             (project_id,changeset_id,revision,verification_id,kind,status,evidence_json,created_at)
             VALUES (?1,?2,?3,?4,?5,'passed',?6,?7)",
            (
                payload["project_id"].as_str().unwrap_or(""),
                payload["id"].as_str().unwrap_or(""),
                final_revision,
                &verification_id,
                kind,
                hash::canonical_json(evidence)
                    .map_err(|err| CoreError::invalid(format!("verification evidence: {err}")))?,
                crate::store::now_unix(),
            ),
        )?;
        verification_ids.push(verification_id);
    }
    verification_ids.sort();

    let mut allowed_paths: Vec<String> = payload["allowed_paths"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|value| value.as_str().map(str::to_string))
        .collect();
    allowed_paths.sort();
    let mut read_set: Vec<String> = payload["read_set"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|value| value.as_str().map(str::to_string))
        .collect();
    read_set.sort();
    let digest_operations: Vec<Value> = operations.iter().map(Operation::digest_entry).collect();
    let preimage = json!({
        "version": CHANGESET_VERSION,
        "project_id": payload["project_id"],
        "plan_id": payload["plan_id"],
        "plan_revision": payload["plan_revision"],
        "step_id": payload["step_id"],
        "base_snapshot_id": payload["base_snapshot_id"],
        "contract_digest": payload["contract_digest"],
        "allowed_paths": allowed_paths,
        "read_set": read_set,
        "operations": digest_operations,
        "toolchain_digest": toolchain_digest,
        "staging_verification_ids": verification_ids,
    });
    let prepared_digest = hash::digest_value(&preimage)
        .map_err(|err| CoreError::invalid(format!("prepared change: {err}")))?;
    let manifest = hash::canonical_json(&preimage)
        .map_err(|err| CoreError::invalid(format!("prepared change: {err}")))?;
    let prepared_artifact = store.put_blob(manifest.as_bytes(), "application/json")?;
    // A standalone, exact machine-readable patch. It is not part of the S19
    // prepared digest preimage (which already binds every operation), but it
    // gives reviewers and recovery audits the precise patch object.
    let patch = json!({
        "version": CHANGESET_VERSION,
        "changeset_id": payload["id"],
        "base_snapshot_id": payload["base_snapshot_id"],
        "operations": preimage["operations"],
    });
    let patch_json = hash::canonical_json(&patch)
        .map_err(|err| CoreError::invalid(format!("patch artifact: {err}")))?;
    let patch_artifact = store.put_blob(patch_json.as_bytes(), "application/json")?;

    let write_set: Vec<String> = {
        let mut set = BTreeSet::new();
        for operation in &operations {
            set.insert(operation.path.clone());
            if let Some(destination) = &operation.destination {
                set.insert(destination.clone());
            }
        }
        set.into_iter().collect()
    };
    payload["revision"] = json!(final_revision);
    payload["operations"] = json!(operations
        .iter()
        .map(Operation::payload)
        .collect::<Vec<_>>());
    payload["write_set"] = json!(write_set);
    payload["toolchain_digest"] = json!(toolchain_digest);
    payload["transform"] = match transform {
        Some(record) => json!({
            "method": record.method,
            "tool": {
                "name": record.tool.name,
                "version": record.tool.version,
                "identity_digest": record.tool.identity_digest,
            },
            "declared_moves": record.declared_moves,
            "patch_artifact": record.patch_artifact,
            "prompt_artifact": record.prompt_artifact,
            "workspace_edit_operations": record.workspace_edit_operations,
            "notes": record.notes,
        }),
        None => Value::Null,
    };
    payload["prepared_digest"] = json!(prepared_digest);
    payload["prepared_artifact"] = json!(prepared_artifact);
    payload["patch_artifact"] = json!(patch_artifact);
    payload["staging_verification_ids"] = json!(verification_ids);
    payload["staging"] = json!({
        "path": staged.path.to_string_lossy(),
        "file_count": staged.files.len(),
        "metadata": STAGING_METADATA,
        "source_mutation": "none",
    });
    payload["updated_at"] = json!(crate::store::now_unix());
    Ok(payload)
}

struct Staged {
    path: PathBuf,
    files: BTreeMap<String, StagedFile>,
}

fn scan_staging(
    store: &mut Store,
    payload: &Value,
    transform: &Option<TransformRecord>,
) -> Result<Staged> {
    let project_id = payload["project_id"].as_str().unwrap_or("");
    let changeset_id = payload["id"].as_str().unwrap_or("");
    let base_snapshot_id = payload["base_snapshot_id"].as_str().unwrap_or("");
    let staging = staging_root(store, project_id, changeset_id)?;
    ensure_staging_outside_workspace(store, project_id, &staging)?;
    seed_staging(store, payload, &staging)?;
    let options: SnapshotCaptureOptions =
        serde_json::from_value(payload["snapshot_options"].clone()).unwrap_or_default();
    let files = walk_staging(&staging, options.max_file_bytes)?;
    let base = snapshot_files(store, project_id, base_snapshot_id)?;
    let allowed: Vec<String> = payload["allowed_paths"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|value| value.as_str().map(str::to_string))
        .collect();
    if allowed.is_empty() {
        return Err(CoreError::invalid("changeset has no allowed_paths"));
    }
    let read_set: Vec<String> = payload["read_set"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|value| value.as_str().map(str::to_string))
        .collect();

    // Every staged path must be inside the authorized write scope, or an
    // unchanged read-set file that was materialized only as context.
    for (path, file) in &files {
        if in_scope(path, &allowed) {
            continue;
        }
        if read_set.iter().any(|item| item == path) {
            let Some(base_file) = base.get(path) else {
                return Err(CoreError::invalid(format!(
                    "read_set path {path} has no base snapshot entry"
                )));
            };
            if base_file.digest != file.digest {
                return Err(CoreError::invalid(format!(
                    "out_of_scope: read_set file {path} was modified in staging"
                )));
            }
            continue;
        }
        return Err(CoreError::invalid(format!(
            "out_of_scope: staged path {path} is not within allowed_paths"
        )));
    }

    // Read-set context must still be present and unchanged; deleting it would
    // silently change the preparation context.
    for path in &read_set {
        let Some(base_file) = base.get(path) else {
            return Err(CoreError::not_found(format!(
                "read_set path {path} is missing from base snapshot"
            )));
        };
        if in_scope(path, &allowed) {
            continue;
        }
        match files.get(path) {
            Some(staged) if staged.digest == base_file.digest => {}
            Some(_) => {
                return Err(CoreError::invalid(format!(
                    "out_of_scope: read_set file {path} was modified in staging"
                )))
            }
            None => {
                return Err(CoreError::invalid(format!(
                    "read_set file {path} was deleted from staging"
                )))
            }
        }
    }
    let _ = transform;
    Ok(Staged {
        path: staging,
        files,
    })
}

fn diff_operations(
    store: &mut Store,
    payload: &Value,
    staged: &Staged,
    transform: &Option<TransformRecord>,
) -> Result<Vec<Operation>> {
    let project_id = payload["project_id"].as_str().unwrap_or("");
    let base_snapshot_id = payload["base_snapshot_id"].as_str().unwrap_or("");
    let base = snapshot_files(store, project_id, base_snapshot_id)?;
    let allowed: Vec<String> = payload["allowed_paths"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|value| value.as_str().map(str::to_string))
        .collect();
    let declared = transform
        .as_ref()
        .map(|record| record.declared_moves.clone())
        .unwrap_or_default();
    // Persist every staged byte sequence as a postimage candidate. A digest
    // alone is not evidence; the CAS must actually hold the bytes.
    for (path, file) in &staged.files {
        let bytes = fs::read(staged.path.join(path))?;
        let digest = store.put_blob(&bytes, "text/plain")?;
        if digest != file.digest {
            return Err(CoreError::Storage(format!(
                "staged file {path} changed while being recorded"
            )));
        }
    }
    let mut move_sources = BTreeSet::new();
    let mut move_targets = BTreeSet::new();
    let mut operations: Vec<Operation> = Vec::new();
    for movement in &declared {
        validate::validate_path("declared_moves[].from", &movement.from)?;
        validate::validate_path("declared_moves[].to", &movement.to)?;
        if movement.from == movement.to {
            return Err(CoreError::invalid("declared move has identical from/to"));
        }
        if !in_scope(&movement.from, &allowed) || !in_scope(&movement.to, &allowed) {
            return Err(CoreError::invalid(format!(
                "declared move {} -> {} is outside allowed_paths",
                movement.from, movement.to
            )));
        }
        if !move_sources.insert(movement.from.clone()) || !move_targets.insert(movement.to.clone())
        {
            return Err(CoreError::invalid(
                "a declared move reuses a source or target",
            ));
        }
        let Some(before) = base.get(&movement.from).cloned() else {
            return Err(CoreError::not_found(format!(
                "declared move source {} is not in the base snapshot",
                movement.from
            )));
        };
        if movement.from != movement.to && base.contains_key(&movement.to) {
            return Err(CoreError::conflict(format!(
                "declared move target {} already exists in the base snapshot",
                movement.to
            )));
        }
        if staged.files.contains_key(&movement.from) {
            return Err(CoreError::invalid(format!(
                "declared move {} -> {} has not been performed in staging",
                movement.from, movement.to
            )));
        }
        let Some(after) = staged.files.get(&movement.to) else {
            return Err(CoreError::invalid(format!(
                "declared move target {} is missing from staging",
                movement.to
            )));
        };
        if after.digest != before.digest || after.size != before.size {
            return Err(CoreError::invalid(format!(
                "v1 move {} -> {} must preserve content; split it into move then update",
                movement.from, movement.to
            )));
        }
        let case_only = movement.from != movement.to
            && movement.from.to_lowercase() == movement.to.to_lowercase();
        operations.push(Operation {
            op: "move".to_string(),
            path: movement.from.clone(),
            destination: Some(movement.to.clone()),
            before: Some(before),
            after: Some((after.digest.clone(), after.size)),
            case_only,
        });
    }

    // Staged files in write scope that are not move targets.
    for (path, staged_file) in &staged.files {
        if move_targets.contains(path) || !in_scope(path, &allowed) {
            continue;
        }
        if move_sources.contains(path) {
            continue;
        }
        match base.get(path) {
            Some(before)
                if before.digest == staged_file.digest && before.size == staged_file.size =>
            {
                // unchanged
            }
            Some(before) => operations.push(Operation {
                op: "update".to_string(),
                path: path.clone(),
                destination: None,
                before: Some(before.clone()),
                after: Some((staged_file.digest.clone(), staged_file.size)),
                case_only: false,
            }),
            None => operations.push(Operation {
                op: "create".to_string(),
                path: path.clone(),
                destination: None,
                before: None,
                after: Some((staged_file.digest.clone(), staged_file.size)),
                case_only: false,
            }),
        }
    }
    // Base files in write scope that are missing from staging (and are not
    // move sources).
    for (path, before) in &base {
        if !in_scope(path, &allowed) || move_sources.contains(path) || move_targets.contains(path) {
            continue;
        }
        if !staged.files.contains_key(path) {
            operations.push(Operation {
                op: "delete".to_string(),
                path: path.clone(),
                destination: None,
                before: Some(before.clone()),
                after: None,
                case_only: false,
            });
        }
    }
    for operation in &operations {
        if let Some((digest, size)) = &operation.after {
            let bytes = store.read_blob(digest)?.ok_or_else(|| {
                CoreError::Storage(format!(
                    "staged content {digest} is not in the object store"
                ))
            })?;
            if bytes.len() as u64 != *size {
                return Err(CoreError::Storage(format!(
                    "staged content {digest} size mismatch"
                )));
            }
        }
    }
    operations.sort_by(|left, right| {
        rank(&left.op)
            .cmp(&rank(&right.op))
            .then(left.path.as_bytes().cmp(right.path.as_bytes()))
            .then(left.destination.cmp(&right.destination))
    });
    Ok(operations)
}

fn rank(op: &str) -> u8 {
    match op {
        "move" => 0,
        "create" => 1,
        "update" => 2,
        "delete" => 3,
        _ => 4,
    }
}

fn seed_staging(store: &mut Store, payload: &Value, staging: &Path) -> Result<()> {
    ensure_staging_segments(store, staging)?;
    let project_id = payload["project_id"].as_str().unwrap_or("");
    let changeset_id = payload["id"].as_str().unwrap_or("");
    let base_snapshot_id = payload["base_snapshot_id"].as_str().unwrap_or("");
    let metadata_path = staging.join(STAGING_METADATA);
    let marker = staging.join(SEED_MARKER);
    if !metadata_path.exists() && marker.exists() {
        // A previous seed was interrupted. Preserve it for audit and start a
        // fresh staging directory; never delete it in place.
        let archived = staging.with_file_name(format!(
            "{changeset_id}.incomplete.{}",
            crate::store::now_unix()
        ));
        fs::rename(staging, &archived)?;
    }
    if metadata_path.exists() {
        let text = fs::read_to_string(&metadata_path)
            .map_err(|err| CoreError::Storage(format!("staging metadata unreadable: {err}")))?;
        let metadata: StagingMetadata = serde_json::from_str(&text)
            .map_err(|err| CoreError::Storage(format!("staging metadata corrupt: {err}")))?;
        if metadata.project_id != project_id
            || metadata.changeset_id != changeset_id
            || metadata.base_snapshot_id != base_snapshot_id
        {
            return Err(CoreError::conflict(
                "staging directory belongs to a different ChangeSet or base snapshot",
            ));
        }
        let _ = fs::remove_file(staging.join(SEED_MARKER));
        return Ok(());
    }
    if staging.exists() {
        let mut entries = fs::read_dir(staging)?;
        if entries.next().is_some() {
            return Err(CoreError::conflict(
                "staging directory exists without ARCHMELD metadata; refusing to overwrite it",
            ));
        }
    } else {
        fs::create_dir_all(staging)?;
    }
    fs::write(
        &marker,
        b"seeding
",
    )?;
    let base = snapshot_files(store, project_id, base_snapshot_id)?;
    let allowed: Vec<String> = payload["allowed_paths"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|value| value.as_str().map(str::to_string))
        .collect();
    let read_set: Vec<String> = payload["read_set"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|value| value.as_str().map(str::to_string))
        .collect();
    let mut seeded = Vec::new();
    for (path, file) in &base {
        let wanted = in_scope(path, &allowed) || read_set.iter().any(|item| item == path);
        if !wanted {
            continue;
        }
        let bytes = store.read_blob(&file.digest)?.ok_or_else(|| {
            CoreError::Storage(format!(
                "base snapshot blob {} for {path} is missing",
                file.digest
            ))
        })?;
        let target = staging.join(path);
        if let Some(parent) = target.parent() {
            fs::create_dir_all(parent)?;
        }
        fs::write(&target, &bytes)?;
        seeded.push(StagingMetadataFile {
            path: path.clone(),
            digest: file.digest.clone(),
            size: file.size,
        });
    }
    let metadata = StagingMetadata {
        version: CHANGESET_VERSION.to_string(),
        project_id: project_id.to_string(),
        changeset_id: changeset_id.to_string(),
        base_snapshot_id: base_snapshot_id.to_string(),
        files: seeded,
        created_at: crate::store::now_unix(),
    };
    let text = hash::canonical_json(
        &serde_json::to_value(&metadata)
            .map_err(|err| CoreError::Storage(format!("staging metadata encode: {err}")))?,
    )
    .map_err(|err| CoreError::invalid(format!("staging metadata: {err}")))?;
    fs::write(&metadata_path, text)?;
    let _ = fs::remove_file(&marker);
    Ok(())
}

fn walk_staging(root: &Path, max_file_bytes: u64) -> Result<BTreeMap<String, StagedFile>> {
    if is_link_like(&fs::symlink_metadata(root)?) {
        return Err(CoreError::invalid(
            "the staging root itself is a link or reparse point",
        ));
    }
    let mut files = BTreeMap::new();
    let mut case_seen: BTreeMap<String, String> = BTreeMap::new();
    walk_dir(root, root, &mut files, &mut case_seen, max_file_bytes)?;
    Ok(files)
}

fn walk_dir(
    root: &Path,
    dir: &Path,
    files: &mut BTreeMap<String, StagedFile>,
    case_seen: &mut BTreeMap<String, String>,
    max_file_bytes: u64,
) -> Result<()> {
    for entry in fs::read_dir(dir)? {
        let entry = entry?;
        let path = entry.path();
        let relative = path
            .strip_prefix(root)
            .map_err(|_| CoreError::Storage("staging path escaped its root".to_string()))?;
        let rel_text = relative
            .to_string_lossy()
            .replace(std::path::MAIN_SEPARATOR, "/");
        if rel_text == STAGING_METADATA || rel_text == SEED_MARKER {
            continue;
        }
        if relative
            .file_name()
            .and_then(|name| name.to_str())
            .map(|name| name.starts_with(".archmeld."))
            .unwrap_or(false)
        {
            return Err(CoreError::invalid(format!(
                "reserved staging path {rel_text} is not a reviewable source file"
            )));
        }
        let metadata = fs::symlink_metadata(&path)?;
        let file_type = metadata.file_type();
        if is_link_like(&metadata) {
            return Err(CoreError::invalid(format!(
                "symlink_in_staging: {rel_text} is a symlink or reparse point"
            )));
        }
        if file_type.is_dir() {
            walk_dir(root, &path, files, case_seen, max_file_bytes)?;
            continue;
        }
        if !file_type.is_file() {
            return Err(CoreError::invalid(format!(
                "unsupported file type in staging: {rel_text}"
            )));
        }
        validate::validate_path("staging.path", &rel_text)?;
        if metadata.len() > max_file_bytes {
            return Err(CoreError::invalid(format!(
                "resource_exhausted: staged file {rel_text} exceeds {max_file_bytes} bytes"
            )));
        }
        if files.len() >= MAX_STAGED_FILES {
            return Err(CoreError::invalid(format!(
                "resource_exhausted: staging holds more than {MAX_STAGED_FILES} files"
            )));
        }
        let folded = rel_text.to_lowercase();
        if let Some(existing) = case_seen.insert(folded, rel_text.clone()) {
            return Err(CoreError::invalid(format!(
                "case-insensitive staging collision: {existing} and {rel_text}"
            )));
        }
        let bytes = fs::read(&path)?;
        if std::str::from_utf8(&bytes).is_err() {
            return Err(CoreError::unsupported(format!(
                "binary modifications are not enabled in v1: {rel_text}"
            )));
        }
        files.insert(
            rel_text,
            StagedFile {
                digest: hash::digest_bytes(&bytes),
                size: bytes.len() as u64,
            },
        );
    }
    Ok(())
}

fn is_link_like(metadata: &fs::Metadata) -> bool {
    #[cfg(windows)]
    {
        use std::os::windows::fs::MetadataExt;
        const FILE_ATTRIBUTE_REPARSE_POINT: u32 = 0x0400;
        metadata.file_attributes() & FILE_ATTRIBUTE_REPARSE_POINT != 0
    }
    #[cfg(not(windows))]
    {
        metadata.file_type().is_symlink()
    }
}

/// Reject a link or reparse point anywhere between the ledger's staging base
/// and this ChangeSet's directory. Without this check a junction planted at
/// `<state>/staging/<project>` would receive materialized bytes before the
/// recursive scan ever runs.
fn ensure_staging_segments(store: &Store, staging: &Path) -> Result<()> {
    let base = store.root().join("staging");
    fs::create_dir_all(&base)?;
    let canonical_root = fs::canonicalize(store.root())?;
    let canonical_base = fs::canonicalize(&base)?;
    if !canonical_base.starts_with(&canonical_root) {
        return Err(CoreError::invalid(
            "the staging base escapes the ledger state root",
        ));
    }
    let relative = staging
        .strip_prefix(&base)
        .map_err(|_| CoreError::invalid("staging path is outside the staging base"))?;
    let mut current = canonical_base;
    for component in relative.components() {
        let Component::Normal(name) = component else {
            return Err(CoreError::invalid(
                "staging path is not a plain relative path",
            ));
        };
        current.push(name);
        match fs::symlink_metadata(&current) {
            Ok(metadata) if is_link_like(&metadata) => {
                return Err(CoreError::invalid(
                    "the staging path traverses a symlink or reparse point",
                ))
            }
            Ok(_) => {}
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => break,
            Err(error) => return Err(CoreError::from(error)),
        }
    }
    Ok(())
}

fn staging_root(store: &Store, project_id: &str, changeset_id: &str) -> Result<PathBuf> {
    validate::require_id("project_id", project_id)?;
    validate::require_id("changeset_id", changeset_id)?;
    Ok(store
        .root()
        .join("staging")
        .join(project_id)
        .join(changeset_id))
}

fn ensure_staging_outside_workspace(store: &Store, project_id: &str, staging: &Path) -> Result<()> {
    let canonical_staging = canonicalize_existing_ancestor(staging)?;
    let mut statement = store
        .conn
        .prepare("SELECT root_path FROM project_roots WHERE project_id=?1 AND active=1")?;
    let roots = statement.query_map((project_id,), |row| row.get::<_, String>(0))?;
    for root in roots {
        let root = root?;
        let canonical_root = fs::canonicalize(&root).unwrap_or_else(|_| PathBuf::from(root));
        if canonical_staging.starts_with(&canonical_root) {
            return Err(CoreError::invalid(
                "the staging directory must not be inside the analyzed repository",
            ));
        }
    }
    Ok(())
}

fn canonicalize_existing_ancestor(path: &Path) -> Result<PathBuf> {
    let mut current = path;
    loop {
        if current.exists() {
            return fs::canonicalize(current).map_err(CoreError::from);
        }
        match current.parent() {
            Some(parent) => current = parent,
            None => {
                return Err(CoreError::Storage(
                    "staging path has no existing ancestor".to_string(),
                ))
            }
        }
    }
}

fn snapshot_files(
    store: &Store,
    project_id: &str,
    snapshot_id: &str,
) -> Result<BTreeMap<String, BaseFile>> {
    let mut statement = store.conn.prepare(
        "SELECT path, blob_digest, size_bytes, executable FROM files
         WHERE project_id=?1 AND snapshot_id=?2 ORDER BY path",
    )?;
    let rows = statement.query_map((project_id, snapshot_id), |row| {
        Ok((
            row.get::<_, String>(0)?,
            row.get::<_, String>(1)?,
            row.get::<_, i64>(2)?,
            row.get::<_, i64>(3)?,
        ))
    })?;
    let mut files = BTreeMap::new();
    for row in rows {
        let (path, digest, size, executable) = row?;
        files.insert(
            path,
            BaseFile {
                digest,
                size: size.max(0) as u64,
                executable: executable == 1,
            },
        );
    }
    Ok(files)
}

pub fn in_scope(path: &str, allowed_paths: &[String]) -> bool {
    allowed_paths.iter().any(|scope| {
        path == scope
            || (path.len() > scope.len()
                && path.starts_with(scope)
                && path.as_bytes()[scope.len()] == b'/')
    })
}

fn verification_id(payload: &Value, revision: i64, kind: &str) -> String {
    let identity = format!(
        "{}:{}:{}:{}",
        payload["project_id"].as_str().unwrap_or(""),
        payload["id"].as_str().unwrap_or(""),
        payload["base_snapshot_id"].as_str().unwrap_or(""),
        kind
    );
    let digest = hash::digest_bytes(identity.as_bytes());
    let hex = digest.strip_prefix("sha256:").unwrap_or(&digest);
    format!("staging.{}.{}.{}", &hex[..24], revision, kind)
}

fn preview(store: &Store, digest: Option<&str>, max_bytes: u64) -> Result<Value> {
    let Some(digest) = digest else {
        return Ok(Value::Null);
    };
    let bytes = store
        .read_blob(digest)?
        .ok_or_else(|| CoreError::Storage(format!("blob {digest} is missing from CAS")))?;
    let truncated = bytes.len() as u64 > max_bytes;
    let slice = &bytes[..(bytes.len() as u64).min(max_bytes) as usize];
    let mut end = slice.len();
    while end > 0 && std::str::from_utf8(&slice[..end]).is_err() {
        end -= 1;
    }
    let text = std::str::from_utf8(&slice[..end]).unwrap_or("");
    Ok(json!({
        "digest": digest,
        "size": bytes.len(),
        "text": text,
        "truncated": truncated,
    }))
}

pub(crate) fn ensure_plan_current(store: &Store, payload: &Value) -> Result<()> {
    let plan = get_plan(
        store,
        payload["project_id"].as_str().unwrap_or(""),
        payload["plan_id"].as_str().unwrap_or(""),
    )?;
    if plan["status"] != "approved" {
        return Err(CoreError::conflict(format!(
            "plan {} is {}; the ChangeSet cannot be prepared or approved",
            payload["plan_id"], plan["status"]
        )));
    }
    if plan["revision"].as_i64() != payload["plan_revision"].as_i64() {
        return Err(CoreError::conflict(
            "stale_plan: the plan has a newer revision",
        ));
    }
    let current: Option<(String, String)> = store
        .conn
        .query_row(
            "SELECT digest, contract_id FROM behavior_contracts
             WHERE project_id=?1 AND contract_id=?2 ORDER BY revision DESC LIMIT 1",
            (
                payload["project_id"].as_str().unwrap_or(""),
                payload["contract_id"].as_str().unwrap_or(""),
            ),
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .ok();
    match current {
        Some((digest, _)) if digest == payload["contract_digest"].as_str().unwrap_or("") => Ok(()),
        _ => Err(CoreError::conflict(
            "stale_contract: the frozen contract changed",
        )),
    }
}

fn validate_transform(store: &Store, transform: &TransformRecord) -> Result<()> {
    if !matches!(
        transform.method.as_str(),
        "native_rename" | "structured_codemod" | "llm_patch" | "manual_edit"
    ) {
        return Err(CoreError::invalid(format!(
            "unsupported transform method {}",
            transform.method
        )));
    }
    validate::require_non_empty("transform.tool.name", &transform.tool.name)?;
    validate::require_non_empty("transform.tool.version", &transform.tool.version)?;
    validate::require_digest(
        "transform.tool.identity_digest",
        &transform.tool.identity_digest,
    )?;
    if store.blob_info(&transform.tool.identity_digest)?.is_none() {
        return Err(CoreError::invalid(
            "transform.tool.identity_digest is not in the object store",
        ));
    }
    for movement in &transform.declared_moves {
        validate::validate_path("declared_moves[].from", &movement.from)?;
        validate::validate_path("declared_moves[].to", &movement.to)?;
    }
    if transform.method == "native_rename" && transform.declared_moves.is_empty() {
        return Err(CoreError::invalid(
            "a native_rename transform must declare its moves",
        ));
    }
    if transform.method != "native_rename" && !transform.declared_moves.is_empty() {
        return Err(CoreError::invalid(
            "only native_rename transforms may declare moves",
        ));
    }
    for artifact in [&transform.patch_artifact, &transform.prompt_artifact]
        .into_iter()
        .flatten()
    {
        validate::require_digest("transform artifact", artifact)?;
        if store.blob_info(artifact)?.is_none() {
            return Err(CoreError::invalid(format!(
                "transform artifact {artifact} is not in the object store"
            )));
        }
    }
    if transform.method == "llm_patch" && transform.patch_artifact.is_none() {
        return Err(CoreError::invalid(
            "an llm_patch transform must record its patch_artifact",
        ));
    }
    for note in &transform.notes {
        validate::require_non_empty("transform.notes[]", note)?;
    }
    Ok(())
}

fn require_toolchain_artifact(store: &Store, digest: &str) -> Result<()> {
    if store.blob_info(digest)?.is_none() {
        return Err(CoreError::invalid(format!(
            "toolchain_digest {digest} is not an object-store artifact"
        )));
    }
    Ok(())
}

fn validate_snapshot_options(options: &SnapshotCaptureOptions) -> Result<()> {
    if options.max_file_bytes == 0 || options.max_file_bytes > 268_435_456 {
        return Err(CoreError::invalid(
            "snapshot max_file_bytes is out of range",
        ));
    }
    if options.max_retries > 10 {
        return Err(CoreError::invalid("snapshot max_retries must be <= 10"));
    }
    let mut seen = BTreeSet::new();
    for dir in &options.excluded_dirs {
        validate::require_non_empty("snapshot excluded_dirs[]", dir)?;
        if !seen.insert(dir) {
            return Err(CoreError::invalid("duplicate excluded directory"));
        }
    }
    Ok(())
}

fn snapshot_manifest_digests(
    build_config: &Value,
    environment: &Value,
) -> Result<(String, String)> {
    let build = hash::digest_value(&json!({
        "version": "1.0",
        "manifest_kind": "build",
        "manifest": build_config,
    }))
    .map_err(|err| CoreError::invalid(format!("build config: {err}")))?;
    let env = hash::digest_value(&json!({
        "version": "1.0",
        "manifest_kind": "environment",
        "manifest": environment,
    }))
    .map_err(|err| CoreError::invalid(format!("environment: {err}")))?;
    Ok((build, env))
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

fn transition_allowed(from: &str, to: &str) -> bool {
    matches!(
        (from, to),
        ("draft", "approved")
            | ("draft", "rejected")
            | ("approved", "preparing")
            | ("approved", "stale")
            | ("approved", "rejected")
            | ("preparing", "prepared")
            | ("preparing", "failed")
            | ("prepared", "applying")
            | ("prepared", "stale")
            | ("prepared", "rejected")
            | ("applying", "applied")
            | ("applied", "verifying")
            | ("verifying", "accepted")
            | ("verifying", "failed")
            | ("applying", "failed")
            | ("applying", "conflicted")
            | ("applied", "recovering")
            | ("failed", "recovering")
            | ("conflicted", "recovering")
            | ("recovering", "recovered")
            | ("recovering", "conflicted")
            | ("recovering", "failed")
            | ("stale", "draft")
            | ("failed", "draft")
    )
}

fn approval_payload(
    store: &Store,
    project_id: &str,
    approval_id: &str,
    payload: &Value,
    prepared_digest: &str,
) -> Result<Value> {
    let (actor, expires_at, created_at): (String, Option<i64>, i64) = store.conn.query_row(
        "SELECT actor, expires_at, created_at FROM approvals
         WHERE project_id=?1 AND approval_id=?2",
        (project_id, approval_id),
        |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
    )?;
    Ok(json!({
        "changeset_id": payload["id"],
        "revision": payload["revision"],
        "prepared_digest": prepared_digest,
        "approval": {
            "approval_id": approval_id,
            "target_kind": "prepared_change",
            "target_id": payload["id"],
            "target_revision": payload["revision"],
            "target_digest": prepared_digest,
            "actor": actor,
            "decision": "approve",
            "expires_at": expires_at,
            "created_at": created_at,
        },
    }))
}
