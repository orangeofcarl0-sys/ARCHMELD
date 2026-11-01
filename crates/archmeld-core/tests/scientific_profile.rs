// Copyright 2026 ARCHMELD contributors
// SPDX-License-Identifier: Apache-2.0

//! B029 / AT064–AT065: a real first-order estimator is refactored through the
//! apply/verify gate. Units, shape, update order, full dynamic trace and phase
//! boundaries are checked against frozen golden truth; the option comparison
//! keeps "do not refactor" legal and refuses unproven abstraction benefits.

use archmeld_core::apply::apply_changeset;
use archmeld_core::changeset::{
    approve_prepared_change, create_changeset, get_changeset, prepare_changeset,
};
use archmeld_core::hash;
use archmeld_core::interpret::propose_architecture;
use archmeld_core::manager::open_project_for_handle;
use archmeld_core::plan::{approve_plan, propose_contract, propose_plan};
use archmeld_core::scenario::run_command_capture;
use archmeld_core::snapshot::{capture_snapshot, SnapshotInputs, SnapshotOptions};
use archmeld_core::store::Store;
use archmeld_core::verification::{accept_changeset, verify_changeset};
use serde_json::{json, Value};
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::Duration;

static EVIDENCE_LOCK: Mutex<()> = Mutex::new(());

const REFACTORED: &str = r#"# Copyright 2026 ARCHMELD contributors
# SPDX-License-Identifier: Apache-2.0
"""IIR estimator with one pure state-update helper.

units: input V, output V, alpha dimensionless, sample rate Hz, phase rad
shape: one-dimensional float64 arrays
state: previous output in V, owned by run_filter
update order: y[n] = alpha*x[n] + (1-alpha)*y[n-1]; state advances only after
the output is appended. The helper isolates the update without adding classes.
"""

import math


def update(previous, sample, alpha):
    return alpha * sample + (1.0 - alpha) * previous


def run_filter(samples, alpha, initial=0.0):
    output = []
    previous = initial
    for sample in samples:
        current = update(previous, sample, alpha)
        output.append(current)
        previous = current
    return output


def wrap_phase(angle):
    wrapped = angle
    while wrapped >= math.pi:
        wrapped -= 2.0 * math.pi
    while wrapped < -math.pi:
        wrapped += 2.0 * math.pi
    return wrapped
"#;

fn copy_dir(source: &Path, destination: &Path) {
    fs::create_dir_all(destination).unwrap();
    for entry in fs::read_dir(source).unwrap() {
        let entry = entry.unwrap();
        let target = destination.join(entry.file_name());
        if entry.file_type().unwrap().is_dir() {
            copy_dir(&entry.path(), &target);
        } else {
            fs::copy(entry.path(), target).unwrap();
        }
    }
}

struct ScientificFixture {
    _dir: tempfile::TempDir,
    root: PathBuf,
    project: String,
    handle: String,
    snapshot_id: String,
    config_digest: String,
    build_config: Value,
    environment: Value,
    toolchain_digest: String,
}

fn setup() -> (ScientificFixture, Store) {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().join("repo");
    copy_dir(
        &Path::new(env!("CARGO_MANIFEST_DIR")).join("../../adapters/scientific/fixtures/filter"),
        &root,
    );
    let state = dir.path().join("state");
    let mut store = Store::open(&state).unwrap();
    let handle = "ws.b029".to_string();
    let project = open_project_for_handle(
        &mut store,
        &handle,
        &root,
        "expedition",
        &state.join("sessions"),
        json!({"profile": "scientific", "fixture": "iir"}),
    )
    .unwrap();
    let project_id = project["id"].as_str().unwrap().to_string();
    let build_config = json!({"kind": "python", "variant": "scientific"});
    let environment = json!({"os": "windows", "dtype": "float64"});
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
    let config_digest = snapshot.snapshot["config_digest"]
        .as_str()
        .unwrap()
        .to_string();
    let toolchain_digest = store
        .put_blob(
            br#"{"toolchain":"python-3.11","profile":"scientific.numeric.v1"}"#,
            "application/json",
        )
        .unwrap();
    (
        ScientificFixture {
            _dir: dir,
            root,
            project: project_id,
            handle,
            snapshot_id: snapshot.snapshot_id,
            config_digest,
            build_config,
            environment,
            toolchain_digest,
        },
        store,
    )
}

