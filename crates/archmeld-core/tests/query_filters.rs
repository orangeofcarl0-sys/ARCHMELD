// Copyright 2026 ARCHMELD contributors
// SPDX-License-Identifier: Apache-2.0

//! P04/P11/P12/P16: query filters must actually narrow the graph, including
//! excluding test/vendored/config entities from impact, and must not silently
//! ignore relation filters on non-traversal queries.

use archmeld_core::bridge::{import_codegraph_export, BridgeConfig};
use archmeld_core::query::run_query;
use archmeld_core::snapshot::{capture_snapshot, SnapshotInputs, SnapshotOptions};
use archmeld_core::store::{FileEntry, Store};
use serde_json::{json, Value};
use std::fs;

const PROJECT: &str = "project.query.filters";
const RUN: &str = "run.query.filters";

struct Fixture {
    _dir: tempfile::TempDir,
    store: Store,
    snapshot: String,
    config: String,
    root: std::path::PathBuf,
    files: Vec<FileEntry>,
}

fn fixture() -> Fixture {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().join("repo");
    fs::create_dir_all(root.join("src")).unwrap();
    fs::create_dir_all(root.join("tests")).unwrap();
    fs::create_dir_all(root.join("vendor")).unwrap();
    fs::write(root.join("src/app.py"), "VALUE = 1\ndef main():\n    return helper()\n").unwrap();
    fs::write(
        root.join("tests/test_app.py"),
        "VALUE = 2\ndef test_main():\n    return helper()\n",
    )
    .unwrap();
    fs::write(root.join("Cargo.toml"), "[package]\nname = \"fixture\"\n").unwrap();
    fs::write(root.join("vendor/lib.min.js"), "function cytoscape() {}\n").unwrap();
    let mut store = Store::open(dir.path().join("state")).unwrap();
    store.ensure_project(PROJECT, "expedition").unwrap();
    let captured = capture_snapshot(
        &mut store,
        PROJECT,
        &root,
        &SnapshotInputs {
            build_config: json!({"variant": "query-filters"}),
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
    Fixture {
        _dir: dir,
        store,
        snapshot: captured.snapshot_id,
        config,
        root,
        files: captured.files,
    }
}

fn node(root: &std::path::Path, id: u64, node_type: &str, path: &str, name: &str) -> Value {
    json!({
        "type": "node",
        "id": id,
        "node_type": node_type,
        "properties": {
            "path": root.join(path).to_string_lossy(),
            "name": name,
            "line_start": 1,
            "line_end": 2
        }
    })
}

fn external_module(id: u64, name: &str) -> Value {
    json!({
        "type": "node",
        "id": id,
        "node_type": "Module",
        "properties": {"name": name}
    })
}

fn edge(id: u64, source: u64, target: u64, properties: Value) -> Value {
    json!({
        "type": "edge",
        "id": id,
        "source": source,
        "target": target,
        "edge_type": "Calls",
        "properties": properties
    })
}

impl Fixture {
    fn import(&mut self, nodes: Vec<Value>, edges: Vec<Value>) {
        let manifest = json!({
            "type": "manifest",
            "format": "archmeld-export-v1",
            "schema_version": "1.2",
            "source": "codegraph",
            "version": "0.2.0",
            "node_count": nodes.len(),
            "edge_count": edges.len(),
            "iteration": "real-iterators",
            "index": {"files_total": 4, "files_parsed": 4, "parser_errors": []}
        });
        let mut lines = vec![manifest.to_string()];
        lines.extend(nodes.into_iter().map(|item| item.to_string()));
        lines.extend(edges.into_iter().map(|item| item.to_string()));
        let files: Vec<FileEntry> = self.files.clone();
        import_codegraph_export(
            &mut self.store,
            &BridgeConfig {
                project_id: PROJECT,
                snapshot_id: &self.snapshot,
                config_digest: &self.config,
                run_id: RUN,
                workspace_root: Some(&self.root),
                max_entities_per_batch: 500,
                max_relations_per_batch: 2000,
            },
            &files,
            &(lines.join("\n") + "\n"),
        )
        .unwrap();
    }

    fn query(&self, request: Value) -> archmeld_core::error::Result<Value> {
        run_query(&self.store, &request)
    }
}

fn entity_id(result: &Value, name: &str) -> String {
    result["entities"]
        .as_array()
        .unwrap()
        .iter()
        .find(|entity| entity["qualified_name"].as_str().unwrap_or("").ends_with(name))
        .unwrap_or_else(|| panic!("missing entity {name}: {result}"))["id"]
        .as_str()
        .unwrap()
        .to_string()
}

#[test]
fn producer_resolution_and_call_site_filters_apply_to_traversal_only() {
    let mut fixture = fixture();
    let root = fixture.root.clone();
    fixture.import(
        vec![
            node(&root, 1, "Function", "src/app.py", "main"),
            node(&root, 2, "Function", "src/app.py", "helper"),
            node(&root, 3, "Function", "src/app.py", "direct_helper"),
            node(&root, 4, "Function", "src/app.py", "no_site"),
        ],
        vec![
            edge(
                10,
                1,
                2,
                json!({"call_site_line": 2, "is_direct": false,
                       "resolved_by": "cross_file_resolution"}),
            ),
            edge(
                11,
                1,
                3,
                json!({"call_site_line": 3, "is_direct": true}),
            ),
            edge(
                12,
                1,
                4,
                json!({"is_direct": false}),
            ),
        ],
    );
    let seed = entity_id(
        &fixture
            .query(json!({
                "project_id": PROJECT, "snapshot_id": fixture.snapshot, "kind": "symbols",
                "query": "main", "entity_ids": [], "relation_kinds": [], "max_nodes": 10,
                "max_depth": 1, "cursor": null
            }))
            .unwrap(),
        "::main",
    );
    let request = json!({
        "project_id": PROJECT, "snapshot_id": fixture.snapshot, "kind": "neighbors",
        "query": "", "entity_ids": [seed], "relation_kinds": ["static_call_candidate"],
        "max_nodes": 20, "max_depth": 1, "cursor": null,
        "min_producer_resolution": "cross_file_resolution",
        "require_call_site": true
    });
    let result = fixture.query(request).unwrap();
    let relations = result["relations"].as_array().unwrap();
    assert_eq!(relations.len(), 2, "{result}");
    let resolutions: Vec<&str> = relations
        .iter()
        .map(|relation| relation["producer_resolution"].as_str().unwrap())
        .collect();
    assert!(resolutions.contains(&"cross_file_resolution"), "{result}");
    assert!(resolutions.contains(&"direct"), "{result}");
    assert!(!resolutions.contains(&"name_match"), "{result}");
    for relation in relations {
        assert!(relation["call_site"]["line"].as_u64().unwrap() >= 1);
        assert_ne!(relation["resolution"], "resolved");
    }

    let direct_only = fixture
        .query(json!({
            "project_id": PROJECT, "snapshot_id": fixture.snapshot, "kind": "neighbors",
            "query": "", "entity_ids": [seed], "relation_kinds": ["static_call_candidate"],
            "max_nodes": 20, "max_depth": 1, "cursor": null,
            "min_producer_resolution": "direct",
            "require_call_site": true
        }))
        .unwrap();
    assert_eq!(
        direct_only["relations"].as_array().unwrap().len(),
        1,
        "{direct_only}"
    );
    assert_eq!(
        direct_only["relations"][0]["producer_resolution"],
        "direct"
    );

    let error = fixture
        .query(json!({
            "project_id": PROJECT, "snapshot_id": fixture.snapshot, "kind": "symbols",
            "query": "main", "entity_ids": [], "relation_kinds": [],
            "max_nodes": 10, "max_depth": 1, "cursor": null,
            "require_call_site": true
        }))
        .unwrap_err();
    assert!(
        error.to_string().contains("only valid for neighbors"),
        "{error}"
    );
}

#[test]
fn exclude_entity_roles_removes_test_references_from_impact() {
    let mut fixture = fixture();
    let root = fixture.root.clone();
    fixture.import(
        vec![
            node(&root, 1, "Function", "src/app.py", "helper"),
            node(&root, 2, "Function", "tests/test_app.py", "test_main"),
        ],
        vec![edge(10, 2, 1, json!({"call_site_line": 2, "is_direct": true}))],
    );
    let helper = entity_id(
        &fixture
            .query(json!({
                "project_id": PROJECT, "snapshot_id": fixture.snapshot, "kind": "symbols",
                "query": "helper", "entity_ids": [], "relation_kinds": [], "max_nodes": 10,
                "max_depth": 1, "cursor": null
            }))
            .unwrap(),
        "::helper",
    );
    let base = json!({
        "project_id": PROJECT, "snapshot_id": fixture.snapshot, "kind": "impact",
        "query": "", "entity_ids": [helper], "relation_kinds": ["static_call_candidate"],
        "max_nodes": 20, "max_depth": 1, "cursor": null
    });
    let default = fixture.query(base.clone()).unwrap();
    assert_eq!(default["relations"].as_array().unwrap().len(), 1);
    let mut excluded_request = base;
    excluded_request["exclude_entity_roles"] = json!(["test"]);
    let excluded = fixture.query(excluded_request).unwrap();
    assert!(
        excluded["relations"].as_array().unwrap().is_empty(),
        "{excluded}"
    );
}

#[test]
fn kind_filter_and_ordering_separate_module_type_and_symbol() {
    let mut fixture = fixture();
    let root = fixture.root.clone();
    fixture.import(
        vec![
            node(&root, 1, "Class", "src/app.py", "Store"),
            node(&root, 2, "Function", "src/app.py", "Store"),
            external_module(3, "Store"),
        ],
        vec![],
    );
    let all = fixture
        .query(json!({
            "project_id": PROJECT, "snapshot_id": fixture.snapshot, "kind": "symbols",
            "query": "Store", "entity_ids": [], "relation_kinds": [], "max_nodes": 10,
            "max_depth": 1, "cursor": null
        }))
        .unwrap();
    let kinds: Vec<&str> = all["entities"]
        .as_array()
        .unwrap()
        .iter()
        .map(|entity| entity["kind"].as_str().unwrap())
        .collect();
    assert_eq!(kinds, vec!["type", "symbol", "module"], "{all}");
    let only_type = fixture
        .query(json!({
            "project_id": PROJECT, "snapshot_id": fixture.snapshot, "kind": "symbols",
            "query": "Store", "entity_ids": [], "relation_kinds": [], "max_nodes": 10,
            "max_depth": 1, "cursor": null, "entity_kinds": ["type"]
        }))
        .unwrap();
    assert_eq!(only_type["entities"].as_array().unwrap().len(), 1);
    assert_eq!(only_type["entities"][0]["kind"], "type");
    let only_module = fixture
        .query(json!({
            "project_id": PROJECT, "snapshot_id": fixture.snapshot, "kind": "symbols",
            "query": "Store", "entity_ids": [], "relation_kinds": [], "max_nodes": 10,
            "max_depth": 1, "cursor": null, "entity_kinds": ["module"]
        }))
        .unwrap();
    assert_eq!(only_module["entities"].as_array().unwrap().len(), 1);
    assert_eq!(only_module["entities"][0]["kind"], "module");
    assert_eq!(only_module["entities"][0]["external"], true);
    assert!(only_module["entities"][0]["anchors"].as_array().unwrap().is_empty());
}

#[test]
fn source_and_symbol_queries_apply_role_exclusions() {
    let mut fixture = fixture();
    let root = fixture.root.clone();
    fixture.import(
        vec![
            node(&root, 1, "Function", "src/app.py", "main"),
            node(&root, 2, "Function", "tests/test_app.py", "test_main"),
            node(&root, 3, "Function", "vendor/lib.min.js", "cytoscape"),
            node(&root, 4, "Function", "Cargo.toml", "package"),
        ],
        vec![],
    );
    let source = fixture
        .query(json!({
            "project_id": PROJECT, "snapshot_id": fixture.snapshot, "kind": "source",
            "query": "VALUE", "entity_ids": [], "relation_kinds": [], "max_nodes": 10,
            "max_depth": 1, "cursor": null, "exclude_entity_roles": ["test"]
        }))
        .unwrap();
    assert_eq!(source["source_matches"].as_array().unwrap().len(), 1, "{source}");
    assert_eq!(source["source_matches"][0]["path"], "src/app.py");

    let vendored = fixture
        .query(json!({
            "project_id": PROJECT, "snapshot_id": fixture.snapshot, "kind": "symbols",
            "query": "cytoscape", "entity_ids": [], "relation_kinds": [], "max_nodes": 10,
            "max_depth": 1, "cursor": null,
            "exclude_entity_roles": ["vendored", "config"]
        }))
        .unwrap();
    assert!(vendored["entities"].as_array().unwrap().is_empty(), "{vendored}");
    let config = fixture
        .query(json!({
            "project_id": PROJECT, "snapshot_id": fixture.snapshot, "kind": "symbols",
            "query": "package", "entity_ids": [], "relation_kinds": [], "max_nodes": 10,
            "max_depth": 1, "cursor": null,
            "exclude_entity_roles": ["vendored", "config"]
        }))
        .unwrap();
    assert!(config["entities"].as_array().unwrap().is_empty(), "{config}");
}
