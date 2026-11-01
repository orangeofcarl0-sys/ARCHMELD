// Copyright 2026 ARCHMELD contributors
// SPDX-License-Identifier: Apache-2.0

//! AT015 (bounded graph queries), AT016 (historical source), AT073 (events and
//! idempotency) at the core boundary.

use archmeld_core::bridge::{import_codegraph_export, BridgeConfig};
use archmeld_core::events::{events_after, prune_events_before};
use archmeld_core::hash::digest_value;
use archmeld_core::jobs::{cancel_job_idempotent, create_job, get_job, transition_job};
use archmeld_core::query::run_query;
use archmeld_core::snapshot::{capture_snapshot, SnapshotInputs, SnapshotOptions};
use archmeld_core::source_api::read_source;
use archmeld_core::store::Store;
use serde_json::{json, Value};
use std::fs;
use std::path::Path;

const PROJECT: &str = "project.query";

struct Fixture {
    _dir: tempfile::TempDir,
    store: Store,
    root: std::path::PathBuf,
    snapshot_id: String,
}

fn node(root: &Path, id: u64, path: &str, name: &str, line: (u64, u64)) -> Value {
    json!({
        "type": "node",
        "id": id,
        "node_type": "Function",
        "properties": {
            "path": root.join(path).to_string_lossy(),
            "name": name,
            "line_start": line.0,
            "line_end": line.1,
        }
    })
}

fn edge(id: u64, source: u64, target: u64) -> Value {
    json!({
        "type": "edge",
        "id": id,
        "source": source,
        "target": target,
        "edge_type": "Calls",
        "properties": {"line": 1}
    })
}

fn fixture() -> Fixture {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().join("repo");
    fs::create_dir_all(root.join("src")).unwrap();
    fs::write(root.join("src/a.py"), "def a1():\n    return b()\n\ndef a2():\n    pass\n\ndef a3():\n    pass\n\ndef a4():\n    pass\n").unwrap();
    fs::write(root.join("src/b.py"), "def b():\n    return c()\n").unwrap();
    fs::write(root.join("src/c.py"), "def c():\n    return a1()\n").unwrap();
    let mut store = Store::open(dir.path().join("state")).unwrap();
    store.ensure_project(PROJECT, "expedition").unwrap();
    let captured = capture_snapshot(
        &mut store,
        PROJECT,
        &root,
        &SnapshotInputs {
            build_config: json!({"variant": "query-test"}),
            environment: json!({"os": "test"}),
        },
        &SnapshotOptions {
            include_untracked: true,
            ..SnapshotOptions::default()
        },
    )
    .unwrap();
    let config_digest = captured.snapshot["config_digest"].as_str().unwrap().to_string();
    let mut lines = vec![json!({
        "type": "manifest",
        "format": "archmeld-export-v1",
        "source": "codegraph",
        "version": "0.2.0",
        "node_count": 6,
        "edge_count": 4,
        "iteration": "real-iterators",
    })
    .to_string()];
    let nodes = vec![
        node(&root, 1, "src/a.py", "a1", (1, 2)),
        node(&root, 2, "src/b.py", "b", (1, 2)),
        node(&root, 3, "src/c.py", "c", (1, 2)),
        node(&root, 4, "src/a.py", "a2", (4, 5)),
        node(&root, 5, "src/a.py", "a3", (7, 8)),
        node(&root, 6, "src/a.py", "a4", (10, 11)),
    ];
    for node in &nodes {
        lines.push(node.to_string());
    }
    for edge in [edge(1, 1, 2), edge(2, 2, 3), edge(3, 3, 1), edge(4, 1, 4)] {
        lines.push(edge.to_string());
    }
    let export = lines.join("\n") + "\n";
    import_codegraph_export(
        &mut store,
        &BridgeConfig {
            project_id: PROJECT,
            snapshot_id: &captured.snapshot_id,
            config_digest: &config_digest,
            run_id: "run.query.001",
            workspace_root: Some(&root),
            max_entities_per_batch: 500,
            max_relations_per_batch: 2000,
        },
        &captured.files,
        &export,
    )
    .unwrap();
    Fixture {
        _dir: dir,
        store,
        root,
        snapshot_id: captured.snapshot_id,
    }
}

fn request(fx: &Fixture, kind: &str, extra: Value) -> Value {
    let mut body = json!({
        "project_id": PROJECT,
        "snapshot_id": fx.snapshot_id,
        "kind": kind,
        "query": "",
        "entity_ids": [],
        "relation_kinds": ["static_call_candidate"],
        "max_nodes": 2,
        "max_depth": 4,
        "cursor": null,
    });
    if let (Some(target), Some(source)) = (body.as_object_mut(), extra.as_object()) {
        for (key, value) in source {
            target.insert(key.clone(), value.clone());
        }
    }
    body
}

