// Copyright 2026 ARCHMELD contributors
// SPDX-License-Identifier: Apache-2.0

//! `index` CLI commands.

use super::*;

#[derive(clap::Args)]
pub(crate) struct IndexArgs {
    #[arg(long)]
    pub(crate) workspace: PathBuf,
    #[arg(long)]
    pub(crate) state: PathBuf,
    #[arg(long, default_value = "project.local")]
    pub(crate) project: String,
    #[arg(long, default_value_t = 5000)]
    pub(crate) max_files: usize,
    /// Override the packaged codegraph helper path (defaults to a sibling
    /// `archmeld-index` executable).
    #[arg(long)]
    pub(crate) index_helper: Option<PathBuf>,
    #[arg(long)]
    pub(crate) run_id: Option<String>,
    /// Include a directory name the indexer would otherwise exclude
    /// (repeatable; for example `examples`).
    #[arg(long = "include-dir")]
    pub(crate) include_dir: Vec<String>,
    /// Add a directory name to the indexer exclusions.
    #[arg(long = "exclude-dir")]
    pub(crate) exclude_dir: Vec<String>,
    /// Override the helper's maximum file size.
    #[arg(long)]
    pub(crate) max_file_size: Option<u64>,
    /// Override the helper's maximum directory depth.
    #[arg(long)]
    pub(crate) max_depth: Option<u32>,
    /// Exclude untracked files even when they are not gitignored.
    #[arg(long, conflicts_with = "include_untracked")]
    pub(crate) tracked_only: bool,
    /// Explicitly include untracked, non-ignored files (the default).
    #[arg(long = "include-untracked")]
    pub(crate) include_untracked: bool,
}

#[derive(clap::Args)]
pub(crate) struct StatusArgs {
    #[arg(long)]
    pub(crate) state: PathBuf,
    /// Restrict the report to one project; otherwise every registered project
    /// is listed.
    #[arg(long)]
    pub(crate) project: Option<String>,
    /// Optional workspace to compare with the latest snapshot. This only reads
    /// the workspace; it never creates a snapshot or records a drift report.
    #[arg(long)]
    pub(crate) workspace: Option<PathBuf>,
}

#[derive(clap::Args)]
pub(crate) struct RefreshArgs {
    #[arg(long)]
    pub(crate) state: PathBuf,
    #[arg(long)]
    pub(crate) project: String,
    #[arg(long)]
    pub(crate) workspace: PathBuf,
    /// Base snapshot to diff from; defaults to the latest snapshot.
    #[arg(long)]
    pub(crate) base_snapshot: Option<String>,
    #[arg(long, default_value_t = 5000)]
    pub(crate) max_files: usize,
    #[arg(long)]
    pub(crate) index_helper: Option<PathBuf>,
    #[arg(long)]
    pub(crate) run_id: Option<String>,
    /// Include a directory name the indexer would otherwise exclude.
    #[arg(long = "include-dir")]
    pub(crate) include_dir: Vec<String>,
    /// Add a directory name to the indexer exclusions.
    #[arg(long = "exclude-dir")]
    pub(crate) exclude_dir: Vec<String>,
    #[arg(long)]
    pub(crate) max_file_size: Option<u64>,
    #[arg(long)]
    pub(crate) max_depth: Option<u32>,
    #[arg(long, conflicts_with = "include_untracked")]
    pub(crate) tracked_only: bool,
    #[arg(long = "include-untracked")]
    pub(crate) include_untracked: bool,
}

pub(crate) fn run_status(args: StatusArgs) -> Result<Value, String> {
    let store = Store::open(&args.state).map_err(|error| error.to_string())?;
    let workspace = match &args.workspace {
        Some(path) => {
            let canonical = path
                .canonicalize()
                .map_err(|error| format!("workspace {}: {error}", path.display()))?;
            if !canonical.is_dir() {
                return Err(format!(
                    "workspace {} is not a directory",
                    canonical.display()
                ));
            }
            Some(canonical)
        }
        None => None,
    };
    let mut result = status_report(&store, args.project.as_deref(), workspace.as_deref())
        .map_err(|error| error.to_string())?;
    result["command"] = json!("status");
    Ok(result)
}

