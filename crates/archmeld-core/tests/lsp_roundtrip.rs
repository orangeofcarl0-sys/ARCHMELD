// Copyright 2026 ARCHMELD contributors
// SPDX-License-Identifier: Apache-2.0

//! Real language-server round-trip (AT023) with explicit spawn permission and
//! a process audit (AT024). Uses the pinned rust-analyzer binary; skips only
//! when that binary is absent.

use archmeld_core::language::{preview_workspace_edit, PreviewRequest};
use archmeld_core::lsp::{file_uri, LspServerSpec, LspSession, SpawnPermission};
use archmeld_core::snapshot::{capture_snapshot, SnapshotInputs, SnapshotOptions};
use archmeld_core::store::Store;
use serde_json::{json, Value};
use std::fs;
use std::path::{Path, PathBuf};
use std::time::Duration;

const PROJECT: &str = "project.lsp";

fn rust_analyzer() -> Option<PathBuf> {
    if let Ok(path) = std::env::var("RUST_ANALYZER") {
        let path = PathBuf::from(path);
        if path.is_file() {
            return Some(path);
        }
    }
    let home = std::env::var_os("USERPROFILE").or_else(|| std::env::var_os("HOME"))?;
    let candidate = PathBuf::from(home)
        .join(".rustup/toolchains/1.94.1-x86_64-pc-windows-gnu/bin/rust-analyzer.exe");
    candidate.is_file().then_some(candidate)
}

fn copy_fixture(destination: &Path) {
    let source = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../upstream-probes/scip/rustdemo");
    fs::create_dir_all(destination.join("src")).unwrap();
    fs::copy(source.join("Cargo.toml"), destination.join("Cargo.toml")).unwrap();
    fs::copy(source.join("src/lib.rs"), destination.join("src/lib.rs")).unwrap();
}

fn spec(program: &Path, root: &Path) -> LspServerSpec {
    let toolchain_bin = program.parent().unwrap().to_path_buf();
    let path = std::env::var("PATH").unwrap_or_default();
    LspServerSpec {
        id: "rust-analyzer".into(),
        program: program.to_path_buf(),
        args: Vec::new(),
        cwd: root.to_path_buf(),
        env: vec![(
            "PATH".into(),
            format!("{};{}", toolchain_bin.display(), path),
        )],
        initialization_options: None,
    }
}

#[test]
fn spawn_requires_an_explicit_grant_and_is_audited() {
    let Some(program) = rust_analyzer() else {
        eprintln!("rust-analyzer missing; skipping");
        return;
    };
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().join("repo");
    copy_fixture(&root);
    let audit = dir.path().join("audit.ndjson");
    let err = match LspSession::start(
        &spec(&program, &root),
        &SpawnPermission::deny("test: no permission presented"),
        &audit,
    ) {
        Ok(_) => panic!("ungranted spawn must be refused"),
        Err(error) => error,
    };
    assert!(err.to_string().contains("permission_denied"), "{err}");
    let log = fs::read_to_string(&audit).unwrap();
    assert!(log.contains("spawn_denied"));
    assert!(!log.contains("\"pid\""), "denied spawn must not report a pid");
}

#[test]
fn real_rename_roundtrip_feeds_the_preview_and_leaves_no_orphans() {
    let Some(program) = rust_analyzer() else {
        eprintln!("rust-analyzer missing; skipping");
        return;
    };
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().join("repo");
    copy_fixture(&root);
    let source_path = root.join("src/lib.rs");
    let source = fs::read_to_string(&source_path).unwrap();
    let mut store = Store::open(dir.path().join("state")).unwrap();
    store.ensure_project(PROJECT, "expedition").unwrap();
    let captured = capture_snapshot(
        &mut store,
        PROJECT,
        &root,
        &SnapshotInputs {
            build_config: json!({"variant": "lsp"}),
            environment: json!({"os": "test"}),
        },
        &SnapshotOptions {
            include_untracked: true,
            ..SnapshotOptions::default()
        },
    )
    .unwrap();
    let before = fs::read(&source_path).unwrap();
    let audit = dir.path().join("audit.ndjson");
    let session = LspSession::start(
        &spec(&program, &root),
        &SpawnPermission::grant("grant.at023", "interactive test grant"),
        &audit,
    )
    .expect("granted spawn");
    session.initialize(&root).expect("initialize");
    session
        .did_open(&source_path, "rust", &source)
        .expect("didOpen");

    let mut edit = Value::Null;
    let mut last_error = String::new();
    for _ in 0..20 {
        match session.rename(&source_path, 5, 4, "assist", Duration::from_secs(20)) {
            Ok(value) if has_edits(&value) => {
                edit = value;
                break;
            }
            Ok(value) => {
                edit = value;
                last_error = "empty WorkspaceEdit".into();
            }
            Err(error) => last_error = error.to_string(),
        }
        std::thread::sleep(Duration::from_millis(1000));
    }
    assert!(
        has_edits(&edit),
        "rust-analyzer returned no rename edits: {last_error}; audit={}",
        audit.display()
    );

    let request = PreviewRequest {
        project_id: PROJECT,
        snapshot_id: &captured.snapshot_id,
        allowed_paths: &[],
        max_files: 8,
        workspace_root: Some(&root),
    };
    let preview = preview_workspace_edit(&store, &request, &edit).expect("preview");
    assert!(preview.complete(), "refusals: {:?}", preview.refusals);
    assert!(preview.patch.contains("assist"), "{}", preview.patch);
    assert_eq!(
        fs::read(&source_path).unwrap(),
        before,
        "a preview must not modify the workspace"
    );

    let summary = session.shutdown().expect("shutdown");
    assert!(summary.audit_available, "process audit unavailable");
    assert!(
        summary.orphan_pids.is_empty(),
        "orphan language-server processes: {:?}",
        summary.orphan_pids
    );
    assert_eq!(summary.exit_code, Some(0), "clean LSP exit expected");
    let log = fs::read_to_string(&audit).unwrap();
    assert!(log.contains("\"event\":\"spawn\""));
    assert!(log.contains("\"event\":\"rename\""));
    assert!(log.contains("\"event\":\"shutdown\""));
    assert!(log.contains("\"orphan_pids\":[]"));
    assert!(!log.contains("grant.at023\":null"));
}

fn has_edits(edit: &Value) -> bool {
    if let Some(changes) = edit["changes"].as_object() {
        if changes.values().any(|edits| edits.as_array().is_some_and(|list| !list.is_empty())) {
            return true;
        }
    }
    if let Some(document_changes) = edit["documentChanges"].as_array() {
        if document_changes.iter().any(|change| {
            change["edits"]
                .as_array()
                .is_some_and(|edits| !edits.is_empty())
        }) {
            return true;
        }
    }
    false
}
