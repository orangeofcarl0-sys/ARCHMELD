// Copyright 2026 ARCHMELD contributors
// SPDX-License-Identifier: Apache-2.0

//! `review` CLI commands.

use super::*;

#[derive(clap::Args)]
pub(crate) struct ReviewDiffArgs {
    #[arg(long)]
    pub(crate) state: PathBuf,
    #[arg(long)]
    pub(crate) project: String,
    /// Workspace to compare with the base snapshot. Required unless
    /// --candidate-snapshot is used.
    #[arg(long, conflicts_with = "candidate_snapshot")]
    pub(crate) workspace: Option<PathBuf>,
    /// Candidate snapshot for a snapshot-to-snapshot structural diff.
    #[arg(long, conflicts_with = "workspace")]
    pub(crate) candidate_snapshot: Option<String>,
    /// Base snapshot; defaults to the project's latest snapshot.
    #[arg(long)]
    pub(crate) base_snapshot: Option<String>,
    /// Persist the result and pin the snapshots it cites.
    #[arg(long, conflicts_with = "report")]
    pub(crate) persist: bool,
    /// Freeze changed/added candidate bytes into the CAS (requires --persist).
    #[arg(long, requires = "persist", conflicts_with_all = ["candidate_snapshot", "report"])]
    pub(crate) freeze: bool,
    /// Read a persisted review report instead of running a new review.
    #[arg(long, conflicts_with_all = ["workspace", "candidate_snapshot", "persist", "freeze"])]
    pub(crate) report: Option<String>,
}

#[derive(clap::Args)]
pub(crate) struct ReviewReportArgs {
    #[arg(long)]
    pub(crate) state: PathBuf,
    #[arg(long)]
    pub(crate) project: String,
    #[command(subcommand)]
    pub(crate) action: ReviewReportAction,
}

#[derive(clap::Subcommand)]
pub(crate) enum ReviewReportAction {
    /// List reports with size and retention state.
    List,
    /// Delete a report and the snapshot pins it created.
    Delete {
        #[arg(long)]
        report: String,
    },
}

pub(crate) fn run_review_diff(args: ReviewDiffArgs) -> Result<Value, String> {
    let mut store = Store::open(&args.state).map_err(|error| error.to_string())?;
    if let Some(report_id) = args.report {
        if !store
            .list_projects()
            .map_err(|error| error.to_string())?
            .iter()
            .any(|project| project == &args.project)
        {
            return Err(format!("project {} is not registered", args.project));
        }
        let report = store
            .get_review_report(&args.project, &report_id)
            .map_err(|error| error.to_string())?
            .ok_or_else(|| format!("review report {report_id} not found"))?;
        let mut response = report["output"].clone();
        if !response.is_object() {
            response = json!({});
        }
        response["command"] = json!("review-diff");
        response["mode"] = json!("report");
        response["report"] = json!({
            "report_id": report["report_id"],
            "base_snapshot_id": report["base_snapshot_id"],
            "candidate_snapshot_id": report["candidate_snapshot_id"],
            "output_digest": report["output_digest"],
            "created_at": report["created_at"],
        });
        return Ok(response);
    }
    let base_snapshot_id = match args.base_snapshot {
        Some(snapshot_id) => snapshot_id,
        None => store
            .latest_snapshot_id(&args.project)
            .map_err(|error| error.to_string())?
            .ok_or_else(|| format!("project {} has no snapshot to review against", args.project))?,
    };
    if let Some(candidate_snapshot_id) = args.candidate_snapshot {
        if args.freeze {
            return Err("--freeze is only valid for a workspace review".to_string());
        }
        let mut result = diff_snapshots(
            &store,
            &args.project,
            &base_snapshot_id,
            &candidate_snapshot_id,
        )
        .map_err(|error| error.to_string())?;
        result["command"] = json!("review-diff");
        result["mode"] = json!("snapshot-diff");
        if args.persist {
            let stored = persist_review_report(
                &mut store,
                &args.project,
                &base_snapshot_id,
                Some(&candidate_snapshot_id),
                "snapshot-diff",
                &result,
            )
            .map_err(|error| error.to_string())?;
            result["report"] = stored;
        }
        return Ok(result);
    }
    let workspace = args
        .workspace
        .ok_or_else(|| {
            "review-diff requires --workspace or --candidate-snapshot".to_string()
        })?
        .canonicalize()
        .map_err(|error| format!("workspace: {error}"))?;
    let mut result = review_workspace_diff(&store, &args.project, &base_snapshot_id, &workspace)
        .map_err(|error| error.to_string())?;
    result["command"] = json!("review-diff");
    result["mode"] = json!("workspace");
    if args.freeze {
        if !args.persist {
            return Err("--freeze requires --persist".to_string());
        }
        let frozen = freeze_review_candidate(&mut store, &workspace, &mut result)
            .map_err(|error| error.to_string())?;
        result["freeze"] = frozen;
    }
    if args.persist {
        let stored = persist_review_report(
            &mut store,
            &args.project,
            &base_snapshot_id,
            None,
            "workspace",
            &result,
        )
        .map_err(|error| error.to_string())?;
        result["report"] = stored;
    }
    Ok(result)
}

