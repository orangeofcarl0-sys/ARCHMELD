// Copyright 2026 ARCHMELD contributors
// SPDX-License-Identifier: Apache-2.0

//! Task-level diff review: the workspace is compared with a frozen snapshot,
//! changed entities are located by line overlap, graph impact stays static, and
//! no build/test is executed. Snapshot-to-snapshot diff is exact.

use archmeld_core::bridge::{import_codegraph_export, BridgeConfig};
use archmeld_core::brief::{build_brief, BriefRecipe, BriefRequest};
use archmeld_core::drift::{analyze_drift, gc};
use archmeld_core::review::{
    diff_snapshots, freeze_review_candidate, persist_review_report, review_workspace_diff,
};
use archmeld_core::retention::{configure_retention, prune_snapshots};
use archmeld_core::snapshot::{capture_snapshot, SnapshotInputs, SnapshotOptions};
use archmeld_core::store::Store;
use serde_json::{json, Value};
use std::fs;
use std::path::Path;

const PROJECT: &str = "project.diff.review";

fn node(root: &Path, id: u64, path: &str, name: &str, start: u64, end: u64) -> Value {
    json!({
        "type":"node","id":id,"node_type":"Function","properties":{
            "path": root.join(path).to_string_lossy(),
            "name": name, "line_start": start, "line_end": end
        }
    })
}

fn edge(id: u64, source: u64, target: u64) -> Value {
    edge_typed(id, source, target, "Calls")
}

fn edge_typed(id: u64, source: u64, target: u64, kind: &str) -> Value {
    json!({
        "type":"edge","id":id,"source":source,"target":target,
        "edge_type":kind,"properties":{"line":2}
    })
}

fn import(
    store: &mut Store,
    snapshot_id: &str,
    config_digest: &str,
    run_id: &str,
    files: &[archmeld_core::store::FileEntry],
    root: &Path,
    nodes: &[Value],
    edges: &[Value],
) {
    let mut lines = vec![json!({
        "type":"manifest","format":"archmeld-export-v1","source":"codegraph",
        "version":"0.2.0","node_count":nodes.len(),"edge_count":edges.len(),
        "iteration":"real-iterators"
    })
    .to_string()];
    for node in nodes {
        lines.push(node.to_string());
    }
    for edge in edges {
        lines.push(edge.to_string());
    }
    import_codegraph_export(
        store,
        &BridgeConfig {
            project_id: PROJECT,
            snapshot_id,
            config_digest,
            run_id,
            workspace_root: Some(root),
            max_entities_per_batch: 500,
            max_relations_per_batch: 2000,
        },
        files,
        &(lines.join("\n") + "\n"),
    )
    .unwrap();
}

fn entity_id(store: &Store, snapshot_id: &str, name: &str) -> String {
    store
        .entity_ids(PROJECT, snapshot_id)
        .unwrap()
        .into_iter()
        .find(|id| {
            store
                .get_entity(PROJECT, snapshot_id, id)
                .unwrap()
                .and_then(|entity| entity["qualified_name"].as_str().map(str::to_string))
                .is_some_and(|qualified| qualified.ends_with(&format!("::{name}")))
        })
        .unwrap_or_else(|| panic!("entity {name} not found"))
}

