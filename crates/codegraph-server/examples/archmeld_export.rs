// Copyright 2026 ARCHMELD contributors
// SPDX-License-Identifier: Apache-2.0

//! One-shot in-memory bridge: parse a workspace with the pinned CodeGraph
//! parsers and write `archmeld-export-v1` NDJSON. No RocksDB, no embedding
//! model, no watcher, no MCP/LSP session.
//!
//! Usage:
//!   archmeld_export <workspace> <export.ndjson> <state-root> [max-files]

use codegraph::export::ndjson::export_ndjson_v1_with_manifest;
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
    let mut positional: Vec<String> = Vec::new();
    let mut include_dirs: Vec<String> = Vec::new();
    let mut exclude_dirs: Vec<String> = Vec::new();
    let mut max_file_size: Option<u64> = None;
    let mut max_depth: Option<u32> = None;
    let mut iter = std::env::args().skip(1);
    while let Some(arg) = iter.next() {
        match arg.as_str() {
            "--version" => {
                println!(
                    "archmeld-export-v1 helper {} schema 1.2 commit {}",
                    env!("CARGO_PKG_VERSION"),
                    codegraph::metadata::GIT_HASH_SHORT
                );
                return Ok(());
            }
            "--schema" => {
                println!("1.2");
                return Ok(());
            }
            "--include-dir" => {
                include_dirs.push(
                    iter.next()
                        .ok_or_else(|| "--include-dir needs a value".to_string())?,
                );
            }
            "--exclude-dir" => {
                exclude_dirs.push(
                    iter.next()
                        .ok_or_else(|| "--exclude-dir needs a value".to_string())?,
                );
            }
            "--max-file-size" => {
                max_file_size = Some(
                    iter.next()
                        .ok_or_else(|| "--max-file-size needs a value".to_string())?
                        .parse()
                        .map_err(|error| format!("--max-file-size: {error}"))?,
                );
            }
            "--max-depth" => {
                max_depth = Some(
                    iter.next()
                        .ok_or_else(|| "--max-depth needs a value".to_string())?
                        .parse()
                        .map_err(|error| format!("--max-depth: {error}"))?,
                );
            }
            _ => positional.push(arg),
        }
    }
    if positional.len() < 3 || positional.len() > 4 {
        eprintln!(
            "usage: archmeld_export <workspace> <export.ndjson> <state-root> [max-files] \
             [--include-dir NAME] [--exclude-dir NAME] [--max-file-size BYTES] [--max-depth N]"
        );
        return Err("invalid arguments".to_string());
    }
    let workspace = PathBuf::from(&positional[0])
        .canonicalize()
        .map_err(|error| format!("workspace {}: {error}", positional[0]))?;
    let export = PathBuf::from(&positional[1]);
    let state_root = PathBuf::from(&positional[2]);
    let max_files = positional
        .get(3)
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
    let mut config = IndexConfig {
        max_files,
        include_dirs: include_dirs.clone(),
        ..IndexConfig::default()
    };
    if !exclude_dirs.is_empty() {
        config.exclude_dirs.extend(exclude_dirs.iter().cloned());
    }
    if let Some(limit) = max_file_size {
        config.max_file_size_bytes = limit;
    }
    if let Some(depth) = max_depth {
        config.max_depth = depth;
    }
    let started = Instant::now();
    let indexed = indexer
        .index_workspace(&graph, &[workspace.clone()], &config)
        .await;
    let elapsed = started.elapsed().as_millis();
    let parser_errors: Vec<serde_json::Value> = indexed
        .parser_errors
        .iter()
        .map(|error| {
            let path = std::path::Path::new(&error.path)
                .strip_prefix(&workspace)
                .map(|path| path.to_string_lossy().replace('\\', "/"))
                .unwrap_or_else(|_| error.path.replace('\\', "/"));
            json!({
                "path": path,
                "language": error.language,
                "message": error.message,
            })
        })
        .collect();
    let index_manifest = json!({
        "files_total": indexed.total_files,
        "files_parsed": indexed.files_parsed,
        "files_skipped": indexed.files_skipped,
        "by_language": &indexed.by_language,
        "skipped_by_reason": &indexed.skipped_by_reason,
        "parser_errors": &parser_errors,
        "max_files_reached": indexed.max_files_reached,
        "max_depth_reached": indexed.max_depth_reached,
        "cross_file_implements": {
            "resolved": indexed.implements_resolved,
            "ambiguous": indexed.implements_ambiguous,
        },
        "config": {
            "max_files": config.max_files,
            "max_file_size_bytes": config.max_file_size_bytes,
            "max_depth": config.max_depth,
            "exclude_dirs": config.exclude_dirs,
            "include_dirs": config.include_dirs,
        },
    });

    let guard = graph.read().await;
    let text = export_ndjson_v1_with_manifest(&guard, Some(&json!({"index": index_manifest})))
        .map_err(|error| error.to_string())?;
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
            "parser_error_details": parser_errors,
            "skipped_by_reason": indexed.skipped_by_reason,
            "max_files_reached": indexed.max_files_reached,
            "max_depth_reached": indexed.max_depth_reached,
            "elapsed_ms": elapsed,
            "storage": "in_memory",
        })
    );
    Ok(())
}
