// Copyright 2026 ARCHMELD contributors
// SPDX-License-Identifier: Apache-2.0

//! Shared fixtures for ChangeSet staging/apply acceptance tests.
#![allow(dead_code)]

use archmeld_core::hash;
use archmeld_core::interpret::propose_architecture;
use archmeld_core::manager::open_project_for_handle;
use archmeld_core::plan::{approve_plan, propose_contract, propose_plan};
use archmeld_core::snapshot::{capture_snapshot, SnapshotInputs, SnapshotOptions};
use archmeld_core::store::Store;
use serde_json::{json, Value};
use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};

pub struct Fixture {
    pub _dir: tempfile::TempDir,
    pub store: Store,
    pub root: PathBuf,
    pub project: String,
    pub handle: String,
    pub snapshot_id: String,
    pub config_digest: String,
    pub architecture_id: String,
    pub contract_id: String,
    pub contract_digest: String,
    pub toolchain_digest: String,
    pub tool_identity: String,
    pub build_config: Value,
    pub environment: Value,
}

pub fn setup(name: &str) -> Fixture {
    setup_prepared(name, |root| {
        fs::create_dir_all(root.join("src/calc")).unwrap();
        fs::create_dir_all(root.join("config")).unwrap();
        fs::write(root.join("src/a.py"), "VALUE = 1\n").unwrap();
        fs::write(root.join("src/b.py"), "OTHER = 2\n").unwrap();
        fs::write(root.join("src/Name.py"), "NAME = \'case\'\n").unwrap();
        fs::write(root.join("src/calc/keep.py"), "KEEP = 0\n").unwrap();
        fs::write(root.join("config/settings.toml"), "mode = \"base\"\n").unwrap();
    })
}

/// Build a fixture around caller-provided repository content. The preparation
/// closure runs before the base snapshot is captured.
pub fn setup_prepared<F>(name: &str, prepare: F) -> Fixture
where
    F: FnOnce(&Path),
{
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().join("repo");
    fs::create_dir_all(&root).unwrap();
    prepare(&root);
    let state = dir.path().join("state");
    let mut store = Store::open(&state).unwrap();
    let handle = format!("ws.{name}");
    let project = open_project_for_handle(
        &mut store,
        &handle,
        &root,
        "expedition",
        &state.join("sessions"),
        json!({"profile": "b020", "name": name}),
    )
    .unwrap();
    let project_id = project["id"].as_str().unwrap().to_string();
    let build_config = json!({"variant": name, "profile": "b020"});
    let environment = json!({"os": "windows", "arch": "x86_64"});
    let snapshot = capture_snapshot(
        &mut store,
        &project_id,
        &root,
        &SnapshotInputs {
            build_config: build_config.clone(),
            environment: environment.clone(),
        },
        &SnapshotOptions {
            include_untracked: true,
            ..SnapshotOptions::default()
        },
    )
    .unwrap();
    let architecture = json!({
        "schema_version": "1.0",
        "id": format!("arch.{name}"),
        "project_id": project_id,
        "snapshot_id": snapshot.snapshot_id,
        "revision": 1,
        "layer": "to_be",
        "status": "candidate",
        "components": [{
            "id": "component.staged",
            "name": "Staged",
            "purpose": "acceptance target",
            "responsibilities": ["staging"],
            "non_responsibilities": [],
            "member_entity_ids": [],
            "owned_state": [],
            "owned_resources": [],
            "runtime_placement": [],
            "evidence_ids": [],
            "open_questions": [],
        }],
        "dependencies": [],
        "rules": [],
        "claim_ids": [],
    });
    let architecture = propose_architecture(
        &mut store,
        &json!({
            "project_id": project_id,
            "model": architecture,
            "expected_revision": 1,
            "idempotency_key": format!("arch-{name}"),
        }),
    )
    .unwrap();
    let contract = contract_payload(&project_id, "contract", &format!("contract.{name}"));
    let contract = propose_contract(
        &mut store,
        &json!({
            "project_id": project_id,
            "contract": contract,
            "expected_revision": 1,
            "idempotency_key": format!("contract-{name}"),
        }),
    )
    .unwrap();
    let toolchain_digest = store
        .put_blob(b"{\"toolchain\":\"archmeld-b020\"}", "application/json")
        .unwrap();
    let tool_identity = store
        .put_blob(
            b"{\"tool\":\"test-tool\",\"version\":\"1\"}",
            "application/json",
        )
        .unwrap();
    Fixture {
        _dir: dir,
        store,
        root,
        project: project_id,
        handle,
        snapshot_id: snapshot.snapshot_id,
        config_digest: snapshot.snapshot["config_digest"]
            .as_str()
            .unwrap()
            .to_string(),
        architecture_id: architecture["id"].as_str().unwrap().to_string(),
        contract_id: contract["id"].as_str().unwrap().to_string(),
        contract_digest: contract["digest"].as_str().unwrap().to_string(),
        toolchain_digest,
        tool_identity,
        build_config,
        environment,
    }
}

