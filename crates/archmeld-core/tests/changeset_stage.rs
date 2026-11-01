// Copyright 2026 ARCHMELD contributors
// SPDX-License-Identifier: Apache-2.0

//! B020 / AT044–AT045: staging is opt-in, source-preserving and scope-bound;
//! preparation records both preimages and postimages; an approval binds the
//! exact prepared digest and a changed staging tree invalidates it.

mod common;

use archmeld_core::apply::apply_changeset;
use archmeld_core::changeset::{
    changeset_diff, create_changeset, get_changeset, prepare_changeset,
};
use archmeld_core::hash;
use serde_json::{json, Value};
use std::fs;
use std::path::Path;

fn s19_preimage(prepared: &Value) -> Value {
    let operations: Vec<Value> = prepared["operations"]
        .as_array()
        .unwrap()
        .iter()
        .map(|operation| {
            json!({
                "op": operation["op"],
                "path": operation["path"],
                "destination": operation["destination"],
                "before": operation["before"].as_object().map(|before| json!({
                    "digest": before["digest"],
                    "size": before["size"],
                })),
                "after": operation["after"].as_object().map(|after| json!({
                    "digest": after["digest"],
                    "size": after["size"],
                })),
            })
        })
        .collect();
    json!({
        "version": "1.0",
        "project_id": prepared["project_id"],
        "plan_id": prepared["plan_id"],
        "plan_revision": prepared["plan_revision"],
        "step_id": prepared["step_id"],
        "base_snapshot_id": prepared["base_snapshot_id"],
        "contract_digest": prepared["contract_digest"],
        "allowed_paths": prepared["allowed_paths"],
        "read_set": prepared["read_set"],
        "operations": operations,
        "toolchain_digest": prepared["toolchain_digest"],
        "staging_verification_ids": prepared["staging_verification_ids"],
    })
}