#[test]
fn workspace_review_locates_changed_entities_and_snapshot_diff_is_exact() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().join("repo");
    fs::create_dir_all(root.join("src")).unwrap();
    fs::write(root.join("src/a.py"), "def a1():\n    return b()\n").unwrap();
    fs::write(root.join("src/b.py"), "def b():\n    return 1\n").unwrap();
    fs::write(root.join("src/c.py"), "def c():\n    return 3\n").unwrap();
    let mut store = Store::open(dir.path().join("state")).unwrap();
    store.ensure_project(PROJECT, "expedition").unwrap();
    let inputs = SnapshotInputs {
        build_config: json!({"variant": "diff-review"}),
        environment: json!({"os": "test"}),
    };
    let first = capture_snapshot(
        &mut store,
        PROJECT,
        &root,
        &inputs,
        &SnapshotOptions {
            include_untracked: true,
            ..SnapshotOptions::default()
        },
    )
    .unwrap();
    let config = first.snapshot["config_digest"].as_str().unwrap().to_string();
    let nodes = [
        node(&root, 1, "src/a.py", "a1", 1, 2),
        node(&root, 2, "src/b.py", "b", 1, 2),
        node(&root, 3, "src/c.py", "c", 1, 2),
    ];
    import(
        &mut store,
        &first.snapshot_id,
        &config,
        "run.diff.first",
        &first.files,
        &root,
        &nodes,
        &[edge(1, 1, 2)],
    );
    let a1 = entity_id(&store, &first.snapshot_id, "a1");

    fs::write(root.join("src/a.py"), "def a1():\n    return 40\n").unwrap();
    fs::write(root.join("src/new.py"), "def new_one():\n    return 9\n").unwrap();
    fs::remove_file(root.join("src/c.py")).unwrap();

    let review = review_workspace_diff(&store, PROJECT, &first.snapshot_id, &root).unwrap();
    assert_eq!(review["state"], "changed");
    assert_eq!(review["summary"]["files"]["changed"], 1);
    assert_eq!(review["summary"]["files"]["added"], 1);
    assert_eq!(review["summary"]["files"]["deleted"], 1);
    assert_eq!(review["verification"]["state"], "not_run");
    let a_file = review["files"]
        .as_array()
        .unwrap()
        .iter()
        .find(|file| file["path"] == "src/a.py")
        .unwrap();
    assert_eq!(a_file["state"], "changed");
    assert!(!a_file["line_changes"].as_array().unwrap().is_empty());
    let a_entity = a_file["entities"]
        .as_array()
        .unwrap()
        .iter()
        .find(|entity| entity["entity_id"] == a1)
        .unwrap();
    assert_eq!(a_entity["state"], "changed");
    assert_eq!(a_entity["confidence"], "static_line_overlap");
    assert!(a_entity["impact"]
        .as_array()
        .unwrap()
        .iter()
        .any(|impact| impact["other_qualified_name"]
            .as_str()
            .unwrap_or("")
            .ends_with("::b")));
    let deleted = review["files"]
        .as_array()
        .unwrap()
        .iter()
        .find(|file| file["path"] == "src/c.py")
        .unwrap();
    assert_eq!(deleted["state"], "deleted");
    assert!(deleted["entities"]
        .as_array()
        .unwrap()
        .iter()
        .any(|entity| entity["state"] == "deleted"));
    let added = review["files"]
        .as_array()
        .unwrap()
        .iter()
        .find(|file| file["path"] == "src/new.py")
        .unwrap();
    assert_eq!(added["confidence"], "no_base_entity_mapping");
    assert_eq!(review["coverage"]["state"], "partial");
    assert_eq!(review["source_mutation"], "none");
    assert_eq!(review["history_mutation"], "none");

    let second = capture_snapshot(
        &mut store,
        PROJECT,
        &root,
        &inputs,
        &SnapshotOptions {
            include_untracked: true,
            ..SnapshotOptions::default()
        },
    )
    .unwrap();
    let second_nodes = [
        node(&root, 11, "src/a.py", "a1", 1, 2),
        node(&root, 12, "src/b.py", "b", 1, 2),
        node(&root, 13, "src/new.py", "new_one", 1, 2),
    ];
    import(
        &mut store,
        &second.snapshot_id,
        &config,
        "run.diff.second",
        &second.files,
        &root,
        &second_nodes,
        &[edge(11, 11, 12)],
    );
    let diff = diff_snapshots(&store, PROJECT, &first.snapshot_id, &second.snapshot_id).unwrap();
    assert_eq!(diff["state"], "changed");
    assert!(diff["files"]["changed"]["items"]
        .as_array()
        .unwrap()
        .iter()
        .any(|file| file["path"] == "src/a.py"));
    assert!(diff["files"]["added"]["items"]
        .as_array()
        .unwrap()
        .iter()
        .any(|file| file["path"] == "src/new.py"));
    assert!(diff["files"]["deleted"]["items"]
        .as_array()
        .unwrap()
        .iter()
        .any(|file| file["path"] == "src/c.py"));
    assert!(diff["entities"]["changed"]["items"]
        .as_array()
        .unwrap()
        .iter()
        .any(|entity| entity["qualified_name"].as_str().unwrap().ends_with("::a1")));
    assert!(diff["entities"]["added"]["items"]
        .as_array()
        .unwrap()
        .iter()
        .any(|entity| entity["qualified_name"].as_str().unwrap().ends_with("::new_one")));
    assert!(diff["entities"]["deleted"]["items"]
        .as_array()
        .unwrap()
        .iter()
        .any(|entity| entity["qualified_name"].as_str().unwrap().ends_with("::c")));
}

