// Copyright 2026 ARCHMELD contributors
// SPDX-License-Identifier: Apache-2.0

//! Offline operator CLI: index a workspace, export a review bundle, or verify
//! an existing bundle. No model provider, network or server is required.

use archmeld_core::bridge::{import_codegraph_export, BridgeConfig};
use archmeld_core::encrypted_backup::{
    create_encrypted_backup, restore_encrypted_backup, retain_encrypted_backup,
    rotate_encrypted_backup, verify_encrypted_backup,
};
use archmeld_core::hash;
use archmeld_core::evaluate_numeric_tolerance_artifacts;
use archmeld_core::sandbox::{materialize_all, sanitize_id};
use archmeld_core::migrate::migrate_ledger;
use archmeld_core::perfetto::perfetto_trace;
use archmeld_core::report::{export_report, verify_bundle};
use archmeld_core::review::{diff_snapshots, review_workspace_diff};
use archmeld_core::runtime::{import_trace, RuntimeImport};
use archmeld_core::scenario::{run_scenario, ExecutionGrant, ScenarioSandbox, ScenarioSpec};
use archmeld_core::status::status_report;
use archmeld_core::trace_adapters::{chrome_trace, probe_jsonl, TraceLocator, TraceMeta};
use archmeld_core::trace_processor::perfetto_trace_via_processor;
use archmeld_core::tracy_csv::import_tracy_capture;
use archmeld_core::watch::{watch_drift, watch_drift_notify, NotifyWatchOptions, WatchOptions};
use archmeld_core::snapshot::{capture_snapshot, SnapshotInputs, SnapshotOptions};
use archmeld_core::store::Store;
use clap::{Parser, Subcommand};
use serde_json::{json, Value};

mod memory_limit;
use std::fs;
use std::path::PathBuf;
use std::process::{Command, ExitCode};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

#[derive(Parser)]
#[command(name = "archmeld", version, about = "Offline code evidence CLI")]
struct Cli {
    #[command(subcommand)]
    command: Commands,
}

#[derive(Subcommand)]
enum Commands {
    /// Index a workspace into a local evidence ledger.
    Index(IndexArgs),
    /// Report projects, latest snapshots, counts, languages and optional
    /// workspace freshness without mutating source or history.
    Status(StatusArgs),
    /// Review the current workspace against a frozen snapshot. Static only:
    /// no build, test or runtime command is executed.
    ReviewDiff(ReviewDiffArgs),
    /// Index the current workspace as a new snapshot and report the exact diff
    /// from the previous snapshot. The old snapshot and evidence are retained.
    Refresh(RefreshArgs),
    /// Export a checksummed offline report bundle for one snapshot.
    Report(ReportArgs),
    /// Verify every file in an existing report bundle.
    VerifyReport(VerifyArgs),
    /// Create a passphrase-encrypted backup of the ledger and CAS objects.
    Backup(BackupArgs),
    /// Verify an encrypted backup bundle.
    VerifyBackup(VerifyBackupArgs),
    /// Restore an encrypted backup into a new state directory.
    RestoreBackup(RestoreBackupArgs),
    /// Re-encrypt an encrypted backup under a new passphrase.
    RotateBackup(RotateBackupArgs),
    /// Copy a verified encrypted backup to a path-based retention target.
    RetainBackup(RetainBackupArgs),
    /// Run one scenario command against an existing snapshot.
    Scenario(ScenarioArgs),
    /// Poll a workspace for drift against a base snapshot.
    Watch(WatchArgs),
    /// Import a native runtime trace into an existing snapshot.
    TraceImport(TraceImportArgs),
    /// Copy a complete ledger into a new, empty state directory with integrity
    /// checks. Merging into an existing ledger is refused.
    LedgerMigrate(MigrateArgs),
    /// Evaluate a numeric_tolerance predicate against artifacts without
    /// creating a verification or accepting a ChangeSet.
    NumericCheck(NumericCheckArgs),
}

#[derive(clap::Args)]
struct NumericCheckArgs {
    /// Ledger used to store the exact artifact bytes.
    #[arg(long)]
    state: PathBuf,
    /// Artifact root; predicate paths resolve inside it.
    #[arg(long, default_value = ".")]
    root: PathBuf,
    /// JSON file containing the numeric_tolerance predicate.
    #[arg(long)]
    predicate: PathBuf,
}

#[derive(clap::Args)]
struct IndexArgs {
    #[arg(long)]
    workspace: PathBuf,
    #[arg(long)]
    state: PathBuf,
    #[arg(long, default_value = "project.local")]
    project: String,
    #[arg(long, default_value_t = 5000)]
    max_files: usize,
    /// Override the packaged codegraph helper path (defaults to a sibling
    /// `archmeld-index` executable).
    #[arg(long)]
    index_helper: Option<PathBuf>,
    #[arg(long)]
    run_id: Option<String>,
}

#[derive(clap::Args)]
struct StatusArgs {
    #[arg(long)]
    state: PathBuf,
    /// Restrict the report to one project; otherwise every registered project
    /// is listed.
    #[arg(long)]
    project: Option<String>,
    /// Optional workspace to compare with the latest snapshot. This only reads
    /// the workspace; it never creates a snapshot or records a drift report.
    #[arg(long)]
    workspace: Option<PathBuf>,
}

#[derive(clap::Args)]
struct ReviewDiffArgs {
    #[arg(long)]
    state: PathBuf,
    #[arg(long)]
    project: String,
    #[arg(long)]
    workspace: PathBuf,
    /// Base snapshot; defaults to the project's latest snapshot.
    #[arg(long)]
    base_snapshot: Option<String>,
}

#[derive(clap::Args)]
struct RefreshArgs {
    #[arg(long)]
    state: PathBuf,
    #[arg(long)]
    project: String,
    #[arg(long)]
    workspace: PathBuf,
    /// Base snapshot to diff from; defaults to the latest snapshot.
    #[arg(long)]
    base_snapshot: Option<String>,
    #[arg(long, default_value_t = 5000)]
    max_files: usize,
    #[arg(long)]
    index_helper: Option<PathBuf>,
    #[arg(long)]
    run_id: Option<String>,
}

