// Copyright 2026 ARCHMELD contributors
// SPDX-License-Identifier: Apache-2.0

//! `evidence` CLI commands.

use super::*;

#[derive(clap::Args)]
pub(crate) struct EvidenceArgs {
    #[command(subcommand)]
    pub(crate) action: EvidenceAction,
}

#[derive(clap::Subcommand)]
pub(crate) enum EvidenceAction {
    /// Import one attestation JSON file. Relations stay basis=human,
    /// resolution=declared and are never promoted to runtime observations.
    ImportHuman(ImportHumanArgs),
}

#[derive(clap::Args)]
pub(crate) struct ImportHumanArgs {
    #[arg(long)]
    pub(crate) state: PathBuf,
    /// Attestation JSON file.
    #[arg(long)]
    pub(crate) input: PathBuf,
}

#[derive(clap::Args)]
pub(crate) struct ScenarioArgs {
    #[arg(long)]
    pub(crate) state: PathBuf,
    #[arg(long)]
    pub(crate) project: String,
    #[arg(long)]
    pub(crate) snapshot: String,
    /// Authorized workspace root. cwd must resolve inside it.
    #[arg(long)]
    pub(crate) workspace: PathBuf,
    #[arg(long)]
    pub(crate) cwd: Option<PathBuf>,
    #[arg(long, default_value_t = 60_000)]
    pub(crate) timeout_ms: u64,
    #[arg(long)]
    pub(crate) grant_id: String,
    #[arg(long)]
    pub(crate) reason: String,
    #[arg(long)]
    pub(crate) run_id: Option<String>,
    /// `workspace` (default), `workspace_copy` or `container`.
    #[arg(long, value_parser = ["workspace", "workspace_copy", "container"])]
    pub(crate) sandbox_mode: Option<String>,
    /// Repeatable. Files or directory prefixes copied from the snapshot.
    #[arg(long)]
    pub(crate) sandbox_include: Vec<String>,
    /// Repeatable. Prefixes excluded from the snapshot copy.
    #[arg(long)]
    pub(crate) sandbox_exclude: Vec<String>,
    /// Digest-pinned local image for `container` mode.
    #[arg(long)]
    pub(crate) sandbox_image: Option<String>,
    /// Only `none` is accepted; omitted also means no network.
    #[arg(long)]
    pub(crate) sandbox_network: Option<String>,
    /// Relative cwd inside the snapshot copy. Defaults to the workspace-relative
    /// `--cwd` (or the snapshot root).
    #[arg(long)]
    pub(crate) sandbox_cwd: Option<String>,
    /// Command and arguments after `--`.
    #[arg(trailing_var_arg = true, required = true)]
    pub(crate) argv: Vec<String>,
}

#[derive(clap::Args)]
pub(crate) struct WatchArgs {
    #[arg(long)]
    pub(crate) state: PathBuf,
    #[arg(long)]
    pub(crate) project: String,
    #[arg(long)]
    pub(crate) snapshot: String,
    #[arg(long)]
    pub(crate) workspace: PathBuf,
    #[arg(long, default_value_t = 1000)]
    pub(crate) interval_ms: u64,
    /// Stop after N ticks; omit to run until the process is terminated.
    #[arg(long)]
    pub(crate) iterations: Option<usize>,
    /// `polling` compares hashes on an interval; `os` waits for real file
    /// notifications and re-hashes after a quiet period.
    #[arg(long, default_value = "polling", value_parser = ["polling", "os"])]
    pub(crate) mode: String,
    #[arg(long, default_value_t = 200)]
    pub(crate) debounce_ms: u64,
}

#[derive(clap::Args)]
pub(crate) struct TraceImportArgs {
    #[arg(long)]
    pub(crate) state: PathBuf,
    #[arg(long)]
    pub(crate) project: String,
    #[arg(long)]
    pub(crate) run_id: String,
    /// Snapshot the trace is bound to. Without it the trace is unbound and
    /// `--allow-unbound` is required.
    #[arg(long)]
    pub(crate) snapshot: Option<String>,
    /// Override is rejected when it disagrees with the snapshot build.
    #[arg(long)]
    pub(crate) config_digest: Option<String>,
    /// Existing scenario run to associate with this trace. The run must
    /// belong to the project and match the trace snapshot/build config.
    #[arg(long)]
    pub(crate) scenario_run_id: Option<String>,
    /// JSON file with the trace build identity; defaults to the snapshot
    /// manifest when bound.
    #[arg(long)]
    pub(crate) build_identity: Option<PathBuf>,
    #[arg(long)]
    pub(crate) allow_unbound: bool,
    #[arg(long, value_parser = ["perfetto", "tracy", "probe", "chrome", "normalized"])]
    pub(crate) format: String,
    /// Perfetto `.pftrace`, probe JSONL or Chrome/VizTracer JSON.
    #[arg(long)]
    pub(crate) input: Option<PathBuf>,
    /// Optional `trace_processor_shell`; when set, Perfetto imports query the
    /// trace with explicit SQL instead of the in-repo protobuf decoder.
    #[arg(long)]
    pub(crate) trace_processor: Option<PathBuf>,
    /// Native `.tracy` capture for `--format tracy`.
    #[arg(long)]
    pub(crate) tracy_capture: Option<PathBuf>,
    /// `tracy-csvexport -u -p` output for `--format tracy`.
    #[arg(long)]
    pub(crate) tracy_csv: Option<PathBuf>,
    /// JSON `{"locations":[{"path","line","entity_id"}],"symbols":{...}}`.
    #[arg(long)]
    pub(crate) locator: Option<PathBuf>,
    #[arg(long, default_value = "archmeld-cli")]
    pub(crate) tool: String,
    #[arg(long, default_value = "1")]
    pub(crate) tool_version: String,
    #[arg(long)]
    pub(crate) out: Option<PathBuf>,
    /// Hard process memory ceiling for the import, enforced by the OS before
    /// the trace is read.
    #[arg(long)]
    pub(crate) memory_limit_mb: Option<u64>,
}

pub(crate) fn run_evidence(args: EvidenceArgs) -> Result<Value, String> {
    match args.action {
        EvidenceAction::ImportHuman(args) => {
            let text = fs::read_to_string(&args.input)
                .map_err(|error| format!("{}: {error}", args.input.display()))?;
            let input = hash::parse_strict(&text)
                .map_err(|error| format!("{}: {error}", args.input.display()))?;
            let mut store = Store::open(&args.state).map_err(|error| error.to_string())?;
            let mut result = archmeld_core::human::import_human_evidence(&mut store, &input)
                .map_err(|error| error.to_string())?;
            result["command"] = json!("evidence");
            result["action"] = json!("import-human");
            Ok(result)
        }
    }
}

pub(crate) fn run_watch(args: WatchArgs) -> Result<Value, String> {
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

pub(crate) fn run_scenario_command(args: ScenarioArgs) -> Result<Value, String> {
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

pub(crate) fn run_trace_import(args: TraceImportArgs) -> Result<Value, String> {
    if let Some(limit) = args.memory_limit_mb {
        crate::memory_limit::apply_limit_mb(limit)?;
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
        None => snapshot
            .clone()
            .unwrap_or_else(|| json!({"kind": "unbound"})),
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
            let timeline =
                import_tracy_capture(&mut store, &request, &capture, &csv, &locator, &meta)
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
