// Copyright 2026 ARCHMELD contributors
// SPDX-License-Identifier: Apache-2.0

//! Task-level diff review: the workspace is compared with a frozen snapshot,
//! changed entities are located by line overlap, graph impact stays static, and
//! no build/test is executed. Snapshot-to-snapshot diff is exact.

use archmeld_core::bridge::{import_codegraph_export, BridgeConfig};
use archmeld_core::review::{diff_snapshots, review_workspace_diff};
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
    json!({
        "type":"edge","id":id,"source":source,"target":target,
        "edge_type":"Calls","properties":{"line":2}
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
