// Copyright 2026 ARCHMELD contributors
// SPDX-License-Identifier: Apache-2.0

//! AT015 regression: paging a traversal must not lose relations that cross a
//! page boundary. Each page reports incident boundary edges with an explicit
//! endpoint stub, so merging pages reconstructs the graph slice.

use archmeld_core::bridge::{import_codegraph_export, BridgeConfig};
use archmeld_core::query::run_query;
use archmeld_core::snapshot::{capture_snapshot, SnapshotInputs, SnapshotOptions};
use archmeld_core::store::Store;
use serde_json::{json, Value};
use std::collections::BTreeSet;
use std::fs;

const PROJECT: &str = "project.query.pagination";

fn node(root: &std::path::Path, id: u64, path: &str, name: &str) -> Value {
    json!({
        "type": "node", "id": id, "node_type": "Function",
        "properties": {
            "path": root.join(path).to_string_lossy(),
            "name": name, "line_start": 1, "line_end": 1,
        }
    })
}

fn edge(id: u64, source: u64, target: u64) -> Value {
    json!({
        "type": "edge", "id": id, "source": source, "target": target,
        "edge_type": "Calls", "properties": {"line": 1}
    })
}

fn query(store: &Store, snapshot_id: &str, entity_id: &str, cursor: Option<&str>) -> Value {
    run_query(
        store,
        &json!({
            "project_id": PROJECT,
            "snapshot_id": snapshot_id,
            "kind": "neighbors",
            "query": "",
            "entity_ids": [entity_id],
            "relation_kinds": ["static_call_candidate"],
            "max_nodes": 2,
            "max_depth": 3,
            "cursor": cursor,
        }),
    )
    .unwrap()
}

#[test]
fn page_boundary_edges_survive_pagination() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().join("repo");
    fs::create_dir_all(root.join("src")).unwrap();
    fs::write(root.join("src/a.py"), "def a():\n    return b()\n").unwrap();
    fs::write(root.join("src/b.py"), "def b():\n    return c()\n").unwrap();
    fs::write(root.join("src/c.py"), "def c():\n    return 1\n").unwrap();
    let mut store = Store::open(dir.path().join("state")).unwrap();
    store.ensure_project(PROJECT, "expedition").unwrap();
    let captured = capture_snapshot(
        &mut store,
        PROJECT,
        &root,
        &SnapshotInputs {
            build_config: json!({"variant": "pagination"}),
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
        node(&root, 1, "src/a.py", "a"),
        node(&root, 2, "src/b.py", "b"),
        node(&root, 3, "src/c.py", "c"),
    ];
    let mut lines = vec![json!({
        "type":"manifest","format":"archmeld-export-v1","source":"codegraph",
        "version":"0.2.0","node_count":3,"edge_count":2,"iteration":"real-iterators"
    })
    .to_string()];
    for node in &nodes {
        lines.push(node.to_string());
    }
    lines.push(edge(1, 1, 2).to_string());
    lines.push(edge(2, 2, 3).to_string());
    import_codegraph_export(
        &mut store,
        &BridgeConfig {
            project_id: PROJECT,
            snapshot_id: &captured.snapshot_id,
            config_digest: &config_digest,
            run_id: "run.query.pagination",
            workspace_root: Some(&root),
            max_entities_per_batch: 500,
            max_relations_per_batch: 2000,
        },
        &captured.files,
        &(lines.join("\n") + "\n"),
    )
    .unwrap();
    let a_id = store
        .entity_ids(PROJECT, &captured.snapshot_id)
        .unwrap()
        .into_iter()
        .find(|id| {
            store
                .get_entity(PROJECT, &captured.snapshot_id, id)
                .unwrap()
                .and_then(|entity| entity["qualified_name"].as_str().map(str::to_string))
                .is_some_and(|name| name.ends_with("::a"))
        })
        .unwrap();

    let first = query(&store, &captured.snapshot_id, &a_id, None);
    assert_eq!(first["entities"].as_array().unwrap().len(), 2);
    assert_eq!(first["pagination"]["total"], 3);
    assert_eq!(first["pagination"]["has_more"], true);
    assert_eq!(first["relations"].as_array().unwrap().len(), 1);
    let first_boundary = first["boundary_edges"].as_array().unwrap();
    assert_eq!(first_boundary.len(), 1);
    assert!(first_boundary[0]["missing_entity"]["qualified_name"]
        .as_str()
        .unwrap()
        .ends_with("::c"));
    let boundary_relation = first_boundary[0]["relation"]["id"].clone();
    let cursor = first["next_cursor"].as_str().unwrap();
    let second = query(&store, &captured.snapshot_id, &a_id, Some(cursor));
    assert_eq!(second["entities"].as_array().unwrap().len(), 1);
    assert_eq!(second["relations"].as_array().unwrap().len(), 0);
    let second_boundary = second["boundary_edges"].as_array().unwrap();
    assert_eq!(second_boundary.len(), 1);
    assert_eq!(second_boundary[0]["relation"]["id"], boundary_relation);

    let mut relation_ids = BTreeSet::new();
    for page in [&first, &second] {
        for relation in page["relations"].as_array().unwrap() {
            relation_ids.insert(relation["id"].to_string());
        }
        for boundary in page["boundary_edges"].as_array().unwrap() {
            relation_ids.insert(boundary["relation"]["id"].to_string());
        }
    }
    assert_eq!(
        relation_ids.len(),
        2,
        "merging pages must preserve the internal and cross-page edges: {relation_ids:?}"
    );
}
