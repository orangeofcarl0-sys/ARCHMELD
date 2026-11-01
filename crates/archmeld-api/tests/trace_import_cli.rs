// Copyright 2026 ARCHMELD contributors
// SPDX-License-Identifier: Apache-2.0

//! CLI trace-import: real Perfetto/Tracy fixtures through the installed
//! `archmeld` command, including native retention and refusal paths.

use archmeld_core::scenario::{run_scenario, ExecutionGrant, ScenarioSpec};
use archmeld_core::snapshot::{capture_snapshot, SnapshotInputs, SnapshotOptions};
use archmeld_core::store::Store;
use serde_json::{json, Value};
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::Duration;

struct Fixture {
    _dir: tempfile::TempDir,
    state: PathBuf,
    snapshot_id: String,
    config_digest: String,
}

fn fixture_path(name: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../adapters/traces/fixtures")
        .join(name)
}

fn setup() -> Fixture {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().join("repo");
    fs::create_dir_all(&root).unwrap();
    fs::write(root.join("app.py"), "def queue():\n    return 1\n").unwrap();
    let state = dir.path().join("state");
    let snapshot_id;
    let config_digest;
    {
        let mut store = Store::open(&state).unwrap();
        store.ensure_project("project.cli.trace", "expedition").unwrap();
        let snapshot = capture_snapshot(
            &mut store,
            "project.cli.trace",
            &root,
            &SnapshotInputs {
                build_config: json!({"variant": "cli-trace"}),
                environment: json!({"os": "test"}),
            },
            &SnapshotOptions {
                include_untracked: true,
                ..SnapshotOptions::default()
            },
        )
        .unwrap();
        config_digest = snapshot.snapshot["config_digest"]
            .as_str()
            .unwrap()
            .to_string();
        snapshot_id = snapshot.snapshot_id.clone();
        let file = &snapshot.files[0];
        let batch = json!({
            "schema_version": "1.0",
            "project_id": "project.cli.trace",
            "snapshot_id": snapshot.snapshot_id,
            "config_digest": config_digest,
            "run_id": "run.cli.fixture",
            "adapter_id": "fixture",
            "adapter_version": "1",
            "sequence": "0",
            "final": true,
            "coverage": {
                "state": "complete",
                "scope": "fixture",
                "expected_count": 1,
                "observed_count": 1,
                "omitted_count": 0,
                "unresolved_count": 0,
                "limitations": [],
            },
            "entities": [{
                "id": "entity.queue",
                "project_id": "project.cli.trace",
                "snapshot_id": snapshot.snapshot_id,
                "config_digest": config_digest,
                "kind": "symbol",
                "language": "python",
                "qualified_name": "app.py::queue",
                "anchors": [{
                    "path": file.path,
                    "blob_digest": file.blob_digest,
                    "precision": "line_only",
                    "byte_start": null,
                    "byte_end": null,
                    "line_start": 1,
                    "line_end": 1,
                    "encoding": "utf-8",
                }],
                "external": false,
                "backend_refs": [],
            }],
            "evidence": [{
                "id": "evidence.syntax.cli",
                "project_id": "project.cli.trace",
                "snapshot_id": snapshot.snapshot_id,
                "config_digest": config_digest,
                "kind": "syntax",
                "producer": {"name": "fixture", "version": "1", "source_ref": "fixture", "run_id": "run.cli.fixture"},
                "artifact_digest": file.blob_digest,
                "locator": "fixture",
                "source_anchor": null,
                "scenario_id": null,
                "runtime_run_id": null,
                "coverage": {
                    "state": "complete",
                    "scope": "fixture",
                    "expected_count": 1,
                    "observed_count": 1,
                    "omitted_count": 0,
                    "unresolved_count": 0,
                    "limitations": [],
                },
                "freshness": "current",
            }],
            "relations": [],
            "diagnostics": [],
        });
        store.import_batch(&batch).unwrap();
    }
    Fixture {
        _dir: dir,
        state,
        snapshot_id,
        config_digest,
    }
}

