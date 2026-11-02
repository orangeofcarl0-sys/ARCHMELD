// Copyright 2026 ARCHMELD contributors
// SPDX-License-Identifier: Apache-2.0

//! AT015 regression: source search reads snapshot bytes, finds text that only
//! exists in a function body, reports a usable location, maps it to entities,
//! and paginates without duplicates.

use archmeld_core::bridge::{import_codegraph_export, BridgeConfig};
use archmeld_core::query::run_query;
use archmeld_core::snapshot::{capture_snapshot, SnapshotInputs, SnapshotOptions};
use archmeld_core::store::Store;
use serde_json::{json, Value};
use std::collections::BTreeSet;
use std::fs;

const PROJECT: &str = "project.source.search";

fn request(
    snapshot_id: &str,
    text: &str,
    max_nodes: u32,
    cursor: Option<&str>,
) -> Value {
    json!({
        "project_id": PROJECT,
        "snapshot_id": snapshot_id,
        "kind": "source",
        "query": text,
        "entity_ids": [],
        "relation_kinds": [],
        "max_nodes": max_nodes,
        "max_depth": 0,
        "cursor": cursor,
    })
}

#[test]
fn source_search_finds_body_text_and_paginates_without_duplicates() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().join("repo");
    fs::create_dir_all(root.join("src")).unwrap();
    let mut late = String::from("FIRST_LINE_MARKER = 1\n");
    for index in 0..220 {
        late.push_str(&format!("# filler {index:03} 迁移\n"));
    }
    let target_line = late.lines().count() + 1;
    late.push_str("def late_target():\n    error_text = 'unique error 9f3'\n    return error_text\n");
    fs::write(root.join("src/late.py"), &late).unwrap();
    let repeated = (0..5)
        .map(|index| format!("# needle occurrence {index}\n"))
        .collect::<String>();
    fs::write(root.join("src/repeated.py"), repeated).unwrap();

    let mut store = Store::open(dir.path().join("state")).unwrap();
    store.ensure_project(PROJECT, "expedition").unwrap();
    let captured = capture_snapshot(
        &mut store,
        PROJECT,
        &root,
        &SnapshotInputs {
            build_config: json!({"variant": "source-search"}),
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
    let node = json!({
        "type":"node","id":1,"node_type":"Function","properties":{
            "path": root.join("src/late.py").to_string_lossy(),
            "name":"late_target","line_start":target_line,"line_end":target_line + 2
        }
    });
    let export = format!(
        "{}\n{}\n",
        json!({
            "type":"manifest","format":"archmeld-export-v1","source":"codegraph",
            "version":"0.2.0","node_count":1,"edge_count":0,"iteration":"real-iterators"
        }),
        node
    );
    import_codegraph_export(
        &mut store,
        &BridgeConfig {
            project_id: PROJECT,
            snapshot_id: &captured.snapshot_id,
            config_digest: &config_digest,
            run_id: "run.source.search",
            workspace_root: Some(&root),
            max_entities_per_batch: 500,
            max_relations_per_batch: 2000,
        },
        &captured.files,
        &export,
    )
    .unwrap();
    let entity_id = store
        .entity_ids(PROJECT, &captured.snapshot_id)
        .unwrap()
        .into_iter()
        .find(|id| {
            store
                .get_entity(PROJECT, &captured.snapshot_id, id)
                .unwrap()
                .and_then(|entity| entity["qualified_name"].as_str().map(str::to_string))
                .is_some_and(|name| name.ends_with("::late_target"))
        })
        .unwrap();

    let found = run_query(
        &store,
        &request(&captured.snapshot_id, "unique error 9f3", 10, None),
    )
    .unwrap();
    assert!(found["entities"].as_array().unwrap().is_empty());
    let matches = found["source_matches"].as_array().unwrap();
    assert_eq!(matches.len(), 1, "{found}");
    assert_eq!(matches[0]["path"], "src/late.py");
    assert_eq!(matches[0]["line"], target_line + 1);
    assert!(matches[0]["byte_start"].as_u64().unwrap() > 1200);
    assert!(matches[0]["text"].as_str().unwrap().contains("unique error 9f3"));
    let late_digest = captured
        .files
        .iter()
        .find(|file| file.path == "src/late.py")
        .unwrap()
        .blob_digest
        .clone();
    assert_eq!(matches[0]["blob_digest"], json!(late_digest));
    assert_eq!(matches[0]["entity_ids"], json!([entity_id]));
    assert_eq!(found["coverage"]["state"], "complete");

    // Metadata search does not see the function body.
    let metadata = run_query(
        &store,
        &json!({
            "project_id": PROJECT,
            "snapshot_id": captured.snapshot_id,
            "kind": "text",
            "query": "unique error 9f3",
            "entity_ids": [],
            "relation_kinds": [],
            "max_nodes": 10,
            "max_depth": 0,
            "cursor": null,
        }),
    )
    .unwrap();
    assert!(metadata["entities"].as_array().unwrap().is_empty());

    let first = run_query(
        &store,
        &request(&captured.snapshot_id, "needle occurrence", 2, None),
    )
    .unwrap();
    assert_eq!(first["source_matches"].as_array().unwrap().len(), 2);
    assert_eq!(first["pagination"]["has_more"], true);
    let cursor = first["next_cursor"].as_str().expect("source cursor");
    let second = run_query(
        &store,
        &request(&captured.snapshot_id, "needle occurrence", 2, Some(cursor)),
    )
    .unwrap();
    assert_eq!(second["source_matches"].as_array().unwrap().len(), 2);
    let cursor = second["next_cursor"].as_str().expect("second source cursor");
    let third = run_query(
        &store,
        &request(&captured.snapshot_id, "needle occurrence", 2, Some(cursor)),
    )
    .unwrap();
    assert_eq!(third["source_matches"].as_array().unwrap().len(), 1);
    assert!(third["next_cursor"].is_null());
    let mut locations = BTreeSet::new();
    for page in [&first, &second, &third] {
        for item in page["source_matches"].as_array().unwrap() {
            locations.insert((
                item["path"].as_str().unwrap().to_string(),
                item["line"].as_u64().unwrap(),
            ));
        }
    }
    assert_eq!(locations.len(), 5, "{locations:?}");

    // A live edit must not change what the old snapshot reports.
    fs::write(
        root.join("src/late.py"),
        "def late_target():\n    error_text = 'rewritten'\n",
    )
    .unwrap();
    let old = run_query(
        &store,
        &request(&captured.snapshot_id, "unique error 9f3", 10, None),
    )
    .unwrap();
    assert_eq!(old["source_matches"].as_array().unwrap().len(), 1);
    assert!(old["source_matches"][0]["text"]
        .as_str()
        .unwrap()
        .contains("unique error 9f3"));
}
