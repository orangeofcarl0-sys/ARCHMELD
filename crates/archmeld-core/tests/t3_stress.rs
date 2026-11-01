// Copyright 2026 ARCHMELD contributors
// SPDX-License-Identifier: Apache-2.0

//! B026 T3 stress: a one-million-event normalized trace and an incremental
//! evidence storm. Both are synthetic stress data (S16 allows this for resource
//! budgets); business correctness is covered by the real-project suites.
//!
//! The raw trace is written to disk before import, so a failed budget run keeps
//! its input. Budgets are wall-clock and size ceilings, not claims about a
//! reference machine.

use archmeld_core::hash;
use archmeld_core::identity::line_entity_id;
use archmeld_core::runtime::{import_trace, runtime_summary, RuntimeImport};
use archmeld_core::snapshot::{capture_snapshot, SnapshotInputs, SnapshotOptions};
use archmeld_core::store::Store;
use serde_json::{json, Value};
use std::fmt::Write as _;
use std::fs;
use std::path::Path;
use std::time::{Duration, Instant};

const PROJECT: &str = "project.t3";
const SPANS: usize = 100_000;
const COUNTERS: usize = 9;
const SAMPLES_PER_COUNTER: usize = 100_000;
const EVENTS: usize = SPANS + COUNTERS * SAMPLES_PER_COUNTER;
const IMPORT_BUDGET: Duration = Duration::from_secs(300);
const ARTIFACT_BUDGET: u64 = 200_000_000;
const DB_BUDGET: u64 = 2_000_000_000;
const STORM_BATCHES: usize = 200;
const STORM_ENTITIES_PER_BATCH: usize = 100;
const STORM_BUDGET: Duration = Duration::from_secs(180);

struct Fixture {
    _dir: tempfile::TempDir,
    store: Store,
    snapshot_id: String,
    config_digest: String,
    file_path: String,
    file_digest: String,
}