pub(crate) fn run_index(args: IndexArgs) -> Result<Value, String> {
    let helper = helper_path(args.index_helper.clone())?;
    let mut runner = helper_runner(
        args.state.join("codegraph"),
        args.max_files,
        args.include_dir.clone(),
        args.exclude_dir.clone(),
        args.max_file_size,
        args.max_depth,
    );
    run_index_with(args, &helper, &mut runner)
}

pub(crate) fn run_refresh(args: RefreshArgs) -> Result<Value, String> {
    let helper = helper_path(args.index_helper.clone())?;
    let mut runner = helper_runner(
        args.state.join("codegraph"),
        args.max_files,
        args.include_dir.clone(),
        args.exclude_dir.clone(),
        args.max_file_size,
        args.max_depth,
    );
    run_refresh_with(args, &helper, &mut runner)
}

pub(crate) fn run_refresh_with(
    args: RefreshArgs,
    helper: &PathBuf,
    runner: &mut HelperRunner<'_>,
) -> Result<Value, String> {
    let started = Instant::now();
    let store = Store::open(&args.state).map_err(|error| error.to_string())?;
    let base_snapshot_id = match args.base_snapshot.clone() {
        Some(snapshot_id) => snapshot_id,
        None => store
            .latest_snapshot_id(&args.project)
            .map_err(|error| error.to_string())?
            .ok_or_else(|| format!("project {} has no snapshot to refresh from", args.project))?,
    };
    if store
        .snapshot_payload(&args.project, &base_snapshot_id)
        .map_err(|error| error.to_string())?
        .is_none()
    {
        return Err(format!(
            "base snapshot {base_snapshot_id} is not registered for project {}",
            args.project
        ));
    }
    let run_id = args
        .run_id
        .clone()
        .unwrap_or_else(|| format!("run.{}.refresh.{}", args.project, now_unix()));
    let index = run_index_with(
        IndexArgs {
            workspace: args.workspace.clone(),
            state: args.state.clone(),
            project: args.project.clone(),
            max_files: args.max_files,
            index_helper: None,
            run_id: Some(run_id),
            include_dir: args.include_dir.clone(),
            tracked_only: args.tracked_only,
            include_untracked: args.include_untracked,
            exclude_dir: args.exclude_dir.clone(),
            max_file_size: args.max_file_size,
            max_depth: args.max_depth,
        },
        helper,
        runner,
    )?;
    let new_snapshot_id = index["snapshot_id"]
        .as_str()
        .ok_or_else(|| "index result has no snapshot_id".to_string())?
        .to_string();
    let store = Store::open(&args.state).map_err(|error| error.to_string())?;
    let diff = diff_snapshots(&store, &args.project, &base_snapshot_id, &new_snapshot_id)
        .map_err(|error| error.to_string())?;
    Ok(json!({
        "command": "refresh",
        "project_id": args.project,
        "base_snapshot_id": base_snapshot_id,
        "new_snapshot_id": new_snapshot_id,
        "state": diff["state"],
        "history": {
            "old_snapshot_retained": true,
            "source_mutation": "none",
        },
        "timings": {"total_ms": started.elapsed().as_millis() as u64},
        "index": index,
        "diff": diff,
    }))
}

/// Return an existing snapshot when the workspace is byte-clean against it.
/// `init` uses this to stay idempotent: re-indexing identical bytes would
/// re-import the same content-addressed snapshot under a new run id, which the
/// ledger rejects because the entity payload would differ.
pub(crate) fn existing_clean_index(
    state: &PathBuf,
    project: &str,
    workspace: &std::path::Path,
) -> Result<Option<Value>, String> {
    let store = Store::open(state).map_err(|error| error.to_string())?;
    let Some(snapshot_id) = store
        .latest_snapshot_id(project)
        .map_err(|error| error.to_string())?
    else {
        return Ok(None);
    };
    let drift = archmeld_core::drift::analyze_drift(&store, project, &snapshot_id, workspace, None)
        .map_err(|error| error.to_string())?;
    if drift["state"] != "clean" {
        return Ok(None);
    }
    let payload = store
        .snapshot_payload(project, &snapshot_id)
        .map_err(|error| error.to_string())?
        .ok_or_else(|| format!("snapshot {snapshot_id} is not registered"))?;
    let counts = store
        .published_counts(project, &snapshot_id)
        .map_err(|error| error.to_string())?;
    let files = payload["files"].as_array().map(Vec::len).unwrap_or(0);
    // A snapshot can exist after capture but before a successful import. Never
    // reuse an empty index for a non-empty workspace: it would hide the
    // failed import behind a "clean" freshness result.
    if counts.entities == 0 && files > 0 {
        return Ok(None);
    }
    Ok(Some(json!({
        "command": "index",
        "state": "reused",
        "index_reuse": true,
        "project_id": project,
        "snapshot_id": snapshot_id,
        "snapshot_files": files,
        "ledger": {
            "entities": counts.entities,
            "relations": counts.relations,
            "evidence": counts.evidence,
            "relation_evidence": counts.relation_evidence,
            "backend_mappings": counts.backend_mappings,
            "diagnostics": counts.diagnostics,
        },
        "bridge": {"coverage": payload["coverage"]},
    })))
}

