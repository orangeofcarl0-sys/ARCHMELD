// Copyright 2026 ARCHMELD contributors
// SPDX-License-Identifier: Apache-2.0

//! B027 / AT059–AT060: self-contained offline report bundles and Expedition →
//! Manager promotion with full object validation.

mod common;

use archmeld_core::changeset::get_changeset;
use archmeld_core::hash;
use archmeld_core::interpret::{
    get_architecture, get_claim, propose_claim, review_claim,
};
use archmeld_core::manager::{get_project, list_sessions, promote_project};
use archmeld_core::plan::{
    approve_plan, get_contract, get_plan, propose_contract, propose_plan,
};
use archmeld_core::report::{export_report, verify_bundle};
use archmeld_core::runtime::{import_trace, RuntimeImport};
use archmeld_core::scenario::run_command_capture;
use archmeld_core::verification::verify_changeset;
use serde_json::{json, Value};
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::Duration;

static EVIDENCE_LOCK: Mutex<()> = Mutex::new(());

fn snapshot_file(fixture: &common::Fixture, path: &str) -> Value {
    fixture
        .store
        .snapshot_payload(&fixture.project, &fixture.snapshot_id)
        .unwrap()
        .unwrap()["files"]
        .as_array()
        .unwrap()
        .iter()
        .find(|file| file["path"] == path)
        .unwrap_or_else(|| panic!("missing snapshot file {path}"))
        .clone()
}

