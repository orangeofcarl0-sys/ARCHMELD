// Copyright 2026 ARCHMELD contributors
// SPDX-License-Identifier: Apache-2.0

//! AT027 slice: compile_commands-driven clangd for C/C++, two build variants,
//! macro definition lookup, virtual dispatch rename, and an indirect call that
//! must not be presented as a resolved target.

use archmeld_core::language::{preview_workspace_edit, PreviewRequest};
use archmeld_core::lsp::{file_uri, LspServerSpec, LspSession, SpawnPermission};
use archmeld_core::snapshot::{capture_snapshot, SnapshotInputs, SnapshotOptions};
use archmeld_core::store::Store;
use serde_json::{json, Value};
use std::fs;
use std::path::{Path, PathBuf};
use std::time::Duration;

const PROJECT: &str = "project.cpp";

fn clangd() -> Option<PathBuf> {
    let path = std::env::var("ARCHMELD_CLANGD")
        .unwrap_or_else(|_| "F:/Codex_Work_Space/_archmeld_tools/llvm18/bin/clangd.exe".into());
    let path = PathBuf::from(path);
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
    let index = text.find(needle).expect("needle present");
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
        changes
            .values()
            .any(|edits| edits.as_array().is_some_and(|list| !list.is_empty()))
    })
}

