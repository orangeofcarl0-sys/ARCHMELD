// Copyright 2026 ARCHMELD contributors
// SPDX-License-Identifier: Apache-2.0

use archmeld_core::brief::{
    session_card_path, session_card_status, write_session_card, SESSION_CARD_MARKER,
};
use serde_json::json;
use std::fs;

fn sample_brief(snapshot: &str) -> serde_json::Value {
    json!({
        "schema_version": "1.0",
        "state": "ok",
        "level": "module",
        "summary": "2 files, 2 entities, 1 relation",
        "scope": {
            "project_id": "project.card",
            "snapshot_id": snapshot,
            "commit": "abc123",
            "coverage": "complete"
        },
        "counts": {
            "confirmed": 1,
            "candidate": 0,
            "unknown": 0,
            "stale": 0,
            "findings": 0
        },
        "findings": [],
        "unknowns": [],
        "next_actions": []
    })
}

#[test]
fn card_is_written_and_reports_snapshot_freshness() {
    let dir = tempfile::tempdir().unwrap();
    let workspace = dir.path();
    let written = write_session_card(workspace, &sample_brief("sha256:abc"), None, false).unwrap();
    assert_eq!(written["written"], true, "{written}");

    let path = session_card_path(workspace);
    let text = fs::read_to_string(&path).unwrap();
    assert!(text.starts_with(SESSION_CARD_MARKER), "{text}");
    assert!(text.contains("- snapshot: sha256:abc"), "{text}");
    assert!(text.contains("- generated_at_unix: "), "{text}");
    assert!(text.contains("# ARCHMELD session card"), "{text}");

    let fresh = session_card_status(workspace, Some("sha256:abc"), false);
    assert_eq!(fresh["present"], true, "{fresh}");
    assert_eq!(fresh["marker"], true, "{fresh}");
    assert_eq!(fresh["snapshot_match"], true, "{fresh}");
    assert_eq!(fresh["stale"], false, "{fresh}");

    let stale = session_card_status(workspace, Some("sha256:def"), false);
    assert_eq!(stale["snapshot_match"], false, "{stale}");
    assert_eq!(stale["stale"], true, "{stale}");

    let dirty = session_card_status(workspace, Some("sha256:abc"), true);
    assert_eq!(dirty["stale"], true, "{dirty}");
}

#[test]
fn a_user_file_is_not_overwritten_without_force() {
    let dir = tempfile::tempdir().unwrap();
    let workspace = dir.path();
    let path = session_card_path(workspace);
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(&path, "# my notes\n").unwrap();

    let refused = write_session_card(workspace, &sample_brief("sha256:abc"), None, false);
    assert!(refused.is_err(), "{refused:?}");
    assert_eq!(fs::read_to_string(&path).unwrap(), "# my notes\n");

    let forced = write_session_card(workspace, &sample_brief("sha256:abc"), None, true).unwrap();
    assert_eq!(forced["written"], true, "{forced}");
    assert!(fs::read_to_string(&path)
        .unwrap()
        .starts_with(SESSION_CARD_MARKER));
}

#[test]
fn a_missing_card_is_not_stale() {
    let dir = tempfile::tempdir().unwrap();
    let status = session_card_status(dir.path(), Some("sha256:abc"), false);
    assert_eq!(status["present"], false, "{status}");
    assert_eq!(status["stale"], false, "{status}");
}

#[test]
fn an_explicit_out_path_is_used() {
    let dir = tempfile::tempdir().unwrap();
    let workspace = dir.path().join("repo");
    fs::create_dir_all(&workspace).unwrap();
    let out = dir.path().join("cards").join("CARD.md");
    let written =
        write_session_card(&workspace, &sample_brief("sha256:abc"), Some(&out), false).unwrap();
    assert_eq!(written["written"], true, "{written}");
    assert!(out.is_file());
    let status = session_card_status(&workspace, Some("sha256:abc"), false);
    assert_eq!(status["present"], false, "{status}");
}

#[test]
fn the_card_does_not_make_the_workspace_stale() {
    use archmeld_core::manager::{open_project, OpenRequest};
    use archmeld_core::snapshot::{capture_snapshot, SnapshotInputs, SnapshotOptions};
    use archmeld_core::status::status_report;
    use archmeld_core::store::Store;

    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().join("repo");
    fs::create_dir_all(root.join("src")).unwrap();
    fs::write(root.join("src/a.py"), "def a():\n    return 1\n").unwrap();
    let state = dir.path().join("state");
    let mut store = Store::open(&state).unwrap();
    open_project(
        &mut store,
        OpenRequest {
            project_id: "project.card",
            mode: "expedition",
            workspace_root: &root,
            state_root: &state.join("sessions"),
            config: json!({"profile": "card"}),
        },
    )
    .unwrap();
    capture_snapshot(
        &mut store,
        "project.card",
        &root,
        &SnapshotInputs {
            build_config: json!({"variant": "card"}),
            environment: json!({"os": "test"}),
        },
        &SnapshotOptions {
            include_untracked: true,
            ..SnapshotOptions::default()
        },
    )
    .unwrap();
    let snapshot = store.latest_snapshot_id("project.card").unwrap().unwrap();
    let brief = sample_brief(&snapshot);
    write_session_card(&root, &brief, None, false).unwrap();

    let status = status_report(&store, Some("project.card"), Some(&root)).unwrap();
    let project = &status["projects"][0];
    assert_eq!(project["freshness"]["state"], "clean", "{status}");
    assert_eq!(project["session_card"]["present"], true, "{status}");
    assert_eq!(project["session_card"]["snapshot_match"], true, "{status}");
    assert_eq!(project["session_card"]["stale"], false, "{status}");
}
