// Copyright 2026 ARCHMELD contributors
// SPDX-License-Identifier: Apache-2.0

use archmeld_core::bridge::{import_codegraph_export, BridgeConfig};
use archmeld_core::cognition_map::check_map;
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
    store.ensure_project("project.map", "expedition").unwrap();
    let captured = capture_snapshot(
        &mut store,
        "project.map",
        &root,
        &SnapshotInputs {
            build_config: json!({"variant": "map"}),
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
            project_id: "project.map",
            snapshot_id: &captured.snapshot_id,
            config_digest: &config_digest,
            run_id: "run.map.001",
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
        project: "project.map".to_string(),
        snapshot_id: captured.snapshot_id,
    }
}

fn write_map(fixture: &Fixture, body: &str) {
    let text = format!(
        "#ARCHMELD-MAP: 1\n#snapshot: {}\n#scope: code\n===src/===\n{body}\n",
        fixture.snapshot_id
    );
    fs::write(fixture.root.join("archmeld.map.txt"), text).unwrap();
}

fn check(fixture: &Fixture) -> serde_json::Value {
    check_map(
        &fixture.store,
        &fixture.project,
        &fixture.root,
        Some(&fixture.snapshot_id),
        None,
    )
    .unwrap()
}

#[test]
fn a_valid_map_resolves_file_and_entity_refs() {
    let fixture = setup();
    write_map(
        &fixture,
        "a.py[L1D2C5M]: F:Entry A | R:entity:function:src/a.py::a,file:src/b.py | A:- | S:-\n\
         b.py[L1D2C5M]: F:Entry B | R:file:src/a.py | A:- | S:-",
    );
    let report = check(&fixture);
    assert_eq!(report["state"], "ok", "{report}");
    assert_eq!(report["entries"]["total"], 2, "{report}");
    assert_eq!(report["refs"]["confirmed"], 3, "{report}");
    assert_eq!(report["refs"]["orphan"], 0, "{report}");
    assert!(report["errors"].as_array().unwrap().is_empty(), "{report}");
    assert_eq!(report["semantic_claims"], "unverified", "{report}");
    assert_eq!(report["verification"], "not_run", "{report}");
    assert_eq!(report["validation"], "refs_checked", "{report}");
    assert_eq!(report["freshness"], "latest", "{report}");
    assert!(
        report["map_digest"]
            .as_str()
            .unwrap()
            .starts_with("sha256:"),
        "{report}"
    );
}

#[test]
fn an_orphan_entity_ref_is_invalid() {
    let fixture = setup();
    write_map(
        &fixture,
        "a.py: F:Entry A | R:entity:function:src/a.py::missing | A:- | S:-\n\
         b.py: F:Entry B | R:- | A:- | S:-",
    );
    let report = check(&fixture);
    assert_eq!(report["state"], "invalid", "{report}");
    assert_eq!(report["refs"]["orphan"], 1, "{report}");
    assert!(report["errors"]
        .as_array()
        .unwrap()
        .iter()
        .any(|issue| issue["code"] == "ref_orphan"));
}

#[test]
fn a_stale_header_is_reported_without_failing_resolution() {
    let fixture = setup();
    let text = format!(
        "#ARCHMELD-MAP: 1\n#snapshot: sha256:old\n===src/===\n\
         a.py: F:Entry A | R:file:src/b.py | A:- | S:-\n\
         b.py: F:Entry B | R:- | A:- | S:-\n"
    );
    fs::write(fixture.root.join("archmeld.map.txt"), text).unwrap();
    let report = check(&fixture);
    assert_eq!(report["state"], "stale", "{report}");
    assert_eq!(report["snapshot"]["match"], false, "{report}");
    assert!(report["warnings"]
        .as_array()
        .unwrap()
        .iter()
        .any(|issue| issue["code"] == "map_stale"));
}

#[test]
fn missing_and_suffixes_are_reported() {
    let fixture = setup();
    write_map(
        &fixture,
        "a.py: F:Entry A | R:entity:function:src/a.py::a@unknown(dynamic dispatch),file:src/b.py@candidate | A:- | S:-",
    );
    let report = check(&fixture);
    assert_eq!(report["state"], "ok", "{report}");
    assert_eq!(report["refs"]["unknown"], 1, "{report}");
    assert_eq!(report["refs"]["candidate"], 1, "{report}");
    assert_eq!(report["coverage"]["missing"][0], "src/b.py", "{report}");
    assert!(report["warnings"]
        .as_array()
        .unwrap()
        .iter()
        .any(|issue| issue["code"] == "candidate_suffix"));
}

