// Copyright 2026 ARCHMELD contributors
// SPDX-License-Identifier: Apache-2.0

//! AT032 slice: rust-analyzer over a real crate with trait dispatch, cfg build
//! variants, a cross-module macro, same-name functions and a function pointer.
//! Trace, controlled apply and full build verification remain B016/B021/B022.

use archmeld_core::language::{preview_workspace_edit, PreviewRequest};
use archmeld_core::lsp::{file_uri, LspServerSpec, LspSession, SpawnPermission};
use archmeld_core::snapshot::{capture_snapshot, SnapshotInputs, SnapshotOptions};
use archmeld_core::store::Store;
use serde_json::{json, Value};
use std::fs;
use std::path::{Path, PathBuf};
use std::time::Duration;

const PROJECT: &str = "project.rustpack";

fn definition_until(
    session: &LspSession,
    root: &Path,
    relative: &str,
    text: &str,
    needle: &str,
    expected: &str,
) -> String {
    let (line, character) = position(text, needle);
    let uri = file_uri(&root.join(relative)).unwrap();
    let mut last = String::new();
    for _ in 0..40 {
        let value = session
            .request(
                "textDocument/definition",
                json!({
                    "textDocument": {"uri": uri},
                    "position": {"line": line, "character": character}
                }),
                Duration::from_secs(20),
            )
            .unwrap_or(Value::Null);
        last = value.to_string();
        if last.contains(expected) {
            return last;
        }
        std::thread::sleep(Duration::from_millis(500));
    }
    last
}

fn rust_analyzer() -> Option<PathBuf> {
    let home = std::env::var_os("USERPROFILE")?;
    let path = PathBuf::from(home)
        .join(".rustup/toolchains/1.94.1-x86_64-pc-windows-gnu/bin/rust-analyzer.exe");
    path.is_file().then_some(path)
}

fn copy_dir(source: &Path, destination: &Path) {
    fs::create_dir_all(destination).unwrap();
    for entry in fs::read_dir(source).unwrap() {
        let entry = entry.unwrap();
        let target = destination.join(entry.file_name());
        if entry.file_type().unwrap().is_dir() {
            copy_dir(&entry.path(), &target);
        } else {
            fs::copy(entry.path(), target).unwrap();
        }
    }
}

fn position(text: &str, needle: &str) -> (u64, u64) {
    let index = text.find(needle).expect("needle");
    let before = &text[..index];
    (
        before.matches('\n').count() as u64,
        before.rsplit('\n').next().unwrap().encode_utf16().count() as u64,
    )
}

fn has_edits(edit: &Value) -> bool {
    edit["documentChanges"].as_array().is_some_and(|changes| {
        changes.iter().any(|change| {
            change["edits"].as_array().is_some_and(|edits| !edits.is_empty())
        })
    }) || edit["changes"].as_object().is_some_and(|changes| {
        changes.values().any(|edits| edits.as_array().is_some_and(|list| !list.is_empty()))
    })
}

fn cargo_check(root: &Path, features: &[&str]) -> i32 {
    let cargo = PathBuf::from(std::env::var_os("USERPROFILE").unwrap())
        .join(".rustup/toolchains/1.94.1-x86_64-pc-windows-gnu/bin/cargo.exe");
    let mut command = std::process::Command::new(cargo);
    command.arg("check").arg("--offline").current_dir(root);
    if !features.is_empty() {
        command.arg("--features").arg(features.join(","));
    }
    command.status().map(|status| status.code().unwrap_or(-1)).unwrap_or(-1)
}