fn run_cli(arguments: &[String]) -> std::process::Output {
    Command::new(env!("CARGO_BIN_EXE_archmeld"))
        .args(arguments)
        .output()
        .expect("archmeld CLI runs")
}

fn run_scenario_for(fixture: &Fixture, run_id: &str) -> Value {
    let mut store = Store::open(&fixture.state).unwrap();
    run_scenario(
        &mut store,
        &ScenarioSpec {
            project_id: "project.cli.trace".to_string(),
            run_id: run_id.to_string(),
            snapshot_id: fixture.snapshot_id.clone(),
            config_digest: fixture.config_digest.clone(),
            argv: vec!["python".into(), "-c".into(), "print('linked')".into()],
            cwd: fixture._dir.path().to_path_buf(),
            env: vec![],
            timeout: Duration::from_secs(20),
            sandbox: None,
        },
        &ExecutionGrant::grant("cli-trace-link", "CLI association test"),
    )
    .unwrap()
}

#[test]
fn perfetto_import_runs_through_the_cli() {
    let fixture = setup();
    let output = run_cli(&[
        "trace-import".into(),
        "--state".into(),
        fixture.state.display().to_string(),
        "--project".into(),
        "project.cli.trace".into(),
        "--run-id".into(),
        "run.cli.perfetto".into(),
        "--snapshot".into(),
        fixture.snapshot_id.clone(),
        "--format".into(),
        "perfetto".into(),
        "--input".into(),
        fixture_path("perfetto-counter-state.pftrace").display().to_string(),
        "--memory-limit-mb".into(),
        "8192".into(),
    ]);
    assert!(
        output.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let value: Value = serde_json::from_slice(&output.stdout).unwrap();
    let timeline = &value["timeline"];
    assert_eq!(value["memory_limit_mb"], json!(8192));
    assert_eq!(timeline["bound"], json!(true));
    assert_eq!(timeline["span_count"], json!(2));
    assert_eq!(timeline["metric_count"], json!(2));
    assert_eq!(timeline["counters"][0]["sample_count"], json!(4));
    assert_eq!(timeline["states"][0]["interval_count"], json!(2));
    assert_eq!(timeline["causal"]["metrics"]["causality"], json!("none: counters and states never create causal edges"));
    // The fixture names entity.queue, which exists in this ledger.
    assert_eq!(timeline["causal"]["mapped_spans"], json!(1));
}

#[test]
fn tracy_import_retains_the_native_capture_through_the_cli() {
    let fixture = setup();
    let dir = fixture._dir.path();
    let locator = dir.join("locator.json");
    fs::write(
        &locator,
        serde_json::to_vec(&json!({
            "locations": [
                {"path": "tracy_probe.cc", "line": 14, "entity_id": "entity.queue"},
                {"path": "tracy_probe.cc", "line": 19, "entity_id": "entity.queue"}
            ]
        }))
        .unwrap(),
    )
    .unwrap();
    let output = run_cli(&[
        "trace-import".into(),
        "--state".into(),
        fixture.state.display().to_string(),
        "--project".into(),
        "project.cli.trace".into(),
        "--run-id".into(),
        "run.cli.tracy".into(),
        "--snapshot".into(),
        fixture.snapshot_id.clone(),
        "--format".into(),
        "tracy".into(),
        "--tracy-capture".into(),
        fixture_path("tracy-zones-plots.tracy").display().to_string(),
        "--tracy-csv".into(),
        fixture_path("tracy-zones-plots.csv").display().to_string(),
        "--locator".into(),
        locator.display().to_string(),
    ]);
    assert!(
        output.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let value: Value = serde_json::from_slice(&output.stdout).unwrap();
    let timeline = &value["timeline"];
    assert_eq!(timeline["span_count"], json!(2));
    assert_eq!(timeline["metric_count"], json!(1));
    assert_eq!(timeline["causal"]["mapped_spans"], json!(2));
    assert_eq!(timeline["lossy"], json!(true));
    let native = value["native_capture"]["sha256"].as_str().unwrap();
    assert_eq!(
        native,
        "sha256:33b3932e9d9d06ecb47d90a2616ecdf5d1036582ea5be6170a76c91bed5fbfbf"
    );
    let hexdigest = native.strip_prefix("sha256:").unwrap();
    let blob = fixture
        .state
        .join("objects")
        .join("sha256")
        .join(&hexdigest[..2])
        .join(&hexdigest[2..]);
    assert_eq!(
        fs::read(&blob).unwrap(),
        fs::read(fixture_path("tracy-zones-plots.tracy")).unwrap(),
        "the raw capture is retained byte for byte"
    );
}

#[test]
fn cli_refuses_unbound_and_mismatched_traces() {
    let fixture = setup();
    let unbound = run_cli(&[
        "trace-import".into(),
        "--state".into(),
        fixture.state.display().to_string(),
        "--project".into(),
        "project.cli.trace".into(),
        "--run-id".into(),
        "run.cli.unbound".into(),
        "--config-digest".into(),
        fixture.config_digest.clone(),
        "--format".into(),
        "probe".into(),
        "--input".into(),
        fixture_path("missing.jsonl").display().to_string(),
    ]);
    assert!(!unbound.status.success());
    assert!(String::from_utf8_lossy(&unbound.stderr).contains("--allow-unbound"));
    let mismatch = run_cli(&[
        "trace-import".into(),
        "--state".into(),
        fixture.state.display().to_string(),
        "--project".into(),
        "project.cli.trace".into(),
        "--run-id".into(),
        "run.cli.mismatch".into(),
        "--snapshot".into(),
        fixture.snapshot_id.clone(),
        "--config-digest".into(),
        format!("sha256:{}", "0".repeat(64)),
        "--format".into(),
        "probe".into(),
        "--input".into(),
        fixture_path("missing.jsonl").display().to_string(),
    ]);
    assert!(!mismatch.status.success());
    assert!(String::from_utf8_lossy(&mismatch.stderr).contains("does not match"));
}

#[test]
fn a_memory_ceiling_below_the_import_footprint_fails_the_process() {
    let fixture = setup();
    let output = run_cli(&[
        "trace-import".into(),
        "--state".into(),
        fixture.state.display().to_string(),
        "--project".into(),
        "project.cli.trace".into(),
        "--run-id".into(),
        "run.cli.memory".into(),
        "--snapshot".into(),
        fixture.snapshot_id.clone(),
        "--format".into(),
        "perfetto".into(),
        "--input".into(),
        fixture_path("perfetto-counter-state.pftrace").display().to_string(),
        "--memory-limit-mb".into(),
        "1".into(),
    ]);
    assert!(!output.status.success(), "an 1 MB ceiling must not succeed");
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        !stderr.contains("AssignProcessToJobObject failed"),
        "the ceiling was not applied: {stderr}"
    );
}

#[test]
fn normalized_trace_import_accepts_an_explicit_envelope() {
    let fixture = setup();
    let trace = serde_json::json!({
        "version": "1.0",
        "snapshot_id": fixture.snapshot_id,
        "config_digest": fixture.config_digest,
        "build_identity": {"kind": "normalized", "variant": "cli"},
        "capture": {"tool": "offline", "version": "1", "lossy": false, "loss_notes": []},
        "clock": {"kind": "monotonic", "unit": "ns", "origin_ns": "0"},
        "probes": [],
        "spans": [{
            "span_id": "span.one",
            "parent_span_id": null,
            "name": "work",
            "kind": "function",
            "start_ns": "100",
            "end_ns": "200",
            "attributes": {},
        }],
        "links": [],
        "counters": [],
        "states": [],
    });
    let path = fixture._dir.path().join("normalized.json");
    fs::write(&path, serde_json::to_vec(&trace).unwrap()).unwrap();
    let output = run_cli(&[
        "trace-import".into(),
        "--state".into(),
        fixture.state.display().to_string(),
        "--project".into(),
        "project.cli.trace".into(),
        "--run-id".into(),
        "run.cli.normalized".into(),
        "--snapshot".into(),
        fixture.snapshot_id.clone(),
        "--format".into(),
        "normalized".into(),
        "--input".into(),
        path.display().to_string(),
    ]);
    assert!(
        output.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let value: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(value["timeline"]["span_count"], json!(1));
    assert_eq!(value["memory_limit_mb"], Value::Null);
}

#[test]
fn explicit_scenario_run_association_runs_through_the_cli() {
    let fixture = setup();
    let manifest = run_scenario_for(&fixture, "run.cli.scenario");
    assert_eq!(manifest["exit_code"], 0);
    let trace = serde_json::json!({
        "version": "1.0",
        "snapshot_id": fixture.snapshot_id,
        "config_digest": fixture.config_digest,
        "build_identity": {"kind": "normalized", "variant": "cli"},
        "capture": {"tool": "offline", "version": "1", "lossy": false, "loss_notes": []},
        "clock": {"kind": "monotonic", "unit": "ns", "origin_ns": "0"},
        "probes": [],
        "spans": [{
            "span_id": "span.linked",
            "parent_span_id": null,
            "name": "work",
            "kind": "function",
            "start_ns": "100",
            "end_ns": "200",
            "attributes": {},
        }],
        "links": [],
        "counters": [],
        "states": [],
    });
    let path = fixture._dir.path().join("linked.json");
    fs::write(&path, serde_json::to_vec(&trace).unwrap()).unwrap();
    let output = run_cli(&[
        "trace-import".into(),
        "--state".into(),
        fixture.state.display().to_string(),
        "--project".into(),
        "project.cli.trace".into(),
        "--run-id".into(),
        "run.cli.linked".into(),
        "--snapshot".into(),
        fixture.snapshot_id.clone(),
        "--scenario-run-id".into(),
        "run.cli.scenario".into(),
        "--format".into(),
        "normalized".into(),
        "--input".into(),
        path.display().to_string(),
    ]);
    assert!(
        output.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let value: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(value["timeline"]["scenario_run_id"], "run.cli.scenario");
    assert_eq!(value["timeline"]["span_count"], json!(1));

    let rejected = run_cli(&[
        "trace-import".into(),
        "--state".into(),
        fixture.state.display().to_string(),
        "--project".into(),
        "project.cli.trace".into(),
        "--run-id".into(),
        "run.cli.linked.missing".into(),
        "--snapshot".into(),
        fixture.snapshot_id.clone(),
        "--scenario-run-id".into(),
        "run.cli.absent".into(),
        "--format".into(),
        "normalized".into(),
        "--input".into(),
        path.display().to_string(),
    ]);
    assert!(!rejected.status.success());
    assert!(String::from_utf8_lossy(&rejected.stderr).contains("not found in this project"));
}

#[test]
fn trace_processor_backend_imports_the_fixture_through_the_cli() {
    let Some(processor) = std::env::var_os("ARCHMELD_TRACE_PROCESSOR") else {
        return;
    };
    let fixture = setup();
    let output = run_cli(&[
        "trace-import".into(),
        "--state".into(),
        fixture.state.display().to_string(),
        "--project".into(),
        "project.cli.trace".into(),
        "--run-id".into(),
        "run.cli.trace_processor".into(),
        "--allow-unbound".into(),
        "--config-digest".into(),
        format!("sha256:{}", "a".repeat(64)),
        "--format".into(),
        "perfetto".into(),
        "--input".into(),
        fixture_path("perfetto-counter-state.pftrace").display().to_string(),
        "--trace-processor".into(),
        processor.to_string_lossy().into_owned(),
    ]);
    assert!(
        output.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let value: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(value["trace_processor"]["slice_rows"], json!(2));
    assert_eq!(value["trace_processor"]["counter_rows"], json!(4));
    assert!(value["trace_processor"]["sha256"]
        .as_str()
        .unwrap()
        .starts_with("sha256:"));
    assert!(value["trace_processor"]["version"]
        .as_str()
        .unwrap()
        .starts_with("Perfetto v59.1"));
    assert_eq!(value["timeline"]["span_count"], json!(2));
    assert_eq!(value["timeline"]["metric_count"], json!(1));
    assert_eq!(value["timeline"]["counters"][0]["sample_count"], json!(4));
    assert_eq!(value["timeline"]["bound"], json!(false));
}

#[test]
fn scenario_cli_runs_a_snapshot_copy_and_refuses_unpinned_containers() {
    let fixture = setup();
    let root = fixture._dir.path().join("repo");
    fs::write(root.join("app.py"), "print('live')\n").unwrap();
    let workspace = root.canonicalize().unwrap();
    let script = "import pathlib; text = pathlib.Path('app.py').read_text(); assert text.startswith('def queue():') and 'return 1' in text; print('cli-sandbox-ok')";
    let output = run_cli(&[
        "scenario".into(),
        "--state".into(),
        fixture.state.display().to_string(),
        "--project".into(),
        "project.cli.trace".into(),
        "--snapshot".into(),
        fixture.snapshot_id.clone(),
        "--workspace".into(),
        workspace.display().to_string(),
        "--grant-id".into(),
        "grant.cli.sandbox".into(),
        "--reason".into(),
        "cli sandbox test".into(),
        "--run-id".into(),
        "run.cli.scenario.sandbox".into(),
        "--sandbox-mode".into(),
        "workspace_copy".into(),
        "--sandbox-include".into(),
        "app.py".into(),
        "--".into(),
        "python".into(),
        "-c".into(),
        script.into(),
    ]);
    assert!(
        output.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let value: Value = serde_json::from_slice(&output.stdout).unwrap();
    let run = &value["result"];
    if run["exit_code"] != json!(0) {
        let digest = run["stderr"]["digest"].as_str().unwrap();
        let hex = digest.strip_prefix("sha256:").unwrap();
        let blob = fixture
            .state
            .join("objects")
            .join("sha256")
            .join(&hex[..2])
            .join(&hex[2..]);
        panic!(
            "exit {} stderr: {}",
            run["exit_code"],
            String::from_utf8_lossy(&fs::read(blob).unwrap())
        );
    }
    assert_eq!(run["sandbox"]["mode"], "workspace_copy");
    assert_eq!(run["sandbox"]["isolated"], json!(true));
    assert_eq!(run["sandbox"]["file_count"], json!(1));
    assert_eq!(
        fs::read_to_string(root.join("app.py")).unwrap(),
        "print('live')\n"
    );
    let digest = run["stdout"]["digest"].as_str().unwrap();
    let hex = digest.strip_prefix("sha256:").unwrap();
    let blob = fixture
        .state
        .join("objects")
        .join("sha256")
        .join(&hex[..2])
        .join(&hex[2..]);
    assert!(String::from_utf8_lossy(&fs::read(blob).unwrap()).contains("cli-sandbox-ok"));

    let unpinned = run_cli(&[
        "scenario".into(),
        "--state".into(),
        fixture.state.display().to_string(),
        "--project".into(),
        "project.cli.trace".into(),
        "--snapshot".into(),
        fixture.snapshot_id.clone(),
        "--workspace".into(),
        workspace.display().to_string(),
        "--grant-id".into(),
        "grant.cli.container".into(),
        "--reason".into(),
        "cli container refusal".into(),
        "--run-id".into(),
        "run.cli.scenario.container".into(),
        "--sandbox-mode".into(),
        "container".into(),
        "--sandbox-include".into(),
        "app.py".into(),
        "--sandbox-image".into(),
        "debian:latest".into(),
        "--".into(),
        "sh".into(),
        "-c".into(),
        "true".into(),
    ]);
    assert!(!unpinned.status.success());
    assert!(
        String::from_utf8_lossy(&unpinned.stderr).contains("digest-pinned"),
        "{}",
        String::from_utf8_lossy(&unpinned.stderr)
    );
}

#[test]
fn ledger_migrate_cli_copies_and_refuses_an_existing_target() {
    let fixture = setup();
    let target = fixture._dir.path().join("migrated-ledger");
    let output = run_cli(&[
        "ledger-migrate".into(),
        "--from".into(),
        fixture.state.display().to_string(),
        "--to".into(),
        target.display().to_string(),
        "--reason".into(),
        "cli migration test".into(),
    ]);
    assert!(
        output.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let value: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(value["command"], "ledger-migrate");
    assert_eq!(value["receipt"]["mode"], "copy_entire_ledger");
    assert_eq!(value["receipt"]["merge"], json!(false));
    assert_eq!(value["receipt"]["verification"]["sqlite_integrity"], "ok");
    assert_eq!(value["receipt"]["cas"]["missing_referenced"], json!(0));
    assert!(target.join("state.sqlite3").is_file());
    assert!(target.join("migration.json").is_file());

    let again = run_cli(&[
        "ledger-migrate".into(),
        "--from".into(),
        fixture.state.display().to_string(),
        "--to".into(),
        target.display().to_string(),
        "--reason".into(),
        "must refuse".into(),
    ]);
    assert!(!again.status.success());
    assert!(
        String::from_utf8_lossy(&again.stderr).contains("already exists"),
        "{}",
        String::from_utf8_lossy(&again.stderr)
    );
}

#[test]
fn numeric_check_cli_exposes_statistical_ulp_and_dimension_modes() {
    let fixture = setup();
    let root = fixture._dir.path().join("numeric-root");
    fs::create_dir_all(&root).unwrap();
    fs::write(root.join("a.csv"), "value\n1\n2\n3\n").unwrap();
    fs::write(root.join("b.csv"), "value\n1\n2\n3\n").unwrap();
    let predicate = |name: &str, body: Value| {
        let path = root.join(name);
        fs::write(&path, serde_json::to_vec(&body).unwrap()).unwrap();
        path
    };
    let statistical = predicate(
        "statistical.json",
        json!({
            "expected_artifact": "a.csv",
            "actual_artifact": "b.csv",
            "mode": "statistical",
            "statistics": {"mean_abs_error_max": "0", "rms_error_max": "0", "max_abs_error_max": "0", "p95_abs_error_max": "0"},
            "nonfinite_policy": "reject",
        }),
    );
    let output = run_cli(&[
        "numeric-check".into(),
        "--state".into(),
        fixture.state.display().to_string(),
        "--root".into(),
        root.display().to_string(),
        "--predicate".into(),
        statistical.display().to_string(),
    ]);
    assert!(
        output.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let value: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(value["result"]["state"], "passed");
    assert_eq!(value["result"]["detail"]["mode"], "statistical");

    let ulp = predicate(
        "ulp.json",
        json!({
            "expected_artifact": "a.csv",
            "actual_artifact": "b.csv",
            "mode": "ulp",
            "max_ulps": "0",
            "nonfinite_policy": "reject",
        }),
    );
    let output = run_cli(&[
        "numeric-check".into(),
        "--state".into(),
        fixture.state.display().to_string(),
        "--root".into(),
        root.display().to_string(),
        "--predicate".into(),
        ulp.display().to_string(),
    ]);
    assert!(
        output.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let value: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(value["result"]["state"], "passed");
    assert_eq!(value["result"]["detail"]["max_ulp_distance"], "0");

    let matlab = predicate(
        "matlab.json",
        json!({
            "expected_artifact": "a.csv",
            "actual_artifact": "b.csv",
            "semantics": "matlab",
            "nonfinite_policy": "reject",
        }),
    );
    let output = run_cli(&[
        "numeric-check".into(),
        "--state".into(),
        fixture.state.display().to_string(),
        "--root".into(),
        root.display().to_string(),
        "--predicate".into(),
        matlab.display().to_string(),
    ]);
    assert!(output.status.success());
    let value: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(value["result"]["state"], "unsupported");
}
