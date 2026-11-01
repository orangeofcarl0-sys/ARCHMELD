// Copyright 2026 ARCHMELD contributors
// SPDX-License-Identifier: Apache-2.0

//! One-shot in-memory bridge: parse a workspace with the pinned CodeGraph
//! parsers and write `archmeld-export-v1` NDJSON. No RocksDB, no embedding
//! model, no watcher, no MCP/LSP session.
//!
//! Usage:
//!   archmeld_export <workspace> <export.ndjson> <state-root> [max-files]

use codegraph::export::ndjson::export_ndjson_v1;
use codegraph::CodeGraph;
use codegraph_server::index_state::IndexState;
use codegraph_server::indexer::{IndexConfig, Indexer};
use codegraph_server::parser_registry::ParserRegistry;
use serde_json::json;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Instant;
use tokio::sync::{Mutex, RwLock};

fn slug(path: &PathBuf) -> String {
    let name = path
        .file_name()
        .map(|name| name.to_string_lossy().to_lowercase())
        .unwrap_or_else(|| "workspace".to_string());
    let mut sanitized: String = name
        .chars()
        .map(|character| {
            if character.is_ascii_alphanumeric() || character == '-' || character == '_' {
                character
            } else {
                '-'
            }
        })
        .collect();
    sanitized.truncate(40);
    if sanitized.is_empty() {
        sanitized = "workspace".to_string();
    }
    sanitized
}

#[tokio::main]
async fn main() -> Result<(), String> {
    let args: Vec<String> = std::env::args().collect();
    if args.len() < 4 || args.len() > 5 {
        eprintln!("usage: archmeld_export <workspace> <export.ndjson> <state-root> [max-files]");
        return Err("invalid arguments".to_string());
    }
    let workspace = PathBuf::from(&args[1])
        .canonicalize()
        .map_err(|error| format!("workspace {}: {error}", args[1]))?;
    let export = PathBuf::from(&args[2]);
    let state_root = PathBuf::from(&args[3]);
    let max_files = args
        .get(4)
        .map(|value| value.parse::<usize>().map_err(|error| error.to_string()))
        .transpose()?
        .unwrap_or(5_000);

    std::env::set_var("ARCHMELD_STATE_ROOT", &state_root);
    let graph = Arc::new(RwLock::new(
        CodeGraph::in_memory().map_err(|error| error.to_string())?,
    ));
    let parsers = Arc::new(ParserRegistry::new());
    let index_state = Arc::new(Mutex::new(IndexState::for_workspace(
        &slug(&workspace),
        &workspace,
    )));
    let indexer = Indexer::new(parsers, index_state);
    let config = IndexConfig {
        max_files,
        ..IndexConfig::default()
    };
    let started = Instant::now();
    let indexed = indexer
        .index_workspace(&graph, &[workspace.clone()], &config)
        .await;
    let elapsed = started.elapsed().as_millis();

    let guard = graph.read().await;
    let text = export_ndjson_v1(&guard).map_err(|error| error.to_string())?;
    let nodes = guard.node_count();
    let edges = guard.edge_count();
    drop(guard);
    if let Some(parent) = export.parent() {
        if !parent.as_os_str().is_empty() {
            std::fs::create_dir_all(parent).map_err(|error| error.to_string())?;
        }
    }
    std::fs::write(&export, text).map_err(|error| error.to_string())?;
    println!(
        "{}",
        json!({
            "format": "archmeld-export-v1",
            "path": export,
            "nodes": nodes,
            "edges": edges,
            "files_total": indexed.total_files,
            "files_parsed": indexed.files_parsed,
            "files_skipped": indexed.files_skipped,
            "by_language": indexed.by_language,
            "parser_errors": indexed.parser_errors_by_language,
            "elapsed_ms": elapsed,
            "storage": "in_memory",
        })
    );
    Ok(())
}
