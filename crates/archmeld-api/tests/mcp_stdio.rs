// Copyright 2026 ARCHMELD contributors
// SPDX-License-Identifier: Apache-2.0

//! B023 / AT053: a real MCP stdio process exposes only read tools, has no
//! approval tool, refuses write-looking calls with the shared error model and
//! never accepts an arbitrary filesystem path.

use archmeld_core::bridge::{import_codegraph_export, BridgeConfig};
use archmeld_core::hash;
use archmeld_core::manager::{open_project, OpenRequest};
use archmeld_core::snapshot::{capture_snapshot, SnapshotInputs, SnapshotOptions};
use archmeld_core::store::Store;
use serde_json::{json, Value};
use std::fs;
use std::io::{BufRead, BufReader, Write};
use std::path::{Path, PathBuf};
use std::process::{Child, ChildStdin, ChildStdout, Command, Stdio};

struct Server {
    child: Child,
    stdin: ChildStdin,
    stdout: BufReader<ChildStdout>,
    transcript: Vec<Value>,
}

impl Server {
    fn start(state: &Path, audit: &Path) -> Self {
        let mut child = Command::new(env!("CARGO_BIN_EXE_archmeld-mcp"))
            .arg("--state")
            .arg(state)
            .arg("--audit")
            .arg(audit)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .expect("spawn mcp");
        let stdin = child.stdin.take().unwrap();
        let stdout = BufReader::new(child.stdout.take().unwrap());
        Self {
            child,
            stdin,
            stdout,
            transcript: Vec::new(),
        }
    }

    fn send(&mut self, request: &Value) -> Value {
        let line = serde_json::to_string(request).unwrap();
        writeln!(self.stdin, "{line}").unwrap();
        self.stdin.flush().unwrap();
        let mut response = String::new();
        self.stdout.read_line(&mut response).unwrap();
        let value: Value = serde_json::from_str(response.trim()).unwrap();
        self.transcript.push(json!({
            "request": request,
            "response": value,
        }));
        value
    }

    fn notify(&mut self, request: &Value) {
        let line = serde_json::to_string(request).unwrap();
        writeln!(self.stdin, "{line}").unwrap();
        self.stdin.flush().unwrap();
        self.transcript.push(json!({"notification": request}));
    }

    fn call(&mut self, id: i64, name: &str, arguments: Value) -> Value {
        self.send(&json!({
            "jsonrpc": "2.0",
            "id": id,
            "method": "tools/call",
            "params": {"name": name, "arguments": arguments},
        }))
    }
}

