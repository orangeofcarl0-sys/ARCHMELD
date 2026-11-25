// Copyright 2026 ARCHMELD contributors
// SPDX-License-Identifier: Apache-2.0

//! B011 language-pack checks: real Pyright and typescript-language-server
//! sessions, cross-file rename previews, alias imports, same-name isolation and
//! dynamic-dispatch resolution that must stay unknown.

use archmeld_core::language::{preview_workspace_edit, PreviewRequest};
use archmeld_core::lsp::{LspServerSpec, LspSession, SpawnPermission};
use archmeld_core::snapshot::{capture_snapshot, SnapshotInputs, SnapshotOptions};
use archmeld_core::store::Store;
use serde_json::{json, Value};
use std::fs;
use std::path::{Path, PathBuf};
use std::time::Duration;

const PROJECT: &str = "project.langpack";

fn tool(env: &str, default: &str) -> Option<PathBuf> {
    let path = std::env::var(env).unwrap_or_else(|_| default.to_string());
    let path = PathBuf::from(path);
    path.is_file().then_some(path)
}

fn node() -> Option<PathBuf> {
    if let Some(path) = tool("ARCHMELD_NODE", "C:/Program Files/nodejs/node.exe") {
        return Some(path);
    }
    std::env::var_os("PATH").and_then(|paths| {
        std::env::split_paths(&paths)
            .map(|dir| dir.join("node.exe"))
            .find(|candidate| candidate.is_file())
    })
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
    let line = before.matches('\n').count() as u64;
    let column = before.rsplit('\n').next().unwrap().encode_utf16().count() as u64;
    (line, column)
}

fn has_edits(edit: &Value) -> bool {
    edit["documentChanges"]
        .as_array()
        .is_some_and(|changes| changes.iter().any(|change| {
            change["edits"].as_array().is_some_and(|edits| !edits.is_empty())
        }))
        || edit["changes"].as_object().is_some_and(|changes| {
            changes.values().any(|edits| edits.as_array().is_some_and(|list| !list.is_empty()))
        })
}

struct PackResult {
    snapshot: String,
    rename_files: Vec<String>,
    dynamic_local_targets: Vec<String>,
    orphans: Vec<u32>,
    exit_code: Option<i32>,
    patch_bytes: usize,
}

