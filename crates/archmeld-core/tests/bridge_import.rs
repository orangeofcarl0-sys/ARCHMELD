// Copyright 2026 ARCHMELD contributors
// SPDX-License-Identifier: Apache-2.0

//! AT013/AT014: the CodeGraph NDJSON bridge keeps backend IDs local, keeps
//! unresolved edges visible, and never promotes candidates to resolved calls.

use archmeld_core::bridge::{import_codegraph_export, BridgeConfig};
use archmeld_core::snapshot::{capture_snapshot, SnapshotInputs, SnapshotOptions};
use archmeld_core::store::{FileEntry, Store};
use serde_json::{json, Value};
use std::fs;
use std::path::Path;

const PROJECT: &str = "project.bridge";
const RUN: &str = "run.codegraph.001";

fn fixture() -> (
    tempfile::TempDir,
    Store,
    String,
    String,
    std::path::PathBuf,
    Vec<FileEntry>,
) {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().join("repo");
    fs::create_dir_all(root.join("src")).unwrap();
    fs::write(root.join("src/app.py"), "def main():\n    return helper()\n").unwrap();
    fs::write(root.join("src/util.py"), "def helper():\n    return 1\n").unwrap();
    let mut store = Store::open(dir.path().join("state")).unwrap();
    store.ensure_project(PROJECT, "expedition").unwrap();
    let options = SnapshotOptions {
        include_untracked: true,
        ..SnapshotOptions::default()
    };
    let captured = capture_snapshot(
        &mut store,
        PROJECT,
        &root,
        &SnapshotInputs {
            build_config: json!({"variant": "test"}),
            environment: json!({"os": "test"}),
        },
        &options,
    )
    .unwrap();
    let config_digest = captured.snapshot["config_digest"]
        .as_str()
        .unwrap()
        .to_string();
    (dir, store, captured.snapshot_id, config_digest, root, captured.files)
}

fn export(root: &Path, nodes: Vec<Value>, edges: Vec<Value>) -> String {
    let mut lines = vec![json!({
        "type": "manifest",
        "format": "archmeld-export-v1",
        "source": "codegraph",
        "version": "0.2.0",
        "node_count": nodes.len(),
        "edge_count": edges.len(),
        "iteration": "real-iterators",
    })
    .to_string()];
    for node in nodes {
        lines.push(node.to_string());
    }
    for edge in edges {
        lines.push(edge.to_string());
    }
    lines.join("\n") + "\n"
}

fn node(root: &Path, id: u64, node_type: &str, path: &str, name: &str, lines: Option<(u64, u64)>) -> Value {
    let absolute = root.join(path);
    json!({
        "type": "node",
        "id": id,
        "node_type": node_type,
        "properties": {
            "path": absolute.to_string_lossy(),
            "name": name,
            "line_start": lines.map(|(start, _)| start),
            "line_end": lines.map(|(_, end)| end),
        }
    })
}

#[test]
fn import_maps_nodes_and_edges_without_using_backend_ids_as_identity() {
    let (_dir, mut store, snapshot_id, config_digest, root, files) = fixture();
    let text = export(
        &root,
        vec![
            node(&root, 5, "CodeFile", "src/app.py", "app.py", None),
            node(&root, 9, "Function", "src/app.py", "main", Some((1, 2))),
            node(&root, 12, "Function", "src/util.py", "helper", Some((1, 2))),
        ],
        vec![json!({
            "type": "edge",
            "id": 3,
            "source": 9,
            "target": 12,
            "edge_type": "Calls",
            "properties": {"line": 2}
        })],
    );
    let config = BridgeConfig {
        project_id: PROJECT,
        snapshot_id: &snapshot_id,
        config_digest: &config_digest,
        run_id: RUN,
        workspace_root: Some(&root),
        max_entities_per_batch: 500,
        max_relations_per_batch: 2000,
    };
    let report = import_codegraph_export(&mut store, &config, &files, &text).expect("import");
    assert_eq!(report.entities, 3);
    assert_eq!(report.relations, 1);
    assert_eq!(report.diagnostics, 0);
    assert_eq!(report.coverage["state"], "complete");

    let counts = store.published_counts(PROJECT, &snapshot_id).unwrap();
    assert_eq!(counts.entities, 3);
    assert_eq!(counts.relations, 1);
    assert_eq!(counts.evidence, 1);
    assert_eq!(counts.backend_mappings, 3);

    let mapped = store
        .backend_mapping(PROJECT, &snapshot_id, "codegraph", RUN, "9")
        .unwrap()
        .expect("backend mapping");
    assert!(mapped.starts_with("syn."));
    assert!(store.integrity_check().unwrap().is_empty());
}