#[test]
fn non_git_workspace_marks_git_provenance_unavailable() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().join("repo");
    fs::create_dir_all(root.join("src")).unwrap();
    fs::write(root.join("src/a.py"), "def a():\n    return 1\n").unwrap();
    let mut store = Store::open(dir.path().join("state")).unwrap();
    store.ensure_project(PROJECT, "expedition").unwrap();
    let captured = capture_snapshot(
        &mut store,
        PROJECT,
        &root,
        &SnapshotInputs {
            build_config: json!({"variant": "non-git"}),
            environment: json!({"os": "test"}),
        },
        &SnapshotOptions {
            include_untracked: true,
            ..SnapshotOptions::default()
        },
    )
    .unwrap();
    fs::write(root.join("src/a.py"), "def a():\n    return 2\n").unwrap();

    let review = review_workspace_diff(&store, PROJECT, &captured.snapshot_id, &root).unwrap();
    let candidate = &review["provenance"]["candidate"];
    assert_eq!(candidate["git_state"], "unavailable", "{review}");
    assert!(candidate["dirty"].is_null(), "{review}");
    assert!(candidate["git_commit"].is_null(), "{review}");
    assert_eq!(candidate["head_tree_basis"], "unavailable_not_git");
    assert!(review["coverage"]["limitations"]
        .as_array()
        .unwrap()
        .iter()
        .any(|item| item
            .as_str()
            .unwrap_or("")
            .contains("not a git repository")));
}

#[test]
fn subdirectory_workspace_reports_a_subtree_oid() {
    let dir = tempfile::tempdir().unwrap();
    let repo = dir.path().join("repo");
    let root = repo.join("sub");
    fs::create_dir_all(root.join("src")).unwrap();
    fs::write(root.join("src/a.py"), "def a():\n    return 1\n").unwrap();
    git(&repo, &["init", "-q"]);
    git(&repo, &["add", "."]);
    git(
        &repo,
        &[
            "-c",
            "user.name=subtree",
            "-c",
            "user.email=subtree@example.com",
            "commit",
            "-q",
            "-m",
            "base",
        ],
    );
    let repo_tree = git(&repo, &["rev-parse", "HEAD^{tree}"]);
    let mut store = Store::open(dir.path().join("state")).unwrap();
    store.ensure_project(PROJECT, "expedition").unwrap();
    let captured = capture_snapshot(
        &mut store,
        PROJECT,
        &root,
        &SnapshotInputs {
            build_config: json!({"variant": "subtree"}),
            environment: json!({"os": "test"}),
        },
        &SnapshotOptions {
            include_untracked: true,
            ..SnapshotOptions::default()
        },
    )
    .unwrap();
    fs::write(root.join("src/a.py"), "def a():\n    return 2\n").unwrap();

    let review = review_workspace_diff(&store, PROJECT, &captured.snapshot_id, &root).unwrap();
    let candidate = &review["provenance"]["candidate"];
    assert_eq!(candidate["head_tree_scope"], "workspace_subtree", "{review}");
    assert_eq!(candidate["workspace_subdir"], "sub/", "{review}");
    assert_ne!(
        candidate["head_tree_oid"].as_str().unwrap_or(""),
        repo_tree,
        "a subdirectory workspace must not report the repository root tree"
    );
    assert_eq!(
        review["provenance"]["base"]["head_tree_scope"], "workspace_subtree",
        "{review}"
    );
}

#[test]
fn duplicate_content_renames_are_ambiguous_not_arbitrary_moves() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().join("repo");
    fs::create_dir_all(root.join("src")).unwrap();
    fs::write(root.join("src/a.py"), "def same():\n    return 1\n").unwrap();
    fs::write(root.join("src/b.py"), "def same():\n    return 1\n").unwrap();
    let mut store = Store::open(dir.path().join("state")).unwrap();
    store.ensure_project(PROJECT, "expedition").unwrap();
    let captured = capture_snapshot(
        &mut store,
        PROJECT,
        &root,
        &SnapshotInputs {
            build_config: json!({"variant": "ambiguous"}),
            environment: json!({"os": "test"}),
        },
        &SnapshotOptions {
            include_untracked: true,
            ..SnapshotOptions::default()
        },
    )
    .unwrap();
    fs::rename(root.join("src/a.py"), root.join("src/c.py")).unwrap();
    fs::rename(root.join("src/b.py"), root.join("src/d.py")).unwrap();

    let drift = analyze_drift(&store, PROJECT, &captured.snapshot_id, &root, None).unwrap();
    assert!(
        drift["moved"].as_array().unwrap().is_empty(),
        "duplicate content must not be paired arbitrarily: {drift}"
    );
    for key in ["deleted", "added"] {
        let files = drift[key].as_array().unwrap();
        assert_eq!(files.len(), 2, "{key}: {drift}");
        assert!(
            files.iter().all(|file| file["ambiguous_move"] == true),
            "{key} entries must be marked ambiguous: {drift}"
        );
    }
    let review = review_workspace_diff(&store, PROJECT, &captured.snapshot_id, &root).unwrap();
    assert_eq!(review["summary"]["files"]["moved"], 0, "{review}");
    assert_eq!(review["summary"]["files"]["deleted"], 2, "{review}");
    assert_eq!(review["summary"]["files"]["added"], 2, "{review}");
}

