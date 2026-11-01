// Copyright 2026 ARCHMELD contributors
// SPDX-License-Identifier: Apache-2.0

//! Import a real `archmeld-export-v1` NDJSON file into an ARCHMELD ledger.
//!
//! Usage:
//!   cargo run -p archmeld-core --example import_export -- \
//!     <workspace-root> <export.ndjson> <state-dir> <project-id> <run-id>
//!
//! The snapshot is captured from the same workspace with untracked files
//! enabled (real analysis targets are clean or dirty checkouts alike).

use archmeld_core::bridge::{import_codegraph_export, BridgeConfig};
use archmeld_core::snapshot::{capture_snapshot, SnapshotInputs, SnapshotOptions};
use archmeld_core::store::Store;
use serde_json::json;
use std::path::PathBuf;
use std::process::ExitCode;

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().collect();
    if args.len() != 6 {
        eprintln!(
            "usage: import_export <workspace-root> <export.ndjson> <state-dir> <project-id> <run-id>"
        );
        return ExitCode::from(2);
    }
    match run(&args[1], &args[2], &args[3], &args[4], &args[5]) {
        Ok(line) => {
            println!("{line}");
            ExitCode::SUCCESS
        }
        Err(err) => {
            eprintln!("import failed: {err}");
            ExitCode::FAILURE
        }
    }
}

fn run(root: &str, export: &str, state: &str, project: &str, run_id: &str) -> Result<String, String> {
    let root = PathBuf::from(root);
    let export_text = std::fs::read_to_string(export)
        .map_err(|err| format!("cannot read {export}: {err}"))?;
    let mut store = Store::open(state).map_err(|err| err.to_string())?;
    store
        .ensure_project(project, "expedition")
        .map_err(|err| err.to_string())?;

    let captured = capture_snapshot(
        &mut store,
        project,
        &root,
        &SnapshotInputs {
            build_config: json!({
                "kind": "native",
                "variant": "default",
                "producer": "codegraph-export-v1"
            }),
            environment: json!({
                "os": std::env::consts::OS,
                "arch": std::env::consts::ARCH,
                "adapter": "codegraph"
            }),
        },
        &SnapshotOptions {
            include_untracked: true,
            ..SnapshotOptions::default()
        },
    )
    .map_err(|err| err.to_string())?;
    let config_digest = captured.snapshot["config_digest"]
        .as_str()
        .ok_or("snapshot has no config_digest")?
        .to_string();

    let report = import_codegraph_export(
        &mut store,
        &BridgeConfig {
            project_id: project,
            snapshot_id: &captured.snapshot_id,
            config_digest: &config_digest,
            run_id,
            workspace_root: Some(&root),
            max_entities_per_batch: 500,
            max_relations_per_batch: 2000,
        },
        &captured.files,
        &export_text,
    )
    .map_err(|err| err.to_string())?;

    let counts = store
        .published_counts(project, &captured.snapshot_id)
        .map_err(|err| err.to_string())?;
    Ok(json!({
        "project_id": project,
        "snapshot_id": captured.snapshot_id,
        "config_digest": config_digest,
        "snapshot_files": captured.files.len(),
        "bridge": {
            "batches": report.batches,
            "entities": report.entities,
            "relations": report.relations,
            "diagnostics": report.diagnostics,
            "skipped_nodes": report.skipped_nodes,
            "unresolved": report.unresolved,
            "coverage": report.coverage,
        },
        "ledger": {
            "entities": counts.entities,
            "evidence": counts.evidence,
            "relations": counts.relations,
            "relation_evidence": counts.relation_evidence,
            "backend_mappings": counts.backend_mappings,
        }
    })
    .to_string())
}
