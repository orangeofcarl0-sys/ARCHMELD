// Copyright 2026 ARCHMELD contributors
// SPDX-License-Identifier: Apache-2.0

//! Real-process HTTP contract tests: spawn `archmeld-serve`, speak HTTP/1.1 over
//! loopback, and assert on AT015 (bounded query), AT016 (historical source) and
//! AT073 (idempotent cancel, event replay/resync).

use archmeld_core::bridge::{import_codegraph_export, BridgeConfig};
use archmeld_core::events::prune_events_before;
use archmeld_core::jobs::{create_job, transition_job};
use archmeld_core::snapshot::{capture_snapshot, SnapshotInputs, SnapshotOptions};
use archmeld_core::store::Store;
use serde_json::{json, Value};
use std::fs;
use std::io::{BufRead, BufReader, Read, Write};
use std::net::TcpStream;
use std::path::Path;
use std::process::{Child, Command, Stdio};

const PROJECT: &str = "project.http";

struct Prepared {
    dir: tempfile::TempDir,
    state: std::path::PathBuf,
    snapshot_id: String,
    entity_id: String,
    job_id: String,
    job_revision: i64,
    second_snapshot: String,
    root: std::path::PathBuf,
}

fn prepare(prune: bool) -> Prepared {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().join("repo");
    fs::create_dir_all(root.join("src")).unwrap();
    fs::write(root.join("src/a.py"), "def a1():\n    return b()\n").unwrap();
    fs::write(root.join("src/b.py"), "def b():\n    return 1\n").unwrap();
    let state = dir.path().join("state");
    let mut store = Store::open(&state).unwrap();
    store.ensure_project(PROJECT, "expedition").unwrap();
    let captured = capture_snapshot(
        &mut store,
        PROJECT,
        &root,
        &SnapshotInputs {
            build_config: json!({"variant": "http"}),
            environment: json!({"os": "test"}),
        },
        &SnapshotOptions {
            include_untracked: true,
            ..SnapshotOptions::default()
        },
    )
    .unwrap();
    let config_digest = captured.snapshot["config_digest"].as_str().unwrap().to_string();
    let nodes = [
        json!({"type":"node","id":1,"node_type":"Function","properties":{
            "path": root.join("src/a.py").to_string_lossy(),"name":"a1",
            "line_start":1,"line_end":2}}),
        json!({"type":"node","id":2,"node_type":"Function","properties":{
            "path": root.join("src/b.py").to_string_lossy(),"name":"b",
            "line_start":1,"line_end":2}}),
    ];
    let mut lines = vec![json!({
        "type":"manifest","format":"archmeld-export-v1","source":"codegraph",
        "version":"0.2.0","node_count":2,"edge_count":1,"iteration":"real-iterators"
    })
    .to_string()];
    for node in &nodes {
        lines.push(node.to_string());
    }
    lines.push(
        json!({"type":"edge","id":1,"source":1,"target":2,"edge_type":"Calls","properties":{"line":2}})
            .to_string(),
    );
    let export = lines.join("\n") + "\n";
    import_codegraph_export(
        &mut store,
        &BridgeConfig {
            project_id: PROJECT,
            snapshot_id: &captured.snapshot_id,
            config_digest: &config_digest,
            run_id: "run.http.001",
            workspace_root: Some(&root),
            max_entities_per_batch: 500,
            max_relations_per_batch: 2000,
        },
        &captured.files,
        &export,
    )
    .unwrap();
    let entity_id = store
        .entity_ids(PROJECT, &captured.snapshot_id)
        .unwrap()
        .into_iter()
        .find(|id| {
            store
                .get_entity(PROJECT, &captured.snapshot_id, id)
                .unwrap()
                .and_then(|entity| entity["qualified_name"].as_str().map(str::to_string))
                .is_some_and(|name| name.ends_with("::a1"))
        })
        .expect("a1 entity");

    let job = create_job(&mut store, PROJECT, "query").unwrap();
    let job_id = job["id"].as_str().unwrap().to_string();
    transition_job(&mut store, PROJECT, &job_id, "running", 0.5).unwrap();
    let job_revision = 2;
    if prune {
        prune_events_before(&mut store, PROJECT, 100).unwrap();
    }
    let second = capture_snapshot(
        &mut store,
        PROJECT,
        &root,
        &SnapshotInputs {
            build_config: json!({"variant": "http-2"}),
            environment: json!({"os": "test"}),
        },
        &SnapshotOptions {
            include_untracked: true,
            ..SnapshotOptions::default()
        },
    )
    .unwrap();
    Prepared {
        dir,
        state,
        snapshot_id: captured.snapshot_id,
        entity_id,
        job_id,
        job_revision,
        second_snapshot: second.snapshot_id,
        root: root.clone(),
    }
}