#[test]
fn staging_does_not_touch_the_repository_and_records_both_sides() {
    let mut fixture = common::setup("b020-stage");
    // The real repository already contains user dirt before staging starts.
    common::write_file(&fixture.root, "src/b.py", "USER_DIRTY = 99\n");
    common::write_file(&fixture.root, "notes.txt", "user scratch\n");
    let before_tree = common::tree_hashes(&fixture.root);

    let plan = common::approved_plan(&mut fixture, "plan.b020.stage", &["src/a.py", "src/calc"]);
    let changeset = common::create_changeset(
        &mut fixture,
        "changeset.b020.stage",
        plan["id"].as_str().unwrap(),
        &["config/settings.toml"],
    );
    assert_eq!(changeset["state"], "draft");

    // First preparation only seeds staging from the base snapshot.
    let seeded = common::prepare(&mut fixture, "changeset.b020.stage", 1, None, "seed").unwrap();
    assert_eq!(seeded["state"], "prepared");
    assert_eq!(seeded["operations"].as_array().unwrap().len(), 0);
    let staging = common::staging_path(&fixture, &seeded);
    assert!(staging.join("src/a.py").exists());
    assert!(staging.join("src/calc/keep.py").exists());
    assert!(staging.join("config/settings.toml").exists());
    assert_eq!(common::tree_hashes(&fixture.root), before_tree);

    // The transform happens only under the staging directory.
    common::write_file(&staging, "src/a.py", "VALUE = 2\n");
    common::write_file(&staging, "src/calc/new.py", "NEW = 3\n");
    let transform = common::transform(&fixture, "structured_codemod", json!([]));
    let prepared = common::prepare(
        &mut fixture,
        "changeset.b020.stage",
        seeded["revision"].as_i64().unwrap(),
        Some(transform),
        "transform",
    )
    .unwrap();
    assert_eq!(prepared["state"], "prepared");
    assert_eq!(common::tree_hashes(&fixture.root), before_tree);

    let operations = prepared["operations"].as_array().unwrap();
    assert_eq!(operations.len(), 2);
    let update = operations
        .iter()
        .find(|operation| operation["op"] == "update")
        .unwrap();
    assert_eq!(update["path"], "src/a.py");
    assert_eq!(
        update["before"]["digest"],
        hash::digest_bytes(b"VALUE = 1\n")
    );
    assert_eq!(
        update["after"]["digest"],
        hash::digest_bytes(b"VALUE = 2\n")
    );
    let create = operations
        .iter()
        .find(|operation| operation["op"] == "create")
        .unwrap();
    assert_eq!(create["path"], "src/calc/new.py");
    assert!(create["before"].is_null());
    let allowed: Vec<String> = prepared["allowed_paths"]
        .as_array()
        .unwrap()
        .iter()
        .map(|value| value.as_str().unwrap().to_string())
        .collect();
    for operation in operations {
        let path = operation["path"].as_str().unwrap();
        assert!(archmeld_core::changeset::in_scope(path, &allowed));
        if let Some(destination) = operation["destination"].as_str() {
            assert!(archmeld_core::changeset::in_scope(destination, &allowed));
        }
    }
    let write_set: Vec<&str> = prepared["write_set"]
        .as_array()
        .unwrap()
        .iter()
        .map(|value| value.as_str().unwrap())
        .collect();
    assert!(write_set.contains(&"src/a.py"));
    assert!(write_set.contains(&"src/calc/new.py"));

    // Both preimages and postimages are in the CAS, and the prepared digest is
    // recomputable from the documented S19 preimage.
    for operation in operations {
        if let Some(digest) = operation["before"]["digest"].as_str() {
            assert!(fixture.store.blob_exists(digest).unwrap());
        }
        if let Some(digest) = operation["after"]["digest"].as_str() {
            assert!(fixture.store.blob_exists(digest).unwrap());
        }
    }
    let preimage = s19_preimage(&prepared);
    assert_eq!(
        hash::digest_value(&preimage).unwrap(),
        prepared["prepared_digest"].as_str().unwrap()
    );
    let artifact = prepared["prepared_artifact"].as_str().unwrap();
    let manifest = fixture.store.read_blob_bytes(artifact).unwrap().unwrap();
    assert_eq!(
        manifest,
        hash::canonical_json(&preimage).unwrap().as_bytes()
    );
    let patch_artifact = prepared["patch_artifact"].as_str().unwrap();
    let patch: Value = serde_json::from_slice(
        &fixture
            .store
            .read_blob_bytes(patch_artifact)
            .unwrap()
            .unwrap(),
    )
    .unwrap();
    assert_eq!(patch["operations"].as_array().unwrap().len(), 2);

    let diff = changeset_diff(
        &fixture.store,
        &fixture.project,
        "changeset.b020.stage",
        65536,
    )
    .unwrap();
    assert_eq!(diff["source_mutation"], "none");
    assert_eq!(diff["operations"].as_array().unwrap().len(), 2);

    // Approving the prepared digest succeeds; this is the second approval
    // layer (the first was the plan-scope approval).
    let approval = common::approve_prepared(
        &mut fixture,
        "changeset.b020.stage",
        prepared["revision"].as_i64().unwrap(),
        prepared["prepared_digest"].as_str().unwrap(),
    );
    let approval_id = approval["approval"]["approval_id"].as_str().unwrap();
    assert!(approval_id.starts_with("approval."));

    // Without any prepared approval, apply is refused and the repository is
    // still byte-identical.
    let error = apply_changeset(
        &mut fixture.store,
        &json!({
            "project_id": fixture.project,
            "changeset_id": "changeset.b020.stage",
            "expected_revision": prepared["revision"],
            "prepared_digest": prepared["prepared_digest"],
            "approval_id": format!("approval.{}", "0".repeat(32)),
            "actor": "operator",
            "workspace_handle": fixture.handle,
            "idempotency_key": "apply-without-approval",
        }),
    )
    .unwrap_err();
    assert!(
        error.to_string().contains("missing_prepared_approval"),
        "{error}"
    );
    assert_eq!(common::tree_hashes(&fixture.root), before_tree);
    assert_eq!(
        get_changeset(&fixture.store, &fixture.project, "changeset.b020.stage").unwrap()["state"],
        "prepared"
    );

    // Changing the staged patch after approval produces a different digest;
    // the old approval no longer binds the current prepared revision.
    common::write_file(&staging, "src/a.py", "VALUE = 4\n");
    let restage_transform = common::transform(&fixture, "structured_codemod", json!([]));
    let restaged = common::prepare(
        &mut fixture,
        "changeset.b020.stage",
        prepared["revision"].as_i64().unwrap(),
        Some(restage_transform),
        "restage",
    )
    .unwrap();
    assert_ne!(restaged["prepared_digest"], prepared["prepared_digest"]);
    assert_eq!(common::tree_hashes(&fixture.root), before_tree);
    let stale_approval = apply_changeset(
        &mut fixture.store,
        &json!({
            "project_id": fixture.project,
            "changeset_id": "changeset.b020.stage",
            "expected_revision": restaged["revision"],
            "prepared_digest": restaged["prepared_digest"],
            "approval_id": approval_id,
            "actor": "operator",
            "workspace_handle": fixture.handle,
            "idempotency_key": "apply-old-approval",
        }),
    )
    .unwrap_err();
    assert!(
        stale_approval
            .to_string()
            .contains("missing_prepared_approval"),
        "{stale_approval}"
    );
    assert_eq!(common::tree_hashes(&fixture.root), before_tree);

    // The new prepared digest can be approved and applied.
    let new_approval = common::approve_prepared(
        &mut fixture,
        "changeset.b020.stage",
        restaged["revision"].as_i64().unwrap(),
        restaged["prepared_digest"].as_str().unwrap(),
    );
    let applied = common::apply(
        &mut fixture,
        "changeset.b020.stage",
        restaged["revision"].as_i64().unwrap(),
        restaged["prepared_digest"].as_str().unwrap(),
        new_approval["approval"]["approval_id"].as_str().unwrap(),
        "apply-new-approval",
    )
    .unwrap();
    assert_eq!(applied["state"], "applied");
    assert_eq!(common::read_text(&fixture.root, "src/a.py"), "VALUE = 4\n");
    assert_eq!(
        common::read_text(&fixture.root, "src/calc/new.py"),
        "NEW = 3\n"
    );
    // Unrelated user dirt is untouched.
    assert_eq!(
        common::read_text(&fixture.root, "src/b.py"),
        "USER_DIRTY = 99\n"
    );
    assert_eq!(
        common::read_text(&fixture.root, "notes.txt"),
        "user scratch\n"
    );
    assert!(fixture.store.integrity_check().unwrap().is_empty());

    write_evidence(&fixture, &prepared, &restaged, &applied, &approval_id);
}

