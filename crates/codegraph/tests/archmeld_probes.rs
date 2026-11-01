// Copyright 2026 ARCHMELD contributors
// SPDX-License-Identifier: Apache-2.0

//! B002 upstream boundary probes for the pinned CodeGraph revision.
//!
//! These are probes, not product acceptance tests: they record what the pinned
//! upstream code actually does so the bridge can avoid relying on it.
//!
//! Run with:
//! `cargo test -p codegraph --test archmeld_probes -- --nocapture --test-threads=1`

use codegraph::export::json::export_json;
use codegraph::{CodeGraph, NodeType, PropertyMap};
use serde_json::Value;
use std::collections::BTreeSet;

fn node(name: &str) -> PropertyMap {
    PropertyMap::new().with("name", name)
}

fn exported_ids(json: &str) -> BTreeSet<u64> {
    let value: Value = serde_json::from_str(json).expect("export json");
    value["nodes"]
        .as_array()
        .expect("nodes array")
        .iter()
        .map(|node| node["id"].as_u64().expect("numeric id"))
        .collect()
}

fn real_ids(graph: &CodeGraph) -> BTreeSet<u64> {
    graph.iter_nodes().map(|(id, _)| id).collect()
}

#[test]
fn probe_sparse_ids_after_delete_and_reinsert() {
    let mut graph = CodeGraph::in_memory().expect("graph");
    let first = graph.add_node(NodeType::Function, node("a")).unwrap();
    let second = graph.add_node(NodeType::Function, node("b")).unwrap();
    let third = graph.add_node(NodeType::Function, node("c")).unwrap();
    let fourth = graph.add_node(NodeType::Function, node("d")).unwrap();
    graph.delete_node(second).unwrap();
    let fifth = graph.add_node(NodeType::Function, node("e")).unwrap();

    let real = real_ids(&graph);
    let exported = exported_ids(&export_json(&graph).unwrap());
    let missing: Vec<u64> = real.difference(&exported).copied().collect();
    let extra: Vec<u64> = exported.difference(&real).copied().collect();
    println!(
        "PROBE sparse_export {{\"count\":{},\"real\":{:?},\"exported\":{:?},\"missing\":{:?},\"extra\":{:?}}}",
        graph.node_count(),
        real,
        exported,
        missing,
        extra
    );
    assert_eq!(
        (first, third, fourth, fifth),
        (0, 2, 3, 4),
        "ID allocation is expected to stay monotonic"
    );
    assert_eq!(graph.node_count(), 4);
    assert_eq!(
        missing,
        vec![4],
        "pinned export_json iterates 0..node_count and therefore drops the highest live ID after a delete"
    );
    assert!(extra.is_empty());
}

#[test]
fn probe_sparse_edges_after_delete_and_reinsert() {
    let mut graph = CodeGraph::in_memory().expect("graph");
    let a = graph.add_node(NodeType::Function, node("a")).unwrap();
    let b = graph.add_node(NodeType::Function, node("b")).unwrap();
    let c = graph.add_node(NodeType::Function, node("c")).unwrap();
    let e1 = graph
        .add_edge(a, b, codegraph::EdgeType::Calls, PropertyMap::new())
        .unwrap();
    let _e2 = graph
        .add_edge(b, c, codegraph::EdgeType::Calls, PropertyMap::new())
        .unwrap();
    graph.delete_edge(e1).unwrap();
    let e3 = graph
        .add_edge(c, a, codegraph::EdgeType::Calls, PropertyMap::new())
        .unwrap();
    assert_eq!(e3, 2);

    let real: BTreeSet<u64> = graph.iter_edges().map(|(id, _)| id).collect();
    let value: Value = serde_json::from_str(&export_json(&graph).unwrap()).unwrap();
    let exported: BTreeSet<u64> = value["links"]
        .as_array()
        .expect("links array")
        .iter()
        .map(|edge| edge["id"].as_u64().expect("numeric id"))
        .collect();
    let missing: Vec<u64> = real.difference(&exported).copied().collect();
    println!(
        "PROBE sparse_edges {{\"count\":{},\"real\":{:?},\"exported\":{:?},\"missing\":{:?}}}",
        graph.edge_count(),
        real,
        exported,
        missing
    );
    assert_eq!(missing, vec![2]);
}

