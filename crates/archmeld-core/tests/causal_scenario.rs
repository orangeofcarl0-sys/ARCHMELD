// Copyright 2026 ARCHMELD contributors
// SPDX-License-Identifier: Apache-2.0

//! B016 end-to-end slice: a real producer process hands a message with an
//! explicit correlation id to a real consumer process, the trace is produced by
//! that execution, and only the correlation id becomes an observed edge. Also
//! records a cancelled child-process scenario for AT037 evidence.

use archmeld_core::identity::line_entity_id;
use archmeld_core::runtime::{import_trace, timeline, RuntimeImport};
use archmeld_core::scenario::{
    run_scenario, run_scenario_with_cancel, ExecutionGrant, ScenarioSpec,
};
use archmeld_core::snapshot::{capture_snapshot, SnapshotInputs, SnapshotOptions};
use archmeld_core::store::Store;
use serde_json::{json, Value};
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

const P: &str = "project.b016";

fn fixture_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/causal")
}

fn copy_fixtures(root: &Path) {
    fs::create_dir_all(root).unwrap();
    for name in [
        "producer.py",
        "consumer_worker.py",
        "helper.py",
    ] {
        fs::copy(fixture_dir().join(name), root.join(name)).unwrap();
    }
}

fn find_line(text: &str, needle: &str) -> u64 {
    text.lines()
        .position(|line| line.contains(needle))
        .map(|index| index as u64 + 1)
        .unwrap_or(1)
}

fn entity_json(
    project: &str,
    snapshot_id: &str,
    config_digest: &str,
    path: &str,
    blob_digest: &str,
    name: &str,
    line: u64,
) -> Value {
    json!({
        "id": line_entity_id(blob_digest, "symbol", line, line, &format!("{path}::symbol::{name}")).unwrap(),
        "project_id": project,
        "snapshot_id": snapshot_id,
        "config_digest": config_digest,
        "kind": "symbol",
        "language": "python",
        "qualified_name": format!("{path}::{name}"),
        "anchors": [{
            "path": path,
            "blob_digest": blob_digest,
            "precision": "line_only",
            "byte_start": null,
            "byte_end": null,
            "line_start": line,
            "line_end": line,
            "encoding": "utf-8",
        }],
        "external": false,
        "backend_refs": [],
    })
}