fn artifact(columns: &[&str], units: &[&str], values: &[f64]) -> Value {
    json!({
        "columns": columns,
        "units": units,
        "data": values.iter().map(|value| json!([value])).collect::<Vec<_>>(),
    })
}

fn golden(fixture: &ScientificFixture) -> Value {
    serde_json::from_slice(&fs::read(fixture.root.join("golden.json")).unwrap()).unwrap()
}

fn expected_artifacts(fixture: &ScientificFixture) -> (Value, Value) {
    let golden = golden(fixture);
    let output: Vec<f64> = golden["expected_output"]
        .as_array()
        .unwrap()
        .iter()
        .map(|value| value.as_f64().unwrap())
        .collect();
    let phase: Vec<f64> = golden["expected_phase"]
        .as_array()
        .unwrap()
        .iter()
        .map(|value| value.as_f64().unwrap())
        .collect();
    (
        artifact(&["output"], &["V"], &output),
        artifact(&["phase"], &["rad"], &phase),
    )
}

fn contract_and_plan(
    fixture: &ScientificFixture,
    store: &mut Store,
    expected_output: &str,
    expected_phase: &str,
    tolerance: (&str, &str),
) -> String {
    let architecture = propose_architecture(
        store,
        &json!({
            "project_id": fixture.project,
            "model": {
                "schema_version": "1.0",
                "id": "arch.b029",
                "project_id": fixture.project,
                "snapshot_id": fixture.snapshot_id,
                "revision": 1,
                "layer": "as_is",
                "status": "candidate",
                "components": [{
                    "id": "component.filter",
                    "name": "Filter",
                    "purpose": "First-order estimator with explicit state ownership.",
                    "responsibilities": ["update scalar state in documented order"],
                    "non_responsibilities": ["change filter mathematics"],
                    "member_entity_ids": [],
                    "owned_state": ["previous output"],
                    "owned_resources": [],
                    "runtime_placement": [],
                    "evidence_ids": [],
                    "open_questions": [],
                }],
                "dependencies": [],
                "rules": [],
                "claim_ids": [],
            },
            "expected_revision": 1,
            "idempotency_key": "b029-arch",
        }),
    )
    .unwrap();
    let checks = vec![
        json!({
            "id": "check.harness",
            "kind": "behavior",
            "required": true,
            "description": "golden harness runs units/shape/boundary/determinism checks",
            "command": {"argv": ["python", "harness.py"], "cwd": null, "timeout_ms": 10000},
            "checker_id": null,
            "sandbox": {"mode": "workspace", "include": [], "exclude": []},
            "predicate": {
                "kind": "test_report",
                "format": "json",
                "report_path": "artifacts/report.json",
                "min_tests": 8,
                "max_failures": 0,
            },
            "input_artifacts": [],
        }),
        json!({
            "id": "check.output",
            "kind": "domain",
            "required": true,
            "description": "full dynamic output trace matches golden truth",
            "command": null,
            "checker_id": "numeric.tolerance.v1",
            "predicate": {
                "kind": "numeric_tolerance",
                "expected_artifact": expected_output,
                "actual_artifact": "artifacts/filter-output.json",
                "columns": ["output"],
                "atol": tolerance.0,
                "rtol": tolerance.1,
                "nonfinite_policy": "reject",
                "unit_mode": "dimension",
            },
            "input_artifacts": [expected_output],
        }),
        json!({
            "id": "check.output.ulp",
            "kind": "domain",
            "required": true,
            "description": "every output value is bit-identical to golden truth",
            "command": null,
            "checker_id": "numeric.tolerance.v1",
            "predicate": {
                "kind": "numeric_tolerance",
                "expected_artifact": expected_output,
                "actual_artifact": "artifacts/filter-output.json",
                "columns": ["output"],
                "mode": "ulp",
                "max_ulps": "0",
                "nonfinite_policy": "reject",
                "unit_mode": "dimension",
            },
            "input_artifacts": [expected_output],
        }),
        json!({
            "id": "check.output.statistical",
            "kind": "domain",
            "required": true,
            "description": "zero error distribution across the full output trace",
            "command": null,
            "checker_id": "numeric.tolerance.v1",
            "predicate": {
                "kind": "numeric_tolerance",
                "expected_artifact": expected_output,
                "actual_artifact": "artifacts/filter-output.json",
                "columns": ["output"],
                "mode": "statistical",
                "statistics": {
                    "mean_abs_error_max": "0",
                    "rms_error_max": "0",
                    "max_abs_error_max": "0",
                    "p95_abs_error_max": "0",
                },
                "nonfinite_policy": "reject",
                "unit_mode": "dimension",
            },
            "input_artifacts": [expected_output],
        }),
        json!({
            "id": "check.phase",
            "kind": "domain",
            "required": true,
            "description": "phase wrap including +pi/-pi boundaries matches",
            "command": null,
            "checker_id": "numeric.tolerance.v1",
            "predicate": {
                "kind": "numeric_tolerance",
                "expected_artifact": expected_phase,
                "actual_artifact": "artifacts/phase.json",
                "columns": ["phase"],
                "atol": tolerance.0,
                "rtol": tolerance.1,
                "nonfinite_policy": "reject",
                "unit_mode": "dimension",
            },
            "input_artifacts": [expected_phase],
        }),
        json!({
            "id": "check.invariant",
            "kind": "domain",
            "required": true,
            "description": "output units, shape and finite range",
            "command": null,
            "checker_id": "numeric.invariants.v1",
            "predicate": {
                "kind": "invariant",
                "checker_id": "numeric.invariants.v1",
                "version": "1",
                "configuration": {
                    "artifact": "artifacts/filter-output.json",
                    "columns": ["output"],
                    "units": {"output": "V"},
                    "shape": [8, 1],
                    "rules": [
                        {"id": "finite", "kind": "finite"},
                        {"id": "range", "kind": "range", "column": "output", "min": "0", "max": "1"}
                    ]
                }
            },
            "input_artifacts": [],
        }),
    ];
    let mut contract = json!({
        "schema_version": "1.0",
        "id": "contract.b029",
        "project_id": fixture.project,
        "revision": 1,
        "kind": "structural_refactor",
        "invariants": ["outputs preserve units, shape and update order"],
        "allowed_changes": ["extract a pure state-update helper"],
        "known_defects": [],
        "checks": checks,
        "digest": format!("sha256:{}", "0".repeat(64)),
        "frozen": false,
    });
    let mut preimage = contract.clone();
    preimage.as_object_mut().unwrap().remove("digest");
    preimage.as_object_mut().unwrap().remove("frozen");
    contract["digest"] = json!(hash::digest_value(&preimage).unwrap());
    let contract = propose_contract(
        store,
        &json!({
            "project_id": fixture.project,
            "contract": contract,
            "expected_revision": 1,
            "idempotency_key": "b029-contract",
        }),
    )
    .unwrap();
    let plan_id = "plan.b029";
    let plan = propose_plan(
        store,
        &json!({
            "project_id": fixture.project,
            "plan": {
                "schema_version": "1.0",
                "id": plan_id,
                "project_id": fixture.project,
                "revision": 1,
                "kind": "structural_refactor",
                "objective": "extract the scalar update without changing mathematics",
                "non_goals": ["no filter redesign", "no class hierarchy"],
                "base_snapshot_id": fixture.snapshot_id,
                "target_architecture_id": architecture["id"],
                "contract_id": contract["id"],
                "contract_digest": contract["digest"],
                "steps": [{
                    "id": "step.1",
                    "title": "pure update helper",
                    "depends_on": [],
                    "allowed_paths": ["estimator.py"],
                    "required_check_ids": [
                        "check.harness",
                        "check.output",
                        "check.phase",
                        "check.invariant"
                    ],
                    "completion_criteria": ["numeric verification accepted"],
                }],
                "risks": ["floating-point expression order must be preserved"],
                "evidence_ids": [],
                "status": "draft",
            },
            "expected_revision": 1,
            "idempotency_key": "b029-plan",
        }),
    )
    .unwrap();
    approve_plan(
        store,
        &fixture.project,
        plan_id,
        plan["revision"].as_i64().unwrap(),
        contract["digest"].as_str().unwrap(),
        "operator",
        "b029-plan-approve",
    )
    .unwrap();
    plan_id.to_string()
}

