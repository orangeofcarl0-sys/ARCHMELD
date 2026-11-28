// Copyright 2026 ARCHMELD contributors
// SPDX-License-Identifier: Apache-2.0

//! `brief` CLI commands.

use super::*;

#[derive(clap::Args)]
pub(crate) struct BriefArgs {
    #[arg(long)]
    pub(crate) state: PathBuf,
    #[arg(long)]
    pub(crate) project: String,
    /// Optional workspace for change review; defaults to the registered root.
    #[arg(long)]
    pub(crate) workspace: Option<PathBuf>,
    /// Base snapshot; defaults to the project's latest snapshot.
    #[arg(long)]
    pub(crate) snapshot: Option<String>,
    /// auto, project or change.
    #[arg(long, default_value = "auto")]
    pub(crate) recipe: String,
    /// Maximum changed-entity findings returned (default 20).
    #[arg(long, default_value_t = 20)]
    pub(crate) max_findings: usize,
    /// Maximum unchanged-context entries returned (default 10).
    #[arg(long, default_value_t = 10)]
    pub(crate) max_context: usize,
    /// Maximum per-file gap entries returned (default 10).
    #[arg(long, default_value_t = 10)]
    pub(crate) max_gaps: usize,
    /// Maximum aggregated relation rows returned (default 10).
    #[arg(long, default_value_t = 10)]
    pub(crate) max_relation_items: usize,
    /// Print a markdown one-screen instead of JSON.
    #[arg(long)]
    pub(crate) markdown: bool,
    /// Write the session card to `<workspace>/.archmeld/SESSION.md` (or --out).
    #[arg(long)]
    pub(crate) write: bool,
    /// Write the session card to this path; implies --write.
    #[arg(long)]
    pub(crate) out: Option<PathBuf>,
    /// Overwrite an existing file that is not an ARCHMELD session card.
    #[arg(long)]
    pub(crate) force: bool,
}

#[derive(clap::Args)]
pub(crate) struct MapArgs {
    #[command(subcommand)]
    pub(crate) action: MapAction,
}

#[derive(clap::Subcommand)]
pub(crate) enum MapAction {
    /// Validate a map and report orphan/ambiguous/stale items.
    Check(MapCheckArgs),
    /// Report map coverage, refs and budget without failing on warnings.
    Status(MapCheckArgs),
    /// List stale/orphan/missing entries without writing the map.
    Maintain(MapCheckArgs),
}

#[derive(clap::Args)]
pub(crate) struct MapCheckArgs {
    #[arg(long)]
    pub(crate) state: PathBuf,
    #[arg(long)]
    pub(crate) project: String,
    /// Workspace containing the map; defaults to the registered root.
    #[arg(long)]
    pub(crate) workspace: Option<PathBuf>,
    /// Snapshot to resolve against; defaults to the latest snapshot.
    #[arg(long)]
    pub(crate) snapshot: Option<String>,
    /// Map path; defaults to <workspace>/archmeld.map.txt.
    #[arg(long)]
    pub(crate) map: Option<PathBuf>,
    /// Validate this candidate file instead of the canonical map.
    #[arg(long)]
    pub(crate) candidate: Option<PathBuf>,
    /// Print a markdown report instead of JSON.
    #[arg(long)]
    pub(crate) markdown: bool,
}

pub(crate) fn run_brief(args: BriefArgs) -> Result<Value, String> {
    let store = Store::open(&args.state).map_err(|error| error.to_string())?;
    let recipe = BriefRecipe::parse(&args.recipe).map_err(|error| error.to_string())?;
    let workspace = match args.workspace {
        Some(path) => Some(
            path.canonicalize()
                .map_err(|error| format!("workspace {}: {error}", path.display()))?,
        ),
        None => store
            .active_project_root(&args.project)
            .map_err(|error| error.to_string())?,
    };
    let mut brief = build_brief_with_budget(
        &store,
        &BriefRequest {
            project_id: &args.project,
            snapshot_id: args.snapshot.as_deref(),
            workspace: workspace.as_deref(),
            recipe,
        },
        BriefBudget {
            max_findings: args.max_findings,
            max_context: args.max_context,
            max_gaps: args.max_gaps,
            max_relation_items: args.max_relation_items,
        },
    )
    .map_err(|error| error.to_string())?;
    if args.write || args.out.is_some() {
        let Some(root) = workspace.as_deref() else {
            return Err(
                "brief --write requires --workspace or a registered project root".to_string(),
            );
        };
        let card = write_session_card(root, &brief, args.out.as_deref(), args.force)
            .map_err(|error| error.to_string())?;
        brief["session_card"] = card;
    }
    if args.markdown {
        Ok(Value::String(render_markdown(&brief)))
    } else {
        Ok(brief)
    }
}