/// The ledger's `parse` block. Export schema 1.2 carries the indexer's own
/// counters, including parser-error paths; older helpers only print a summary
/// on stdout, so fall back to it field by field.
pub(crate) fn parse_block(index: &Value, helper_summary: &Value) -> Value {
    fn pick(primary: &Value, fallback: &Value, key: &str) -> Value {
        primary
            .get(key)
            .filter(|value| !value.is_null())
            .cloned()
            .unwrap_or_else(|| fallback.get(key).cloned().unwrap_or(Value::Null))
    }
    if index.is_object() {
        json!({
            "files_total": pick(index, helper_summary, "files_total"),
            "files_parsed": pick(index, helper_summary, "files_parsed"),
            "files_skipped": pick(index, helper_summary, "files_skipped"),
            "by_language": pick(index, helper_summary, "by_language"),
            "skipped_by_reason": pick(index, helper_summary, "skipped_by_reason"),
            "parser_errors": pick(index, helper_summary, "parser_errors"),
            "max_files_reached": pick(index, helper_summary, "max_files_reached"),
            "max_depth_reached": pick(index, helper_summary, "max_depth_reached"),
            "cross_file_implements": pick(index, helper_summary, "cross_file_implements"),
        })
    } else {
        json!({
            "files_total": helper_summary["files_total"],
            "files_parsed": helper_summary["files_parsed"],
            "files_skipped": helper_summary["files_skipped"],
            "by_language": helper_summary["by_language"],
            "skipped_by_reason": helper_summary["skipped_by_reason"],
            "parser_errors": helper_summary["parser_error_details"],
            "max_files_reached": helper_summary["max_files_reached"],
            "max_depth_reached": helper_summary["max_depth_reached"],
        })
    }
}