#[test]
fn identical_snapshot_diff_marks_structural_coverage_not_run() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().join("repo");
    fs::create_dir_all(root.join("src")).unwrap();
    fs::write(root.join("src/a.py"), "def a():\n    return 1\n").unwrap();
    let mut store = Store::open(dir.path().join("state")).unwrap();
    store.ensure_project(PROJECT, "expedition").unwrap();
    let captured = capture_snapshot(
        &mut store,
        PROJECT,
        &root,
        &SnapshotInputs {
            build_config: json!({"variant": "identical"}),
            environment: json!({"os": "test"}),
        },
        &SnapshotOptions {
            include_untracked: true,
            ..SnapshotOptions::default()
        },
    )
    .unwrap();
    let diff = diff_snapshots(
        &store,
        PROJECT,
        &captured.snapshot_id,
        &captured.snapshot_id,
    )
    .unwrap();
    assert_eq!(diff["schema_version"], "1.0", "{diff}");
    assert_eq!(diff["state"], "unchanged", "{diff}");
    assert_eq!(diff["structural"]["paths"]["coverage"], "not_run", "{diff}");
    assert!(!diff["structural"]["paths"]["limitations"]
        .as_array()
        .unwrap()
        .is_empty());
    assert_eq!(
        diff["provenance"]["base"]["head_tree_basis"], "unavailable_no_workspace",
        "{diff}"
    );
}

fn git(root: &Path, args: &[&str]) -> String {
    let output = std::process::Command::new("git")
        .arg("-C")
        .arg(root)
        .args(args)
        .output()
        .expect("git available");
    assert!(
        output.status.success(),
        "git {:?} failed: {}",
        args,
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8_lossy(&output.stdout).trim().to_string()
}

#[test]
fn review_and_diff_expose_git_provenance_and_tree_equivalence() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().join("repo");
    fs::create_dir_all(root.join("src")).unwrap();
    fs::write(root.join("src/a.py"), "def a():\n    return 1\n").unwrap();
    git(&root, &["init", "-q"]);
    git(&root, &["add", "."]);
    git(
        &root,
        &[
            "-c",
            "user.name=provenance",
            "-c",
            "user.email=provenance@example.com",
            "commit",
            "-q",
            "-m",
            "base",
        ],
    );
    let mut store = Store::open(dir.path().join("state")).unwrap();
    store.ensure_project(PROJECT, "expedition").unwrap();
    let inputs = SnapshotInputs {
        build_config: json!({"variant": "provenance"}),
        environment: json!({"os": "test"}),
    };
    let first = capture_snapshot(
        &mut store,
        PROJECT,
        &root,
        &inputs,
        &SnapshotOptions {
            include_untracked: true,
            ..SnapshotOptions::default()
        },
    )
    .unwrap();
    let first_commit = first.snapshot["git_commit"]
        .as_str()
        .expect("git commit captured")
        .to_string();

    git(
        &root,
        &[
            "-c",
            "user.name=provenance",
            "-c",
            "user.email=provenance@example.com",
            "commit",
            "--allow-empty",
            "-q",
            "-m",
            "same tree",
        ],
    );
    let second = capture_snapshot(
        &mut store,
        PROJECT,
        &root,
        &inputs,
        &SnapshotOptions {
            include_untracked: true,
            ..SnapshotOptions::default()
        },
    )
    .unwrap();
    let second_commit = second.snapshot["git_commit"]
        .as_str()
        .expect("second git commit captured")
        .to_string();
    assert_ne!(first_commit, second_commit);

    let diff = diff_snapshots(&store, PROJECT, &first.snapshot_id, &second.snapshot_id).unwrap();
    assert_eq!(diff["state"], "unchanged", "{diff}");
    assert_eq!(diff["provenance"]["tree_equivalent"], true, "{diff}");
    assert_eq!(
        diff["provenance"]["base"]["git_commit"], first_commit,
        "{diff}"
    );
    assert_eq!(
        diff["provenance"]["candidate"]["git_commit"], second_commit,
        "{diff}"
    );
    assert_eq!(
        diff["provenance"]["base"]["content_tree_digest"],
        diff["provenance"]["candidate"]["content_tree_digest"],
        "{diff}"
    );
    assert_eq!(diff["identity_changes"].as_array().unwrap().len(), 0, "{diff}");

    fs::write(root.join("src/a.py"), "def a():\n    return 2\n").unwrap();
    let review = review_workspace_diff(&store, PROJECT, &second.snapshot_id, &root).unwrap();
    assert_eq!(review["provenance"]["base"]["git_commit"], second_commit);
    assert!(
        review["provenance"]["base"]["head_tree_oid"]
            .as_str()
            .is_some_and(|oid| oid.len() == 40 || oid.len() == 64),
        "{review}"
    );
    assert_eq!(review["provenance"]["candidate"]["dirty"], true, "{review}");
    assert_eq!(review["provenance"]["tree_equivalent"], false, "{review}");
    assert_ne!(
        review["provenance"]["base"]["content_tree_digest"],
        review["provenance"]["candidate"]["content_tree_digest"],
        "{review}"
    );
    assert_eq!(
        review["files"][0]["base_side"], "base",
        "file entries must tag the base side: {review}"
    );
    assert_eq!(
        review["files"][0]["current_side"], "candidate",
        "file entries must tag the candidate side: {review}"
    );
}

