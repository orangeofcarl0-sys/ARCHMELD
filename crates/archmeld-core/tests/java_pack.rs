// Copyright 2026 ARCHMELD contributors
// SPDX-License-Identifier: Apache-2.0

//! Real jdtls pack evidence: overload resolution, interface dispatch,
//! annotation navigation, reflection not being guessed, and a cross-file rename
//! that leaves same-named methods in other classes and a reflective string
//! literal untouched.

use archmeld_core::language::{preview_workspace_edit, PreviewRequest};
use archmeld_core::lsp::{file_uri, LspServerSpec, LspSession, SpawnPermission};
use archmeld_core::snapshot::{capture_snapshot, SnapshotInputs, SnapshotOptions};
use archmeld_core::store::Store;
use serde_json::{json, Value};
use std::fs;
use std::path::{Path, PathBuf};
use std::time::Duration;

const P: &str = "project.javapack";

fn copy(source: &Path, target: &Path) {
    fs::create_dir_all(target).unwrap();
    for entry in fs::read_dir(source).unwrap() {
        let entry = entry.unwrap();
        let destination = target.join(entry.file_name());
        if entry.file_type().unwrap().is_dir() {
            copy(&entry.path(), &destination)
        } else {
            fs::copy(entry.path(), destination).unwrap();
        }
    }
}

fn pos(text: &str, needle: &str) -> (u64, u64) {
    let index = text.find(needle).unwrap();
    let before = &text[..index];
    (
        before.matches('\n').count() as u64,
        before.rsplit('\n').next().unwrap().encode_utf16().count() as u64,
    )
}

fn has(edit: &Value) -> bool {
    edit["documentChanges"]
        .as_array()
        .is_some_and(|changes| {
            changes.iter().any(|change| {
                change["edits"]
                    .as_array()
                    .is_some_and(|edits| !edits.is_empty())
            })
        })
        || edit["changes"].as_object().is_some_and(|changes| {
            changes
                .values()
                .any(|edits| edits.as_array().is_some_and(|items| !items.is_empty()))
        })
}

fn locations(value: &Value) -> Vec<(String, u64, u64)> {
    let items: Vec<&Value> = match value {
        Value::Array(items) => items.iter().collect(),
        Value::Object(_) => vec![value],
        _ => Vec::new(),
    };
    items
        .into_iter()
        .filter_map(|item| {
            let uri = item["uri"]
                .as_str()
                .or_else(|| item["targetUri"].as_str())?
                .to_string();
            let range = if item["range"].is_object() {
                &item["range"]
            } else {
                &item["targetSelectionRange"]
            };
            Some((
                uri,
                range["start"]["line"].as_u64()?,
                range["start"]["character"].as_u64()?,
            ))
        })
        .collect()
}

fn definition(
    session: &LspSession,
    root: &Path,
    path: &str,
    line: u64,
    character: u64,
) -> Vec<(String, u64, u64)> {
    let mut last = Vec::new();
    for _ in 0..40 {
        let value = session
            .request(
                "textDocument/definition",
                json!({
                    "textDocument": {"uri": file_uri(&root.join(path)).unwrap()},
                    "position": {"line": line, "character": character},
                }),
                Duration::from_secs(20),
            )
            .unwrap_or(Value::Null);
        last = locations(&value);
        if !last.is_empty() {
            break;
        }
        std::thread::sleep(Duration::from_millis(500));
    }
    last
}