#[test]
fn export_ndjson_v1_matches_real_iterators() {
    let mut graph = CodeGraph::in_memory().expect("graph");
    let a = graph.add_node(NodeType::Function, node("a")).unwrap();
    let b = graph.add_node(NodeType::Function, node("b")).unwrap();
    let c = graph.add_node(NodeType::Function, node("c")).unwrap();
    graph.delete_node(b).unwrap();
    let d = graph.add_node(NodeType::Function, node("d")).unwrap();
    graph
        .add_edge(a, c, codegraph::EdgeType::Calls, PropertyMap::new())
        .unwrap();
    graph
        .add_edge(c, d, codegraph::EdgeType::References, PropertyMap::new())
        .unwrap();

    let text = codegraph::export::ndjson::export_ndjson_v1(&graph).unwrap();
    let mut exported_nodes = BTreeSet::new();
    let mut exported_edges = BTreeSet::new();
    for line in text.lines() {
        let value: Value = serde_json::from_str(line).unwrap();
        match value["type"].as_str().unwrap() {
            "manifest" => {}
            "node" => {
                exported_nodes.insert(value["id"].as_u64().unwrap());
            }
            "edge" => {
                exported_edges.insert(value["id"].as_u64().unwrap());
            }
            other => panic!("unexpected {other}"),
        }
    }
    assert_eq!(exported_nodes, real_ids(&graph));
    assert_eq!(
        exported_edges,
        graph.iter_edges().map(|(id, _)| id).collect::<BTreeSet<u64>>()
    );
}

#[cfg(feature = "rocksdb-backend")]
#[test]
fn probe_sparse_ids_survive_persistence() {
    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path().join("graph.db");
    let live_ids;
    {
        let mut graph = CodeGraph::open(&path).expect("open");
        let _a = graph.add_node(NodeType::Function, node("a")).unwrap();
        let b = graph.add_node(NodeType::Function, node("b")).unwrap();
        let _c = graph.add_node(NodeType::Function, node("c")).unwrap();
        graph.delete_node(b).unwrap();
        let _d = graph.add_node(NodeType::Function, node("d")).unwrap();
        live_ids = real_ids(&graph);
    }
    let graph = CodeGraph::open(&path).expect("reopen");
    let after = real_ids(&graph);
    let exported = exported_ids(&export_json(&graph).unwrap());
    let missing: Vec<u64> = after.difference(&exported).copied().collect();
    println!(
        "PROBE persistence {{\"before\":{:?},\"after\":{:?},\"exported\":{:?},\"missing\":{:?}}}",
        live_ids, after, exported, missing
    );
    assert_eq!(live_ids, after);
    assert_eq!(missing, vec![3]);
}

#[cfg(feature = "rocksdb-backend")]
#[test]
fn probe_cache_directories_are_not_shared() {
    let dir = tempfile::tempdir().expect("tempdir");
    let one = dir.path().join("one.db");
    let two = dir.path().join("two.db");
    {
        let mut graph = CodeGraph::open(&one).expect("one");
        graph.add_node(NodeType::Function, node("one-a")).unwrap();
        graph.add_node(NodeType::Function, node("one-b")).unwrap();
    }
    {
        let mut graph = CodeGraph::open(&two).expect("two");
        graph.add_node(NodeType::Function, node("two-a")).unwrap();
    }
    let one_graph = CodeGraph::open(&one).expect("one reopen");
    let two_graph = CodeGraph::open(&two).expect("two reopen");
    let one_names: Vec<String> = one_graph
        .iter_nodes()
        .map(|(_, n)| n.properties.get_string("name").unwrap_or_default().to_owned())
        .collect();
    let two_names: Vec<String> = two_graph
        .iter_nodes()
        .map(|(_, n)| n.properties.get_string("name").unwrap_or_default().to_owned())
        .collect();
    println!(
        "PROBE cache_isolation {{\"one\":{:?},\"two\":{:?}}}",
        one_names, two_names
    );
    assert_eq!(one_names, vec!["one-a", "one-b"]);
    assert_eq!(two_names, vec!["two-a"]);
}