#[test]
fn snapshot_diff_detects_moves_and_convergent_paths() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().join("repo");
    fs::create_dir_all(root.join("src")).unwrap();
    fs::write(
        root.join("src/flow.py"),
        "def dispatch():\n    return settle_old()\n\ndef settle_old():\n    return ledger()\n\ndef ledger():\n    return 1\n",
    )
    .unwrap();
    let mut store = Store::open(dir.path().join("state")).unwrap();
    store.ensure_project(PROJECT, "expedition").unwrap();
    let inputs = SnapshotInputs {
        build_config: json!({"variant": "structural"}),
        environment: json!({"os": "test"}),
    };
    let options = SnapshotOptions {
        include_untracked: true,
        ..SnapshotOptions::default()
    };
    let first = capture_snapshot(&mut store, PROJECT, &root, &inputs, &options).unwrap();
    let config = first.snapshot["config_digest"]
        .as_str()
        .unwrap()
        .to_string();
    let base_nodes = [
        node(&root, 1, "src/flow.py", "dispatch", 1, 2),
        node(&root, 2, "src/flow.py", "settle_old", 4, 5),
        node(&root, 3, "src/flow.py", "ledger", 7, 8),
    ];
    import(
        &mut store,
        &first.snapshot_id,
        &config,
        "run.struct.base",
        &first.files,
        &root,
        &base_nodes,
        &[edge(1, 1, 2), edge(2, 2, 3)],
    );

    fs::write(
        root.join("src/flow.py"),
        "def dispatch():\n    return settle_a() if flag else settle_b()\n\ndef settle_a():\n    return ledger()\n\ndef settle_b():\n    return ledger()\n\ndef ledger():\n    return 1\n",
    )
    .unwrap();
    let second = capture_snapshot(&mut store, PROJECT, &root, &inputs, &options).unwrap();
    let new_nodes = [
        node(&root, 11, "src/flow.py", "dispatch", 1, 2),
        node(&root, 12, "src/flow.py", "settle_a", 4, 5),
        node(&root, 13, "src/flow.py", "settle_b", 7, 8),
        node(&root, 14, "src/flow.py", "ledger", 10, 11),
    ];
    import(
        &mut store,
        &second.snapshot_id,
        &config,
        "run.struct.new",
        &second.files,
        &root,
        &new_nodes,
        &[
            edge(11, 11, 12),
            edge(12, 12, 14),
            edge(13, 11, 13),
            edge(14, 13, 14),
            edge_typed(15, 12, 14, "Writes"),
            edge_typed(16, 13, 14, "Writes"),
        ],
    );

    let diff = diff_snapshots(&store, PROJECT, &first.snapshot_id, &second.snapshot_id).unwrap();
    assert!(
        diff["structural"]["dependencies"]["added"]["count"]
            .as_u64()
            .unwrap_or(0)
            >= 2,
        "{diff}"
    );
    assert!(
        diff["structural"]["dependencies"]["removed"]["count"]
            .as_u64()
            .unwrap_or(0)
            >= 2,
        "{diff}"
    );
    let convergence = diff["structural"]["paths"]["convergence"]
        .as_array()
        .unwrap();
    let ledger = convergence
        .iter()
        .find(|item| {
            item["target"]
                .as_str()
                .unwrap_or("")
                .ends_with("::ledger")
        })
        .expect("convergent ledger path");
    assert!(
        ledger["paths"].as_array().unwrap().len() >= 2,
        "{diff}"
    );
    assert_eq!(ledger["hypothesis"], true, "{diff}");
    assert!(
        diff["structural"]["ownership"]["added"]["count"]
            .as_u64()
            .unwrap_or(0)
            >= 2,
        "{diff}"
    );
    assert_eq!(
        diff["structural"]["ownership"]["coverage"], "partial",
        "{diff}"
    );
    assert_eq!(
        diff["structural"]["entry_points"]["basis"], "unavailable",
        "{diff}"
    );

    let moved_root = dir.path().join("moved");
    fs::create_dir_all(moved_root.join("src")).unwrap();
    fs::write(moved_root.join("src/a.py"), "def a():\n    return 1\n").unwrap();
    let moved_first = capture_snapshot(&mut store, PROJECT, &moved_root, &inputs, &options).unwrap();
    fs::rename(
        moved_root.join("src/a.py"),
        moved_root.join("src/b.py"),
    )
    .unwrap();
    let moved_second = capture_snapshot(&mut store, PROJECT, &moved_root, &inputs, &options).unwrap();
    let moved_diff = diff_snapshots(
        &store,
        PROJECT,
        &moved_first.snapshot_id,
        &moved_second.snapshot_id,
    )
    .unwrap();
    assert_eq!(
        moved_diff["files"]["moved"]["count"].as_u64().unwrap_or(0),
        1,
        "{moved_diff}"
    );
    assert_eq!(
        moved_diff["files"]["added"]["count"].as_u64().unwrap_or(0),
        0,
        "{moved_diff}"
    );
    assert_eq!(
        moved_diff["files"]["deleted"]["count"].as_u64().unwrap_or(0),
        0,
        "{moved_diff}"
    );
    assert_eq!(moved_diff["state"], "changed", "{moved_diff}");
    assert!(moved_diff["structural"].is_object(), "{moved_diff}");
    assert!(moved_diff["coverage"].is_object(), "{moved_diff}");

    let same = diff_snapshots(
        &store,
        PROJECT,
        &moved_second.snapshot_id,
        &moved_second.snapshot_id,
    )
    .unwrap();
    assert_eq!(same["files"]["changed"]["count"], 0, "{same}");
    assert!(same["files"]["changed"].is_object(), "{same}");
    assert!(same["structural"].is_object(), "{same}");
    assert!(same["coverage"].is_object(), "{same}");
    assert!(same["summary"].is_object(), "{same}");
}

