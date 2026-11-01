// Copyright 2026 ARCHMELD contributors
// SPDX-License-Identifier: Apache-2.0

//! AT009 (dirty workspace), AT010 (build variants), AT011/AT012 (coordinates,
//! stability) at the snapshot boundary.

use archmeld_core::hash::{digest_bytes, digest_value};
use archmeld_core::snapshot::{
    capture_snapshot, capture_snapshot_with_hook, SnapshotInputs, SnapshotOptions,
};
use archmeld_core::source::{validate_range, LineIndex};
use archmeld_core::store::Store;
use serde_json::{json, Value};
use std::fs;
use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};

const PROJECT: &str = "project.snapshot";

fn inputs(build: Value) -> SnapshotInputs {
    SnapshotInputs {
        build_config: build,
        environment: json!({"os": "test", "toolchain": "unit"}),
    }
}

fn git(root: &Path, args: &[&str]) {
    let status = std::process::Command::new("git")
        .arg("-C")
        .arg(root)
        .args(["-c", "user.name=Archmeld Test", "-c", "user.email=test@archmeld.invalid"])
        .args(args)
        .status()
        .expect("git available");
    assert!(status.success(), "git {args:?} failed");
}

fn init_repo(root: &Path) {
    git(root, &["init", "-q"]);
    git(root, &["add", "."]);
    git(root, &["commit", "-qm", "init"]);
}

fn store(dir: &Path) -> Store {
    let mut store = Store::open(dir.join("state")).expect("store");
    store.ensure_project(PROJECT, "manager").expect("project");
    store
}

#[test]
fn dirty_workspace_hashes_disk_bytes_and_keeps_git_state() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().join("repo");
    fs::create_dir_all(root.join("src")).unwrap();
    fs::write(root.join("src/app.py"), "print('v1')\n").unwrap();
    fs::write(root.join(".gitignore"), "*.log\n").unwrap();
    fs::write(root.join("debug.log"), "not included\n").unwrap();
    init_repo(&root);

    let status_before = std::process::Command::new("git")
        .arg("-C")
        .arg(&root)
        .args(["status", "--porcelain"])
        .output()
        .unwrap();
    fs::write(root.join("src/app.py"), "print('v2')\n").unwrap();
    fs::write(root.join("notes.txt"), "untracked but included\n").unwrap();
    let dirty_status = std::process::Command::new("git")
        .arg("-C")
        .arg(&root)
        .args(["status", "--porcelain"])
        .output()
        .unwrap();
    assert_ne!(status_before.stdout, dirty_status.stdout);

    let mut store = store(dir.path());
    let options = SnapshotOptions {
        include_untracked: true,
        ..SnapshotOptions::default()
    };
    let captured = capture_snapshot(
        &mut store,
        PROJECT,
        &root,
        &inputs(json!({"variant": "debug"})),
        &options,
    )
    .expect("capture");

    let app = captured
        .files
        .iter()
        .find(|file| file.path == "src/app.py")
        .expect("app.py");
    assert_eq!(app.blob_digest, digest_bytes(b"print('v2')\n"));
    assert_eq!(app.origin.as_str(), "tracked");
    let notes = captured
        .files
        .iter()
        .find(|file| file.path == "notes.txt")
        .expect("notes captured");
    assert_eq!(notes.origin.as_str(), "untracked");
    assert!(
        captured.omitted.iter().any(|item| item.path == "debug.log" && item.reason == "gitignored"),
        "ignored file must be recorded as omitted"
    );
    assert!(captured.git_commit.is_some());
    assert!(captured.snapshot_id.starts_with("sha256:"));
    assert_eq!(captured.snapshot["id"].as_str(), Some(captured.snapshot_id.as_str()));

    let after_status = std::process::Command::new("git")
        .arg("-C")
        .arg(&root)
        .args(["status", "--porcelain"])
        .output()
        .unwrap();
    assert_eq!(
        dirty_status.stdout, after_status.stdout,
        "capture must not modify the working tree"
    );

    // Snapshot identity excludes created_at and id.
    let mut identity = captured.snapshot.clone();
    identity.as_object_mut().unwrap().remove("created_at");
    identity.as_object_mut().unwrap().remove("id");
    assert_eq!(digest_value(&identity).unwrap(), captured.snapshot_id);
}

#[test]
fn same_bytes_reuse_the_snapshot_identity() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().join("repo");
    fs::create_dir_all(&root).unwrap();
    fs::write(root.join("main.py"), "x = 1\n").unwrap();
    let mut store = store(dir.path());
    let options = SnapshotOptions {
        include_untracked: true,
        ..SnapshotOptions::default()
    };
    let first = capture_snapshot(&mut store, PROJECT, &root, &inputs(json!({"v": 1})), &options)
        .expect("first");
    let second = capture_snapshot(&mut store, PROJECT, &root, &inputs(json!({"v": 1})), &options)
        .expect("second");
    assert_eq!(first.snapshot_id, second.snapshot_id);
    assert_eq!(store.snapshot_count(PROJECT).unwrap(), 1);
}