fn run_python(root: &Path, script: &str) -> Value {
    let capture = run_command_capture(
        &["python".to_string(), script.to_string()],
        root,
        &[],
        Duration::from_secs(60),
        None,
    )
    .unwrap();
    assert_eq!(
        capture.exit_code,
        Some(0),
        "{}",
        String::from_utf8_lossy(&capture.stderr)
    );
    serde_json::from_slice(&capture.stdout).unwrap()
}

#[test]
fn scientific_refactor_preserves_units_shape_dynamics_and_phase_boundaries() {
    let (fixture, mut store) = setup();
    let golden = golden(&fixture);
    let tolerance = (
        golden["tolerance"]["atol"].as_str().unwrap().to_string(),
        golden["tolerance"]["rtol"].as_str().unwrap().to_string(),
    );
    let (expected_output, expected_phase) = expected_artifacts(&fixture);
    let expected_output_digest = store
        .put_blob(
            &serde_json::to_vec(&expected_output).unwrap(),
            "application/json",
        )
        .unwrap();
    let expected_phase_digest = store
        .put_blob(
            &serde_json::to_vec(&expected_phase).unwrap(),
            "application/json",
        )
        .unwrap();

    // Baseline: the same golden harness runs against the pre-refactor code in a
    // separate copy, so generated artifacts do not enter the snapshot.
    let baseline_dir = tempfile::tempdir().unwrap();
    copy_dir(
        &Path::new(env!("CARGO_MANIFEST_DIR")).join("../../adapters/scientific/fixtures/filter"),
        &baseline_dir.path().join("repo"),
    );
    let baseline = run_python(&baseline_dir.path().join("repo"), "harness.py");
    let baseline_output: Vec<f64> = baseline["output"]
        .as_array()
        .unwrap()
        .iter()
        .map(|value| value.as_f64().unwrap())
        .collect();
    let expected_output_values: Vec<f64> = golden["expected_output"]
        .as_array()
        .unwrap()
        .iter()
        .map(|value| value.as_f64().unwrap())
        .collect();
    let baseline_max_error = baseline_output
        .iter()
        .zip(expected_output_values.iter())
        .map(|(left, right)| (left - right).abs())
        .fold(0.0_f64, f64::max);
    assert!(baseline_max_error <= 1e-12, "{baseline_max_error}");
    let baseline_phase = baseline["phase"].as_array().unwrap();
    assert_eq!(baseline_phase.len(), 6);
    assert!((baseline_phase[4].as_f64().unwrap() + std::f64::consts::PI).abs() < 1e-12);

    let plan_id = contract_and_plan(
        &fixture,
        &mut store,
        &expected_output_digest,
        &expected_phase_digest,
        (&tolerance.0, &tolerance.1),
    );
    let changeset_id = "changeset.b029";
    let changeset = create_changeset(
        &mut store,
        &json!({
            "schema_version": "1.0",
            "project_id": fixture.project,
            "changeset_id": changeset_id,
            "plan_id": plan_id,
            "plan_revision": 2,
            "step_id": "step.1",
            "read_set": [],
            "build_config": fixture.build_config,
            "environment": fixture.environment,
            "snapshot_options": {
                "include_untracked": true,
                "max_file_bytes": 1048576,
                "excluded_dirs": [".git", "target", "node_modules", "__pycache__"],
                "max_retries": 2
            },
            "expected_revision": 1,
            "idempotency_key": "b029-create",
        }),
    )
    .unwrap();
    let editor_identity = store
        .put_blob(br#"{"name":"diff-only-editor","version":"1.0.0"}"#, "application/json")
        .unwrap();
    let seeded = prepare_changeset(
        &mut store,
        &json!({
            "project_id": fixture.project,
            "changeset_id": changeset_id,
            "expected_revision": changeset["revision"],
            "toolchain_digest": fixture.toolchain_digest,
            "transform": null,
            "idempotency_key": "b029-prepare-seed",
        }),
    )
    .unwrap();
    let staging = PathBuf::from(seeded["staging"]["path"].as_str().unwrap());
    fs::write(staging.join("estimator.py"), REFACTORED).unwrap();
    let prepared = prepare_changeset(
        &mut store,
        &json!({
            "project_id": fixture.project,
            "changeset_id": changeset_id,
            "expected_revision": seeded["revision"],
            "toolchain_digest": fixture.toolchain_digest,
            "transform": {
                "method": "manual_edit",
                "tool": {
                    "name": "diff-only-editor",
                    "version": "1.0.0",
                    "identity_digest": editor_identity
                },
                "declared_moves": [],
                "patch_artifact": null,
                "prompt_artifact": null,
                "workspace_edit_operations": 0,
                "notes": ["extract pure update helper; no class hierarchy"]
            },
            "idempotency_key": "b029-prepare-transform",
        }),
    )
    .unwrap();
    let approval = approve_prepared_change(
        &mut store,
        &json!({
            "project_id": fixture.project,
            "changeset_id": changeset_id,
            "expected_revision": prepared["revision"],
            "prepared_digest": prepared["prepared_digest"],
            "actor": "operator",
            "idempotency_key": "b029-approve",
        }),
    )
    .unwrap();
    let applied = apply_changeset(
        &mut store,
        &json!({
            "project_id": fixture.project,
            "changeset_id": changeset_id,
            "expected_revision": prepared["revision"],
            "prepared_digest": prepared["prepared_digest"],
            "approval_id": approval["approval"]["approval_id"],
            "actor": "operator",
            "workspace_handle": fixture.handle,
            "idempotency_key": "b029-apply",
        }),
    )
    .unwrap();
    assert_eq!(applied["state"], "applied");

    let verification = verify_changeset(
        &mut store,
        &json!({
            "project_id": fixture.project,
            "changeset_id": changeset_id,
            "expected_revision": applied["revision"],
            "workspace_handle": fixture.handle,
            "execution_grant": {
                "grant_id": "grant.b029",
                "reason": "scientific numeric acceptance"
            },
            "environment": {"profile": "scientific.numeric.v1"},
            "snapshot_id": null,
            "idempotency_key": "b029-verify",
        }),
    )
    .unwrap();
    assert_eq!(verification["state"], "accepted", "{verification}");
    assert_eq!(verification["verdict"]["acceptance"], true);
    let checks = verification["checks"].as_array().unwrap();
    let by_id = |id: &str| checks.iter().find(|check| check["check_id"] == id).unwrap();
    assert_eq!(by_id("check.harness")["state"], "passed");
    assert_eq!(by_id("check.output")["state"], "passed");
    assert_eq!(by_id("check.phase")["state"], "passed");
    assert_eq!(by_id("check.invariant")["state"], "passed");
    let output_detail = &by_id("check.output")["detail"];
    let output_detail = if output_detail.get("predicate").is_some() {
        &output_detail["predicate"]
    } else {
        output_detail
    };
    let phase_detail = &by_id("check.phase")["detail"];
    let phase_detail = if phase_detail.get("predicate").is_some() {
        &phase_detail["predicate"]
    } else {
        phase_detail
    };
    assert_eq!(output_detail["mismatch_count"], 0);
    assert_eq!(output_detail["shape"], json!([8, 1]));
    assert_eq!(phase_detail["mismatch_count"], 0);
    assert_eq!(phase_detail["shape"], json!([6, 1]));

    let verifying = get_changeset(&store, &fixture.project, changeset_id).unwrap();
    let accepted = accept_changeset(
        &mut store,
        &json!({
            "project_id": fixture.project,
            "changeset_id": changeset_id,
            "expected_revision": verifying["revision"],
            "verification_id": verification["verification_id"],
            "workspace_handle": fixture.handle,
            "actor": "operator",
            "idempotency_key": "b029-accept",
        }),
    )
    .unwrap();
    assert_eq!(accepted["state"], "accepted");

    // AT065: measure the minimal extraction and record why class abstraction is
    // not justified. The no-refactor option remains legitimate.
    let measure_dir = tempfile::tempdir().unwrap();
    fs::copy(
        Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../adapters/scientific/fixtures/filter/baseline_estimator.py"),
        measure_dir.path().join("baseline_estimator.py"),
    )
    .unwrap();
    fs::copy(
        fixture.root.join("estimator.py"),
        measure_dir.path().join("estimator.py"),
    )
    .unwrap();
    fs::write(
        measure_dir.path().join("measure.py"),
        r#"import importlib.util, json, statistics, time

def load(name):
    spec = importlib.util.spec_from_file_location(name, name + ".py")
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return module

samples = [0.0, 1.0] * 50000
results = {}
outputs = {}
for name in ("baseline_estimator", "estimator"):
    module = load(name)
    times = []
    output = None
    for _ in range(5):
        started = time.perf_counter()
        output = module.run_filter(samples, 0.5)
        times.append((time.perf_counter() - started) * 1000.0)
    results[name] = {"median_ms": round(statistics.median(times), 3), "samples_ms": [round(value, 3) for value in times]}
    outputs[name] = output
print(json.dumps({"baseline": results["baseline_estimator"], "refactored": results["estimator"], "outputs_equal": outputs["baseline_estimator"] == outputs["estimator"], "samples": len(samples)}))
"#,
    )
    .unwrap();
    let measurements = run_python(measure_dir.path(), "measure.py");
    assert_eq!(measurements["outputs_equal"], true);

    let fixture_root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../adapters/scientific");
    let golden_digest = hash::digest_bytes(&fs::read(fixture.root.join("golden.json")).unwrap());
    let profile_digest = hash::digest_bytes(
        &fs::read(fixture_root.join("profiles/scientific.json")).unwrap(),
    );
    write_b029_evidence(json!({
        "AT064": {
            "status": "passed",
            "contract_digest": verification["contract_digest"],
            "snapshot_id": verification["snapshot_id"],
            "config_digest": fixture.config_digest,
            "tolerance": tolerance,
            "units_checked_before_values": true,
            "shape_checked_before_values": true,
            "full_trace_compared": output_detail["compared"],
            "dynamic_checks": golden["dynamic_checks"],
            "phase_boundaries": golden["expected_phase"],
            "output_check": output_detail,
            "phase_check": phase_detail,
            "check_states": checks.iter().map(|check| json!({
                "check_id": check["check_id"],
                "state": check["state"],
            })).collect::<Vec<_>>(),
            "baseline_max_abs_error": baseline_max_error,
            "baseline_phase": baseline_phase,
            "accepted_state": accepted["state"],
            "golden_contract_digest": golden_digest,
            "profile_digest": profile_digest,
            "source": "deterministic synthetic truth plus measured baseline; no user data was available",
        },
        "AT065": {
            "status": "passed",
            "options": [
                {
                    "option": "no_refactor",
                    "legal": true,
                    "behavior_risk": "none",
                    "abstraction_benefit_claimed": false,
                    "reason": "math is already clear; no measured maintenance benefit"
                },
                {
                    "option": "minimal_pure_step",
                    "chosen": true,
                    "classes_added": 0,
                    "state_owner": "run_filter",
                    "update_order_preserved": true,
                    "measurements": measurements
                },
                {
                    "option": "class_abstraction",
                    "chosen": false,
                    "implemented": false,
                    "abstraction_benefit_claimed": false,
                    "reason": "no evidence that a class/lifecycle improves this scalar update; adds state ambiguity and review surface"
                }
            ],
            "no_refactor_remains_legitimate": true,
            "unproven_abstraction_counted_as_complete": false,
            "adr": "docs/adr/ADR-034-scientific-refactor-boundary.md",
        }
    }));
}

fn write_b029_evidence(value: Value) {
    if std::env::var("ARCHMELD_WRITE_EVIDENCE").ok().as_deref() != Some("1") {
        return;
    }
    let _guard = EVIDENCE_LOCK.lock().unwrap();
    let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../docs/evidence/B029-scientific.json");
    let mut root: Value = if path.exists() {
        serde_json::from_str(&fs::read_to_string(&path).unwrap()).unwrap_or_else(|_| json!({}))
    } else {
        json!({
            "generated_by": "crates/archmeld-core/tests/scientific_profile.rs",
            "platform": std::env::consts::OS,
            "acceptance": {},
        })
    };
    root["acceptance"] = value;
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(path, serde_json::to_string_pretty(&root).unwrap() + "\n").unwrap();
}