#[test]
fn persisted_review_report_pins_its_base_snapshot() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().join("repo");
    fs::create_dir_all(root.join("src")).unwrap();
    fs::write(root.join("src/a.py"), "def a():\n    return 1\n").unwrap();
    let mut store = Store::open(dir.path().join("state")).unwrap();
    store.ensure_project(PROJECT, "expedition").unwrap();
    let inputs = SnapshotInputs {
        build_config: json!({"variant": "pin"}),
        environment: json!({"os": "test"}),
    };
    let options = SnapshotOptions {
        include_untracked: true,
        ..SnapshotOptions::default()
    };
    let first = capture_snapshot(&mut store, PROJECT, &root, &inputs, &options).unwrap();
    fs::write(root.join("src/a.py"), "def a():\n    return 2\n").unwrap();
    let second = capture_snapshot(&mut store, PROJECT, &root, &inputs, &options).unwrap();
    assert_ne!(first.snapshot_id, second.snapshot_id);

    let mut review = review_workspace_diff(&store, PROJECT, &first.snapshot_id, &root).unwrap();
    let frozen = freeze_review_candidate(&mut store, &root, &mut review).unwrap();
    let frozen_files = frozen["files"].as_array().unwrap();
    assert_eq!(frozen_files.len(), 1, "{frozen}");
    let frozen_digest = frozen_files[0]["blob_digest"].as_str().unwrap();
    assert_eq!(
        store.read_blob_bytes(frozen_digest).unwrap().unwrap(),
        fs::read(root.join("src/a.py")).unwrap()
    );
    assert_eq!(review["provenance"]["candidate"]["durable"], true);
    let persisted = persist_review_report(
        &mut store,
        PROJECT,
        &first.snapshot_id,
        None,
        "workspace",
        &review,
    )
    .unwrap();
    let report_id = persisted["stored"]["report_id"]
        .as_str()
        .expect("report id")
        .to_string();
    let repeated = persist_review_report(
        &mut store,
        PROJECT,
        &first.snapshot_id,
        None,
        "workspace",
        &review,
    )
    .unwrap();
    assert_eq!(
        repeated["stored"]["report_id"], report_id,
        "identical review output must keep the report id stable"
    );
    assert_eq!(store.list_review_reports(PROJECT).unwrap().len(), 1);
    let stored = store
        .get_review_report(PROJECT, &report_id)
        .unwrap()
        .expect("stored report");
    assert_eq!(stored["output"]["base_snapshot_id"], first.snapshot_id);
    assert_eq!(store.list_review_reports(PROJECT).unwrap().len(), 1);

    fs::write(root.join("src/a.py"), "def a():\n    return 3\n").unwrap();
    let _third = capture_snapshot(&mut store, PROJECT, &root, &inputs, &options).unwrap();

    configure_retention(
        &mut store,
        &json!({"project_id": PROJECT, "keep_snapshots": 1, "auto_prune": false}),
    )
    .unwrap();
    let pruned = prune_snapshots(
        &mut store,
        &json!({"project_id": PROJECT, "keep_snapshots": 1, "dry_run": false}),
    )
    .unwrap();
    assert!(
        store
            .snapshot_payload(PROJECT, &first.snapshot_id)
            .unwrap()
            .is_some(),
        "pinned base snapshot must survive: {pruned}"
    );
    assert!(
        pruned["retained"]
            .as_array()
            .into_iter()
            .flatten()
            .any(|item| item["snapshot_id"] == first.snapshot_id),
        "{pruned}"
    );
}

