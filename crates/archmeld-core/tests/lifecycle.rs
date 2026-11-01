// Copyright 2026 ARCHMELD contributors
// SPDX-License-Identifier: Apache-2.0

//! AT019 (Expedition does not pollute), AT020 (Manager continuity across
//! restart and relocation), AT080 (no implicit side effects) at the lifecycle
//! boundary.

use archmeld_core::bridge::{import_codegraph_export, BridgeConfig};
use archmeld_core::hash::digest_bytes;
use archmeld_core::manager::{
    close_project, get_project, list_roots, list_sessions, open_project,
    promote_project, recover_interrupted_sessions, OpenRequest,
};
use archmeld_core::snapshot::{capture_snapshot, SnapshotInputs, SnapshotOptions};
use archmeld_core::store::Store;
use serde_json::{json, Value};
use std::fs;
use std::path::Path;

fn config(project_id: &str, mode: &str) -> Value {
    json!({
        "schema_version": "1.0",
        "project_id": project_id,
        "mode": mode,
        "default_trust": "read_only",
        "telemetry": false,
        "auto_model_download": false,
        "source_egress": "deny",
        "include_globs": ["**/*"],
        "exclude_globs": [],
        "language_profiles": ["python"],
        "instigator_enabled": false,
        "retention_unpinned_snapshots": 30,
        "persist_config_in_repository": false,
    })
}

fn tree_digest(root: &Path) -> String {
    let mut parts = Vec::new();
    fn walk(root: &Path, dir: &Path, parts: &mut Vec<String>) {
        let mut entries: Vec<_> = fs::read_dir(dir).unwrap().filter_map(Result::ok).collect();
        entries.sort_by_key(|entry| entry.path());
        for entry in entries {
            let path = entry.path();
            let name = entry.file_name();
            if name == ".git" {
                continue;
            }
            if path.is_dir() {
                walk(root, &path, parts);
            } else {
                let relative = path.strip_prefix(root).unwrap().to_string_lossy().replace('\\', "/");
                let bytes = fs::read(&path).unwrap();
                parts.push(format!("{relative}:{}", digest_bytes(&bytes)));
            }
        }
    }
    walk(root, root, &mut parts);
    digest_bytes(parts.join("\n").as_bytes())
}

fn git(root: &Path, args: &[&str]) -> String {
    let output = std::process::Command::new("git")
        .arg("-C")
        .arg(root)
        .args(["-c", "user.name=Archmeld Test", "-c", "user.email=test@archmeld.invalid"])
        .args(args)
        .output()
        .expect("git");
    String::from_utf8_lossy(&output.stdout).to_string()
}

fn dirty_repo(dir: &Path) -> std::path::PathBuf {
    let root = dir.join("repo");
    fs::create_dir_all(root.join("src")).unwrap();
    fs::write(root.join("src/app.py"), "def main():\n    return 1\n").unwrap();
    git(&root, &["init", "-q"]);
    git(&root, &["add", "."]);
    git(&root, &["commit", "-qm", "init"]);
    fs::write(root.join("src/app.py"), "def main():\n    return 2\n").unwrap();
    fs::write(root.join("notes.txt"), "untracked\n").unwrap();
    root
}

fn analyze(store: &mut Store, project_id: &str, root: &Path) -> String {
    let captured = capture_snapshot(
        store,
        project_id,
        root,
        &SnapshotInputs {
            build_config: json!({"variant": "lifecycle"}),
            environment: json!({"os": "test"}),
        },
        &SnapshotOptions {
            include_untracked: true,
            ..SnapshotOptions::default()
        },
    )
    .unwrap();
    let config_digest = captured.snapshot["config_digest"].as_str().unwrap().to_string();
    let export = vec![
        json!({
            "type":"manifest","format":"archmeld-export-v1","source":"codegraph",
            "version":"0.2.0","node_count":1,"edge_count":0,"iteration":"real-iterators"
        })
        .to_string(),
        json!({
            "type":"node","id":1,"node_type":"Function","properties":{
                "path": root.join("src/app.py").to_string_lossy(),"name":"main",
                "line_start":1,"line_end":2
            }
        })
        .to_string(),
    ]
    .join("\n")
        + "\n";
    import_codegraph_export(
        store,
        &BridgeConfig {
            project_id,
            snapshot_id: &captured.snapshot_id,
            config_digest: &config_digest,
            run_id: "run.lifecycle.001",
            workspace_root: Some(root),
            max_entities_per_batch: 500,
            max_relations_per_batch: 2000,
        },
        &captured.files,
        &export,
    )
    .unwrap();
    captured.snapshot_id
}