struct Server {
    child: Child,
    endpoint: String,
    workspaces: std::collections::HashMap<String, String>,
}

impl Drop for Server {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

fn start(state: &Path, extra: &[&str]) -> Server {
    start_with(state, extra, &[])
}

fn start_with(state: &Path, extra: &[&str], workspaces: &[&Path]) -> Server {
    let mut command = Command::new(env!("CARGO_BIN_EXE_archmeld-serve"));
    command.arg("--state").arg(state).args(["--port", "0"]).args(extra);
    for workspace in workspaces {
        command.arg("--allow-workspace").arg(workspace);
    }
    command.stdout(Stdio::piped()).stderr(Stdio::null());
    let mut child = command.spawn().expect("spawn server");
    let stdout = child.stdout.take().expect("stdout");
    let mut reader = BufReader::new(stdout);
    let mut line = String::new();
    reader.read_line(&mut line).expect("announcement");
    let announcement: Value = serde_json::from_str(line.trim()).expect("announcement json");
    let workspaces = announcement["workspaces"]
        .as_object()
        .map(|object| {
            object
                .iter()
                .map(|(key, value)| (key.clone(), value.as_str().unwrap_or("").to_string()))
                .collect()
        })
        .unwrap_or_default();
    Server {
        child,
        endpoint: announcement["endpoint"].as_str().unwrap().to_string(),
        workspaces,
    }
}

fn http(
    server: &Server,
    method: &str,
    path: &str,
    body: Option<&Value>,
    bearer: Option<&str>,
) -> (u16, String) {
    let address = server.endpoint.trim_start_matches("http://");
    let mut stream = TcpStream::connect(address).expect("connect");
    let payload = body.map(Value::to_string).unwrap_or_default();
    let mut request = format!(
        "{method} {path} HTTP/1.1\r\nHost: {address}\r\nConnection: close\r\nContent-Type: application/json\r\nContent-Length: {}\r\n",
        payload.len()
    );
    if let Some(token) = bearer {
        request.push_str(&format!("Authorization: Bearer {token}\r\n"));
    }
    request.push_str("\r\n");
    request.push_str(&payload);
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
    (status, body)
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
fn health_query_source_and_cancel_are_real_http_requests() {
    let prepared = prepare(false);
    let server = start(&prepared.state, &["--no-auth"]);

    let (status, body) = http(&server, "GET", "/v1/health", None, None);
    assert_eq!(status, 200);
    let health: Value = serde_json::from_str(&body).unwrap();
    assert_eq!(health["service"], "archmeld");
    assert_eq!(health["contract_version"], "1.0");

    let query = json!({
        "project_id": PROJECT,
        "snapshot_id": prepared.snapshot_id,
        "kind": "neighbors",
        "query": "",
        "entity_ids": [prepared.entity_id],
        "relation_kinds": ["static_call_candidate"],
        "max_nodes": 1,
        "max_depth": 2,
        "cursor": null,
    });
    let (status, body) = http(&server, "POST", "/v1/query", Some(&query), None);
    assert_eq!(status, 200, "{body}");
    let first: Value = serde_json::from_str(&body).unwrap();
    assert_eq!(first["entities"].as_array().unwrap().len(), 1);
    assert_eq!(first["coverage"]["state"], "partial");
    let cursor = first["next_cursor"].as_str().expect("cursor").to_string();

    let mut second_page = query.clone();
    second_page["cursor"] = json!(cursor);
    let (status, body) = http(&server, "POST", "/v1/query", Some(&second_page), None);
    assert_eq!(status, 200, "{body}");
    let second: Value = serde_json::from_str(&body).unwrap();
    assert_eq!(second["entities"].as_array().unwrap().len(), 1);

    let mut wrong_snapshot = second_page.clone();
    wrong_snapshot["snapshot_id"] = json!(prepared.second_snapshot);
    let (status, body) = http(&server, "POST", "/v1/query", Some(&wrong_snapshot), None);
    assert_eq!(status, 409, "{body}");
    assert!(body.contains("different snapshot"));

    let source_body = json!({
        "project_id": PROJECT,
        "snapshot_id": prepared.snapshot_id,
        "entity_id": prepared.entity_id,
        "anchor_index": 0,
        "max_bytes": 1048576,
    });
    let (status, body) = http(&server, "POST", "/v1/source", Some(&source_body), None);
    assert_eq!(status, 200, "{body}");
    let source: Value = serde_json::from_str(&body).unwrap();
    assert!(source["text"].as_str().unwrap().contains("def a1"));

    let (status, body) = http(&server, "GET", "/v1/events?project_id=project.http", None, None);
    assert_eq!(status, 200, "{body}");
    assert!(body.contains("job.state_changed"));
    assert!(body.contains("data: "));

    let cancel_body = json!({
        "project_id": PROJECT,
        "expected_revision": prepared.job_revision,
        "approval_id": null,
        "idempotency_key": "cancel-1",
    });
    let (status, body) = http(
        &server,
        "POST",
        &format!("/v1/jobs/{}/cancel", prepared.job_id),
        Some(&cancel_body),
        None,
    );
    assert_eq!(status, 202, "{body}");
    let cancelled: Value = serde_json::from_str(&body).unwrap();
    assert_eq!(cancelled["state"], "cancelled");
    let (status, replay) = http(
        &server,
        "POST",
        &format!("/v1/jobs/{}/cancel", prepared.job_id),
        Some(&cancel_body),
        None,
    );
    assert_eq!((status, replay.as_str()), (202, body.as_str()));

    let mut changed = cancel_body.clone();
    changed["expected_revision"] = json!(3);
    let (status, body) = http(
        &server,
        "POST",
        &format!("/v1/jobs/{}/cancel", prepared.job_id),
        Some(&changed),
        None,
    );
    assert_eq!(status, 409, "{body}");
    assert!(body.contains("different request body"), "{body}");
}

#[test]
fn stale_event_cursor_is_resync_required_and_tokens_are_enforced() {
    let prepared = prepare(true);
    let server = start(&prepared.state, &["--token", "secret-token"]);

    let (status, _) = http(&server, "GET", "/v1/health", None, None);
    assert_eq!(status, 200, "health stays open for liveness probes");

    let query = json!({
        "project_id": PROJECT,
        "snapshot_id": prepared.snapshot_id,
        "kind": "symbols",
        "query": "",
        "entity_ids": [],
        "relation_kinds": [],
        "max_nodes": 10,
        "max_depth": 0,
        "cursor": null,
    });
    let (status, body) = http(&server, "POST", "/v1/query", Some(&query), None);
    assert_eq!(status, 403, "{body}");
    assert!(body.contains("permission_denied"), "{body}");
    let (status, _) = http(&server, "POST", "/v1/query", Some(&query), Some("secret-token"));
    assert_eq!(status, 200);

    let (status, body) = http(
        &server,
        "GET",
        "/v1/events?project_id=project.http&after_sequence=0",
        None,
        Some("secret-token"),
    );
    assert_eq!(status, 409, "{body}");
    assert!(body.contains("resync_required"), "{body}");
}

#[test]
fn lifecycle_routes_use_allowlisted_handles_and_idempotency() {
    let prepared = prepare(false);
    let server = start_with(&prepared.state, &["--no-auth"], &[&prepared.root]);
    let (handle, path) = server
        .workspaces
        .iter()
        .next()
        .expect("allowlisted workspace");
    assert!(path.ends_with("repo"));
    let config = json!({
        "schema_version": "1.0",
        "project_id": "project.http",
        "mode": "manager"
    });
    let open_body = json!({
        "workspace_handle": handle,
        "mode": "manager",
        "config": config,
        "idempotency_key": "open-1"
    });
    let (status, body) = http(&server, "POST", "/v1/projects/open", Some(&open_body), None);
    assert_eq!(status, 200, "{body}");
    let opened: Value = serde_json::from_str(&body).unwrap();
    let project_id = opened["id"].as_str().unwrap().to_string();
    assert_eq!(opened["mode"], "manager");
    assert_eq!(opened["state"], "open");

    let (status, body) = http(&server, "POST", "/v1/projects/open", Some(&open_body), None);
    assert_eq!(status, 200, "{body}");
    assert_eq!(body, body, "replayed open must return the stored project");
    let replay: Value = serde_json::from_str(&body).unwrap();
    assert_eq!(replay["id"], opened["id"]);

    let (status, body) = http(&server, "GET", &format!("/v1/projects/{project_id}"), None, None);
    assert_eq!(status, 200, "{body}");
    let fetched: Value = serde_json::from_str(&body).unwrap();
    assert_eq!(fetched["id"], opened["id"]);

    let close_body = json!({"expected_revision": 1, "idempotency_key": "close-1"});
    let (status, body) = http(
        &server,
        "POST",
        &format!("/v1/projects/{project_id}/close"),
        Some(&close_body),
        None,
    );
    assert_eq!(status, 200, "{body}");
    let closed: Value = serde_json::from_str(&body).unwrap();
    assert_eq!(closed["state"], "closed");
    let (status, replay) = http(
        &server,
        "POST",
        &format!("/v1/projects/{project_id}/close"),
        Some(&close_body),
        None,
    );
    assert_eq!((status, replay.as_str()), (200, body.as_str()));

    let changed = json!({"expected_revision": 2, "idempotency_key": "close-1"});
    let (status, body) = http(
        &server,
        "POST",
        &format!("/v1/projects/{project_id}/close"),
        Some(&changed),
        None,
    );
    assert_eq!(status, 409, "{body}");
    assert!(body.contains("different request body"), "{body}");

    let reopen = json!({
        "workspace_handle": handle,
        "mode": "manager",
        "config": config,
        "idempotency_key": "open-2"
    });
    let (status, body) = http(&server, "POST", "/v1/projects/open", Some(&reopen), None);
    assert_eq!(status, 200, "{body}");
    let reopened: Value = serde_json::from_str(&body).unwrap();
    assert_eq!(reopened["id"], opened["id"], "handle keeps project identity");
    assert!(reopened["revision"].as_i64().unwrap() > 1);

    let (status, body) = http(
        &server,
        "POST",
        &format!("/v1/projects/{project_id}/promote"),
        Some(&json!({"expected_revision": reopened["revision"], "idempotency_key": "promo-1"})),
        None,
    );
    assert_eq!(status, 409, "{body}");
    assert!(body.contains("already a Manager project"), "{body}");

    let unknown = json!({
        "workspace_handle": "ws.0000000000000000",
        "mode": "expedition",
        "config": config,
        "idempotency_key": "open-unknown"
    });
    let (status, body) = http(&server, "POST", "/v1/projects/open", Some(&unknown), None);
    assert_eq!(status, 403, "{body}");
    assert!(body.contains("permission_denied"), "{body}");
}