#[test]
fn build_variants_have_distinct_config_digests_and_ids() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().join("repo");
    fs::create_dir_all(&root).unwrap();
    fs::write(root.join("plugin.cpp"), "#ifdef FEATURE\nint f(){return 1;}\n#endif\n").unwrap();
    let mut store = store(dir.path());
    let options = SnapshotOptions {
        include_untracked: true,
        ..SnapshotOptions::default()
    };
    let on = capture_snapshot(
        &mut store,
        PROJECT,
        &root,
        &inputs(json!({"defines": ["FEATURE"]})),
        &options,
    )
    .expect("variant on");
    let off = capture_snapshot(
        &mut store,
        PROJECT,
        &root,
        &inputs(json!({"defines": []})),
        &options,
    )
    .expect("variant off");
    assert_ne!(on.snapshot["config_digest"], off.snapshot["config_digest"]);
    assert_ne!(on.snapshot_id, off.snapshot_id);
    assert_eq!(store.snapshot_count(PROJECT).unwrap(), 2);
}

#[test]
fn persistent_mutation_is_rejected_as_unstable() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().join("repo");
    fs::create_dir_all(&root).unwrap();
    let file = root.join("data.txt");
    fs::write(&file, "start\n").unwrap();
    let mut store = store(dir.path());
    let options = SnapshotOptions {
        include_untracked: true,
        max_retries: 2,
        ..SnapshotOptions::default()
    };
    let mut hook = {
        let file = file.clone();
        move || {
            let mut bytes = fs::read(&file).unwrap_or_default();
            bytes.extend_from_slice(b"x");
            fs::write(&file, bytes).unwrap();
        }
    };
    let err = capture_snapshot_with_hook(
        &mut store,
        PROJECT,
        &root,
        &inputs(json!({"v": 1})),
        &options,
        Some(&mut hook),
    )
    .expect_err("unstable workspace must not publish");
    assert!(err.to_string().contains("unstable_workspace"), "{err}");
    assert_eq!(store.snapshot_count(PROJECT).unwrap(), 0);
}

#[test]
fn single_mutation_is_retried_and_new_bytes_are_captured() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().join("repo");
    fs::create_dir_all(&root).unwrap();
    let file = root.join("data.txt");
    fs::write(&file, "start\n").unwrap();
    let mut store = store(dir.path());
    let options = SnapshotOptions {
        include_untracked: true,
        max_retries: 2,
        ..SnapshotOptions::default()
    };
    let mutated = AtomicBool::new(false);
    let mut hook = {
        let file = file.clone();
        move || {
            if !mutated.swap(true, Ordering::SeqCst) {
                fs::write(&file, "start\nmore\n").unwrap();
            }
        }
    };
    let captured = capture_snapshot_with_hook(
        &mut store,
        PROJECT,
        &root,
        &inputs(json!({"v": 1})),
        &options,
        Some(&mut hook),
    )
    .expect("retry captures stable bytes");
    let entry = captured
        .files
        .iter()
        .find(|file| file.path == "data.txt")
        .unwrap();
    assert_eq!(entry.blob_digest, digest_bytes(b"start\nmore\n"));
    assert_eq!(store.snapshot_count(PROJECT).unwrap(), 1);
}

#[test]
fn utf8_ranges_and_utf16_columns_cover_cjk_and_emoji() {
    let text = "路径=😀\r\nsecond\n";
    let bytes = text.as_bytes();
    let index = LineIndex::new(bytes);
    assert!(validate_range(bytes, 0, bytes.len() as u64));
    let (line, _) = index.line_col(0).unwrap();
    assert_eq!(line, 0);
    // "路径" are two 3-byte chars, '=' 1 byte, emoji 4 bytes.
    assert_eq!(index.utf16_col_to_byte(bytes, 0, 0), Some(0));
    assert_eq!(index.utf16_col_to_byte(bytes, 0, 1), Some(3));
    assert_eq!(index.utf16_col_to_byte(bytes, 0, 2), Some(6));
    assert_eq!(index.utf16_col_to_byte(bytes, 0, 3), Some(7), "emoji start");
    assert_eq!(index.utf16_col_to_byte(bytes, 0, 4), None, "inside surrogate pair");
    assert_eq!(index.utf16_col_to_byte(bytes, 0, 5), Some(11), "after the emoji");
    assert_eq!(index.utf16_col_to_byte(bytes, 0, 6), Some(12), "CR is a real code unit");
    assert_eq!(index.utf16_col_to_byte(bytes, 1, 6), Some(19));
    assert_eq!(index.byte_to_utf16_col(bytes, 11), Some(5));
}