impl Drop for Server {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

fn setup() -> (tempfile::TempDir, PathBuf, PathBuf, String, String) {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().join("repo");
    fs::create_dir_all(root.join("src")).unwrap();
    fs::write(root.join("src/a.py"), "def a1():\n    return b()\n").unwrap();
    fs::write(root.join("src/b.py"), "def b():\n    return 1\n").unwrap();
    let state = dir.path().join("state");
    let mut store = Store::open(&state).unwrap();
    open_project(
        &mut store,
        OpenRequest {
            project_id: "project.mcp",
            mode: "expedition",
            workspace_root: &root,
            state_root: &state.join("sessions"),
            config: json!({"profile": "mcp"}),
        },
    )
    .unwrap();
    let captured = capture_snapshot(
        &mut store,
        "project.mcp",
        &root,
        &SnapshotInputs {
            build_config: json!({"variant": "mcp"}),
            environment: json!({"os": "test"}),
        },
        &SnapshotOptions {
            include_untracked: true,
            ..SnapshotOptions::default()
        },
    )
    .unwrap();
    let config_digest = captured.snapshot["config_digest"]
        .as_str()
        .unwrap()
        .to_string();
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
    let export = format!("{}\n", lines.join("\n"));
    import_codegraph_export(
        &mut store,
        &BridgeConfig {
            project_id: "project.mcp",
            snapshot_id: &captured.snapshot_id,
            config_digest: &config_digest,
            run_id: "run.mcp.001",
            workspace_root: Some(&root),
            max_entities_per_batch: 500,
            max_relations_per_batch: 2000,
        },
        &captured.files,
        &export,
    )
    .unwrap();
    let entity_id = store
        .entity_ids("project.mcp", &captured.snapshot_id)
        .unwrap()
        .into_iter()
        .find(|id| {
            store
                .get_entity("project.mcp", &captured.snapshot_id, id)
                .unwrap()
                .and_then(|entity| entity["qualified_name"].as_str().map(str::to_string))
                .is_some_and(|name| name.ends_with("::a1"))
        })
        .expect("a1 entity");
    (dir, state, root, captured.snapshot_id, entity_id)
}

fn text_of(response: &Value) -> Value {
    let text = response["result"]["content"][0]["text"]
        .as_str()
        .unwrap_or_else(|| panic!("no text content: {response}"));
    serde_json::from_str(text).unwrap()
}

#[test]
fn mcp_has_no_approval_tool_and_refuses_writes_and_paths() {
    let (dir, state, root, snapshot_id, entity_id) = setup();
    let source_before = hash::digest_bytes(&fs::read(root.join("src/a.py")).unwrap());
    let audit = dir.path().join("mcp-audit.jsonl");
    let mut server = Server::start(&state, &audit);

    let init = server.send(&json!({
        "jsonrpc": "2.0",
        "id": 1,
        "method": "initialize",
        "params": {"protocolVersion": "2024-11-05", "capabilities": {}, "clientInfo": {"name": "test"}},
    }));
    assert_eq!(init["result"]["serverInfo"]["name"], "archmeld");
    server.notify(&json!({"jsonrpc": "2.0", "method": "notifications/initialized"}));

    let list = server.send(&json!({"jsonrpc": "2.0", "id": 2, "method": "tools/list"}));
    let tools = list["result"]["tools"].as_array().unwrap();
    assert_eq!(tools.len(), 7);
    let names: Vec<&str> = tools
        .iter()
        .filter_map(|tool| tool["name"].as_str())
        .collect();
    assert_eq!(
        names,
        vec![
            "archmeld_inspect",
            "archmeld_query",
            "archmeld_context",
            "archmeld_source",
            "archmeld_status",
            "archmeld_review",
            "archmeld_capabilities",
        ]
    );
    assert!(!tools.iter().any(|tool| {
        let name = tool["name"].as_str().unwrap_or("").to_lowercase();
        name.contains("approve") || name.contains("accept")
    }));
    for tool in tools {
        assert_eq!(tool["x-implementation"]["status"], "implemented", "{tool}");
        assert_eq!(tool["x-implementation"]["surface"], "read_only");
    }
    let inspect_tool = tools
        .iter()
        .find(|tool| tool["name"] == "archmeld_inspect")
        .unwrap();
    let advertised_kinds = inspect_tool["inputSchema"]["properties"]["kind"]["enum"]
        .as_array()
        .unwrap();
    assert!(!advertised_kinds.iter().any(|kind| kind == "runtime_run"));
    assert!(!advertised_kinds.iter().any(|kind| kind == "report"));

    let capabilities = server.call(10, "archmeld_capabilities", json!({}));
    assert_eq!(capabilities["result"]["isError"], false);
    let capabilities = text_of(&capabilities);
    assert_eq!(capabilities["surface"], "mcp_stdio_read_only");
    assert_eq!(capabilities["approval_exposed"], false);
    assert_eq!(capabilities["grant_procedure"]["mcp_self_approval"], false);
    assert_eq!(
        capabilities["inspect_kinds"]["implemented"],
        json!(advertised_kinds)
    );
    assert_eq!(
        capabilities["inspect_kinds"]["not_implemented"],
        json!(["runtime_run", "report"])
    );
    let change_action = capabilities["tools"]
        .as_array()
        .unwrap()
        .iter()
        .find(|tool| tool["name"] == "archmeld_change_action")
        .unwrap();
    assert_eq!(change_action["status"], "requires_grant");
    assert_eq!(change_action["authorization"]["self_approval"], false);
    assert!(capabilities["tools"]
        .as_array()
        .unwrap()
        .iter()
        .all(|tool| tool["status"] == "implemented"
            || tool["status"] == "requires_grant"
            || tool["status"] == "not_implemented"));

    let status = server.call(11, "archmeld_status", json!({}));
    assert_eq!(status["result"]["isError"], false, "{status}");
    let status = text_of(&status);
    assert_eq!(status["projects"][0]["latest_snapshot_id"], snapshot_id);
    assert_eq!(status["projects"][0]["counts"]["entities"], 2);
    assert_eq!(status["projects"][0]["freshness"]["state"], "clean");

    let review = server.call(12, "archmeld_review", json!({}));
    assert_eq!(review["result"]["isError"], false, "{review}");
    let review = text_of(&review);
    assert_eq!(review["state"], "clean");
    assert_eq!(review["verification"]["state"], "not_run");
    assert_eq!(review["source_mutation"], "none");

    let query = server.call(
        3,
        "archmeld_query",
        json!({
            "project_id": "project.mcp",
            "snapshot_id": snapshot_id,
            "kind": "neighbors",
            "query": "",
            "entity_ids": [entity_id],
            "relation_kinds": ["static_call_candidate"],
            "max_nodes": 10,
            "max_depth": 2,
            "cursor": null,
        }),
    );
    assert_eq!(query["result"]["isError"], false);
    let result = text_of(&query);
    assert_eq!(result["entities"].as_array().unwrap().len(), 2);

    let inspect = server.call(
        4,
        "archmeld_inspect",
        json!({"kind": "entity", "id": entity_id}),
    );
    assert_eq!(inspect["result"]["isError"], false);
    assert_eq!(text_of(&inspect)["object"]["id"], entity_id);

    let source = server.call(
        5,
        "archmeld_source",
        json!({
            "project_id": "project.mcp",
            "snapshot_id": snapshot_id,
            "entity_id": entity_id,
            "anchor_index": 0,
            "max_bytes": 1024,
        }),
    );
    assert_eq!(source["result"]["isError"], false);
    assert!(text_of(&source)["text"]
        .as_str()
        .unwrap()
        .contains("def a1"));

    let apply = server.call(
        6,
        "archmeld_change_action",
        json!({
            "project_id": "project.mcp",
            "expected_revision": 1,
            "approval_id": null,
            "idempotency_key": "mcp-apply-1",
            "changeset_id": "changeset.mcp",
            "action": "apply",
        }),
    );
    assert_eq!(apply["result"]["isError"], true);
    let denied = text_of(&apply);
    assert_eq!(denied["code"], "permission_denied");
    assert!(denied["message"]
        .as_str()
        .unwrap()
        .contains("permission_denied"));
    assert_eq!(
        hash::digest_bytes(&fs::read(root.join("src/a.py")).unwrap()),
        source_before
    );

    // No approval endpoint exists at all.
    let approve = server.call(
        7,
        "archmeld_approve",
        json!({"changeset_id": "changeset.mcp"}),
    );
    assert_eq!(approve["result"]["isError"], true);
    assert!(text_of(&approve)["message"]
        .as_str()
        .unwrap()
        .contains("permission_denied"));

    // IDs are checked as IDs, never joined to a filesystem path.
    let traversal = server.call(
        8,
        "archmeld_inspect",
        json!({"kind": "entity", "id": "../../secret"}),
    );
    assert_eq!(traversal["result"]["isError"], true);
    let traversal_code = text_of(&traversal)["code"].as_str().unwrap().to_string();
    assert!(
        matches!(traversal_code.as_str(), "invalid_request" | "not_found"),
        "{traversal_code}"
    );
    let path_argument = server.call(
        9,
        "archmeld_source",
        json!({
            "project_id": "project.mcp",
            "snapshot_id": snapshot_id,
            "entity_id": entity_id,
            "anchor_index": 0,
            "max_bytes": 1024,
            "path": "C:/Windows/win.ini",
        }),
    );
    assert_eq!(path_argument["result"]["isError"], true);
    assert_eq!(text_of(&path_argument)["code"], "invalid_request");
    assert_eq!(
        hash::digest_bytes(&fs::read(root.join("src/a.py")).unwrap()),
        source_before
    );

    let audit_text = fs::read_to_string(&audit).unwrap();
    let audit_lines: Vec<Value> = audit_text
        .lines()
        .map(|line| serde_json::from_str(line).unwrap())
        .collect();
    assert!(audit_lines
        .iter()
        .any(|line| line["tool"] == "archmeld_query" && line["outcome"] == "ok"));
    assert!(audit_lines.iter().any(|line| {
        line["tool"] == "archmeld_change_action"
            && line["outcome"] == "error"
            && line["code"] == "permission_denied"
    }));

    if std::env::var("ARCHMELD_WRITE_EVIDENCE").ok().as_deref() == Some("1") {
        let payload = json!({
            "generated_by": "crates/archmeld-api/tests/mcp_stdio.rs",
            "platform": std::env::consts::OS,
            "acceptance": {
                "AT053": {
                    "status": "passed",
                    "tool_count": tools.len(),
                    "approval_tool_present": false,
                    "write_rejected": denied,
                    "unknown_approval_rejected": text_of(&approve),
                    "traversal_rejected": text_of(&traversal),
                    "path_argument_rejected": text_of(&path_argument),
                    "source_unchanged": true,
                    "audit": audit_lines,
                }
            },
            "transcript": server.transcript,
        });
        let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../docs/evidence/B023-mcp.json");
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, serde_json::to_string_pretty(&payload).unwrap() + "\n").unwrap();
    }
}