#[derive(clap::Args)]
struct ReportArgs {
    #[arg(long)]
    state: PathBuf,
    #[arg(long)]
    project: String,
    #[arg(long)]
    snapshot: String,
    #[arg(long)]
    out: PathBuf,
    #[arg(long, default_value_t = 5000)]
    max_entities: u32,
    #[arg(long, default_value_t = 20000)]
    max_relations: u32,
    /// Opt in to copying snapshot source bytes into the bundle. The bundle is
    /// then sensitive and the manifest says so.
    #[arg(long)]
    include_source: bool,
    /// Opt in to copying runtime/native trace artifacts into the bundle.
    #[arg(long)]
    include_raw_traces: bool,
}

#[derive(clap::Args)]
struct VerifyArgs {
    #[arg(long)]
    dir: PathBuf,
}

#[derive(clap::Args)]
struct BackupArgs {
    #[arg(long)]
    state: PathBuf,
    #[arg(long, default_value = "project.local")]
    project: String,
    #[arg(long)]
    out: PathBuf,
    /// Environment variable holding the passphrase. The passphrase is never
    /// read from argv because process lists are observable.
    #[arg(long, default_value = "ARCHMELD_BACKUP_PASSPHRASE")]
    passphrase_env: String,
}

#[derive(clap::Args)]
struct VerifyBackupArgs {
    #[arg(long)]
    dir: PathBuf,
    #[arg(long, default_value = "ARCHMELD_BACKUP_PASSPHRASE")]
    passphrase_env: String,
}

#[derive(clap::Args)]
struct RestoreBackupArgs {
    #[arg(long)]
    dir: PathBuf,
    #[arg(long)]
    target: PathBuf,
    #[arg(long, default_value = "ARCHMELD_BACKUP_PASSPHRASE")]
    passphrase_env: String,
}

#[derive(clap::Args)]
struct RotateBackupArgs {
    #[arg(long)]
    dir: PathBuf,
    #[arg(long)]
    out: PathBuf,
    #[arg(long, default_value = "ARCHMELD_BACKUP_PASSPHRASE")]
    old_passphrase_env: String,
    #[arg(long, default_value = "ARCHMELD_BACKUP_NEW_PASSPHRASE")]
    new_passphrase_env: String,
}

#[derive(clap::Args)]
struct RetainBackupArgs {
    #[arg(long)]
    dir: PathBuf,
    /// Directory on any mounted filesystem (including a network mount).
    #[arg(long)]
    target: PathBuf,
    #[arg(long)]
    retention_id: String,
    #[arg(long, default_value = "ARCHMELD_BACKUP_PASSPHRASE")]
    passphrase_env: String,
}

#[derive(clap::Args)]
struct ScenarioArgs {
    #[arg(long)]
    state: PathBuf,
    #[arg(long)]
    project: String,
    #[arg(long)]
    snapshot: String,
    /// Authorized workspace root. cwd must resolve inside it.
    #[arg(long)]
    workspace: PathBuf,
    #[arg(long)]
    cwd: Option<PathBuf>,
    #[arg(long, default_value_t = 60_000)]
    timeout_ms: u64,
    #[arg(long)]
    grant_id: String,
    #[arg(long)]
    reason: String,
    #[arg(long)]
    run_id: Option<String>,
    /// `workspace` (default), `workspace_copy` or `container`.
    #[arg(long, value_parser = ["workspace", "workspace_copy", "container"])]
    sandbox_mode: Option<String>,
    /// Repeatable. Files or directory prefixes copied from the snapshot.
    #[arg(long)]
    sandbox_include: Vec<String>,
    /// Repeatable. Prefixes excluded from the snapshot copy.
    #[arg(long)]
    sandbox_exclude: Vec<String>,
    /// Digest-pinned local image for `container` mode.
    #[arg(long)]
    sandbox_image: Option<String>,
    /// Only `none` is accepted; omitted also means no network.
    #[arg(long)]
    sandbox_network: Option<String>,
    /// Relative cwd inside the snapshot copy. Defaults to the workspace-relative
    /// `--cwd` (or the snapshot root).
    #[arg(long)]
    sandbox_cwd: Option<String>,
    /// Command and arguments after `--`.
    #[arg(trailing_var_arg = true, required = true)]
    argv: Vec<String>,
}

#[derive(clap::Args)]
struct WatchArgs {
    #[arg(long)]
    state: PathBuf,
    #[arg(long)]
    project: String,
    #[arg(long)]
    snapshot: String,
    #[arg(long)]
    workspace: PathBuf,
    #[arg(long, default_value_t = 1000)]
    interval_ms: u64,
    /// Stop after N ticks; omit to run until the process is terminated.
    #[arg(long)]
    iterations: Option<usize>,
    /// `polling` compares hashes on an interval; `os` waits for real file
    /// notifications and re-hashes after a quiet period.
    #[arg(long, default_value = "polling", value_parser = ["polling", "os"])]
    mode: String,
    #[arg(long, default_value_t = 200)]
    debounce_ms: u64,
}

#[derive(clap::Args)]
struct MigrateArgs {
    /// Existing complete ledger to copy. It is opened read-only.
    #[arg(long)]
    from: PathBuf,
    /// Destination state directory. It must not exist.
    #[arg(long)]
    to: PathBuf,
    /// Operator justification recorded in migration.json.
    #[arg(long)]
    reason: String,
}

