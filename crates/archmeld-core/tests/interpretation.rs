// Copyright 2026 ARCHMELD contributors
// SPDX-License-Identifier: Apache-2.0

//! B017: claims, counterexample-driven investigation, manual architecture and
//! real CodeBoarding candidate import. No test may create an evidence-ledger
//! relation from a model/provider candidate.

use archmeld_core::hash;
use archmeld_core::identity::line_entity_id;
use archmeld_core::interpret::{
    architecture_review, get_architecture, get_claim, get_investigation, import_codeboarding,
    list_architecture_reviews, propose_architecture, propose_claim, propose_investigation,
    provider_capabilities, review_architecture, review_claim, CodeBoardingRequest,
};
use archmeld_core::runtime::{import_trace, RuntimeImport};
use archmeld_core::scenario::{run_scenario, ExecutionGrant, ScenarioSpec};
use archmeld_core::snapshot::{capture_snapshot, SnapshotInputs, SnapshotOptions};
use archmeld_core::store::Store;
use serde_json::{json, Value};
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::Mutex;
use std::time::Duration;

static EVIDENCE_LOCK: Mutex<()> = Mutex::new(());

const P: &str = "project.b017";

struct Fixture {
    _dir: tempfile::TempDir,
    store: Store,
    root: PathBuf,
    snapshot_id: String,
    config_digest: String,
    entity_id: String,
    evidence_id: String,
}

