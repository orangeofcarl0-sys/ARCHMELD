// Copyright 2026 ARCHMELD contributors
// SPDX-License-Identifier: Apache-2.0

//! Real HTTP test for the B027 offline report route. The server owns the output
//! directory; a caller can only choose a safe bundle name, never an arbitrary
//! filesystem path.

use serde_json::{json, Value};
use std::fs;
use std::io::{BufRead, BufReader, Read, Write};
use std::net::TcpStream;
use std::path::Path;
use std::process::{Child, Command, Stdio};
use archmeld_core::snapshot::{capture_snapshot, SnapshotInputs, SnapshotOptions};
use archmeld_core::store::Store;

const PROJECT: &str = "project.http.b027";

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
    let mut child = Command::new(env!("CARGO_BIN_EXE_archmeld-serve"))
        .arg("--state")
        .arg(state)
        .args(["--port", "0", "--no-auth"])
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
fn report_route_owns_the_output_directory_and_rejects_unsafe_names() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().join("repo");
    fs::create_dir_all(&root).unwrap();
    fs::write(root.join("a.py"), "VALUE = 1\n").unwrap();
    let state = dir.path().join("state");
    let mut store = Store::open(&state).unwrap();
    store.ensure_project(PROJECT, "expedition").unwrap();
    let snapshot = capture_snapshot(
        &mut store,
        PROJECT,
        &root,
        &SnapshotInputs {
            build_config: json!({"variant": "b027-http"}),
            environment: json!({"os": "test"}),
        },
        &SnapshotOptions {
            include_untracked: true,
            ..SnapshotOptions::default()
        },
    )
    .unwrap();
    let snapshot_id = snapshot.snapshot_id.clone();
    drop(store);

    let server = start(&state);
    let (status, body) = http(
        &server,
        "POST",
        "/v1/reports",
        Some(&json!({
            "project_id": PROJECT,
            "snapshot_id": snapshot_id,
            "output_name": "review-b027",
        })),
    );
    assert_eq!(status, 200, "{body}");
    assert_eq!(body["offline"], true);
    assert_eq!(body["manifest"]["scripts"], 0);
    assert_eq!(body["manifest"]["external_urls"], 0);
    assert_eq!(body["source_included"], false);
    let destination = state.join("exports").join("review-b027");
    assert!(destination.join("report.html").is_file());
    assert!(destination.join("manifest.json").is_file());
    let manifest: Value =
        serde_json::from_slice(&fs::read(destination.join("manifest.json")).unwrap()).unwrap();
    assert_eq!(manifest["files"].as_array().unwrap().len(), 4);

    let (status, body) = http(
        &server,
        "POST",
        "/v1/reports",
        Some(&json!({
            "project_id": PROJECT,
            "snapshot_id": snapshot_id,
            "output_name": "../escape",
        })),
    );
    assert_eq!(status, 400, "{body}");
    assert!(body.to_string().contains("output_name"), "{body}");

    let (status, body) = http(
        &server,
        "POST",
        "/v1/reports",
        Some(&json!({
            "project_id": PROJECT,
            "snapshot_id": snapshot_id,
            "output_name": "with-source",
            "include_source": true,
        })),
    );
    assert_eq!(status, 200, "{body}");
    assert_eq!(body["source_included"], true);
    assert_eq!(body["source_state"], "included");
    assert_eq!(body["risk_level"], "high");
    assert_eq!(body["manifest"]["source_included"], true);
    assert_eq!(body["manifest"]["risk_level"], "high");
    assert!(body["manifest"]["risk_labels"]
        .as_array()
        .unwrap()
        .iter()
        .any(|label| label.as_str().unwrap_or("").contains("sensitive")));
    let source_bundle = state.join("exports").join("with-source");
    assert_eq!(
        fs::read_to_string(source_bundle.join("source/a.py")).unwrap(),
        "VALUE = 1\n"
    );
    let source_manifest: Value =
        serde_json::from_slice(&fs::read(source_bundle.join("manifest.json")).unwrap()).unwrap();
    assert_eq!(source_manifest["files"].as_array().unwrap().len(), 5);
    assert!(source_manifest["files"]
        .as_array()
        .unwrap()
        .iter()
        .any(|file| file["path"] == "source/a.py" && file["kind"] == "source"));
    let source_html = fs::read_to_string(source_bundle.join("report.html")).unwrap();
    assert!(source_html.contains("Sensitive bundle"));
    assert!(!source_html.to_lowercase().contains("<script"));
    assert!(!source_html.to_lowercase().contains("http://"));
    assert!(!source_html.to_lowercase().contains("https://"));

    let (status, body) = http(
        &server,
        "POST",
        "/v1/reports",
        Some(&json!({
            "project_id": PROJECT,
            "snapshot_id": snapshot_id,
            "output_name": "with-traces",
            "include_raw_traces": true,
        })),
    );
    assert_eq!(status, 200, "{body}");
    assert_eq!(body["raw_trace_state"], "none");
    assert_eq!(body["raw_traces_included"], false);

    if std::env::var("ARCHMELD_WRITE_EVIDENCE").ok().as_deref() == Some("1") {
        let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../docs/evidence/B027-report-promotion.json");
        let mut root: Value = if path.exists() {
            serde_json::from_str(&fs::read_to_string(&path).unwrap()).unwrap_or_else(|_| json!({}))
        } else {
            json!({})
        };
        root["acceptance"]["AT059"]["http_route"] = json!({
            "status": "passed",
            "route": "POST /v1/reports",
            "output_name": "review-b027",
            "output_owned_by_server": true,
            "files": manifest["files"],
            "unsafe_name_status": 400,
            "source_inclusion_status": 200,
            "source_directory_created": true,
            "source_state": "included",
            "risk_level": "high",
            "trace_request_without_runs_state": "none",
        });
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, serde_json::to_string_pretty(&root).unwrap() + "
").unwrap();
    }
}