#[test]
fn clangd_uses_compile_commands_for_macros_virtuals_and_indirect_calls() {
    let Some(clangd) = clangd() else {
        eprintln!("clangd missing; skipping C/C++ pack");
        return;
    };
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().join("repo");
    copy_dir(
        &Path::new(env!("CARGO_MANIFEST_DIR")).join("../../adapters/languages/fixtures/c-cpp"),
        &root,
    );
    let variant_a = root.join("build-a");
    let mut store = Store::open(dir.path().join("state")).unwrap();
    store.ensure_project(PROJECT, "expedition").unwrap();
    let captured = capture_snapshot(
        &mut store,
        PROJECT,
        &root,
        &SnapshotInputs {
            build_config: json!({"kind": "c++", "variant": "a", "compile_commands": "build-a", "defines": ["FACTOR=2"]}),
            environment: json!({"os": "test", "compiler": "g++13"}),
        },
        &SnapshotOptions {
            include_untracked: true,
            ..SnapshotOptions::default()
        },
    )
    .unwrap();
    let captured_b = capture_snapshot(
        &mut store,
        PROJECT,
        &root,
        &SnapshotInputs {
            build_config: json!({"kind": "c++", "variant": "b", "compile_commands": "build-b", "defines": ["FACTOR=3"]}),
            environment: json!({"os": "test", "compiler": "g++13"}),
        },
        &SnapshotOptions {
            include_untracked: true,
            ..SnapshotOptions::default()
        },
    )
    .unwrap();
    let config_a = captured.snapshot["config_digest"].as_str().unwrap().to_string();
    let config_b = captured_b.snapshot["config_digest"].as_str().unwrap().to_string();
    assert_ne!(config_a, config_b, "build variants must not share a config digest");

    let session = LspSession::start(
        &LspServerSpec {
            id: "clangd".into(),
            program: clangd,
            args: vec![
                format!("--compile-commands-dir={}", variant_a.to_string_lossy()),
                "--background-index=false".into(),
                "--log=info".into(),
            ],
            cwd: root.clone(),
            env: Vec::new(),
            initialization_options: None,
        },
        &SpawnPermission::grant("grant.b012", "C/C++ language pack integration test"),
        &PathBuf::from("F:/Codex_Work_Space/_archmeld_temp/cpp-audit.ndjson"),
    )
    .expect("clangd start");
    session.initialize(&root).expect("initialize");
    for relative in ["src/shape.h", "src/circle.cpp", "src/main.cpp"] {
        let text = fs::read_to_string(root.join(relative)).unwrap();
        let language = if relative.ends_with(".h") { "cpp" } else { "cpp" };
        session
            .did_open(&root.join(relative), language, &text)
            .expect("didOpen");
    }

    let circle = fs::read_to_string(root.join("src/circle.cpp")).unwrap();
    let (line, character) = position(&circle, "area");
    let definition = session
        .request(
            "textDocument/definition",
            json!({
                "textDocument": {"uri": file_uri(&root.join("src/circle.cpp")).unwrap()},
                "position": {"line": line, "character": character}
            }),
            Duration::from_secs(20),
        )
        .unwrap_or(Value::Null);
    eprintln!("area definition: {}", definition);
    let expected_files = ["src/shape.h", "src/circle.cpp", "src/main.cpp"];
    let mut last_error = String::new();
    let mut preview = None;
    for _ in 0..20 {
        let edit = match session.rename(
            &root.join("src/circle.cpp"),
            line,
            character,
            "surface",
            Duration::from_secs(25),
        ) {
            Ok(value) if has_edits(&value) => value,
            Ok(value) => {
                last_error = format!("empty: {value}");
                std::thread::sleep(Duration::from_millis(1000));
                continue;
            }
            Err(error) => {
                last_error = error.to_string();
                std::thread::sleep(Duration::from_millis(1000));
                continue;
            }
        };
        let candidate = preview_workspace_edit(
            &store,
            &PreviewRequest {
                project_id: PROJECT,
                snapshot_id: &captured.snapshot_id,
                allowed_paths: &[],
                max_files: 8,
                workspace_root: Some(&root),
            },
            &edit,
        )
        .expect("preview");
        let files: Vec<String> = candidate.files.iter().map(|file| file.path.clone()).collect();
        if expected_files.iter().all(|expected| files.contains(&expected.to_string())) {
            preview = Some(candidate);
            break;
        }
        // The workspace edit arrived before clangd finished indexing every
        // translation unit. Retrying keeps the complete-file requirement.
        last_error = format!("incomplete rename preview: {files:?}");
        std::thread::sleep(Duration::from_millis(1000));
    }
    let preview = preview.expect(&format!("clangd returned no complete rename edits: {last_error}"));
    assert!(preview.complete(), "{:?}", preview.refusals);
    let files: Vec<String> = preview.files.iter().map(|file| file.path.clone()).collect();
    for expected in expected_files {
        assert!(files.contains(&expected.to_string()), "{files:?}");
    }

    let main = fs::read_to_string(root.join("src/main.cpp")).unwrap();
    let (macro_line, macro_character) = position(&main, "SCALE");
    let macro_definition = session
        .request(
            "textDocument/definition",
            json!({
                "textDocument": {"uri": file_uri(&root.join("src/main.cpp")).unwrap()},
                "position": {"line": macro_line, "character": macro_character}
            }),
            Duration::from_secs(20),
        )
        .unwrap_or(Value::Null)
        .to_string();
    assert!(macro_definition.contains("shape.h"), "macro definition not found: {macro_definition}");

    let (indirect_line, indirect_character) = position(&main, "fn(value)");
    let indirect = session
        .request(
            "textDocument/definition",
            json!({
                "textDocument": {"uri": file_uri(&root.join("src/main.cpp")).unwrap()},
                "position": {"line": indirect_line, "character": indirect_character}
            }),
            Duration::from_secs(20),
        )
        .unwrap_or(Value::Null)
        .to_string();
    assert!(
        !indirect.contains("circle.cpp") && !indirect.contains("scale_value"),
        "indirect call must not resolve to a fabricated target: {indirect}"
    );

    let summary = session.shutdown().expect("shutdown");
    assert!(summary.orphan_pids.is_empty(), "{:?}", summary.orphan_pids);
    assert_eq!(summary.exit_code, Some(0));
    if std::env::var("ARCHMELD_WRITE_EVIDENCE").ok().as_deref() == Some("1") {
        let payload = json!({
            "language": "c++",
            "server": "clangd 18.1.8",
            "snapshot_id": captured.snapshot_id,
            "config_digest_variant_a": config_a,
            "config_digest_variant_b": config_b,
            "compile_commands": ["build-a/compile_commands.json", "build-b/compile_commands.json"],
            "rename_files": files,
            "macro_definition_isa": "shape.h",
            "indirect_call_resolved": false,
            "process_audit": {"orphan_pids": summary.orphan_pids, "exit_code": summary.exit_code},
            "runtime_trace": "pending:B016",
            "build_verify": "pending:B022",
            "controlled_apply": "pending:B020/B021"
        });
        fs::write(
            Path::new(env!("CARGO_MANIFEST_DIR")).join("../../adapters/languages/evidence/c-cpp.json"),
            serde_json::to_string_pretty(&payload).unwrap(),
        )
        .unwrap();
    }
}