#[test]
fn expedition_analysis_and_close_leave_the_dirty_repo_untouched() {
    let dir = tempfile::tempdir().unwrap();
    let root = dirty_repo(dir.path());
    let state = dir.path().join("expedition-state");
    let tree_before = tree_digest(&root);
    let status_before = git(&root, &["status", "--porcelain"]);

    let mut store = Store::open(dir.path().join("control")).unwrap();
    let project = open_project(
        &mut store,
        OpenRequest {
            project_id: "project.expedition",
            mode: "expedition",
            workspace_root: &root,
            state_root: &state,
            config: config("project.expedition", "expedition"),
        },
    )
    .unwrap();
    assert_eq!(project["mode"], "expedition");
    let snapshot = analyze(&mut store, "project.expedition", &root);

    // Rebuildable scratch that a real run could leave behind.
    fs::create_dir_all(state.join("cache/tool")).unwrap();
    fs::write(state.join("cache/tool/index.bin"), b"cache").unwrap();
    fs::create_dir_all(state.join("scratch")).unwrap();
    fs::write(state.join("scratch/tmp.txt"), b"tmp").unwrap();

    let closed = close_project(&mut store, "project.expedition", true).unwrap();
    assert_eq!(closed["state"], "closed");
    assert!(
        dir.path().join("control/state.sqlite3").exists(),
        "the external ledger survives close"
    );
    assert!(!state.join("cache").exists(), "expedition cache purged");
    assert!(!state.join("scratch").exists(), "expedition scratch purged");

    assert_eq!(tree_digest(&root), tree_before, "workspace bytes changed");
    assert_eq!(git(&root, &["status", "--porcelain"]), status_before);

    let counts = store.published_counts("project.expedition", &snapshot).unwrap();
    assert_eq!(counts.entities, 1);
    assert_eq!(store.snapshot_count("project.expedition").unwrap(), 1);
    let sessions = list_sessions(&store, "project.expedition").unwrap();
    assert_eq!(sessions.last().unwrap().state, "closed");
}

#[test]
fn manager_survives_restart_relocation_without_changing_identity() {
    let dir = tempfile::tempdir().unwrap();
    let root = dirty_repo(dir.path());
    let state = dir.path().join("manager-state");
    let control = dir.path().join("control");

    let snapshot;
    {
        let mut store = Store::open(&control).unwrap();
        open_project(
            &mut store,
            OpenRequest {
                project_id: "project.manager",
                mode: "manager",
                workspace_root: &root,
                state_root: &state,
                config: config("project.manager", "manager"),
            },
        )
        .unwrap();
        snapshot = analyze(&mut store, "project.manager", &root);
        // No close: simulate a crash so recovery has something to report.
    }

    let mut store = Store::open(&control).unwrap();
    assert_eq!(recover_interrupted_sessions(&mut store).unwrap(), 1);
    let recovered = get_project(&store, "project.manager").unwrap().unwrap();
    assert_eq!(recovered["state"], "recovering");
    assert_eq!(list_sessions(&store, "project.manager").unwrap()[0].state, "interrupted");

    let moved = dir.path().join("repo-moved");
    fs::rename(&root, &moved).unwrap();
    let reopened = open_project(
        &mut store,
        OpenRequest {
            project_id: "project.manager",
            mode: "manager",
            workspace_root: &moved,
            state_root: &state,
            config: config("project.manager", "manager"),
        },
    )
    .unwrap();
    assert_eq!(reopened["id"], "project.manager");
    assert_eq!(reopened["state"], "open");
    assert!(reopened["revision"].as_i64().unwrap() >= 2);

    let roots = list_roots(&store, "project.manager").unwrap();
    assert_eq!(roots.len(), 2, "both locations are retained as history");
    let active: Vec<_> = roots.iter().filter(|root| root.active).collect();
    assert_eq!(active.len(), 1);
    assert!(active[0].root_path.ends_with("repo-moved"));

    let counts = store.published_counts("project.manager", &snapshot).unwrap();
    assert_eq!(counts.entities, 1, "evidence survives restart and relocation");
    assert_eq!(store.snapshot_count("project.manager").unwrap(), 1);
}

