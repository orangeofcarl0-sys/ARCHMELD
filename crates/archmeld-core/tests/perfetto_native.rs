// Copyright 2026 ARCHMELD contributors
// SPDX-License-Identifier: Apache-2.0

//! B016 native state/counter capture: Perfetto protobuf, Chrome counter tracks
//! and explicit probe events. Counters and states are observations only; they
//! never create spans, causal edges or observed calls.

use archmeld_core::perfetto::perfetto_trace;
use archmeld_core::runtime::{import_trace, RuntimeImport};
use archmeld_core::snapshot::{capture_snapshot, SnapshotInputs, SnapshotOptions};
use archmeld_core::store::Store;
use archmeld_core::trace_adapters::{chrome_trace, probe_jsonl, TraceLocator, TraceMeta};
use serde_json::{json, Value};
use std::fs;
use std::path::Path;

const PROJECT: &str = "project.metrics";

struct Fixture {
    _dir: tempfile::TempDir,
    store: Store,
    snapshot_id: String,
    config_digest: String,
}

fn anchor(path: &str, digest: &str) -> Value {
    json!({
        "path": path,
        "blob_digest": digest,
        "precision": "line_only",
        "byte_start": null,
        "byte_end": null,
        "line_start": 1,
        "line_end": 1,
        "encoding": "utf-8",
    })
}

