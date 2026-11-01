// Copyright 2026 ARCHMELD contributors
// SPDX-License-Identifier: Apache-2.0

//! B028 / AT062: loopback-only HTTP service, Host/Origin rejection, bearer-only
//! tokens and no token material in URLs or stderr.
//!
//! The test speaks raw HTTP so it can send the hostile Host/Origin values that a
//! browser or DNS-rebinding page would send.

use serde_json::{json, Value};
use std::fs;
use std::io::{BufRead, BufReader, Read, Write};
use std::net::TcpStream;
use std::path::Path;
use std::process::{Child, Command, Stdio};
use std::time::Duration;

struct Server {
    child: Child,
    endpoint: String,
    token: Option<String>,
    stderr: std::path::PathBuf,
}

impl Drop for Server {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

fn start(state: &Path, token: bool) -> Server {
    let stderr = state.join("server.stderr.log");
    let mut command = Command::new(env!("CARGO_BIN_EXE_archmeld-serve"));
    command
        .arg("--state")
        .arg(state)
        .args(["--port", "0"])
        .stdout(Stdio::piped())
        .stderr(Stdio::from(fs::File::create(&stderr).unwrap()));
    if token {
        command.args(["--token", "b028-secret-token"]);
    } else {
        command.arg("--no-auth");
    }
    let mut child = command.spawn().expect("spawn server");
    let stdout = child.stdout.take().expect("stdout");
    let mut reader = BufReader::new(stdout);
    let mut line = String::new();
    reader.read_line(&mut line).expect("announcement");
    let announcement: Value = serde_json::from_str(line.trim()).expect("announcement json");
    Server {
        child,
        endpoint: announcement["endpoint"].as_str().unwrap().to_string(),
        token: announcement["token"].as_str().map(str::to_string),
        stderr,
    }
}

fn request(
    server: &Server,
    method: &str,
    path: &str,
    host: Option<&str>,
    origin: Option<&str>,
    authorization: Option<&str>,
) -> (u16, Value) {
    let address = server.endpoint.trim_start_matches("http://");
    let host = host.unwrap_or(address);
    let mut stream = TcpStream::connect(address).expect("connect");
    stream
        .set_read_timeout(Some(Duration::from_secs(5)))
        .unwrap();
    let mut headers = format!(
        "{method} {path} HTTP/1.1\r\nHost: {host}\r\nConnection: close\r\nContent-Type: application/json\r\nContent-Length: 0\r\n"
    );
    if let Some(origin) = origin {
        headers.push_str(&format!("Origin: {origin}\r\n"));
    }
    if let Some(authorization) = authorization {
        headers.push_str(&format!("Authorization: Bearer {authorization}\r\n"));
    }
    headers.push_str("\r\n");
    stream.write_all(headers.as_bytes()).expect("write");
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
    let value = serde_json::from_str(&body).unwrap_or_else(|_| json!({"raw": body}));
    (status, value)
}

#[test]
fn local_service_rejects_foreign_hosts_origins_and_url_tokens() {
    let dir = tempfile::tempdir().unwrap();
    let state = dir.path().join("state");
    fs::create_dir_all(&state).unwrap();

    let no_auth = start(&state, false);
    assert!(
        no_auth.endpoint.starts_with("http://127.0.0.1:"),
        "{}",
        no_auth.endpoint
    );
    let (status, _) = request(&no_auth, "GET", "/v1/health", None, None, None);
    assert_eq!(status, 200);
    let (status, _) = request(
        &no_auth,
        "GET",
        "/v1/health",
        Some("localhost:1"),
        None,
        None,
    );
    assert_eq!(status, 200, "loopback localhost is allowed");
    let (status, body) = request(
        &no_auth,
        "GET",
        "/v1/health",
        Some("evil.example:80"),
        None,
        None,
    );
    assert_eq!(status, 403, "{body}");
    assert!(body.to_string().contains("permission_denied"), "{body}");
    let (status, body) = request(
        &no_auth,
        "GET",
        "/v1/health",
        None,
        Some("https://evil.example"),
        None,
    );
    assert_eq!(status, 403, "{body}");
    assert!(body.to_string().contains("Origin"), "{body}");
    let (status, body) = request(
        &no_auth,
        "POST",
        "/v1/query",
        None,
        Some("null"),
        None,
    );
    assert_eq!(status, 403, "{body}");
    drop(no_auth);

    let token_server = start(&state, true);
    assert_eq!(token_server.token.as_deref(), Some("b028-secret-token"));
    assert!(
        !token_server.endpoint.contains("b028-secret-token"),
        "token must not appear in the URL"
    );
    let (status, _) = request(&token_server, "GET", "/v1/health", None, None, None);
    assert_eq!(status, 200, "health stays available for readiness probes");
    let (status, body) = request(
        &token_server,
        "GET",
        "/v1/projects/missing?token=b028-secret-token",
        None,
        None,
        None,
    );
    assert_eq!(status, 403, "{body}");
    assert!(body.to_string().contains("permission_denied"), "{body}");
    let (status, body) = request(
        &token_server,
        "GET",
        "/v1/projects/missing",
        None,
        Some("http://127.0.0.1:1234"),
        Some("b028-secret-token"),
    );
    assert_eq!(status, 403, "{body}");
    let (status, body) = request(
        &token_server,
        "GET",
        "/v1/projects/missing",
        Some("evil.example:80"),
        None,
        Some("b028-secret-token"),
    );
    assert_eq!(status, 403, "{body}");
    let (status, body) = request(
        &token_server,
        "GET",
        "/v1/projects/missing",
        None,
        None,
        Some("b028-secret-token"),
    );
    assert_eq!(status, 404, "{body}");
    let (status, body) = request(
        &token_server,
        "GET",
        "/v1/projects/missing",
        None,
        None,
        Some("wrong-token"),
    );
    assert_eq!(status, 403, "{body}");
    let token = token_server.token.clone().unwrap();
    let stderr = fs::read_to_string(&token_server.stderr).unwrap_or_default();
    assert!(!stderr.contains(&token), "token leaked to stderr: {stderr}");
    let announcement_endpoint = token_server.endpoint.clone();
    assert!(!announcement_endpoint.contains(&token));
    drop(token_server);

    write_security_evidence(json!({
        "AT062": {
            "status": "passed",
            "loopback_only": true,
            "endpoint": announcement_endpoint,
            "foreign_host_rejected": true,
            "foreign_origin_rejected": true,
            "url_token_rejected": true,
            "wrong_token_rejected": true,
            "token_in_url": false,
            "token_in_stderr": false,
            "health_unauthenticated": true,
            "webview_csp": "checked separately by node --test and benchmarks/security-csp.js",
            "raw_transcript": [
                {"case": "foreign Host", "status": 403},
                {"case": "foreign Origin", "status": 403},
                {"case": "query token without Authorization", "status": 403},
                {"case": "wrong bearer", "status": 403},
                {"case": "valid bearer", "status": 404},
                {"case": "health on loopback", "status": 200}
            ],
            "retries": 0,
        }
    }));
}

fn write_security_evidence(value: Value) {
    if std::env::var("ARCHMELD_WRITE_EVIDENCE").ok().as_deref() != Some("1") {
        return;
    }
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../docs/evidence/B028-security.json");
    let mut root: Value = if path.exists() {
        serde_json::from_str(&fs::read_to_string(&path).unwrap()).unwrap_or_else(|_| json!({}))
    } else {
        json!({
            "generated_by": "crates/archmeld-api/tests/security_http.rs",
            "platform": std::env::consts::OS,
            "acceptance": {},
        })
    };
    for (key, item) in value.as_object().cloned().unwrap_or_default() {
        root["acceptance"][key] = item;
    }
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(path, serde_json::to_string_pretty(&root).unwrap() + "\n").unwrap();
}
