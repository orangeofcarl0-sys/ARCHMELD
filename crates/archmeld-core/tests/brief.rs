// Copyright 2026 ARCHMELD contributors
// SPDX-License-Identifier: Apache-2.0

use archmeld_core::bridge::{import_codegraph_export, BridgeConfig};
use archmeld_core::brief::{build_brief, BriefRecipe, BriefRequest};
use archmeld_core::manager::{open_project, OpenRequest};
use archmeld_core::snapshot::{capture_snapshot, SnapshotInputs, SnapshotOptions};
use archmeld_core::store::Store;
use serde_json::json;
use std::fs;
use std::path::PathBuf;

struct Fixture {
    _dir: tempfile::TempDir,
    store: Store,
    root: PathBuf,
    project: String,
    snapshot_id: String,
}

fn setup() -> Fixture {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().join("repo");
    fs::create_dir_all(root.join("src")).unwrap();
    fs::write(root.join("src/a.py"), "def a():\n    return b()\n").unwrap();
    fs::write(root.join("src/b.py"), "def b():\n    return 1\n").unwrap();
    let state = dir.path().join("state");
    let mut store = Store::open(&state).unwrap();
    open_project(
        &mut store,
        OpenRequest {
            project_id: "project.brief",
            mode: "expedition",
            workspace_root: &root,
            state_root: &state.join("sessions"),
            config: json!({"profile": "brief"}),
        },
    )
    .unwrap();
    let captured = capture_snapshot(
        &mut store,
        "project.brief",
        &root,
        &SnapshotInputs {
            build_config: json!({"variant": "brief"}),
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
            "path": root.join("src/a.py").to_string_lossy(),"name":"a",
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
            project_id: "project.brief",
            snapshot_id: &captured.snapshot_id,
            config_digest: &config_digest,
            run_id: "run.brief.001",
            workspace_root: Some(&root),
            max_entities_per_batch: 500,
            max_relations_per_batch: 2000,
        },
        &captured.files,
        &export,
    )
    .unwrap();
    Fixture {
        _dir: dir,
        store,
        root,
        project: "project.brief".to_string(),
        snapshot_id: captured.snapshot_id,
    }
}

#[test]
fn project_brief_reports_counts_and_next_actions() {
    let fixture = setup();
    let brief = build_brief(
        &fixture.store,
        &BriefRequest {
            project_id: &fixture.project,
            snapshot_id: Some(&fixture.snapshot_id),
            workspace: Some(&fixture.root),
            recipe: BriefRecipe::Project,
        },
    )
    .unwrap();
    assert_eq!(brief["state"], "ok");
    assert_eq!(brief["level"], "module");
    assert_eq!(brief["scope"]["snapshot_id"], fixture.snapshot_id);
    assert_eq!(brief["counts"]["candidate"], 1);
    assert!(brief["summary"].as_str().unwrap().contains("entities"));
    assert!(brief["next_actions"].as_array().unwrap().len() >= 2);
}

#[test]
fn change_brief_is_static_and_keeps_candidate_resolution() {
    let fixture = setup();
    fs::write(fixture.root.join("src/b.py"), "def b():\n    return 5\n").unwrap();
    let brief = build_brief(
        &fixture.store,
        &BriefRequest {
            project_id: &fixture.project,
            snapshot_id: Some(&fixture.snapshot_id),
            workspace: Some(&fixture.root),
            recipe: BriefRecipe::Change,
        },
    )
    .unwrap();
    assert_eq!(brief["state"], "ok");
    assert_eq!(brief["level"], "change");
    assert_eq!(brief["counts"]["stale"], 1);
    let findings = brief["findings"].as_array().unwrap();
    assert!(!findings.is_empty());
    for finding in findings {
        assert_eq!(finding["verification"], "not_run");
    }
    assert!(brief["findings"]
        .as_array()
        .unwrap()
        .iter()
        .any(|finding| finding["evidence"]
            .as_array()
            .unwrap()
            .iter()
            .any(|evidence| evidence["resolution"] == "candidate")));
    assert!(!brief["unknowns"]
        .as_array()
        .unwrap()
        .iter()
        .any(|unknown| unknown["question"]
            == "line overlap is static text evidence, not semantic equivalence"));
}

#[test]
fn brief_returns_needs_init_without_a_snapshot() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().join("repo");
    fs::create_dir_all(&root).unwrap();
    let state = dir.path().join("state");
    let mut store = Store::open(&state).unwrap();
    store.ensure_project("project.empty", "expedition").unwrap();
    let brief = build_brief(
        &store,
        &BriefRequest {
            project_id: "project.empty",
            snapshot_id: None,
            workspace: Some(&root),
            recipe: BriefRecipe::Auto,
        },
    )
    .unwrap();
    assert_eq!(brief["state"], "needs_init");
    assert_eq!(brief["next_actions"][0]["tool"], "archmeld_init");
}