#[derive(clap::Args)]
struct TraceImportArgs {
    #[arg(long)]
    state: PathBuf,
    #[arg(long)]
    project: String,
    #[arg(long)]
    run_id: String,
    /// Snapshot the trace is bound to. Without it the trace is unbound and
    /// `--allow-unbound` is required.
    #[arg(long)]
    snapshot: Option<String>,
    /// Override is rejected when it disagrees with the snapshot build.
    #[arg(long)]
    config_digest: Option<String>,
    /// Existing scenario run to associate with this trace. The run must
    /// belong to the project and match the trace snapshot/build config.
    #[arg(long)]
    scenario_run_id: Option<String>,
    /// JSON file with the trace build identity; defaults to the snapshot
    /// manifest when bound.
    #[arg(long)]
    build_identity: Option<PathBuf>,
    #[arg(long)]
    allow_unbound: bool,
    #[arg(long, value_parser = ["perfetto", "tracy", "probe", "chrome", "normalized"])]
    format: String,
    /// Perfetto `.pftrace`, probe JSONL or Chrome/VizTracer JSON.
    #[arg(long)]
    input: Option<PathBuf>,
    /// Optional `trace_processor_shell`; when set, Perfetto imports query the
    /// trace with explicit SQL instead of the in-repo protobuf decoder.
    #[arg(long)]
    trace_processor: Option<PathBuf>,
    /// Native `.tracy` capture for `--format tracy`.
    #[arg(long)]
    tracy_capture: Option<PathBuf>,
    /// `tracy-csvexport -u -p` output for `--format tracy`.
    #[arg(long)]
    tracy_csv: Option<PathBuf>,
    /// JSON `{"locations":[{"path","line","entity_id"}],"symbols":{...}}`.
    #[arg(long)]
    locator: Option<PathBuf>,
    #[arg(long, default_value = "archmeld-cli")]
    tool: String,
    #[arg(long, default_value = "1")]
    tool_version: String,
    #[arg(long)]
    out: Option<PathBuf>,
    /// Hard process memory ceiling for the import, enforced by the OS before
    /// the trace is read.
    #[arg(long)]
    memory_limit_mb: Option<u64>,
}

fn passphrase(variable: &str) -> Result<String, String> {
    let value = std::env::var(variable)
        .map_err(|_| format!("passphrase environment variable {variable} is not set"))?;
    if value.is_empty() {
        return Err(format!("passphrase environment variable {variable} is empty"));
    }
    Ok(value)
}

fn now_unix() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_secs())
        .unwrap_or(0)
}

fn helper_path(explicit: Option<PathBuf>) -> Result<PathBuf, String> {
    if let Some(path) = explicit {
        return Ok(path);
    }
    if let Ok(path) = std::env::var("ARCHMELD_INDEX_HELPER") {
        return Ok(PathBuf::from(path));
    }
    let exe = std::env::current_exe().map_err(|error| error.to_string())?;
    let dir = exe
        .parent()
        .ok_or_else(|| "current executable has no parent directory".to_string())?;
    let candidates = if cfg!(windows) {
        vec![dir.join("archmeld-index.exe"), dir.join("archmeld_export.exe")]
    } else {
        vec![dir.join("archmeld-index"), dir.join("archmeld_export")]
    };
    candidates
        .into_iter()
        .find(|candidate| candidate.is_file())
        .ok_or_else(|| {
            "archmeld-index helper not found next to archmeld; pass --index-helper".to_string()
        })
}

struct HelperOutput {
    success: bool,
    status: Option<i32>,
    stdout: Vec<u8>,
    stderr: Vec<u8>,
}

type HelperRunner<'a> =
    dyn FnMut(&PathBuf, &std::path::Path, &std::path::Path, &std::path::Path) -> Result<HelperOutput, String>
        + 'a;

fn run_status(args: StatusArgs) -> Result<Value, String> {
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

fn helper_runner(
    state_root: PathBuf,
    max_files: usize,
) -> impl FnMut(&PathBuf, &std::path::Path, &std::path::Path, &std::path::Path) -> Result<HelperOutput, String> {
    move |helper, analysis_input, export, _state| {
        let output = Command::new(helper)
            .arg(analysis_input)
            .arg(export)
            .arg(&state_root)
            .arg(max_files.to_string())
            .output()
            .map_err(|error| format!("run {}: {error}", helper.display()))?;
        Ok(HelperOutput {
            success: output.status.success(),
            status: output.status.code(),
            stdout: output.stdout,
            stderr: output.stderr,
        })
    }
}

fn run_index(args: IndexArgs) -> Result<Value, String> {
    let helper = helper_path(args.index_helper.clone())?;
    let mut runner = helper_runner(args.state.join("codegraph"), args.max_files);
    run_index_with(args, &helper, &mut runner)
}

fn run_refresh(args: RefreshArgs) -> Result<Value, String> {
    let helper = helper_path(args.index_helper.clone())?;
    let mut runner = helper_runner(args.state.join("codegraph"), args.max_files);
    run_refresh_with(args, &helper, &mut runner)
}

fn run_refresh_with(
    args: RefreshArgs,
    helper: &PathBuf,
    runner: &mut HelperRunner<'_>,
) -> Result<Value, String> {
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
        "index": index,
        "diff": diff,
    }))
}

fn run_review_diff(args: ReviewDiffArgs) -> Result<Value, String> {
    let store = Store::open(&args.state).map_err(|error| error.to_string())?;
    let base_snapshot_id = match args.base_snapshot {
        Some(snapshot_id) => snapshot_id,
        None => store
            .latest_snapshot_id(&args.project)
            .map_err(|error| error.to_string())?
            .ok_or_else(|| format!("project {} has no snapshot to review against", args.project))?,
    };
    let workspace = args
        .workspace
        .canonicalize()
        .map_err(|error| format!("workspace {}: {error}", args.workspace.display()))?;
    let mut result = review_workspace_diff(&store, &args.project, &base_snapshot_id, &workspace)
        .map_err(|error| error.to_string())?;
    result["command"] = json!("review-diff");
    Ok(result)
}