pub(crate) fn run_index_with(
    args: IndexArgs,
    helper: &PathBuf,
    runner: &mut HelperRunner<'_>,
) -> Result<Value, String> {
    let total_started = Instant::now();
    let workspace = args
        .workspace
        .canonicalize()
        .map_err(|error| format!("workspace {}: {error}", args.workspace.display()))?;
    if !workspace.is_dir() {
        return Err(format!(
            "workspace {} is not a directory",
            workspace.display()
        ));
    }
    fs::create_dir_all(&args.state)
        .map_err(|error| format!("state {}: {error}", args.state.display()))?;
    let run_id = args
        .run_id
        .clone()
        .unwrap_or_else(|| format!("run.{}.index.{}", args.project, now_unix()));
    let export = args
        .state
        .join("index-runs")
        .join(format!("{run_id}.ndjson"));
    if let Some(parent) = export.parent() {
        fs::create_dir_all(parent).map_err(|error| error.to_string())?;
    }
    let mut store = Store::open(&args.state).map_err(|error| error.to_string())?;
    store
        .ensure_project(&args.project, "expedition")
        .map_err(|error| error.to_string())?;
    // The MCP status/review tools resolve the project's active workspace root.
    // Indexing must therefore register the same root; otherwise a real
    // `archmeld index` followed by `archmeld-mcp` reports freshness=unknown and
    // review fails with "no active workspace root".
    archmeld_core::manager::register_project_root(
        &mut store,
        &args.project,
        &workspace,
        &args.state,
    )
    .map_err(|error| error.to_string())?;

    // Capture first, parse second. The helper reads a materialization of the
    // captured CAS blobs, so a save in the live workspace cannot produce a
    // graph that is bound to different bytes.
    let capture_started = Instant::now();
    let captured = capture_snapshot(
        &mut store,
        &args.project,
        &workspace,
        &SnapshotInputs {
            build_config: json!({
                "kind": "native",
                "variant": "default",
                "producer": "codegraph-export-v1",
                "helper": "archmeld-index",
                "product_build": concat!(env!("CARGO_PKG_VERSION"), "+", env!("ARCHMELD_GIT_SHORT")),
                "product_dirty": env!("ARCHMELD_GIT_DIRTY"),
                "analysis_input": "snapshot_materialization",
            }),
            environment: json!({
                "os": std::env::consts::OS,
                "arch": std::env::consts::ARCH,
                "adapter": "codegraph",
                "capture_order": "snapshot_then_parse",
            }),
        },
        &SnapshotOptions {
            include_untracked: !args.tracked_only,
            ..SnapshotOptions::default()
        },
    )
    .map_err(|error| error.to_string())?;
    let capture_ms = capture_started.elapsed().as_millis() as u64;
    let config_digest = captured.snapshot["config_digest"]
        .as_str()
        .ok_or_else(|| "snapshot has no config_digest".to_string())?
        .to_string();
    let project_scope = sanitize_id(&args.project);
    // Materialize into a temporary sibling and swap it into a stable
    // per-project path. IndexState keys are absolute paths, so a fresh random
    // directory per run would never hit the unchanged-file cache.
    let materialized = materialize_all(
        &mut store,
        &args.project,
        &captured.snapshot_id,
        &["index-work-tmp", project_scope.as_str()],
    )
    .map_err(|error| {
        format!(
            "materialize snapshot {} for indexing: {error}",
            captured.snapshot_id
        )
    })?;
    let stable_root = args.state.join("index-work").join(&project_scope);
    if let Some(parent) = stable_root.parent() {
        fs::create_dir_all(parent).map_err(|error| error.to_string())?;
    }
    if stable_root.exists() {
        fs::remove_dir_all(&stable_root)
            .map_err(|error| format!("reset {}: {error}", stable_root.display()))?;
    }
    if let Err(error) = fs::rename(&materialized.root, &stable_root) {
        let _ = fs::remove_dir_all(&materialized.root);
        return Err(format!(
            "swap stable materialization {}: {error}",
            stable_root.display()
        ));
    }
    let analysis = json!({
        "mode": "snapshot_materialization",
        "input": stable_root.to_string_lossy(),
        "temporary": materialized.detail,
        "stable": true,
    });
    let analysis_root = stable_root.clone();
    let analysis_input = stable_root.clone();

    let started = SystemTime::now();
    let output = runner(helper, &analysis_input, &export, &args.state)?;
    let elapsed_ms = started.elapsed().map(|d| d.as_millis()).unwrap_or(0);
    let stdout = String::from_utf8_lossy(&output.stdout).trim().to_string();
    let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
    if !output.success {
        let _ = fs::remove_dir_all(&analysis_root);
        return Err(format!(
            "archmeld-index ({}) exited {:?} ({}): {}; snapshot {} was captured but not indexed",
            helper.display(),
            output.status,
            helper_exit_hint(output.status),
            if stderr.is_empty() { &stdout } else { &stderr },
            captured.snapshot_id
        ));
    }
    let helper_summary: Value = match serde_json::from_str(&stdout) {
        Ok(summary) => summary,
        Err(error) => {
            let _ = fs::remove_dir_all(&analysis_root);
            return Err(format!("index helper returned non-JSON: {error}: {stdout}"));
        }
    };
    let export_bytes = match fs::read(&export) {
        Ok(bytes) => bytes,
        Err(error) => {
            let _ = fs::remove_dir_all(&analysis_root);
            return Err(error.to_string());
        }
    };
    let export_digest = hash::digest_bytes(&export_bytes);
    let export_text = match String::from_utf8(export_bytes) {
        Ok(text) => text,
        Err(_) => {
            let _ = fs::remove_dir_all(&analysis_root);
            return Err("archmeld-index wrote non-UTF-8 NDJSON".to_string());
        }
    };
    let import_started = Instant::now();
    let bridge = import_codegraph_export(
        &mut store,
        &BridgeConfig {
            project_id: &args.project,
            snapshot_id: &captured.snapshot_id,
            config_digest: &config_digest,
            run_id: &run_id,
            workspace_root: Some(&analysis_input),
            max_entities_per_batch: 500,
            max_relations_per_batch: 2000,
        },
        &captured.files,
        &export_text,
    );
    let analysis_removed = fs::remove_dir_all(&analysis_root).is_ok();
    let bridge = bridge.map_err(|error| {
        format!(
            "import export for snapshot {}: {error}",
            captured.snapshot_id
        )
    })?;
    let import_ms = import_started.elapsed().as_millis() as u64;
    let counts = store
        .published_counts(&args.project, &captured.snapshot_id)
        .map_err(|error| error.to_string())?;
    let index_run = json!({
        "project_id": args.project.clone(),
        "snapshot_id": captured.snapshot_id.clone(),
        "run_id": run_id.clone(),
        "cli_commit": env!("ARCHMELD_GIT_SHORT"),
        "cli_dirty": env!("ARCHMELD_GIT_DIRTY") == "true",
        "helper_commit": bridge.producer_commit.clone(),
        "helper_dirty": bridge.producer_dirty,
        "export_schema": bridge.export_schema_version.clone(),
        "index_config": {
            "max_files": args.max_files,
            "include_dirs": args.include_dir.clone(),
            "exclude_dirs": args.exclude_dir.clone(),
            "max_file_size": args.max_file_size,
            "max_depth": args.max_depth,
            "helper": helper,
        },
        "parse": parse_block(&bridge.index, &helper_summary),
        "index_mode": helper_summary["index_mode"],
        "files_reused": helper_summary["files_reused"],
        "files_deleted": helper_summary["files_deleted"],
        "transport": {
            "nodes_expected": bridge.coverage["expected_count"],
            "nodes_observed": bridge.coverage["observed_count"],
            "edges_exported": helper_summary["edges"],
            "relations_imported": bridge.relations,
            "collapsed_edges": bridge.collapsed_edges,
            "relations_with_call_site": bridge.relations_with_call_site,
            "relations_without_call_site": bridge.relations_without_call_site,
            "unresolved": bridge.unresolved,
        },
        "coverage": bridge.coverage.clone(),
    });
    store
        .insert_index_run(&index_run)
        .map_err(|error| format!("record index run: {error}"))?;
    let index_mode = helper_summary["index_mode"]
        .as_str()
        .unwrap_or("full_parse")
        .to_string();
    let files_reused = helper_summary["files_reused"].as_u64().unwrap_or(0);
    let files_deleted = helper_summary["files_deleted"].as_u64().unwrap_or(0);
    Ok(json!({
        "command": "index",
        "project_id": args.project,
        "snapshot_id": captured.snapshot_id,
        "config_digest": config_digest,
        "run_id": run_id,
        "workspace": workspace,
        "state": args.state,
        "analysis": {
            "mode": "snapshot_materialization",
            "input": analysis,
            "removed": analysis_removed,
            "stable_root": stable_root,
        },
        "export": export,
        "export_digest": export_digest,
        "helper": helper,
        "helper_elapsed_ms": elapsed_ms,
        "index_mode": index_mode,
        "files_reused": files_reused,
        "files_deleted": files_deleted,
        "timings": {
            "capture_ms": capture_ms,
            "helper_ms": elapsed_ms,
            "import_ms": import_ms,
            "total_ms": total_started.elapsed().as_millis() as u64,
        },
        "helper_summary": helper_summary,
        "snapshot_files": captured.files.len(),
        "bridge": {
            "batches": bridge.batches,
            "entities": bridge.entities,
            "relations": bridge.relations,
            "diagnostics": bridge.diagnostics,
            "skipped_nodes": bridge.skipped_nodes,
            "merged_nodes": bridge.merged_nodes,
            "collapsed_edges": bridge.collapsed_edges,
            "relations_with_call_site": bridge.relations_with_call_site,
            "relations_without_call_site": bridge.relations_without_call_site,
            "unresolved": bridge.unresolved,
            "coverage": bridge.coverage,
            "index": bridge.index,
            "export_schema_version": bridge.export_schema_version,
            "producer_commit": bridge.producer_commit,
            "producer_dirty": bridge.producer_dirty,
        },
        "ledger": {
            "entities": counts.entities,
            "evidence": counts.evidence,
            "relations": counts.relations,
            "relation_evidence": counts.relation_evidence,
            "backend_mappings": counts.backend_mappings,
        },
    }))
}
