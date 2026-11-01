// Copyright 2026 ARCHMELD contributors
// SPDX-License-Identifier: Apache-2.0

//! B022 / AT050–AT052: frozen-check verification, missing/unsupported checks,
//! snapshot binding and process-tree resource limits.

mod common;

use archmeld_core::changeset::get_changeset;
use archmeld_core::hash;
use archmeld_core::plan::{approve_plan, propose_contract, propose_plan};
use archmeld_core::verification::{accept_changeset, list_verifications, verify_changeset};
use serde_json::{json, Value};
use std::fs;
use std::path::Path;
use std::sync::Mutex;

static EVIDENCE_LOCK: Mutex<()> = Mutex::new(());

fn contract_with_checks(fixture: &mut common::Fixture, revision: i64, checks: Vec<Value>) -> Value {
    let mut contract = json!({
        "schema_version": "1.0",
        "id": fixture.contract_id,
        "project_id": fixture.project,
        "revision": revision,
        "kind": "structural_refactor",
        "invariants": ["behavior stays unchanged"],
        "allowed_changes": ["internal refactor"],
        "known_defects": [],
        "checks": checks,
        "digest": format!("sha256:{}", "0".repeat(64)),
        "frozen": false,
    });
    let mut preimage = contract.clone();
    preimage.as_object_mut().unwrap().remove("digest");
    preimage.as_object_mut().unwrap().remove("frozen");
    contract["digest"] = json!(hash::digest_value(&preimage).unwrap());
    propose_contract(
        &mut fixture.store,
        &json!({
            "project_id": fixture.project,
            "contract": contract,
            "expected_revision": 1,
            "idempotency_key": format!("verify-contract-{revision}"),
        }),
    )
    .unwrap()
}