#[test]
fn malformed_entry_and_secret_are_errors() {
    let fixture = setup();
    write_map(
        &fixture,
        "a.py: F:Entry with sk-1234567890abcdef | R:- | A:-",
    );
    let report = check(&fixture);
    assert_eq!(report["state"], "invalid", "{report}");
    assert!(report["errors"]
        .as_array()
        .unwrap()
        .iter()
        .any(|issue| issue["code"] == "entry_field"));
    assert!(!serde_json::to_string(&report)
        .unwrap()
        .contains("sk-1234567890abcdef"));
}

#[test]
fn crlf_and_a_missing_map_are_handled() {
    let fixture = setup();
    let text = format!(
        "#ARCHMELD-MAP: 1\r\n#snapshot: {}\r\n===src/===\r\n\
         a.py: F:Entry A | R:file:src/b.py | A:- | S:-\r\n\
         b.py: F:Entry B | R:- | A:- | S:-\r\n",
        fixture.snapshot_id
    );
    fs::write(fixture.root.join("archmeld.map.txt"), text).unwrap();
    let report = check(&fixture);
    assert_eq!(report["state"], "ok", "{report}");
    assert_eq!(report["entries"]["total"], 2, "{report}");

    fs::remove_file(fixture.root.join("archmeld.map.txt")).unwrap();
    let absent = check(&fixture);
    assert_eq!(absent["state"], "absent", "{absent}");
    assert_eq!(absent["present"], false, "{absent}");
}

#[test]
fn a_relation_ref_is_rejected_until_the_python_fix() {
    let fixture = setup();
    write_map(
        &fixture,
        "a.py: F:Entry A | R:relation:call:src/a.py::a->src/b.py::b | A:- | S:-\n\
         b.py: F:Entry B | R:- | A:- | S:-",
    );
    let report = check(&fixture);
    assert_eq!(report["state"], "invalid", "{report}");
    assert!(report["errors"]
        .as_array()
        .unwrap()
        .iter()
        .any(|issue| issue["code"] == "ref_relation_deferred"));
}

#[test]
fn a_secret_literal_is_rejected_without_echoing_it() {
    let fixture = setup();
    write_map(
        &fixture,
        "a.py: F:Uses sk-1234567890abcdef for the client | R:- | A:- | S:-\n\
         b.py: F:Entry B | R:- | A:- | S:-",
    );
    let report = check(&fixture);
    assert_eq!(report["state"], "invalid", "{report}");
    assert!(report["errors"]
        .as_array()
        .unwrap()
        .iter()
        .any(|issue| issue["code"] == "secret_literal"));
    assert!(!serde_json::to_string(&report)
        .unwrap()
        .contains("sk-1234567890abcdef"));
}

#[test]
fn summarize_affected_and_entry_report_read_the_map() {
    use archmeld_core::cognition_map::{affected, entry_report, summarize};

    let fixture = setup();
    write_map(
        &fixture,
        "a.py[L1D2C5M]: F:Entry A | R:file:src/b.py | A:- | S:-\n\
         b.py: F:Entry B | R:- | A:- | S:-",
    );
    let summary = summarize(Some(&fixture.root), Some(&fixture.snapshot_id));
    assert_eq!(summary["present"], true, "{summary}");
    assert_eq!(summary["entries"], 2, "{summary}");
    assert_eq!(summary["snapshot_match"], true, "{summary}");
    assert_eq!(summary["semantic_claims"], "unverified", "{summary}");
    assert_eq!(summary["validation"], "not_run", "{summary}");
    assert_eq!(summary["freshness"], "latest", "{summary}");
    assert!(
        summary["map_digest"]
            .as_str()
            .unwrap()
            .starts_with("sha256:"),
        "{summary}"
    );

    let changed = affected(
        &fixture.root,
        Some(&fixture.snapshot_id),
        &["src/a.py".into(), "src/new.py".into()],
    );
    assert_eq!(
        changed["affected"].as_array().unwrap().len(),
        1,
        "{changed}"
    );
    assert_eq!(changed["state"], "ok", "{changed}");
    assert_eq!(changed["snapshot_match"], true, "{changed}");
    assert_eq!(changed["binding"]["file"], 1, "{changed}");
    assert_eq!(changed["affected"][0]["path"], "src/a.py", "{changed}");
    assert_eq!(changed["affected"][0]["api"], "-", "{changed}");
    assert_eq!(changed["affected"][0]["constraint"], "-", "{changed}");
    assert!(
        changed["map_digest"]
            .as_str()
            .unwrap()
            .starts_with("sha256:"),
        "{changed}"
    );
    assert_eq!(
        changed["unmapped_changed_files"],
        serde_json::json!(["src/new.py"]),
        "{changed}"
    );

    let entry = entry_report(
        &fixture.store,
        &fixture.project,
        &fixture.root,
        Some(&fixture.snapshot_id),
        None,
        "src/a.py",
    )
    .unwrap();
    assert_eq!(entry["entry"]["path"], "src/a.py", "{entry}");
    assert_eq!(entry["entry"]["refs"][0]["kind"], "file", "{entry}");
    assert_eq!(entry["entry"]["constraint"], "-", "{entry}");
    assert_eq!(entry["semantic_claims"], "unverified", "{entry}");
    assert_eq!(entry["validation"], "refs_checked", "{entry}");
    assert_eq!(entry["freshness"], "latest", "{entry}");
    assert_eq!(entry["snapshot_match"], true, "{entry}");
    assert!(entry["errors"].as_array().unwrap().is_empty(), "{entry}");
}