pub(crate) fn run_review_report(args: ReviewReportArgs) -> Result<Value, String> {
    let mut store = Store::open(&args.state).map_err(|error| error.to_string())?;
    if !store
        .list_projects()
        .map_err(|error| error.to_string())?
        .iter()
        .any(|project| project == &args.project)
    {
        return Err(format!("project {} is not registered", args.project));
    }
    match args.action {
        ReviewReportAction::List => {
            let reports = store
                .list_review_reports(&args.project)
                .map_err(|error| error.to_string())?;
            Ok(json!({
                "command": "review-report",
                "action": "list",
                "project_id": args.project,
                "count": reports.len(),
                "reports": reports,
            }))
        }
        ReviewReportAction::Delete { report } => {
            let deleted = store
                .delete_review_report(&args.project, &report)
                .map_err(|error| error.to_string())?;
            Ok(json!({
                "command": "review-report",
                "action": "delete",
                "project_id": args.project,
                "result": deleted,
            }))
        }
    }
}

#[cfg(test)]
mod diff_cli_tests {
    use super::*;
    use archmeld_core::snapshot::{capture_snapshot, SnapshotInputs, SnapshotOptions};

    fn fake_export(analysis: &std::path::Path, export: &std::path::Path) -> Result<(), String> {
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
            "files_reused":0,"files_deleted":0,"index_mode":"full_parse",
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
        store
            .ensure_project("project.review.cli", "expedition")
            .unwrap();
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
            state: state.clone(),
            project: "project.review.cli".to_string(),
            workspace: Some(root.clone()),
            candidate_snapshot: None,
            base_snapshot: None,
            persist: false,
            freeze: false,
            report: None,
        })
        .unwrap();
        assert_eq!(result["command"], "review-diff");
        assert_eq!(result["state"], "changed");
        assert_eq!(result["verification"]["state"], "not_run");
        let file = &result["files"][0];
        assert_eq!(file["path"], "src/a.py");
        assert_eq!(
            file["entities"][0]["qualified_name"],
            format!("src/a.py::alpha")
        );
        assert_eq!(file["entities"][0]["state"], "changed");

        let persisted = run_review_diff(ReviewDiffArgs {
            state: state.clone(),
            project: "project.review.cli".to_string(),
            workspace: Some(root),
            candidate_snapshot: None,
            base_snapshot: None,
            persist: true,
            freeze: false,
            report: None,
        })
        .unwrap();
        let report_id = persisted["report"]["stored"]["report_id"]
            .as_str()
            .expect("report id")
            .to_string();
        let loaded = run_review_diff(ReviewDiffArgs {
            state: state.clone(),
            project: "project.review.cli".to_string(),
            workspace: None,
            candidate_snapshot: None,
            base_snapshot: None,
            persist: false,
            freeze: false,
            report: Some(report_id.clone()),
        })
        .unwrap();
        assert_eq!(loaded["command"], "review-diff");
        assert_eq!(loaded["mode"], "report");
        assert_eq!(loaded["state"], "changed");
        assert_eq!(loaded["report"]["report_id"], report_id);

        let listed = run_review_report(ReviewReportArgs {
            state: state.clone(),
            project: "project.review.cli".to_string(),
            action: ReviewReportAction::List,
        })
        .unwrap();
        assert_eq!(listed["command"], "review-report");
        assert_eq!(listed["action"], "list");
        assert_eq!(listed["count"], 1, "{listed}");
        assert_eq!(listed["reports"][0]["retention"]["state"], "pinned");

        let deleted = run_review_report(ReviewReportArgs {
            state: state.clone(),
            project: "project.review.cli".to_string(),
            action: ReviewReportAction::Delete {
                report: report_id.clone(),
            },
        })
        .unwrap();
        assert_eq!(deleted["result"]["removed"], true, "{deleted}");
        assert!(
            deleted["result"]["pins_removed"].as_u64().unwrap() >= 1,
            "{deleted}"
        );
        let missing = run_review_diff(ReviewDiffArgs {
            state: state.clone(),
            project: "project.review.cli".to_string(),
            workspace: None,
            candidate_snapshot: None,
            base_snapshot: None,
            persist: false,
            freeze: false,
            report: Some(report_id),
        })
        .unwrap_err();
        assert!(missing.contains("not found"), "{missing}");

        let unregistered = run_review_report(ReviewReportArgs {
            state,
            project: "project.review.missing".to_string(),
            action: ReviewReportAction::List,
        })
        .unwrap_err();
        assert!(unregistered.contains("not registered"), "{unregistered}");
    }

    #[test]
    fn review_diff_rejects_freeze_without_a_workspace_review() {
        let dir = tempfile::tempdir().unwrap();
        let state = dir.path().join("state");
        Store::open(&state).unwrap();
        let error = run_review_diff(ReviewDiffArgs {
            state,
            project: "project.review.cli".to_string(),
            workspace: None,
            candidate_snapshot: Some("sha256:base".to_string()),
            base_snapshot: Some("sha256:base".to_string()),
            persist: true,
            freeze: true,
            report: None,
        })
        .unwrap_err();
        assert!(
            error.contains("--freeze is only valid for a workspace review"),
            "{error}"
        );
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
            include_dir: Vec::new(),
            tracked_only: false,
            include_untracked: false,
            exclude_dir: Vec::new(),
            max_file_size: None,
            max_depth: None,
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
                include_dir: Vec::new(),
                exclude_dir: Vec::new(),
                max_file_size: None,
                max_depth: None,
                tracked_only: false,
                include_untracked: false,
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
            .any(|entity| entity["qualified_name"]
                .as_str()
                .unwrap()
                .ends_with("::alpha")));
        assert!(refreshed["diff"]["entities"]["added"]["items"]
            .as_array()
            .unwrap()
            .iter()
            .any(|entity| entity["qualified_name"]
                .as_str()
                .unwrap()
                .ends_with("::beta")));
        let store = Store::open(&state).unwrap();
        assert_eq!(store.snapshot_count("project.refresh").unwrap(), 2);
    }
}