fn setup() -> Fixture {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().join("repo");
    fs::create_dir_all(&root).unwrap();
    fs::write(root.join("app.py"), "def work():\n    return 1\n").unwrap();
    let mut store = Store::open(dir.path().join("state")).unwrap();
    store.ensure_project(PROJECT, "expedition").unwrap();
    let snapshot = capture_snapshot(
        &mut store,
        PROJECT,
        &root,
        &SnapshotInputs {
            build_config: json!({"variant": "t3"}),
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
        store,
        snapshot_id: snapshot.snapshot_id,
        config_digest,
        file_path: snapshot.files[0].path.clone(),
        file_digest: snapshot.files[0].blob_digest.clone(),
    }
}

fn build_trace(snapshot_id: &str, config_digest: &str) -> String {
    let mut trace = String::with_capacity(100 * 1024 * 1024);
    trace.push_str(&format!(
        "{{\"version\":\"1.0\",\"snapshot_id\":\"{snapshot_id}\",\"config_digest\":\"{config_digest}\",\
         \"build_identity\":{{\"kind\":\"stress\",\"variant\":\"t3\"}},\
         \"capture\":{{\"tool\":\"t3-stress\",\"version\":\"1\",\"lossy\":false,\"loss_notes\":[]}},\
         \"clock\":{{\"kind\":\"monotonic\",\"unit\":\"ns\",\"origin_ns\":\"0\"}},\
         \"probes\":[],\"links\":[],\"spans\":["
    ));
    for index in 0..SPANS {
        if index > 0 {
            trace.push(',');
        }
        let start = index as u64 * 1000;
        let _ = write!(
            trace,
            "{{\"span_id\":\"t3.span.{index}\",\"parent_span_id\":null,\"name\":\"span.{}\",\
             \"kind\":\"function\",\"start_ns\":\"{start}\",\"end_ns\":\"{}\",\
             \"process_id\":\"p\",\"thread_id\":\"{}\",\"attributes\":{{}}}}",
            index % 100,
            start + 1,
            index % 8
        );
    }
    trace.push_str("],\"counters\":[");
    for counter in 0..COUNTERS {
        if counter > 0 {
            trace.push(',');
        }
        let _ = write!(
            trace,
            "{{\"counter_id\":\"t3.counter.{counter}\",\"name\":\"queue.{counter}\",\
             \"unit\":\"items\",\"clock_id\":\"default\",\"attributes\":{{\"stress\":true}},\
             \"samples\":["
        );
        for sample in 0..SAMPLES_PER_COUNTER {
            if sample > 0 {
                trace.push(',');
            }
            let timestamp = (sample as u64) * 1000;
            let _ = write!(
                trace,
                "{{\"timestamp_ns\":\"{timestamp}\",\"value\":{},\"process_id\":\"p\",\"thread_id\":\"0\"}}",
                sample % 17
            );
        }
        trace.push_str("]}");
    }
    trace.push_str("],\"states\":[]}");
    trace
}

fn write_evidence(payload: &Value) {
    let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../docs/release-evidence/B026-t3.json");
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(path, serde_json::to_string_pretty(payload).unwrap() + "\n").unwrap();
}

#[test]
fn million_event_trace_import_stays_within_budget() {
    let mut fixture = setup();
    let trace_text = build_trace(&fixture.snapshot_id, &fixture.config_digest);
    assert_eq!(EVENTS, 1_000_000);
    let artifact_dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../target/b026-t3");
    fs::create_dir_all(&artifact_dir).unwrap();
    let raw_path = artifact_dir.join("t3-trace.json");
    // Written before import: a failed run keeps its input.
    fs::write(&raw_path, trace_text.as_bytes()).unwrap();
    let trace_hash = hash::digest_bytes(trace_text.as_bytes());
    let trace_size = trace_text.len() as u64;
    let parsed: Value = serde_json::from_str(&trace_text).unwrap();

    let started = Instant::now();
    let timeline = import_trace(
        &mut fixture.store,
        &RuntimeImport {
            project_id: PROJECT,
            run_id: "run.t3.trace",
            snapshot_id: Some(&fixture.snapshot_id),
            config_digest: &fixture.config_digest,
            allow_unbound: false,
            scenario_run_id: None,
        },
        &parsed,
    )
    .unwrap();
    let elapsed = started.elapsed();
    let summary = runtime_summary(&fixture.store, PROJECT, "run.t3.trace").unwrap();
    assert_eq!(summary["span_count"], json!(SPANS));
    assert_eq!(summary["metric_count"], json!(COUNTERS));
    assert_eq!(summary["sample_count"], json!(COUNTERS * SAMPLES_PER_COUNTER));
    assert_eq!(summary["event_count"], json!(EVENTS));
    assert_eq!(timeline["truncated"], json!(true), "timeline stays bounded");
    assert!(elapsed <= IMPORT_BUDGET, "import took {elapsed:?}");
    let artifact_size = summary["artifact_size"].as_u64().unwrap();
    assert!(artifact_size > 50_000_000, "the trace artifact is substantial");
    assert!(artifact_size <= ARTIFACT_BUDGET, "artifact is {artifact_size} bytes");
    let db_size = fs::metadata(fixture._dir.path().join("state/state.sqlite3"))
        .unwrap()
        .len();
    assert!(db_size <= DB_BUDGET, "database is {db_size} bytes");
    assert!(fixture.store.integrity_check().unwrap().is_empty());
    // A bounded timeline query must not fail or expand the series count.
    let bounded = archmeld_core::runtime::timeline(&fixture.store, PROJECT, "run.t3.trace", None, None, 2).unwrap();
    assert_eq!(bounded["counters"].as_array().unwrap().len(), 2);
    assert!(bounded["metrics_truncated"].as_bool().unwrap());

    if std::env::var("ARCHMELD_WRITE_EVIDENCE").ok().as_deref() == Some("1") {
        write_evidence(&json!({
            "schema_version": "1.0",
            "generated_by": "crates/archmeld-core/tests/t3_stress.rs",
            "platform": {"os": std::env::consts::OS, "arch": std::env::consts::ARCH},
            "trace": {
                "path": "target/b026-t3/t3-trace.json",
                "sha256": trace_hash,
                "size": trace_size,
                "events": EVENTS,
                "spans": SPANS,
                "counters": COUNTERS,
                "samples": COUNTERS * SAMPLES_PER_COUNTER,
            },
            "import": {
                "elapsed_ms": elapsed.as_millis(),
                "budget_ms": IMPORT_BUDGET.as_millis(),
                "artifact_size": artifact_size,
                "artifact_budget": ARTIFACT_BUDGET,
                "db_size": db_size,
                "db_budget": DB_BUDGET,
            },
            "summary": summary,
            "bounded_timeline_series": 2,
            "limitations": [
                "synthetic stress data per S16; business correctness is covered by real project fixtures",
                "counters are process-scope, so this trace creates no observed relations",
                "budgets are local wall-clock/size ceilings, not cross-machine latency guarantees",
            ],
        }));
    }
}

#[test]
fn incremental_evidence_storm_stays_within_budget() {
    let mut fixture = setup();
    let file_path = fixture.file_path.clone();
    let file_digest = fixture.file_digest.clone();
    let started = Instant::now();
    for batch_index in 0..STORM_BATCHES {
        let mut entities = Vec::with_capacity(STORM_ENTITIES_PER_BATCH);
        for entity_index in 0..STORM_ENTITIES_PER_BATCH {
            let ordinal = batch_index * STORM_ENTITIES_PER_BATCH + entity_index;
            let id = line_entity_id(
                &file_digest,
                "symbol",
                1,
                1,
                &format!("storm::{ordinal}"),
            )
            .unwrap();
            entities.push(json!({
                "id": id,
                "project_id": PROJECT,
                "snapshot_id": fixture.snapshot_id,
                "config_digest": fixture.config_digest,
                "kind": "symbol",
                "language": "python",
                "qualified_name": format!("app.py::storm_{ordinal}"),
                "anchors": [{
                    "path": file_path,
                    "blob_digest": file_digest,
                    "precision": "line_only",
                    "byte_start": null,
                    "byte_end": null,
                    "line_start": 1,
                    "line_end": 1,
                    "encoding": "utf-8",
                }],
                "external": false,
                "backend_refs": [],
            }));
        }
        let batch = json!({
            "schema_version": "1.0",
            "project_id": PROJECT,
            "snapshot_id": fixture.snapshot_id,
            "config_digest": fixture.config_digest,
            "run_id": format!("run.storm.{batch_index}"),
            "adapter_id": "storm",
            "adapter_version": "1",
            "sequence": "0",
            "final": true,
            "coverage": {
                "state": "complete",
                "scope": "storm batch",
                "expected_count": STORM_ENTITIES_PER_BATCH,
                "observed_count": STORM_ENTITIES_PER_BATCH,
                "omitted_count": 0,
                "unresolved_count": 0,
                "limitations": [],
            },
            "entities": entities,
            "evidence": [{
                "id": format!("evidence.storm.{batch_index}"),
                "project_id": PROJECT,
                "snapshot_id": fixture.snapshot_id,
                "config_digest": fixture.config_digest,
                "kind": "syntax",
                "producer": {"name": "storm", "version": "1", "source_ref": "storm", "run_id": format!("run.storm.{batch_index}")},
                "artifact_digest": file_digest,
                "locator": "storm",
                "source_anchor": null,
                "scenario_id": null,
                "runtime_run_id": null,
                "coverage": {
                    "state": "complete",
                    "scope": "storm evidence",
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
        fixture.store.import_batch(&batch).unwrap();
    }
    let elapsed = started.elapsed();
    let ids = fixture
        .store
        .entity_ids(PROJECT, &fixture.snapshot_id)
        .unwrap();
    assert_eq!(ids.len(), STORM_BATCHES * STORM_ENTITIES_PER_BATCH);
    assert!(elapsed <= STORM_BUDGET, "storm took {elapsed:?}");
    assert!(fixture.store.integrity_check().unwrap().is_empty());
    if std::env::var("ARCHMELD_WRITE_EVIDENCE").ok().as_deref() == Some("1") {
        let path = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../docs/release-evidence/B026-t3-storm.json");
        fs::write(
            path,
            serde_json::to_string_pretty(&json!({
                "schema_version": "1.0",
                "generated_by": "crates/archmeld-core/tests/t3_stress.rs",
                "batches": STORM_BATCHES,
                "entities": STORM_BATCHES * STORM_ENTITIES_PER_BATCH,
                "elapsed_ms": elapsed.as_millis(),
                "budget_ms": STORM_BUDGET.as_millis(),
                "integrity": "ok",
                "limitations": ["synthetic incremental batches; no filesystem changes in this test"],
            }))
            .unwrap()
                + "\n",
        )
        .unwrap();
    }
}