fn find_id(store: &Store, snapshot_id: &str, name: &str) -> String {
    let ids = store.entity_ids(PROJECT, snapshot_id).unwrap();
    for id in ids {
        if let Some(entity) = store.get_entity(PROJECT, snapshot_id, &id).unwrap() {
            if entity["qualified_name"]
                .as_str()
                .is_some_and(|qualified| qualified.ends_with(&format!("::{name}")))
            {
                return id;
            }
        }
    }
    panic!("entity {name} not found")
}

#[test]
fn neighbors_are_bounded_paginated_and_cycle_safe() {
    let mut fx = fixture();
    let a1 = find_id(&fx.store, &fx.snapshot_id, "a1");
    let first = run_query(
        &fx.store,
        &request(&fx, "neighbors", json!({"entity_ids": [a1], "max_nodes": 2})),
    )
    .expect("first page");
    assert_eq!(first["entities"].as_array().unwrap().len(), 2);
    assert!(first["next_cursor"].is_string(), "paging cursor expected");
    assert_eq!(first["coverage"]["state"], "partial");
    assert!(first["warnings"]
        .as_array()
        .unwrap()
        .iter()
        .any(|warning| warning.as_str().unwrap().contains("paged")));

    let mut second_request = request(&fx, "neighbors", json!({"entity_ids": [a1], "max_nodes": 2}));
    second_request["cursor"] = first["next_cursor"].clone();
    let second = run_query(&fx.store, &second_request).expect("second page");
    assert_eq!(second["entities"].as_array().unwrap().len(), 2);
    let ids_first: Vec<&str> = first["entities"]
        .as_array()
        .unwrap()
        .iter()
        .map(|entity| entity["id"].as_str().unwrap())
        .collect();
    for entity in second["entities"].as_array().unwrap() {
        assert!(!ids_first.contains(&entity["id"].as_str().unwrap()));
    }
    assert!(second["coverage"]["expected_count"].as_u64().unwrap() >= 4);

    // A cursor is bound to the snapshot and to the query.
    let other = capture_snapshot(
        &mut fx.store,
        PROJECT,
        &fx.root,
        &SnapshotInputs {
            build_config: json!({"variant": "other"}),
            environment: json!({"os": "test"}),
        },
        &SnapshotOptions {
            include_untracked: true,
            ..SnapshotOptions::default()
        },
    )
    .expect("second snapshot");
    let mut other_snapshot = second_request.clone();
    other_snapshot["snapshot_id"] = json!(other.snapshot_id);
    let err = run_query(&fx.store, &other_snapshot).expect_err("cursor scope");
    assert!(err.to_string().contains("different snapshot"), "{err}");

    let mut changed_query = second_request.clone();
    changed_query["max_depth"] = json!(3);
    let err = run_query(&fx.store, &changed_query).expect_err("cursor query digest");
    assert!(err.to_string().contains("different query"), "{err}");
}

#[test]
fn impact_walks_incoming_edges_and_path_is_shortest() {
    let fx = fixture();
    let c = find_id(&fx.store, &fx.snapshot_id, "c");
    let impact = run_query(
        &fx.store,
        &request(&fx, "impact", json!({"entity_ids": [c], "max_nodes": 4})),
    )
    .expect("impact");
    let names: Vec<String> = impact["entities"]
        .as_array()
        .unwrap()
        .iter()
        .map(|entity| entity["qualified_name"].as_str().unwrap_or("").to_string())
        .collect();
    assert!(names.iter().any(|name| name.ends_with("::b")));
    assert!(names.iter().any(|name| name.ends_with("::a1")));

    let a1 = find_id(&fx.store, &fx.snapshot_id, "a1");
    let path = run_query(
        &fx.store,
        &request(
            &fx,
            "path",
            json!({"entity_ids": [a1, c], "max_nodes": 10}),
        ),
    )
    .expect("path");
    assert!(path["next_cursor"].is_null());
    let path_names: Vec<String> = path["entities"]
        .as_array()
        .unwrap()
        .iter()
        .map(|entity| entity["qualified_name"].as_str().unwrap_or("").to_string())
        .collect();
    assert_eq!(path_names.len(), 3, "{path_names:?}");
    assert!(path_names[0].ends_with("::a1"));
    assert!(path_names[2].ends_with("::c"));
    assert_eq!(path["relations"].as_array().unwrap().len(), 2);
}

#[test]
fn search_and_entity_reads_are_snapshot_scoped() {
    let fx = fixture();
    let result = run_query(
        &fx.store,
        &request(&fx, "symbols", json!({"query": "a", "max_nodes": 10})),
    )
    .expect("symbols");
    assert!(result["entities"].as_array().unwrap().len() >= 4);
    assert!(result["warnings"]
        .as_array()
        .unwrap()
        .iter()
        .any(|warning| warning.as_str().unwrap().contains("FTS")));

    let id = find_id(&fx.store, &fx.snapshot_id, "a1");
    let entity = fx
        .store
        .get_entity(PROJECT, &fx.snapshot_id, &id)
        .unwrap()
        .expect("entity");
    assert!(entity["qualified_name"].as_str().unwrap().ends_with("::a1"));
    assert!(fx
        .store
        .get_entity(PROJECT, &format!("sha256:{}", "ee".repeat(32)), &id)
        .is_err());
}

