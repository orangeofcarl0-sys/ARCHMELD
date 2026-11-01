// Copyright 2026 ARCHMELD contributors
// SPDX-License-Identifier: Apache-2.0

//! Snapshot file excerpts for trace views: bytes always come from the snapshot
//! CAS, paths are relative POSIX paths, and an excerpt never reads the current
//! workspace file.

use archmeld_core::snapshot::{capture_snapshot, SnapshotInputs, SnapshotOptions};
use archmeld_core::source_api::read_source_file;
use archmeld_core::store::Store;
use serde_json::{json, Value};
use std::fs;

const P: &str = "project.source.file";

struct Fixture {
    _dir: tempfile::TempDir,
    root: std::path::PathBuf,
    store: Store,
    snapshot_id: String,
}

fn setup() -> Fixture {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().join("repo");
    fs::create_dir_all(root.join("src")).unwrap();
    fs::write(
        root.join("src/app.py"),
        "line 1\nline 2\nline 3\nline 4\nline 5\nline 6\n",
    )
    .unwrap();
    fs::write(root.join("bin.dat"), [0xffu8, 0xfe, 0x00, 0x01]).unwrap();
    let mut store = Store::open(dir.path().join("state")).unwrap();
    store.ensure_project(P, "expedition").unwrap();
    let snapshot = capture_snapshot(
        &mut store,
        P,
        &root,
        &SnapshotInputs {
            build_config: json!({"variant": "source-file"}),
            environment: json!({"os": "test"}),
        },
        &SnapshotOptions {
            include_untracked: true,
            ..SnapshotOptions::default()
        },
    )
    .unwrap();
    Fixture {
        _dir: dir,
        root,
        store,
        snapshot_id: snapshot.snapshot_id,
    }
}

fn read(fixture: &Fixture, request: Value) -> Result<Value, archmeld_core::CoreError> {
    read_source_file(&fixture.store, &request)
}

#[test]
fn source_file_excerpt_is_line_accurate_and_snapshot_bound() {
    let fixture = setup();
    let excerpt = read(
        &fixture,
        json!({
            "project_id": P,
            "snapshot_id": fixture.snapshot_id,
            "path": "src/app.py",
            "line": 4,
            "context_lines": 1,
        }),
    )
    .unwrap();
    assert_eq!(excerpt["requested_line"], 4);
    assert_eq!(excerpt["line_start"], 3);
    assert_eq!(excerpt["line_end"], 5);
    assert_eq!(excerpt["total_lines"], 7);
    assert_eq!(excerpt["text"], "line 3\nline 4\nline 5\n");
    assert_eq!(excerpt["path"], "src/app.py");
    assert!(excerpt["blob_digest"]
        .as_str()
        .unwrap()
        .starts_with("sha256:"));

    // The workspace may change afterwards; the snapshot excerpt does not.
    fs::write(fixture.root.join("src/app.py"), "changed\n").unwrap();
    let again = read(
        &fixture,
        json!({
            "project_id": P,
            "snapshot_id": fixture.snapshot_id,
            "path": "src/app.py",
            "line": 4,
            "context_lines": 1,
        }),
    )
    .unwrap();
    assert_eq!(again["text"], excerpt["text"]);
}

#[test]
fn source_file_rejects_escapes_missing_paths_and_bad_windows() {
    let fixture = setup();
    let escape = read(
        &fixture,
        json!({
            "project_id": P,
            "snapshot_id": fixture.snapshot_id,
            "path": "../etc/passwd",
            "line": 1,
        }),
    )
    .unwrap_err();
    assert!(escape.to_string().contains("must not contain '..'"), "{escape}");

    let missing = read(
        &fixture,
        json!({
            "project_id": P,
            "snapshot_id": fixture.snapshot_id,
            "path": "src/missing.py",
            "line": 1,
        }),
    )
    .unwrap_err();
    assert!(missing.to_string().contains("not part of snapshot"), "{missing}");

    let bad_context = read(
        &fixture,
        json!({
            "project_id": P,
            "snapshot_id": fixture.snapshot_id,
            "path": "src/app.py",
            "line": 1,
            "context_lines": 201,
        }),
    )
    .unwrap_err();
    assert!(bad_context.to_string().contains("context_lines"), "{bad_context}");

    let other_snapshot = read(
        &fixture,
        json!({
            "project_id": P,
            "snapshot_id": format!("sha256:{}", "0".repeat(64)),
            "path": "src/app.py",
            "line": 1,
        }),
    )
    .unwrap_err();
    assert!(other_snapshot.to_string().contains("snapshot"), "{other_snapshot}");
}

#[test]
fn source_file_clamps_lines_and_reports_truncation() {
    let fixture = setup();
    let first = read(
        &fixture,
        json!({
            "project_id": P,
            "snapshot_id": fixture.snapshot_id,
            "path": "src/app.py",
            "line": 0,
            "context_lines": 0,
        }),
    )
    .unwrap();
    assert_eq!(first["requested_line"], 1);
    assert_eq!(first["line_start"], 1);
    assert_eq!(first["line_end"], 1);
    assert_eq!(first["text"], "line 1\n");

    let truncated = read(
        &fixture,
        json!({
            "project_id": P,
            "snapshot_id": fixture.snapshot_id,
            "path": "src/app.py",
            "line": 1,
            "context_lines": 5,
            "max_bytes": 10,
        }),
    )
    .unwrap();
    assert_eq!(truncated["truncated"], true);
    assert_eq!(truncated["text"], "line 1\nlin");
}