fn contract_payload(project_id: &str, id: &str, contract_id: &str) -> Value {
    let mut contract = json!({
        "schema_version": "1.0",
        "id": contract_id,
        "project_id": project_id,
        "revision": 1,
        "kind": "structural_refactor",
        "invariants": ["behavior stays unchanged"],
        "allowed_changes": ["rename and internal refactor"],
        "known_defects": [],
        "checks": [{
            "id": "check.tests",
            "kind": "behavior",
            "required": true,
            "description": "tests pass",
            "command": {"argv": ["python", "-m", "pytest"], "cwd": ".", "timeout_ms": 60000},
            "checker_id": null,
            "predicate": {"kind": "exit_zero"},
            "input_artifacts": [],
            "sandbox": {"mode": "workspace", "include": [], "exclude": []},
        }],
        "digest": format!("sha256:{}", "0".repeat(64)),
        "frozen": false,
    });
    let _ = id;
    let mut preimage = contract.clone();
    preimage.as_object_mut().unwrap().remove("digest");
    preimage.as_object_mut().unwrap().remove("frozen");
    contract["digest"] = json!(hash::digest_value(&preimage).unwrap());
    contract
}

pub fn approved_plan(fixture: &mut Fixture, plan_id: &str, allowed_paths: &[&str]) -> Value {
    let plan = json!({
        "schema_version": "1.0",
        "id": plan_id,
        "project_id": fixture.project,
        "revision": 1,
        "kind": "structural_refactor",
        "objective": "apply a staged refactor",
        "non_goals": [],
        "base_snapshot_id": fixture.snapshot_id,
        "target_architecture_id": fixture.architecture_id,
        "contract_id": fixture.contract_id,
        "contract_digest": fixture.contract_digest,
        "steps": [{
            "id": "step.1",
            "title": "staged step",
            "depends_on": [],
            "allowed_paths": allowed_paths,
            "required_check_ids": ["check.tests"],
            "completion_criteria": ["staged content verified"],
        }],
        "risks": [],
        "evidence_ids": [],
        "status": "draft",
    });
    let plan = propose_plan(
        &mut fixture.store,
        &json!({
            "project_id": fixture.project,
            "plan": plan,
            "expected_revision": 1,
            "idempotency_key": format!("plan-{plan_id}"),
        }),
    )
    .unwrap();
    approve_plan(
        &mut fixture.store,
        &fixture.project,
        plan_id,
        plan["revision"].as_i64().unwrap(),
        &fixture.contract_digest,
        "operator",
        &format!("approve-{plan_id}"),
    )
    .unwrap()
}

pub fn create_changeset(
    fixture: &mut Fixture,
    changeset_id: &str,
    plan_id: &str,
    read_set: &[&str],
) -> Value {
    archmeld_core::changeset::create_changeset(
        &mut fixture.store,
        &json!({
            "schema_version": "1.0",
            "project_id": fixture.project,
            "changeset_id": changeset_id,
            "plan_id": plan_id,
            "plan_revision": 2,
            "step_id": "step.1",
            "read_set": read_set,
            "build_config": fixture.build_config,
            "environment": fixture.environment,
            "snapshot_options": serde_json::to_value(
                archmeld_core::changeset::SnapshotCaptureOptions {
                    include_untracked: true,
                    ..Default::default()
                }
            ).unwrap(),
            "expected_revision": 1,
            "idempotency_key": format!("create-{changeset_id}"),
        }),
    )
    .unwrap()
}

pub fn transform(fixture: &Fixture, method: &str, moves: Value) -> Value {
    json!({
        "method": method,
        "tool": {
            "name": "archmeld-test-tool",
            "version": "1.0.0",
            "identity_digest": fixture.tool_identity,
        },
        "declared_moves": moves,
        "patch_artifact": null,
        "prompt_artifact": null,
        "workspace_edit_operations": 0,
        "notes": ["acceptance fixture"],
    })
}