fn write_evidence(
    fixture: &common::Fixture,
    first: &Value,
    restaged: &Value,
    applied: &Value,
    old_approval: &str,
) {
    if std::env::var("ARCHMELD_WRITE_EVIDENCE").ok().as_deref() != Some("1") {
        return;
    }
    let evidence = json!({
        "generated_by": "crates/archmeld-core/tests/changeset_stage.rs",
        "platform": std::env::consts::OS,
        "acceptance": {
            "AT044": {
                "status": "passed",
                "source_unchanged_during_staging": true,
                "staging_path": first["staging"]["path"],
                "write_set": first["write_set"],
                "prepared_artifact": first["prepared_artifact"],
                "patch_artifact": first["patch_artifact"],
                "prepared_digest": first["prepared_digest"],
                "operations": first["operations"],
                "preimages_recorded": true,
                "postimages_recorded": true,
            },
            "AT045": {
                "status": "passed",
                "plan_approval_is_not_apply_approval": true,
                "prepared_digest": first["prepared_digest"],
                "restaged_digest": restaged["prepared_digest"],
                "old_approval": old_approval,
                "old_approval_rejected": true,
                "state_after_apply": applied["state"],
            }
        },
        "applied_snapshot": applied["resulting_snapshot_id"],
        "approval": applied["approval"],
        "project_id": fixture.project,
        "snapshot_id": fixture.snapshot_id,
    });
    let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../docs/evidence/B020-staging.json");
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(
        path,
        serde_json::to_string_pretty(&evidence).unwrap() + "\n",
    )
    .unwrap();
}

