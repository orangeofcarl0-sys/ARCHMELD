// Copyright 2026 ARCHMELD contributors
// SPDX-License-Identifier: Apache-2.0

//! B025: polling drift watcher persists repeated reports, sees a workspace
//! change, and never writes the workspace itself.

use archmeld_core::drift::list_drift_reports;
use archmeld_core::hash;
use archmeld_core::snapshot::{capture_snapshot, SnapshotInputs, SnapshotOptions};
use archmeld_core::store::Store;
use archmeld_core::watch::{watch_drift, watch_drift_notify, NotifyWatchOptions, WatchOptions};
use serde_json::json;
use std::fs;
use std::time::Duration;

const PROJECT: &str = "project.watch";

#[test]
fn polling_watcher_records_drift_and_keeps_the_workspace_read_only() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().join("repo");
    fs::create_dir_all(root.join("src")).unwrap();
    fs::write(root.join("src/a.py"), "VALUE = 1\n").unwrap();
    let mut store = Store::open(dir.path().join("state")).unwrap();
    store.ensure_project(PROJECT, "expedition").unwrap();
    let snapshot = capture_snapshot(
        &mut store,
        PROJECT,
        &root,
        &SnapshotInputs {
            build_config: json!({"variant": "watch"}),
            environment: json!({"os": "test"}),
        },
        &SnapshotOptions {
            include_untracked: true,
            ..SnapshotOptions::default()
        },
    )
    .unwrap();
    fs::write(root.join("src/a.py"), "VALUE = 2\n").unwrap();
    let before = hash::digest_bytes(&fs::read(root.join("src/a.py")).unwrap());

    let result = watch_drift(
        &mut store,
        &WatchOptions {
            project_id: PROJECT.to_string(),
            base_snapshot_id: snapshot.snapshot_id.clone(),
            root: root.clone(),
            interval: Duration::from_millis(20),
            iterations: Some(2),
            stop: None,
        },
    )
    .unwrap();
    assert_eq!(result["ticks"], 2, "{result}");
    assert_eq!(result["watcher_kind"], "polling");
    assert_eq!(result["source_mutation"], "none");
    let reports = result["reports"].as_array().unwrap();
    assert_eq!(reports.len(), 2);
    for report in reports {
        assert_ne!(report["state"], "clean", "{report}");
        assert_eq!(report["changed"][0]["path"], "src/a.py");
    }
    assert_eq!(
        hash::digest_bytes(&fs::read(root.join("src/a.py")).unwrap()),
        before
    );
    let stored = list_drift_reports(&store, PROJECT).unwrap();
    assert_eq!(stored.len(), 2);
    assert!(store.integrity_check().unwrap().is_empty());

    // A stop flag before the first tick produces zero ticks, not a fake run.
    let flag = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(true));
    let stopped = watch_drift(
        &mut store,
        &WatchOptions {
            project_id: PROJECT.to_string(),
            base_snapshot_id: snapshot.snapshot_id,
            root,
            interval: Duration::from_millis(20),
            iterations: Some(5),
            stop: Some(flag),
        },
    )
    .unwrap();
    assert_eq!(stopped["ticks"], 0);
    assert_eq!(stopped["stopped_by"], "stop_flag");
}

#[test]
fn os_notification_watcher_reacts_to_a_real_file_change_without_polling() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().join("repo");
    fs::create_dir_all(root.join("src")).unwrap();
    fs::write(root.join("src/a.py"), "VALUE = 1\n").unwrap();
    let mut store = Store::open(dir.path().join("state")).unwrap();
    store.ensure_project(PROJECT, "expedition").unwrap();
    let snapshot = capture_snapshot(
        &mut store,
        PROJECT,
        &root,
        &SnapshotInputs {
            build_config: json!({"variant": "watch-os"}),
            environment: json!({"os": "test"}),
        },
        &SnapshotOptions {
            include_untracked: true,
            ..SnapshotOptions::default()
        },
    )
    .unwrap();
    let file = root.join("src/a.py");
    let writer = file.clone();
    let changer = std::thread::spawn(move || {
        std::thread::sleep(Duration::from_millis(300));
        fs::write(&writer, "VALUE = 2\n").unwrap();
    });
    let started = std::time::Instant::now();
    let result = watch_drift_notify(
        &mut store,
        &NotifyWatchOptions {
            base: WatchOptions {
                project_id: PROJECT.to_string(),
                base_snapshot_id: snapshot.snapshot_id.clone(),
                root: root.clone(),
                // Deliberately huge: this branch must not poll.
                interval: Duration::from_secs(3600),
                iterations: Some(2),
                stop: None,
            },
            debounce: Duration::from_millis(120),
        },
    )
    .unwrap();
    changer.join().unwrap();
    assert!(
        started.elapsed() < Duration::from_secs(10),
        "OS watcher returned quickly"
    );
    assert_eq!(result["ticks"], 2, "{result}");
    assert_eq!(result["watcher_kind"], "os_notify");
    assert_eq!(result["watcher_backend"], "notify::RecommendedWatcher");
    assert_eq!(result["source_mutation"], "none");
    let reports = result["reports"].as_array().unwrap();
    assert_eq!(reports.len(), 2);
    assert_eq!(reports[0]["state"], "clean");
    assert_ne!(reports[1]["state"], "clean");
    assert_eq!(reports[1]["changed"][0]["path"], "src/a.py");
    assert_eq!(fs::read_to_string(&file).unwrap(), "VALUE = 2\n");
    let stored = list_drift_reports(&store, PROJECT).unwrap();
    assert_eq!(stored.len(), 2, "initial report plus the change report");
    assert!(store.integrity_check().unwrap().is_empty());
}

#[test]
fn os_notification_watcher_honors_a_stop_flag_between_events() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().join("repo");
    fs::create_dir_all(&root).unwrap();
    fs::write(root.join("a.py"), "VALUE = 1\n").unwrap();
    let mut store = Store::open(dir.path().join("state-os-stop")).unwrap();
    store.ensure_project("project.watch.os.stop", "expedition").unwrap();
    let snapshot = capture_snapshot(
        &mut store,
        "project.watch.os.stop",
        &root,
        &SnapshotInputs {
            build_config: json!({"variant": "watch-os-stop"}),
            environment: json!({"os": "test"}),
        },
        &SnapshotOptions {
            include_untracked: true,
            ..SnapshotOptions::default()
        },
    )
    .unwrap();
    let flag = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
    let setter = flag.clone();
    let stopper = std::thread::spawn(move || {
        std::thread::sleep(Duration::from_millis(250));
        setter.store(true, std::sync::atomic::Ordering::SeqCst);
    });
    let result = watch_drift_notify(
        &mut store,
        &NotifyWatchOptions {
            base: WatchOptions {
                project_id: "project.watch.os.stop".to_string(),
                base_snapshot_id: snapshot.snapshot_id,
                root,
                interval: Duration::from_secs(3600),
                iterations: None,
                stop: Some(flag),
            },
            debounce: Duration::from_millis(100),
        },
    )
    .unwrap();
    stopper.join().unwrap();
    assert_eq!(result["ticks"], 1, "only the initial report");
    assert_eq!(result["stopped_by"], "stop_flag");
    assert_eq!(result["reports"][0]["state"], "clean");
}