pub fn prepare(
    fixture: &mut Fixture,
    changeset_id: &str,
    expected_revision: i64,
    transform_value: Option<Value>,
    key_suffix: &str,
) -> Result<Value, archmeld_core::CoreError> {
    archmeld_core::changeset::prepare_changeset(
        &mut fixture.store,
        &json!({
            "project_id": fixture.project,
            "changeset_id": changeset_id,
            "expected_revision": expected_revision,
            "toolchain_digest": fixture.toolchain_digest,
            "transform": transform_value,
            "idempotency_key": format!("prepare-{changeset_id}-{expected_revision}-{key_suffix}"),
        }),
    )
}

pub fn approve_prepared(
    fixture: &mut Fixture,
    changeset_id: &str,
    revision: i64,
    prepared_digest: &str,
) -> Value {
    archmeld_core::changeset::approve_prepared_change(
        &mut fixture.store,
        &json!({
            "project_id": fixture.project,
            "changeset_id": changeset_id,
            "expected_revision": revision,
            "prepared_digest": prepared_digest,
            "actor": "operator",
            "idempotency_key": format!("approve-prepared-{changeset_id}-{revision}"),
        }),
    )
    .unwrap()
}

pub fn apply(
    fixture: &mut Fixture,
    changeset_id: &str,
    revision: i64,
    prepared_digest: &str,
    approval_id: &str,
    key: &str,
) -> Result<Value, archmeld_core::CoreError> {
    archmeld_core::apply::apply_changeset(
        &mut fixture.store,
        &json!({
            "project_id": fixture.project,
            "changeset_id": changeset_id,
            "expected_revision": revision,
            "prepared_digest": prepared_digest,
            "approval_id": approval_id,
            "actor": "operator",
            "workspace_handle": fixture.handle,
            "idempotency_key": key,
        }),
    )
}

pub fn apply_with_hook(
    fixture: &mut Fixture,
    changeset_id: &str,
    revision: i64,
    prepared_digest: &str,
    approval_id: &str,
    key: &str,
    ttl: u64,
    hook: &mut dyn FnMut(
        &mut Store,
        &archmeld_core::apply::ApplyEvent,
    ) -> archmeld_core::Result<()>,
) -> Result<Value, archmeld_core::CoreError> {
    archmeld_core::apply::apply_changeset_with_hook(
        &mut fixture.store,
        &json!({
            "project_id": fixture.project,
            "changeset_id": changeset_id,
            "expected_revision": revision,
            "prepared_digest": prepared_digest,
            "approval_id": approval_id,
            "actor": "operator",
            "workspace_handle": fixture.handle,
            "idempotency_key": key,
            "lease_ttl_secs": ttl,
        }),
        Some(hook),
    )
}

pub fn recover(
    fixture: &mut Fixture,
    changeset_id: &str,
    revision: i64,
    key: &str,
) -> Result<Value, archmeld_core::CoreError> {
    archmeld_core::apply::recover_changeset(
        &mut fixture.store,
        &json!({
            "project_id": fixture.project,
            "changeset_id": changeset_id,
            "expected_revision": revision,
            "actor": "operator",
            "workspace_handle": fixture.handle,
            "idempotency_key": key,
        }),
    )
}

pub fn staging_path(_fixture: &Fixture, prepared: &Value) -> PathBuf {
    PathBuf::from(prepared["staging"]["path"].as_str().unwrap())
}

pub fn tree_hashes(root: &Path) -> BTreeMap<String, String> {
    fn walk(root: &Path, dir: &Path, out: &mut BTreeMap<String, String>) {
        for entry in fs::read_dir(dir).unwrap() {
            let entry = entry.unwrap();
            let path = entry.path();
            let relative = path
                .strip_prefix(root)
                .unwrap()
                .to_string_lossy()
                .replace('\\', "/");
            let metadata = fs::symlink_metadata(&path).unwrap();
            if metadata.is_dir() {
                walk(root, &path, out);
            } else if metadata.is_file() {
                out.insert(relative, hash::digest_bytes(&fs::read(&path).unwrap()));
            }
        }
    }
    let mut out = BTreeMap::new();
    walk(root, root, &mut out);
    out
}

pub fn write_file(root: &Path, relative: &str, content: &str) -> String {
    let path = root.join(relative);
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).unwrap();
    }
    fs::write(&path, content).unwrap();
    hash::digest_bytes(content.as_bytes())
}

pub fn read_text(root: &Path, relative: &str) -> String {
    fs::read_to_string(root.join(relative)).unwrap()
}
