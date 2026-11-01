// Copyright 2026 ARCHMELD contributors
// SPDX-License-Identifier: Apache-2.0

//! Real HTTP tests for the B017 interpretation routes.

use archmeld_core::hash;
use archmeld_core::identity::line_entity_id;
use archmeld_core::snapshot::{capture_snapshot, SnapshotInputs, SnapshotOptions};
use archmeld_core::store::Store;
use serde_json::{json, Value};
use std::fs;
use std::io::{BufRead, BufReader, Read, Write};
use std::net::TcpStream;
use std::path::Path;
use std::process::{Child, Command, Stdio};

const PROJECT: &str = "project.http.b017";

struct Server {
    child: Child,
    endpoint: String,
}

impl Drop for Server {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

fn start(state: &Path) -> Server {
    let mut command = Command::new(env!("CARGO_BIN_EXE_archmeld-serve"));
    command
        .arg("--state")
        .arg(state)
        .args(["--port", "0", "--no-auth"])
        .stdout(Stdio::piped())
        .stderr(Stdio::null());
    let mut child = command.spawn().expect("spawn server");
    let stdout = child.stdout.take().expect("stdout");
    let mut reader = BufReader::new(stdout);
    let mut line = String::new();
    reader.read_line(&mut line).expect("announcement");
    let announcement: Value = serde_json::from_str(line.trim()).expect("announcement json");
    Server {
        child,
        endpoint: announcement["endpoint"].as_str().unwrap().to_string(),
    }
}

fn http(server: &Server, method: &str, path: &str, body: Option<&Value>) -> (u16, Value) {
    let address = server.endpoint.trim_start_matches("http://");
    let mut stream = TcpStream::connect(address).expect("connect");
    let payload = body.map(Value::to_string).unwrap_or_default();
    let request = format!(
        "{method} {path} HTTP/1.1\r\nHost: {address}\r\nConnection: close\r\nContent-Type: application/json\r\nContent-Length: {}\r\n\r\n{payload}",
        payload.len()
    );
    stream.write_all(request.as_bytes()).expect("write");
    let mut response = String::new();
    stream.read_to_string(&mut response).expect("read");
    let status = response
        .split_whitespace()
        .nth(1)
        .and_then(|code| code.parse().ok())
        .unwrap_or(0);
    let body = response
        .split_once("\r\n\r\n")
        .map(|(_, body)| body.to_string())
        .unwrap_or_default();
    let body = if response.contains("Transfer-Encoding: chunked") {
        decode_chunked(&body)
    } else {
        body
    };
    let value = serde_json::from_str(&body).unwrap_or_else(|_| json!({"raw": body}));
    (status, value)
}

fn decode_chunked(body: &str) -> String {
    let mut out = String::new();
    let mut rest = body;
    loop {
        let Some((size_line, tail)) = rest.split_once("\r\n") else {
            break;
        };
        let size = usize::from_str_radix(size_line.trim(), 16).unwrap_or(0);
        if size == 0 {
            break;
        }
        if tail.len() < size {
            break;
        }
        out.push_str(&tail[..size]);
        rest = tail[size..].trim_start_matches("\r\n");
    }
    out
}

#[test]
fn claims_architectures_and_capabilities_are_real_http_routes() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().join("repo");
    fs::create_dir_all(&root).unwrap();
    fs::write(root.join("calc.py"), "def compute():\n    return 1\n").unwrap();
    let state = dir.path().join("state");
    let mut store = Store::open(&state).unwrap();
    store.ensure_project(PROJECT, "expedition").unwrap();
    let snap = capture_snapshot(
        &mut store,
        PROJECT,
        &root,
        &SnapshotInputs {
            build_config: json!({"variant": "b017-http"}),
            environment: json!({"os": "test"}),
        },
        &SnapshotOptions {
            include_untracked: true,
            ..SnapshotOptions::default()
        },
    )
    .unwrap();
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
    let evidence_id = "evidence.syntax.b017.http";
    let batch = json!({
        "schema_version": "1.0",
        "project_id": PROJECT,
        "snapshot_id": snap.snapshot_id,
        "config_digest": config_digest,
        "run_id": "run.b017.http",
        "adapter_id": "fixture.syntax",
        "adapter_version": "1",
        "sequence": "0",
        "final": true,
        "coverage": {
            "state": "complete",
            "scope": "b017 http fixture",
            "expected_count": 1,
            "observed_count": 1,
            "omitted_count": 0,
            "unresolved_count": 0,
            "limitations": [],
        },
        "entities": [{
            "id": entity_id,
            "project_id": PROJECT,
            "snapshot_id": snap.snapshot_id,
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
            "project_id": PROJECT,
            "snapshot_id": snap.snapshot_id,
            "config_digest": config_digest,
            "kind": "syntax",
            "producer": {"name": "fixture.syntax", "version": "1", "source_ref": "line-anchor", "run_id": "run.b017.http"},
            "artifact_digest": file.blob_digest,
            "locator": "calc.py",
            "source_anchor": null,
            "scenario_id": null,
            "runtime_run_id": null,
            "coverage": {
                "state": "complete",
                "scope": "b017 http fixture",
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
    drop(store);

    let server = start(&state);
    let claim = json!({
        "project_id": PROJECT,
        "claim": {
            "id": "claim.http.b017",
            "project_id": PROJECT,
            "snapshot_id": snap.snapshot_id,
            "claim_type": "hypothesis",
            "text": "compute dominates",
            "status": "proposed",
            "support_ids": [],
            "counterevidence_ids": [],
            "scope": "scenario.http",
            "producer": {"name": "human", "version": "manual-1", "source_ref": "operator-note", "run_id": "run.http.manual"},
            "unresolved_questions": [],
        },
        "expected_revision": 1,
        "idempotency_key": "http-claim-1",
    });
    let (status, body) = http(&server, "POST", "/v1/claims", Some(&claim));
    assert_eq!(status, 200, "{body}");
    assert_eq!(body["status"], "proposed");

    let (status, body) = http(
        &server,
        "GET",
        &format!("/v1/claims/claim.http.b017?project_id={PROJECT}"),
        None,
    );
    assert_eq!(status, 200, "{body}");
    assert_eq!(body["text"], "compute dominates");

    let review = json!({
        "project_id": PROJECT,
        "expected_revision": 1,
        "status": "rejected",
        "reason": "counterexample: queue wait dominates",
        "actor": {"name": "operator"},
        "counterevidence_ids": [evidence_id],
        "idempotency_key": "http-claim-review-1",
    });
    let (status, body) = http(
        &server,
        "POST",
        "/v1/claims/claim.http.b017/review",
        Some(&review),
    );
    assert_eq!(status, 200, "{body}");
    assert_eq!(body["status"], "rejected");
    assert_eq!(body["counterevidence_ids"][0], evidence_id);

    let architecture = json!({
        "schema_version": "1.0",
        "id": "arch.http.b017",
        "project_id": PROJECT,
        "snapshot_id": snap.snapshot_id,
        "revision": 1,
        "layer": "as_is",
        "status": "candidate",
        "components": [{
            "id": "component.calc",
            "name": "Calculation",
            "purpose": "Computes the fixture value.",
            "responsibilities": ["compute"],
            "non_responsibilities": [],
            "member_entity_ids": [entity_id],
            "owned_state": [],
            "owned_resources": [],
            "runtime_placement": [],
            "evidence_ids": [evidence_id],
            "open_questions": [],
        }],
        "dependencies": [],
        "rules": [],
        "claim_ids": ["claim.http.b017"],
    });
    let proposal = json!({
        "project_id": PROJECT,
        "model": architecture,
        "expected_revision": 1,
        "idempotency_key": "http-arch-1",
    });
    let (status, body) = http(&server, "POST", "/v1/architectures", Some(&proposal));
    assert_eq!(status, 200, "{body}");
    assert_eq!(body["status"], "candidate");
    let stored_architecture = body.clone();

    let (status, body) = http(
        &server,
        "GET",
        &format!("/v1/architectures/arch.http.b017/review?project_id={PROJECT}"),
        None,
    );
    assert_eq!(status, 200, "{body}");
    assert_eq!(body["candidate_only"], true);
    assert_eq!(body["certain_calls_added"], 0);
    assert_eq!(body["member_entities"]["matched"], 1);

    let mut target = stored_architecture;
    target["id"] = json!("arch.http.target");
    target["layer"] = json!("to_be");
    target["revision"] = json!(1);
    target["dependencies"] = json!([]);
    target["rules"] = json!([]);
    let (status, diff) = http(
        &server,
        "POST",
        "/v1/architectures/arch.http.b017/diff",
        Some(&json!({"project_id": PROJECT, "target": target})),
    );
    assert_eq!(status, 200, "{diff}");
    assert_eq!(diff["source_mutation"]["state"], "none");
    assert_eq!(diff["source_mutation"]["files_written"], 0);
    let (status, checks) = http(
        &server,
        "POST",
        "/v1/architectures/arch.http.b017/check",
        Some(&json!({"project_id": PROJECT})),
    );
    assert_eq!(status, 200, "{checks}");
    assert_eq!(checks["results"].as_array().unwrap().len(), 0);
    assert_eq!(checks["state"], "unverified");

    let (status, body) = http(&server, "GET", "/v1/capabilities", None);
    assert_eq!(status, 200, "{body}");
    assert_eq!(body["model"]["state"], "unconfigured");
    assert_eq!(body["manual_modeling"]["state"], "available");

    let mut contract = json!({
        "schema_version": "1.0",
        "id": "contract.http.b019",
        "project_id": PROJECT,
        "revision": 1,
        "kind": "behavior_fix",
        "invariants": ["output stays finite"],
        "allowed_changes": ["internal refactor"],
        "known_defects": [],
        "checks": [{
            "id": "check.tests",
            "kind": "behavior",
            "required": true,
            "description": "tests pass",
            "command": {"argv": ["python", "-m", "pytest"], "cwd": ".", "timeout_ms": 1000},
            "checker_id": null,
            "predicate": {"kind": "exit_zero"},
            "input_artifacts": [],
            "sandbox": {"mode": "workspace", "include": [], "exclude": []},
        }],
        "digest": format!("sha256:{}", "0".repeat(64)),
        "frozen": false,
    });
    let mut preimage = contract.clone();
    preimage.as_object_mut().unwrap().remove("digest");
    preimage.as_object_mut().unwrap().remove("frozen");
    contract["digest"] = json!(hash::digest_value(&preimage).unwrap());
    let (status, stored_contract) = http(
        &server,
        "POST",
        "/v1/contracts",
        Some(&json!({
            "project_id": PROJECT,
            "contract": contract,
            "expected_revision": 1,
            "idempotency_key": "http-contract-1",
        })),
    );
    assert_eq!(status, 200, "{stored_contract}");
    assert_eq!(stored_contract["frozen"], true);

    let plan = json!({
        "schema_version": "1.0",
        "id": "plan.http.b019",
        "project_id": PROJECT,
        "revision": 1,
        "kind": "behavior_fix",
        "objective": "fix the fixture",
        "non_goals": [],
        "base_snapshot_id": snap.snapshot_id,
        "target_architecture_id": "arch.http.b017",
        "contract_id": "contract.http.b019",
        "contract_digest": stored_contract["digest"],
        "steps": [{
            "id": "step.1",
            "title": "fix",
            "depends_on": [],
            "allowed_paths": ["calc.py"],
            "required_check_ids": ["check.tests"],
            "completion_criteria": ["tests pass"],
        }],
        "risks": [],
        "evidence_ids": [],
        "status": "draft",
    });
    let (status, stored_plan) = http(
        &server,
        "POST",
        "/v1/plans",
        Some(&json!({
            "project_id": PROJECT,
            "plan": plan,
            "expected_revision": 1,
            "idempotency_key": "http-plan-1",
        })),
    );
    assert_eq!(status, 200, "{stored_plan}");
    assert_eq!(stored_plan["status"], "draft");
    let (status, approved) = http(
        &server,
        "POST",
        "/v1/plans/plan.http.b019/approve",
        Some(&json!({
            "project_id": PROJECT,
            "expected_revision": 1,
            "contract_digest": stored_contract["digest"],
            "actor": "operator",
            "idempotency_key": "http-plan-approve-1",
        })),
    );
    assert_eq!(status, 200, "{approved}");
    assert_eq!(approved["status"], "approved");
    assert_eq!(approved["revision"], 2);

    let analysis = json!({
        "metadata": {"repo_name": "fixture", "depth_level": 1},
        "description": "Fixture architecture.",
        "components": [{
            "component_id": "1",
            "name": "Calculation",
            "description": "Computes the fixture value.",
            "key_entities": [{
                "qualified_name": "calc.py::compute",
                "reference_file": "calc.py",
                "reference_start_line": 1,
                "reference_end_line": 1
            }],
            "components": [],
            "components_relations": []
        }],
        "components_relations": []
    });
    let codeboarding = json!({
        "project_id": PROJECT,
        "snapshot_id": snap.snapshot_id,
        "expected_revision": 1,
        "idempotency_key": "http-codeboarding-1",
        "provider_version": "0.15.3",
        "run_id": "run.http.codeboarding",
        "commands": [],
        "model": {
            "provider": "opencode go orange",
            "model": "deepseek-v4.1-flash",
            "base_url_host": "opencode.ai",
            "calls": 1,
            "input_tokens": 10,
            "output_tokens": 5
        },
        "analysis": analysis,
    });
    let (status, imported) = http(
        &server,
        "POST",
        "/v1/architectures/codeboarding",
        Some(&codeboarding),
    );
    assert_eq!(status, 200, "{imported}");
    assert_eq!(imported["status"], "candidate");
    let mut rejected_model = codeboarding.clone();
    rejected_model["idempotency_key"] = json!("http-codeboarding-bad");
    rejected_model["model"]["api_key"] = json!("never-accepted");
    let (status, rejected) = http(
        &server,
        "POST",
        "/v1/architectures/codeboarding",
        Some(&rejected_model),
    );
    assert_eq!(status, 400, "{rejected}");
    drop(server);
    let store = Store::open(&state).unwrap();
    let producer = archmeld_core::interpret::architecture_producer(
        &store,
        PROJECT,
        imported["id"].as_str().unwrap(),
    )
    .unwrap();
    assert_eq!(producer["model"]["provider"], "opencode go orange");
    assert_eq!(producer["model"]["model"], "deepseek-v4.1-flash");
    assert_eq!(producer["model"]["base_url_host"], "opencode.ai");
    assert!(!producer.to_string().to_lowercase().contains("never-accepted"));
}
