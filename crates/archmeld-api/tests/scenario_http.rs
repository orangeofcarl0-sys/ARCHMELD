// Copyright 2026 ARCHMELD contributors
// SPDX-License-Identifier: Apache-2.0

//! Real HTTP test for B016 scenario jobs: explicit grant, authorized workspace
//! cwd, persisted run, and refusal without a grant.

use archmeld_core::hash;
use archmeld_core::snapshot::{capture_snapshot, SnapshotInputs, SnapshotOptions};
use archmeld_core::store::Store;
use serde_json::{json, Value};
use std::fs;
use std::io::{BufRead, BufReader, Read, Write};
use std::net::TcpStream;
use std::path::Path;
use std::process::{Child, Command, Stdio};

const PROJECT: &str = "project.http.scenario";

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

fn start(state: &Path, workspace: &Path) -> Server {
    let mut child = Command::new(env!("CARGO_BIN_EXE_archmeld-serve"))
        .arg("--state")
        .arg(state)
        .args(["--port", "0", "--no-auth", "--allow-workspace"])
        .arg(workspace)
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .expect("spawn server");
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
        if size == 0 || tail.len() < size {
            break;
        }
        out.push_str(&tail[..size]);
        rest = tail[size..].trim_start_matches("\r\n");
    }
    out
}

#[test]
fn scenario_http_requires_grant_and_persists_the_run() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().join("workspace");
    fs::create_dir_all(&root).unwrap();
    fs::write(root.join("app.py"), "print('hello')\n").unwrap();
    let state = dir.path().join("state");
    let mut store = Store::open(&state).unwrap();
    store.ensure_project(PROJECT, "expedition").unwrap();
    let snapshot = capture_snapshot(
        &mut store,
        PROJECT,
        &root,
        &SnapshotInputs {
            build_config: json!({"variant": "scenario-http"}),
            environment: json!({"os": "test"}),
        },
        &SnapshotOptions {
            include_untracked: true,
            ..SnapshotOptions::default()
        },
    )
    .unwrap();
    drop(store);
    let canonical = root.canonicalize().unwrap();
    let digest = hash::digest_bytes(canonical.to_string_lossy().as_bytes());
    let handle = format!("ws.{}", &digest.trim_start_matches("sha256:")[..16]);
    let server = start(&state, &canonical);
    let body = json!({
        "project_id": PROJECT,
        "snapshot_id": snapshot.snapshot_id,
        "workspace_handle": handle,
        "cwd": canonical.to_string_lossy(),
        "argv": ["python", "-c", "print(7)"],
        "timeout_ms": 10000,
        "execution_grant": {"granted": true, "grant_id": "grant.http.test", "reason": "http test"},
        "run_id": "run.http.scenario.1",
    });
    let (status, run) = http(&server, "POST", "/v1/scenarios", Some(&body));
    assert_eq!(status, 200, "{run}");
    assert_eq!(run["completion"], "succeeded");
    assert_eq!(run["exit_code"], 0);
    assert!(run["stdout"]["digest"].as_str().unwrap().starts_with("sha256:"));
    let (status, fetched) = http(
        &server,
        "GET",
        "/v1/scenarios/run.http.scenario.1?project_id=project.http.scenario",
        None,
    );
    assert_eq!(status, 200, "{fetched}");
    assert_eq!(fetched["run_id"], "run.http.scenario.1");

    let mut denied = body.clone();
    denied["execution_grant"]["granted"] = json!(false);
    denied["run_id"] = json!("run.http.scenario.denied");
    let (status, error) = http(&server, "POST", "/v1/scenarios", Some(&denied));
    assert_eq!(status, 403, "{error}");
    assert_eq!(error["code"], "permission_denied");

    let mut escaped = body.clone();
    escaped["cwd"] = json!(dir.path().parent().unwrap().to_string_lossy());
    escaped["run_id"] = json!("run.http.scenario.escaped");
    let (status, error) = http(&server, "POST", "/v1/scenarios", Some(&escaped));
    assert_eq!(status, 400, "{error}");
}

#[test]
fn scenario_http_sandbox_uses_the_snapshot_and_rejects_unknown_fields() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().join("workspace");
    fs::create_dir_all(&root).unwrap();
    fs::write(root.join("app.py"), "print('hello')\n").unwrap();
    let state = dir.path().join("state");
    let mut store = Store::open(&state).unwrap();
    store.ensure_project(PROJECT, "expedition").unwrap();
    let snapshot = capture_snapshot(
        &mut store,
        PROJECT,
        &root,
        &SnapshotInputs {
            build_config: json!({"variant": "scenario-http-sandbox"}),
            environment: json!({"os": "test"}),
        },
        &SnapshotOptions {
            include_untracked: true,
            ..SnapshotOptions::default()
        },
    )
    .unwrap();
    drop(store);
    // The live workspace moves on; the sandbox must still read the snapshot.
    fs::write(root.join("app.py"), "print('live')\n").unwrap();
    let canonical = root.canonicalize().unwrap();
    let digest = hash::digest_bytes(canonical.to_string_lossy().as_bytes());
    let handle = format!("ws.{}", &digest.trim_start_matches("sha256:")[..16]);
    let server = start(&state, &canonical);
    let body = json!({
        "project_id": PROJECT,
        "snapshot_id": snapshot.snapshot_id,
        "workspace_handle": handle,
        "cwd": canonical.to_string_lossy(),
        "argv": ["python", "-c", "import pathlib; assert pathlib.Path('app.py').read_text() == \"print('hello')\\n\"; print('http-copy-ok')"],
        "timeout_ms": 10000,
        "execution_grant": {"granted": true, "grant_id": "grant.http.sandbox", "reason": "http sandbox test"},
        "run_id": "run.http.scenario.sandbox",
        "sandbox": {"mode": "workspace_copy", "include": ["app.py"], "exclude": []},
    });
    let (status, run) = http(&server, "POST", "/v1/scenarios", Some(&body));
    assert_eq!(status, 200, "{run}");
    assert_eq!(run["completion"], "succeeded", "{run}");
    assert_eq!(run["sandbox"]["mode"], "workspace_copy");
    assert_eq!(run["sandbox"]["isolated"], true);
    assert_eq!(run["sandbox"]["file_count"], 1);
    assert_eq!(run["sandbox"]["retained"], false);
    assert_eq!(
        fs::read_to_string(root.join("app.py")).unwrap(),
        "print('live')\n"
    );

    let mut unknown = body.clone();
    unknown["run_id"] = json!("run.http.scenario.unknown");
    unknown["sandbox"]["api_key"] = json!("must-not-be-accepted");
    let (status, error) = http(&server, "POST", "/v1/scenarios", Some(&unknown));
    assert_eq!(status, 400, "{error}");
    assert_eq!(error["code"], "invalid_request");

    let mut unpinned = body.clone();
    unpinned["run_id"] = json!("run.http.scenario.unpinned");
    unpinned["sandbox"] = json!({
        "mode": "container",
        "include": ["app.py"],
        "exclude": [],
        "image": "debian:latest",
        "network": "none",
    });
    let (status, error) = http(&server, "POST", "/v1/scenarios", Some(&unpinned));
    assert_eq!(status, 400, "{error}");
    assert!(
        error["message"].as_str().unwrap_or("").contains("digest-pinned"),
        "{error}"
    );
}
