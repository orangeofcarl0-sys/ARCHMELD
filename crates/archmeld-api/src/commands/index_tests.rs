// Copyright 2026 ARCHMELD contributors
// SPDX-License-Identifier: Apache-2.0

//! CLI tests for index, refresh and status.

use super::*;

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
            include_dir: Vec::new(),
            tracked_only: false,
            include_untracked: false,
            exclude_dir: Vec::new(),
            max_file_size: None,
            max_depth: None,
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
        let registered_root = store
            .active_project_root("project.index.race")
            .unwrap()
            .expect("index must register the workspace root for MCP status/review");
        assert_eq!(
            registered_root.to_string_lossy().replace('\\', "/"),
            live.to_string_lossy().replace('\\', "/")
        );
        let status = status_report(&store, Some("project.index.race"), Some(&live)).unwrap();
        assert_eq!(
            status["projects"][0]["freshness"]["state"], "changed",
            "the registered root must let status see the concurrent save"
        );
        let entity_ids = store.entity_ids("project.index.race", snapshot_id).unwrap();
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

    #[test]
    fn index_prefers_the_manifest_index_block_over_helper_stdout() {
        let dir = tempfile::tempdir().unwrap();
        let workspace = dir.path().join("repo");
        fs::create_dir_all(workspace.join("src")).unwrap();
        fs::write(workspace.join("src/a.py"), "def a():\n    return 1\n").unwrap();
        let state = dir.path().join("state");
        let mut runner = |_helper: &PathBuf,
                          analysis: &std::path::Path,
                          export: &std::path::Path,
                          _state: &std::path::Path|
         -> Result<HelperOutput, String> {
            let manifest = json!({
                "type":"manifest","format":"archmeld-export-v1","source":"codegraph",
                "schema_version":"1.2","version":"0.2.0","node_count":1,"edge_count":0,
                "iteration":"real-iterators",
                "index":{
                    "files_total":1,"files_parsed":1,"files_skipped":0,
                    "by_language":{"python":1},
                    "skipped_by_reason":{"excluded_dir":2,"too_large":1},
                    "parser_errors":[{
                        "path":"src/broken.kt","language":"kotlin","message":"bad"
                    }],
                    "max_files_reached":false,"max_depth_reached":true,
                    "cross_file_implements":{"resolved":3,"ambiguous":0}
                }
            });
            let node = json!({
                "type":"node","id":1,"node_type":"Function","properties":{
                    "path": analysis.join("src/a.py").to_string_lossy(),
                    "name":"a","line_start":1,"line_end":2
                }
            });
            fs::write(export, format!("{manifest}\n{node}\n"))
                .map_err(|error| format!("helper export: {error}"))?;
            // Deliberately contradict the manifest: stdout says 999 files and
            // a count-only parser error map. The ledger must follow the
            // manifest, which carries paths.
            let summary = json!({
                "format":"archmeld-export-v1","nodes":1,"edges":0,
                "files_total":999,"files_parsed":999,"files_skipped":0,
                "by_language":{"python":999},"parser_errors":{"kotlin":9},
                "elapsed_ms":1,"storage":"in_memory"
            });
            Ok(HelperOutput {
                success: true,
                status: Some(0),
                stdout: serde_json::to_vec(&summary).unwrap(),
                stderr: Vec::new(),
            })
        };
        let args = IndexArgs {
            workspace,
            state: state.clone(),
            project: "project.index.manifest".to_string(),
            max_files: 500,
            index_helper: None,
            run_id: Some("run.index.manifest".to_string()),
            include_dir: Vec::new(),
            tracked_only: false,
            include_untracked: false,
            exclude_dir: Vec::new(),
            max_file_size: None,
            max_depth: None,
        };
        let result = run_index_with(args, &dir.path().join("fake-helper.exe"), &mut runner)
            .expect("index run");
        let snapshot_id = result["snapshot_id"].as_str().unwrap();
        let store = Store::open(&state).unwrap();
        let run = store
            .latest_index_run("project.index.manifest", snapshot_id)
            .unwrap()
            .expect("index run recorded");
        assert_eq!(run["parse"]["files_total"], 1, "{run}");
        assert_eq!(run["parse"]["skipped_by_reason"]["too_large"], 1, "{run}");
        assert_eq!(
            run["parse"]["parser_errors"][0]["path"], "src/broken.kt",
            "{run}"
        );
        assert_eq!(run["parse"]["max_depth_reached"], true, "{run}");
        assert_eq!(
            run["parse"]["cross_file_implements"]["resolved"], 3,
            "{run}"
        );
        assert_eq!(run["export_schema"], "1.2", "{run}");
    }

    #[test]
    fn clean_workspace_reuses_the_indexed_snapshot() {
        let dir = tempfile::tempdir().unwrap();
        let workspace = dir.path().join("repo");
        fs::create_dir_all(workspace.join("src")).unwrap();
        fs::write(workspace.join("src/a.py"), "def a():\n    return 1\n").unwrap();
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
                    "name":"a","line_start":1,"line_end":2
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
        let args = || IndexArgs {
            workspace: workspace.clone(),
            state: state.clone(),
            project: "project.index.reuse".to_string(),
            max_files: 500,
            index_helper: None,
            run_id: Some("run.index.reuse.1".to_string()),
            include_dir: Vec::new(),
            tracked_only: false,
            include_untracked: false,
            exclude_dir: Vec::new(),
            max_file_size: None,
            max_depth: None,
        };
        let first = run_index_with(args(), &dir.path().join("fake-helper.exe"), &mut runner)
            .expect("index run");
        let snapshot_id = first["snapshot_id"].as_str().unwrap().to_string();

        let reused = existing_clean_index(&state, "project.index.reuse", &workspace)
            .unwrap()
            .expect("a clean workspace should reuse its snapshot");
        assert_eq!(reused["snapshot_id"], snapshot_id);
        assert_eq!(reused["index_reuse"], true);

        fs::write(workspace.join("src/a.py"), "def a():\n    return 2\n").unwrap();
        assert!(
            existing_clean_index(&state, "project.index.reuse", &workspace)
                .unwrap()
                .is_none(),
            "a dirty workspace must be re-indexed"
        );
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
        store
            .ensure_project("project.status", "expedition")
            .unwrap();
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