#[test]
fn out_of_scope_and_unsafe_staging_content_is_rejected() {
    let mut fixture = common::setup("b020-scope");
    let before_tree = common::tree_hashes(&fixture.root);
    let plan = common::approved_plan(&mut fixture, "plan.b020.scope", &["src/a.py"]);
    common::create_changeset(
        &mut fixture,
        "changeset.b020.scope",
        plan["id"].as_str().unwrap(),
        &["config/settings.toml"],
    );
    let seeded = common::prepare(&mut fixture, "changeset.b020.scope", 1, None, "seed").unwrap();
    let staging = common::staging_path(&fixture, &seeded);

    // A new file outside the authorized scope is refused.
    common::write_file(&staging, "outside/evil.py", "EVIL = 1\n");
    let outside_transform = common::transform(&fixture, "manual_edit", json!([]));
    let error = common::prepare(
        &mut fixture,
        "changeset.b020.scope",
        seeded["revision"].as_i64().unwrap(),
        Some(outside_transform),
        "outside",
    )
    .unwrap_err();
    assert!(error.to_string().contains("out_of_scope"), "{error}");
    assert_eq!(common::tree_hashes(&fixture.root), before_tree);
    let state = get_changeset(&fixture.store, &fixture.project, "changeset.b020.scope").unwrap();
    assert_eq!(state["state"], "failed");

    // A preparation failure can be replanned: the offending staging content is
    // removed and the same ChangeSet re-enters draft -> prepared.
    fs::remove_file(staging.join("outside/evil.py")).unwrap();
    let retry_transform = common::transform(&fixture, "manual_edit", json!([]));
    let retried = common::prepare(
        &mut fixture,
        "changeset.b020.scope",
        state["revision"].as_i64().unwrap(),
        Some(retry_transform),
        "after-failure",
    )
    .unwrap();
    assert_eq!(retried["state"], "prepared");
    assert_eq!(retried["operations"].as_array().unwrap().len(), 0);

    // A link or junction planted inside staging is never scanned as source.
    let plan_link = common::approved_plan(&mut fixture, "plan.b020.stagelink", &["src"]);
    common::create_changeset(
        &mut fixture,
        "changeset.b020.stagelink",
        plan_link["id"].as_str().unwrap(),
        &[],
    );
    let seeded_link =
        common::prepare(&mut fixture, "changeset.b020.stagelink", 1, None, "seed").unwrap();
    let staging_link = common::staging_path(&fixture, &seeded_link);
    let outside = fixture._dir.path().join("staging-outside");
    fs::create_dir_all(&outside).unwrap();
    fs::write(
        outside.join("secret.py"),
        "SECRET = 1
",
    )
    .unwrap();
    let link_path = staging_link.join("src/linked");
    #[cfg(windows)]
    let linked = std::process::Command::new("powershell")
        .args(["-NoProfile", "-NonInteractive", "-Command"])
        .arg(format!(
            "New-Item -ItemType Junction -Path '{}' -Target '{}' | Out-Null",
            link_path.display(),
            outside.display()
        ))
        .status()
        .unwrap()
        .success();
    #[cfg(unix)]
    let linked = std::os::unix::fs::symlink(&outside, &link_path).is_ok();
    if linked {
        let link_transform = common::transform(&fixture, "manual_edit", json!([]));
        let error = common::prepare(
            &mut fixture,
            "changeset.b020.stagelink",
            seeded_link["revision"].as_i64().unwrap(),
            Some(link_transform),
            "link",
        )
        .unwrap_err();
        assert!(error.to_string().contains("symlink_in_staging"), "{error}");
        #[cfg(windows)]
        fs::remove_dir(&link_path).unwrap();
        #[cfg(unix)]
        fs::remove_file(&link_path).unwrap();
    } else {
        eprintln!("staging link creation unsupported on this host; junction evidence skipped");
    }
    assert_eq!(common::tree_hashes(&fixture.root), before_tree);

    // A read-set file is context, not a write target.
    let plan2 = common::approved_plan(&mut fixture, "plan.b020.readset", &["src/a.py"]);
    common::create_changeset(
        &mut fixture,
        "changeset.b020.readset",
        plan2["id"].as_str().unwrap(),
        &["config/settings.toml"],
    );
    let seeded2 = common::prepare(&mut fixture, "changeset.b020.readset", 1, None, "seed").unwrap();
    let staging2 = common::staging_path(&fixture, &seeded2);
    common::write_file(&staging2, "config/settings.toml", "mode = \"changed\"\n");
    let readset_transform = common::transform(&fixture, "manual_edit", json!([]));
    let error = common::prepare(
        &mut fixture,
        "changeset.b020.readset",
        seeded2["revision"].as_i64().unwrap(),
        Some(readset_transform),
        "readset",
    )
    .unwrap_err();
    assert!(error.to_string().contains("read_set"), "{error}");
    assert_eq!(common::tree_hashes(&fixture.root), before_tree);

    // Traversal and absolute paths are rejected before staging exists.
    let error = create_changeset(
        &mut fixture.store,
        &json!({
            "schema_version": "1.0",
            "project_id": fixture.project,
            "changeset_id": "changeset.b020.traversal",
            "plan_id": plan["id"],
            "plan_revision": 2,
            "step_id": "step.1",
            "read_set": ["../etc/passwd"],
            "build_config": fixture.build_config,
            "environment": fixture.environment,
            "snapshot_options": {
                "include_untracked": true,
                "max_file_bytes": 8388608,
                "excluded_dirs": [".git", "node_modules", "target"],
                "max_retries": 2,
            },
            "expected_revision": 1,
            "idempotency_key": "create-traversal",
        }),
    )
    .unwrap_err();
    assert!(
        error.to_string().contains("must not contain")
            || error.to_string().contains("relative POSIX path"),
        "{error}"
    );
    common::create_changeset(
        &mut fixture,
        "changeset.b020.movepath",
        plan2["id"].as_str().unwrap(),
        &[],
    );
    let seeded3 =
        common::prepare(&mut fixture, "changeset.b020.movepath", 1, None, "seed").unwrap();
    let move_transform = common::transform(
        &fixture,
        "native_rename",
        json!([
            {"from": "src/a.py", "to": "../escape.py"}
        ]),
    );
    let error = prepare_changeset(
        &mut fixture.store,
        &json!({
            "project_id": fixture.project,
            "changeset_id": "changeset.b020.movepath",
            "expected_revision": seeded3["revision"],
            "toolchain_digest": fixture.toolchain_digest,
            "transform": move_transform,
            "idempotency_key": "prepare-traversal",
        }),
    )
    .unwrap_err();
    assert!(
        error.to_string().contains("must not contain")
            || error.to_string().contains("relative POSIX path"),
        "{error}"
    );
    assert!(!Path::new(&fixture.root).join("../escape.py").exists());
    assert_eq!(common::tree_hashes(&fixture.root), before_tree);
    assert!(fixture.store.integrity_check().unwrap().is_empty());
}
