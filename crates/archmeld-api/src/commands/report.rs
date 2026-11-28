// Copyright 2026 ARCHMELD contributors
// SPDX-License-Identifier: Apache-2.0

//! `report` CLI commands.

use super::*;

#[derive(clap::Args)]
pub(crate) struct NumericCheckArgs {
    /// Ledger used to store the exact artifact bytes.
    #[arg(long)]
    pub(crate) state: PathBuf,
    /// Artifact root; predicate paths resolve inside it.
    #[arg(long, default_value = ".")]
    pub(crate) root: PathBuf,
    /// JSON file containing the numeric_tolerance predicate.
    #[arg(long)]
    pub(crate) predicate: PathBuf,
}

#[derive(clap::Args)]
pub(crate) struct ReportArgs {
    #[arg(long)]
    pub(crate) state: PathBuf,
    #[arg(long)]
    pub(crate) project: String,
    #[arg(long)]
    pub(crate) snapshot: String,
    #[arg(long)]
    pub(crate) out: PathBuf,
    #[arg(long, default_value_t = 5000)]
    pub(crate) max_entities: u32,
    #[arg(long, default_value_t = 20000)]
    pub(crate) max_relations: u32,
    /// Opt in to copying snapshot source bytes into the bundle. The bundle is
    /// then sensitive and the manifest says so.
    #[arg(long)]
    pub(crate) include_source: bool,
    /// Opt in to copying runtime/native trace artifacts into the bundle.
    #[arg(long)]
    pub(crate) include_raw_traces: bool,
}

#[derive(clap::Args)]
pub(crate) struct VerifyArgs {
    #[arg(long)]
    pub(crate) dir: PathBuf,
}

pub(crate) fn run_report(args: ReportArgs) -> Result<Value, String> {
    let mut store = Store::open(&args.state).map_err(|error| error.to_string())?;
    let result = export_report(
        &mut store,
        &json!({
            "project_id": args.project,
            "snapshot_id": args.snapshot,
            "output_dir": args.out.to_string_lossy(),
            "include_source": args.include_source,
            "include_raw_traces": args.include_raw_traces,
            "max_entities": args.max_entities,
            "max_relations": args.max_relations,
        }),
    )
    .map_err(|error| error.to_string())?;
    Ok(json!({
        "command": "report",
        "project_id": args.project,
        "snapshot_id": args.snapshot,
        "output_dir": args.out,
        "report": result,
    }))
}

pub(crate) fn run_numeric_check(args: NumericCheckArgs) -> Result<Value, String> {
    let raw = fs::read(&args.predicate)
        .map_err(|error| format!("predicate {}: {error}", args.predicate.display()))?;
    let predicate: Value = serde_json::from_slice(&raw)
        .map_err(|error| format!("predicate {}: {error}", args.predicate.display()))?;
    let root = args
        .root
        .canonicalize()
        .map_err(|error| format!("root {}: {error}", args.root.display()))?;
    let mut store = Store::open(&args.state).map_err(|error| error.to_string())?;
    let result = evaluate_numeric_tolerance_artifacts(&mut store, &predicate, &root)
        .map_err(|error| error.to_string())?;
    Ok(json!({
        "command": "numeric-check",
        "predicate": args.predicate,
        "root": root,
        "result": result,
    }))
}

pub(crate) fn run_verify(args: VerifyArgs) -> Result<Value, String> {
    let manifest_path = args.dir.join("manifest.json");
    let manifest: Value = serde_json::from_slice(
        &fs::read(&manifest_path)
            .map_err(|error| format!("{}: {error}", manifest_path.display()))?,
    )
    .map_err(|error| format!("{}: {error}", manifest_path.display()))?;
    let verified = verify_bundle(&args.dir, &manifest).map_err(|error| error.to_string())?;
    Ok(json!({"command": "verify-report", "dir": args.dir, "verified": verified}))
}