#[test]
fn unresolved_endpoint_and_heuristic_runtime_edge_stay_visible_but_unimported() {
    let (_dir, mut store, snapshot_id, config_digest, root, files) = fixture();
    let text = export(
        &root,
        vec![node(&root, 1, "Function", "src/app.py", "main", Some((1, 2)))],
        vec![
            json!({"type":"edge","id":2,"source":1,"target":99,"edge_type":"Calls","properties":{}}),
            json!({"type":"edge","id":3,"source":1,"target":1,"edge_type":"RuntimeCalls","properties":{}}),
        ],
    );
    let config = BridgeConfig {
        project_id: PROJECT,
        snapshot_id: &snapshot_id,
        config_digest: &config_digest,
        run_id: RUN,
        workspace_root: Some(&root),
        max_entities_per_batch: 500,
        max_relations_per_batch: 2000,
    };
    let report = import_codegraph_export(&mut store, &config, &files, &text).expect("import");
    assert_eq!(report.entities, 1);
    assert_eq!(report.relations, 0);
    assert_eq!(report.unresolved, 2);
    assert_eq!(report.coverage["state"], "partial");
    let counts = store.published_counts(PROJECT, &snapshot_id).unwrap();
    assert_eq!(counts.relations, 0);
}

#[test]
fn repeated_import_is_idempotent_and_conflicting_payload_is_rejected() {
    let (_dir, mut store, snapshot_id, config_digest, root, files) = fixture();
    let text = export(
        &root,
        vec![node(&root, 1, "Function", "src/app.py", "main", Some((1, 2)))],
        vec![],
    );
    let config = BridgeConfig {
        project_id: PROJECT,
        snapshot_id: &snapshot_id,
        config_digest: &config_digest,
        run_id: RUN,
        workspace_root: Some(&root),
        max_entities_per_batch: 500,
        max_relations_per_batch: 2000,
    };
    import_codegraph_export(&mut store, &config, &files, &text).expect("first");
    let counts_after_first = store.published_counts(PROJECT, &snapshot_id).unwrap();
    // Same run, same bytes: idempotent replay.
    import_codegraph_export(&mut store, &config, &files, &text).expect("idempotent replay");
    assert_eq!(
        counts_after_first,
        store.published_counts(PROJECT, &snapshot_id).unwrap()
    );
    // Same sequence with a different payload must conflict.
    let err = store
        .import_batch(
            &json!({
                "schema_version": "1.0",
                "project_id": PROJECT,
                "snapshot_id": snapshot_id,
                "config_digest": config_digest,
                "run_id": RUN,
                "adapter_id": "codegraph",
                "adapter_version": "0.2.0",
                "sequence": "0",
                "final": true,
                "coverage": {
                    "state": "complete",
                    "scope": "manual",
                    "expected_count": 1,
                    "observed_count": 1,
                    "omitted_count": 0,
                    "unresolved_count": 0,
                    "limitations": []
                },
                "entities": [],
                "evidence": [],
                "relations": [],
                "diagnostics": []
            }),
        )
        .expect_err("different payload for the same sequence must conflict");
    assert!(err.to_string().contains("different payload digest"), "{err}");
}
