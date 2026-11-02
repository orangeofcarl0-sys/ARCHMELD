// Copyright 2026 ARCHMELD contributors
// SPDX-License-Identifier: Apache-2.0

//! B030 / AT066: C++ simulated capture → Python estimator → static UI, with a
//! real trace mapped to source entities, a frozen numeric contract, a staged
//! refactor, independent verification and an offline review bundle.

use archmeld_core::apply::apply_changeset;
use archmeld_core::changeset::{
    approve_prepared_change, create_changeset, get_changeset, prepare_changeset,
};
use archmeld_core::hash;
use archmeld_core::interpret::propose_architecture;
use archmeld_core::manager::open_project_for_handle;
use archmeld_core::plan::{approve_plan, propose_contract, propose_plan};
use archmeld_core::report::{export_report, verify_bundle};
use archmeld_core::runtime::{import_trace, RuntimeImport};
use archmeld_core::scenario::run_command_capture;
use archmeld_core::snapshot::{capture_snapshot, SnapshotInputs, SnapshotOptions};
use archmeld_core::store::Store;
use archmeld_core::verification::{accept_changeset, verify_changeset};
use serde_json::{json, Value};
use std::fs;
use std::path::{Path, PathBuf};
use std::time::Duration;

fn copy_dir(source: &Path, destination: &Path) {
    fs::create_dir_all(destination).unwrap();
    for entry in fs::read_dir(source).unwrap() {
        let entry = entry.unwrap();
        let name = entry.file_name().to_string_lossy().to_string();
        if matches!(name.as_str(), "__pycache__" | "artifacts" | ".git") {
            continue;
        }
        let target = destination.join(entry.file_name());
        if entry.file_type().unwrap().is_dir() {
            copy_dir(&entry.path(), &target);
        } else {
            fs::copy(entry.path(), target).unwrap();
        }
    }
}

fn run(cwd: &Path, argv: Vec<String>, env: &[(String, String)]) -> Value {
    let capture = run_command_capture(&argv, cwd, env, Duration::from_secs(120), None)
        .unwrap_or_else(|error| panic!("{argv:?}: {error}"));
    json!({
        "argv": argv,
        "exit_code": capture.exit_code,
        "wall_ms": capture.wall_ms,
        "stdout": String::from_utf8_lossy(&capture.stdout).chars().take(4096).collect::<String>(),
        "stdout_digest": hash::digest_bytes(&capture.stdout),
        "stderr": String::from_utf8_lossy(&capture.stderr).chars().take(4096).collect::<String>(),
        "stderr_digest": hash::digest_bytes(&capture.stderr),
        "process_audit": capture.process_audit,
    })
}

const REFACTORED_ESTIMATOR: &str = r#"#!/usr/bin/env python3
# Copyright 2026 ARCHMELD contributors
# SPDX-License-Identifier: Apache-2.0
"""First-order estimator consuming the C++ capture CSV.

units: input V, output V; shape: N x 1; state: previous output in V.
update order: y[n] = alpha*x[n] + (1-alpha)*y[n-1].
"""
import csv
import json
import sys


def update(previous, sample, alpha):
    return alpha * sample + (1.0 - alpha) * previous


def run_filter(samples, alpha=0.5, initial=0.0):
    output = []
    previous = initial
    for sample in samples:
        current = update(previous, sample, alpha)
        output.append(current)
        previous = current
    return output


def main(argv):
    if len(argv) != 3:
        print("usage: estimator.py <capture.csv> <estimate.json>", file=sys.stderr)
        return 2
    samples = []
    with open(argv[1], newline="", encoding="utf-8") as handle:
        for row in csv.DictReader(handle):
            samples.append(float(row["voltage_v"]))
    output = run_filter(samples)
    with open(argv[2], "w", encoding="utf-8") as handle:
        json.dump(
            {
                "columns": ["voltage"],
                "units": ["V"],
                "data": [[value] for value in output],
                "metadata": {
                    "dtype": "float64",
                    "alpha": "0.5",
                    "initial": "0.0",
                    "input_samples": len(samples),
                    "state_update_order": "y[n]=alpha*x[n]+(1-alpha)*y[n-1]",
                },
            },
            handle,
        )
    return 0