fn edit_lines(edit: &Value, suffix: &str) -> Vec<(u64, u64)> {
    let mut ranges = Vec::new();
    if let Some(changes) = edit["documentChanges"].as_array() {
        for change in changes {
            let uri = change["textDocument"]["uri"].as_str().unwrap_or("");
            if !uri.ends_with(suffix) {
                continue;
            }
            if let Some(edits) = change["edits"].as_array() {
                for item in edits {
                    if let (Some(start), Some(end)) = (
                        item["range"]["start"]["line"].as_u64(),
                        item["range"]["end"]["line"].as_u64(),
                    ) {
                        ranges.push((start, end));
                    }
                }
            }
        }
    }
    if let Some(changes) = edit["changes"].as_object() {
        for (uri, edits) in changes {
            if !uri.ends_with(suffix) {
                continue;
            }
            if let Some(items) = edits.as_array() {
                for item in items {
                    if let (Some(start), Some(end)) = (
                        item["range"]["start"]["line"].as_u64(),
                        item["range"]["end"]["line"].as_u64(),
                    ) {
                        ranges.push((start, end));
                    }
                }
            }
        }
    }
    ranges
}

fn touches(ranges: &[(u64, u64)], line: u64) -> bool {
    ranges.iter().any(|(start, end)| *start <= line && line <= *end)
}

