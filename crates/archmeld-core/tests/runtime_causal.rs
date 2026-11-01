// Copyright 2026 ARCHMELD contributors
// SPDX-License-Identifier: Apache-2.0

//! B016 causal mapping: only explicit parent/flow/message identifiers create
//! observed edges; explicit entity/probe mapping is required; static candidates
//! are never promoted.

use archmeld_core::identity::line_entity_id;
use archmeld_core::runtime::{import_trace, timeline, RuntimeImport};
use archmeld_core::snapshot::{capture_snapshot, SnapshotInputs, SnapshotOptions};
use archmeld_core::store::Store;
use serde_json::{json, Value};
use std::fs;

const P: &str = "project.causal";

struct Fixture {
    _dir: tempfile::TempDir,
    store: Store,
    snapshot_id: String,
    config_digest: String,
    entities: Vec<String>,
}

fn line_anchor(path: &str, digest: &str, line: u64) -> Value {
    json!({
        "path": path,
        "blob_digest": digest,
        "precision": "line_only",
        "byte_start": null,
        "byte_end": null,
        "line_start": line,
        "line_end": line,
        "encoding": "utf-8",
    })
}

fn setup(files: &[(&str, &str)], names: &[&str]) -> Fixture {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().join("repo");
    for (path, text) in files {
        let full = root.join(path);
        fs::create_dir_all(full.parent().unwrap()).unwrap();
        fs::write(full, text).unwrap();
    }
    let mut store = Store::open(dir.path().join("state")).unwrap();
    store.ensure_project(P, "expedition").unwrap();
    let snap = capture_snapshot(
        &mut store,
        P,
        &root,
        &SnapshotInputs {
            build_config: json!({"variant": "causal-test"}),
            environment: json!({"os": "test"}),
        },
        &SnapshotOptions {
            include_untracked: true,
            ..SnapshotOptions::default()
        },
    )
    .unwrap();
    let config_digest = snap.snapshot["config_digest"]
        .as_str()
        .unwrap()
        .to_string();
    let mut entities = Vec::new();
    let mut batch_entities = Vec::new();
    for (index, name) in names.iter().enumerate() {
        let file = &snap.files[index % snap.files.len()];
        let (line_start, line_end) = (1, 1);
        let id = line_entity_id(
            &file.blob_digest,
            "symbol",
            line_start,
            line_end,
            &format!("{}::symbol::{name}", file.path),
        )
        .unwrap();
        batch_entities.push(json!({
            "id": id,
            "project_id": P,
            "snapshot_id": snap.snapshot_id,
            "config_digest": config_digest,
            "kind": "symbol",
            "language": "python",
            "qualified_name": format!("{}::{name}", file.path),
            "anchors": [line_anchor(&file.path, &file.blob_digest, line_start)],
            "external": false,
            "backend_refs": [],
        }));
        entities.push(id);
    }
    let evidence_id = "evidence.syntax.fixture";
    let artifact = snap.files[0].blob_digest.clone();
    let batch = json!({
        "schema_version": "1.0",
        "project_id": P,
        "snapshot_id": snap.snapshot_id,
        "config_digest": config_digest,
        "run_id": "run.fixture.1",
        "adapter_id": "fixture",
        "adapter_version": "1",
        "sequence": "0",
        "final": true,
        "coverage": {
            "state": "complete",
            "scope": "fixture entities",
            "expected_count": batch_entities.len(),
            "observed_count": batch_entities.len(),
            "omitted_count": 0,
            "unresolved_count": 0,
            "limitations": [],
        },
        "entities": batch_entities,
        "evidence": [{
            "id": evidence_id,
            "project_id": P,
            "snapshot_id": snap.snapshot_id,
            "config_digest": config_digest,
            "kind": "syntax",
            "producer": {"name": "fixture", "version": "1", "source_ref": "fixture", "run_id": "run.fixture.1"},
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
        "relations": [{
            "id": "rel.static.fixture",
            "project_id": P,
            "snapshot_id": snap.snapshot_id,
            "config_digest": config_digest,
            "source_id": entities[0],
            "target_id": entities[1],
            "kind": "static_call_candidate",
            "basis": "syntax",
            "evidence_ids": [evidence_id],
            "conditions": ["fixture static candidate"],
            "resolution": "candidate",
        }],
        "diagnostics": [],
    });
    store.import_batch(&batch).unwrap();
    Fixture {
        _dir: dir,
        store,
        snapshot_id: snap.snapshot_id,
        config_digest,
        entities,
    }
}

fn trace(
    snapshot_id: &str,
    config_digest: &str,
    spans: Value,
    links: Value,
    probes: Value,
    scenario: &str,
) -> Value {
    json!({
        "version": "1.0",
        "snapshot_id": snapshot_id,
        "config_digest": config_digest,
        "scenario_id": scenario,
        "build_identity": {"kind": "python", "variant": "causal-test"},
        "capture": {"tool": "in-process-probe", "version": "1", "lossy": false, "loss_notes": []},
        "clock": {"kind": "monotonic", "unit": "ns", "origin_ns": "0"},
        "probes": probes,
        "spans": spans,
        "links": links,
    })
}

fn relation(payloads: &[Value], source: &str, target: &str, kind: &str) -> Option<Value> {
    payloads
        .iter()
        .find(|item| {
            item["source_id"] == source
                && item["target_id"] == target
                && item["kind"] == kind
        })
        .cloned()
}

#[test]
fn causal_edges_require_explicit_links_and_never_promote_candidates() {
    let mut fixture = setup(
        &[
            ("src/producer.py", "def produce():\n    return 1\n"),
            ("src/consumer.py", "def consume():\n    return 2\n"),
            ("src/helper.py", "def helper():\n    return 3\n"),
        ],
        &["produce", "consume", "helper"],
    );
    let [a, b, c] = [
        fixture.entities[0].clone(),
        fixture.entities[1].clone(),
        fixture.entities[2].clone(),
    ];
    let spans = json!([
        {"span_id": "root", "parent_span_id": null, "name": "root", "kind": "function",
         "start_ns": "50", "end_ns": "500", "attributes": {"archmeld.entity_id": a}},
        {"span_id": "leaf", "parent_span_id": "root", "name": "leaf", "kind": "function",
         "start_ns": "100", "end_ns": "150", "attributes": {"archmeld.entity_id": c}},
        {"span_id": "prod", "parent_span_id": null, "name": "produce", "kind": "function",
         "start_ns": "160", "end_ns": "200", "attributes": {"archmeld.entity_id": a}},
        {"span_id": "helper", "parent_span_id": null, "name": "helper", "kind": "function",
         "start_ns": "201", "end_ns": "299", "attributes": {}},
        {"span_id": "cons", "parent_span_id": null, "name": "consume", "kind": "function",
         "start_ns": "300", "end_ns": "400", "attributes": {"archmeld.probe_id": "probe.consumer"}}
    ]);
    let links = json!([{
        "link_id": "link.message.1",
        "source_span_id": "prod",
        "target_span_id": "cons",
        "kind": "message",
        "correlation_id": "order-42",
        "attributes": {"queue": "orders"},
    }]);
    let probes = json!([{"probe_id": "probe.consumer", "entity_id": b}]);
    let report = import_trace(
        &mut fixture.store,
        &RuntimeImport {
            project_id: P,
            run_id: "run.causal.1",
            snapshot_id: Some(&fixture.snapshot_id),
            config_digest: &fixture.config_digest,
            allow_unbound: false,
            scenario_run_id: None,
        },
        &trace(&fixture.snapshot_id, &fixture.config_digest, spans, links, probes, "scenario.async.1"),
    )
    .unwrap();

    assert_eq!(report["causal"]["state"], "partial");
    assert_eq!(report["causal"]["mapped_spans"], 4);
    assert_eq!(report["causal"]["unmapped_spans"], 1);
    assert_eq!(report["causal"]["unknown_entity"], 0);
    assert_eq!(report["causal"]["correlation_links"], 1);
    assert_eq!(report["causal"]["link_count"], 1);
    assert_eq!(report["links"].as_array().unwrap().len(), 1);
    assert_eq!(report["links"][0]["strength"], "explicit-correlation-id");
    assert_eq!(report["links"][0]["cross_clock"], false);
    // The unlinked helper is visible as a missing association, never an edge.
    assert!(report["causal"]["missing_association"]
        .as_array()
        .unwrap()
        .iter()
        .any(|sample| sample["span_id"] == "helper" && sample["reason"] == "no_explicit_entity_or_probe"));

    let relations = fixture.store.relation_payloads(P, &fixture.snapshot_id).unwrap();
    let static_edge = relation(&relations, &a, &b, "static_call_candidate").expect("static candidate");
    assert_eq!(static_edge["resolution"], "candidate");
    assert_eq!(static_edge["basis"], "syntax");

    let async_edge = relation(&relations, &a, &b, "communicates_with").expect("async edge");
    assert_eq!(async_edge["resolution"], "observed");
    assert_eq!(async_edge["basis"], "runtime");
    assert_ne!(async_edge["id"], static_edge["id"]);
    assert!(async_edge["conditions"]
        .as_array()
        .unwrap()
        .iter()
        .any(|condition| condition == "correlation-id:order-42"));

    let call_edge = relation(&relations, &a, &c, "observed_call").expect("parent edge");
    assert_eq!(call_edge["basis"], "runtime");
    assert_eq!(call_edge["resolution"], "observed");

    // Temporal adjacency (helper between producer and consumer) created no edge.
    assert!(!relations.iter().any(|item| {
        item["source_id"] == c && item["kind"] == "observed_call"
            || item["target_id"] == c && item["kind"] == "communicates_with"
    }));

    let evidence = fixture.store.evidence_payloads(P, &fixture.snapshot_id).unwrap();
    let runtime_evidence = evidence
        .iter()
        .find(|item| item["id"] == "evidence.runtime.run.causal.1")
        .expect("runtime evidence");
    assert_eq!(runtime_evidence["kind"], "runtime");
    assert_eq!(runtime_evidence["runtime_run_id"], "run.causal.1");
    assert_eq!(runtime_evidence["scenario_id"], "scenario.async.1");
    assert_eq!(runtime_evidence["freshness"], "current");
    assert!(runtime_evidence["artifact_digest"]
        .as_str()
        .unwrap()
        .starts_with("sha256:"));
    assert!(fixture.store.integrity_check().unwrap().is_empty());

    // A message link without a correlation id is rejected instead of inferred.
    let no_correlation = json!([{
        "link_id": "link.bad",
        "source_span_id": "prod",
        "target_span_id": "cons",
        "kind": "message",
    }]);
    let spans = report["spans"].clone();
    let error = import_trace(
        &mut fixture.store,
        &RuntimeImport {
            project_id: P,
            run_id: "run.causal.bad",
            snapshot_id: Some(&fixture.snapshot_id),
            config_digest: &fixture.config_digest,
            allow_unbound: false,
            scenario_run_id: None,
        },
        &trace(&fixture.snapshot_id, &fixture.config_digest, spans, no_correlation, json!([]), "scenario.async.1"),
    );
    assert!(error.is_err());

    // Unknown entity mappings stay unmapped and create no relation.
    let unknown = format!("syn.{}", "0".repeat(64));
    let unknown_spans = json!([
        {"span_id": "u1", "parent_span_id": null, "name": "u1", "kind": "function",
         "start_ns": "10", "end_ns": "20", "attributes": {"archmeld.entity_id": unknown}},
        {"span_id": "u2", "parent_span_id": "u1", "name": "u2", "kind": "function",
         "start_ns": "11", "end_ns": "19", "attributes": {"archmeld.entity_id": a}}
    ]);
    let unknown_report = import_trace(
        &mut fixture.store,
        &RuntimeImport {
            project_id: P,
            run_id: "run.causal.unknown",
            snapshot_id: Some(&fixture.snapshot_id),
            config_digest: &fixture.config_digest,
            allow_unbound: false,
            scenario_run_id: None,
        },
        &trace(&fixture.snapshot_id, &fixture.config_digest, unknown_spans, json!([]), json!([]), "scenario.async.1"),
    )
    .unwrap();
    assert_eq!(unknown_report["causal"]["unknown_entity"], 1);
    assert_eq!(unknown_report["causal"]["mapped_spans"], 1);
    let after_unknown = fixture.store.relation_payloads(P, &fixture.snapshot_id).unwrap();
    assert_eq!(after_unknown.len(), relations.len());

    // Unbound traces can be browsed but never produce observed relations.
    let mut unbound = trace(
        &fixture.snapshot_id,
        &format!("sha256:{}", "a".repeat(64)),
        report["spans"].clone(),
        json!([]),
        json!([]),
        "scenario.async.1",
    );
    unbound["config_digest"] = json!(format!("sha256:{}", "a".repeat(64)));
    let unbound_report = import_trace(
        &mut fixture.store,
        &RuntimeImport {
            project_id: P,
            run_id: "run.causal.unbound",
            snapshot_id: Some(&fixture.snapshot_id),
            config_digest: &fixture.config_digest,
            allow_unbound: true,
            scenario_run_id: None,
        },
        &unbound,
    )
    .unwrap();
    assert_eq!(unbound_report["bound"], false);
    assert_eq!(unbound_report["causal"]["state"], "unbound");
    let after_unbound = fixture.store.relation_payloads(P, &fixture.snapshot_id).unwrap();
    assert_eq!(after_unbound.len(), relations.len());
    let evidence = fixture.store.evidence_payloads(P, &fixture.snapshot_id).unwrap();
    let unbound_evidence = evidence
        .iter()
        .find(|item| item["id"] == "evidence.runtime.run.causal.unbound")
        .expect("unbound evidence");
    assert_eq!(unbound_evidence["freshness"], "unbound");
}

#[test]
fn multiple_clock_domains_are_never_falsely_total_ordered() {
    let mut fixture = setup(
        &[
            ("src/a.py", "def a():\n    return 1\n"),
            ("src/b.py", "def b():\n    return 2\n"),
            ("src/c.py", "def c():\n    return 3\n"),
            ("src/d.py", "def d():\n    return 4\n"),
        ],
        &["a", "b", "c", "d"],
    );
    let [a, b, c, d] = [
        fixture.entities[0].clone(),
        fixture.entities[1].clone(),
        fixture.entities[2].clone(),
        fixture.entities[3].clone(),
    ];
    let spans = json!([
        {"span_id": "a1", "parent_span_id": null, "name": "a1", "kind": "function",
         "clock_id": "clock-a", "start_ns": "100", "end_ns": "150",
         "attributes": {"archmeld.entity_id": a}},
        {"span_id": "a2", "parent_span_id": null, "name": "a2", "kind": "function",
         "clock_id": "clock-a", "start_ns": "200", "end_ns": "260",
         "attributes": {"archmeld.entity_id": b}},
        {"span_id": "b1", "parent_span_id": null, "name": "b1", "kind": "function",
         "clock_id": "clock-b", "start_ns": "120", "end_ns": "180",
         "attributes": {"archmeld.entity_id": c}},
        {"span_id": "b2", "parent_span_id": null, "name": "b2", "kind": "function",
         "clock_id": "clock-b", "start_ns": "210", "end_ns": "270",
         "attributes": {"archmeld.entity_id": d}}
    ]);
    let links = json!([{
        "link_id": "link.flow.1",
        "source_span_id": "a1",
        "target_span_id": "b1",
        "kind": "flow",
        "correlation_id": "msg-7",
    }]);
    let mut trace = trace(
        &fixture.snapshot_id,
        &fixture.config_digest,
        spans,
        links,
        json!([]),
        "scenario.cross-clock",
    );
    trace["clock"] = json!({
        "kind": "monotonic",
        "unit": "ns",
        "origin_ns": "0",
        "domains": [
            {"clock_id": "clock-a", "kind": "monotonic", "unit": "ns", "origin_ns": "0",
             "calibrated": false, "uncertainty_ns": "1000", "wall_clock": false},
            {"clock_id": "clock-b", "kind": "monotonic", "unit": "ns", "origin_ns": "5000",
             "calibrated": false, "uncertainty_ns": "2000", "wall_clock": false}
        ]
    });
    let report = import_trace(
        &mut fixture.store,
        &RuntimeImport {
            project_id: P,
            run_id: "run.clocks.1",
            snapshot_id: Some(&fixture.snapshot_id),
            config_digest: &fixture.config_digest,
            allow_unbound: false,
            scenario_run_id: None,
        },
        &trace,
    )
    .unwrap();

    assert_eq!(report["clock"]["total_order"], false);
    assert!(report["clock"]["ordering"]
        .as_str()
        .unwrap()
        .contains("no cross-domain total order"));
    assert_eq!(report["clock"]["domains"].as_array().unwrap().len(), 2);
    assert_eq!(report["clock"]["domains"][1]["uncertainty_ns"], "2000");
    assert_eq!(report["spans"][0]["clock_id"], "clock-a");
    assert_eq!(report["spans"][1]["clock_id"], "clock-a");
    assert_eq!(report["spans"][2]["clock_id"], "clock-b");
    // Numeric starts of clock-b (120) are smaller than clock-a's second span
    // (200); the timeline must not interleave them to imply a global order.
    assert_eq!(report["spans"][0]["span_id"], "a1");
    assert_eq!(report["spans"][1]["span_id"], "a2");

    assert_eq!(report["links"][0]["cross_clock"], true);
    assert_eq!(report["links"][0]["clock_uncertainty_ns"], "3000");
    assert_eq!(report["links"][0]["strength"], "explicit-correlation-id");
    assert_eq!(report["causal"]["state"], "complete");
    assert_eq!(report["causal"]["unmapped_spans"], 0);

    let relations = fixture.store.relation_payloads(P, &fixture.snapshot_id).unwrap();
    let linked = relation(&relations, &a, &c, "communicates_with").expect("linked edge");
    assert_eq!(linked["basis"], "runtime");
    assert!(linked["conditions"]
        .as_array()
        .unwrap()
        .iter()
        .any(|condition| condition == "correlation-id:msg-7"));
    assert!(linked["conditions"]
        .as_array()
        .unwrap()
        .iter()
        .any(|condition| condition == "cross-clock:explicit-link; no total order claimed"));
    // a2 and b2 overlap in numeric time but have no explicit identifier.
    assert!(relation(&relations, &b, &d, "communicates_with").is_none());
    assert!(relation(&relations, &b, &d, "observed_call").is_none());
    assert_eq!(
        relations
            .iter()
            .filter(|item| item["basis"] == "runtime")
            .count(),
        1
    );

    let window = timeline(
        &fixture.store,
        P,
        "run.clocks.1",
        Some("150"),
        Some("220"),
        100,
    )
    .unwrap();
    assert_eq!(window["clock"]["total_order"], false);
    assert!(!window["spans"].as_array().unwrap().is_empty());
    assert!(fixture.store.integrity_check().unwrap().is_empty());
}

