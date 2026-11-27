// Copyright 2026 ARCHMELD contributors
// SPDX-License-Identifier: Apache-2.0

use archmeld_api::onboard;
use archmeld_core::manager::{open_project, OpenRequest};
use archmeld_core::snapshot::{capture_snapshot, SnapshotInputs, SnapshotOptions};
use archmeld_core::store::Store;
use serde_json::json;
use std::fs;
use std::path::Path;

#[test]
fn default_paths_are_stable_and_project_scoped() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().join("my-repo");
    fs::create_dir_all(&root).unwrap();
    let state = onboard::default_state_root(&root).unwrap();
    assert!(state.to_string_lossy().contains("projects"));
    let project = onboard::default_project_id(&root);
    assert!(project.starts_with("project.my-repo."));
    assert_eq!(project, onboard::default_project_id(&root));
}

#[test]
fn mcp_plans_render_real_client_commands() {
    let state = Path::new("state-root");
    let claude = onboard::plan_mcp_install("claude", state, "project.demo").unwrap();
    assert_eq!(claude["client"], "claude");
    assert_eq!(claude["command"][0], "claude");
    assert_eq!(claude["command"][1], "mcp");
    assert_eq!(claude["command"][2], "add");
    assert_eq!(claude["snippet_format"], "json");
    assert!(claude["snippet"]["mcpServers"]["archmeld"]["command"].is_string());

    let codex = onboard::plan_mcp_install("codex", state, "project.demo").unwrap();
    assert_eq!(codex["command"][0], "codex");
    assert_eq!(codex["snippet_format"], "toml");
    assert!(codex["snippet"]
        .as_str()
        .unwrap()
        .contains("[mcp_servers.archmeld]"));

    let generic = onboard::plan_mcp_install("generic", state, "project.demo").unwrap();
    assert!(generic["command"].as_array().unwrap().is_empty());
    assert!(generic["snippet"]["mcpServers"]["archmeld"].is_object());

    assert!(onboard::plan_mcp_install("unknown", state, "project.demo").is_err());
}

#[test]
fn rule_block_is_previewed_written_and_idempotent() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().join("repo");
    fs::create_dir_all(&root).unwrap();
    fs::write(root.join("AGENTS.md"), "# Host rules\n\nKeep this line.\n").unwrap();

    let preview = onboard::install_rules(&root, "AGENTS.md", false).unwrap();
    assert_eq!(preview["written"], false);
    let preview_text = preview["content"].as_str().unwrap();
    assert!(preview_text.contains("<!-- ARCHMELD:BEGIN -->"));
    assert!(
        preview_text.contains("### Session start"),
        "rules must install the session-start contract"
    );
    assert!(preview_text.contains("archmeld_status"));
    assert!(preview_text.contains("### Before you finish"));

    let written = onboard::install_rules(&root, "AGENTS.md", true).unwrap();
    assert_eq!(written["written"], true);
    let text = fs::read_to_string(root.join("AGENTS.md")).unwrap();
    assert!(text.contains("# Host rules"));
    assert!(text.contains("<!-- ARCHMELD:END -->"));
    assert_eq!(text.matches("<!-- ARCHMELD:BEGIN -->").count(), 1);

    let second = onboard::install_rules(&root, "AGENTS.md", true).unwrap();
    assert_eq!(second["changed"], false);
    assert_eq!(
        fs::read_to_string(root.join("AGENTS.md"))
            .unwrap()
            .matches("<!-- ARCHMELD:BEGIN -->")
            .count(),
        1
    );

    let claude = onboard::install_rules(&root, "CLAUDE.md", false).unwrap();
    assert!(claude["content"]
        .as_str()
        .unwrap()
        .contains("archmeld_brief"));
}

#[test]
fn preflight_skips_vendor_and_metadata_directories() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().join("repo");
    fs::create_dir_all(root.join("src")).unwrap();
    fs::create_dir_all(root.join(".git/objects")).unwrap();
    fs::create_dir_all(root.join("node_modules/pkg")).unwrap();
    fs::write(root.join("src/a.py"), "x = 1\n").unwrap();
    fs::write(root.join(".git/objects/x"), "x").unwrap();
    fs::write(root.join("node_modules/pkg/y.js"), "x").unwrap();
    let count = onboard::count_source_files(&root, 100).unwrap();
    assert_eq!(count["files"], 1);
    assert_eq!(count["truncated"], false);
}

#[test]
fn state_inside_workspace_is_rejected() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().join("repo");
    fs::create_dir_all(root.join("state")).unwrap();
    assert!(onboard::state_inside_workspace(&root.join("state"), &root).unwrap());
    assert!(!onboard::state_inside_workspace(&dir.path().join("outside"), &root).unwrap());
}

#[test]
fn real_mcp_handshake_reports_root_and_tools() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().join("repo");
    fs::create_dir_all(root.join("src")).unwrap();
    fs::write(root.join("src/a.py"), "VALUE = 1\n").unwrap();
    let state = dir.path().join("state");
    let mut store = Store::open(&state).unwrap();
    open_project(
        &mut store,
        OpenRequest {
            project_id: "project.onboard",
            mode: "expedition",
            workspace_root: &root,
            state_root: &state.join("sessions"),
            config: json!({"profile": "onboard"}),
        },
    )
    .unwrap();
    capture_snapshot(
        &mut store,
        "project.onboard",
        &root,
        &SnapshotInputs {
            build_config: json!({"variant": "onboard"}),
            environment: json!({"os": "test"}),
        },
        &SnapshotOptions {
            include_untracked: true,
            ..SnapshotOptions::default()
        },
    )
    .unwrap();

    let binary = Path::new(env!("CARGO_BIN_EXE_archmeld-mcp"));
    let health = onboard::health_check_with_binary(binary, &state, "project.onboard").unwrap();
    assert_eq!(health["state"], "ok");
    assert_eq!(health["tools"], 8);
    assert!(health["tool_names"]
        .as_array()
        .unwrap()
        .iter()
        .any(|name| name.as_str() == Some("archmeld_brief")));
    assert_eq!(
        health["status"]["projects"][0]["freshness"]["state"],
        "clean"
    );
}