fn run_index_with(
    args: IndexArgs,
    helper: &PathBuf,
    runner: &mut HelperRunner<'_>,
) -> Result<Value, String> {
    let workspace = args
        .workspace
        .canonicalize()
        .map_err(|error| format!("workspace {}: {error}", args.workspace.display()))?;
    if !workspace.is_dir() {
        return Err(format!("workspace {} is not a directory", workspace.display()));
    }
    fs::create_dir_all(&args.state).map_err(|error| format!("state {}: {error}", args.state.display()))?;
    let run_id = args
        .run_id
        .clone()
        .unwrap_or_else(|| format!("run.{}.index.{}", args.project, now_unix()));
    let export = args.state.join("index-runs").join(format!("{run_id}.ndjson"));
    if let Some(parent) = export.parent() {
        fs::create_dir_all(parent).map_err(|error| error.to_string())?;
    }
    let mut store = Store::open(&args.state).map_err(|error| error.to_string())?;
    store
        .ensure_project(&args.project, "expedition")
        .map_err(|error| error.to_string())?;

    // Capture first, parse second. The helper reads a materialization of the
    // captured CAS blobs, so a save in the live workspace cannot produce a
    // graph that is bound to different bytes.
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
            include_untracked: true,
            ..SnapshotOptions::default()
        },
    )
    .map_err(|error| error.to_string())?;
    let config_digest = captured.snapshot["config_digest"]
        .as_str()
        .ok_or_else(|| "snapshot has no config_digest".to_string())?
        .to_string();
    let project_scope = sanitize_id(&args.project);
    let run_scope = sanitize_id(&run_id);
    let materialized = materialize_all(
        &mut store,
        &args.project,
        &captured.snapshot_id,
        &["index-sandboxes", project_scope.as_str(), run_scope.as_str()],
    )
    .map_err(|error| {
        format!(
            "materialize snapshot {} for indexing: {error}",
            captured.snapshot_id
        )
    })?;
    let analysis = materialized.detail.clone();
    let analysis_root = materialized.root.clone();
    let analysis_input = materialized.cwd.clone();

    let started = SystemTime::now();
    let output = runner(helper, &analysis_input, &export, &args.state)?;
    let elapsed_ms = started.elapsed().map(|d| d.as_millis()).unwrap_or(0);
    let stdout = String::from_utf8_lossy(&output.stdout).trim().to_string();
    let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
    if !output.success {
        let _ = fs::remove_dir_all(&analysis_root);
        return Err(format!(
            "archmeld-index exited {:?}: {}; snapshot {} was captured but not indexed",
            output.status,
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
    let counts = store
        .published_counts(&args.project, &captured.snapshot_id)
        .map_err(|error| error.to_string())?;
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
        },
        "export": export,
        "export_digest": export_digest,
        "helper": helper,
        "helper_elapsed_ms": elapsed_ms,
        "helper_summary": helper_summary,
        "snapshot_files": captured.files.len(),
        "bridge": {
            "batches": bridge.batches,
            "entities": bridge.entities,
            "relations": bridge.relations,
            "diagnostics": bridge.diagnostics,
            "skipped_nodes": bridge.skipped_nodes,
            "unresolved": bridge.unresolved,
            "coverage": bridge.coverage,
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

fn run_report(args: ReportArgs) -> Result<Value, String> {
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

fn run_watch(args: WatchArgs) -> Result<Value, String> {
    let workspace = args
        .workspace
        .canonicalize()
        .map_err(|error| format!("workspace {}: {error}", args.workspace.display()))?;
    let mut store = Store::open(&args.state).map_err(|error| error.to_string())?;
    let base = WatchOptions {
        project_id: args.project.clone(),
        base_snapshot_id: args.snapshot.clone(),
        root: workspace,
        interval: Duration::from_millis(args.interval_ms.max(10)),
        iterations: args.iterations,
        stop: None,
    };
    let result = if args.mode == "os" {
        watch_drift_notify(
            &mut store,
            &NotifyWatchOptions {
                base,
                debounce: Duration::from_millis(args.debounce_ms.max(10)),
            },
        )
    } else {
        watch_drift(&mut store, &base)
    }
    .map_err(|error| error.to_string())?;
    Ok(json!({
        "command": "watch",
        "mode": args.mode,
        "project_id": args.project,
        "snapshot_id": args.snapshot,
        "result": result,
    }))
}

fn run_scenario_command(args: ScenarioArgs) -> Result<Value, String> {
    let workspace = args
        .workspace
        .canonicalize()
        .map_err(|error| format!("workspace {}: {error}", args.workspace.display()))?;
    let sandbox_mode = args
        .sandbox_mode
        .clone()
        .unwrap_or_else(|| "workspace".to_string());
    let declares_sandbox = args.sandbox_mode.is_some()
        || !args.sandbox_include.is_empty()
        || !args.sandbox_exclude.is_empty()
        || args.sandbox_image.is_some()
        || args.sandbox_network.is_some()
        || args.sandbox_cwd.is_some();
    let sandbox_cwd = if sandbox_mode == "workspace" {
        None
    } else if let Some(relative) = &args.sandbox_cwd {
        if std::path::Path::new(relative).is_absolute() {
            return Err("--sandbox-cwd must be relative to the snapshot root".into());
        }
        Some(relative.clone())
    } else {
        let requested = args
            .cwd
            .as_ref()
            .map(|path| {
                if path.is_absolute() {
                    path.clone()
                } else {
                    workspace.join(path)
                }
            })
            .unwrap_or_else(|| workspace.clone());
        let canonical = requested
            .canonicalize()
            .map_err(|error| format!("cwd: {error}"))?;
        if !canonical.starts_with(&workspace) {
            return Err("scenario cwd resolves outside the authorized workspace".into());
        }
        let relative = canonical
            .strip_prefix(&workspace)
            .map_err(|_| "scenario cwd is outside the authorized workspace".to_string())?
            .to_string_lossy()
            .replace('\\', "/");
        if relative.is_empty() {
            None
        } else {
            Some(relative)
        }
    };
    let cwd = if sandbox_mode == "workspace" {
        let cwd = args
            .cwd
            .unwrap_or_else(|| workspace.clone())
            .canonicalize()
            .map_err(|error| format!("cwd: {error}"))?;
        if !cwd.starts_with(&workspace) {
            return Err("scenario cwd resolves outside the authorized workspace".into());
        }
        cwd
    } else {
        workspace.clone()
    };
    if args.argv.is_empty() || args.argv[0].is_empty() {
        return Err("scenario argv must not be empty".into());
    }
    let mut store = Store::open(&args.state).map_err(|error| error.to_string())?;
    let snapshot_payload = store
        .snapshot_payload(&args.project, &args.snapshot)
        .map_err(|error| error.to_string())?
        .ok_or_else(|| format!("snapshot {} not found", args.snapshot))?;
    let config_digest = snapshot_payload["config_digest"]
        .as_str()
        .ok_or_else(|| "snapshot has no config_digest".to_string())?
        .to_string();
    let run_id = args
        .run_id
        .unwrap_or_else(|| format!("run.{}.scenario.{}", args.project, now_unix()));
    let spec = ScenarioSpec {
        project_id: args.project.clone(),
        run_id: run_id.clone(),
        snapshot_id: args.snapshot.clone(),
        config_digest,
        argv: args.argv.clone(),
        cwd,
        env: Vec::new(),
        timeout: Duration::from_millis(args.timeout_ms),
        sandbox: if declares_sandbox {
            Some(ScenarioSandbox {
                mode: sandbox_mode,
                include: args.sandbox_include.clone(),
                exclude: args.sandbox_exclude.clone(),
                image: args.sandbox_image.clone(),
                network: args.sandbox_network.clone(),
                cwd: sandbox_cwd,
            })
        } else {
            None
        },
    };
    let grant = ExecutionGrant::grant(&args.grant_id, &args.reason);
    let result = run_scenario(&mut store, &spec, &grant).map_err(|error| error.to_string())?;
    Ok(json!({
        "command": "scenario",
        "project_id": args.project,
        "snapshot_id": args.snapshot,
        "run_id": run_id,
        "result": result,
    }))
}

fn run_numeric_check(args: NumericCheckArgs) -> Result<Value, String> {
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

fn run_migrate(args: MigrateArgs) -> Result<Value, String> {
    let receipt = migrate_ledger(&args.from, &args.to, &args.reason)
        .map_err(|error| error.to_string())?;
    Ok(json!({
        "command": "ledger-migrate",
        "from": args.from,
        "to": args.to,
        "receipt": receipt,
    }))
}

fn run_backup(args: BackupArgs) -> Result<Value, String> {
    let secret = passphrase(&args.passphrase_env)?;
    let mut store = Store::open(&args.state).map_err(|error| error.to_string())?;
    let result = create_encrypted_backup(&mut store, &args.project, &args.out, &secret)
        .map_err(|error| error.to_string())?;
    Ok(json!({"command": "backup", "project_id": args.project, "result": result}))
}

fn run_verify_backup(args: VerifyBackupArgs) -> Result<Value, String> {
    let secret = passphrase(&args.passphrase_env)?;
    let result = verify_encrypted_backup(&args.dir, &secret).map_err(|error| error.to_string())?;
    Ok(json!({"command": "verify-backup", "dir": args.dir, "verified": result}))
}

fn run_restore_backup(args: RestoreBackupArgs) -> Result<Value, String> {
    let secret = passphrase(&args.passphrase_env)?;
    let result = restore_encrypted_backup(&args.dir, &secret, &args.target)
        .map_err(|error| error.to_string())?;
    Ok(json!({"command": "restore-backup", "target": args.target, "restored": result}))
}

fn run_rotate_backup(args: RotateBackupArgs) -> Result<Value, String> {
    let old_secret = passphrase(&args.old_passphrase_env)?;
    let new_secret = passphrase(&args.new_passphrase_env)?;
    let result = rotate_encrypted_backup(&args.dir, &old_secret, &new_secret, &args.out)
        .map_err(|error| error.to_string())?;
    Ok(json!({"command": "rotate-backup", "dir": args.dir, "out": args.out, "rotated": result}))
}

fn run_retain_backup(args: RetainBackupArgs) -> Result<Value, String> {
    let secret = passphrase(&args.passphrase_env)?;
    let result = retain_encrypted_backup(&args.dir, &args.target, &args.retention_id, &secret)
        .map_err(|error| error.to_string())?;
    Ok(json!({
        "command": "retain-backup",
        "dir": args.dir,
        "target": args.target,
        "retention_id": args.retention_id,
        "retention": result,
    }))
}

fn run_verify(args: VerifyArgs) -> Result<Value, String> {
    let manifest_path = args.dir.join("manifest.json");
    let manifest: Value = serde_json::from_slice(
        &fs::read(&manifest_path)
            .map_err(|error| format!("{}: {error}", manifest_path.display()))?,
    )
    .map_err(|error| format!("{}: {error}", manifest_path.display()))?;
    let verified = verify_bundle(&args.dir, &manifest).map_err(|error| error.to_string())?;
    Ok(json!({"command": "verify-report", "dir": args.dir, "verified": verified}))
}

fn load_locator(path: Option<&PathBuf>) -> Result<TraceLocator, String> {
    let mut locator = TraceLocator::new();
    let Some(path) = path else {
        return Ok(locator);
    };
    let value: Value = serde_json::from_slice(
        &fs::read(path).map_err(|error| format!("{}: {error}", path.display()))?,
    )
    .map_err(|error| format!("{}: {error}", path.display()))?;
    if let Some(locations) = value["locations"].as_array() {
        for entry in locations {
            let entity_id = entry["entity_id"]
                .as_str()
                .ok_or_else(|| "locator location needs entity_id".to_string())?;
            let line = entry["line"]
                .as_u64()
                .ok_or_else(|| "locator location needs a numeric line".to_string())?;
            let path = entry["path"]
                .as_str()
                .ok_or_else(|| "locator location needs path".to_string())?;
            locator.add_location(path, line, entity_id);
        }
    }
    if let Some(symbols) = value["symbols"].as_object() {
        for (symbol, entity) in symbols {
            let entity = entity
                .as_str()
                .ok_or_else(|| "locator symbol mapping must be a string".to_string())?;
            locator.add_symbol(symbol, entity);
        }
    }
    Ok(locator)
}

fn run_trace_import(args: TraceImportArgs) -> Result<Value, String> {
    if let Some(limit) = args.memory_limit_mb {
        memory_limit::apply_limit_mb(limit)?;
    }
    let mut store = Store::open(&args.state).map_err(|error| error.to_string())?;
    let snapshot = match &args.snapshot {
        Some(snapshot_id) => Some(
            store
                .snapshot_payload(&args.project, snapshot_id)
                .map_err(|error| error.to_string())?
                .ok_or_else(|| format!("snapshot {snapshot_id} not found"))?,
        ),
        None => None,
    };
    if snapshot.is_none() && !args.allow_unbound {
        return Err("unbound trace import requires --allow-unbound".to_string());
    }
    let snapshot_config = snapshot
        .as_ref()
        .and_then(|value| value["config_digest"].as_str())
        .map(str::to_string);
    if let (Some(requested), Some(actual)) = (&args.config_digest, &snapshot_config) {
        if requested != actual {
            return Err(format!(
                "--config-digest {requested} does not match the snapshot build {actual}"
            ));
        }
    }
    let config_digest = args
        .config_digest
        .clone()
        .or(snapshot_config)
        .ok_or_else(|| "a snapshot or --config-digest is required".to_string())?;
    let build_identity = match &args.build_identity {
        Some(path) => serde_json::from_slice(
            &fs::read(path).map_err(|error| format!("{}: {error}", path.display()))?,
        )
        .map_err(|error| format!("{}: {error}", path.display()))?,
        None => snapshot.clone().unwrap_or_else(|| json!({"kind": "unbound"})),
    };
    let locator = load_locator(args.locator.as_ref())?;
    let meta = TraceMeta::new(
        args.snapshot.as_deref().unwrap_or(""),
        &config_digest,
        build_identity,
        &args.tool,
        &args.tool_version,
    );
    let request = RuntimeImport {
        project_id: &args.project,
        run_id: &args.run_id,
        snapshot_id: args.snapshot.as_deref(),
        config_digest: &config_digest,
        allow_unbound: args.allow_unbound,
        scenario_run_id: args.scenario_run_id.as_deref(),
    };
    let mut processor_info: Option<Value> = None;
    let trace = match args.format.as_str() {
        "perfetto" => {
            let input = args
                .input
                .as_ref()
                .ok_or_else(|| "--input is required for perfetto".to_string())?;
            if let Some(processor) = &args.trace_processor {
                let capture = perfetto_trace_via_processor(input, processor, &locator, &meta)
                    .map_err(|error| error.to_string())?;
                processor_info = Some(json!({
                    "path": capture.tool_path,
                    "version": capture.tool_version,
                    "sha256": capture.tool_sha256,
                    "slice_rows": capture.slice_rows,
                    "counter_rows": capture.counter_rows,
                }));
                capture.trace
            } else {
                let bytes =
                    fs::read(input).map_err(|error| format!("{}: {error}", input.display()))?;
                perfetto_trace(&bytes, &locator, &meta).map_err(|error| error.to_string())?
            }
        }
        "tracy" => {
            let capture_path = args
                .tracy_capture
                .as_ref()
                .ok_or_else(|| "--tracy-capture is required for tracy".to_string())?;
            let csv_path = args
                .tracy_csv
                .as_ref()
                .ok_or_else(|| "--tracy-csv is required for tracy".to_string())?;
            let capture = fs::read(capture_path)
                .map_err(|error| format!("{}: {error}", capture_path.display()))?;
            let csv = fs::read_to_string(csv_path)
                .map_err(|error| format!("{}: {error}", csv_path.display()))?;
            let timeline = import_tracy_capture(
                &mut store, &request, &capture, &csv, &locator, &meta,
            )
            .map_err(|error| error.to_string())?;
            let result = json!({
                "command": "trace-import",
                "format": "tracy",
                "project_id": args.project,
                "run_id": args.run_id,
                "bound": args.snapshot.is_some(),
                "memory_limit_mb": args.memory_limit_mb,
                "native_capture": {
                    "path": capture_path,
                    "size": capture.len(),
                    "sha256": hash::digest_bytes(&capture),
                },
                "timeline": timeline,
            });
            return write_trace_result(args.out.as_ref(), result);
        }
        "probe" => {
            let input = args
                .input
                .as_ref()
                .ok_or_else(|| "--input is required for probe".to_string())?;
            let text = fs::read_to_string(input)
                .map_err(|error| format!("{}: {error}", input.display()))?;
            probe_jsonl(&text, &locator, &meta).map_err(|error| error.to_string())?
        }
        "chrome" => {
            let input = args
                .input
                .as_ref()
                .ok_or_else(|| "--input is required for chrome".to_string())?;
            let value: Value = serde_json::from_slice(
                &fs::read(input).map_err(|error| format!("{}: {error}", input.display()))?,
            )
            .map_err(|error| format!("{}: {error}", input.display()))?;
            chrome_trace(&value, &locator, &meta, false).map_err(|error| error.to_string())?
        }
        "normalized" => {
            let input = args
                .input
                .as_ref()
                .ok_or_else(|| "--input is required for normalized".to_string())?;
            let value: Value = serde_json::from_slice(
                &fs::read(input).map_err(|error| format!("{}: {error}", input.display()))?,
            )
            .map_err(|error| format!("{}: {error}", input.display()))?;
            value
        }
        other => return Err(format!("unsupported trace format {other}")),
    };
    let timeline = import_trace(&mut store, &request, &trace).map_err(|error| error.to_string())?;
    let result = json!({
        "command": "trace-import",
        "format": args.format,
        "project_id": args.project,
        "run_id": args.run_id,
        "bound": args.snapshot.is_some(),
        "memory_limit_mb": args.memory_limit_mb,
        "trace_processor": processor_info,
        "timeline": timeline,
    });
    write_trace_result(args.out.as_ref(), result)
}

fn write_trace_result(out: Option<&PathBuf>, result: Value) -> Result<Value, String> {
    if let Some(path) = out {
        fs::write(
            path,
            serde_json::to_string_pretty(&result).map_err(|error| error.to_string())? + "\n",
        )
        .map_err(|error| format!("{}: {error}", path.display()))?;
    }
    Ok(result)
}

#[cfg(test)]
mod index_tests {
    use super::*;
    use archmeld_core::source_api::read_source;

    #[test]
    fn index_parses_the_captured_snapshot_not_a_concurrent_save() {
        let dir = tempfile::tempdir().unwrap();
        let workspace = dir.path().join("repo");
        fs::create_dir_all(workspace.join("src")).unwrap();
        let old = "def old_symbol():\n    return 'old'\n";
        let new = "def new_symbol():\n    return 'new'\n";
        fs::write(workspace.join("src/target.py"), old).unwrap();
        let live = workspace.canonicalize().unwrap();
        let state = dir.path().join("state");
        let mut helper_saw_live = false;
        let mut runner = |_helper: &PathBuf,
                          analysis: &std::path::Path,
                          export: &std::path::Path,
                          _state: &std::path::Path|
         -> Result<HelperOutput, String> {
            if analysis == live {
                helper_saw_live = true;
            }
            let captured = fs::read(analysis.join("src/target.py"))
                .map_err(|error| format!("helper read: {error}"))?;
            assert_eq!(
                String::from_utf8(captured).unwrap(),
                old,
                "helper must parse the materialized snapshot"
            );
            // Simulate a user save after snapshot capture, while the helper is
            // still parsing.
            fs::write(workspace.join("src/target.py"), new).unwrap();
            let manifest = json!({
                "type":"manifest","format":"archmeld-export-v1","source":"codegraph",
                "version":"0.2.0","node_count":1,"edge_count":0,"iteration":"real-iterators"
            });
            let node = json!({
                "type":"node","id":1,"node_type":"Function","properties":{
                    "path": analysis.join("src/target.py").to_string_lossy(),
                    "name":"old_symbol","line_start":1,"line_end":2
                }
            });
            fs::write(export, format!("{manifest}\n{node}\n"))
                .map_err(|error| format!("helper export: {error}"))?;
            let summary = json!({
                "format":"archmeld-export-v1","nodes":1,"edges":0,
                "files_total":1,"files_parsed":1,"files_skipped":0,
                "by_language":{"python":1},"parser_errors":{},"elapsed_ms":1,
                "storage":"in_memory"
            });
            Ok(HelperOutput {
                success: true,
                status: Some(0),
                stdout: serde_json::to_vec(&summary).unwrap(),
                stderr: Vec::new(),
            })
        };
        let args = IndexArgs {
            workspace: workspace.clone(),
            state: state.clone(),
            project: "project.index.race".to_string(),
            max_files: 500,
            index_helper: None,
            run_id: Some("run.index.race".to_string()),
        };
        let result = run_index_with(args, &dir.path().join("fake-helper.exe"), &mut runner)
            .expect("index run");
        assert!(!helper_saw_live, "helper received the live workspace");
        assert_eq!(
            fs::read_to_string(workspace.join("src/target.py")).unwrap(),
            new,
            "the simulated save must have happened"
        );
        let snapshot_id = result["snapshot_id"].as_str().unwrap();
        let store = Store::open(&state).unwrap();
        let entity_ids = store
            .entity_ids("project.index.race", snapshot_id)
            .unwrap();
        assert_eq!(entity_ids.len(), 1);
        let entity = store
            .get_entity("project.index.race", snapshot_id, &entity_ids[0])
            .unwrap()
            .unwrap();
        assert!(
            entity["qualified_name"]
                .as_str()
                .unwrap()
                .ends_with("::old_symbol"),
            "{entity}"
        );
        let source = read_source(
            &store,
            &json!({
                "project_id":"project.index.race",
                "snapshot_id":snapshot_id,
                "entity_id":entity_ids[0],
                "anchor_index":0,
                "max_bytes":1024
            }),
        )
        .unwrap();
        let text = source["text"].as_str().unwrap();
        assert!(text.contains("old_symbol"), "{text}");
        assert!(!text.contains("new_symbol"), "{text}");
        assert_eq!(result["analysis"]["mode"], "snapshot_materialization");
        assert_eq!(result["analysis"]["removed"], true);
        assert_eq!(result["snapshot_files"], 1);
    }
}

fn main() -> ExitCode {
    let cli = Cli::parse();
    let result = match cli.command {
        Commands::Index(args) => run_index(args),
        Commands::Status(args) => run_status(args),
        Commands::ReviewDiff(args) => run_review_diff(args),
        Commands::Refresh(args) => run_refresh(args),
        Commands::Report(args) => run_report(args),
        Commands::VerifyReport(args) => run_verify(args),
        Commands::Backup(args) => run_backup(args),
        Commands::VerifyBackup(args) => run_verify_backup(args),
        Commands::RestoreBackup(args) => run_restore_backup(args),
        Commands::RotateBackup(args) => run_rotate_backup(args),
        Commands::RetainBackup(args) => run_retain_backup(args),
        Commands::Scenario(args) => run_scenario_command(args),
        Commands::Watch(args) => run_watch(args),
        Commands::TraceImport(args) => run_trace_import(args),
        Commands::LedgerMigrate(args) => run_migrate(args),
        Commands::NumericCheck(args) => run_numeric_check(args),
    };
    match result {
        Ok(value) => {
            println!("{}", value);
            ExitCode::SUCCESS
        }
        Err(error) => {
            eprintln!("{}", json!({"error": error}));
            ExitCode::FAILURE
        }
    }
}

#[cfg(test)]
mod status_tests {
    use super::*;
    use archmeld_core::snapshot::{capture_snapshot, SnapshotInputs, SnapshotOptions};

    #[test]
    fn status_reports_the_latest_snapshot_and_workspace_freshness_without_mutation() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join("repo");
        fs::create_dir_all(&root).unwrap();
        fs::write(root.join("a.py"), "VALUE = 1\n").unwrap();
        let state = dir.path().join("state");
        let mut store = Store::open(&state).unwrap();
        store.ensure_project("project.status", "expedition").unwrap();
        let captured = capture_snapshot(
            &mut store,
            "project.status",
            &root,
            &SnapshotInputs {
                build_config: json!({"variant": "status"}),
                environment: json!({"os": "test"}),
            },
            &SnapshotOptions {
                include_untracked: true,
                ..SnapshotOptions::default()
            },
        )
        .unwrap();
        drop(store);
        fs::write(root.join("a.py"), "VALUE = 2\n").unwrap();
        let result = run_status(StatusArgs {
            state: state.clone(),
            project: Some("project.status".to_string()),
            workspace: Some(root.clone()),
        })
        .unwrap();
        let project = &result["projects"][0];
        assert_eq!(project["latest_snapshot_id"], captured.snapshot_id);
        assert_eq!(project["snapshot_count"], 1);
        assert_eq!(project["snapshot"]["file_count"], 1);
        assert_eq!(project["freshness"]["state"], "changed");
        assert_eq!(project["freshness"]["changed"][0]["path"], "a.py");
        assert_eq!(project["freshness"]["source_mutation"], "none");
        let reopened = Store::open(&state).unwrap();
        assert_eq!(reopened.snapshot_count("project.status").unwrap(), 1);
    }
}

#[cfg(test)]
mod diff_cli_tests {
    use super::*;
    use archmeld_core::snapshot::{capture_snapshot, SnapshotInputs, SnapshotOptions};

    fn fake_export(
        analysis: &std::path::Path,
        export: &std::path::Path,
    ) -> Result<(), String> {
        let source = fs::read_to_string(analysis.join("src/a.py"))
            .map_err(|error| format!("fake helper read: {error}"))?;
        let name = source
            .split("def ")
            .nth(1)
            .and_then(|rest| rest.split('(').next())
            .unwrap_or("unknown")
            .trim();
        let manifest = json!({
            "type":"manifest","format":"archmeld-export-v1","source":"codegraph",
            "version":"0.2.0","node_count":1,"edge_count":0,"iteration":"real-iterators"
        });
        let node = json!({
            "type":"node","id":1,"node_type":"Function","properties":{
                "path": analysis.join("src/a.py").to_string_lossy(),
                "name": name, "line_start": 1, "line_end": 2
            }
        });
        fs::write(export, format!("{manifest}\n{node}\n"))
            .map_err(|error| format!("fake helper export: {error}"))
    }

    fn fake_summary() -> Vec<u8> {
        serde_json::to_vec(&json!({
            "format":"archmeld-export-v1","nodes":1,"edges":0,
            "files_total":1,"files_parsed":1,"files_skipped":0,
            "by_language":{"python":1},"parser_errors":{},"elapsed_ms":1,
            "storage":"in_memory"
        }))
        .unwrap()
    }

    #[test]
    fn review_diff_cli_reports_changed_entity_and_not_run_verification() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join("repo");
        fs::create_dir_all(root.join("src")).unwrap();
        fs::write(root.join("src/a.py"), "def alpha():\n    return 1\n").unwrap();
        let state = dir.path().join("state");
        let mut store = Store::open(&state).unwrap();
        store.ensure_project("project.review.cli", "expedition").unwrap();
        let captured = capture_snapshot(
            &mut store,
            "project.review.cli",
            &root,
            &SnapshotInputs {
                build_config: json!({"variant":"review-cli"}),
                environment: json!({"os":"test"}),
            },
            &SnapshotOptions {
                include_untracked: true,
                ..SnapshotOptions::default()
            },
        )
        .unwrap();
        let config = captured.snapshot["config_digest"]
            .as_str()
            .unwrap()
            .to_string();
        let export = json!({
            "type":"node","id":1,"node_type":"Function","properties":{
                "path": root.join("src/a.py").to_string_lossy(),
                "name":"alpha","line_start":1,"line_end":2
            }
        });
        import_codegraph_export(
            &mut store,
            &BridgeConfig {
                project_id: "project.review.cli",
                snapshot_id: &captured.snapshot_id,
                config_digest: &config,
                run_id: "run.review.cli",
                workspace_root: Some(&root),
                max_entities_per_batch: 500,
                max_relations_per_batch: 2000,
            },
            &captured.files,
            &format!(
                "{}\n{}\n",
                json!({
                    "type":"manifest","format":"archmeld-export-v1","source":"codegraph",
                    "version":"0.2.0","node_count":1,"edge_count":0,"iteration":"real-iterators"
                }),
                export
            ),
        )
        .unwrap();
        drop(store);
        fs::write(root.join("src/a.py"), "def alpha():\n    return 2\n").unwrap();
        let result = run_review_diff(ReviewDiffArgs {
            state,
            project: "project.review.cli".to_string(),
            workspace: root,
            base_snapshot: None,
        })
        .unwrap();
        assert_eq!(result["command"], "review-diff");
        assert_eq!(result["state"], "changed");
        assert_eq!(result["verification"]["state"], "not_run");
        let file = &result["files"][0];
        assert_eq!(file["path"], "src/a.py");
        assert_eq!(file["entities"][0]["qualified_name"], format!("src/a.py::alpha"));
        assert_eq!(file["entities"][0]["state"], "changed");
    }

    #[test]
    fn refresh_creates_a_new_snapshot_and_diffs_from_the_base() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join("repo");
        fs::create_dir_all(root.join("src")).unwrap();
        fs::write(root.join("src/a.py"), "def alpha():\n    return 1\n").unwrap();
        let state = dir.path().join("state");
        let helper = dir.path().join("fake-helper.exe");
        let mut runner = |_helper: &PathBuf,
                          analysis: &std::path::Path,
                          export: &std::path::Path,
                          _state: &std::path::Path|
         -> Result<HelperOutput, String> {
            fake_export(analysis, export)?;
            Ok(HelperOutput {
                success: true,
                status: Some(0),
                stdout: fake_summary(),
                stderr: Vec::new(),
            })
        };
        let base = run_index_with(
            IndexArgs {
                workspace: root.clone(),
                state: state.clone(),
                project: "project.refresh".to_string(),
                max_files: 100,
                index_helper: None,
                run_id: Some("run.refresh.base".to_string()),
            },
            &helper,
            &mut runner,
        )
        .unwrap();
        let base_snapshot = base["snapshot_id"].as_str().unwrap().to_string();
        fs::write(root.join("src/a.py"), "def beta():\n    return 2\n").unwrap();
        let refreshed = run_refresh_with(
            RefreshArgs {
                state: state.clone(),
                project: "project.refresh".to_string(),
                workspace: root,
                base_snapshot: None,
                max_files: 100,
                index_helper: None,
                run_id: Some("run.refresh.new".to_string()),
            },
            &helper,
            &mut runner,
        )
        .unwrap();
        assert_eq!(refreshed["command"], "refresh");
        assert_eq!(refreshed["state"], "changed");
        assert_eq!(refreshed["history"]["old_snapshot_retained"], true);
        assert_ne!(refreshed["base_snapshot_id"], refreshed["new_snapshot_id"]);
        assert_eq!(refreshed["base_snapshot_id"], base_snapshot);
        assert!(refreshed["diff"]["files"]["changed"]["items"]
            .as_array()
            .unwrap()
            .iter()
            .any(|file| file["path"] == "src/a.py"));
        assert!(refreshed["diff"]["entities"]["deleted"]["items"]
            .as_array()
            .unwrap()
            .iter()
            .any(|entity| entity["qualified_name"].as_str().unwrap().ends_with("::alpha")));
        assert!(refreshed["diff"]["entities"]["added"]["items"]
            .as_array()
            .unwrap()
            .iter()
            .any(|entity| entity["qualified_name"].as_str().unwrap().ends_with("::beta")));
        let store = Store::open(&state).unwrap();
        assert_eq!(store.snapshot_count("project.refresh").unwrap(), 2);
    }
}
