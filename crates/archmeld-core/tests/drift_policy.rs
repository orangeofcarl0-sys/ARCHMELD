// Copyright 2026 ARCHMELD contributors
// SPDX-License-Identifier: Apache-2.0

//! Drift must apply the snapshot's workspace policy, not a weaker walker.

use archmeld_core::drift::analyze_drift;
use archmeld_core::snapshot::{capture_snapshot, SnapshotInputs, SnapshotOptions};
use archmeld_core::store::Store;
use serde_json::json;
use std::fs;
use std::path::Path;
use std::process::Command;

const PROJECT: &str = "project.drift.policy";

fn git(root: &Path, args: &[&str]) {
    let status = Command::new("git")
        .arg("-C")
        .arg(root)
        .args(args)
        .status()
        .expect("git");
    assert!(status.success(), "git {args:?} failed");
}

#[test]
fn ignored_build_output_is_omitted_not_added_and_new_source_is_reviewed() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().join("repo");
    fs::create_dir_all(root.join("src")).unwrap();
    fs::write(root.join(".gitignore"), "target-linux/\n*.log\n").unwrap();
    fs::write(root.join("src/a.py"), "VALUE = 1\n").unwrap();
    git(&root, &["init", "-q"]);
    git(&root, &["config", "user.email", "test@example.invalid"]);
    git(&root, &["config", "user.name", "test"]);
    git(&root, &["add", "."]);
    git(&root, &["commit", "-qm", "base"]);

    let mut store = Store::open(dir.path().join("state")).unwrap();
    store.ensure_project(PROJECT, "expedition").unwrap();
    let captured = capture_snapshot(
        &mut store,
        PROJECT,
        &root,
        &SnapshotInputs {
            build_config: json!({"variant": "test"}),
            environment: json!({"os": "test"}),
        },
        &SnapshotOptions {
            include_untracked: true,
            ..SnapshotOptions::default()
        },
    )
    .unwrap();
    assert!(captured.snapshot["workspace_policy"].is_object());
    assert!(captured.snapshot["git_tree_digest"].is_string());

    fs::create_dir_all(root.join("target-linux")).unwrap();
    fs::write(root.join("target-linux/artifact.bin"), vec![0u8; 4096]).unwrap();
    fs::write(root.join("build.log"), "ignored\n").unwrap();
    let clean = analyze_drift(&store, PROJECT, &captured.snapshot_id, &root, None).unwrap();
    assert_eq!(clean["state"], "clean", "{clean}");
    assert!(clean["added"].as_array().unwrap().is_empty(), "{clean}");
    assert!(clean["omitted"].as_array().unwrap().is_empty(), "{clean}");
    assert_eq!(clean["needs_review"], false);
    assert!(clean["policy"]["gitignored"].as_u64().unwrap() >= 2);

    fs::write(root.join("src/b.py"), "OTHER = 2\n").unwrap();
    let changed = analyze_drift(&store, PROJECT, &captured.snapshot_id, &root, None).unwrap();
    assert_eq!(changed["state"], "changed");
    assert_eq!(changed["needs_review"], true);
    let added = changed["added"].as_array().unwrap();
    assert_eq!(added.len(), 1);
    assert_eq!(added[0]["path"], "src/b.py");
}

#[test]
fn git_tree_digest_is_line_ending_independent_while_snapshot_id_tracks_bytes() {
    let dir = tempfile::tempdir().unwrap();
    let lf_root = dir.path().join("lf");
    let crlf_root = dir.path().join("crlf");
    for (root, autocrlf, bytes) in [
        (&lf_root, "false", "VALUE = 1\n"),
        (&crlf_root, "true", "VALUE = 1\r\n"),
    ] {
        fs::create_dir_all(root.join("src")).unwrap();
        fs::write(root.join(".gitattributes"), "* text=auto\n").unwrap();
        fs::write(root.join("src/a.py"), bytes).unwrap();
        git(root, &["init", "-q"]);
        git(root, &["config", "user.email", "test@example.invalid"]);
        git(root, &["config", "user.name", "test"]);
        git(root, &["config", "core.autocrlf", autocrlf]);
        git(root, &["add", "."]);
        git(root, &["commit", "-qm", "base"]);
    }

    let mut store = Store::open(dir.path().join("state")).unwrap();
    store.ensure_project("project.crlf.lf", "expedition").unwrap();
    store
        .ensure_project("project.crlf.crlf", "expedition")
        .unwrap();
    let inputs = SnapshotInputs {
        build_config: json!({"variant": "test"}),
        environment: json!({"os": "test"}),
    };
    let options = SnapshotOptions {
        include_untracked: false,
        ..SnapshotOptions::default()
    };
    let lf = capture_snapshot(&mut store, "project.crlf.lf", &lf_root, &inputs, &options).unwrap();
    let crlf =
        capture_snapshot(&mut store, "project.crlf.crlf", &crlf_root, &inputs, &options).unwrap();

    assert_eq!(
        lf.snapshot["git_tree_digest"], crlf.snapshot["git_tree_digest"],
        "same blobs in the index must have the same content identity"
    );
    assert_ne!(
        lf.snapshot_id, crlf.snapshot_id,
        "the byte-level snapshot ID must still distinguish LF from CRLF"
    );
}

#[test]
fn omitted_base_files_are_not_reported_as_deleted() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().join("repo");
    fs::create_dir_all(root.join("src")).unwrap();
    fs::create_dir_all(root.join("scripts")).unwrap();
    fs::write(root.join("src/a.py"), "VALUE = 1\n").unwrap();
    // A file whose basename matches an excluded directory name is still a
    // file; capture includes it and drift must not exclude it.
    fs::write(root.join("scripts/build"), "not a directory\n").unwrap();
    git(&root, &["init", "-q"]);
    git(&root, &["config", "user.email", "test@example.invalid"]);
    git(&root, &["config", "user.name", "test"]);
    git(&root, &["add", "."]);
    git(&root, &["commit", "-qm", "base"]);

    let mut store = Store::open(dir.path().join("state")).unwrap();
    store.ensure_project(PROJECT, "expedition").unwrap();
    let captured = capture_snapshot(
        &mut store,
        PROJECT,
        &root,
        &SnapshotInputs {
            build_config: json!({"variant": "omitted-base"}),
            environment: json!({"os": "test"}),
        },
        &SnapshotOptions {
            include_untracked: true,
            ..SnapshotOptions::default()
        },
    )
    .unwrap();
    fs::write(root.join("src/a.py"), [0u8, 1, 2, 3, 4]).unwrap();

    let report = analyze_drift(&store, PROJECT, &captured.snapshot_id, &root, None).unwrap();
    assert!(
        report["deleted"]
            .as_array()
            .unwrap()
            .iter()
            .all(|item| item["path"] != "src/a.py"),
        "{report}"
    );
    assert!(
        report["omitted_base"]
            .as_array()
            .unwrap()
            .iter()
            .any(|item| item["path"] == "src/a.py"),
        "{report}"
    );
    assert_eq!(report["coverage"]["state"], "partial", "{report}");
    assert_eq!(report["needs_review"], true, "{report}");
    assert!(
        report["deleted"]
            .as_array()
            .unwrap()
            .iter()
            .all(|item| item["path"] != "scripts/build"),
        "a file named build must not be treated as an excluded directory: {report}"
    );
}
