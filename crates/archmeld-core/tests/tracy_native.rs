// Copyright 2026 ARCHMELD contributors
// SPDX-License-Identifier: Apache-2.0

//! B016 native Tracy retention: a real `.tracy` capture is kept in the CAS,
//! and the explicit `tracy-csvexport -u -p` summary supplies zones and plots.

use archmeld_core::runtime::RuntimeImport;
use archmeld_core::snapshot::{capture_snapshot, SnapshotInputs, SnapshotOptions};
use archmeld_core::store::Store;
use archmeld_core::trace_adapters::{TraceLocator, TraceMeta};
use archmeld_core::tracy_csv::{import_tracy_capture, tracy_csv};
use serde_json::{json, Value};
use std::fs;
use std::path::Path;

const PROJECT: &str = "project.tracy";

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
    fs::write(root.join("tracy_probe.cc"), "int main() { return 0; }\n").unwrap();
    let mut store = Store::open(dir.path().join("state")).unwrap();
    store.ensure_project(PROJECT, "expedition").unwrap();
    let snapshot = capture_snapshot(
        &mut store,
        PROJECT,
        &root,
        &SnapshotInputs {
            build_config: json!({"variant": "tracy-test"}),
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
    let entities: Vec<Value> = ["entity.queue", "entity.compute"]
        .iter()
        .map(|id| {
            json!({
                "id": id,
                "project_id": PROJECT,
                "snapshot_id": snapshot.snapshot_id,
                "config_digest": config_digest,
                "kind": "symbol",
                "language": "cpp",
                "qualified_name": format!("tracy_probe.cc::{id}"),
                "anchors": [anchor(&file.path, &file.blob_digest)],
                "external": false,
                "backend_refs": [],
            })
        })
        .collect();
    let batch = json!({
        "schema_version": "1.0",
        "project_id": PROJECT,
        "snapshot_id": snapshot.snapshot_id,
        "config_digest": config_digest,
        "run_id": "run.tracy.fixture",
        "adapter_id": "fixture",
        "adapter_version": "1",
        "sequence": "0",
        "final": true,
        "coverage": {
            "state": "complete",
            "scope": "fixture entities",
            "expected_count": 2,
            "observed_count": 2,
            "omitted_count": 0,
            "unresolved_count": 0,
            "limitations": [],
        },
        "entities": entities,
        "evidence": [{
            "id": "evidence.syntax.tracy",
            "project_id": PROJECT,
            "snapshot_id": snapshot.snapshot_id,
            "config_digest": config_digest,
            "kind": "syntax",
            "producer": {"name": "fixture", "version": "1", "source_ref": "fixture", "run_id": "run.tracy.fixture"},
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

fn meta(fixture: &Fixture) -> TraceMeta {
    TraceMeta::new(
        &fixture.snapshot_id,
        &fixture.config_digest,
        json!({"kind": "cpp", "variant": "tracy-test"}),
        "tracy",
        "0.13.1",
    )
}

fn fixtures() -> (Vec<u8>, String) {
    let base = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../adapters/traces/fixtures");
    (
        fs::read(base.join("tracy-zones-plots.tracy")).unwrap(),
        fs::read_to_string(base.join("tracy-zones-plots.csv")).unwrap(),
    )
}

#[test]
fn tracy_csv_maps_zones_and_plots() {
    let fixture = setup();
    let (_, csv) = fixtures();
    let mut locator = TraceLocator::new();
    locator.add_location("tracy_probe.cc", 14, "entity.queue");
    locator.add_location("tracy_probe.cc", 19, "entity.compute");
    let trace = tracy_csv(&csv, &locator, &meta(&fixture)).unwrap();
    let spans = trace["spans"].as_array().unwrap();
    assert_eq!(spans.len(), 2);
    let queue = spans
        .iter()
        .find(|span| span["name"] == json!("stage.queue"))
        .unwrap();
    let compute = spans
        .iter()
        .find(|span| span["name"] == json!("stage.compute"))
        .unwrap();
    assert_eq!(queue["start_ns"], json!("224661472"));
    assert_eq!(queue["end_ns"], json!("329839654"));
    assert_eq!(queue["attributes"]["archmeld.entity_id"], json!("entity.queue"));
    assert_eq!(compute["attributes"]["archmeld.entity_id"], json!("entity.compute"));
    let counters = trace["counters"].as_array().unwrap();
    assert_eq!(counters.len(), 1);
    let samples = counters[0]["samples"].as_array().unwrap();
    assert_eq!(samples.len(), 4);
    let values: Vec<f64> = samples
        .iter()
        .map(|sample| sample["value"].as_f64().unwrap())
        .collect();
    assert_eq!(values, vec![0.0, 3.0, 1.0, 0.0]);
    assert!(trace["capture"]["lossy"].as_bool().unwrap());
}

#[test]
fn tracy_capture_is_retained_and_imported() {
    let mut fixture = setup();
    let (capture, csv) = fixtures();
    let mut locator = TraceLocator::new();
    locator.add_location("tracy_probe.cc", 14, "entity.queue");
    locator.add_location("tracy_probe.cc", 19, "entity.compute");
    let meta = meta(&fixture);
    let timeline = import_tracy_capture(
        &mut fixture.store,
        &RuntimeImport {
            project_id: PROJECT,
            run_id: "run.tracy.native",
            snapshot_id: Some(&fixture.snapshot_id),
            config_digest: &fixture.config_digest,
            allow_unbound: false,
            scenario_run_id: None,
        },
        &capture,
        &csv,
        &locator,
        &meta,
    )
    .unwrap();
    assert_eq!(timeline["span_count"], json!(2));
    assert_eq!(timeline["metric_count"], json!(1));
    assert_eq!(timeline["counters"][0]["sample_count"], json!(4));
    assert_eq!(timeline["causal"]["mapped_spans"], json!(2));
    let run_artifact = fixture
        .store
        .read_blob_bytes(timeline["artifact_digest"].as_str().unwrap())
        .unwrap()
        .unwrap();
    let normalized: Value = serde_json::from_slice(&run_artifact).unwrap();
    let native_digest = normalized["capture"]["native_artifact"]["artifact_digest"]
        .as_str()
        .unwrap()
        .to_string();
    assert!(native_digest.starts_with("sha256:"));
    let stored = fixture
        .store
        .read_blob_bytes(&native_digest)
        .unwrap()
        .unwrap();
    assert_eq!(
        stored, capture,
        "the real .tracy artifact is retained byte for byte"
    );
    assert!(
        fixture
            .store
            .has_artifact_ref("runtime_native", "run.tracy.native", &native_digest)
            .unwrap(),
        "the native artifact is referenced from the ledger"
    );
    assert_eq!(
        fixture
            .store
            .relation_payloads(PROJECT, &fixture.snapshot_id)
            .unwrap()
            .len(),
        0
    );
}

#[test]
fn tracy_csv_rejects_declared_corruption() {
    let fixture = setup();
    let meta = meta(&fixture);
    let wrong_header = "name,file,line\nx,y,z\n";
    assert!(tracy_csv(wrong_header, &TraceLocator::new(), &meta).is_err());
    let bad_plot = "name,src_file,src_line,ns_since_start,exec_time_ns,thread,value\nq,,,10,,,not-a-number\n";
    assert!(tracy_csv(bad_plot, &TraceLocator::new(), &meta).is_err());
    let negative = "name,src_file,src_line,ns_since_start,exec_time_ns,thread,value\nz,f.cc,1,-5,10,1,\n";
    assert!(tracy_csv(negative, &TraceLocator::new(), &meta).is_err());
    let short = "name,src_file,src_line,ns_since_start,exec_time_ns,thread,value\nz,f.cc,1\n";
    assert!(tracy_csv(short, &TraceLocator::new(), &meta).is_err());
}
