// Copyright 2026 ARCHMELD contributors
// SPDX-License-Identifier: Apache-2.0

//! Import a real SCIP index into an ARCHMELD ledger.
//!
//! Usage:
//!   cargo run -p archmeld-core --example import_scip -- \
//!     <workspace-root> <index.scip> <state-dir> <project-id> <run-id>
//!
//! Captures a disk snapshot (untracked files included), then imports the
//! index. The snapshot is printed with the report so the result is bound to
//! concrete bytes and a config digest.

use archmeld_core::scip::{import_scip, ScipConfig};
use archmeld_core::snapshot::{capture_snapshot, SnapshotInputs, SnapshotOptions};
use archmeld_core::store::Store;
use serde_json::json;
use std::path::PathBuf;
use std::process::ExitCode;

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().collect();
    if args.len() != 6 {
        eprintln!("usage: import_scip <workspace-root> <index.scip> <state-dir> <project-id> <run-id>");
        return ExitCode::from(2);
    }
    match run(&args[1], &args[2], &args[3], &args[4], &args[5]) {
        Ok(line) => {
            println!("{line}");
            ExitCode::SUCCESS
        }
        Err(error) => {
            eprintln!("SCIP import failed: {error}");
            ExitCode::FAILURE
        }
    }
}

fn run(root: &str, index: &str, state: &str, project: &str, run_id: &str) -> Result<String, String> {
    let root = PathBuf::from(root);
    let index_bytes = std::fs::read(index).map_err(|err| format!("cannot read {index}: {err}"))?;
    let mut store = Store::open(state).map_err(|err| err.to_string())?;
    store
        .ensure_project(project, "expedition")
        .map_err(|err| err.to_string())?;
    let captured = capture_snapshot(
        &mut store,
        project,
        &root,
        &SnapshotInputs {
            build_config: json!({"kind": "scip", "variant": "default"}),
            environment: json!({
                "os": std::env::consts::OS,
                "arch": std::env::consts::ARCH,
                "adapter": "scip"
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
    let report = import_scip(
        &mut store,
        &ScipConfig {
            project_id: project,
            snapshot_id: &captured.snapshot_id,
            config_digest: &config_digest,
            run_id,
            max_entities_per_batch: 500,
            max_relations_per_batch: 2000,
        },
        &captured.files,
        &index_bytes,
    )
    .map_err(|err| err.to_string())?;
    let counts = store
        .published_counts(project, &captured.snapshot_id)
        .map_err(|err| err.to_string())?;
    let kinds = store
        .relation_kinds(project, &captured.snapshot_id)
        .map_err(|err| err.to_string())?;
    Ok(json!({
        "project_id": project,
        "snapshot_id": captured.snapshot_id,
        "config_digest": config_digest,
        "snapshot_files": captured.files.len(),
        "index_bytes": index_bytes.len(),
        "scip": {
            "batches": report.batches,
            "documents": report.documents,
            "entities": report.entities,
            "relations": report.relations,
            "external_entities": report.external_entities,
            "diagnostics": report.diagnostics,
            "omitted_documents": report.omitted_documents,
            "unresolved": report.unresolved,
            "coverage": report.coverage,
        },
        "ledger": {
            "entities": counts.entities,
            "relations": counts.relations,
            "relation_kinds": kinds,
            "backend_mappings": counts.backend_mappings,
        }
    })
    .to_string())
}
