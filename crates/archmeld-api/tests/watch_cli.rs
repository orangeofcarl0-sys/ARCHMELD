// Copyright 2026 ARCHMELD contributors
// SPDX-License-Identifier: Apache-2.0

//! CLI watch mode wiring: `--mode os` selects the notify watcher and still
//! writes the initial report without touching the workspace.

use archmeld_core::snapshot::{capture_snapshot, SnapshotInputs, SnapshotOptions};
use archmeld_core::store::Store;
use serde_json::{json, Value};
use std::fs;
use std::process::Command;

#[test]
fn cli_watch_os_mode_runs_the_notify_watcher() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().join("repo");
    fs::create_dir_all(&root).unwrap();
    fs::write(root.join("a.py"), "VALUE = 1\n").unwrap();
    let state = dir.path().join("state");
    let snapshot_id;
    {
        let mut store = Store::open(&state).unwrap();
        store.ensure_project("project.cli.watch", "expedition").unwrap();
        let snapshot = capture_snapshot(
            &mut store,
            "project.cli.watch",
            &root,
            &SnapshotInputs {
                build_config: json!({"variant": "cli-watch"}),
                environment: json!({"os": "test"}),
            },
            &SnapshotOptions {
                include_untracked: true,
                ..SnapshotOptions::default()
            },
        )
        .unwrap();
        snapshot_id = snapshot.snapshot_id;
    }
    let output = Command::new(env!("CARGO_BIN_EXE_archmeld"))
        .args([
            "watch",
            "--state",
            &state.display().to_string(),
            "--project",
            "project.cli.watch",
            "--snapshot",
            &snapshot_id,
            "--workspace",
            &root.display().to_string(),
            "--mode",
            "os",
            "--iterations",
            "1",
            "--debounce-ms",
            "50",
        ])
        .output()
        .expect("archmeld CLI runs");
    assert!(
        output.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let value: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(value["mode"], "os");
    let result = &value["result"];
    assert_eq!(result["watcher_kind"], "os_notify");
    assert_eq!(result["watcher_backend"], "notify::RecommendedWatcher");
    assert_eq!(result["ticks"], 1);
    assert_eq!(result["reports"][0]["state"], "clean");
    assert_eq!(result["source_mutation"], "none");
    assert_eq!(fs::read_to_string(root.join("a.py")).unwrap(), "VALUE = 1\n");
}
