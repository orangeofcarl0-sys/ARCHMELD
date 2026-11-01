// Copyright 2026 ARCHMELD contributors
// SPDX-License-Identifier: Apache-2.0

//! AT023 (native refactor preview) and AT024 (language-server execution
//! permission) at the broker boundary. No language server is started here.

use archmeld_core::language::{
    decide, preview_workspace_edit, BrokerRequest, Decision, ExecutionPolicy, PreviewRequest,
    Registry,
};
use archmeld_core::snapshot::{capture_snapshot, SnapshotInputs, SnapshotOptions};
use archmeld_core::store::Store;
use serde_json::{json, Value};
use std::fs;
use std::path::Path;

const PROJECT: &str = "project.broker";

struct Fixture {
    _dir: tempfile::TempDir,
    store: Store,
    root: std::path::PathBuf,
    snapshot_id: String,
}

fn fixture() -> Fixture {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().join("repo");
    fs::create_dir_all(root.join("src")).unwrap();
    fs::write(root.join("src/app.py"), "中文 = \"😀\"\ncompute(中文)\n").unwrap();
    fs::write(root.join("other.py"), "compute()\n").unwrap();
    let mut store = Store::open(dir.path().join("state")).unwrap();
    store.ensure_project(PROJECT, "expedition").unwrap();
    let captured = capture_snapshot(
        &mut store,
        PROJECT,
        &root,
        &SnapshotInputs {
            build_config: json!({"variant": "broker"}),
            environment: json!({"os": "test"}),
        },
        &SnapshotOptions {
            include_untracked: true,
            ..SnapshotOptions::default()
        },
    )
    .unwrap();
    Fixture {
        _dir: dir,
        store,
        root,
        snapshot_id: captured.snapshot_id,
    }
}

fn preview_request<'a>(fx: &'a Fixture, allowed: &'a [String]) -> PreviewRequest<'a> {
    PreviewRequest {
        project_id: PROJECT,
        snapshot_id: &fx.snapshot_id,
        allowed_paths: allowed,
        max_files: 8,
        workspace_root: None,
    }
}

fn edit(range: (u64, u64, u64, u64), text: &str) -> Value {
    json!({
        "range": {
            "start": {"line": range.0, "character": range.1},
            "end": {"line": range.2, "character": range.3}
        },
        "newText": text
    })
}

#[test]
fn preview_converts_utf16_ranges_and_never_writes() {
    let fixture = fixture();
    let before_app = fs::read(fixture.root.join("src/app.py")).unwrap();
    let before_other = fs::read(fixture.root.join("other.py")).unwrap();
    let edit_set = json!({
        "position_encoding": "utf-16",
        "changes": {
            "src/app.py": [
                // Replace 中文 at UTF-16 columns 8..10 on line 1.
                edit((1, 8, 1, 10), "计算"),
                // Rename compute at columns 0..7 on line 1.
                edit((1, 0, 1, 7), "total")
            ],
            "other.py": [edit((0, 0, 0, 7), "total")]
        }
    });
    let allowed = vec!["src/".to_string(), "other.py".to_string()];
    let preview = preview_workspace_edit(&fixture.store, &preview_request(&fixture, &allowed), &edit_set)
        .expect("preview");
    assert!(preview.complete(), "{:?}", preview.refusals);
    assert_eq!(preview.files.len(), 2);
    assert_eq!(preview.total_edits, 3);
    assert!(preview.patch.contains("计算"));
    assert!(preview.patch.contains("total"));
    assert!(preview.files.iter().all(|file| file.before_digest != file.after_digest));
    assert_eq!(fs::read(fixture.root.join("src/app.py")).unwrap(), before_app);
    assert_eq!(fs::read(fixture.root.join("other.py")).unwrap(), before_other);
    let payload = preview.to_json();
    assert_eq!(payload["writes_applied"], false);
}

#[test]
fn out_of_scope_edits_abort_the_whole_preview() {
    let fixture = fixture();
    let edit_set = json!({
        "changes": {
            "src/app.py": [edit((1, 0, 1, 7), "total")],
            "other.py": [edit((0, 0, 0, 7), "total")]
        }
    });
    let allowed = vec!["src/".to_string()];
    let preview = preview_workspace_edit(&fixture.store, &preview_request(&fixture, &allowed), &edit_set)
        .expect("preview");
    assert!(!preview.complete());
    assert!(preview.files.is_empty(), "a partial patch must not be presented");
    assert!(preview.patch.is_empty());
    assert!(preview.refusals.iter().any(|refusal| refusal.code == "scope_violation"));
}