#[test]
fn real_async_handoff_creates_only_the_correlated_observed_edge() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().join("repo");
    copy_fixtures(&root);
    let mut store = Store::open(dir.path().join("state")).unwrap();
    store.ensure_project(P, "expedition").unwrap();
    let snap = capture_snapshot(
        &mut store,
        P,
        &root,
        &SnapshotInputs {
            build_config: json!({"variant": "b016"}),
            environment: json!({"os": "test"}),
        },
        &SnapshotOptions {
            include_untracked: true,
            ..SnapshotOptions::default()
        },
    )
    .unwrap();
    let snapshot_id = snap.snapshot_id.clone();
    let config_digest = snap.snapshot["config_digest"]
        .as_str()
        .unwrap()
        .to_string();
    let file = |name: &str| {
        snap.files
            .iter()
            .find(|entry| entry.path == name)
            .unwrap_or_else(|| panic!("snapshot is missing {name}"))
    };
    let producer_text = fs::read_to_string(root.join("producer.py")).unwrap();
    let consumer_text = fs::read_to_string(root.join("consumer_worker.py")).unwrap();
    let helper_text = fs::read_to_string(root.join("helper.py")).unwrap();
    let producer = entity_json(
        P,
        &snapshot_id,
        &config_digest,
        "producer.py",
        &file("producer.py").blob_digest,
        "produce",
        find_line(&producer_text, "def produce"),
    );
    let consumer = entity_json(
        P,
        &snapshot_id,
        &config_digest,
        "consumer_worker.py",
        &file("consumer_worker.py").blob_digest,
        "consume",
        find_line(&consumer_text, "payload = json.loads"),
    );
    let helper = entity_json(
        P,
        &snapshot_id,
        &config_digest,
        "helper.py",
        &file("helper.py").blob_digest,
        "helper",
        find_line(&helper_text, "def helper"),
    );
    let producer_id = producer["id"].as_str().unwrap().to_string();
    let consumer_id = consumer["id"].as_str().unwrap().to_string();
    let helper_id = helper["id"].as_str().unwrap().to_string();
    let evidence_id = "evidence.syntax.b016";
    let batch = json!({
        "schema_version": "1.0",
        "project_id": P,
        "snapshot_id": snapshot_id,
        "config_digest": config_digest,
        "run_id": "run.b016.syntax",
        "adapter_id": "fixture.syntax",
        "adapter_version": "1",
        "sequence": "0",
        "final": true,
        "coverage": {
            "state": "complete",
            "scope": "b016 fixture entities",
            "expected_count": 3,
            "observed_count": 3,
            "omitted_count": 0,
            "unresolved_count": 0,
            "limitations": [],
        },
        "entities": [producer, consumer, helper],
        "evidence": [{
            "id": evidence_id,
            "project_id": P,
            "snapshot_id": snapshot_id,
            "config_digest": config_digest,
            "kind": "syntax",
            "producer": {"name": "fixture.syntax", "version": "1", "source_ref": "line-anchor", "run_id": "run.b016.syntax"},
            "artifact_digest": file("producer.py").blob_digest,
            "locator": "producer.py",
            "source_anchor": null,
            "scenario_id": null,
            "runtime_run_id": null,
            "coverage": {
                "state": "complete",
                "scope": "b016 fixture evidence",
                "expected_count": 3,
                "observed_count": 3,
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

    let run_dir = dir.path().join("run");
    fs::create_dir_all(&run_dir).unwrap();
    let trace_path = run_dir.join("trace.json");
    let producer_script = root.join("producer.py");
    let worker_script = root.join("consumer_worker.py");
    let scenario = ScenarioSpec {
        project_id: P.into(),
        run_id: "run.b016.async".into(),
        snapshot_id: snapshot_id.clone(),
        config_digest: config_digest.clone(),
        argv: vec![
            "python".into(),
            producer_script.to_string_lossy().into_owned(),
            trace_path.to_string_lossy().into_owned(),
            worker_script.to_string_lossy().into_owned(),
        ],
        cwd: run_dir.clone(),
        env: vec![],
        timeout: Duration::from_secs(30),
        sandbox: None,
    };
    let manifest = run_scenario(
        &mut store,
        &scenario,
        &ExecutionGrant::grant("b016-async", "user requested real scenario execution"),
    )
    .unwrap();
    assert_eq!(manifest["exit_code"], 0, "scenario failed: {manifest}");
    assert_eq!(manifest["completion"], "succeeded");
    assert_eq!(manifest["overhead"]["state"], "unknown");
    assert_eq!(manifest["process_audit"]["state"], "complete");

    let raw: Value = serde_json::from_str(&fs::read_to_string(&trace_path).unwrap()).unwrap();
    assert_eq!(raw["links"].as_array().unwrap().len(), 1);
    let trace = json!({
        "version": "1.0",
        "snapshot_id": snapshot_id,
        "config_digest": config_digest,
        "scenario_id": "scenario.b016.async",
        "build_identity": {"kind": "python", "variant": "b016"},
        "capture": {"tool": "python-pipeline-probe", "version": "1", "lossy": false, "loss_notes": []},
        "clock": {
            "kind": "monotonic",
            "unit": "ns",
            "origin_ns": "0",
            "domains": [
                {"clock_id": "clock-parent", "kind": "monotonic", "unit": "ns", "origin_ns": "0",
                 "calibrated": false, "uncertainty_ns": "500000000", "wall_clock": false},
                {"clock_id": "clock-child", "kind": "monotonic", "unit": "ns", "origin_ns": "0",
                 "calibrated": false, "uncertainty_ns": "500000000", "wall_clock": false}
            ]
        },
        "probes": [
            {"probe_id": "probe.producer", "entity_id": producer_id},
            {"probe_id": "probe.consumer", "entity_id": consumer_id},
            {"probe_id": "probe.helper", "entity_id": helper_id}
        ],
        "spans": raw["spans"],
        "links": raw["links"],
    });
    let report = import_trace(
        &mut store,
        &RuntimeImport {
            project_id: P,
            run_id: "run.b016.async",
            snapshot_id: Some(&snapshot_id),
            config_digest: &config_digest,
            allow_unbound: false,
            scenario_run_id: None,
        },
        &trace,
    )
    .unwrap();
    assert_eq!(report["bound"], true);
    assert_eq!(report["causal"]["state"], "complete", "{report}");
    assert_eq!(report["causal"]["mapped_spans"], 3);
    assert_eq!(report["causal"]["unmapped_spans"], 0);
    assert_eq!(report["causal"]["correlation_links"], 1);
    assert_eq!(report["clock"]["total_order"], false);
    assert_eq!(report["links"][0]["cross_clock"], true);
    assert_eq!(report["links"][0]["clock_uncertainty_ns"], "1000000000");

    let relations = store.relation_payloads(P, &snapshot_id).unwrap();
    let observed: Vec<&Value> = relations
        .iter()
        .filter(|relation| relation["basis"] == "runtime")
        .collect();
    assert_eq!(observed.len(), 1, "{relations:?}");
    let edge = observed[0];
    assert_eq!(edge["source_id"], producer_id);
    assert_eq!(edge["target_id"], consumer_id);
    assert_eq!(edge["kind"], "communicates_with");
    assert_eq!(edge["resolution"], "observed");
    assert!(edge["conditions"]
        .as_array()
        .unwrap()
        .iter()
        .any(|condition| condition == "correlation-id:order-42"));
    assert!(edge["conditions"]
        .as_array()
        .unwrap()
        .iter()
        .any(|condition| condition == "cross-clock:explicit-link; no total order claimed"));
    assert!(!relations.iter().any(|relation| {
        relation["source_id"] == helper_id || relation["target_id"] == helper_id
    }), "temporally adjacent helper must not gain an edge");
    let evidence = store.evidence_payloads(P, &snapshot_id).unwrap();
    let runtime_evidence = evidence
        .iter()
        .find(|item| item["id"] == "evidence.runtime.run.b016.async")
        .expect("runtime evidence")
        .clone();

    let cancel = AtomicBool::new(false);
    let cancel_spec = ScenarioSpec {
        project_id: P.into(),
        run_id: "run.b016.cancel".into(),
        snapshot_id: snapshot_id.clone(),
        config_digest: config_digest.clone(),
        argv: vec![
            "python".into(),
            "-c".into(),
            "import subprocess,sys,time; subprocess.Popen([sys.executable,'-c','import time;time.sleep(60)']); time.sleep(60)".into(),
        ],
        cwd: run_dir.clone(),
        env: vec![],
        timeout: Duration::from_secs(30),
        sandbox: None,
    };
    let cancelled = std::thread::scope(|scope| {
        scope.spawn(|| {
            std::thread::sleep(Duration::from_millis(1200));
            cancel.store(true, Ordering::SeqCst);
        });
        run_scenario_with_cancel(
            &mut store,
            &cancel_spec,
            &ExecutionGrant::grant("b016-cancel", "user requested cancellation audit"),
            Some(&cancel),
        )
        .unwrap()
    });
    assert_eq!(cancelled["cancelled"], true);
    assert_eq!(cancelled["completion"], "cancelled");
    assert_eq!(cancelled["process_audit"]["state"], "complete");
    assert!(cancelled["process_audit"]["observed"].as_u64().unwrap() >= 1);
    assert_eq!(
        cancelled["process_audit"]["survivors"]
            .as_array()
            .unwrap()
            .len(),
        0
    );
    assert!(cancelled["process_audit"]["cleanup"]
        .as_array()
        .unwrap()
        .iter()
        .any(|item| item["result"] == "terminated"));

    let workspace = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    let evidence_dir = workspace.join("docs/evidence");
    let evidence_file = json!({
        "generated_by": "crates/archmeld-core/tests/causal_scenario.rs",
        "project_id": P,
        "snapshot": {"snapshot_id": snapshot_id, "config_digest": config_digest},
        "acceptance": {
            "AT036": {
                "status": "passed",
                "real_execution": "two OS processes exchanged one message over stdin/stdout",
                "correlation_id": "order-42",
                "observed_relation": edge,
                "time_adjacent_without_id": "helper span has no relation",
                "causal_report": report["causal"],
            },
            "AT037": {
                "status": "passed",
                "normal_manifest": manifest,
                "cancel_manifest": cancelled,
                "overhead": "unknown (scenario runner does not instrument the target)"
            },
            "AT074": {
                "status": "passed",
                "clock": report["clock"],
                "link": report["links"][0],
                "no_total_order": report["clock"]["total_order"] == false,
                "uncertainty_basis": "declared bounds; not measured against a shared clock"
            }
        },
        "runtime_evidence": runtime_evidence,
        "timeline_window": timeline(&store, P, "run.b016.async", None, None, 100).unwrap()["coverage"].clone(),
    });
    if std::env::var("ARCHMELD_WRITE_EVIDENCE").ok().as_deref() == Some("1") {
        fs::create_dir_all(&evidence_dir).unwrap();
        fs::write(
            evidence_dir.join("B016-causal-async.json"),
            serde_json::to_string_pretty(&evidence_file).unwrap() + "\n",
        )
        .unwrap();
    }
    assert!(store.integrity_check().unwrap().is_empty());
}