#[test]
fn source_reads_old_snapshot_bytes_after_the_file_moves() {
    let fx = fixture();
    let a1 = find_id(&fx.store, &fx.snapshot_id, "a1");
    let body = json!({
        "project_id": PROJECT,
        "snapshot_id": fx.snapshot_id,
        "entity_id": a1,
        "anchor_index": 0,
        "max_bytes": 1048576,
    });
    let old = read_source(&fx.store, &body).expect("old source");
    assert!(old["text"].as_str().unwrap().contains("def a1"));
    assert_eq!(old["redacted"], false);
    assert_eq!(old["truncated"], false);

    // Move the file on disk; the historical snapshot must not follow it.
    fs::write(fx.root.join("src/a.py"), "# moved\ndef a1():\n    return 0\n").unwrap();
    let again = read_source(&fx.store, &body).expect("historical source");
    assert_eq!(old["text"], again["text"]);
    assert!(again["text"].as_str().unwrap().starts_with("def a1"));

    let mut small = body.clone();
    small["max_bytes"] = json!(5);
    let clipped = read_source(&fx.store, &small).expect("clipped source");
    assert_eq!(clipped["truncated"], true);
    assert!(clipped["text"].as_str().unwrap().len() <= 5);
}

fn empty_store() -> (tempfile::TempDir, Store) {
    let dir = tempfile::tempdir().unwrap();
    let mut store = Store::open(dir.path().join("state")).unwrap();
    store.ensure_project(PROJECT, "expedition").unwrap();
    (dir, store)
}

#[test]
fn job_transitions_and_cancel_idempotency() {
    let (_dir, mut store) = empty_store();
    let job = create_job(&mut store, PROJECT, "query").expect("job");
    let job_id = job["id"].as_str().unwrap().to_string();
    assert_eq!(job["state"], "queued");
    assert_eq!(job["progress"], 0.0);

    let running = transition_job(&mut store, PROJECT, &job_id, "running", 0.5).expect("running");
    assert_eq!(running["state"], "running");
    let err = transition_job(&mut store, PROJECT, &job_id, "queued", 0.0)
        .expect_err("illegal backwards transition");
    assert!(err.to_string().contains("cannot move"), "{err}");

    let digest = digest_value(&json!({"job": job_id})).unwrap();
    let (status, cancelled) =
        cancel_job_idempotent(&mut store, PROJECT, &job_id, 2, "idem-1", &digest).expect("cancel");
    assert_eq!(status, 202);
    assert_eq!(cancelled["state"], "cancelled");

    let (status_again, replay) =
        cancel_job_idempotent(&mut store, PROJECT, &job_id, 2, "idem-1", &digest).expect("replay");
    assert_eq!(status_again, 202);
    assert_eq!(replay, cancelled);

    let other = digest_value(&json!({"job": job_id, "other": true})).unwrap();
    let err = cancel_job_idempotent(&mut store, PROJECT, &job_id, 2, "idem-1", &other)
        .expect_err("same key different body");
    assert!(err.to_string().contains("different request body"), "{err}");

    let fresh = create_job(&mut store, PROJECT, "query-2").expect("job 2");
    let fresh_id = fresh["id"].as_str().unwrap().to_string();
    let err = cancel_job_idempotent(&mut store, PROJECT, &fresh_id, 99, "idem-2", &digest)
        .expect_err("stale revision");
    assert!(err.to_string().contains("expected_revision"), "{err}");
    assert_eq!(get_job(&store, PROJECT, &fresh_id).unwrap().unwrap()["state"], "queued");
}

#[test]
fn events_replay_and_resync_after_pruning() {
    let (_dir, mut store) = empty_store();
    let job = create_job(&mut store, PROJECT, "query").expect("job");
    let job_id = job["id"].as_str().unwrap().to_string();
    transition_job(&mut store, PROJECT, &job_id, "running", 0.1).expect("running");
    transition_job(&mut store, PROJECT, &job_id, "succeeded", 1.0).expect("succeeded");

    let page = events_after(&store, PROJECT, None).expect("events");
    assert!(!page.resync_required);
    assert_eq!(page.events.len(), 2);
    assert_eq!(page.events[0]["type"], "job.state_changed");
    assert_eq!(page.events[0]["sequence"], "0");
    assert_eq!(page.events[1]["sequence"], "1");
    assert!(page.events[0]["timestamp"].as_str().unwrap().ends_with('Z'));

    let after_last = events_after(&store, PROJECT, Some(1)).expect("tail");
    assert!(after_last.events.is_empty());

    prune_events_before(&mut store, PROJECT, 2).expect("prune");
    let stale = events_after(&store, PROJECT, Some(0)).expect("stale cursor");
    assert!(stale.resync_required, "pruned cursor must request resync");
    let ok = events_after(&store, PROJECT, Some(1)).expect("post-prune cursor");
    assert!(!ok.resync_required);
}