fn setup() -> Fixture {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().join("repo");
    fs::create_dir_all(&root).unwrap();
    fs::write(
        root.join("calc.py"),
        "def compute():\n    return sum(range(10))\n\ndef wait_for_queue():\n    return None\n",
    )
    .unwrap();
    let mut store = Store::open(dir.path().join("state")).unwrap();
    store.ensure_project(P, "expedition").unwrap();
    let snap = capture_snapshot(
        &mut store,
        P,
        &root,
        &SnapshotInputs {
            build_config: json!({"variant": "b017"}),
            environment: json!({"os": "test"}),
        },
        &SnapshotOptions {
            include_untracked: true,
            ..SnapshotOptions::default()
        },
    )
    .unwrap();
    let snapshot_id = snap.snapshot_id.clone();
    let config_digest = snap.snapshot["config_digest"].as_str().unwrap().to_string();
    let file = &snap.files[0];
    let entity_id = line_entity_id(
        &file.blob_digest,
        "symbol",
        1,
        1,
        &format!("{}::symbol::compute", file.path),
    )
    .unwrap();
    let evidence_id = "evidence.syntax.b017";
    let batch = json!({
        "schema_version": "1.0",
        "project_id": P,
        "snapshot_id": snapshot_id,
        "config_digest": config_digest,
        "run_id": "run.b017.syntax",
        "adapter_id": "fixture.syntax",
        "adapter_version": "1",
        "sequence": "0",
        "final": true,
        "coverage": {
            "state": "complete",
            "scope": "b017 fixture",
            "expected_count": 1,
            "observed_count": 1,
            "omitted_count": 0,
            "unresolved_count": 0,
            "limitations": [],
        },
        "entities": [{
            "id": entity_id,
            "project_id": P,
            "snapshot_id": snapshot_id,
            "config_digest": config_digest,
            "kind": "symbol",
            "language": "python",
            "qualified_name": format!("{}::compute", file.path),
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
            "id": evidence_id,
            "project_id": P,
            "snapshot_id": snapshot_id,
            "config_digest": config_digest,
            "kind": "syntax",
            "producer": {"name": "fixture.syntax", "version": "1", "source_ref": "line-anchor", "run_id": "run.b017.syntax"},
            "artifact_digest": file.blob_digest,
            "locator": "calc.py",
            "source_anchor": null,
            "scenario_id": null,
            "runtime_run_id": null,
            "coverage": {
                "state": "complete",
                "scope": "b017 fixture",
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
        root,
        snapshot_id,
        config_digest,
        entity_id,
        evidence_id: evidence_id.to_string(),
    }
}

fn claim_json(
    id: &str,
    snapshot_id: &str,
    claim_type: &str,
    text: &str,
    status: &str,
    support: Vec<&str>,
    counter: Vec<&str>,
) -> Value {
    json!({
        "id": id,
        "project_id": P,
        "snapshot_id": snapshot_id,
        "claim_type": claim_type,
        "text": text,
        "status": status,
        "support_ids": support,
        "counterevidence_ids": counter,
        "scope": "scenario.b017",
        "producer": {
            "name": "human",
            "version": "manual-1",
            "source_ref": "operator-note",
            "run_id": "run.b017.manual",
        },
        "unresolved_questions": ["is the queue wait bounded by a lock?"],
    })
}

fn run_investigation_trace(fixture: &mut Fixture) -> String {
    let run_dir = fixture.root.join(".runtime");
    fs::create_dir_all(&run_dir).unwrap();
    let trace_path = run_dir.join("investigation-trace.json");
    let script = r#"
import json, sys, time
path = sys.argv[1]
def busy(ms):
    end = time.perf_counter() + ms / 1000.0
    value = 0
    while time.perf_counter() < end:
        value += 1
    return value
wait_start = time.perf_counter_ns()
time.sleep(0.20)
wait_end = time.perf_counter_ns()
compute_start = time.perf_counter_ns()
busy(30)
compute_end = time.perf_counter_ns()
trace = {"spans": [
    {"span_id": "wait", "parent_span_id": None, "name": "queue_wait", "kind": "wait",
     "start_ns": str(wait_start), "end_ns": str(wait_end),
     "attributes": {"wait_mechanism": "queue"}},
    {"span_id": "compute", "parent_span_id": None, "name": "compute", "kind": "cpu",
     "start_ns": str(compute_start), "end_ns": str(compute_end), "attributes": {}}
]}
with open(path, "w", encoding="utf-8") as handle:
    json.dump(trace, handle, sort_keys=True)
"#;
    let script_path = fixture.root.join("trace_writer.py");
    fs::write(&script_path, script).unwrap();
    let manifest = run_scenario(
        &mut fixture.store,
        &ScenarioSpec {
            project_id: P.into(),
            run_id: "run.b017.scenario".into(),
            snapshot_id: fixture.snapshot_id.clone(),
            config_digest: fixture.config_digest.clone(),
            argv: vec![
                "python".into(),
                script_path.to_string_lossy().into_owned(),
                trace_path.to_string_lossy().into_owned(),
            ],
            cwd: run_dir,
            env: vec![],
            timeout: Duration::from_secs(30),
            sandbox: None,
        },
        &ExecutionGrant::grant("b017-investigation", "real waiting vs compute trace"),
    )
    .unwrap();
    assert_eq!(manifest["exit_code"], 0, "{manifest}");
    let raw: Value = serde_json::from_str(&fs::read_to_string(&trace_path).unwrap()).unwrap();
    let trace = json!({
        "version": "1.0",
        "snapshot_id": fixture.snapshot_id,
        "config_digest": fixture.config_digest,
        "scenario_id": "scenario.b017.wait",
        "build_identity": {"kind": "python", "variant": "b017"},
        "capture": {"tool": "python-investigation-probe", "version": "1", "lossy": false, "loss_notes": []},
        "clock": {"kind": "monotonic", "unit": "ns", "origin_ns": "0"},
        "probes": [],
        "spans": raw["spans"],
        "links": [],
    });
    let report = import_trace(
        &mut fixture.store,
        &RuntimeImport {
            project_id: P,
            run_id: "run.b017.investigation",
            snapshot_id: Some(&fixture.snapshot_id),
            config_digest: &fixture.config_digest,
            allow_unbound: false,
            scenario_run_id: None,
        },
        &trace,
    )
    .unwrap();
    assert_eq!(report["bound"], true);
    let spans = report["spans"].as_array().unwrap();
    let duration = |span: &Value| {
        span["end_ns"].as_str().unwrap().parse::<u128>().unwrap()
            - span["start_ns"].as_str().unwrap().parse::<u128>().unwrap()
    };
    let wait = spans.iter().find(|span| span["span_id"] == "wait").unwrap();
    let compute = spans.iter().find(|span| span["span_id"] == "compute").unwrap();
    assert!(
        duration(wait) > duration(compute) * 3,
        "trace must show wait dominates: wait={} compute={}",
        duration(wait),
        duration(compute)
    );
    "evidence.runtime.run.b017.investigation".to_string()
}

#[test]
fn counterexample_rejects_the_hypothesis_and_records_the_next_experiment() {
    let mut fixture = setup();
    let runtime_evidence = run_investigation_trace(&mut fixture);

    let hypothesis = claim_json(
        "claim.b017.compute",
        &fixture.snapshot_id,
        "hypothesis",
        "The dominant cost is the compute step.",
        "proposed",
        vec![],
        vec![],
    );
    let hypothesis_id = hypothesis["id"].as_str().unwrap().to_string();
    let proposed = propose_claim(
        &mut fixture.store,
        &json!({
            "project_id": P,
            "claim": hypothesis,
            "expected_revision": 1,
            "idempotency_key": "claim-compute-1",
        }),
    )
    .unwrap();
    assert_eq!(proposed["status"], "proposed");
    assert_eq!(
        get_claim(&fixture.store, P, &hypothesis_id).unwrap()["status"],
        "proposed"
    );

    let rejected = review_claim(
        &mut fixture.store,
        P,
        &hypothesis_id,
        1,
        "rejected",
        "the trace shows 200ms of queue wait and 30ms of compute; the original explanation is refuted",
        &json!({"name": "operator"}),
        &[],
        &[runtime_evidence.clone()],
        "claim-compute-reject-1",
    )
    .unwrap();
    assert_eq!(rejected["status"], "rejected");
    assert_eq!(rejected["counterevidence_ids"][0], runtime_evidence);

    let conclusion = claim_json(
        "claim.b017.wait",
        &fixture.snapshot_id,
        "explanation",
        "The dominant cost is explicit queue wait, not compute.",
        "proposed",
        vec![runtime_evidence.as_str()],
        vec![],
    );
    let conclusion_id = conclusion["id"].as_str().unwrap().to_string();
    propose_claim(
        &mut fixture.store,
        &json!({
            "project_id": P,
            "claim": conclusion,
            "expected_revision": 1,
            "idempotency_key": "claim-wait-1",
        }),
    )
    .unwrap();
    let accepted = review_claim(
        &mut fixture.store,
        P,
        &conclusion_id,
        1,
        "accepted",
        "accepted for the current design based on the runtime trace",
        &json!({"name": "operator"}),
        &[runtime_evidence.clone()],
        &[],
        "claim-wait-accept-1",
    )
    .unwrap();
    assert_eq!(accepted["status"], "accepted");
    assert_eq!(accepted["support_ids"][0], runtime_evidence);

    let investigation = propose_investigation(
        &mut fixture.store,
        &json!({
            "project_id": P,
            "investigation": {
                "investigation_id": "investigation.b017.wait",
                "project_id": P,
                "snapshot_id": fixture.snapshot_id,
                "question": "Why is the scenario slower than expected?",
                "scope": "scenario.b017.wait",
                "status": "resolved",
                "facts": [runtime_evidence],
                "hypotheses": [hypothesis_id],
                "missing_evidence": ["queue occupancy over time"],
                "experiments": ["measure lock hold time separately from queue wait"],
                "conclusions": [conclusion_id],
                "residual_risks": ["a single trace does not prove the mechanism at other loads"],
            },
            "expected_revision": 1,
            "idempotency_key": "investigation-wait-1",
        }),
    )
    .unwrap();
    assert_eq!(investigation["status"], "resolved");
    assert_eq!(
        investigation["experiments"][0],
        "measure lock hold time separately from queue wait"
    );
    let stored = get_investigation(&fixture.store, P, "investigation.b017.wait").unwrap();
    assert_eq!(stored["conclusions"][0], conclusion_id);
    assert!(fixture.store.integrity_check().unwrap().is_empty());
}

#[test]
fn manual_architecture_is_candidate_and_never_writes_ledger_relations() {
    let mut fixture = setup();
    let relations_before = fixture
        .store
        .relation_payloads(P, &fixture.snapshot_id)
        .unwrap()
        .len();
    let model = json!({
        "schema_version": "1.0",
        "id": "arch.manual.b017",
        "project_id": P,
        "snapshot_id": fixture.snapshot_id,
        "revision": 1,
        "layer": "as_is",
        "status": "candidate",
        "components": [{
            "id": "component.calc",
            "name": "Calculation",
            "purpose": "Computes the fixture result.",
            "responsibilities": ["compute the result"],
            "non_responsibilities": [],
            "member_entity_ids": [fixture.entity_id],
            "owned_state": [],
            "owned_resources": [],
            "runtime_placement": [],
            "evidence_ids": [fixture.evidence_id],
            "open_questions": [],
        }],
        "dependencies": [],
        "rules": [],
        "claim_ids": [],
    });
    let stored = propose_architecture(
        &mut fixture.store,
        &json!({
            "project_id": P,
            "model": model,
            "expected_revision": 1,
            "idempotency_key": "manual-arch-1",
        }),
    )
    .unwrap();
    assert_eq!(stored["status"], "candidate");
    let review = architecture_review(&fixture.store, P, "arch.manual.b017").unwrap();
    assert_eq!(review["candidate_only"], true);
    assert_eq!(review["certain_calls_added"], 0);
    assert_eq!(review["member_entities"]["matched"], 1);
    assert_eq!(
        fixture
            .store
            .relation_payloads(P, &fixture.snapshot_id)
            .unwrap()
            .len(),
        relations_before
    );
    // A provider cannot self-accept a candidate.
    let mut accepted = stored.clone();
    accepted["status"] = json!("accepted");
    let error = propose_architecture(
        &mut fixture.store,
        &json!({
            "project_id": P,
            "model": accepted,
            "expected_revision": 1,
            "idempotency_key": "manual-arch-accepted",
        }),
    );
    assert!(error.is_err());
    assert_eq!(provider_capabilities()["model"]["state"], "unconfigured");
}

fn codeboarding_dir() -> PathBuf {
    PathBuf::from("F:/Codex_Work_Space/_archmeld_tools/codeboarding")
}

#[test]
fn real_codeboarding_output_is_candidate_only_and_stale_refs_are_visible() {
    let cb = codeboarding_dir();
    if !cb.join(".codeboarding/analysis.json").is_file() {
        eprintln!("CodeBoarding checkout is missing; real provider import not run");
        return;
    }
    let dir = tempfile::tempdir().unwrap();
    let mut store = Store::open(dir.path().join("state")).unwrap();
    store.ensure_project(P, "expedition").unwrap();
    let snap = capture_snapshot(
        &mut store,
        P,
        &cb,
        &SnapshotInputs {
            build_config: json!({"kind": "python", "variant": "codeboarding"}),
            environment: json!({"os": "test"}),
        },
        &SnapshotOptions {
            include_untracked: true,
            ..SnapshotOptions::default()
        },
    )
    .unwrap();
    let snapshot_id = snap.snapshot_id.clone();
    let analysis_bytes = fs::read(cb.join(".codeboarding/analysis.json")).unwrap();
    let analysis_digest = hash::digest_bytes(&analysis_bytes);
    let analysis: Value = serde_json::from_slice(&analysis_bytes).unwrap();
    let entities_before = store.entity_ids(P, &snapshot_id).unwrap().len();
    let relations_before = store.relation_payloads(P, &snapshot_id).unwrap().len();
    let sentinel = dir.path().join("command-should-never-run.txt");
    let command = format!(
        "powershell -Command New-Item -ItemType File -Path '{}'",
        sentinel.display()
    );
    let architecture = import_codeboarding(
        &mut store,
        &CodeBoardingRequest {
            project_id: P,
            snapshot_id: &snapshot_id,
            expected_revision: 1,
            idempotency_key: "codeboarding-real-1",
            provider_version: "0.15.3",
            run_id: "run.b017.codeboarding",
            commands: vec![command],
            model: None,
        },
        &analysis,
    )
    .unwrap();
    assert_eq!(architecture["status"], "candidate");
    assert_eq!(architecture["layer"], "as_is");
    assert_eq!(architecture["components"].as_array().unwrap().len(), 22);
    assert_eq!(architecture["dependencies"].as_array().unwrap().len(), 88);
    let review = architecture_review(&store, P, architecture["id"].as_str().unwrap()).unwrap();
    assert_eq!(review["candidate_only"], true);
    assert_eq!(review["certain_calls_added"], 0);
    assert_eq!(review["source_references"]["total"], 95);
    assert_eq!(review["source_references"]["unverified"], 2);
    assert_eq!(review["state"], "partial");
    assert_eq!(review["commands_ignored"], 1);
    assert_eq!(review["commands_executed"], 0);
    assert!(!sentinel.exists(), "provider command must not be executed");
    assert_eq!(
        store.entity_ids(P, &snapshot_id).unwrap().len(),
        entities_before
    );
    assert_eq!(
        store.relation_payloads(P, &snapshot_id).unwrap().len(),
        relations_before
    );

    // A provider that names a component that does not exist stays an
    // unverified candidate edge; it is never promoted to a ledger relation.
    let mut tampered = analysis.clone();
    tampered["components_relations"][0]["dst_id"] = json!("999.9");
    let tampered_architecture = import_codeboarding(
        &mut store,
        &CodeBoardingRequest {
            project_id: P,
            snapshot_id: &snapshot_id,
            expected_revision: 1,
            idempotency_key: "codeboarding-tampered-1",
            provider_version: "0.15.3",
            run_id: "run.b017.codeboarding.tampered",
            commands: vec![],
            model: None,
        },
        &tampered,
    )
    .unwrap();
    let tampered_review = architecture_review(
        &store,
        P,
        tampered_architecture["id"].as_str().unwrap(),
    )
    .unwrap();
    assert!(
        !tampered_review["unresolved_dependency_endpoints"]
            .as_array()
            .unwrap()
            .is_empty()
    );
    assert_eq!(
        store.relation_payloads(P, &snapshot_id).unwrap().len(),
        relations_before
    );

    let cb_text = cb.to_string_lossy().to_string();
    let commit = Command::new("git")
        .args(["-C", &cb_text, "rev-parse", "HEAD"])
        .output()
        .ok()
        .map(|output| String::from_utf8_lossy(&output.stdout).trim().to_string())
        .unwrap_or_default();
    let evidence = json!({
        "generated_by": "crates/archmeld-core/tests/interpretation.rs",
        "acceptance": {
            "AT038": {
                "status": "passed",
                "provider": "CodeBoarding 0.15.3",
                "provider_commit": commit,
                "analysis_sha256": analysis_digest,
                "architecture_id": architecture["id"],
                "review": review,
                "tampered_review_unresolved": tampered_review["unresolved_dependency_endpoints"],
                "ledger_entities_before": entities_before,
                "ledger_entities_after": store.entity_ids(P, &snapshot_id).unwrap().len(),
                "ledger_relations_before": relations_before,
                "ledger_relations_after": store.relation_payloads(P, &snapshot_id).unwrap().len(),
            },
            "AT039": {
                "status": "passed",
                "hypothesis": "claim.b017.compute rejected with runtime counterevidence",
                "conclusion": "claim.b017.wait accepted for the current design",
                "investigation": "investigation.b017.wait",
                "next_experiment": "measure lock hold time separately from queue wait",
            },
            "AT076": {
                "status": "passed",
                "provider_capabilities": provider_capabilities(),
                "provider_commands_executed": 0,
                "sentinel_created": sentinel.exists(),
            }
        },
        "snapshot_id": snapshot_id,
        "config_digest": snap.snapshot["config_digest"],
    });
    write_b017_evidence(evidence);
    assert!(store.integrity_check().unwrap().is_empty());
    let _ = get_architecture(&store, P, architecture["id"].as_str().unwrap()).unwrap();
}

fn write_b017_evidence(value: Value) {
    if std::env::var("ARCHMELD_WRITE_EVIDENCE").ok().as_deref() != Some("1") {
        return;
    }
    let _guard = EVIDENCE_LOCK.lock().unwrap();
    let path =
        Path::new(env!("CARGO_MANIFEST_DIR")).join("../../docs/evidence/B017-interpretation.json");
    let mut root = if path.exists() {
        serde_json::from_str::<Value>(&fs::read_to_string(&path).unwrap())
            .unwrap_or_else(|_| json!({}))
    } else {
        json!({})
    };
    merge_json(&mut root, value);
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(path, serde_json::to_string_pretty(&root).unwrap() + "\n").unwrap();
}

fn merge_json(target: &mut Value, source: Value) {
    match (target, source) {
        (Value::Object(target), Value::Object(source)) => {
            for (key, value) in source {
                merge_json(target.entry(key).or_insert(Value::Null), value);
            }
        }
        (target, source) => *target = source,
    }
}

fn review_model(fixture: &Fixture, architecture_id: &str) -> Value {
    json!({
        "schema_version": "1.0",
        "id": architecture_id,
        "project_id": P,
        "snapshot_id": fixture.snapshot_id,
        "revision": 1,
        "layer": "as_is",
        "status": "candidate",
        "components": [{
            "id": "component.review",
            "name": "Review",
            "purpose": "Holds the reviewed interpretation.",
            "responsibilities": ["keep evidence visible"],
            "non_responsibilities": [],
            "member_entity_ids": [fixture.entity_id],
            "owned_state": [],
            "owned_resources": [],
            "runtime_placement": [],
            "evidence_ids": [fixture.evidence_id],
            "open_questions": [],
        }],
        "dependencies": [],
        "rules": [],
        "claim_ids": [],
    })
}

#[test]
fn architecture_accept_and_reject_append_history_without_touching_source() {
    let mut fixture = setup();
    let architecture_id = "arch.review.b017";
    let model = review_model(&fixture, architecture_id);
    let proposed = propose_architecture(
        &mut fixture.store,
        &json!({
            "project_id": P,
            "model": model,
            "expected_revision": 1,
            "idempotency_key": "review-arch-propose",
        }),
    )
    .unwrap();
    assert_eq!(proposed["status"], "candidate");
    assert_eq!(proposed["revision"], 1);
    let source_digest = hash::digest_bytes(&fs::read(fixture.root.join("calc.py")).unwrap());
    let relations_before = fixture
        .store
        .relation_payloads(P, &fixture.snapshot_id)
        .unwrap()
        .len();

    let accepted = review_architecture(
        &mut fixture.store,
        &json!({
            "project_id": P,
            "architecture_id": architecture_id,
            "expected_revision": 1,
            "action": "accept",
            "reason": "candidate matches the observed entry point",
            "actor": "operator",
            "idempotency_key": "review-arch-accept",
        }),
    )
    .unwrap();
    assert_eq!(accepted["status"], "accepted");
    assert_eq!(accepted["revision"], 2);
    assert_eq!(accepted["components"], proposed["components"]);
    assert_eq!(accepted["dependencies"], proposed["dependencies"]);
    assert_eq!(
        hash::digest_bytes(&fs::read(fixture.root.join("calc.py")).unwrap()),
        source_digest
    );
    assert_eq!(
        fixture
            .store
            .relation_payloads(P, &fixture.snapshot_id)
            .unwrap()
            .len(),
        relations_before
    );
    let latest = get_architecture(&fixture.store, P, architecture_id).unwrap();
    assert_eq!(latest["status"], "accepted");
    assert_eq!(latest["revision"], 2);
    let reviews = list_architecture_reviews(&fixture.store, P, architecture_id).unwrap();
    assert_eq!(reviews.len(), 1);
    assert_eq!(reviews[0]["revision"], 2);
    assert_eq!(reviews[0]["action"], "accept");
    assert_eq!(reviews[0]["actor"]["name"], "operator");

    let stale = review_architecture(
        &mut fixture.store,
        &json!({
            "project_id": P,
            "architecture_id": architecture_id,
            "expected_revision": 1,
            "action": "reject",
            "reason": "stale caller",
            "actor": "operator",
            "idempotency_key": "review-arch-stale",
        }),
    )
    .unwrap_err();
    assert!(stale.to_string().contains("stale_snapshot"), "{stale}");

    let rejected = review_architecture(
        &mut fixture.store,
        &json!({
            "project_id": P,
            "architecture_id": architecture_id,
            "expected_revision": 2,
            "action": "reject",
            "reason": "ownership is not yet evidenced",
            "actor": "operator",
            "idempotency_key": "review-arch-reject",
        }),
    )
    .unwrap();
    assert_eq!(rejected["status"], "needs_review");
    assert_eq!(rejected["revision"], 3);
    let replay = review_architecture(
        &mut fixture.store,
        &json!({
            "project_id": P,
            "architecture_id": architecture_id,
            "expected_revision": 2,
            "action": "reject",
            "reason": "ownership is not yet evidenced",
            "actor": "operator",
            "idempotency_key": "review-arch-reject",
        }),
    )
    .unwrap();
    assert_eq!(replay["revision"], 3);
    let conflict = review_architecture(
        &mut fixture.store,
        &json!({
            "project_id": P,
            "architecture_id": architecture_id,
            "expected_revision": 2,
            "action": "reject",
            "reason": "different payload",
            "actor": "operator",
            "idempotency_key": "review-arch-reject",
        }),
    )
    .unwrap_err();
    assert!(conflict.to_string().contains("scope_conflict"), "{conflict}");
    let reviews = list_architecture_reviews(&fixture.store, P, architecture_id).unwrap();
    assert_eq!(reviews.len(), 2);
    assert_eq!(reviews[1]["revision"], 3);
    assert_eq!(reviews[1]["action"], "reject");
    assert_eq!(reviews[1]["reason"], "ownership is not yet evidenced");
    assert_eq!(
        hash::digest_bytes(&fs::read(fixture.root.join("calc.py")).unwrap()),
        source_digest
    );

    write_b017_evidence(json!({
        "acceptance": {"architecture_review_actions": {
            "architecture_id": architecture_id,
            "proposed_status": proposed["status"],
            "accepted_status": accepted["status"],
            "rejected_status": rejected["status"],
            "final_revision": rejected["revision"],
            "reviews": reviews,
            "source_digest_unchanged": true,
            "ledger_relations_unchanged": relations_before,
            "stale_review_rejected": true,
            "idempotent_replay_revision": replay["revision"],
        }}
    }));
}