pub(crate) fn run_map_command(args: MapArgs) -> Result<Value, String> {
    let (check, command) = match args.action {
        MapAction::Check(args) => (args, "check"),
        MapAction::Status(args) => (args, "status"),
        MapAction::Maintain(args) => (args, "maintain"),
    };
    let store = Store::open(&check.state).map_err(|error| error.to_string())?;
    let workspace = match check.workspace {
        Some(path) => Some(
            path.canonicalize()
                .map_err(|error| format!("workspace {}: {error}", path.display()))?,
        ),
        None => store
            .active_project_root(&check.project)
            .map_err(|error| error.to_string())?,
    };
    let Some(workspace) = workspace else {
        return Err("map check requires --workspace or a registered project root".to_string());
    };
    let path = check.candidate.as_deref().or(check.map.as_deref());
    let mut value = if command == "maintain" {
        maintain(
            &store,
            &check.project,
            &workspace,
            check.snapshot.as_deref(),
            path,
        )
    } else {
        check_map(
            &store,
            &check.project,
            &workspace,
            check.snapshot.as_deref(),
            path,
        )
    }
    .map_err(|error| error.to_string())?;
    value["command"] = json!(command);
    if check.markdown {
        value["markdown"] = json!(render_map_markdown(&value));
    }
    Ok(value)
}

#[cfg(test)]
mod map_tests {
    use super::*;

    fn args(state: PathBuf, root: PathBuf) -> MapCheckArgs {
        MapCheckArgs {
            state,
            project: "project.mapcli".to_string(),
            workspace: Some(root),
            snapshot: None,
            map: None,
            candidate: None,
            markdown: false,
        }
    }

    #[test]
    fn map_check_reads_the_canonical_map_and_absent_is_not_an_error() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join("repo");
        fs::create_dir_all(&root).unwrap();
        fs::write(root.join("a.py"), "x = 1\n").unwrap();
        let state = dir.path().join("state");
        let mut store = Store::open(&state).unwrap();
        store
            .ensure_project("project.mapcli", "expedition")
            .unwrap();
        let captured = capture_snapshot(
            &mut store,
            "project.mapcli",
            &root,
            &SnapshotInputs {
                build_config: json!({"variant": "mapcli"}),
                environment: json!({"os": "test"}),
            },
            &SnapshotOptions {
                include_untracked: true,
                ..SnapshotOptions::default()
            },
        )
        .unwrap();
        fs::write(
            root.join("archmeld.map.txt"),
            format!(
                "#ARCHMELD-MAP: 1\n#snapshot: {}\n===./===\n\
                 a.py: F:Returns one. | R:file:a.py | A:- | S:-\n",
                captured.snapshot_id
            ),
        )
        .unwrap();

        let checked = run_map_command(MapArgs {
            action: MapAction::Check(args(state.clone(), root.clone())),
        })
        .unwrap();
        assert_eq!(checked["state"], "ok", "{checked}");
        assert_eq!(checked["refs"]["confirmed"], 1, "{checked}");

        fs::remove_file(root.join("archmeld.map.txt")).unwrap();
        let absent = run_map_command(MapArgs {
            action: MapAction::Status(args(state, root)),
        })
        .unwrap();
        assert_eq!(absent["state"], "absent", "{absent}");
        assert_eq!(absent["command"], "status", "{absent}");
    }

    #[test]
    fn index_reports_incremental_mode_reuse_and_deletions_from_the_helper() {
        let dir = tempfile::tempdir().unwrap();
        let workspace = dir.path().join("repo");
        fs::create_dir_all(workspace.join("src")).unwrap();
        fs::write(workspace.join("src/a.py"), "def alpha():\n    return 1\n").unwrap();
        let state = dir.path().join("state");
        let mut runner = |_helper: &PathBuf,
                          analysis: &std::path::Path,
                          export: &std::path::Path,
                          _state: &std::path::Path|
         -> Result<HelperOutput, String> {
            let manifest = json!({
                "type":"manifest","format":"archmeld-export-v1","source":"codegraph",
                "version":"0.2.0","node_count":1,"edge_count":0,"iteration":"real-iterators"
            });
            let node = json!({
                "type":"node","id":1,"node_type":"Function","properties":{
                    "path": analysis.join("src/a.py").to_string_lossy(),
                    "name":"alpha","line_start":1,"line_end":2
                }
            });
            fs::write(export, format!("{manifest}\n{node}\n"))
                .map_err(|error| format!("helper export: {error}"))?;
            Ok(HelperOutput {
                success: true,
                status: Some(0),
                stdout: serde_json::to_vec(&json!({
                    "format":"archmeld-export-v1","nodes":1,"edges":0,
                    "files_total":6,"files_parsed":1,"files_skipped":5,
                    "files_reused":5,"files_deleted":2,"index_mode":"incremental_parse",
                    "by_language":{"python":1},"parser_errors":{},"elapsed_ms":1,
                    "storage":"persistent","state_loaded":5
                }))
                .unwrap(),
                stderr: Vec::new(),
            })
        };
        let result = run_index_with(
            IndexArgs {
                workspace: workspace.clone(),
                state: state.clone(),
                project: "project.index.incremental".to_string(),
                max_files: 500,
                index_helper: None,
                run_id: Some("run.index.incremental".to_string()),
                include_dir: Vec::new(),
                tracked_only: false,
                include_untracked: false,
                exclude_dir: Vec::new(),
                max_file_size: None,
                max_depth: None,
            },
            &dir.path().join("fake-helper.exe"),
            &mut runner,
        )
        .unwrap();
        assert_eq!(result["index_mode"], "incremental_parse", "{result}");
        assert_eq!(result["files_reused"], 5, "{result}");
        assert_eq!(result["files_deleted"], 2, "{result}");
        assert!(
            result["analysis"]["stable_root"]
                .as_str()
                .unwrap()
                .contains("index-work"),
            "materialization must use the stable per-project root: {result}"
        );
    }
}