#[test]
fn overlapping_ranges_resource_ops_and_absolute_uris_are_refused() {
    let fixture = fixture();
    let allowed: Vec<String> = Vec::new();
    let overlapping = json!({
        "changes": {
            "src/app.py": [edit((1, 0, 1, 7), "x"), edit((1, 3, 1, 5), "y")]
        }
    });
    let preview =
        preview_workspace_edit(&fixture.store, &preview_request(&fixture, &allowed), &overlapping)
            .expect("preview");
    assert!(preview.refusals.iter().any(|refusal| refusal.code == "overlapping_edits"));

    let out_of_range = json!({
        "changes": {"src/app.py": [edit((99, 0, 99, 1), "x")]}
    });
    let preview =
        preview_workspace_edit(&fixture.store, &preview_request(&fixture, &allowed), &out_of_range)
            .expect("preview");
    assert!(preview.refusals.iter().any(|refusal| refusal.code == "invalid_range"));

    let resource = json!({
        "documentChanges": [{
            "kind": "create",
            "uri": "file:///C:/tmp/new.py"
        }]
    });
    let preview =
        preview_workspace_edit(&fixture.store, &preview_request(&fixture, &allowed), &resource)
            .expect("preview");
    assert!(preview
        .refusals
        .iter()
        .any(|refusal| refusal.code == "resource_operation_not_supported"));

    let absolute = json!({
        "changes": {"file:///C:/tmp/app.py": [edit((0, 0, 0, 1), "x")]}
    });
    let preview =
        preview_workspace_edit(&fixture.store, &preview_request(&fixture, &allowed), &absolute)
            .expect("preview");
    assert!(preview
        .refusals
        .iter()
        .any(|refusal| refusal.code == "absolute_uri_not_authorized"));
}

#[test]
fn contract_profiles_never_grant_unverified_capabilities() {
    let contract = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../docs/archmeld/contracts/language-profiles.json");
    let registry = Registry::from_contract_json(&fs::read_to_string(contract).unwrap()).unwrap();
    let python = registry.profile("python").expect("python profile");
    assert!(!python.supports("definition_reference"));
    assert_eq!(
        decide(python, BrokerRequest::Definition, &ExecutionPolicy::default()),
        Decision::Deny {
            code: "unsupported_capability",
            reason: "profile python has no verified definition/reference capability".into()
        }
    );
    assert!(matches!(
        decide(python, BrokerRequest::RenamePreview, &ExecutionPolicy::default()),
        Decision::Deny { code: "unsupported_capability", .. }
    ));
}

#[test]
fn execution_commands_need_permission_and_are_never_automatic() {
    let registry = Registry::from_contract_json(
        r#"{"profiles":[{"id":"verified","languages":["python"],"capabilities":{
            "definition_reference":"verified","bounded_refactor":"verified"}}]}"#,
    )
    .unwrap();
    let profile = registry.profile("verified").unwrap();
    assert_eq!(
        decide(profile, BrokerRequest::Definition, &ExecutionPolicy::default()),
        Decision::Allow
    );
    assert_eq!(
        decide(profile, BrokerRequest::RenamePreview, &ExecutionPolicy::default()),
        Decision::Allow
    );
    assert_eq!(
        decide(profile, BrokerRequest::ExecuteCommand, &ExecutionPolicy::default()),
        Decision::Deny {
            code: "execution_not_permitted",
            reason: "executeCommand is disabled; no language-server command is run automatically".into()
        }
    );
    assert!(matches!(
        decide(
            profile,
            BrokerRequest::ExecuteCommand,
            &ExecutionPolicy {
                allow_read_only: true,
                execute_command_permitted: true
            }
        ),
        Decision::NeedsPermission { capability: "execute_command", .. }
    ));
    assert!(matches!(
        decide(profile, BrokerRequest::InstallTool, &ExecutionPolicy::default()),
        Decision::Deny { code: "auto_install_forbidden", .. }
    ));
    assert!(matches!(
        decide(profile, BrokerRequest::NetworkFetch, &ExecutionPolicy::default()),
        Decision::Deny { code: "network_denied", .. }
    ));
}
