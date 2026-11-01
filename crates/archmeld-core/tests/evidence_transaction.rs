// Copyright 2026 ARCHMELD contributors
// SPDX-License-Identifier: Apache-2.0

//! AT007 (atomic batch import), AT008 (reject fabricated facts), AT071
//! (cross-object semantic consistency) at the ledger boundary.

use archmeld_core::store::{
    Coverage, CoverageState, FaultPoint, ImportStatus, SnapshotInsert, Store,
};
use serde_json::{json, Value};

const PROJECT: &str = "project.test";
const RUN: &str = "run.syntax";

struct Fixture {
    _dir: tempfile::TempDir,
    store: Store,
    snapshot_id: String,
    config_digest: String,
    source_digest: String,
    source: &'static str,
}

impl Fixture {
    fn new() -> Self {
        let dir = tempfile::tempdir().expect("temp dir");
        let mut store = Store::open_in_memory(dir.path()).expect("store");
        store.ensure_project(PROJECT, "expedition").expect("project");
        let source = "def compute():\n    return 42\n\ndef main():\n    return compute()\n";
        let source_digest = store
            .put_blob(source.as_bytes(), "text/x-python")
            .expect("source blob");
        let manifest = store
            .put_blob(br#"{"files":["src/example.py"]}"#, "application/json")
            .expect("manifest blob");
        let snapshot_id = format!("sha256:{}", "a1".repeat(32));
        let config_digest = format!("sha256:{}", "b2".repeat(32));
        store
            .insert_snapshot(SnapshotInsert {
                snapshot_id: snapshot_id.clone(),
                project_id: PROJECT.to_string(),
                config_digest: config_digest.clone(),
                environment_digest: format!("sha256:{}", "c3".repeat(32)),
                exclusions_digest: format!("sha256:{}", "d4".repeat(32)),
                manifest_artifact: manifest,
                stable: true,
                coverage: coverage("partial", None, 1, 0, 0),
                payload: json!({"kind": "test"}),
                files: vec![],
            })
            .expect("snapshot");
        Self {
            _dir: dir,
            store,
            snapshot_id,
            config_digest,
            source_digest,
            source,
        }
    }

    fn entity(&self, id: &str, name: &str, start: u64, end: u64) -> Value {
        json!({
            "id": id,
            "project_id": PROJECT,
            "snapshot_id": self.snapshot_id,
            "config_digest": self.config_digest,
            "kind": "symbol",
            "language": "python",
            "qualified_name": name,
            "anchors": [{
                "path": "src/example.py",
                "blob_digest": self.source_digest,
                "precision": "bytes",
                "byte_start": start,
                "byte_end": end,
                "line_start": 0,
                "line_end": 1,
                "encoding": "utf-8"
            }],
            "external": false,
            "backend_refs": []
        })
    }

    fn evidence(&self, id: &str, kind: &str) -> Value {
        json!({
            "id": id,
            "project_id": PROJECT,
            "snapshot_id": self.snapshot_id,
            "config_digest": self.config_digest,
            "kind": kind,
            "producer": {
                "name": "test-adapter",
                "version": "1.0",
                "source_ref": "unit-test",
                "run_id": RUN
            },
            "artifact_digest": self.source_digest,
            "locator": "src/example.py",
            "source_anchor": null,
            "scenario_id": if kind == "runtime" { Value::String("scenario.test".to_string()) } else { Value::Null },
            "runtime_run_id": if kind == "runtime" { Value::String("runtime.test".to_string()) } else { Value::Null },
            "coverage": coverage("complete", Some(1), 1, 0, 0),
            "freshness": "historical"
        })
    }

    fn relation(&self, id: &str, source: &str, target: &str, kind: &str, basis: &str, resolution: &str, evidence: &[&str]) -> Value {
        json!({
            "id": id,
            "project_id": PROJECT,
            "snapshot_id": self.snapshot_id,
            "config_digest": self.config_digest,
            "source_id": source,
            "target_id": target,
            "kind": kind,
            "basis": basis,
            "evidence_ids": evidence,
            "conditions": [],
            "resolution": resolution
        })
    }

    fn batch(&self, sequence: u64, is_final: bool, entities: Vec<Value>, evidence: Vec<Value>, relations: Vec<Value>) -> Value {
        let final_coverage = if is_final {
            coverage("complete", Some(1), 1, 0, 0)
        } else {
            coverage("partial", None, 2, 0, 0)
        };
        json!({
            "schema_version": "1.0",
            "project_id": PROJECT,
            "snapshot_id": self.snapshot_id,
            "config_digest": self.config_digest,
            "run_id": RUN,
            "adapter_id": "synthetic",
            "adapter_version": "1.0",
            "sequence": sequence.to_string(),
            "final": is_final,
            "coverage": final_coverage,
            "entities": entities,
            "evidence": evidence,
            "relations": relations,
            "diagnostics": []
        })
    }

    fn two_entities(&self) -> Vec<Value> {
        vec![
            self.entity("symbol.compute", "example.compute", 4, 11),
            self.entity("symbol.main", "example.main", 31, 38),
        ]
    }
}

fn coverage(state: &str, expected: Option<u64>, observed: u64, omitted: u64, unresolved: u64) -> Coverage {
    Coverage {
        state: match state {
            "complete" => CoverageState::Complete,
            "partial" => CoverageState::Partial,
            "unsupported" => CoverageState::Unsupported,
            "failed" => CoverageState::Failed,
            "cancelled" => CoverageState::Cancelled,
            _ => CoverageState::Unknown,
        },
        scope: "unit test scope".to_string(),
        expected_count: expected,
        observed_count: observed,
        omitted_count: omitted,
        unresolved_count: unresolved,
        limitations: vec![],
    }
}

#[test]
fn batch_publishes_atomically_and_retry_is_idempotent() {
    let mut fx = Fixture::new();
    let evidence = vec![fx.evidence("evidence.syntax", "syntax")];
    let first = fx.batch(0, false, fx.two_entities(), evidence.clone(), vec![]);
    fx.store.import_batch(&first).expect("stage batch 0");

    let relation = fx.relation(
        "relation.call_candidate",
        "symbol.main",
        "symbol.compute",
        "static_call_candidate",
        "syntax",
        "candidate",
        &["evidence.syntax"],
    );
    let final_batch = fx.batch(1, true, vec![], evidence, vec![relation]);
    let outcome = fx.store.import_batch(&final_batch).expect("publish");
    assert_eq!(outcome.status, ImportStatus::Applied);
    assert!(outcome.published);

    let counts = fx.store.published_counts(PROJECT, &fx.snapshot_id).expect("counts");
    assert_eq!(counts.entities, 2);
    assert_eq!(counts.evidence, 1);
    assert_eq!(counts.relations, 1);
    assert_eq!(counts.relation_evidence, 1);
    assert!(fx.store.integrity_check().expect("integrity").is_empty());
    assert_eq!(
        fx.store
            .generation_status(PROJECT, &fx.snapshot_id, "synthetic", RUN)
            .unwrap()
            .as_deref(),
        Some("published")
    );

    let retry = fx.store.import_batch(&final_batch).expect("idempotent retry");
    assert_eq!(retry.status, ImportStatus::AlreadyApplied);
    assert!(retry.published);
    let counts_after = fx.store.published_counts(PROJECT, &fx.snapshot_id).expect("counts");
    assert_eq!(counts, counts_after);
}

#[test]
fn injected_crash_before_commit_leaves_no_partial_state() {
    let mut fx = Fixture::new();
    let evidence = vec![fx.evidence("evidence.syntax", "syntax")];
    let relation = fx.relation(
        "relation.call_candidate",
        "symbol.main",
        "symbol.compute",
        "static_call_candidate",
        "syntax",
        "candidate",
        &["evidence.syntax"],
    );
    let batch = fx.batch(0, true, fx.two_entities(), evidence.clone(), vec![relation]);

    let err = fx
        .store
        .import_batch_with_fault(&batch, FaultPoint::BeforeCommit)
        .expect_err("fault must abort");
    assert!(err.to_string().contains("injected fault"));

    let counts = fx.store.published_counts(PROJECT, &fx.snapshot_id).expect("counts");
    assert_eq!(counts, Default::default());
    assert_eq!(
        fx.store
            .generation_status(PROJECT, &fx.snapshot_id, "synthetic", RUN)
            .unwrap(),
        None,
        "generation row must be rolled back with the batch"
    );
    assert!(fx.store.integrity_check().expect("integrity").is_empty());

    let retry = fx.store.import_batch(&batch).expect("retry after crash");
    assert_eq!(retry.status, ImportStatus::Applied);
    assert_eq!(
        fx.store.published_counts(PROJECT, &fx.snapshot_id).unwrap().entities,
        2
    );
}

#[test]
fn missing_batch_is_not_published_as_complete() {
    let mut fx = Fixture::new();
    let batch0 = fx.batch(0, false, fx.two_entities(), vec![fx.evidence("evidence.syntax", "syntax")], vec![]);
    fx.store.import_batch(&batch0).expect("stage batch 0");

    let skipped = fx.batch(2, true, vec![], vec![], vec![]);
    let err = fx.store.import_batch(&skipped).expect_err("gap must be rejected");
    assert!(err.to_string().contains("sequence_gap"), "{err}");

    let counts = fx.store.published_counts(PROJECT, &fx.snapshot_id).expect("counts");
    assert_eq!(counts, Default::default());
    assert_eq!(
        fx.store
            .generation_status(PROJECT, &fx.snapshot_id, "synthetic", RUN)
            .unwrap()
            .as_deref(),
        Some("staging")
    );

    fx.store
        .abort_generation(PROJECT, &fx.snapshot_id, "synthetic", RUN)
        .expect("abort");
    assert_eq!(
        fx.store
            .generation_status(PROJECT, &fx.snapshot_id, "synthetic", RUN)
            .unwrap()
            .as_deref(),
        Some("failed")
    );
}

#[test]
fn same_sequence_with_different_payload_conflicts() {
    let mut fx = Fixture::new();
    let batch = fx.batch(0, false, fx.two_entities(), vec![fx.evidence("evidence.syntax", "syntax")], vec![]);
    fx.store.import_batch(&batch).expect("first");

    let mut changed = batch.clone();
    changed["entities"][0]["qualified_name"] = json!("example.changed");
    let err = fx.store.import_batch(&changed).expect_err("conflict");
    assert!(err.to_string().contains("different payload digest"), "{err}");
    assert_eq!(
        fx.store.published_counts(PROJECT, &fx.snapshot_id).unwrap(),
        Default::default()
    );
}

#[test]
fn model_evidence_cannot_support_resolved_call() {
    let mut fx = Fixture::new();
    let evidence = vec![
        fx.evidence("evidence.model", "model"),
        fx.evidence("evidence.syntax", "syntax"),
    ];
    let relation = fx.relation(
        "relation.fake",
        "symbol.main",
        "symbol.compute",
        "resolved_call",
        "model",
        "resolved",
        &["evidence.model"],
    );
    let batch = fx.batch(0, true, fx.two_entities(), evidence, vec![relation]);
    let err = fx.store.import_batch(&batch).expect_err("must reject");
    assert!(err.to_string().contains("resolved_call requires basis=semantic"), "{err}");
    assert_eq!(
        fx.store.published_counts(PROJECT, &fx.snapshot_id).unwrap(),
        Default::default()
    );
    assert!(fx.store.integrity_check().unwrap().is_empty());
}

#[test]
fn resolved_call_needs_semantic_evidence_not_just_syntax() {
    let mut fx = Fixture::new();
    let evidence = vec![
        fx.evidence("evidence.syntax", "syntax"),
        fx.evidence("evidence.semantic", "semantic"),
    ];
    let relation = fx.relation(
        "relation.call_resolved",
        "symbol.main",
        "symbol.compute",
        "resolved_call",
        "semantic",
        "resolved",
        &["evidence.syntax"],
    );
    let batch = fx.batch(0, true, fx.two_entities(), evidence, vec![relation]);
    let err = fx.store.import_batch(&batch).expect_err("must reject");
    assert!(err.to_string().contains("no evidence of basis semantic"), "{err}");
    let counts = fx.store.published_counts(PROJECT, &fx.snapshot_id).unwrap();
    assert_eq!(counts, Default::default());
    assert_eq!(
        fx.store.generation_status(PROJECT, &fx.snapshot_id, "synthetic", RUN).unwrap(),
        None,
        "failed final validation must not leave a staging generation behind"
    );
}

#[test]
fn dangling_relation_endpoint_is_rejected() {
    let mut fx = Fixture::new();
    let evidence = vec![fx.evidence("evidence.syntax", "syntax")];
    let relation = fx.relation(
        "relation.dangling",
        "symbol.main",
        "symbol.missing",
        "static_call_candidate",
        "syntax",
        "candidate",
        &["evidence.syntax"],
    );
    let batch = fx.batch(0, true, fx.two_entities(), evidence, vec![relation]);
    let err = fx.store.import_batch(&batch).expect_err("must reject");
    assert!(err.to_string().contains("does not exist in snapshot"), "{err}");
    assert_eq!(
        fx.store.published_counts(PROJECT, &fx.snapshot_id).unwrap(),
        Default::default()
    );
}

#[test]
fn anchor_offsets_must_fit_the_blob_and_utf8_boundaries() {
    let mut fx = Fixture::new();
    let too_far = fx.entity("symbol.main", "example.main", 0, fx.source.len() as u64 + 1);
    let batch = fx.batch(0, false, vec![too_far], vec![], vec![]);
    let err = fx.store.import_batch(&batch).expect_err("out of range");
    assert!(err.to_string().contains("exceeds blob size"), "{err}");

    // "中" occupies bytes 0..3 in a UTF-8 blob; offset 1 is a continuation byte.
    let cjk = "中文()\n";
    let cjk_digest = fx
        .store
        .put_blob(cjk.as_bytes(), "text/plain")
        .expect("cjk blob");
    let entity = json!({
        "id": "symbol.cjk",
        "project_id": PROJECT,
        "snapshot_id": fx.snapshot_id,
        "config_digest": fx.config_digest,
        "kind": "symbol",
        "language": "python",
        "qualified_name": "例子.cjk",
        "anchors": [{
            "path": "src/例子.py",
            "blob_digest": cjk_digest,
            "precision": "bytes",
            "byte_start": 1,
            "byte_end": 3,
            "encoding": "utf-8"
        }],
        "external": false,
        "backend_refs": []
    });
    let batch = fx.batch(0, false, vec![entity], vec![], vec![]);
    let err = fx.store.import_batch(&batch).expect_err("not a char boundary");
    assert!(err.to_string().contains("not a UTF-8 character boundary"), "{err}");
}

#[test]
fn scope_mismatch_is_rejected() {
    let mut fx = Fixture::new();
    let mut entity = fx.entity("symbol.main", "example.main", 31, 38);
    entity["snapshot_id"] = json!(format!("sha256:{}", "ff".repeat(32)));
    let batch = fx.batch(0, false, vec![entity], vec![], vec![]);
    let err = fx.store.import_batch(&batch).expect_err("cross-snapshot");
    assert!(err.to_string().contains("belongs to snapshot"), "{err}");
}

#[test]
fn ledger_survives_reopen() {
    let dir = tempfile::tempdir().expect("temp dir");
    let snapshot_id;
    let config_digest = format!("sha256:{}", "b2".repeat(32));
    {
        let mut store = Store::open(dir.path()).expect("open");
        store.ensure_project(PROJECT, "expedition").expect("project");
        let source_digest = store.put_blob(b"x = 1\n", "text/x-python").expect("blob");
        let manifest = store
            .put_blob(br#"{"files":[]}"#, "application/json")
            .expect("manifest");
        snapshot_id = format!("sha256:{}", "a1".repeat(32));
        store
            .insert_snapshot(SnapshotInsert {
                snapshot_id: snapshot_id.clone(),
                project_id: PROJECT.to_string(),
                config_digest: config_digest.clone(),
                environment_digest: format!("sha256:{}", "c3".repeat(32)),
                exclusions_digest: format!("sha256:{}", "d4".repeat(32)),
                manifest_artifact: manifest,
                stable: true,
                coverage: coverage("complete", Some(1), 1, 0, 0),
                payload: json!({"kind": "test"}),
                files: vec![],
            })
            .expect("snapshot");
        let batch = json!({
            "schema_version": "1.0",
            "project_id": PROJECT,
            "snapshot_id": snapshot_id,
            "config_digest": config_digest,
            "run_id": RUN,
            "adapter_id": "synthetic",
            "adapter_version": "1.0",
            "sequence": "0",
            "final": true,
            "coverage": {
                "state": "complete",
                "scope": "one file",
                "expected_count": 1,
                "observed_count": 1,
                "omitted_count": 0,
                "unresolved_count": 0,
                "limitations": []
            },
            "entities": [{
                "id": "symbol.x",
                "project_id": PROJECT,
                "snapshot_id": snapshot_id,
                "config_digest": config_digest,
                "kind": "symbol",
                "language": "python",
                "qualified_name": "x",
                "anchors": [{
                    "path": "src/x.py",
                    "blob_digest": source_digest,
                    "precision": "bytes",
                    "byte_start": 0,
                    "byte_end": 1,
                    "encoding": "utf-8"
                }],
                "external": false,
                "backend_refs": []
            }],
            "evidence": [],
            "relations": [],
            "diagnostics": []
        });
        store.import_batch(&batch).expect("import");
    }
    let store = Store::open(dir.path()).expect("reopen");
    assert_eq!(
        store.published_counts(PROJECT, &snapshot_id).unwrap().entities,
        1
    );
    assert!(store.integrity_check().unwrap().is_empty());
}