if __name__ == "__main__":
    raise SystemExit(main(sys.argv))
"#;

fn file_digest(root: &Path, path: &str) -> String {
    hash::digest_bytes(&fs::read(root.join(path)).unwrap())
}

#[test]
fn case_b_capture_estimator_ui_is_traceable_and_verified() {
    let temp = tempfile::Builder::new()
        .prefix("b030-case-b-")
        .tempdir_in("<workspace>/_archmeld_temp")
        .unwrap();
    let root = temp.path().join("case-b");
    copy_dir(
        &Path::new(env!("CARGO_MANIFEST_DIR")).join("../../examples/case-b"),
        &root,
    );
    let state = temp.path().join("state");
    let mut store = Store::open(&state).unwrap();
    let handle = "ws.caseb";
    let project = open_project_for_handle(
        &mut store,
        handle,
        &root,
        "expedition",
        &state.join("sessions"),
        json!({"profile": "case-b", "device": "simulated"}),
    )
    .unwrap();
    let project_id = project["id"].as_str().unwrap().to_string();
    let build_config = json!({"pipeline": "case-b", "variant": "simulated-device"});
    let environment = json!({"os": "windows", "device": "simulated"});
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
    let toolchain = store
        .put_blob(
            br#"{"tools":["g++-mingw","python-3.11"],"profile":"case-b"}"#,
            "application/json",
        )
        .unwrap();
    let work = temp.path().join("work");
    fs::create_dir_all(&work).unwrap();
    let build_dir = work.join("build");
    fs::create_dir_all(&build_dir).unwrap();
    let capture_exe = build_dir.join("capture.exe");
    let build = run(
        &root,
        vec![
            "D:/Strawberry/c/bin/g++.exe".to_string(),
            "-std=c++17".to_string(),
            "-O2".to_string(),
            "capture/main.cpp".to_string(),
            "-o".to_string(),
            capture_exe.to_string_lossy().to_string(),
        ],
        &[],
    );
    assert_eq!(build["exit_code"], 0, "{build}");

    let baseline_work = work.join("baseline");
    let baseline = run(
        &root,
        vec![
            "python".to_string(),
            "pipeline.py".to_string(),
            "--capture".to_string(),
            capture_exe.to_string_lossy().to_string(),
            "--work".to_string(),
            baseline_work.to_string_lossy().to_string(),
        ],
        &[],
    );
    assert_eq!(baseline["exit_code"], 0, "{baseline}");
    let baseline_estimate_bytes = fs::read(root.join("artifacts/estimate.json")).unwrap();
    let baseline_estimate: Value = serde_json::from_slice(&baseline_estimate_bytes).unwrap();
    assert_eq!(baseline_estimate["data"].as_array().unwrap().len(), 32);
    assert_eq!(baseline_estimate["units"][0], "V");
    let baseline_ui_digest = file_digest(&root, "artifacts/ui-report.html");
    let capture_digest = hash::digest_bytes(&fs::read(baseline_work.join("capture.csv")).unwrap());

    let snapshot_payload = store
        .snapshot_payload(&project_id, &snapshot.snapshot_id)
        .unwrap()
        .unwrap();
    let file_blob = |path: &str| -> String {
        snapshot_payload["files"]
            .as_array()
            .unwrap()
            .iter()
            .find(|file| file["path"] == path)
            .unwrap_or_else(|| panic!("missing {path}"))["blob_digest"]
            .as_str()
            .unwrap()
            .to_string()
    };
    let capture_entity = "entity.caseb.capture";
    let estimator_entity = "entity.caseb.estimator";
    let ui_entity = "entity.caseb.ui";
    let evidence_id = "evidence.caseb.capture";
    let entity = |id: &str, path: &str, name: &str| {
        json!({
            "id": id,
            "project_id": project_id,
            "snapshot_id": snapshot.snapshot_id,
            "config_digest": config_digest,
            "kind": "symbol",
            "language": if path.ends_with(".cpp") { "cpp" } else { "python" },
            "qualified_name": name,
            "anchors": [{
                "path": path,
                "blob_digest": file_blob(path),
                "precision": "line_only",
                "byte_start": null,
                "byte_end": null,
                "line_start": 1,
                "line_end": 1,
                "encoding": "utf-8",
            }],
            "external": false,
            "backend_refs": [],
        })
    };
    let relation = |id: &str, source: &str, target: &str| {
        json!({
            "id": id,
            "project_id": project_id,
            "snapshot_id": snapshot.snapshot_id,
            "config_digest": config_digest,
            "source_id": source,
            "target_id": target,
            "kind": "references",
            "basis": "syntax",
            "evidence_ids": [evidence_id],
            "conditions": ["case-b pipeline wiring"],
            "resolution": "candidate",
        })
    };
    let batch = json!({
        "schema_version": "1.0",
        "project_id": project_id,
        "snapshot_id": snapshot.snapshot_id,
        "config_digest": config_digest,
        "run_id": "run.caseb.graph",
        "adapter_id": "caseb.syntax",
        "adapter_version": "1",
        "sequence": "0",
        "final": true,
        "coverage": {
            "state": "complete",
            "scope": "case-b source graph",
            "expected_count": 3,
            "observed_count": 3,
            "omitted_count": 0,
            "unresolved_count": 0,
            "limitations": [],
        },
        "entities": [
            entity(capture_entity, "capture/main.cpp", "capture::main"),
            entity(estimator_entity, "estimator/estimator.py", "estimator.run_filter"),
            entity(ui_entity, "ui/render.py", "ui.render"),
        ],
        "evidence": [{
            "id": evidence_id,
            "project_id": project_id,
            "snapshot_id": snapshot.snapshot_id,
            "config_digest": config_digest,
            "kind": "syntax",
            "producer": {
                "name": "caseb.syntax",
                "version": "1",
                "source_ref": "pipeline-wiring",
                "run_id": "run.caseb.graph",
            },
            "artifact_digest": file_blob("capture/main.cpp"),
            "locator": "capture/",
            "source_anchor": null,
            "scenario_id": null,
            "runtime_run_id": null,
            "coverage": {
                "state": "complete",
                "scope": "case-b source graph",
                "expected_count": 3,
                "observed_count": 3,
                "omitted_count": 0,
                "unresolved_count": 0,
                "limitations": [],
            },
            "freshness": "current",
        }],
        "relations": [
            relation("relation.caseb.estimator.capture", estimator_entity, capture_entity),
            relation("relation.caseb.ui.estimator", ui_entity, estimator_entity),
        ],
        "diagnostics": [],
    });
    store.import_batch(&batch).unwrap();

    let architecture = propose_architecture(
        &mut store,
        &json!({
            "project_id": project_id,
            "model": {
                "schema_version": "1.0",
                "id": "arch.caseb",
                "project_id": project_id,
                "snapshot_id": snapshot.snapshot_id,
                "revision": 1,
                "layer": "as_is",
                "status": "candidate",
                "components": [
                    {
                        "id": "component.capture",
                        "name": "Capture",
                        "purpose": "Simulated C++ device producing voltage samples.",
                        "responsibilities": ["write deterministic capture CSV"],
                        "non_responsibilities": ["estimate state"],
                        "member_entity_ids": [capture_entity],
                        "owned_state": [],
                        "owned_resources": ["simulated device"],
                        "runtime_placement": ["native process"],
                        "evidence_ids": [evidence_id],
                        "open_questions": [],
                    },
                    {
                        "id": "component.estimator",
                        "name": "Estimator",
                        "purpose": "First-order state estimator over captured voltage.",
                        "responsibilities": ["own previous output state"],
                        "non_responsibilities": ["render UI"],
                        "member_entity_ids": [estimator_entity],
                        "owned_state": ["previous output"],
                        "owned_resources": [],
                        "runtime_placement": ["python process"],
                        "evidence_ids": [evidence_id],
                        "open_questions": ["queue saturation and cancellation are not simulated"],
                    },
                    {
                        "id": "component.ui",
                        "name": "UI",
                        "purpose": "Static view of the current estimate.",
                        "responsibilities": ["render estimate without executing source"],
                        "non_responsibilities": ["own estimator state"],
                        "member_entity_ids": [ui_entity],
                        "owned_state": [],
                        "owned_resources": [],
                        "runtime_placement": ["static HTML"],
                        "evidence_ids": [evidence_id],
                        "open_questions": [],
                    }
                ],
                "dependencies": [
                    {
                        "source_id": "component.estimator",
                        "target_id": "component.capture",
                        "reason": "estimator consumes the capture CSV",
                        "evidence_ids": [evidence_id],
                        "status": "candidate",
                    },
                    {
                        "source_id": "component.ui",
                        "target_id": "component.estimator",
                        "reason": "UI consumes the estimate JSON",
                        "evidence_ids": [evidence_id],
                        "status": "candidate",
                    }
                ],
                "rules": [],
                "claim_ids": [],
            },
            "expected_revision": 1,
            "idempotency_key": "caseb-arch",
        }),
    )
    .unwrap();

    // Real trace with explicit source entity ids, bound to the base snapshot.
    let entities_file = work.join("entities.json");
    fs::write(
        &entities_file,
        serde_json::to_vec(&json!({
            "capture": capture_entity,
            "estimator": estimator_entity,
            "ui": ui_entity,
        }))
        .unwrap(),
    )
    .unwrap();
    let trace_file = work.join("trace.json");
    let trace_run = run(
        &root,
        vec![
            "python".to_string(),
            "pipeline.py".to_string(),
            "--capture".to_string(),
            capture_exe.to_string_lossy().to_string(),
            "--work".to_string(),
            work.join("traced").to_string_lossy().to_string(),
            "--snapshot-id".to_string(),
            snapshot.snapshot_id.clone(),
            "--config-digest".to_string(),
            config_digest.clone(),
            "--entities".to_string(),
            entities_file.to_string_lossy().to_string(),
            "--trace".to_string(),
            trace_file.to_string_lossy().to_string(),
        ],
        &[],
    );
    assert_eq!(trace_run["exit_code"], 0, "{trace_run}");
    let trace: Value = serde_json::from_slice(&fs::read(&trace_file).unwrap()).unwrap();
    let trace_report = import_trace(
        &mut store,
        &RuntimeImport {
            project_id: &project_id,
            run_id: "run.caseb.trace",
            snapshot_id: Some(&snapshot.snapshot_id),
            config_digest: &config_digest,
            allow_unbound: false,
            scenario_run_id: None,
        },
        &trace,
    )
    .unwrap();
    assert_eq!(trace_report["bound"], true, "{trace_report}");
    assert_eq!(trace_report["causal"]["mapped_spans"], 3, "{trace_report}");
    assert_eq!(trace_report["causal"]["mapped_entities"], 3, "{trace_report}");
    assert_eq!(trace_report["causal"]["unmapped_spans"], 0, "{trace_report}");

    let expected_estimate_digest = store
        .put_blob(&baseline_estimate_bytes, "application/json")
        .unwrap();
    let checks = vec![
        json!({
            "id": "check.pipeline",
            "kind": "behavior",
            "required": true,
            "description": "C++ capture, estimator and UI all execute",
            "command": {
                "argv": [
                    "python",
                    "pipeline.py",
                    "--capture",
                    capture_exe.to_string_lossy(),
                    "--work",
                    work.join("verified").to_string_lossy(),
                ],
                "cwd": null,
                "timeout_ms": 20000,
            },
            "checker_id": null,
            "sandbox": {"mode": "workspace", "include": [], "exclude": []},
            "predicate": {
                "kind": "test_report",
                "format": "json",
                "report_path": "artifacts/report.json",
                "min_tests": 3,
                "max_failures": 0,
            },
            "input_artifacts": [],
        }),
        json!({
            "id": "check.estimate",
            "kind": "domain",
            "required": true,
            "description": "full 32-sample estimate matches the baseline from the real C++ capture",
            "command": null,
            "checker_id": "numeric.tolerance.v1",
            "predicate": {
                "kind": "numeric_tolerance",
                "expected_artifact": expected_estimate_digest,
                "actual_artifact": "artifacts/estimate.json",
                "columns": ["voltage"],
                "atol": "1e-12",
                "rtol": "1e-12",
                "nonfinite_policy": "reject",
            },
            "input_artifacts": [expected_estimate_digest],
        }),
        json!({
            "id": "check.invariant",
            "kind": "domain",
            "required": true,
            "description": "estimate units, shape and finite range",
            "command": null,
            "checker_id": "numeric.invariants.v1",
            "predicate": {
                "kind": "invariant",
                "checker_id": "numeric.invariants.v1",
                "version": "1",
                "configuration": {
                    "artifact": "artifacts/estimate.json",
                    "columns": ["voltage"],
                    "units": {"voltage": "V"},
                    "shape": [32, 1],
                    "rules": [
                        {"id": "finite", "kind": "finite"},
                        {"id": "range", "kind": "range", "column": "voltage", "min": "0", "max": "2"}
                    ]
                }
            },
            "input_artifacts": [],
        }),
        json!({
            "id": "check.ui",
            "kind": "architecture",
            "required": true,
            "description": "UI renderer produced the same static document",
            "command": null,
            "checker_id": "file.digest.v1",
            "predicate": {
                "kind": "file_digest",
                "path": "artifacts/ui-report.html",
                "expected_digest": baseline_ui_digest,
            },
            "input_artifacts": [baseline_ui_digest],
        }),
    ];
    let mut contract = json!({
        "schema_version": "1.0",
        "id": "contract.caseb",
        "project_id": project_id,
        "revision": 1,
        "kind": "structural_refactor",
        "invariants": ["capture/estimate units and full dynamic trace are unchanged"],
        "allowed_changes": ["extract one pure estimator update helper"],
        "known_defects": ["simulated device only; no queue saturation or cancellation scenario"],
        "checks": checks,
        "digest": format!("sha256:{}", "0".repeat(64)),
        "frozen": false,
    });
    let mut preimage = contract.clone();
    preimage.as_object_mut().unwrap().remove("digest");
    preimage.as_object_mut().unwrap().remove("frozen");
    contract["digest"] = json!(hash::digest_value(&preimage).unwrap());
    let contract = propose_contract(
        &mut store,
        &json!({
            "project_id": project_id,
            "contract": contract,
            "expected_revision": 1,
            "idempotency_key": "caseb-contract",
        }),
    )
    .unwrap();
    let plan_id = "plan.caseb";
    let plan = propose_plan(
        &mut store,
        &json!({
            "project_id": project_id,
            "plan": {
                "schema_version": "1.0",
                "id": plan_id,
                "project_id": project_id,
                "revision": 1,
                "kind": "structural_refactor",
                "objective": "isolate estimator state update without changing the pipeline",
                "non_goals": ["no capture or UI change", "no protocol redesign"],
                "base_snapshot_id": snapshot.snapshot_id,
                "target_architecture_id": architecture["id"],
                "contract_id": contract["id"],
                "contract_digest": contract["digest"],
                "steps": [{
                    "id": "step.1",
                    "title": "pure estimator update",
                    "depends_on": [],
                    "allowed_paths": ["estimator/estimator.py"],
                    "required_check_ids": ["check.pipeline", "check.estimate", "check.invariant", "check.ui"],
                    "completion_criteria": ["all pipeline checks accepted"],
                }],
                "risks": ["state update order and floating-point expression order"],
                "evidence_ids": [evidence_id],
                "status": "draft",
            },
            "expected_revision": 1,
            "idempotency_key": "caseb-plan",
        }),
    )
    .unwrap();
    approve_plan(
        &mut store,
        &project_id,
        plan_id,
        plan["revision"].as_i64().unwrap(),
        contract["digest"].as_str().unwrap(),
        "operator",
        "caseb-plan-approve",
    )
    .unwrap();
    let capture_before = file_digest(&root, "capture/main.cpp");
    let ui_before = file_digest(&root, "ui/render.py");
    let estimator_before = file_digest(&root, "estimator/estimator.py");
    let changeset_id = "changeset.caseb";
    let changeset = create_changeset(
        &mut store,
        &json!({
            "schema_version": "1.0",
            "project_id": project_id,
            "changeset_id": changeset_id,
            "plan_id": plan_id,
            "plan_revision": 2,
            "step_id": "step.1",
            "read_set": [],
            "build_config": build_config,
            "environment": environment,
            "snapshot_options": {
                "include_untracked": true,
                "max_file_bytes": 1048576,
                "excluded_dirs": [".git", "target", "node_modules", "__pycache__"],
                "max_retries": 2
            },
            "expected_revision": 1,
            "idempotency_key": "caseb-create",
        }),
    )
    .unwrap();
    let editor = store
        .put_blob(br#"{"name":"caseb-editor","version":"1"}"#, "application/json")
        .unwrap();
    let seeded = prepare_changeset(
        &mut store,
        &json!({
            "project_id": project_id,
            "changeset_id": changeset_id,
            "expected_revision": changeset["revision"],
            "toolchain_digest": toolchain,
            "transform": null,
            "idempotency_key": "caseb-prepare-seed",
        }),
    )
    .unwrap();
    fs::write(
        PathBuf::from(seeded["staging"]["path"].as_str().unwrap()).join("estimator/estimator.py"),
        REFACTORED_ESTIMATOR,
    )
    .unwrap();
    let prepared = prepare_changeset(
        &mut store,
        &json!({
            "project_id": project_id,
            "changeset_id": changeset_id,
            "expected_revision": seeded["revision"],
            "toolchain_digest": toolchain,
            "transform": {
                "method": "manual_edit",
                "tool": {
                    "name": "caseb-editor",
                    "version": "1",
                    "identity_digest": editor,
                },
                "declared_moves": [],
                "patch_artifact": null,
                "prompt_artifact": null,
                "workspace_edit_operations": 0,
                "notes": ["extract pure update helper"],
            },
            "idempotency_key": "caseb-prepare-transform",
        }),
    )
    .unwrap();
    let approval = approve_prepared_change(
        &mut store,
        &json!({
            "project_id": project_id,
            "changeset_id": changeset_id,
            "expected_revision": prepared["revision"],
            "prepared_digest": prepared["prepared_digest"],
            "actor": "operator",
            "idempotency_key": "caseb-approve",
        }),
    )
    .unwrap();
    let applied = apply_changeset(
        &mut store,
        &json!({
            "project_id": project_id,
            "changeset_id": changeset_id,
            "expected_revision": prepared["revision"],
            "prepared_digest": prepared["prepared_digest"],
            "approval_id": approval["approval"]["approval_id"],
            "actor": "operator",
            "workspace_handle": handle,
            "idempotency_key": "caseb-apply",
        }),
    )
    .unwrap();
    assert_eq!(applied["state"], "applied");
    let verification = verify_changeset(
        &mut store,
        &json!({
            "project_id": project_id,
            "changeset_id": changeset_id,
            "expected_revision": applied["revision"],
            "workspace_handle": handle,
            "execution_grant": {
                "grant_id": "grant.caseb",
                "reason": "case-b end-to-end verification"
            },
            "environment": {"profile": "case-b"},
            "snapshot_id": null,
            "idempotency_key": "caseb-verify",
        }),
    )
    .unwrap();
    assert_eq!(verification["state"], "accepted", "{verification}");
    let checks_result = verification["checks"].as_array().unwrap();
    let by_id = |id: &str| checks_result.iter().find(|check| check["check_id"] == id).unwrap();
    for id in ["check.pipeline", "check.estimate", "check.invariant", "check.ui"] {
        assert_eq!(by_id(id)["state"], "passed", "{id}: {verification}");
    }
    let estimate_detail = &by_id("check.estimate")["detail"];
    let estimate_detail = if estimate_detail.get("predicate").is_some() {
        &estimate_detail["predicate"]
    } else {
        estimate_detail
    };
    assert_eq!(estimate_detail["mismatch_count"], 0);
    assert_eq!(estimate_detail["compared"].as_u64(), Some(32));
    assert_eq!(estimate_detail["shape"], json!([32, 1]));
    let verifying = get_changeset(&store, &project_id, changeset_id).unwrap();
    let accepted = accept_changeset(
        &mut store,
        &json!({
            "project_id": project_id,
            "changeset_id": changeset_id,
            "expected_revision": verifying["revision"],
            "verification_id": verification["verification_id"],
            "workspace_handle": handle,
            "actor": "operator",
            "idempotency_key": "caseb-accept",
        }),
    )
    .unwrap();
    assert_eq!(accepted["state"], "accepted");
    let report = export_report(
        &mut store,
        &json!({
            "project_id": project_id,
            "snapshot_id": verification["snapshot_id"],
            "output_dir": work.join("offline-report").to_string_lossy(),
            "include_source": false,
            "include_raw_traces": false,
        }),
    )
    .unwrap();
    let verified_bundle = verify_bundle(
        Path::new(report["output_dir"].as_str().unwrap()),
        &report["manifest"],
    )
    .unwrap();
    assert_eq!(verified_bundle["checked_files"], 4);
    let capture_after = file_digest(&root, "capture/main.cpp");
    let ui_after = file_digest(&root, "ui/render.py");
    let estimator_after = file_digest(&root, "estimator/estimator.py");
    assert_eq!(capture_after, capture_before);
    assert_eq!(ui_after, ui_before);
    assert_ne!(estimator_after, estimator_before);

    if std::env::var("ARCHMELD_WRITE_EVIDENCE").ok().as_deref() == Some("1") {
        let path = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../docs/evidence/B030-e2e-case-b.json");
        let mut root_evidence: Value = if path.exists() {
            serde_json::from_str(&fs::read_to_string(&path).unwrap()).unwrap_or_else(|_| json!({}))
        } else {
            json!({
                "generated_by": "crates/archmeld-core/tests/e2e_case_b.rs",
                "platform": std::env::consts::OS,
                "acceptance": {},
            })
        };
        root_evidence["acceptance"]["AT066"] = json!({
            "status": "passed",
            "case": "simulated C++ capture -> Python estimator -> static UI",
            "simulated_device": true,
            "source_hashes": {
                "capture": capture_after,
                "estimator_before": estimator_before,
                "estimator_after": estimator_after,
                "ui": ui_after,
            },
            "capture_csv_digest": capture_digest,
            "baseline_estimate_digest": expected_estimate_digest,
            "baseline_ui_digest": baseline_ui_digest,
            "build": build,
            "baseline_run": baseline,
            "trace_run": trace_run,
            "trace_import": trace_report,
            "architecture_id": architecture["id"],
            "contract_digest": contract["digest"],
            "plan_id": plan_id,
            "changeset_id": changeset_id,
            "prepared_digest": prepared["prepared_digest"],
            "applied_state": applied["state"],
            "verification_id": verification["verification_id"],
            "check_states": checks_result.iter().map(|check| json!({
                "check_id": check["check_id"],
                "state": check["state"],
                "detail": check["detail"],
            })).collect::<Vec<_>>(),
            "numeric_comparison": estimate_detail,
            "accepted_state": accepted["state"],
            "report": {
                "report_id": report["report_id"],
                "bundle_digest": report["bundle_digest"],
                "files": report["manifest"]["files"],
                "verified_files": verified_bundle["checked_files"],
                "source_included": false,
                "offline": true,
            },
            "limitations": [
                "capture device is simulated; no real hardware was accessed",
                "queue saturation, slow consumer and device disconnect are not simulated in this case",
                "trace maps spans to source entities; it does not fabricate observed call edges",
                "UI is static HTML, not the VS Code panel"
            ],
        });
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, serde_json::to_string_pretty(&root_evidence).unwrap() + "\n").unwrap();
    }
}