#[test]
fn maintain_marks_changed_files_after_a_new_snapshot() {
    use archmeld_core::cognition_map::maintain;
    use archmeld_core::snapshot::{capture_snapshot, SnapshotInputs, SnapshotOptions};

    let mut fixture = setup();
    write_map(
        &fixture,
        "a.py: F:Entry A | R:file:src/b.py | A:- | S:-\n\
         b.py: F:Entry B | R:- | A:- | S:-",
    );
    fs::write(
        fixture.root.join("src/a.py"),
        "def a():\n    return b() + 1\n",
    )
    .unwrap();
    capture_snapshot(
        &mut fixture.store,
        &fixture.project,
        &fixture.root,
        &SnapshotInputs {
            build_config: json!({"variant": "map-2"}),
            environment: json!({"os": "test"}),
        },
        &SnapshotOptions {
            include_untracked: true,
            ..SnapshotOptions::default()
        },
    )
    .unwrap();
    let report = maintain(&fixture.store, &fixture.project, &fixture.root, None, None).unwrap();
    assert_eq!(report["state"], "stale", "{report}");
    assert_eq!(report["entries"]["stale"], 1, "{report}");
    assert_eq!(report["stale_entries"][0]["path"], "src/a.py", "{report}");
    assert_eq!(report["entries"]["unchanged"], 1, "{report}");
    assert_eq!(report["semantic_claims"], "unverified", "{report}");
    assert_eq!(report["validation"], "refs_checked", "{report}");
    assert_eq!(report["freshness"], "behind", "{report}");
    assert!(
        report["map_digest"]
            .as_str()
            .unwrap()
            .starts_with("sha256:"),
        "{report}"
    );

    let markdown = archmeld_core::cognition_map::render_markdown(&report);
    assert!(markdown.contains("- freshness: behind"), "{markdown}");
    assert!(markdown.contains("1 stale"), "{markdown}");
    assert!(!markdown.contains("snapshot: none"), "{markdown}");
}

#[test]
fn absent_and_snapshotless_maps_return_state_not_storage_errors() {
    use archmeld_core::cognition_map::{entry_report, maintain};

    let fixture = setup();
    let missing = entry_report(
        &fixture.store,
        &fixture.project,
        &fixture.root,
        Some(&fixture.snapshot_id),
        None,
        "src/a.py",
    )
    .unwrap();
    assert_eq!(missing["state"], "absent", "{missing}");
    assert_eq!(missing["present"], false, "{missing}");
    let maintained = maintain(&fixture.store, &fixture.project, &fixture.root, None, None).unwrap();
    assert_eq!(maintained["state"], "absent", "{maintained}");
    assert_eq!(maintained["command"], "maintain", "{maintained}");

    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().join("repo");
    fs::create_dir_all(root.join("src")).unwrap();
    fs::write(root.join("src/a.py"), "def a():\n    return 1\n").unwrap();
    fs::write(
        root.join("archmeld.map.txt"),
        "#ARCHMELD-MAP: 1\n#snapshot: sha256:none\n===src/===\n\
         a.py: F:Entry A | R:- | A:- | S:-\n",
    )
    .unwrap();
    let mut store = Store::open(&dir.path().join("state")).unwrap();
    store
        .ensure_project("project.nosnap", "expedition")
        .unwrap();
    let report = check_map(&store, "project.nosnap", &root, None, None).unwrap();
    assert_eq!(report["state"], "needs_snapshot", "{report}");
    assert_eq!(report["freshness"], "unknown", "{report}");
    assert!(report["map_digest"].as_str().is_some(), "{report}");
    let maintained = maintain(&store, "project.nosnap", &root, None, None).unwrap();
    assert_eq!(maintained["state"], "needs_snapshot", "{maintained}");
    assert_eq!(maintained["command"], "maintain", "{maintained}");
    let entry = entry_report(&store, "project.nosnap", &root, None, None, "src/a.py").unwrap();
    assert_eq!(entry["state"], "needs_snapshot", "{entry}");
    assert_eq!(entry["present"], true, "{entry}");
}