fn setup() -> Fixture {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().join("repo");
    fs::create_dir_all(root.join("src")).unwrap();
    fs::write(root.join("src/app.py"), "class Queue:\n    pass\n").unwrap();
    let mut store = Store::open(dir.path().join("state")).unwrap();
    store.ensure_project(PROJECT, "expedition").unwrap();
    let snapshot = capture_snapshot(
        &mut store,
        PROJECT,
        &root,
        &SnapshotInputs {
            build_config: json!({"variant": "metrics-test"}),
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
    let file = &snapshot.files[0];
    let artifact = file.blob_digest.clone();
    let batch = json!({
        "schema_version": "1.0",
        "project_id": PROJECT,
        "snapshot_id": snapshot.snapshot_id,
        "config_digest": config_digest,
        "run_id": "run.metrics.1",
        "adapter_id": "fixture",
        "adapter_version": "1",
        "sequence": "0",
        "final": true,
        "coverage": {
            "state": "complete",
            "scope": "fixture entities",
            "expected_count": 1,
            "observed_count": 1,
            "omitted_count": 0,
            "unresolved_count": 0,
            "limitations": [],
        },
        "entities": [{
            "id": "entity.queue",
            "project_id": PROJECT,
            "snapshot_id": snapshot.snapshot_id,
            "config_digest": config_digest,
            "kind": "symbol",
            "language": "python",
            "qualified_name": "src/app.py::Queue",
            "anchors": [anchor(&file.path, &file.blob_digest)],
            "external": false,
            "backend_refs": [],
        }],
        "evidence": [{
            "id": "evidence.syntax.metrics",
            "project_id": PROJECT,
            "snapshot_id": snapshot.snapshot_id,
            "config_digest": config_digest,
            "kind": "syntax",
            "producer": {"name": "fixture", "version": "1", "source_ref": "fixture", "run_id": "run.metrics.1"},
            "artifact_digest": artifact,
            "locator": "fixture",
            "source_anchor": null,
            "scenario_id": null,
            "runtime_run_id": null,
            "coverage": {
                "state": "complete",
                "scope": "fixture evidence",
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
    Fixture {
        _dir: dir,
        store,
        snapshot_id: snapshot.snapshot_id,
        config_digest,
    }
}

fn meta(fixture: &Fixture, tool: &str) -> TraceMeta {
    TraceMeta::new(
        &fixture.snapshot_id,
        &fixture.config_digest,
        json!({"kind": "python", "variant": "metrics-test"}),
        tool,
        "1",
    )
}

fn import(fixture: &mut Fixture, run_id: &str, trace: &Value) -> Value {
    import_trace(
        &mut fixture.store,
        &RuntimeImport {
            project_id: PROJECT,
            run_id,
            snapshot_id: Some(&fixture.snapshot_id),
            config_digest: &fixture.config_digest,
            allow_unbound: false,
            scenario_run_id: None,
        },
        trace,
    )
    .unwrap()
}

fn relation_count(store: &Store, snapshot_id: &str) -> usize {
    store.relation_payloads(PROJECT, snapshot_id).unwrap().len()
}

#[test]
fn perfetto_native_decodes_slices_counters_and_states() {
    let fixture = setup();
    let path = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../adapters/traces/fixtures/perfetto-counter-state.pftrace");
    let bytes = fs::read(&path).unwrap();
    assert!(
        bytes.len() > 500,
        "fixture must be a real Perfetto trace, got {} bytes",
        bytes.len()
    );
    let locator = TraceLocator::new();
    let trace = perfetto_trace(&bytes, &locator, &meta(&fixture, "perfetto-sdk")).unwrap();

    let spans = trace["spans"].as_array().unwrap();
    assert_eq!(spans.len(), 2, "two real slices");
    let names: Vec<&str> = spans
        .iter()
        .filter_map(|span| span["name"].as_str())
        .collect();
    assert!(names.contains(&"stage.queue"));
    assert!(names.contains(&"stage.compute"));
    let queue = spans
        .iter()
        .find(|span| span["name"] == json!("stage.queue"))
        .unwrap();
    let compute = spans
        .iter()
        .find(|span| span["name"] == json!("stage.compute"))
        .unwrap();
    let queue_start: i128 = queue["start_ns"].as_str().unwrap().parse().unwrap();
    let queue_end: i128 = queue["end_ns"].as_str().unwrap().parse().unwrap();
    let compute_start: i128 = compute["start_ns"].as_str().unwrap().parse().unwrap();
    let compute_end: i128 = compute["end_ns"].as_str().unwrap().parse().unwrap();
    assert!(queue_start <= compute_start && compute_end <= queue_end);
    assert!(queue_end - queue_start > 50_000_000, "captured ~105ms slice");
    assert!(
        queue["attributes"]["archmeld.entity_id"] == json!("entity.queue"),
        "explicit entity annotation is preserved"
    );

    let counters = trace["counters"].as_array().unwrap();
    assert_eq!(counters.len(), 1);
    assert_eq!(counters[0]["name"], json!("queue_depth"));
    let samples = counters[0]["samples"].as_array().unwrap();
    assert_eq!(samples.len(), 4, "all four real counter samples");
    let values: Vec<i64> = samples
        .iter()
        .map(|sample| sample["value"].as_i64().unwrap())
        .collect();
    assert_eq!(values, vec![0, 3, 1, 0]);

    let states = trace["states"].as_array().unwrap();
    assert_eq!(states.len(), 1);
    assert_eq!(states[0]["name"], json!("queue.state"));
    let intervals = states[0]["intervals"].as_array().unwrap();
    assert_eq!(intervals.len(), 2, "closed idle/running intervals");
    assert_eq!(intervals[0]["value"], json!("idle"));
    assert_eq!(intervals[1]["value"], json!("running"));
    let first_end: i128 = intervals[0]["end_ns"].as_str().unwrap().parse().unwrap();
    let second_start: i128 = intervals[1]["start_ns"].as_str().unwrap().parse().unwrap();
    assert_eq!(first_end, second_start);
    assert!(trace["capture"]["lossy"].as_bool().unwrap());
    let notes = trace["capture"]["loss_notes"].as_array().unwrap();
    assert!(
        notes
            .iter()
            .any(|note| note.as_str().unwrap().contains("open final interval")),
        "the final open state is reported, not fabricated"
    );
}

#[test]
fn counters_and_states_are_imported_without_relations() {
    let mut fixture = setup();
    let path = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../adapters/traces/fixtures/perfetto-counter-state.pftrace");
    let bytes = fs::read(path).unwrap();
    let trace = perfetto_trace(&bytes, &TraceLocator::new(), &meta(&fixture, "perfetto-sdk")).unwrap();
    let before = relation_count(&fixture.store, &fixture.snapshot_id);
    let timeline = import(&mut fixture, "run.metrics.perfetto", &trace);
    let after = relation_count(&fixture.store, &fixture.snapshot_id);
    assert_eq!(before, after, "metrics never create relations");

    assert_eq!(timeline["metric_count"], json!(2));
    assert_eq!(timeline["span_count"], json!(2));
    assert_eq!(timeline["counters"][0]["sample_count"], json!(4));
    assert_eq!(timeline["states"][0]["interval_count"], json!(2));
    assert_eq!(timeline["counters"][0]["samples"][1]["value"], json!(3));
    assert_eq!(timeline["states"][0]["intervals"][0]["value"], json!("idle"));
    assert_eq!(timeline["causal"]["metrics"]["counters"], json!(1));
    assert_eq!(timeline["causal"]["metrics"]["states"], json!(1));
    assert_eq!(timeline["causal"]["metrics"]["causality"], json!("none: counters and states never create causal edges"));
    assert_eq!(timeline["causal"]["mapped_spans"], json!(1));
    assert_eq!(timeline["causal"]["unmapped_spans"], json!(1));
    assert_eq!(timeline["causal"]["metrics"]["process_scope"], json!(2));
    assert_eq!(timeline["causal"]["state"], json!("partial"));
    let evidence = fixture
        .store
        .evidence_payloads(PROJECT, &fixture.snapshot_id)
        .unwrap()
        .into_iter()
        .find(|payload| payload["id"] == json!("evidence.runtime.run.metrics.perfetto"))
        .expect("runtime evidence exists");
    let coverage = &evidence["coverage"];
    assert_eq!(coverage["expected_count"], json!(4));
    assert_eq!(coverage["observed_count"], json!(4));
    assert_eq!(coverage["unresolved_count"], json!(1));
    assert_eq!(coverage["state"], json!("partial"));
    let observed = fixture
        .store
        .relation_payloads(PROJECT, &fixture.snapshot_id)
        .unwrap()
        .into_iter()
        .filter(|payload| {
            payload["kind"] == json!("observed") && payload["resolution"] == json!("observed")
        })
        .count();
    assert_eq!(observed, 0, "no observed call is inferred from slices");

    // Re-importing the identical artifact is idempotent.
    let again = import(&mut fixture, "run.metrics.perfetto", &trace);
    assert_eq!(again["metric_count"], json!(2));
}

#[test]
fn probe_events_capture_state_and_counters_with_explicit_mapping() {
    let mut fixture = setup();
    let jsonl = [
        json!({"span_id": "span.work", "name": "work", "start_ns": "100", "end_ns": "200", "entity_id": "entity.queue"}),
        json!({"event": "counter", "counter_id": "queue.depth", "name": "queue.depth", "unit": "items", "timestamp_ns": "120", "value": 2, "entity_id": "entity.queue"}),
        json!({"event": "counter", "counter_id": "queue.depth", "name": "queue.depth", "unit": "items", "timestamp_ns": "180", "value": 0, "entity_id": "entity.queue"}),
        json!({"event": "state", "state_id": "queue.state", "name": "queue.state", "start_ns": "100", "end_ns": "150", "value": "idle", "entity_id": "entity.queue"}),
        json!({"event": "state", "state_id": "queue.state", "name": "queue.state", "start_ns": "150", "end_ns": "200", "value": "running", "entity_id": "entity.queue"}),
    ]
    .iter()
    .map(|value| value.to_string())
    .collect::<Vec<_>>()
    .join("\n");
    let trace = probe_jsonl(&jsonl, &TraceLocator::new(), &meta(&fixture, "probe")).unwrap();
    let timeline = import(&mut fixture, "run.metrics.probe", &trace);
    assert_eq!(timeline["metric_count"], json!(2));
    assert_eq!(timeline["counters"][0]["entity_id"], json!("entity.queue"));
    assert_eq!(timeline["states"][0]["entity_id"], json!("entity.queue"));
    assert_eq!(timeline["causal"]["metrics"]["mapped"], json!(2));
    assert_eq!(timeline["causal"]["metrics"]["process_scope"], json!(0));
    assert_eq!(timeline["causal"]["state"], json!("complete"));
    assert_eq!(timeline["states"][0]["interval_count"], json!(2));
}

#[test]
fn probe_metric_validation_rejects_fabricated_values() {
    let fixture = setup();
    let meta = meta(&fixture, "probe");
    let bad_value = json!({"event": "counter", "counter_id": "q", "timestamp_ns": "1", "value": "three"}).to_string();
    assert!(probe_jsonl(&bad_value, &TraceLocator::new(), &meta).is_err());
    let negative_interval = json!({"event": "state", "state_id": "q", "start_ns": "20", "end_ns": "10", "value": "idle"}).to_string();
    assert!(probe_jsonl(&negative_interval, &TraceLocator::new(), &meta).is_err());
    let unknown_event = json!({"event": "sample", "value": 1}).to_string();
    assert!(probe_jsonl(&unknown_event, &TraceLocator::new(), &meta).is_err());
    let no_timestamp = json!({"event": "counter", "counter_id": "q", "value": 1}).to_string();
    assert!(probe_jsonl(&no_timestamp, &TraceLocator::new(), &meta).is_err());
}

#[test]
fn chrome_counter_tracks_are_series_not_spans() {
    let fixture = setup();
    let chrome = json!({
        "traceEvents": [
            {"ph": "X", "name": "work", "ts": 10.0, "dur": 5.0, "pid": 1, "tid": 2},
            {"ph": "C", "name": "queue_depth", "ts": 10.0, "pid": 1, "tid": 2, "args": {"value": 4}},
            {"ph": "C", "name": "queue_depth", "ts": 12.0, "pid": 1, "tid": 2, "args": {"value": 1}},
            {"ph": "C", "name": "rss", "ts": 12.0, "pid": 1, "tid": 2, "args": {"bytes": 1024}}
        ]
    });
    let trace = chrome_trace(&chrome, &TraceLocator::new(), &meta(&fixture, "chrome"), false).unwrap();
    assert_eq!(trace["spans"].as_array().unwrap().len(), 1);
    let counters = trace["counters"].as_array().unwrap();
    assert_eq!(counters.len(), 2);
    let depth = counters
        .iter()
        .find(|counter| counter["name"] == json!("queue_depth"))
        .unwrap();
    assert_eq!(depth["samples"].as_array().unwrap().len(), 2);
    assert_eq!(depth["samples"][0]["timestamp_ns"], json!("10000"));
    let rss = counters
        .iter()
        .find(|counter| counter["name"] == json!("rss"))
        .unwrap();
    assert_eq!(rss["samples"][0]["value"], json!(1024));
    let ambiguous = json!({
        "traceEvents": [
            {"ph": "C", "name": "bad", "ts": 1.0, "pid": 1, "tid": 1, "args": {"a": 1, "b": 2}}
        ]
    });
    let ambiguous = chrome_trace(&ambiguous, &TraceLocator::new(), &meta(&fixture, "chrome"), false).unwrap();
    assert!(ambiguous["counters"].as_array().unwrap().is_empty());
}
