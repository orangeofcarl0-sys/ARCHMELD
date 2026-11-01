// Copyright 2026 ARCHMELD contributors
// SPDX-License-Identifier: Apache-2.0

//! B016 scenario/trace association: a trace is linked to a recorded scenario
//! run only when the caller declares the run explicitly and its snapshot and
//! build config match. A `scenario_id` embedded in the trace without that
//! declaration stays producer metadata and is marked unverified.

use archmeld_core::runtime::{import_trace, runtime_summary, timeline, RuntimeImport};
use archmeld_core::scenario::{run_scenario, ExecutionGrant, ScenarioSpec};
use archmeld_core::snapshot::{capture_snapshot, SnapshotInputs, SnapshotOptions};
use archmeld_core::store::Store;
use serde_json::{json, Value};
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::Duration;

static EVIDENCE_LOCK: Mutex<()> = Mutex::new(());

const P: &str = "project.trace.link";

struct Fixture {
    _dir: tempfile::TempDir,
    root: PathBuf,
    store: Store,
    snapshot_id: String,
    config_digest: String,
}

fn setup() -> Fixture {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().join("repo");
    fs::create_dir_all(root.join("src")).unwrap();
    fs::write(root.join("src/app.py"), "def work():\n    return 1\n").unwrap();
    let mut store = Store::open(dir.path().join("state")).unwrap();
    store.ensure_project(P, "expedition").unwrap();
    let snapshot = capture_snapshot(
        &mut store,
        P,
        &root,
        &SnapshotInputs {
            build_config: json!({"variant": "trace-link"}),
            environment: json!({"os": "test"}),
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
    Fixture {
        _dir: dir,
        root,
        store,
        snapshot_id: snapshot.snapshot_id,
        config_digest,
    }
}

fn run_scenario_fixture(fixture: &mut Fixture, run_id: &str) -> Value {
    run_scenario(
        &mut fixture.store,
        &ScenarioSpec {
            project_id: P.to_string(),
            run_id: run_id.to_string(),
            snapshot_id: fixture.snapshot_id.clone(),
            config_digest: fixture.config_digest.clone(),
            argv: vec!["python".into(), "-c".into(), "print('linked')".into()],
            cwd: fixture.root.clone(),
            env: vec![],
            timeout: Duration::from_secs(20),
            sandbox: None,
        },
        &ExecutionGrant::grant("b016-trace-link", "scenario/trace association test"),
    )
    .unwrap()
}

fn trace(fixture: &Fixture, snapshot_id: &str, scenario: Option<&str>) -> Value {
    let mut value = json!({
        "version": "1.0",
        "snapshot_id": snapshot_id,
        "config_digest": fixture.config_digest,
        "build_identity": {"kind": "python", "variant": "trace-link"},
        "capture": {"tool": "test-probe", "version": "1", "lossy": false, "loss_notes": []},
        "clock": {"kind": "monotonic", "unit": "ns", "origin_ns": "0"},
        "probes": [],
        "spans": [{
            "span_id": "span.work",
            "parent_span_id": null,
            "name": "work",
            "kind": "function",
            "start_ns": "100",
            "end_ns": "200",
            "attributes": {},
        }],
        "links": [],
    });
    if let Some(scenario) = scenario {
        value["scenario_id"] = json!(scenario);
    }
    value
}

fn import(
    fixture: &mut Fixture,
    run_id: &str,
    scenario_run_id: Option<&str>,
    trace_value: &Value,
) -> Result<Value, archmeld_core::CoreError> {
    import_trace(
        &mut fixture.store,
        &RuntimeImport {
            project_id: P,
            run_id,
            snapshot_id: Some(&fixture.snapshot_id),
            config_digest: &fixture.config_digest,
            allow_unbound: false,
            scenario_run_id,
        },
        trace_value,
    )
}

#[test]
fn explicit_matching_scenario_run_is_linked_and_queryable() {
    let mut fixture = setup();
    let manifest = run_scenario_fixture(&mut fixture, "run.link.ok");
    assert_eq!(manifest["exit_code"], 0);
    let trace_value = trace(&fixture, &fixture.snapshot_id, Some("run.link.ok"));
    let report = import(
        &mut fixture,
        "run.trace.ok",
        Some("run.link.ok"),
        &trace_value,
    )
    .unwrap();
    assert_eq!(report["scenario_run_id"], "run.link.ok");
    assert_eq!(report["span_count"], 1);

    let summary = runtime_summary(&fixture.store, P, "run.trace.ok").unwrap();
    assert_eq!(summary["scenario_run_id"], "run.link.ok");
    assert_eq!(summary["span_count"], 1);

    let timeline = timeline(&fixture.store, P, "run.trace.ok", None, None, 10).unwrap();
    assert_eq!(timeline["scenario_run_id"], "run.link.ok");

    let evidence = fixture
        .store
        .evidence_payloads(P, &fixture.snapshot_id)
        .unwrap();
    let runtime = evidence
        .iter()
        .find(|item| item["id"] == "evidence.runtime.run.trace.ok")
        .expect("runtime evidence");
    assert_eq!(runtime["scenario_id"], "run.link.ok");
    assert_eq!(runtime["scenario"]["run_id"], "run.link.ok");
    assert_eq!(runtime["scenario"]["association"], "explicit: existing scenario run with matching snapshot and build config");
    assert_eq!(runtime["scenario"]["exit_code"], 0);
    assert_eq!(runtime["scenario"]["timed_out"], false);
    assert_eq!(runtime["scenario"]["snapshot_id"], fixture.snapshot_id);

    write_evidence(json!({
        "case": "explicit_link",
        "scenario_run_id": timeline["scenario_run_id"],
        "evidence_scenario": runtime["scenario"],
        "evidence_limitations": runtime["coverage"]["limitations"],
        "summary_scenario_run_id": summary["scenario_run_id"],
    }));

    // The same trace without the explicit declaration is a different
    // association and must not silently reuse the linked run.
    let trace_value = trace(&fixture, &fixture.snapshot_id, Some("run.link.ok"));
    let error = import(&mut fixture, "run.trace.ok", None, &trace_value).unwrap_err();
    assert!(
        error.to_string().contains("different scenario association"),
        "{error}"
    );
}

#[test]
fn missing_and_mismatched_scenario_runs_are_rejected() {
    let mut fixture = setup();
    run_scenario_fixture(&mut fixture, "run.link.ok");
    let trace_value = trace(&fixture, &fixture.snapshot_id, None);
    let error = import(
        &mut fixture,
        "run.trace.missing",
        Some("run.link.absent"),
        &trace_value,
    )
    .unwrap_err();
    assert!(error.to_string().contains("not found in this project"), "{error}");

    let trace_value = trace(&fixture, &fixture.snapshot_id, Some("run.link.other"));
    let error = import(
        &mut fixture,
        "run.trace.mismatch",
        Some("run.link.ok"),
        &trace_value,
    )
    .unwrap_err();
    assert!(
        error.to_string().contains("does not match the declared scenario run"),
        "{error}"
    );
}

#[test]
fn scenario_run_from_another_snapshot_is_rejected() {
    let mut fixture = setup();
    run_scenario_fixture(&mut fixture, "run.link.old");
    fs::write(fixture.root.join("src/app.py"), "def work():\n    return 2\n").unwrap();
    let second = capture_snapshot(
        &mut fixture.store,
        P,
        &fixture.root,
        &SnapshotInputs {
            build_config: json!({"variant": "trace-link"}),
            environment: json!({"os": "test"}),
        },
        &SnapshotOptions {
            include_untracked: true,
            ..SnapshotOptions::default()
        },
    )
    .unwrap();
    assert_ne!(second.snapshot_id, fixture.snapshot_id);
    let trace_value = trace(&fixture, &second.snapshot_id, None);
    let error = import_trace(
        &mut fixture.store,
        &RuntimeImport {
            project_id: P,
            run_id: "run.trace.other.snapshot",
            snapshot_id: Some(&second.snapshot_id),
            config_digest: &fixture.config_digest,
            allow_unbound: false,
            scenario_run_id: Some("run.link.old"),
        },
        &trace_value,
    )
    .unwrap_err();
    assert!(error.to_string().contains("snapshot differs"), "{error}");
}

#[test]
fn producer_declared_scenario_id_is_recorded_as_unverified() {
    let mut fixture = setup();
    let trace_value = trace(&fixture, &fixture.snapshot_id, Some("producer.only"));
    let report = import(
        &mut fixture,
        "run.trace.producer",
        None,
        &trace_value,
    )
    .unwrap();
    assert_eq!(report["scenario_run_id"], Value::Null);
    let evidence = fixture
        .store
        .evidence_payloads(P, &fixture.snapshot_id)
        .unwrap();
    let runtime = evidence
        .iter()
        .find(|item| item["id"] == "evidence.runtime.run.trace.producer")
        .expect("runtime evidence");
    assert_eq!(runtime["scenario_id"], "producer.only");
    assert_eq!(
        runtime["scenario"]["association"],
        "producer_declared_unverified"
    );
    assert!(runtime["coverage"]["limitations"]
        .as_array()
        .unwrap()
        .iter()
        .any(|item| item.as_str().unwrap_or("").contains("producer-declared")));
    let summary = runtime_summary(&fixture.store, P, "run.trace.producer").unwrap();
    assert_eq!(summary["scenario_run_id"], Value::Null);

    write_evidence(json!({
        "case": "producer_declared",
        "scenario_id": runtime["scenario_id"],
        "evidence_scenario": runtime["scenario"],
        "evidence_limitations": runtime["coverage"]["limitations"],
        "summary_scenario_run_id": summary["scenario_run_id"],
    }));
}

fn write_evidence(case: Value) {
    if std::env::var("ARCHMELD_WRITE_EVIDENCE").ok().as_deref() != Some("1") {
        return;
    }
    let _guard = EVIDENCE_LOCK.lock().unwrap();
    let path = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../docs/release-evidence/B016-scenario-trace-link.json");
    let mut root = if path.exists() {
        serde_json::from_str::<Value>(&fs::read_to_string(&path).unwrap())
            .unwrap_or_else(|_| json!({}))
    } else {
        json!({
            "schema_version": "1.0",
            "generated_by": "crates/archmeld-core/tests/scenario_trace_link.rs",
            "platform": std::env::consts::OS,
            "cases": [],
        })
    };
    let cases = root["cases"].as_array_mut().unwrap();
    let name = case["case"].as_str().unwrap_or("").to_string();
    if let Some(existing) = cases.iter_mut().find(|item| item["case"] == name) {
        *existing = case;
    } else {
        cases.push(case);
    }
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(path, serde_json::to_string_pretty(&root).unwrap() + "
").unwrap();
}
