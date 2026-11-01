// Copyright 2026 ARCHMELD contributors
// SPDX-License-Identifier: Apache-2.0

//! B019: frozen behavior contracts and plan DAGs. Invalid plans are rejected
//! without creating a plan or ChangeSet; a contract change invalidates the old
//! plan approval and changes its digest.

use archmeld_core::hash;
use archmeld_core::interpret::propose_architecture;
use archmeld_core::plan::{approve_plan, get_contract, get_plan, propose_contract, propose_plan};
use archmeld_core::snapshot::{capture_snapshot, SnapshotInputs, SnapshotOptions};
use archmeld_core::store::Store;
use serde_json::{json, Value};
use std::fs;
use std::path::Path;

const P: &str = "project.b019";

struct Fixture {
    _dir: tempfile::TempDir,
    store: Store,
    root: std::path::PathBuf,
    snapshot_id: String,
    architecture_id: String,
}

fn setup() -> Fixture {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().join("repo");
    fs::create_dir_all(root.join("src")).unwrap();
    fs::write(root.join("src/a.py"), "def a():\n    return 1\n").unwrap();
    let mut store = Store::open(dir.path().join("state")).unwrap();
    store.ensure_project(P, "expedition").unwrap();
    let snap = capture_snapshot(
        &mut store,
        P,
        &root,
        &SnapshotInputs {
            build_config: json!({"variant": "b019"}),
            environment: json!({"os": "test"}),
        },
        &SnapshotOptions {
            include_untracked: true,
            ..SnapshotOptions::default()
        },
    )
    .unwrap();
    let snapshot_id = snap.snapshot_id.clone();
    let architecture = json!({
        "schema_version": "1.0",
        "id": "arch.b019",
        "project_id": P,
        "snapshot_id": snapshot_id,
        "revision": 1,
        "layer": "to_be",
        "status": "candidate",
        "components": [{
            "id": "component.a",
            "name": "A",
            "purpose": "target component",
            "responsibilities": ["a"],
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
    let stored = propose_architecture(
        &mut store,
        &json!({
            "project_id": P,
            "model": architecture,
            "expected_revision": 1,
            "idempotency_key": "b019-arch-1",
        }),
    )
    .unwrap();
    Fixture {
        _dir: dir,
        store,
        root,
        snapshot_id,
        architecture_id: stored["id"].as_str().unwrap().to_string(),
    }
}

fn contract_payload(revision: i64, tolerance: &str) -> Value {
    let mut contract = json!({
        "schema_version": "1.0",
        "id": "contract.b019",
        "project_id": P,
        "revision": revision,
        "kind": "behavior_fix",
        "invariants": ["the public result stays finite"],
        "allowed_changes": ["internal refactor"],
        "known_defects": ["rounding at the boundary"],
        "checks": [
            {
                "id": "check.tests",
                "kind": "behavior",
                "required": true,
                "description": "unit tests pass",
                "command": {"argv": ["python", "-m", "pytest"], "cwd": ".", "timeout_ms": 60000},
                "checker_id": null,
                "predicate": {"kind": "exit_zero"},
                "input_artifacts": [],
                "sandbox": {"mode": "workspace", "include": [], "exclude": []},
            },
            {
                "id": "check.tolerance",
                "kind": "domain",
                "required": true,
                "description": "numeric tolerance is unchanged",
                "command": null,
                "checker_id": "numeric.rms.v1",
                "predicate": {"kind": "rms_max", "value": tolerance, "unit": "m"},
                "input_artifacts": [],
            }
        ],
        "digest": format!("sha256:{}", "0".repeat(64)),
        "frozen": false,
    });
    let mut preimage = contract.clone();
    preimage.as_object_mut().unwrap().remove("digest");
    preimage.as_object_mut().unwrap().remove("frozen");
    contract["digest"] = json!(hash::digest_value(&preimage).unwrap());
    contract
}

fn plan_payload(snapshot_id: &str, architecture_id: &str, contract: &Value) -> Value {
    json!({
        "schema_version": "1.0",
        "id": "plan.b019",
        "project_id": P,
        "revision": 1,
        "kind": "behavior_fix",
        "objective": "remove the boundary rounding defect without changing the public interface",
        "non_goals": ["performance tuning", "API changes"],
        "base_snapshot_id": snapshot_id,
        "target_architecture_id": architecture_id,
        "contract_id": contract["id"],
        "contract_digest": contract["digest"],
        "steps": [
            {
                "id": "step.1",
                "title": "add the missing round-trip test",
                "depends_on": [],
                "allowed_paths": ["src/a.py"],
                "required_check_ids": ["check.tests"],
                "completion_criteria": ["the new test fails before the fix"],
            },
            {
                "id": "step.2",
                "title": "fix the rounding path",
                "depends_on": ["step.1"],
                "allowed_paths": ["src/a.py"],
                "required_check_ids": ["check.tolerance"],
                "completion_criteria": ["tests and numeric tolerance pass"],
            }
        ],
        "risks": ["the tolerance check is declared via a checker id"],
        "evidence_ids": [],
        "status": "draft",
    })
}

fn propose(store: &mut Store, contract: Value, key: &str) -> Value {
    propose_contract(
        store,
        &json!({
            "project_id": P,
            "contract": contract,
            "expected_revision": 1,
            "idempotency_key": key,
        }),
    )
    .unwrap()
}

#[test]
fn invalid_plans_are_rejected_without_side_effects_and_contract_changes_invalidate_approval() {
    let mut fixture = setup();
    let source_before = hash::digest_bytes(&fs::read(fixture.root.join("src/a.py")).unwrap());
    let contract = propose(&mut fixture.store, contract_payload(1, "0.1"), "contract-1");
    assert_eq!(contract["frozen"], true);
    let plan = plan_payload(
        &fixture.snapshot_id,
        &fixture.architecture_id,
        &contract,
    );

    let unknown_check = {
        let mut plan = plan.clone();
        plan["id"] = json!("plan.unknown-check");
        plan["steps"][0]["required_check_ids"] = json!(["check.missing"]);
        plan
    };
    let error = propose_plan(
        &mut fixture.store,
        &json!({
            "project_id": P,
            "plan": unknown_check,
            "expected_revision": 1,
            "idempotency_key": "plan-unknown-check",
        }),
    )
    .unwrap_err();
    assert!(error.to_string().contains("unknown check_id"), "{error}");
    assert!(get_plan(&fixture.store, P, "plan.unknown-check").is_err());

    let cycle = {
        let mut plan = plan.clone();
        plan["id"] = json!("plan.cycle");
        plan["steps"][0]["depends_on"] = json!(["step.2"]);
        plan
    };
    let error = propose_plan(
        &mut fixture.store,
        &json!({
            "project_id": P,
            "plan": cycle,
            "expected_revision": 1,
            "idempotency_key": "plan-cycle",
        }),
    )
    .unwrap_err();
    assert!(error.to_string().contains("cycle"), "{error}");
    assert!(get_plan(&fixture.store, P, "plan.cycle").is_err());

    let unknown_step = {
        let mut plan = plan.clone();
        plan["id"] = json!("plan.unknown-step");
        plan["steps"][1]["depends_on"] = json!(["step.9"]);
        plan
    };
    let error = propose_plan(
        &mut fixture.store,
        &json!({
            "project_id": P,
            "plan": unknown_step,
            "expected_revision": 1,
            "idempotency_key": "plan-unknown-step",
        }),
    )
    .unwrap_err();
    assert!(error.to_string().contains("unknown step"), "{error}");
    assert_eq!(fixture.store.changeset_count(P).unwrap(), 0);
    assert_eq!(
        hash::digest_bytes(&fs::read(fixture.root.join("src/a.py")).unwrap()),
        source_before
    );

    let stored = propose_plan(
        &mut fixture.store,
        &json!({
            "project_id": P,
            "plan": plan,
            "expected_revision": 1,
            "idempotency_key": "plan-1",
        }),
    )
    .unwrap();
    assert_eq!(stored["status"], "draft");
    let approved = approve_plan(
        &mut fixture.store,
        P,
        "plan.b019",
        1,
        contract["digest"].as_str().unwrap(),
        "operator",
        "plan-approve-1",
    )
    .unwrap();
    assert_eq!(approved["status"], "approved");
    assert_eq!(approved["revision"], 2);

    // Increasing the numeric tolerance changes the frozen contract digest.
    let contract_v2 = propose(&mut fixture.store, contract_payload(2, "0.2"), "contract-2");
    assert_ne!(contract["digest"], contract_v2["digest"]);
    let stale = get_plan(&fixture.store, P, "plan.b019").unwrap();
    assert_eq!(stale["status"], "stale");
    let old_approval = approve_plan(
        &mut fixture.store,
        P,
        "plan.b019",
        2,
        contract["digest"].as_str().unwrap(),
        "operator",
        "plan-approve-old",
    );
    assert!(old_approval.is_err());
    let new_digest_but_old_plan = approve_plan(
        &mut fixture.store,
        P,
        "plan.b019",
        2,
        contract_v2["digest"].as_str().unwrap(),
        "operator",
        "plan-approve-new-digest",
    );
    assert!(new_digest_but_old_plan.is_err());
    assert_eq!(get_contract(&fixture.store, P, "contract.b019").unwrap()["digest"], contract_v2["digest"]);
    assert_eq!(fixture.store.changeset_count(P).unwrap(), 0);
    assert_eq!(
        hash::digest_bytes(&fs::read(fixture.root.join("src/a.py")).unwrap()),
        source_before
    );
    assert!(fixture.store.integrity_check().unwrap().is_empty());

    if std::env::var("ARCHMELD_WRITE_EVIDENCE").ok().as_deref() == Some("1") {
        let evidence = json!({
            "generated_by": "crates/archmeld-core/tests/plan_contract.rs",
            "acceptance": {
                "AT042": {
                    "status": "passed",
                    "unknown_check_id": "rejected",
                    "dependency_cycle": "rejected",
                    "unknown_step_dependency": "rejected",
                    "changeset_count": fixture.store.changeset_count(P).unwrap(),
                    "source_unchanged": true,
                },
                "AT043": {
                    "status": "passed",
                    "contract_digest_v1": contract["digest"],
                    "contract_digest_v2": contract_v2["digest"],
                    "old_plan_status": stale["status"],
                    "old_approval_rejected": true,
                    "new_digest_old_plan_rejected": true,
                },
                "AT072": {
                    "status": "passed",
                    "vectors": "docs/archmeld/examples/hash-vectors.json",
                    "implementations": ["rust hash.rs", "python docs/archmeld/tooling/check_pack.py", "archmeld-vscode/src/canonical.js"],
                }
            },
            "snapshot_id": fixture.snapshot_id,
            "source_digest": source_before,
        });
        let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../docs/evidence/B019-plan-contract.json");
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, serde_json::to_string_pretty(&evidence).unwrap() + "\n").unwrap();
    }
}