#[test]
fn promotion_keeps_evidence_and_turns_expedition_into_manager() {
    let dir = tempfile::tempdir().unwrap();
    let root = dirty_repo(dir.path());
    let state = dir.path().join("promote-state");
    let mut store = Store::open(dir.path().join("control")).unwrap();
    open_project(
        &mut store,
        OpenRequest {
            project_id: "project.promote",
            mode: "expedition",
            workspace_root: &root,
            state_root: &state,
            config: config("project.promote", "expedition"),
        },
    )
    .unwrap();
    let snapshot = analyze(&mut store, "project.promote", &root);
    let before = store.published_counts("project.promote", &snapshot).unwrap();

    let promoted = promote_project(&mut store, "project.promote").unwrap();
    assert_eq!(promoted["mode"], "manager");
    assert_eq!(promoted["state"], "open");
    let after = store.published_counts("project.promote", &snapshot).unwrap();
    assert_eq!(before, after, "promotion must not drop evidence");
    let sessions = list_sessions(&store, "project.promote").unwrap();
    assert!(sessions.iter().any(|session| session.state == "promoted"));
    assert!(sessions.iter().any(|session| session.mode == "manager" && session.state == "open"));
}

#[test]
fn lifecycle_rejects_pollution_paths_and_mode_changes() {
    let dir = tempfile::tempdir().unwrap();
    let root = dirty_repo(dir.path());
    let mut store = Store::open(dir.path().join("control")).unwrap();
    let err = open_project(
        &mut store,
        OpenRequest {
            project_id: "project.bad",
            mode: "expedition",
            workspace_root: &root,
            state_root: &root.join("inside-cache"),
            config: config("project.bad", "expedition"),
        },
    )
    .expect_err("state root inside workspace");
    assert!(err.to_string().contains("state_root_inside_workspace"), "{err}");

    open_project(
        &mut store,
        OpenRequest {
            project_id: "project.bad",
            mode: "expedition",
            workspace_root: &root,
            state_root: &dir.path().join("external"),
            config: config("project.bad", "expedition"),
        },
    )
    .unwrap();
    let err = open_project(
        &mut store,
        OpenRequest {
            project_id: "project.bad",
            mode: "manager",
            workspace_root: &root,
            state_root: &dir.path().join("external"),
            config: config("project.bad", "manager"),
        },
    )
    .expect_err("mode change needs promote");
    assert!(err.to_string().contains("use promote"), "{err}");
}

#[test]
fn both_modes_open_without_touching_the_workspace() {
    let dir = tempfile::tempdir().unwrap();
    let root = dirty_repo(dir.path());
    let tree_before = tree_digest(&root);
    let status_before = git(&root, &["status", "--porcelain"]);
    let mut store = Store::open(dir.path().join("control")).unwrap();
    for (mode, id) in [("manager", "project.m"), ("expedition", "project.e")] {
        open_project(
            &mut store,
            OpenRequest {
                project_id: id,
                mode,
                workspace_root: &root,
                state_root: &dir.path().join(format!("state-{mode}")),
                config: config(id, mode),
            },
        )
        .unwrap();
        let project = get_project(&store, id).unwrap().unwrap();
        assert_eq!(project["mode"], mode);
        close_project(&mut store, id, true).unwrap();
    }
    assert_eq!(tree_digest(&root), tree_before);
    assert_eq!(git(&root, &["status", "--porcelain"]), status_before);
    assert!(!root.join("cache").exists());
    assert!(!root.join(".archmeld").exists());
}