#[test]
fn jdtls_overloads_interfaces_annotations_reflection_and_cross_file_rename() {
    let jdk = PathBuf::from("<workspace>/_archmeld_tools/jdk25/jdk-25.0.5+8/bin/java.exe");
    let jdtls = PathBuf::from("<workspace>/_archmeld_tools/jdtls");
    let launcher = fs::read_dir(jdtls.join("plugins"))
        .ok()
        .and_then(|entries| {
            entries
                .filter_map(Result::ok)
                .map(|entry| entry.path())
                .find(|path| {
                    path.file_name()
                        .unwrap()
                        .to_string_lossy()
                        .starts_with("org.eclipse.equinox.launcher_")
                })
        });
    let (Some(launcher), true) = (launcher, jdk.is_file()) else {
        eprintln!("jdtls/jdk missing");
        return;
    };
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().join("repo");
    copy(
        &Path::new(env!("CARGO_MANIFEST_DIR")).join("../../adapters/languages/fixtures/java"),
        &root,
    );
    let mut store = Store::open(dir.path().join("state")).unwrap();
    store.ensure_project(P, "expedition").unwrap();
    let snap = capture_snapshot(
        &mut store,
        P,
        &root,
        &SnapshotInputs {
            build_config: json!({"kind": "java", "jdk": "25"}),
            environment: json!({"os": "test"}),
        },
        &SnapshotOptions {
            include_untracked: true,
            ..SnapshotOptions::default()
        },
    )
    .unwrap();
    let cfg = jdtls.join("config_win");
    let mut args = vec![
        "-Declipse.application=org.eclipse.jdt.ls.core.id1".into(),
        "-Declipse.product=org.eclipse.jdt.ls.core.product".into(),
        "-Dosgi.bundles.defaultStartLevel=4".into(),
        format!("-Dosgi.sharedConfiguration.area={}", cfg.display()),
        "-Dosgi.sharedConfiguration.area.readOnly=true".into(),
        "-Dosgi.configuration.cascaded=true".into(),
        "-noverify".into(),
        "-Xms512m".into(),
        "-Xmx1024m".into(),
        "--add-modules=ALL-SYSTEM".into(),
    ];
    for module in [
        "java.util",
        "java.lang",
        "java.nio",
        "java.io",
        "java.net",
        "java.text",
        "java.lang.reflect",
        "java.lang.invoke",
    ] {
        args.push(format!("--add-opens=java.base/{module}=ALL-UNNAMED"));
    }
    args.push("--add-opens=java.base/java.util.concurrent=ALL-UNNAMED".into());
    args.push("--add-opens=java.base/java.util.concurrent.atomic=ALL-UNNAMED".into());
    args.push("--add-opens=java.base/sun.nio.ch=ALL-UNNAMED".into());
    args.push("-jar".into());
    args.push(launcher.to_string_lossy().into());
    args.push("-configuration".into());
    args.push(cfg.to_string_lossy().into());
    args.push("-data".into());
    args.push(root.join("jdt-data").to_string_lossy().into());
    let session = LspSession::start(
        &LspServerSpec {
            id: "jdtls".into(),
            program: jdk,
            args,
            cwd: root.clone(),
            env: vec![],
            initialization_options: None,
        },
        &SpawnPermission::grant("grant.b013.java", "Java pack test"),
        &PathBuf::from("<workspace>/_archmeld_temp/java-audit.ndjson"),
    )
    .expect("jdtls start");
    session.initialize(&root).expect("initialize");
    let files = [
        "src/main/java/pkg/Util.java",
        "src/main/java/pkg/Other.java",
        "src/main/java/pkg/App.java",
        "src/main/java/pkg/Marker.java",
        "src/main/java/pkg/Transformer.java",
        "src/main/java/pkg/Doubler.java",
    ];
    for path in files {
        let text = fs::read_to_string(root.join(path)).unwrap();
        session.did_open(&root.join(path), "java", &text).unwrap();
    }
    let util = fs::read_to_string(root.join("src/main/java/pkg/Util.java")).unwrap();
    let app = fs::read_to_string(root.join("src/main/java/pkg/App.java")).unwrap();
    let marker = fs::read_to_string(root.join("src/main/java/pkg/Marker.java")).unwrap();

    // Overload resolution: each call must point at its own declaration.
    let (call_int_line, call_int_char) = pos(&app, "Util.helper(1)");
    let int_def = definition(
        &session,
        &root,
        "src/main/java/pkg/App.java",
        call_int_line,
        call_int_char + 5,
    );
    assert!(
        int_def
            .iter()
            .any(|(uri, line, _)| uri.ends_with("Util.java") && *line == 4),
        "int overload definition: {int_def:?}"
    );
    let (call_string_line, call_string_char) = pos(&app, "Util.helper(\"xy\")");
    let string_def = definition(
        &session,
        &root,
        "src/main/java/pkg/App.java",
        call_string_line,
        call_string_char + 5,
    );
    assert!(
        string_def
            .iter()
            .any(|(uri, line, _)| uri.ends_with("Util.java") && *line == 8),
        "String overload definition: {string_def:?}"
    );
    let (other_call_line, other_call_char) = pos(&app, "Other.helper(9)");
    let other_def = definition(
        &session,
        &root,
        "src/main/java/pkg/App.java",
        other_call_line,
        other_call_char + 6,
    );
    assert!(
        other_def
            .iter()
            .any(|(uri, line, _)| uri.ends_with("Other.java") && *line == 3),
        "same-named method in another class: {other_def:?}"
    );

    // Interface dispatch stays on the interface declaration.
    let (interface_line, interface_char) = pos(&app, "transformer.apply(3)");
    let interface_def = definition(
        &session,
        &root,
        "src/main/java/pkg/App.java",
        interface_line,
        interface_char + 12,
    );
    assert!(
        interface_def
            .iter()
            .any(|(uri, _, _)| uri.ends_with("Transformer.java")),
        "interface method definition: {interface_def:?}"
    );

    // Annotation usage resolves to the annotation type.
    let (marker_line, marker_char) = pos(&util, "@Marker");
    let marker_def = definition(
        &session,
        &root,
        "src/main/java/pkg/Util.java",
        marker_line,
        marker_char + 1,
    );
    assert!(
        marker_def
            .iter()
            .any(|(uri, _, _)| uri.ends_with("Marker.java")),
        "annotation definition: {marker_def:?}"
    );
    let (marker_decl_line, marker_decl_char) = pos(&marker, "interface Marker");
    let references = session
        .request(
            "textDocument/references",
            json!({
                "textDocument": {"uri": file_uri(&root.join("src/main/java/pkg/Marker.java")).unwrap()},
                "position": {"line": marker_decl_line, "character": marker_decl_char + 10},
                "context": {"includeDeclaration": true},
            }),
            Duration::from_secs(20),
        )
        .unwrap_or(Value::Null);
    assert!(
        locations(&references)
            .iter()
            .any(|(uri, _, _)| uri.ends_with("Util.java")),
        "annotation reference: {references}"
    );

    // Reflection is never guessed into a source call.
    let (reflection_line, reflection_char) = pos(&app, "getMethod(\"helper\"");
    let reflection_def = definition(
        &session,
        &root,
        "src/main/java/pkg/App.java",
        reflection_line,
        reflection_char + 1,
    );
    assert!(
        !reflection_def
            .iter()
            .any(|(uri, _, _)| uri.ends_with("Util.java")),
        "reflection must not resolve to Util.helper: {reflection_def:?}"
    );

    // Rename the int overload. The rename may cover both overloads, but it must
    // not touch Other.java, the Other.helper(9) call, or the reflective string.
    let (rename_line, rename_char) = pos(&util, "helper(int value)");
    let mut edit = Value::Null;
    let mut last = String::new();
    for _ in 0..45 {
        match session.rename(
            &root.join("src/main/java/pkg/Util.java"),
            rename_line,
            rename_char,
            "assist",
            Duration::from_secs(30),
        ) {
            Ok(value) if has(&value) => {
                edit = value;
                break;
            }
            Ok(value) => last = format!("empty {value}"),
            Err(error) => last = error.to_string(),
        }
        std::thread::sleep(Duration::from_millis(1000));
    }
    assert!(has(&edit), "no jdtls rename edits: {last}");
    let app_edits = edit_lines(&edit, "App.java");
    assert!(
        touches(&app_edits, call_int_line),
        "rename must touch Util.helper(1): {app_edits:?}"
    );
    assert!(
        touches(&app_edits, call_string_line),
        "rename must touch Util.helper(\"xy\"): {app_edits:?}"
    );
    assert!(
        !touches(&app_edits, other_call_line),
        "rename must not touch Other.helper(9): {app_edits:?}"
    );
    assert!(
        !touches(&app_edits, reflection_line),
        "rename must not touch the reflective string: {app_edits:?}"
    );
    let preview = preview_workspace_edit(
        &store,
        &PreviewRequest {
            project_id: P,
            snapshot_id: &snap.snapshot_id,
            allowed_paths: &[],
            max_files: 12,
            workspace_root: Some(&root),
        },
        &edit,
    )
    .expect("preview");
    assert!(preview.complete(), "{:?}", preview.refusals);
    let renamed: Vec<String> = preview.files.iter().map(|file| file.path.clone()).collect();
    assert!(
        renamed.iter().any(|path| path.ends_with("Util.java"))
            && renamed.iter().any(|path| path.ends_with("App.java")),
        "{renamed:?}"
    );
    assert!(
        !renamed.iter().any(|path| path.ends_with("Other.java")),
        "{renamed:?}"
    );
    let summary = session.shutdown().unwrap();
    assert!(summary.orphan_pids.is_empty());
    let out = json!({
        "language": "java",
        "server": "jdtls 1.64.0.202610301801 / JDK 25.0.5+8",
        "snapshot_id": snap.snapshot_id,
        "checks": {
            "overload_int_definition": int_def,
            "overload_string_definition": string_def,
            "other_class_definition": other_def,
            "interface_definition": interface_def,
            "annotation_definition": marker_def,
            "annotation_references": locations(&references),
            "reflection_definition": reflection_def,
            "reflection_not_guessed": true,
            "rename_touched_app_lines": app_edits,
            "rename_did_not_touch": {
                "other_call_line": other_call_line,
                "reflection_line": reflection_line,
            }
        },
        "rename_files": renamed,
        "process_audit": {"orphan_pids": summary.orphan_pids, "exit_code": summary.exit_code},
        "runtime_trace": "B016 runtime mapping available; Java runtime scenario not yet run",
    });
    if std::env::var("ARCHMELD_WRITE_EVIDENCE").ok().as_deref() == Some("1") {
        fs::write(
            Path::new(env!("CARGO_MANIFEST_DIR")).join("../../adapters/languages/evidence/java.json"),
            serde_json::to_string_pretty(&out).unwrap(),
        )
        .unwrap();
    }
}