#[test]
fn deleting_a_review_report_releases_pins_and_frozen_objects() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().join("repo");
    fs::create_dir_all(root.join("src")).unwrap();
    fs::write(root.join("src/a.py"), "def a():\n    return 1\n").unwrap();
    let mut store = Store::open(dir.path().join("state")).unwrap();
    store.ensure_project(PROJECT, "expedition").unwrap();
    let inputs = SnapshotInputs {
        build_config: json!({"variant": "report-lifecycle"}),
        environment: json!({"os": "test"}),
    };
    let options = SnapshotOptions {
        include_untracked: true,
        ..SnapshotOptions::default()
    };
    let first = capture_snapshot(&mut store, PROJECT, &root, &inputs, &options).unwrap();
    fs::write(root.join("src/a.py"), "def a():\n    return 2\n").unwrap();
    let mut review = review_workspace_diff(&store, PROJECT, &first.snapshot_id, &root).unwrap();
    let frozen = freeze_review_candidate(&mut store, &root, &mut review).unwrap();
    assert_eq!(frozen["durable"], true, "{frozen}");
    let frozen_digest = frozen["files"][0]["blob_digest"]
        .as_str()
        .expect("frozen digest")
        .to_string();
    let persisted = persist_review_report(
        &mut store,
        PROJECT,
        &first.snapshot_id,
        None,
        "workspace",
        &review,
    )
    .unwrap();
    let report_id = persisted["stored"]["report_id"]
        .as_str()
        .expect("report id")
        .to_string();
    let listed = store.list_review_reports(PROJECT).unwrap();
    assert_eq!(listed.len(), 1, "{listed:?}");
    assert_eq!(listed[0]["retention"]["state"], "pinned", "{listed:?}");
    assert_eq!(
        listed[0]["retention"]["pin_ids"].as_array().unwrap().len(),
        1,
        "{listed:?}"
    );
    assert!(listed[0]["size_bytes"].as_i64().unwrap() > 0, "{listed:?}");

    let deleted = store.delete_review_report(PROJECT, &report_id).unwrap();
    assert_eq!(deleted["removed"], true, "{deleted}");
    assert!(deleted["pins_removed"].as_u64().unwrap() >= 2, "{deleted}");
    assert!(store
        .get_review_report(PROJECT, &report_id)
        .unwrap()
        .is_none());
    assert!(store.list_review_reports(PROJECT).unwrap().is_empty());

    let collected = gc(&mut store, false).unwrap();
    assert!(
        collected["deleted_count"].as_u64().unwrap() >= 1,
        "{collected}"
    );
    assert_eq!(
        store.read_blob_bytes(&frozen_digest).unwrap(),
        None,
        "frozen bytes must be reclaimable after the report that cited them is deleted"
    );
}

