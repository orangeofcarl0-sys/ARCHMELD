// Copyright 2026 ARCHMELD contributors
// SPDX-License-Identifier: Apache-2.0

//! P13/P14: the indexer must report what it skipped and why, and parser
//! failures must carry the file path, not just a per-language count.

use codegraph::CodeGraph;
use codegraph_server::index_state::IndexState;
use codegraph_server::indexer::{IndexConfig, Indexer};
use codegraph_server::parser_registry::ParserRegistry;
use std::path::Path;
use std::sync::Arc;
use tokio::sync::{Mutex, RwLock};

fn indexer(workspace: &Path) -> Indexer {
    Indexer::new(
        Arc::new(ParserRegistry::new()),
        Arc::new(Mutex::new(IndexState::for_workspace(
            "index-coverage",
            workspace,
        ))),
    )
}

async fn index(
    workspace: &Path,
    config: &IndexConfig,
) -> (
    codegraph_server::indexer::IndexResult,
    Arc<RwLock<CodeGraph>>,
) {
    let graph = Arc::new(RwLock::new(CodeGraph::in_memory().unwrap()));
    let indexer = indexer(workspace);
    let result = indexer
        .index_workspace(&graph, &[workspace.to_path_buf()], config)
        .await;
    (result, graph)
}

#[tokio::test]
async fn skip_reasons_and_parser_error_paths_are_reported() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    std::fs::create_dir_all(root.join("src")).unwrap();
    std::fs::create_dir_all(root.join("examples")).unwrap();
    std::fs::write(root.join("src/a.rs"), "pub fn a() {}\n").unwrap();
    std::fs::write(root.join("examples/demo.py"), "VALUE = 1\n").unwrap();
    std::fs::write(root.join("big.py"), vec![b'x'; 2048]).unwrap();
    std::fs::write(root.join("notes.txt"), "not source\n").unwrap();
    std::fs::write(root.join(".hidden.rs"), "pub fn hidden() {}\n").unwrap();
    // Invalid UTF-8 is a deterministic parser failure: the TOML parser reads
    // the file as text before tree-sitter sees it.
    std::fs::write(root.join("broken.toml"), [0xff_u8, 0xfe]).unwrap();

    let config = IndexConfig {
        max_file_size_bytes: 1024,
        ..IndexConfig::default()
    };
    let (result, _graph) = index(root, &config).await;

    assert!(result.total_files >= 1, "{result:?}");
    assert_eq!(result.by_language.get("rust"), Some(&1), "{result:?}");
    assert_eq!(
        result.by_language.get("python"),
        None,
        "examples/ is excluded by default: {result:?}"
    );

    let skipped = &result.skipped_by_reason;
    assert!(skipped.get("excluded_dir").copied().unwrap_or(0) >= 1, "{result:?}");
    assert!(skipped.get("too_large").copied().unwrap_or(0) >= 1, "{result:?}");
    assert!(
        skipped.get("unsupported_extension").copied().unwrap_or(0) >= 1,
        "{result:?}"
    );
    assert!(skipped.get("hidden").copied().unwrap_or(0) >= 1, "{result:?}");

    let error = result
        .parser_errors
        .iter()
        .find(|error| error.path.ends_with("broken.toml"))
        .unwrap_or_else(|| panic!("no parser error for broken.toml: {result:?}"));
    assert_eq!(error.language, "toml");
    assert!(!error.message.is_empty());
    assert_eq!(
        result.parser_errors_by_language.get("toml"),
        Some(&1),
        "{result:?}"
    );
}

#[tokio::test]
async fn include_dir_admits_examples_and_survives_the_walk() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    std::fs::create_dir_all(root.join("examples")).unwrap();
    std::fs::write(root.join("examples/demo.py"), "VALUE = 1\n").unwrap();

    let config = IndexConfig {
        include_dirs: vec!["examples".to_string()],
        ..IndexConfig::default()
    };
    let (result, graph) = index(root, &config).await;

    assert_eq!(result.by_language.get("python"), Some(&1), "{result:?}");
    assert_eq!(
        result.skipped_by_reason.get("excluded_dir"),
        None,
        "{result:?}"
    );
    let graph = graph.read().await;
    let found = graph.nodes_iter().any(|(_, node)| {
        node.properties
            .get_string("path")
            .map(|path| path.replace('\\', "/").ends_with("examples/demo.py"))
            .unwrap_or(false)
    });
    assert!(found, "examples/demo.py must be in the graph");
}

#[tokio::test]
async fn max_files_and_max_depth_are_visible() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    std::fs::create_dir_all(root.join("a/b/c")).unwrap();
    std::fs::write(root.join("a/b/c/deep.rs"), "pub fn deep() {}\n").unwrap();
    std::fs::write(root.join("first.rs"), "pub fn first() {}\n").unwrap();
    std::fs::write(root.join("second.rs"), "pub fn second() {}\n").unwrap();

    let limited = IndexConfig {
        max_files: 1,
        ..IndexConfig::default()
    };
    let (files, _) = index(root, &limited).await;
    assert!(files.max_files_reached, "{files:?}");
    assert!(files.total_files <= 1, "{files:?}");
    assert!(
        files.skipped_by_reason.get("max_files").copied().unwrap_or(0) >= 1,
        "{files:?}"
    );

    let shallow = IndexConfig {
        max_depth: 1,
        ..IndexConfig::default()
    };
    let (depth, _) = index(root, &shallow).await;
    assert!(depth.max_depth_reached, "{depth:?}");
    assert!(
        depth
            .skipped_by_reason
            .get("max_depth")
            .copied()
            .unwrap_or(0)
            >= 1,
        "{depth:?}"
    );
}