fn plan_with_contract(
    fixture: &mut common::Fixture,
    plan_id: &str,
    contract: &Value,
    allowed: &[&str],
    required_check_ids: &[&str],
) -> Value {
    let plan = json!({
        "schema_version": "1.0",
        "id": plan_id,
        "project_id": fixture.project,
        "revision": 1,
        "kind": "structural_refactor",
        "objective": "verify a staged refactor",
        "non_goals": [],
        "base_snapshot_id": fixture.snapshot_id,
        "target_architecture_id": fixture.architecture_id,
        "contract_id": contract["id"],
        "contract_digest": contract["digest"],
        "steps": [{
            "id": "step.1",
            "title": "verified step",
            "depends_on": [],
            "allowed_paths": allowed,
            "required_check_ids": required_check_ids,
            "completion_criteria": ["verification passes"],
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
            "idempotency_key": format!("verify-plan-{plan_id}"),
        }),
    )
    .unwrap();
    approve_plan(
        &mut fixture.store,
        &fixture.project,
        plan_id,
        plan["revision"].as_i64().unwrap(),
        contract["digest"].as_str().unwrap(),
        "operator",
        &format!("verify-approve-{plan_id}"),
    )
    .unwrap()
}

fn apply_change(
    fixture: &mut common::Fixture,
    plan_id: &str,
    changeset_id: &str,
    path: &str,
    content: &str,
) -> Value {
    common::create_changeset(fixture, changeset_id, plan_id, &[]);
    let seeded = common::prepare(fixture, changeset_id, 1, None, "seed").unwrap();
    let staging = common::staging_path(fixture, &seeded);
    common::write_file(&staging, path, content);
    let transform = common::transform(fixture, "manual_edit", json!([]));
    let prepared = common::prepare(
        fixture,
        changeset_id,
        seeded["revision"].as_i64().unwrap(),
        Some(transform),
        "transform",
    )
    .unwrap();
    let approval = common::approve_prepared(
        fixture,
        changeset_id,
        prepared["revision"].as_i64().unwrap(),
        prepared["prepared_digest"].as_str().unwrap(),
    );
    common::apply(
        fixture,
        changeset_id,
        prepared["revision"].as_i64().unwrap(),
        prepared["prepared_digest"].as_str().unwrap(),
        approval["approval"]["approval_id"].as_str().unwrap(),
        &format!("verify-apply-{changeset_id}"),
    )
    .unwrap()
}

fn verify(
    fixture: &mut common::Fixture,
    changeset_id: &str,
    revision: i64,
    key: &str,
    snapshot_id: Option<&str>,
) -> Result<Value, archmeld_core::CoreError> {
    verify_changeset(
        &mut fixture.store,
        &json!({
            "project_id": fixture.project,
            "changeset_id": changeset_id,
            "expected_revision": revision,
            "workspace_handle": fixture.handle,
            "execution_grant": {"grant_id": "grant.b022", "reason": "acceptance verification"},
            "environment": {"profile": "b022"},
            "snapshot_id": snapshot_id,
            "idempotency_key": key,
        }),
    )
}

fn accept(
    fixture: &mut common::Fixture,
    changeset_id: &str,
    revision: i64,
    verification_id: &str,
    key: &str,
) -> Result<Value, archmeld_core::CoreError> {
    accept_changeset(
        &mut fixture.store,
        &json!({
            "project_id": fixture.project,
            "changeset_id": changeset_id,
            "expected_revision": revision,
            "verification_id": verification_id,
            "workspace_handle": fixture.handle,
            "actor": "operator",
            "idempotency_key": key,
        }),
    )
}

fn check_command(argv: Vec<&str>, timeout_ms: u64, predicate: Value) -> Value {
    json!({
        "id": "check.command",
        "kind": "behavior",
        "required": true,
        "description": "command check",
        "command": {"argv": argv, "cwd": null, "timeout_ms": timeout_ms},
        "checker_id": null,
        "predicate": predicate,
        "input_artifacts": [],
        "sandbox": {"mode": "workspace", "include": [], "exclude": []},
    })
}

#[test]
fn missing_unsupported_and_zero_test_checks_never_pass() {
    let mut fixture = common::setup("b022-missing");
    let contract = contract_with_checks(
        &mut fixture,
        2,
        vec![
            check_command(
                vec!["python", "-c", "print('ok')"],
                5000,
                json!({"kind": "exit_code", "expected_codes": [0]}),
            ),
            json!({
                "id": "check.unsupported",
                "kind": "domain",
                "required": true,
                "description": "numeric checker that is not registered",
                "command": null,
                "checker_id": "numeric.rms.v1",
                "predicate": {"kind": "numeric_tolerance", "atol": "0.1", "rtol": "0.0"},
                "input_artifacts": [],
            }),
            json!({
                "id": "check.optional",
                "kind": "behavior",
                "required": false,
                "description": "optional invariant with no registered checker",
                "command": null,
                "checker_id": "optional.unknown",
                "predicate": {"kind": "invariant", "checker_id": "optional.unknown"},
                "input_artifacts": [],
            }),
        ],
    );
    plan_with_contract(
        &mut fixture,
        "plan.b022.missing",
        &contract,
        &["src/a.py"],
        &["check.command", "check.unsupported", "check.optional"],
    );
    let applied = apply_change(
        &mut fixture,
        "plan.b022.missing",
        "changeset.b022.missing",
        "src/a.py",
        "VALUE = 20\n",
    );
    assert_eq!(applied["state"], "applied");
    let verification = verify(
        &mut fixture,
        "changeset.b022.missing",
        applied["revision"].as_i64().unwrap(),
        "verify-missing",
        None,
    )
    .unwrap();
    assert_eq!(verification["state"], "failed");
    assert_eq!(verification["verdict"]["acceptance"], false);
    assert_eq!(verification["verdict"]["coverage"]["state"], "partial");
    let checks = verification["checks"].as_array().unwrap();
    let by_id = |id: &str| checks.iter().find(|check| check["check_id"] == id).unwrap();
    assert_eq!(by_id("check.command")["state"], "passed");
    assert_eq!(by_id("check.unsupported")["state"], "unsupported");
    assert_eq!(by_id("check.optional")["state"], "unsupported");
    let required_not_passed = verification["verdict"]["required_not_passed"]
        .as_array()
        .unwrap();
    assert_eq!(required_not_passed.len(), 1);
    assert_eq!(required_not_passed[0]["check_id"], "check.unsupported");
    let failed_state =
        get_changeset(&fixture.store, &fixture.project, "changeset.b022.missing").unwrap();
    assert_eq!(failed_state["state"], "failed");
    let rejected_accept = accept(
        &mut fixture,
        "changeset.b022.missing",
        failed_state["revision"].as_i64().unwrap(),
        verification["verification_id"].as_str().unwrap(),
        "accept-missing",
    )
    .unwrap_err();
    assert!(
        rejected_accept.to_string().contains("acceptance requires"),
        "{rejected_accept}"
    );

    // A report that exists but contains zero tests is a failure, not success.
    let zero_contract = contract_with_checks(
        &mut fixture,
        3,
        vec![
            check_command(
                vec![
                    "python",
                    "-c",
                    "import json;open('report.json','w').write(json.dumps({'tests':0,'failures':0,'errors':0}))",
                ],
                5000,
                json!({
                    "kind": "test_report",
                    "format": "json",
                    "report_path": "report.json",
                    "min_tests": 1,
                    "max_failures": 0
                }),
            ),
        ],
    );
    plan_with_contract(
        &mut fixture,
        "plan.b022.zero",
        &zero_contract,
        &["src/b.py"],
        &["check.command"],
    );
    let applied = apply_change(
        &mut fixture,
        "plan.b022.zero",
        "changeset.b022.zero",
        "src/b.py",
        "OTHER = 21\n",
    );
    let verification = verify(
        &mut fixture,
        "changeset.b022.zero",
        applied["revision"].as_i64().unwrap(),
        "verify-zero",
        None,
    )
    .unwrap();
    assert_eq!(verification["verdict"]["acceptance"], false);
    let zero_check = &verification["checks"].as_array().unwrap()[0];
    assert_eq!(zero_check["state"], "failed");
    assert_eq!(
        zero_check["detail"]["predicate"]["reason"],
        "the report contains fewer tests than required"
    );
    assert_eq!(zero_check["detail"]["predicate"]["tests"], 0);

    // A complete required set can be accepted; optional omissions stay visible.
    let complete_contract = contract_with_checks(
        &mut fixture,
        4,
        vec![check_command(
            vec!["python", "-c", "print('complete')"],
            5000,
            json!({"kind": "exit_code", "expected_codes": [0]}),
        )],
    );
    plan_with_contract(
        &mut fixture,
        "plan.b022.complete",
        &complete_contract,
        &["src/Name.py"],
        &["check.command"],
    );
    let applied = apply_change(
        &mut fixture,
        "plan.b022.complete",
        "changeset.b022.complete",
        "src/Name.py",
        "NAME = 'verified'\n",
    );
    let verification = verify(
        &mut fixture,
        "changeset.b022.complete",
        applied["revision"].as_i64().unwrap(),
        "verify-complete",
        None,
    )
    .unwrap();
    assert_eq!(verification["state"], "accepted");
    assert_eq!(verification["verdict"]["acceptance"], true);
    let verifying =
        get_changeset(&fixture.store, &fixture.project, "changeset.b022.complete").unwrap();
    assert_eq!(verifying["state"], "verifying");
    let accepted = accept(
        &mut fixture,
        "changeset.b022.complete",
        verifying["revision"].as_i64().unwrap(),
        verification["verification_id"].as_str().unwrap(),
        "accept-complete",
    )
    .unwrap();
    assert_eq!(accepted["state"], "accepted");
    assert_eq!(accepted["verification_id"], verification["verification_id"]);

    write_evidence(
        &fixture,
        "AT050",
        json!({
            "verification_id": verification["verification_id"],
            "unsupported_required_state": "unsupported",
            "optional_state": "unsupported",
            "required_not_passed": required_not_passed,
            "zero_test_state": zero_check["state"],
            "zero_test_reason": zero_check["detail"]["predicate"]["reason"],
            "complete_required_accepted": true,
            "changeset_after_accept": accepted["state"],
            "verifications": list_verifications(&fixture.store, &fixture.project, Some("changeset.b022.missing")).unwrap(),
        }),
    );
}

#[test]
fn staged_or_changed_snapshots_can_never_be_accepted() {
    let mut fixture = common::setup("b022-binding");
    let contract = contract_with_checks(
        &mut fixture,
        2,
        vec![check_command(
            vec!["python", "-c", "print('binding')"],
            5000,
            json!({"kind": "exit_code", "expected_codes": [0]}),
        )],
    );
    plan_with_contract(
        &mut fixture,
        "plan.b022.binding",
        &contract,
        &["src/calc/keep.py"],
        &["check.command"],
    );
    let applied = apply_change(
        &mut fixture,
        "plan.b022.binding",
        "changeset.b022.binding",
        "src/calc/keep.py",
        "KEEP = 99\n",
    );
    let resulting = applied["resulting_snapshot_id"]
        .as_str()
        .unwrap()
        .to_string();
    assert_ne!(resulting, fixture.snapshot_id);

    // A caller cannot attach verification to the pre-apply snapshot.
    let base_snapshot = fixture.snapshot_id.clone();
    let error = verify(
        &mut fixture,
        "changeset.b022.binding",
        applied["revision"].as_i64().unwrap(),
        "verify-staged-snapshot",
        Some(&base_snapshot),
    )
    .unwrap_err();
    assert!(
        error.to_string().contains("resulting applied snapshot"),
        "{error}"
    );

    // Change one applied file after the snapshot was captured; verification
    // must see evidence_invalid rather than reuse a staged pass.
    common::write_file(&fixture.root, "src/calc/keep.py", "KEEP = 123\n");
    let verification = verify(
        &mut fixture,
        "changeset.b022.binding",
        applied["revision"].as_i64().unwrap(),
        "verify-after-edit",
        None,
    )
    .unwrap();
    assert_eq!(verification["state"], "failed");
    assert_eq!(verification["verdict"]["acceptance"], false);
    let drift = verification["verdict"]["drift"].as_array().unwrap();
    assert_eq!(drift.len(), 1);
    assert_eq!(drift[0]["path"], "src/calc/keep.py");
    assert_eq!(drift[0]["state"], "changed");
    assert!(verification["verdict"]["reasons"]
        .as_array()
        .unwrap()
        .iter()
        .any(|reason| reason.as_str().unwrap().contains("evidence_invalid")));
    let failed = get_changeset(&fixture.store, &fixture.project, "changeset.b022.binding").unwrap();
    assert_eq!(failed["state"], "failed");
    let rejected = accept(
        &mut fixture,
        "changeset.b022.binding",
        failed["revision"].as_i64().unwrap(),
        verification["verification_id"].as_str().unwrap(),
        "accept-binding",
    )
    .unwrap_err();
    assert!(
        rejected.to_string().contains("acceptance requires"),
        "{rejected}"
    );

    write_evidence(
        &fixture,
        "AT051",
        json!({
            "base_snapshot_id": fixture.snapshot_id,
            "resulting_snapshot_id": resulting,
            "staged_snapshot_rejected": true,
            "drift": drift,
            "verdict": verification["verdict"],
            "changeset_state": failed["state"],
        }),
    );
}

#[test]
fn timeout_is_explicit_and_the_spawned_tree_is_reaped() {
    let mut fixture = common::setup("b022-timeout");
    let contract = contract_with_checks(
        &mut fixture,
        2,
        vec![check_command(
            vec![
                "python",
                "-c",
                "import subprocess,sys,time; subprocess.Popen([sys.executable,'-c','import time;time.sleep(60)']); time.sleep(60)",
            ],
            1000,
            json!({"kind": "exit_code", "expected_codes": [0]}),
        )],
    );
    plan_with_contract(
        &mut fixture,
        "plan.b022.timeout",
        &contract,
        &["src/a.py"],
        &["check.command"],
    );
    let applied = apply_change(
        &mut fixture,
        "plan.b022.timeout",
        "changeset.b022.timeout",
        "src/a.py",
        "VALUE = 30\n",
    );
    let started = std::time::Instant::now();
    let verification = verify(
        &mut fixture,
        "changeset.b022.timeout",
        applied["revision"].as_i64().unwrap(),
        "verify-timeout",
        None,
    )
    .unwrap();
    let elapsed = started.elapsed();
    assert!(
        elapsed.as_secs() < 20,
        "verification retried or hung: {elapsed:?}"
    );
    assert_eq!(verification["verdict"]["acceptance"], false);
    let check = &verification["checks"].as_array().unwrap()[0];
    assert_eq!(check["state"], "timed_out");
    assert_eq!(check["timed_out"], true);
    let audit = &check["detail"]["process_audit"];
    assert_eq!(audit["state"], "complete");
    assert_eq!(audit["survivors"].as_array().unwrap().len(), 0);
    assert!(audit["cleanup"]
        .as_array()
        .unwrap()
        .iter()
        .any(|item| item["result"] == "terminated"));
    let failed = get_changeset(&fixture.store, &fixture.project, "changeset.b022.timeout").unwrap();
    assert_eq!(failed["state"], "failed");

    write_evidence(
        &fixture,
        "AT052",
        json!({
            "timeout_ms": 1000,
            "elapsed_ms": elapsed.as_millis(),
            "check_state": check["state"],
            "process_audit": audit,
            "retried": false,
            "acceptance": verification["verdict"]["acceptance"],
            "changeset_state": failed["state"],
        }),
    );
}

fn write_evidence(fixture: &common::Fixture, case_id: &str, case: Value) {
    if std::env::var("ARCHMELD_WRITE_EVIDENCE").ok().as_deref() != Some("1") {
        return;
    }
    let _guard = EVIDENCE_LOCK.lock().unwrap();
    let path =
        Path::new(env!("CARGO_MANIFEST_DIR")).join("../../docs/evidence/B022-verification.json");
    let mut root = if path.exists() {
        serde_json::from_str::<Value>(&fs::read_to_string(&path).unwrap())
            .unwrap_or_else(|_| json!({}))
    } else {
        json!({
            "generated_by": "crates/archmeld-core/tests/verification.rs",
            "platform": std::env::consts::OS,
            "acceptance": {},
        })
    };
    root["project_id"] = json!(fixture.project);
    root["snapshot_id"] = json!(fixture.snapshot_id);
    root["acceptance"][case_id] = case;
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(path, serde_json::to_string_pretty(&root).unwrap() + "\n").unwrap();
}

fn predicate_check(
    id: &str,
    kind: &str,
    required: bool,
    command: Option<Value>,
    checker_id: Option<&str>,
    predicate: Value,
    input_artifacts: Vec<String>,
) -> Value {
    let has_command = command.is_some();
    let mut check = json!({
        "id": id,
        "kind": kind,
        "required": required,
        "description": format!("{id} predicate coverage"),
        "command": command,
        "checker_id": checker_id,
        "predicate": predicate,
        "input_artifacts": input_artifacts,
    });
    if has_command {
        check["sandbox"] = json!({"mode": "workspace", "include": [], "exclude": []});
    }
    check
}

fn put_json_artifact(fixture: &mut common::Fixture, value: &Value) -> String {
    let bytes = serde_json::to_vec(value).unwrap();
    fixture.store.put_blob(&bytes, "application/json").unwrap()
}

fn predicate_detail(check: &Value) -> &Value {
    check
        .get("detail")
        .and_then(|detail| detail.get("predicate"))
        .unwrap_or_else(|| &check["detail"])
}

#[test]
fn numeric_invariant_and_performance_predicates_are_deterministic() {
    let mut fixture = common::setup("b022-predicates");
    let environment = hash::digest_bytes(b"b022-predicates-environment");
    let other_environment = hash::digest_bytes(b"b022-other-environment");

    let expected_digest = put_json_artifact(
        &mut fixture,
        &json!({"columns": ["x", "y"], "data": [[1, 2], [3, 4]]}),
    );
    let failing_digest = put_json_artifact(
        &mut fixture,
        &json!({"columns": ["x", "y"], "data": [[1.01, 2], [3, 4]]}),
    );
    let shape_digest = put_json_artifact(
        &mut fixture,
        &json!({"columns": ["x", "y"], "data": [[1, 2]]}),
    );
    let expected_units_digest = put_json_artifact(
        &mut fixture,
        &json!({"columns": ["x", "y"], "units": ["m", "s"], "data": [[1, 2], [3, 4]]}),
    );
    let actual_units_digest = put_json_artifact(
        &mut fixture,
        &json!({"columns": ["x", "y"], "units": ["cm", "s"], "data": [[1, 2], [3, 4]]}),
    );
    let nan_expected_digest = put_json_artifact(
        &mut fixture,
        &json!({"columns": ["x"], "data": [["NaN"], ["Infinity"]]}),
    );
    let nan_actual_digest = put_json_artifact(
        &mut fixture,
        &json!({"columns": ["x"], "data": [["NaN"], ["Infinity"]]}),
    );
    let invariant_digest = put_json_artifact(
        &mut fixture,
        &json!({
            "columns": ["x", "y"],
            "units": ["m", "s"],
            "data": [[0, 1], [1, 2], [2, 3]],
        }),
    );
    let measurements_digest = put_json_artifact(
        &mut fixture,
        &json!({
            "metric": "query.p95",
            "unit": "ms",
            "environment_digest": environment,
            "samples": [10, 11, 12, 13, 14],
        }),
    );
    let baseline_digest = put_json_artifact(
        &mut fixture,
        &json!({
            "metric": "query.p95",
            "unit": "ms",
            "environment_digest": environment,
            "samples": [10, 10, 10, 10, 10],
        }),
    );
    let wrong_environment_digest = put_json_artifact(
        &mut fixture,
        &json!({
            "metric": "query.p95",
            "unit": "ms",
            "environment_digest": other_environment,
            "samples": [10, 11, 12, 13, 14],
        }),
    );
    let short_digest = put_json_artifact(
        &mut fixture,
        &json!({
            "metric": "query.p95",
            "unit": "ms",
            "environment_digest": environment,
            "samples": [10, 11],
        }),
    );

    let contract_checks = vec![
        predicate_check(
            "check.numeric.command",
            "behavior",
            true,
            Some(json!({
                "argv": [
                    "python",
                    "-c",
                    "import json;open('actual-numbers.json','w').write(json.dumps({'columns':['x','y'],'data':[[1.0005,2.0005],[3.0005,4.0005]]}))",
                ],
                "cwd": null,
                "timeout_ms": 5000,
            })),
            None,
            json!({
                "kind": "numeric_tolerance",
                "expected_artifact": expected_digest.clone(),
                "actual_artifact": "actual-numbers.json",
                "columns": ["x", "y"],
                "atol": "0.001",
                "rtol": "0.0",
                "nonfinite_policy": "reject",
            }),
            vec![expected_digest.clone()],
        ),
        predicate_check(
            "check.numeric.nan",
            "domain",
            true,
            None,
            Some("numeric.tolerance.v1"),
            json!({
                "kind": "numeric_tolerance",
                "expected_artifact": nan_expected_digest.clone(),
                "actual_artifact": nan_actual_digest.clone(),
                "columns": ["x"],
                "atol": "0.0",
                "rtol": "0.0",
                "nonfinite_policy": "match",
            }),
            vec![nan_expected_digest.clone(), nan_actual_digest.clone()],
        ),
        predicate_check(
            "check.numeric.fail",
            "domain",
            false,
            None,
            Some("numeric.tolerance.v1"),
            json!({
                "kind": "numeric_tolerance",
                "expected_artifact": expected_digest.clone(),
                "actual_artifact": failing_digest.clone(),
                "columns": ["x", "y"],
                "atol": "0.001",
                "rtol": "0.0",
                "nonfinite_policy": "reject",
            }),
            vec![expected_digest.clone(), failing_digest.clone()],
        ),
        predicate_check(
            "check.numeric.shape",
            "domain",
            false,
            None,
            Some("numeric.tolerance.v1"),
            json!({
                "kind": "numeric_tolerance",
                "expected_artifact": expected_digest.clone(),
                "actual_artifact": shape_digest.clone(),
                "columns": ["x", "y"],
                "atol": "1.0",
                "rtol": "0.0",
                "nonfinite_policy": "reject",
            }),
            vec![expected_digest.clone(), shape_digest.clone()],
        ),
        predicate_check(
            "check.numeric.units",
            "domain",
            false,
            None,
            Some("numeric.tolerance.v1"),
            json!({
                "kind": "numeric_tolerance",
                "expected_artifact": expected_units_digest.clone(),
                "actual_artifact": actual_units_digest.clone(),
                "columns": ["x", "y"],
                "atol": "0.0",
                "rtol": "0.0",
                "nonfinite_policy": "reject",
            }),
            vec![expected_units_digest.clone(), actual_units_digest.clone()],
        ),
        predicate_check(
            "check.invariant.pass",
            "domain",
            true,
            None,
            Some("numeric.invariants.v1"),
            json!({
                "kind": "invariant",
                "checker_id": "numeric.invariants.v1",
                "version": "1",
                "configuration": {
                    "artifact": invariant_digest.clone(),
                    "columns": ["x", "y"],
                    "units": {"x": "m", "y": "s"},
                    "shape": [3, 2],
                    "rules": [
                        {"id": "finite", "kind": "finite"},
                        {"id": "range.x", "kind": "range", "column": "x", "min": "0", "max": "2"},
                        {"id": "mono.y", "kind": "monotonic", "column": "y", "direction": "nondecreasing"},
                        {"id": "sum.x", "kind": "sum", "column": "x", "expected": "3", "atol": "0.0"}
                    ]
                }
            }),
            vec![invariant_digest.clone()],
        ),
        predicate_check(
            "check.invariant.unknown",
            "domain",
            false,
            None,
            Some("numeric.invariants.v1"),
            json!({
                "kind": "invariant",
                "checker_id": "numeric.invariants.v1",
                "version": "1",
                "configuration": {
                    "artifact": invariant_digest.clone(),
                    "rules": [{"id": "periodic", "kind": "periodic", "period": "2"}]
                }
            }),
            vec![invariant_digest.clone()],
        ),
        predicate_check(
            "check.invariant.fail",
            "domain",
            false,
            None,
            Some("numeric.invariants.v1"),
            json!({
                "kind": "invariant",
                "checker_id": "numeric.invariants.v1",
                "version": "1",
                "configuration": {
                    "artifact": invariant_digest.clone(),
                    "rules": [{"id": "range.x", "kind": "range", "column": "x", "min": "1", "max": "5"}]
                }
            }),
            vec![invariant_digest.clone()],
        ),
        predicate_check(
            "check.performance.pass",
            "performance",
            true,
            None,
            Some("performance.budget.v1"),
            json!({
                "kind": "performance_budget",
                "measurements_artifact": measurements_digest.clone(),
                "metric": "query.p95",
                "statistic": "p95",
                "threshold": "20",
                "unit": "ms",
                "comparison": "max",
                "min_repetitions": 3,
                "environment_digest": environment.clone(),
            }),
            vec![measurements_digest.clone()],
        ),
        predicate_check(
            "check.performance.baseline",
            "performance",
            true,
            None,
            Some("performance.budget.v1"),
            json!({
                "kind": "performance_budget",
                "measurements_artifact": measurements_digest.clone(),
                "baseline_artifact": baseline_digest.clone(),
                "metric": "query.p95",
                "statistic": "p95",
                "threshold": "1.5",
                "unit": "ms",
                "comparison": "baseline_max_ratio",
                "min_repetitions": 3,
                "environment_digest": environment.clone(),
            }),
            vec![measurements_digest.clone(), baseline_digest.clone()],
        ),
        predicate_check(
            "check.performance.fail",
            "performance",
            false,
            None,
            Some("performance.budget.v1"),
            json!({
                "kind": "performance_budget",
                "measurements_artifact": measurements_digest.clone(),
                "metric": "query.p95",
                "statistic": "p95",
                "threshold": "12",
                "unit": "ms",
                "comparison": "max",
                "min_repetitions": 3,
                "environment_digest": environment.clone(),
            }),
            vec![measurements_digest.clone()],
        ),
        predicate_check(
            "check.performance.baseline_missing",
            "performance",
            false,
            None,
            Some("performance.budget.v1"),
            json!({
                "kind": "performance_budget",
                "measurements_artifact": measurements_digest.clone(),
                "metric": "query.p95",
                "statistic": "p95",
                "threshold": "1.5",
                "unit": "ms",
                "comparison": "baseline_max_ratio",
                "min_repetitions": 3,
                "environment_digest": environment.clone(),
            }),
            vec![measurements_digest.clone()],
        ),
        predicate_check(
            "check.performance.environment",
            "performance",
            false,
            None,
            Some("performance.budget.v1"),
            json!({
                "kind": "performance_budget",
                "measurements_artifact": wrong_environment_digest.clone(),
                "metric": "query.p95",
                "statistic": "p95",
                "threshold": "20",
                "unit": "ms",
                "comparison": "max",
                "min_repetitions": 3,
                "environment_digest": environment.clone(),
            }),
            vec![wrong_environment_digest.clone()],
        ),
        predicate_check(
            "check.performance.short",
            "performance",
            false,
            None,
            Some("performance.budget.v1"),
            json!({
                "kind": "performance_budget",
                "measurements_artifact": short_digest.clone(),
                "metric": "query.p95",
                "statistic": "p95",
                "threshold": "20",
                "unit": "ms",
                "comparison": "max",
                "min_repetitions": 5,
                "environment_digest": environment.clone(),
            }),
            vec![short_digest.clone()],
        ),
    ];
    let contract = contract_with_checks(&mut fixture, 2, contract_checks);
    let required = [
        "check.numeric.command",
        "check.numeric.nan",
        "check.invariant.pass",
        "check.performance.pass",
        "check.performance.baseline",
    ];
    plan_with_contract(
        &mut fixture,
        "plan.predicates",
        &contract,
        &["src/a.py"],
        &required,
    );
    let applied = apply_change(
        &mut fixture,
        "plan.predicates",
        "changeset.predicates",
        "src/a.py",
        "VALUE = 40\n",
    );
    let verification = verify(
        &mut fixture,
        "changeset.predicates",
        applied["revision"].as_i64().unwrap(),
        "verify-predicates",
        None,
    )
    .unwrap();
    assert_eq!(verification["state"], "accepted");
    assert_eq!(verification["verdict"]["acceptance"], true);
    let results = verification["checks"].as_array().unwrap();
    let by_id = |id: &str| {
        results
            .iter()
            .find(|check| check["check_id"] == id)
            .unwrap()
    };
    assert_eq!(by_id("check.numeric.command")["state"], "passed");
    assert_eq!(by_id("check.numeric.nan")["state"], "passed");
    assert_eq!(by_id("check.numeric.fail")["state"], "failed");
    assert_eq!(by_id("check.numeric.shape")["state"], "failed");
    assert_eq!(
        predicate_detail(by_id("check.numeric.shape"))["reason"],
        "shape mismatch"
    );
    assert_eq!(by_id("check.numeric.units")["state"], "failed");
    assert_eq!(
        predicate_detail(by_id("check.numeric.units"))["reason"],
        "unit mismatch (checked before numeric values)"
    );
    assert_eq!(by_id("check.invariant.pass")["state"], "passed");
    assert_eq!(by_id("check.invariant.unknown")["state"], "unsupported");
    assert_eq!(by_id("check.invariant.fail")["state"], "failed");
    assert_eq!(by_id("check.performance.pass")["state"], "passed");
    assert_eq!(by_id("check.performance.baseline")["state"], "passed");
    assert_eq!(by_id("check.performance.fail")["state"], "failed");
    assert_eq!(
        by_id("check.performance.baseline_missing")["state"],
        "inconclusive"
    );
    assert_eq!(
        by_id("check.performance.environment")["state"],
        "inconclusive"
    );
    assert_eq!(by_id("check.performance.short")["state"], "inconclusive");
    let baseline_detail = predicate_detail(by_id("check.performance.baseline"));
    assert_eq!(baseline_detail["comparison"], "baseline_max_ratio");
    assert_eq!(baseline_detail["coverage"]["satisfied"], true);
    assert_eq!(baseline_detail["statistics"]["count"], 5);
    assert_eq!(baseline_detail["baseline"]["statistics"]["count"], 5);
    let ratio: f64 = baseline_detail["ratio"].as_str().unwrap().parse().unwrap();
    assert!((ratio - 1.4).abs() < 1e-12, "{ratio}");
    let optional = verification["verdict"]["optional_not_passed"]
        .as_array()
        .unwrap();
    assert!(optional
        .iter()
        .any(|item| item["check_id"] == "check.numeric.fail"));
    assert!(optional
        .iter()
        .any(|item| item["check_id"] == "check.performance.baseline_missing"));
    let verifying =
        get_changeset(&fixture.store, &fixture.project, "changeset.predicates").unwrap();
    assert_eq!(verifying["state"], "verifying");
    let accepted = accept(
        &mut fixture,
        "changeset.predicates",
        verifying["revision"].as_i64().unwrap(),
        verification["verification_id"].as_str().unwrap(),
        "accept-predicates",
    )
    .unwrap();
    assert_eq!(accepted["state"], "accepted");

    write_predicate_evidence(
        &fixture,
        json!({
            "environment_digest": environment,
            "checks": results.iter().map(|check| json!({
                "check_id": check["check_id"],
                "state": check["state"],
                "predicate": predicate_detail(check),
            })).collect::<Vec<_>>(),
            "required_not_passed": verification["verdict"]["required_not_passed"],
            "optional_not_passed": optional,
            "acceptance": verification["verdict"]["acceptance"],
            "accepted_state": accepted["state"],
        }),
    );
}

fn write_predicate_evidence(fixture: &common::Fixture, case: Value) {
    if std::env::var("ARCHMELD_WRITE_EVIDENCE").ok().as_deref() != Some("1") {
        return;
    }
    let _guard = EVIDENCE_LOCK.lock().unwrap();
    let path =
        Path::new(env!("CARGO_MANIFEST_DIR")).join("../../docs/evidence/B022-predicates.json");
    let mut root = if path.exists() {
        serde_json::from_str::<Value>(&fs::read_to_string(&path).unwrap())
            .unwrap_or_else(|_| json!({}))
    } else {
        json!({
            "generated_by": "crates/archmeld-core/tests/verification.rs",
            "platform": std::env::consts::OS,
            "acceptance": {},
        })
    };
    root["project_id"] = json!(fixture.project);
    root["snapshot_id"] = json!(fixture.snapshot_id);
    root["acceptance"]["predicates"] = case;
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(path, serde_json::to_string_pretty(&root).unwrap() + "\n").unwrap();
}

fn contract_result(
    fixture: &mut common::Fixture,
    revision: i64,
    checks: Vec<Value>,
) -> Result<Value, archmeld_core::CoreError> {
    let mut contract = json!({
        "schema_version": "1.0",
        "id": fixture.contract_id,
        "project_id": fixture.project,
        "revision": revision,
        "kind": "structural_refactor",
        "invariants": ["behavior stays unchanged"],
        "allowed_changes": ["internal refactor"],
        "known_defects": [],
        "checks": checks,
        "digest": format!("sha256:{}", "0".repeat(64)),
        "frozen": false,
    });
    let mut preimage = contract.clone();
    preimage.as_object_mut().unwrap().remove("digest");
    preimage.as_object_mut().unwrap().remove("frozen");
    contract["digest"] = json!(hash::digest_value(&preimage).unwrap());
    propose_contract(
        &mut fixture.store,
        &json!({
            "project_id": fixture.project,
            "contract": contract,
            "expected_revision": 1,
            "idempotency_key": format!("sandbox-contract-{revision}"),
        }),
    )
}

fn sandboxed_check(id: &str, script: &str, include: Vec<&str>, cwd: Option<&str>) -> Value {
    json!({
        "id": id,
        "kind": "behavior",
        "required": true,
        "description": "command runs inside a declared snapshot sandbox",
        "command": {
            "argv": ["python", "-c", script],
            "cwd": cwd,
            "timeout_ms": 15000,
        },
        "checker_id": null,
        "predicate": {"kind": "exit_code", "expected_codes": [0]},
        "input_artifacts": [],
        "sandbox": {"mode": "workspace_copy", "include": include, "exclude": []},
    })
}

#[test]
fn sandboxed_check_reads_applied_snapshot_and_never_writes_the_workspace() {
    let mut fixture = common::setup("b022-sandbox-copy");
    let script = concat!(
        "import pathlib\n",
        "assert pathlib.Path('src/a.py').read_text().startswith('VALUE = 20')\n",
        "assert pathlib.Path('config/settings.toml').exists()\n",
        "pathlib.Path('scratch').mkdir()\n",
        "pathlib.Path('scratch/report.txt').write_text('ran')\n",
        "print('sandbox-ok')\n",
    );
    let contract = contract_with_checks(
        &mut fixture,
        2,
        vec![sandboxed_check(
            "check.sandbox.copy",
            script,
            vec!["src", "config"],
            None,
        )],
    );
    plan_with_contract(
        &mut fixture,
        "plan.b022.sandbox",
        &contract,
        &["src/a.py"],
        &["check.sandbox.copy"],
    );
    let applied = apply_change(
        &mut fixture,
        "plan.b022.sandbox",
        "changeset.b022.sandbox",
        "src/a.py",
        "VALUE = 20\n",
    );
    assert_eq!(applied["state"], "applied");
    let verification = verify(
        &mut fixture,
        "changeset.b022.sandbox",
        applied["revision"].as_i64().unwrap(),
        "verify-sandbox-copy",
        None,
    )
    .unwrap();
    assert_eq!(verification["state"], "accepted");
    assert_eq!(verification["verdict"]["acceptance"], true);
    let checks = verification["checks"].as_array().unwrap();
    let check = checks
        .iter()
        .find(|check| check["check_id"] == "check.sandbox.copy")
        .unwrap();
    assert_eq!(check["state"], "passed");
    let sandbox = &check["detail"]["sandbox"];
    assert_eq!(sandbox["mode"], "workspace_copy");
    assert_eq!(sandbox["declared"], true);
    assert_eq!(sandbox["isolated"], true);
    assert_eq!(sandbox["enforcement"], "materialized_snapshot_copy");
    assert_eq!(sandbox["source"], "resulting_snapshot");
    assert_eq!(sandbox["file_count"], 5);
    assert_eq!(sandbox["retained"], false);
    assert!(sandbox["manifest_digest"]
        .as_str()
        .unwrap()
        .starts_with("sha256:"));
    let sandbox_root = Path::new(sandbox["root"].as_str().unwrap());
    assert!(!sandbox_root.exists(), "a passing sandbox is removed");
    // The command wrote inside the copy, not the authorized workspace.
    assert!(!fixture.root.join("scratch/report.txt").exists());
    assert_eq!(
        fs::read_to_string(fixture.root.join("src/a.py")).unwrap(),
        "VALUE = 20\n"
    );
    let stdout_digest = check["stdout_digest"].as_str().unwrap();
    let stdout = fixture
        .store
        .read_blob_bytes(stdout_digest)
        .unwrap()
        .unwrap();
    assert!(String::from_utf8_lossy(&stdout).contains("sandbox-ok"));

    write_sandbox_evidence(
        &fixture,
        json!({
            "case": "workspace_copy",
            "state": verification["state"],
            "acceptance": verification["verdict"]["acceptance"],
            "sandbox": sandbox,
            "workspace_scratch_present": fixture.root.join("scratch/report.txt").exists(),
            "applied_file_content": fs::read_to_string(fixture.root.join("src/a.py")).unwrap(),
        }),
    );
}

#[test]
fn sandboxed_check_cannot_see_excluded_snapshot_files() {
    let mut fixture = common::setup("b022-sandbox-excluded");
    let script = concat!(
        "import pathlib\n",
        "assert pathlib.Path('src/a.py').exists()\n",
        "assert not pathlib.Path('config/settings.toml').exists()\n",
        "assert not pathlib.Path('src/b.py').exists()\n",
        "print('excluded-ok')\n",
    );
    let contract = contract_with_checks(
        &mut fixture,
        2,
        vec![sandboxed_check(
            "check.sandbox.excluded",
            script,
            vec!["src/a.py"],
            None,
        )],
    );
    plan_with_contract(
        &mut fixture,
        "plan.b022.sandbox.excluded",
        &contract,
        &["src/a.py"],
        &["check.sandbox.excluded"],
    );
    let applied = apply_change(
        &mut fixture,
        "plan.b022.sandbox.excluded",
        "changeset.b022.sandbox.excluded",
        "src/a.py",
        "VALUE = 3\n",
    );
    let verification = verify(
        &mut fixture,
        "changeset.b022.sandbox.excluded",
        applied["revision"].as_i64().unwrap(),
        "verify-sandbox-excluded",
        None,
    )
    .unwrap();
    assert_eq!(verification["state"], "accepted");
    let check = &verification["checks"][0];
    assert_eq!(check["state"], "passed");
    assert_eq!(check["detail"]["sandbox"]["file_count"], 1);
}

#[test]
fn sandboxed_check_cwd_must_stay_inside_the_materialized_copy() {
    let mut fixture = common::setup("b022-sandbox-escape");
    let script = "print('never runs')\n";
    let contract = contract_with_checks(
        &mut fixture,
        2,
        vec![sandboxed_check(
            "check.sandbox.escape",
            script,
            vec!["src"],
            Some(".."),
        )],
    );
    plan_with_contract(
        &mut fixture,
        "plan.b022.sandbox.escape",
        &contract,
        &["src/a.py"],
        &["check.sandbox.escape"],
    );
    let applied = apply_change(
        &mut fixture,
        "plan.b022.sandbox.escape",
        "changeset.b022.sandbox.escape",
        "src/a.py",
        "VALUE = 4\n",
    );
    let verification = verify(
        &mut fixture,
        "changeset.b022.sandbox.escape",
        applied["revision"].as_i64().unwrap(),
        "verify-sandbox-escape",
        None,
    )
    .unwrap();
    assert_eq!(verification["state"], "failed");
    let check = &verification["checks"][0];
    assert_eq!(check["state"], "error");
    assert_eq!(check["detail"]["sandbox"]["materialized"], false);
    let reason = check["detail"]["reason"].as_str().unwrap();
    assert!(
        reason.contains("'..'") || reason.contains("escapes the workspace"),
        "{reason}"
    );
}

#[test]
fn sandbox_declarations_are_validated_before_a_contract_is_frozen() {
    let mut fixture = common::setup("b022-sandbox-validation");
    let empty_include = json!({
        "id": "check.sandbox.invalid",
        "kind": "behavior",
        "required": true,
        "description": "empty include",
        "command": {"argv": ["python", "-c", "print('x')"], "cwd": null},
        "checker_id": null,
        "predicate": {"kind": "exit_code", "expected_codes": [0]},
        "input_artifacts": [],
        "sandbox": {"mode": "workspace_copy", "include": [], "exclude": []},
    });
    let error = contract_result(&mut fixture, 2, vec![empty_include]).unwrap_err();
    assert!(
        error.to_string().contains("include must not be empty"),
        "{error}"
    );

    let checker_sandbox = json!({
        "id": "check.sandbox.checker",
        "kind": "domain",
        "required": true,
        "description": "a checker has no command to confine",
        "command": null,
        "checker_id": "file.digest.v1",
        "predicate": {"kind": "file_digest", "path": "src/a.py", "digest": "sha256:00"},
        "input_artifacts": [],
        "sandbox": {"mode": "workspace_copy", "include": ["src"], "exclude": []},
    });
    let error = contract_result(&mut fixture, 3, vec![checker_sandbox]).unwrap_err();
    assert!(error.to_string().contains("has no command"), "{error}");

    let workspace_with_include = json!({
        "id": "check.sandbox.workspace",
        "kind": "behavior",
        "required": true,
        "description": "workspace mode has no allowlist",
        "command": {"argv": ["python", "-c", "print('x')"], "cwd": null},
        "checker_id": null,
        "predicate": {"kind": "exit_code", "expected_codes": [0]},
        "input_artifacts": [],
        "sandbox": {"mode": "workspace", "include": ["src"], "exclude": []},
    });
    let error = contract_result(&mut fixture, 4, vec![workspace_with_include]).unwrap_err();
    assert!(
        error.to_string().contains("declares no include/exclude"),
        "{error}"
    );

    let undeclared = json!({
        "id": "check.sandbox.undeclared",
        "kind": "behavior",
        "required": true,
        "description": "command checks must declare a sandbox",
        "command": {"argv": ["python", "-c", "print('x')"], "cwd": null},
        "checker_id": null,
        "predicate": {"kind": "exit_code", "expected_codes": [0]},
        "input_artifacts": [],
    });
    let error = contract_result(&mut fixture, 5, vec![undeclared]).unwrap_err();
    assert!(
        error.to_string().contains("no sandbox declaration"),
        "{error}"
    );

    let container_without_image = json!({
        "id": "check.sandbox.container",
        "kind": "behavior",
        "required": true,
        "description": "container without an image",
        "command": {"argv": ["true"], "cwd": null},
        "checker_id": null,
        "predicate": {"kind": "exit_code", "expected_codes": [0]},
        "input_artifacts": [],
        "sandbox": {"mode": "container", "include": ["src"], "exclude": []},
    });
    let error = contract_result(&mut fixture, 6, vec![container_without_image]).unwrap_err();
    assert!(error.to_string().contains("requires a pinned image"), "{error}");

    let container_with_tag = json!({
        "id": "check.sandbox.container.tag",
        "kind": "behavior",
        "required": true,
        "description": "container image is a mutable tag",
        "command": {"argv": ["true"], "cwd": null},
        "checker_id": null,
        "predicate": {"kind": "exit_code", "expected_codes": [0]},
        "input_artifacts": [],
        "sandbox": {"mode": "container", "include": ["src"], "exclude": [], "image": "debian:bookworm-slim"},
    });
    let error = contract_result(&mut fixture, 7, vec![container_with_tag]).unwrap_err();
    assert!(error.to_string().contains("pinned by digest"), "{error}");

    let container_with_network = json!({
        "id": "check.sandbox.container.network",
        "kind": "behavior",
        "required": true,
        "description": "container requests a network",
        "command": {"argv": ["true"], "cwd": null},
        "checker_id": null,
        "predicate": {"kind": "exit_code", "expected_codes": [0]},
        "input_artifacts": [],
        "sandbox": {"mode": "container", "include": ["src"], "exclude": [], "image": "sha256:0000000000000000000000000000000000000000000000000000000000000000", "network": "host"},
    });
    let error = contract_result(&mut fixture, 8, vec![container_with_network]).unwrap_err();
    assert!(error.to_string().contains("network must be none"), "{error}");
}

fn write_sandbox_evidence(fixture: &common::Fixture, case: Value) {
    if std::env::var("ARCHMELD_WRITE_EVIDENCE").ok().as_deref() != Some("1") {
        return;
    }
    let _guard = EVIDENCE_LOCK.lock().unwrap();
    let path = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../docs/release-evidence/B022-sandbox.json");
    let mut root = if path.exists() {
        serde_json::from_str::<Value>(&fs::read_to_string(&path).unwrap())
            .unwrap_or_else(|_| json!({}))
    } else {
        json!({
            "schema_version": "1.0",
            "generated_by": "crates/archmeld-core/tests/verification.rs",
            "platform": std::env::consts::OS,
            "cases": [],
        })
    };
    root["project_id"] = json!(fixture.project);
    root["snapshot_id"] = json!(fixture.snapshot_id);
    let cases = root["cases"].as_array_mut().unwrap();
    let case_name = case["case"].as_str().unwrap_or("").to_string();
    if let Some(existing) = cases.iter_mut().find(|item| item["case"] == case_name) {
        *existing = case;
    } else {
        cases.push(case);
    }
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(path, serde_json::to_string_pretty(&root).unwrap() + "\n").unwrap();
}

#[test]
fn container_sandbox_isolates_filesystem_and_network() {
    let Some(image) = std::env::var_os("ARCHMELD_CONTAINER_IMAGE") else {
        return;
    };
    let image = image.to_string_lossy().to_string();
    let mut fixture = common::setup("b022-container");
    let script = concat!(
        "set -e\n",
        "test -f src/a.py\n",
        "test ! -e config/settings.toml\n",
        "test ! -e src/b.py\n",
        "echo ok > /workspace/container-report.txt\n",
        "echo tmp > /tmp/t\n",
        "if echo x > /root/x 2>/dev/null; then exit 7; fi\n",
        "if grep -q eth0 /proc/net/dev; then exit 8; fi\n",
        "echo container-ok\n",
    );
    let check = json!({
        "id": "check.sandbox.container.run",
        "kind": "behavior",
        "required": true,
        "description": "command runs in a pinned container over the snapshot copy",
        "command": {"argv": ["sh", "-c", script], "cwd": null, "timeout_ms": 60000},
        "checker_id": null,
        "predicate": {"kind": "exit_code", "expected_codes": [0]},
        "input_artifacts": [],
        "sandbox": {
            "mode": "container",
            "include": ["src/a.py"],
            "exclude": [],
            "image": image,
            "network": "none",
        },
    });
    let contract = contract_with_checks(&mut fixture, 2, vec![check]);
    plan_with_contract(
        &mut fixture,
        "plan.b022.container",
        &contract,
        &["src/a.py"],
        &["check.sandbox.container.run"],
    );
    let applied = apply_change(
        &mut fixture,
        "plan.b022.container",
        "changeset.b022.container",
        "src/a.py",
        "VALUE = 20\n",
    );
    let verification = verify(
        &mut fixture,
        "changeset.b022.container",
        applied["revision"].as_i64().unwrap(),
        "verify-container",
        None,
    )
    .unwrap();
    assert_eq!(verification["state"], "accepted", "{verification}");
    let check = &verification["checks"][0];
    assert_eq!(check["state"], "passed");
    let sandbox = &check["detail"]["sandbox"];
    assert_eq!(sandbox["mode"], "container");
    assert_eq!(sandbox["isolated"], true);
    assert_eq!(sandbox["enforcement"], "oci_container");
    assert_eq!(sandbox["network"], "none");
    assert_eq!(sandbox["read_only"], true);
    assert_eq!(sandbox["file_count"], 1);
    assert_eq!(sandbox["retained"], false);
    assert!(sandbox["image_id"]
        .as_str()
        .unwrap()
        .starts_with("sha256:"));
    assert!(sandbox["platform"]
        .as_str()
        .unwrap()
        .starts_with("linux/"));
    assert_eq!(sandbox["container"]["removed"], true);
    assert!(!Path::new(sandbox["root"].as_str().unwrap()).exists());
    assert!(!fixture.root.join("container-report.txt").exists());
    assert_eq!(
        fs::read_to_string(fixture.root.join("src/a.py")).unwrap(),
        "VALUE = 20\n"
    );
    let stdout = fixture
        .store
        .read_blob_bytes(check["stdout_digest"].as_str().unwrap())
        .unwrap()
        .unwrap();
    assert!(String::from_utf8_lossy(&stdout).contains("container-ok"));

    if std::env::var("ARCHMELD_WRITE_EVIDENCE").ok().as_deref() == Some("1") {
        let path = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../docs/release-evidence/B022-container-sandbox.json");
        let evidence = json!({
            "schema_version": "1.0",
            "generated_by": "crates/archmeld-core/tests/verification.rs",
            "platform": std::env::consts::OS,
            "image": {
                "reference": sandbox["image"],
                "image_id": sandbox["image_id"],
                "repo_digests": sandbox["repo_digests"],
                "platform": sandbox["platform"],
            },
            "container": sandbox["container"],
            "isolation": {
                "network": sandbox["network"],
                "read_only": sandbox["read_only"],
                "tmpfs": sandbox["tmpfs"],
                "workdir": sandbox["workdir"],
                "bind_mount": "/workspace is the materialized snapshot copy",
            },
            "check": {
                "state": check["state"],
                "exit_code": check["exit_code"],
                "stdout_contains": "container-ok",
                "workspace_report_present": fixture.root.join("container-report.txt").exists(),
                "workspace_file_content": fs::read_to_string(fixture.root.join("src/a.py")).unwrap(),
                "sandbox_retained": sandbox["retained"],
            },
            "limitations": sandbox["limitations"],
        });
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, serde_json::to_string_pretty(&evidence).unwrap() + "\n").unwrap();
    }
}
