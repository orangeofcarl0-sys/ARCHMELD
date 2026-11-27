// Copyright 2026 ARCHMELD contributors
// SPDX-License-Identifier: Apache-2.0

//! P15: a trait may be parsed after the file that implements it. The indexer
//! must still produce one `Implements` edge, and must not guess when two
//! interfaces share a name.

use codegraph::{CodeGraph, EdgeType, NodeId, NodeType};
use codegraph_server::index_state::IndexState;
use codegraph_server::indexer::{IndexConfig, Indexer};
use codegraph_server::parser_registry::ParserRegistry;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use tokio::sync::{Mutex, RwLock};

async fn index_roots(roots: &[PathBuf]) -> (codegraph_server::indexer::IndexResult, Arc<RwLock<CodeGraph>>) {
    let graph = Arc::new(RwLock::new(CodeGraph::in_memory().unwrap()));
    let indexer = Indexer::new(
        Arc::new(ParserRegistry::new()),
        Arc::new(Mutex::new(IndexState::for_workspace(
            "cross-file-trait",
            &roots[0],
        ))),
    );
    let config = IndexConfig::default();
    let result = indexer.index_workspace(&graph, roots, &config).await;
    (result, graph)
}

fn write(path: &Path, content: &str) {
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(path, content).unwrap();
}

fn node_id(graph: &CodeGraph, name: &str) -> NodeId {
    let ids = graph.query().property("name", name).execute().unwrap();
    assert_eq!(ids.len(), 1, "expected exactly one node named {name}: {ids:?}");
    ids[0]
}

fn implements_edges(graph: &CodeGraph, from: NodeId, to: NodeId) -> usize {
    graph
        .get_edges_between(from, to)
        .unwrap()
        .into_iter()
        .filter(|edge_id| {
            graph
                .get_edge(*edge_id)
                .map(|edge| edge.edge_type == EdgeType::Implements)
                .unwrap_or(false)
        })
        .count()
}

#[tokio::test]
async fn implements_resolves_when_the_impl_file_is_indexed_first() {
    let tmp = tempfile::tempdir().unwrap();
    let impl_root = tmp.path().join("impl");
    let trait_root = tmp.path().join("trait");
    write(
        &impl_root.join("english.rs"),
        "pub struct English {}\nimpl Greeter for English { fn greet(&self) {} }\n",
    );
    write(
        &trait_root.join("greeter.rs"),
        "pub trait Greeter { fn greet(&self); }\n",
    );

    let (result, graph) = index_roots(&[impl_root, trait_root]).await;
    assert_eq!(result.implements_resolved, 1, "{result:?}");
    assert_eq!(result.implements_ambiguous, 0, "{result:?}");
    let graph = graph.read().await;
    let implementor = node_id(&graph, "English");
    let trait_id = node_id(&graph, "Greeter");
    assert_eq!(implements_edges(&graph, implementor, trait_id), 1);
}

#[tokio::test]
async fn implements_resolves_when_the_trait_file_is_indexed_first() {
    let tmp = tempfile::tempdir().unwrap();
    let impl_root = tmp.path().join("impl");
    let trait_root = tmp.path().join("trait");
    write(
        &impl_root.join("english.rs"),
        "pub struct English {}\nimpl Greeter for English { fn greet(&self) {} }\n",
    );
    write(
        &trait_root.join("greeter.rs"),
        "pub trait Greeter { fn greet(&self); }\n",
    );

    let (result, graph) = index_roots(&[trait_root, impl_root]).await;
    assert_eq!(result.implements_ambiguous, 0, "{result:?}");
    // The trait was already in the graph while the impl file was mapped, so the
    // edge may have been added immediately (resolved == 0) rather than by the
    // post-pass. Either path must produce exactly one edge.
    let graph = graph.read().await;
    let implementor = node_id(&graph, "English");
    let trait_id = node_id(&graph, "Greeter");
    assert_eq!(implements_edges(&graph, implementor, trait_id), 1);
}

#[tokio::test]
async fn duplicate_trait_names_are_not_guessed() {
    let tmp = tempfile::tempdir().unwrap();
    let impl_root = tmp.path().join("impl");
    let trait_a = tmp.path().join("trait-a");
    let trait_b = tmp.path().join("trait-b");
    write(
        &impl_root.join("thing.rs"),
        "pub struct Thing {}\nimpl Duplicated for Thing { fn run(&self) {} }\n",
    );
    write(&trait_a.join("a.rs"), "pub trait Duplicated { fn run(&self); }\n");
    write(&trait_b.join("b.rs"), "pub trait Duplicated { fn run(&self); }\n");

    let (result, graph) = index_roots(&[impl_root, trait_a, trait_b]).await;
    assert_eq!(result.implements_ambiguous, 1, "{result:?}");
    assert_eq!(result.implements_resolved, 0, "{result:?}");
    let graph = graph.read().await;
    let implementor = node_id(&graph, "Thing");
    let trait_ids: Vec<NodeId> = graph
        .query()
        .property("name", "Duplicated")
        .execute()
        .unwrap()
        .into_iter()
        .filter(|id| {
            graph
                .get_node(*id)
                .map(|node| node.node_type == NodeType::Interface)
                .unwrap_or(false)
        })
        .collect();
    assert_eq!(trait_ids.len(), 2);
    for trait_id in trait_ids {
        assert_eq!(
            implements_edges(&graph, implementor, trait_id),
            0,
            "ambiguous trait must stay unresolved"
        );
    }
}