fn seed_evidence_and_claim(fixture: &mut common::Fixture) -> (String, String) {
    let file = snapshot_file(fixture, "src/a.py");
    let blob_digest = file["blob_digest"].as_str().unwrap().to_string();
    let evidence_id = "evidence.report.b027".to_string();
    let entity_id = "entity.report.b027".to_string();
    let batch = json!({
        "schema_version": "1.0",
        "project_id": fixture.project,
        "snapshot_id": fixture.snapshot_id,
        "config_digest": fixture.config_digest,
        "run_id": "run.report.b027",
        "adapter_id": "fixture.report",
        "adapter_version": "1",
        "sequence": "0",
        "final": true,
        "coverage": {
            "state": "complete",
            "scope": "B027 report fixture",
            "expected_count": 1,
            "observed_count": 1,
            "omitted_count": 0,
            "unresolved_count": 0,
            "limitations": [],
        },
        "entities": [{
            "id": entity_id,
            "project_id": fixture.project,
            "snapshot_id": fixture.snapshot_id,
            "config_digest": fixture.config_digest,
            "kind": "symbol",
            "language": "python",
            "qualified_name": "src/a.py::sample",
            "anchors": [{
                "path": "src/a.py",
                "blob_digest": blob_digest,
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
            "project_id": fixture.project,
            "snapshot_id": fixture.snapshot_id,
            "config_digest": fixture.config_digest,
            "kind": "syntax",
            "producer": {
                "name": "fixture.report",
                "version": "1",
                "source_ref": "line-anchor",
                "run_id": "run.report.b027",
            },
            "artifact_digest": blob_digest,
            "locator": "src/a.py",
            "source_anchor": null,
            "scenario_id": null,
            "runtime_run_id": null,
            "coverage": {
                "state": "complete",
                "scope": "B027 report fixture",
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
    let claim = json!({
        "id": "claim.report.b027",
        "project_id": fixture.project,
        "snapshot_id": fixture.snapshot_id,
        "claim_type": "decision",
        "text": "The sample function is the stable entry point.",
        "status": "proposed",
        "support_ids": [evidence_id],
        "counterevidence_ids": [],
        "scope": "src/a.py",
        "producer": {
            "name": "human",
            "version": "manual-1",
            "source_ref": "review-note",
            "run_id": "run.report.b027",
        },
        "unresolved_questions": ["does cancellation release every child process?"],
    });
    let proposed = propose_claim(
        &mut fixture.store,
        &json!({
            "project_id": fixture.project,
            "claim": claim,
            "expected_revision": 1,
            "idempotency_key": "report-claim-1",
        }),
    )
    .unwrap();
    review_claim(
        &mut fixture.store,
        &fixture.project,
        proposed["id"].as_str().unwrap(),
        1,
        "accepted",
        "accepted for the current design with a recorded unknown",
        &json!({"name": "operator"}),
        &[],
        &[],
        "report-claim-accept-1",
    )
    .unwrap();
    (evidence_id, proposed["id"].as_str().unwrap().to_string())
}

fn failed_verification_plan(fixture: &mut common::Fixture) -> String {
    let checks = vec![json!({
        "id": "check.fail",
        "kind": "behavior",
        "required": true,
        "description": "intentional failure for report evidence",
        "command": {
            "argv": ["python", "-c", "import sys; sys.exit(1)"],
            "cwd": null,
            "timeout_ms": 5000,
        },
        "checker_id": null,
        "predicate": {"kind": "exit_code", "expected_codes": [0]},
        "input_artifacts": [],
        "sandbox": {"mode": "workspace", "include": [], "exclude": []},
    })];
    let mut contract = json!({
        "schema_version": "1.0",
        "id": "contract.report.failed",
        "project_id": fixture.project,
        "revision": 1,
        "kind": "structural_refactor",
        "invariants": ["behavior stays unchanged"],
        "allowed_changes": ["report fixture"],
        "known_defects": [],
        "checks": checks,
        "digest": format!("sha256:{}", "0".repeat(64)),
        "frozen": false,
    });
    let mut preimage = contract.clone();
    preimage.as_object_mut().unwrap().remove("digest");
    preimage.as_object_mut().unwrap().remove("frozen");
    contract["digest"] = json!(hash::digest_value(&preimage).unwrap());
    let contract = propose_contract(
        &mut fixture.store,
        &json!({
            "project_id": fixture.project,
            "contract": contract,
            "expected_revision": 1,
            "idempotency_key": "report-contract-failed",
        }),
    )
    .unwrap();
    let plan_id = "plan.report.failed";
    let plan = propose_plan(
        &mut fixture.store,
        &json!({
            "project_id": fixture.project,
            "plan": {
                "schema_version": "1.0",
                "id": plan_id,
                "project_id": fixture.project,
                "revision": 1,
                "kind": "structural_refactor",
                "objective": "record a failed check in the report",
                "non_goals": [],
                "base_snapshot_id": fixture.snapshot_id,
                "target_architecture_id": fixture.architecture_id,
                "contract_id": contract["id"],
                "contract_digest": contract["digest"],
                "steps": [{
                    "id": "step.1",
                    "title": "failed report step",
                    "depends_on": [],
                    "allowed_paths": ["src/a.py"],
                    "required_check_ids": ["check.fail"],
                    "completion_criteria": ["check intentionally fails"],
                }],
                "risks": [],
                "evidence_ids": [],
                "status": "draft",
            },
            "expected_revision": 1,
            "idempotency_key": "report-plan-failed",
        }),
    )
    .unwrap();
    approve_plan(
        &mut fixture.store,
        &fixture.project,
        plan_id,
        plan["revision"].as_i64().unwrap(),
        contract["digest"].as_str().unwrap(),
        "operator",
        "report-plan-approve",
    )
    .unwrap();
    plan_id.to_string()
}

fn apply_report_change(fixture: &mut common::Fixture, plan_id: &str) -> Value {
    let changeset_id = "changeset.report.failed";
    common::create_changeset(fixture, changeset_id, plan_id, &[]);
    let seeded = common::prepare(fixture, changeset_id, 1, None, "seed").unwrap();
    let staging = common::staging_path(fixture, &seeded);
    common::write_file(
        &staging,
        "src/a.py",
        "VALUE = 42\n# SOURCE_SECRET_MARKER_DO_NOT_EXPORT\n",
    );
    let transform = common::transform(fixture, "manual_edit", json!([]));
    let prepared = common::prepare(
        fixture,
        changeset_id,
        seeded["revision"].as_i64().unwrap(),
        Some(transform),
        "transform",
    )
    .unwrap();
    let approval = common::approve_prepared(
        fixture,
        changeset_id,
        prepared["revision"].as_i64().unwrap(),
        prepared["prepared_digest"].as_str().unwrap(),
    );
    common::apply(
        fixture,
        changeset_id,
        prepared["revision"].as_i64().unwrap(),
        prepared["prepared_digest"].as_str().unwrap(),
        approval["approval"]["approval_id"].as_str().unwrap(),
        "report-apply",
    )
    .unwrap()
}

fn failed_verification(fixture: &mut common::Fixture) -> Value {
    let plan_id = failed_verification_plan(fixture);
    let applied = apply_report_change(fixture, &plan_id);
    let verification = verify_changeset(
        &mut fixture.store,
        &json!({
            "project_id": fixture.project,
            "changeset_id": "changeset.report.failed",
            "expected_revision": applied["revision"],
            "workspace_handle": fixture.handle,
            "execution_grant": {
                "grant_id": "grant.b027.report",
                "reason": "intentional failed check for offline report evidence",
            },
            "environment": {"profile": "b027-report"},
            "snapshot_id": null,
            "idempotency_key": "report-verify-failed",
        }),
    )
    .unwrap();
    assert_eq!(verification["state"], "failed");
    assert_eq!(verification["verdict"]["acceptance"], false);
    assert_eq!(verification["checks"][0]["state"], "failed");
    let changeset = get_changeset(
        &fixture.store,
        &fixture.project,
        "changeset.report.failed",
    )
    .unwrap();
    assert_eq!(changeset["state"], "failed");
    verification
}

fn chrome_path() -> Option<PathBuf> {
    [
        "C:/Program Files/Google/Chrome/Application/chrome.exe",
        "C:/Program Files (x86)/Microsoft/Edge/Application/msedge.exe",
    ]
    .iter()
    .map(PathBuf::from)
    .find(|path| path.is_file())
}

#[test]
fn offline_report_is_self_contained_and_promotion_preserves_objects() {
    let mut fixture = common::setup("b027-report");
    let (evidence_id, claim_id) = seed_evidence_and_claim(&mut fixture);
    let verification = failed_verification(&mut fixture);
    let bundle_dir = fixture._dir.path().join("offline-report");
    let bundle = export_report(
        &mut fixture.store,
        &json!({
            "project_id": fixture.project,
            "snapshot_id": fixture.snapshot_id,
            "output_dir": bundle_dir.to_string_lossy(),
            "include_source": false,
            "include_raw_traces": false,
        }),
    )
    .unwrap();
    assert_eq!(bundle["offline"], true);
    assert_eq!(bundle["manifest"]["scripts"], 0);
    assert_eq!(bundle["manifest"]["external_urls"], 0);
    assert_eq!(bundle["manifest"]["source_included"], false);
    let files = bundle["manifest"]["files"].as_array().unwrap();
    let paths: Vec<&str> = files.iter().filter_map(|file| file["path"].as_str()).collect();
    assert_eq!(paths.len(), 4);
    for expected in ["report.html", "report.json", "model.json", "evidence/index.json"] {
        assert!(paths.contains(&expected), "{paths:?}");
    }
    let verified = verify_bundle(&bundle_dir, &bundle["manifest"]).unwrap();
    assert_eq!(verified["checked_files"], 4);
    assert_eq!(verified["bundle_digest"], bundle["bundle_digest"]);

    let report_bytes = fs::read(bundle_dir.join("report.json")).unwrap();
    let report: Value = serde_json::from_slice(&report_bytes).unwrap();
    assert_eq!(report["snapshot"]["id"], fixture.snapshot_id);
    assert!(!report["claims"].as_array().unwrap().is_empty());
    assert_eq!(report["claims"][0]["status"], "accepted");
    assert_eq!(report["claims"][0]["support_ids"][0], evidence_id);
    assert_eq!(report["claims"][0]["id"], claim_id);
    assert_eq!(report["verifications"][0]["state"], "failed");
    assert_eq!(report["verifications"][0]["checks"][0]["state"], "failed");
    assert!(!report["unknowns"].as_array().unwrap().is_empty());
    assert!(!report["evidence"].as_array().unwrap().is_empty());
    assert_eq!(report["evidence"][0]["artifact_available"], true);
    assert_eq!(report["source_policy"]["source_included"], false);
    assert_eq!(report["graph"]["entities_included"], 1);

    let html = fs::read_to_string(bundle_dir.join("report.html")).unwrap();
    let lower = html.to_lowercase();
    assert!(!lower.contains("<script"));
    assert!(!lower.contains("http://"));
    assert!(!lower.contains("https://"));
    assert!(!html.contains("SOURCE_SECRET_MARKER_DO_NOT_EXPORT"));
    assert!(!html.contains(&fixture.root.to_string_lossy().to_string()));
    assert!(html.contains("ARCHMELD offline review"));
    assert!(html.contains("Verification"));
    assert!(html.contains("Evidence index"));
    assert!(!String::from_utf8_lossy(&report_bytes).contains("SOURCE_SECRET_MARKER_DO_NOT_EXPORT"));

    let browser = chrome_path().map(|chrome| {
        let profile = fixture._dir.path().join("chrome-profile");
        let uri = format!(
            "file:///{}",
            bundle_dir.join("report.html").to_string_lossy().replace('\\', "/")
        );
        let capture = run_command_capture(
            &[
                chrome.to_string_lossy().to_string(),
                "--headless=new".to_string(),
                "--disable-gpu".to_string(),
                "--no-sandbox".to_string(),
                "--disable-background-networking".to_string(),
                "--disable-component-update".to_string(),
                "--no-first-run".to_string(),
                format!("--user-data-dir={}", profile.to_string_lossy()),
                "--host-resolver-rules=MAP * ~NOTFOUND".to_string(),
                "--dump-dom".to_string(),
                uri,
            ],
            &fixture._dir.path(),
            &[],
            Duration::from_secs(60),
            None,
        );
        match capture {
            Ok(capture) => json!({
                "state": if capture.exit_code == Some(0) { "passed" } else { "failed" },
                "tool": chrome.to_string_lossy(),
                "exit_code": capture.exit_code,
                "network": "host resolver maps every host to ~NOTFOUND",
                "dom_contains_title": String::from_utf8_lossy(&capture.stdout).contains("ARCHMELD offline review"),
                "stdout_digest": hash::digest_bytes(&capture.stdout),
                "stderr_digest": hash::digest_bytes(&capture.stderr),
            }),
            Err(error) => json!({"state": "failed", "reason": error.to_string()}),
        }
    });
    if let Some(browser) = &browser {
        assert_eq!(browser["state"], "passed", "{browser}");
        assert_eq!(browser["dom_contains_title"], true);
    }

    // Every attachment is checksummed: tampering is detected.
    let html_path = bundle_dir.join("report.html");
    let mut tampered = fs::read(&html_path).unwrap();
    tampered.extend_from_slice(b"\n<!-- tampered -->\n");
    fs::write(&html_path, tampered).unwrap();
    let tamper_error = verify_bundle(&bundle_dir, &bundle["manifest"]).unwrap_err();
    assert!(tamper_error.to_string().contains("manifest digest"), "{tamper_error}");

    // Promotion keeps the same project identity, snapshot, claim, contract and
    // approved plan; it does not call a model or rewrite records.
    let mut promote_fixture = common::setup("b027-promote");
    let (_, promote_claim_id) = seed_evidence_and_claim(&mut promote_fixture);
    let plan_id = "plan.promote.b027";
    common::approved_plan(&mut promote_fixture, plan_id, &["src/a.py"]);
    common::create_changeset(&mut promote_fixture, "changeset.promote.b027", plan_id, &[]);
    common::prepare(
        &mut promote_fixture,
        "changeset.promote.b027",
        1,
        None,
        "promotion-seed",
    )
    .unwrap();
    let before_project = get_project(&promote_fixture.store, &promote_fixture.project).unwrap().unwrap();
    let before_snapshot = promote_fixture
        .store
        .snapshot_payload(&promote_fixture.project, &promote_fixture.snapshot_id)
        .unwrap()
        .unwrap();
    let before_claim = get_claim(&promote_fixture.store, &promote_fixture.project, &promote_claim_id).unwrap();
    let before_architecture = get_architecture(
        &promote_fixture.store,
        &promote_fixture.project,
        &promote_fixture.architecture_id,
    )
    .unwrap();
    let before_contract = get_contract(
        &promote_fixture.store,
        &promote_fixture.project,
        &promote_fixture.contract_id,
    )
    .unwrap();
    let before_plan = get_plan(&promote_fixture.store, &promote_fixture.project, plan_id).unwrap();
    assert_eq!(before_plan["status"], "approved");
    let promoted = promote_project(&mut promote_fixture.store, &promote_fixture.project).unwrap();
    assert_eq!(promoted["id"], before_project["id"]);
    assert_eq!(promoted["mode"], "manager");
    assert_eq!(promoted["state"], "open");
    assert_eq!(promoted["promotion"]["records_rewritten"], 0);
    assert_eq!(promoted["promotion"]["model_calls"], 0);
    assert_eq!(promoted["promotion"]["source_session_preserved"], true);
    assert_eq!(promoted["promotion"]["object_validation"]["missing"], json!([]));
    assert_eq!(promoted["promotion"]["object_validation"]["hash_mismatches"], json!([]));
    assert_eq!(
        promoted["promotion"]["counts_before"],
        promoted["promotion"]["counts_after"]
    );
    assert!(promoted["promotion"]["object_validation"]["checked_objects"]
        .as_u64()
        .unwrap()
        > 0);
    let after_snapshot = promote_fixture
        .store
        .snapshot_payload(&promote_fixture.project, &promote_fixture.snapshot_id)
        .unwrap()
        .unwrap();
    assert_eq!(before_snapshot, after_snapshot);
    assert_eq!(
        before_claim,
        get_claim(&promote_fixture.store, &promote_fixture.project, &promote_claim_id).unwrap()
    );
    assert_eq!(
        before_architecture,
        get_architecture(
            &promote_fixture.store,
            &promote_fixture.project,
            &promote_fixture.architecture_id,
        )
        .unwrap()
    );
    assert_eq!(
        before_contract,
        get_contract(
            &promote_fixture.store,
            &promote_fixture.project,
            &promote_fixture.contract_id,
        )
        .unwrap()
    );
    assert_eq!(
        before_plan,
        get_plan(&promote_fixture.store, &promote_fixture.project, plan_id).unwrap()
    );
    let sessions = list_sessions(&promote_fixture.store, &promote_fixture.project).unwrap();
    assert!(sessions.iter().any(|session| session.state == "promoted"));
    assert!(sessions
        .iter()
        .any(|session| session.mode == "manager" && session.state == "open"));
    let repeated = promote_project(&mut promote_fixture.store, &promote_fixture.project).unwrap_err();
    assert!(repeated.to_string().contains("already a Manager"), "{repeated}");

    // A missing object blocks promotion before the mode changes; the source
    // session and its readable records remain available.
    let mut broken = common::setup("b027-broken");
    let (broken_evidence, broken_claim) = seed_evidence_and_claim(&mut broken);
    let manifest_artifact = broken
        .store
        .snapshot_manifest_digest(&broken.project, &broken.snapshot_id)
        .unwrap()
        .unwrap();
    let hex = manifest_artifact.strip_prefix("sha256:").unwrap();
    let object_path = broken
        .store
        .root()
        .join("objects")
        .join("sha256")
        .join(&hex[..2])
        .join(&hex[2..]);
    fs::remove_file(&object_path).unwrap();
    let failed = promote_project(&mut broken.store, &broken.project).unwrap_err();
    assert!(failed.to_string().contains("object_validation_failed"), "{failed}");
    let still = get_project(&broken.store, &broken.project).unwrap().unwrap();
    assert_eq!(still["mode"], "expedition");
    assert_eq!(still["state"], "open");
    assert!(get_claim(&broken.store, &broken.project, &broken_claim).is_ok());
    assert!(broken
        .store
        .snapshot_payload(&broken.project, &broken.snapshot_id)
        .unwrap()
        .is_some());
    let broken_sessions = list_sessions(&broken.store, &broken.project).unwrap();
    assert!(broken_sessions
        .iter()
        .any(|session| session.mode == "expedition" && session.state == "open"));

    write_b027_evidence(json!({
        "AT059": {
            "status": "passed",
            "bundle": {
                "report_id": bundle["report_id"],
                "bundle_digest": bundle["bundle_digest"],
                "files": files,
                "verified_files": verified["checked_files"],
                "offline": true,
                "scripts": 0,
                "external_urls": 0,
                "source_included": false,
                "raw_traces_included": false,
                "source_marker_absent": true,
                "absolute_workspace_path_absent": true,
                "failed_check_state": report["verifications"][0]["checks"][0]["state"],
                "unknowns": report["unknowns"],
                "evidence_entries": report["evidence"].as_array().unwrap().len(),
                "tamper_detected": tamper_error.to_string(),
            },
            "offline_browser": browser,
            "verification_id": verification["verification_id"],
            "snapshot_id": fixture.snapshot_id,
            "evidence_id": evidence_id,
        },
        "AT060": {
            "status": "passed",
            "project_id": before_project["id"],
            "mode_before": before_project["mode"],
            "mode_after": promoted["mode"],
            "identity_preserved": promoted["id"] == before_project["id"],
            "snapshot_preserved": before_snapshot == after_snapshot,
            "claim_preserved": before_claim == get_claim(&promote_fixture.store, &promote_fixture.project, &promote_claim_id).unwrap(),
            "contract_preserved": before_contract["digest"] == get_contract(&promote_fixture.store, &promote_fixture.project, &promote_fixture.contract_id).unwrap()["digest"],
            "plan_status": before_plan["status"],
            "object_validation": promoted["promotion"]["object_validation"],
            "counts_before": promoted["promotion"]["counts_before"],
            "counts_after": promoted["promotion"]["counts_after"],
            "records_rewritten": promoted["promotion"]["records_rewritten"],
            "model_calls": promoted["promotion"]["model_calls"],
            "sessions": sessions.iter().map(|session| json!({
                "mode": session.mode,
                "state": session.state,
            })).collect::<Vec<_>>(),
            "broken_evidence_id": broken_evidence,
            "broken_claim_still_readable": true,
            "broken_promotion_error": failed.to_string(),
            "broken_project_mode": still["mode"],
        }
    }));
}

fn write_b027_evidence(value: Value) {
    if std::env::var("ARCHMELD_WRITE_EVIDENCE").ok().as_deref() != Some("1") {
        return;
    }
    let _guard = EVIDENCE_LOCK.lock().unwrap();
    let path =
        Path::new(env!("CARGO_MANIFEST_DIR")).join("../../docs/evidence/B027-report-promotion.json");
    let mut root = if path.exists() {
        serde_json::from_str::<Value>(&fs::read_to_string(&path).unwrap())
            .unwrap_or_else(|_| json!({}))
    } else {
        json!({
            "generated_by": "crates/archmeld-core/tests/offline_report.rs",
            "platform": std::env::consts::OS,
            "acceptance": {},
        })
    };
    let existing_http = root["acceptance"]["AT059"]["http_route"].clone();
    let mut at59 = value["AT059"].clone();
    if !existing_http.is_null() {
        at59["http_route"] = existing_http;
    }
    root["acceptance"]["AT059"] = at59;
    root["acceptance"]["AT060"] = value["AT060"].clone();
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(path, serde_json::to_string_pretty(&root).unwrap() + "\n").unwrap();
}

#[test]
fn opted_in_source_trace_export_is_labelled_and_verified() {
    let mut fixture = common::setup("b027-opt-in");
    seed_evidence_and_claim(&mut fixture);
    let trace = json!({
        "version": "1.0",
        "snapshot_id": fixture.snapshot_id,
        "config_digest": fixture.config_digest,
        "build_identity": {"kind": "normalized", "variant": "report"},
        "capture": {"tool": "offline", "version": "1", "lossy": false, "loss_notes": []},
        "clock": {"kind": "monotonic", "unit": "ns", "origin_ns": "0"},
        "probes": [],
        "spans": [{
            "span_id": "span.report.one",
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
    import_trace(
        &mut fixture.store,
        &RuntimeImport {
            project_id: &fixture.project,
            run_id: "run.report.trace",
            snapshot_id: Some(&fixture.snapshot_id),
            config_digest: &fixture.config_digest,
            allow_unbound: false,
            scenario_run_id: None,
        },
        &trace,
    )
    .unwrap();
    let native = b"raw trace bytes that must travel with the opt-in bundle".to_vec();
    let native_digest = fixture
        .store
        .put_blob(&native, "application/octet-stream")
        .unwrap();
    fixture
        .store
        .reference_artifact("runtime_native", "run.report.trace", &native_digest)
        .unwrap();

    let bundle_dir = fixture._dir.path().join("opt-in-report");
    let bundle = export_report(
        &mut fixture.store,
        &json!({
            "project_id": fixture.project,
            "snapshot_id": fixture.snapshot_id,
            "output_dir": bundle_dir.to_string_lossy(),
            "include_source": true,
            "include_raw_traces": true,
        }),
    )
    .unwrap();
    assert_eq!(bundle["source_state"], "included");
    assert_eq!(bundle["raw_trace_state"], "included");
    assert_eq!(bundle["risk_level"], "high");
    assert_eq!(bundle["manifest"]["source_file_count"], 5);
    assert_eq!(bundle["manifest"]["trace_file_count"], 2);
    assert_eq!(bundle["manifest"]["risk_level"], "high");
    assert_eq!(
        fs::read_to_string(bundle_dir.join("source/src/a.py")).unwrap(),
        "VALUE = 1\n"
    );
    let native_hex = native_digest.strip_prefix("sha256:").unwrap();
    let trace_path = format!("traces/sha256/{native_hex}.bin");
    assert_eq!(fs::read(bundle_dir.join(&trace_path)).unwrap(), native);
    assert!(bundle["manifest"]["files"]
        .as_array()
        .unwrap()
        .iter()
        .any(|file| file["kind"] == "raw_trace"
            && file["path"].as_str().unwrap_or("").ends_with(".json")));
    let verified = verify_bundle(&bundle_dir, &bundle["manifest"]).unwrap();
    assert_eq!(
        verified["checked_files"],
        bundle["manifest"]["files"].as_array().unwrap().len()
    );

    let report: Value =
        serde_json::from_slice(&fs::read(bundle_dir.join("report.json")).unwrap()).unwrap();
    assert_eq!(report["source_policy"]["source_state"], "included");
    assert_eq!(report["source_policy"]["raw_trace_state"], "included");
    assert_eq!(
        report["source_policy"]["secrets_included"],
        "unknown: no secret scanner ran"
    );
    assert_eq!(report["viewer"]["scripts"], 0);
    assert_eq!(report["viewer"]["external_urls"], 0);
    assert!(!report.to_string().contains("raw trace bytes that must travel"));

    let html = fs::read_to_string(bundle_dir.join("report.html")).unwrap();
    let lower = html.to_lowercase();
    assert!(!lower.contains("<script"));
    assert!(!lower.contains("http://"));
    assert!(!lower.contains("https://"));
    assert!(html.contains("Sensitive bundle"));
    assert!(html.contains("Graph view"));
    assert!(html.contains("id=\"entity-"));
    assert!(html.contains("href=\"#entity-"));
    assert!(html.contains(":target"));
    assert!(html.contains("source/src/a.py"));
    assert!(html.contains("raw trace bytes that must travel") == false);

    let mut tampered = fs::read(bundle_dir.join("source/src/a.py")).unwrap();
    tampered.extend_from_slice(b"\n# tampered\n");
    fs::write(bundle_dir.join("source/src/a.py"), tampered).unwrap();
    let error = verify_bundle(&bundle_dir, &bundle["manifest"]).unwrap_err();
    assert!(error.to_string().contains("manifest digest"), "{error}");

    if std::env::var("ARCHMELD_WRITE_EVIDENCE").ok().as_deref() == Some("1") {
        let path = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../docs/release-evidence/B027-opt-in-export.json");
        let evidence = json!({
            "schema_version": "1.0",
            "generated_by": "crates/archmeld-core/tests/offline_report.rs",
            "platform": std::env::consts::OS,
            "bundle_digest": bundle["bundle_digest"],
            "manifest": bundle["manifest"],
            "source_policy": report["source_policy"],
            "risk": report["risk"],
            "viewer": report["viewer"],
            "verified_checked_files": verified["checked_files"],
            "html": {
                "scripts": 0,
                "external_urls": 0,
                "sensitive_banner": true,
                "graph_view": true,
                "anchor_links": true,
            },
            "tamper": {
                "detected": true,
                "error": error.to_string(),
            },
        });
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, serde_json::to_string_pretty(&evidence).unwrap() + "\n").unwrap();
    }
}