#[test]
fn workspace_review_treats_a_rename_as_moved_not_deleted_plus_added() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().join("repo");
    fs::create_dir_all(root.join("src")).unwrap();
    fs::write(root.join("src/a.py"), "def alpha():\n    return 1\n").unwrap();
    let mut store = Store::open(dir.path().join("state")).unwrap();
    store.ensure_project(PROJECT, "expedition").unwrap();
    let captured = capture_snapshot(
        &mut store,
        PROJECT,
        &root,
        &SnapshotInputs {
            build_config: json!({"variant": "move"}),
            environment: json!({"os": "test"}),
        },
        &SnapshotOptions {
            include_untracked: true,
            ..SnapshotOptions::default()
        },
    )
    .unwrap();
    let config = captured.snapshot["config_digest"]
        .as_str()
        .unwrap()
        .to_string();
    import(
        &mut store,
        &captured.snapshot_id,
        &config,
        "run.move",
        &captured.files,
        &root,
        &[node(&root, 1, "src/a.py", "alpha", 1, 2)],
        &[],
    );
    fs::rename(root.join("src/a.py"), root.join("src/b.py")).unwrap();

    let review = review_workspace_diff(&store, PROJECT, &captured.snapshot_id, &root).unwrap();
    assert_eq!(review["summary"]["files"]["moved"], 1, "{review}");
    assert_eq!(review["summary"]["files"]["deleted"], 0, "{review}");
    assert_eq!(review["summary"]["files"]["added"], 0, "{review}");
    let moved = &review["files"][0];
    assert_eq!(moved["state"], "moved", "{review}");
    assert_eq!(moved["from"], "src/a.py");
    assert_eq!(moved["path"], "src/b.py");
    assert_eq!(moved["entities"][0]["state"], "context_only", "{review}");

    let brief = build_brief(
        &store,
        &BriefRequest {
            project_id: PROJECT,
            snapshot_id: Some(&captured.snapshot_id),
            workspace: Some(&root),
            recipe: BriefRecipe::Change,
        },
    )
    .unwrap();
    assert_eq!(brief["moved"].as_array().unwrap().len(), 1, "{brief}");
    assert!(brief["findings"].as_array().unwrap().is_empty(), "{brief}");
    assert!(
        brief["context"]
            .as_array()
            .unwrap()
            .iter()
            .any(|entry| entry["entity"]["qualified_name"]
                .as_str()
                .unwrap_or("")
                .ends_with("::alpha")),
        "{brief}"
    );
}

#[test]
fn freeze_refuses_bytes_that_changed_after_the_review() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().join("repo");
    fs::create_dir_all(root.join("src")).unwrap();
    fs::write(root.join("src/a.py"), "def a():\n    return 1\n").unwrap();
    let mut store = Store::open(dir.path().join("state")).unwrap();
    store.ensure_project(PROJECT, "expedition").unwrap();
    let captured = capture_snapshot(
        &mut store,
        PROJECT,
        &root,
        &SnapshotInputs {
            build_config: json!({"variant": "freeze-mismatch"}),
            environment: json!({"os": "test"}),
        },
        &SnapshotOptions {
            include_untracked: true,
            ..SnapshotOptions::default()
        },
    )
    .unwrap();
    fs::write(root.join("src/a.py"), "def a():\n    return 2\n").unwrap();
    let mut review = review_workspace_diff(&store, PROJECT, &captured.snapshot_id, &root).unwrap();
    fs::write(root.join("src/a.py"), "def a():\n    return 3\n").unwrap();
    let frozen = freeze_review_candidate(&mut store, &root, &mut review).unwrap();
    assert_eq!(frozen["durable"], false, "{frozen}");
    assert!(frozen["files"].as_array().unwrap().is_empty(), "{frozen}");
    assert!(!frozen["limitations"].as_array().unwrap().is_empty(), "{frozen}");
    assert_eq!(review["provenance"]["candidate"]["durable"], false);
}