#[allow(clippy::too_many_arguments)]
fn exercise(
    server_js: &Path,
    fixture: &str,
    files: &[(&str, &str)],
    rename_file: &str,
    rename_needle: &str,
    dynamic_file: &str,
    dynamic_needle: &str,
) -> PackResult {
    let node = node().expect("node available");
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().join("repo");
    copy_dir(
        &Path::new(env!("CARGO_MANIFEST_DIR")).join("../../adapters/languages/fixtures").join(fixture),
        &root,
    );
    let mut store = Store::open(dir.path().join("state")).unwrap();
    store.ensure_project(PROJECT, "expedition").unwrap();
    let captured = capture_snapshot(
        &mut store,
        PROJECT,
        &root,
        &SnapshotInputs {
            build_config: json!({"variant": fixture}),
            environment: json!({"os": "test"}),
        },
        &SnapshotOptions {
            include_untracked: true,
            ..SnapshotOptions::default()
        },
    )
    .unwrap();
    let session = LspSession::start(
        &LspServerSpec {
            id: fixture.to_string(),
            program: node,
            args: vec![server_js.to_string_lossy().to_string(), "--stdio".into()],
            cwd: root.clone(),
            env: Vec::new(),
            initialization_options: None,
        },
        &SpawnPermission::grant("grant.b011", "language pack integration test"),
        &dir.path().join("audit.ndjson"),
    )
    .expect("language server start");
    session.initialize(&root).expect("initialize");
    for (relative, language) in files {
        let text = fs::read_to_string(root.join(relative)).unwrap();
        session
            .did_open(&root.join(relative), language, &text)
            .expect("didOpen");
    }

    let rename_text = fs::read_to_string(root.join(rename_file)).unwrap();
    let (line, character) = position(&rename_text, rename_needle);
    let mut edit = Value::Null;
    for _ in 0..20 {
        match session.rename(&root.join(rename_file), line, character, "assist", Duration::from_secs(25)) {
            Ok(value) if has_edits(&value) => {
                edit = value;
                break;
            }
            Ok(_) | Err(_) => std::thread::sleep(Duration::from_millis(1000)),
        }
    }
    assert!(has_edits(&edit), "no rename edits for {fixture}");
    let preview = preview_workspace_edit(
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
    assert!(preview.complete(), "{fixture} refusals: {:?}", preview.refusals);
    let rename_files: Vec<String> = preview.files.iter().map(|file| file.path.clone()).collect();

    let dynamic_text = fs::read_to_string(root.join(dynamic_file)).unwrap();
    let (dynamic_line, dynamic_character) = position(&dynamic_text, dynamic_needle);
    let definition = session
        .request(
            "textDocument/definition",
            json!({
                "textDocument": {"uri": archmeld_core::lsp::file_uri(&root.join(dynamic_file)).unwrap()},
                "position": {"line": dynamic_line, "character": dynamic_character}
            }),
            Duration::from_secs(20),
        )
        .unwrap_or(Value::Null);
    let definition_text = definition.to_string();
    let dynamic_local_targets: Vec<String> = ["util.", "other."]
        .iter()
        .filter(|needle| definition_text.contains(&format!("/{needle}")))
        .map(|needle| (*needle).to_string())
        .collect();

    let summary = session.shutdown().expect("shutdown");
    PackResult {
        snapshot: captured.snapshot_id,
        rename_files,
        dynamic_local_targets,
        orphans: summary.orphan_pids,
        exit_code: summary.exit_code,
        patch_bytes: preview.patch.len(),
    }
}

fn evidence(name: &str, language: &str, server: &str, result: &PackResult) {
    if std::env::var("ARCHMELD_WRITE_EVIDENCE").ok().as_deref() != Some("1") {
        return;
    }
    let path = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../adapters/languages/evidence")
        .join(format!("{name}.json"));
    let payload = json!({
        "language": language,
        "server": server,
        "snapshot_id": result.snapshot,
        "rename_files": result.rename_files,
        "patch_bytes": result.patch_bytes,
        "dynamic_dispatch_local_targets": result.dynamic_local_targets,
        "process_audit": {"orphan_pids": result.orphans, "exit_code": result.exit_code},
        "runtime_linkage": "pending:B016",
        "controlled_apply": "pending:B020/B021",
        "generated_by": "cargo test -p archmeld-core --test language_packs",
    });
    fs::write(path, serde_json::to_string_pretty(&payload).unwrap()).unwrap();
}

#[test]
fn python_alias_imports_same_names_and_dynamic_getattr() {
    let Some(server) = tool(
        "ARCHMELD_PYRIGHT",
        "<workspace>/_archmeld_tools/lang-servers/node_modules/pyright/langserver.index.js",
    ) else {
        eprintln!("pyright missing; skipping Python pack");
        return;
    };
    let result = exercise(
        &server,
        "python",
        &[
            ("pkg/util.py", "python"),
            ("pkg/other.py", "python"),
            ("app.py", "python"),
        ],
        "pkg/util.py",
        "helper",
        "app.py",
        "getattr",
    );
    assert!(result.rename_files.contains(&"pkg/util.py".to_string()));
    assert!(result.rename_files.contains(&"app.py".to_string()), "{:?}", result.rename_files);
    assert!(
        !result.rename_files.contains(&"pkg/other.py".to_string()),
        "same-name function in another module must not be renamed: {:?}",
        result.rename_files
    );
    assert!(result.dynamic_local_targets.is_empty(), "{:?}", result.dynamic_local_targets);
    assert!(result.orphans.is_empty());
    evidence("python", "python", "pyright 1.1.408", &result);
}

#[test]
fn typescript_alias_imports_same_names_and_dynamic_index() {
    let Some(server) = tool(
        "ARCHMELD_TS_SERVER",
        "<workspace>/_archmeld_tools/lang-servers/node_modules/typescript-language-server/lib/cli.mjs",
    ) else {
        eprintln!("typescript-language-server missing; skipping TypeScript pack");
        return;
    };
    let result = exercise(
        &server,
        "typescript",
        &[
            ("src/util.ts", "typescript"),
            ("src/other.ts", "typescript"),
            ("src/app.ts", "typescript"),
        ],
        "src/util.ts",
        "helper",
        "src/app.ts",
        "obj[key]",
    );
    assert!(result.rename_files.contains(&"src/util.ts".to_string()));
    assert!(result.rename_files.contains(&"src/app.ts".to_string()), "{:?}", result.rename_files);
    assert!(
        !result.rename_files.contains(&"src/other.ts".to_string()),
        "same-name export in another file must not be renamed: {:?}",
        result.rename_files
    );
    assert!(
        !result
            .dynamic_local_targets
            .iter()
            .any(|target| target == "util." || target == "other."),
        "dynamic dispatch must not resolve to a project target: {:?}",
        result.dynamic_local_targets
    );
    assert!(result.orphans.is_empty());
    evidence("typescript", "typescript", "typescript-language-server 4.3.4", &result);
}

#[test]
fn javascript_alias_imports_same_names_and_dynamic_index() {
    let Some(server) = tool(
        "ARCHMELD_TS_SERVER",
        "<workspace>/_archmeld_tools/lang-servers/node_modules/typescript-language-server/lib/cli.mjs",
    ) else {
        eprintln!("typescript-language-server missing; skipping JavaScript pack");
        return;
    };
    let result = exercise(
        &server,
        "javascript",
        &[
            ("src/util.js", "javascript"),
            ("src/other.js", "javascript"),
            ("src/app.js", "javascript"),
        ],
        "src/util.js",
        "helper",
        "src/app.js",
        "obj[key]",
    );
    assert!(result.rename_files.contains(&"src/util.js".to_string()));
    assert!(
        result.rename_files.contains(&"src/app.js".to_string()),
        "{:?}",
        result.rename_files
    );
    assert!(
        !result.rename_files.contains(&"src/other.js".to_string()),
        "same-name export in another file must not be renamed: {:?}",
        result.rename_files
    );
    assert!(
        !result
            .dynamic_local_targets
            .iter()
            .any(|target| target == "util." || target == "other."),
        "dynamic dispatch must not resolve to a project target: {:?}",
        result.dynamic_local_targets
    );
    assert!(result.orphans.is_empty());
    evidence(
        "javascript",
        "javascript",
        "typescript-language-server 4.3.4",
        &result,
    );
}
