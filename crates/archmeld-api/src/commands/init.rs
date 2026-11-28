// Copyright 2026 ARCHMELD contributors
// SPDX-License-Identifier: Apache-2.0

//! `init` CLI commands.

use super::*;

#[derive(clap::Args)]
pub(crate) struct InitArgs {
    /// Repository to initialize; defaults to the current directory.
    #[arg(long, default_value = ".")]
    pub(crate) workspace: PathBuf,
    /// Ledger location; defaults to a per-workspace user data directory.
    #[arg(long)]
    pub(crate) state: Option<PathBuf>,
    /// Stable project id; defaults to a repo-name plus path hash.
    #[arg(long)]
    pub(crate) project: Option<String>,
    /// Comma-separated clients: claude,codex,generic. Defaults to claude,codex.
    #[arg(long = "client", value_delimiter = ',')]
    pub(crate) clients: Vec<String>,
    /// Write the agent rule block and execute the MCP client command.
    #[arg(long)]
    pub(crate) write: bool,
    /// Preflight only: show the plan without creating a ledger.
    #[arg(long)]
    pub(crate) dry_run: bool,
    /// Health-check an existing state instead of indexing.
    #[arg(long)]
    pub(crate) check: bool,
    #[arg(long, default_value_t = 5000)]
    pub(crate) max_files: usize,
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
    /// Override the packaged index helper (hidden; for tests and custom builds).
    #[arg(long, hide = true)]
    pub(crate) index_helper: Option<PathBuf>,
}