#[test]
fn rust_analyzer_trait_cfg_macro_and_function_pointer() {
    let Some(server) = rust_analyzer() else {
        eprintln!("rust-analyzer missing; skipping Rust pack");
        return;
    };
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().join("repo");
    copy_dir(
        &Path::new(env!("CARGO_MANIFEST_DIR")).join("../../adapters/languages/fixtures/rust"),
        &root,
    );
    let mut store = Store::open(dir.path().join("state")).unwrap();
    store.ensure_project(PROJECT, "expedition").unwrap();
    let default_snapshot = capture_snapshot(
        &mut store,
        PROJECT,
        &root,
        &SnapshotInputs {
            build_config: json!({"kind": "rust", "features": [], "profile": "dev"}),
            environment: json!({"os": "test", "toolchain": "1.94.1"}),
        },
        &SnapshotOptions {
            include_untracked: true,
            ..SnapshotOptions::default()
        },
    )
    .unwrap();
    let fast_snapshot = capture_snapshot(
        &mut store,
        PROJECT,
        &root,
        &SnapshotInputs {
            build_config: json!({"kind": "rust", "features": ["fast"], "profile": "dev"}),
            environment: json!({"os": "test", "toolchain": "1.94.1"}),
        },
        &SnapshotOptions {
            include_untracked: true,
            ..SnapshotOptions::default()
        },
    )
    .unwrap();
    assert_ne!(
        default_snapshot.snapshot["config_digest"],
        fast_snapshot.snapshot["config_digest"],
        "cfg feature variants must be distinct"
    );
    assert_eq!(cargo_check(&root, &[]), 0);
    assert_eq!(cargo_check(&root, &["fast"]), 0);

    let session = LspSession::start(
        &LspServerSpec {
            id: "rust-analyzer".into(),
            program: server,
            args: Vec::new(),
            cwd: root.clone(),
            env: Vec::new(),
            initialization_options: None,
        },
        &SpawnPermission::grant("grant.b014", "Rust language pack test"),
        &PathBuf::from("F:/Codex_Work_Space/_archmeld_temp/rust-audit.ndjson"),
    )
    .expect("rust-analyzer start");
    session.initialize(&root).expect("initialize");
    for relative in ["src/lib.rs", "src/util.rs", "src/other.rs"] {
        let text = fs::read_to_string(root.join(relative)).unwrap();
        session.did_open(&root.join(relative), "rust", &text).expect("didOpen");
    }

    let util = fs::read_to_string(root.join("src/util.rs")).unwrap();
    let (line, character) = position(&util, "helper");
    let mut edit = Value::Null;
    let mut last_error = String::new();
    for _ in 0..60 {
        match session.rename(&root.join("src/util.rs"), line, character, "assist", Duration::from_secs(25)) {
            Ok(value) if has_edits(&value) => {
                edit = value;
                break;
            }
            Ok(value) => last_error = format!("empty: {value}"),
            Err(error) => last_error = error.to_string(),
        }
        std::thread::sleep(Duration::from_millis(1000));
    }
    assert!(has_edits(&edit), "no rename edits: {last_error}");
    let preview = preview_workspace_edit(
        &store,
        &PreviewRequest {
            project_id: PROJECT,
            snapshot_id: &default_snapshot.snapshot_id,
            allowed_paths: &[],
            max_files: 8,
            workspace_root: Some(&root),
        },
        &edit,
    )
    .expect("preview");
    assert!(preview.complete(), "{:?}", preview.refusals);
    let files: Vec<String> = preview.files.iter().map(|file| file.path.clone()).collect();
    assert!(files.contains(&"src/util.rs".to_string()));
    assert!(files.contains(&"src/lib.rs".to_string()));
    assert!(!files.contains(&"src/other.rs".to_string()), "{files:?}");

    let lib = fs::read_to_string(root.join("src/lib.rs")).unwrap();
    let macro_def = definition_until(&session, &root, "src/lib.rs", &lib, "scale!", "util.rs");
    assert!(macro_def.contains("util.rs"), "macro definition not mapped: {macro_def}");

    let trait_def = definition_until(&session, &root, "src/lib.rs", &lib, "run()", "lib.rs");
    assert!(
        trait_def.contains("lib.rs"),
        "trait method definition not mapped: {trait_def}"
    );
    let fn_def = definition_until(&session, &root, "src/lib.rs", &lib, "f(value)", "lib.rs");
    assert!(
        fn_def.contains("lib.rs") && !fn_def.contains("util.rs") && !fn_def.contains("other.rs"),
        "function pointer must not resolve to a fabricated target: {fn_def}"
    );

    let summary = session.shutdown().expect("shutdown");
    assert!(summary.orphan_pids.is_empty());
    assert_eq!(summary.exit_code, Some(0));
    if std::env::var("ARCHMELD_WRITE_EVIDENCE").ok().as_deref() == Some("1") {
        let payload = json!({
            "language": "rust",
            "server": "rust-analyzer 1.94.1",
            "snapshot_id": default_snapshot.snapshot_id,
            "config_digest_default": default_snapshot.snapshot["config_digest"],
            "config_digest_fast": fast_snapshot.snapshot["config_digest"],
            "cargo_check": {"default": 0, "fast": 0},
            "rename_files": files,
            "macro_definition_contains_util_rs": true,
            "trait_definition": trait_def,
            "function_pointer_resolved": false,
            "process_audit": {"orphan_pids": summary.orphan_pids, "exit_code": summary.exit_code},
            "runtime_trace": "pending:B016",
            "controlled_apply": "pending:B020/B021",
            "build_verify": "pending:B022"
        });
        fs::write(
            Path::new(env!("CARGO_MANIFEST_DIR")).join("../../adapters/languages/evidence/rust.json"),
            serde_json::to_string_pretty(&payload).unwrap(),
        )
        .unwrap();
    }
}