pub(crate) fn run_init(args: InitArgs) -> Result<Value, String> {
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
    let state = match &args.state {
        Some(path) => path.clone(),
        None => onboard::default_state_root(&workspace)?,
    };
    let project = args
        .project
        .clone()
        .unwrap_or_else(|| onboard::default_project_id(&workspace));
    if onboard::state_inside_workspace(&state, &workspace)? {
        return Err(
            "state_root_inside_workspace: choose a --state outside the repository; the ledger stores source bytes"
                .to_string(),
        );
    }
    let mut clients = args.clients.clone();
    if clients.is_empty() {
        clients = vec!["claude".to_string(), "codex".to_string()];
    }
    let preflight = onboard::count_source_files_with(
        &workspace,
        args.max_files,
        &args.include_dir,
        &args.exclude_dir,
        args.max_file_size,
    )?;
    if args.dry_run {
        return Ok(json!({
            "command": "init",
            "state": "dry_run",
            "workspace": workspace.to_string_lossy(),
            "state_root": state.to_string_lossy(),
            "project_id": project,
            "clients": clients,
            "preflight": preflight,
            "next": "re-run without --dry-run to index and render the client configuration"
        }));
    }
    if args.check {
        let health = onboard::health_check(&state, &project)?;
        return Ok(json!({
            "command": "init",
            "state": "check",
            "workspace": workspace.to_string_lossy(),
            "state_root": state.to_string_lossy(),
            "project_id": project,
            "health": health
        }));
    }
    // Install the rule block before indexing so a `--write` run leaves the
    // snapshot and the working tree in sync; preview runs write nothing.
    let rules = vec![
        onboard::install_rules(&workspace, "AGENTS.md", args.write)?,
        onboard::install_rules(&workspace, "CLAUDE.md", args.write)?,
    ];
    let mut warnings = Vec::new();
    if preflight["files"].as_u64().unwrap_or(0) == 0 {
        warnings.push(json!("no source files were found in the workspace"));
    }
    if preflight["truncated"].as_bool().unwrap_or(false) {
        warnings.push(json!(format!(
            "preflight stopped at max_files={}; the snapshot budget may truncate the index",
            args.max_files
        )));
    }
    fs::create_dir_all(&state).map_err(|error| format!("state {}: {error}", state.display()))?;
    let index = match existing_clean_index(&state, &project, &workspace)? {
        Some(existing) => {
            let mut store = Store::open(&state).map_err(|error| error.to_string())?;
            archmeld_core::manager::register_project_root(&mut store, &project, &workspace, &state)
                .map_err(|error| error.to_string())?;
            existing
        }
        None => run_index(IndexArgs {
            workspace: workspace.clone(),
            state: state.clone(),
            project: project.clone(),
            max_files: args.max_files,
            index_helper: args.index_helper.clone(),
            run_id: None,
            include_dir: args.include_dir.clone(),
            tracked_only: args.tracked_only,
            include_untracked: args.include_untracked,
            exclude_dir: args.exclude_dir.clone(),
            max_file_size: args.max_file_size,
            max_depth: args.max_depth,
        })?,
    };
    let mut mcp = Vec::new();
    for client in &clients {
        let plan = onboard::plan_mcp_install(client, &state, &project)?;
        let install = if args.write
            && !plan["command"]
                .as_array()
                .map(|command| command.is_empty())
                .unwrap_or(true)
        {
            match onboard::execute_mcp_install(&plan) {
                Ok(value) => value,
                Err(error) => {
                    warnings.push(json!({"client": client, "error": error}));
                    json!({"executed": false})
                }
            }
        } else {
            json!({"executed": false, "reason": "preview only; pass --write to install"})
        };
        mcp.push(json!({"plan": plan, "install": install}));
    }
    let health = match onboard::health_check(&state, &project) {
        Ok(value) => value,
        Err(error) => {
            warnings.push(json!({"health": error}));
            json!({"state": "failed", "error": error})
        }
    };
    let source_mutation = if rules.iter().any(|rule| {
        rule["written"].as_bool().unwrap_or(false) && rule["changed"].as_bool().unwrap_or(false)
    }) {
        "agent_rules_only"
    } else {
        "none"
    };
    let snapshot_id = index["snapshot_id"].clone();
    let files = index["snapshot_files"]
        .as_u64()
        .unwrap_or_else(|| preflight["files"].as_u64().unwrap_or(0));
    let counts = index["ledger"].clone();
    let coverage = index["bridge"]["coverage"].clone();
    let health_summary = json!({
        "state": health["state"],
        "tools": health["tools"],
        "project": health["project"],
        "snapshot_id": health["status"]["projects"][0]["latest_snapshot_id"],
        "freshness": health["status"]["projects"][0]["freshness"]["state"],
    });
    let rules_summary: Vec<Value> = rules
        .iter()
        .map(|rule| {
            json!({
                "path": rule["path"],
                "written": rule["written"],
                "changed": rule["changed"],
            })
        })
        .collect();
    let mcp_summary: Vec<Value> = mcp
        .iter()
        .map(|entry| {
            json!({
                "client": entry["plan"]["client"],
                "target": entry["plan"]["target"],
                "command_line": entry["plan"]["command_line"],
                "executed": entry["install"]["executed"],
            })
        })
        .collect();
    let reused = index["index_reuse"].as_bool().unwrap_or(false);
    let index_state = if reused { "reused" } else { "indexed" };
    let summary = if reused {
        format!(
            "{} file(s) already indexed (workspace clean); {} entities; {} relations; coverage {}; MCP health {}",
            files,
            counts["entities"].as_u64().unwrap_or(0),
            counts["relations"].as_u64().unwrap_or(0),
            coverage["state"].as_str().unwrap_or("unknown"),
            health_summary["state"].as_str().unwrap_or("unknown"),
        )
    } else {
        format!(
            "{} file(s) indexed; {} entities; {} relations; coverage {}; MCP health {}",
            files,
            counts["entities"].as_u64().unwrap_or(0),
            counts["relations"].as_u64().unwrap_or(0),
            coverage["state"].as_str().unwrap_or("unknown"),
            health_summary["state"].as_str().unwrap_or("unknown"),
        )
    };
    Ok(json!({
        "command": "init",
        "state": "ok",
        "index_state": index_state,
        "summary": summary,
        "workspace": workspace.to_string_lossy(),
        "state_root": state.to_string_lossy(),
        "project_id": project,
        "snapshot_id": snapshot_id,
        "counts": {
            "files": files,
            "entities": counts["entities"],
            "relations": counts["relations"],
        },
        "coverage": coverage,
        "rules": rules_summary,
        "mcp": mcp_summary,
        "health": health_summary,
        "warnings": warnings,
        "source_mutation": source_mutation,
        "next_prompt": "Ask your agent: what changed, who calls it, or summarize this project."
    }))
}
