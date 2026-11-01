// Copyright 2026 ARCHMELD contributors
// SPDX-License-Identifier: Apache-2.0

//! B021 / AT046–AT049, AT078: per-file CAS, fencing leases, durable journal,
//! crash recovery, conflict preservation, path/link races and case-only moves.
//! Everything runs against real files on disk and real durable ledger rows.

mod common;

use archmeld_core::apply::{
    acquire_writer_lease, interrupted_changesets, release_writer_lease, ApplyEvent,
};
use archmeld_core::changeset::get_changeset;
use archmeld_core::hash;
use serde_json::{json, Value};
use std::collections::BTreeSet;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::Duration;

static EVIDENCE_LOCK: Mutex<()> = Mutex::new(());

struct Prepared {
    changeset_id: String,
    revision: i64,
    prepared_digest: String,
    approval_id: String,
    staging: PathBuf,
}

fn prepared(
    fixture: &mut common::Fixture,
    plan_id: &str,
    changeset_id: &str,
    allowed: &[&str],
    read_set: &[&str],
    edits: &[(&str, &str)],
    method: &str,
    moves: Value,
) -> Prepared {
    common::approved_plan(fixture, plan_id, allowed);
    common::create_changeset(fixture, changeset_id, plan_id, read_set);
    let seeded = common::prepare(fixture, changeset_id, 1, None, "seed").unwrap();
    let staging = common::staging_path(fixture, &seeded);
    for (path, content) in edits {
        common::write_file(&staging, path, content);
    }
    let transform = common::transform(fixture, method, moves);
    let prepared = common::prepare(
        fixture,
        changeset_id,
        seeded["revision"].as_i64().unwrap(),
        Some(transform),
        "transform",
    )
    .unwrap();
    let approval = common::approve_prepared(
        fixture,
        changeset_id,
        prepared["revision"].as_i64().unwrap(),
        prepared["prepared_digest"].as_str().unwrap(),
    );
    Prepared {
        changeset_id: changeset_id.to_string(),
        revision: prepared["revision"].as_i64().unwrap(),
        prepared_digest: prepared["prepared_digest"].as_str().unwrap().to_string(),
        approval_id: approval["approval"]["approval_id"]
            .as_str()
            .unwrap()
            .to_string(),
        staging,
    }
}

#[test]
fn external_edits_make_the_change_stale_and_old_leases_cannot_write() {
    let mut fixture = common::setup("b021-cas");
    let plan = "plan.b021.cas";
    let change = prepared(
        &mut fixture,
        plan,
        "changeset.b021.cas",
        &["src"],
        &["config/settings.toml"],
        &[("src/a.py", "VALUE = 2\n")],
        "manual_edit",
        json!([]),
    );
    let original_tree = common::tree_hashes(&fixture.root);

    // An external editor changes the target between prepare and apply.
    common::write_file(&fixture.root, "src/a.py", "EXTERNAL = 1\n");
    let before_apply = common::tree_hashes(&fixture.root);
    let error = common::apply(
        &mut fixture,
        &change.changeset_id,
        change.revision,
        &change.prepared_digest,
        &change.approval_id,
        "apply-external-target",
    )
    .unwrap_err();
    assert!(
        error.to_string().contains("stale_inputs")
            || error.to_string().contains("changed since preparation"),
        "{error}"
    );
    assert_eq!(common::tree_hashes(&fixture.root), before_apply);
    assert_eq!(
        common::read_text(&fixture.root, "src/a.py"),
        "EXTERNAL = 1\n"
    );
    let state = get_changeset(&fixture.store, &fixture.project, &change.changeset_id).unwrap();
    assert!(
        matches!(state["state"].as_str(), Some("stale" | "conflicted")),
        "{state}"
    );

    // An external edit to a read-set/build-context file is also stale.
    let change2 = prepared(
        &mut fixture,
        "plan.b021.cas2",
        "changeset.b021.cas2",
        &["src"],
        &["config/settings.toml"],
        &[("src/a.py", "VALUE = 3\n")],
        "manual_edit",
        json!([]),
    );
    common::write_file(
        &fixture.root,
        "config/settings.toml",
        "mode = \"external\"\n",
    );
    let before_readset = common::tree_hashes(&fixture.root);
    let error = common::apply(
        &mut fixture,
        &change2.changeset_id,
        change2.revision,
        &change2.prepared_digest,
        &change2.approval_id,
        "apply-external-readset",
    )
    .unwrap_err();
    assert!(error.to_string().contains("read_set"), "{error}");
    assert_eq!(common::tree_hashes(&fixture.root), before_readset);
    let state = get_changeset(&fixture.store, &fixture.project, &change2.changeset_id).unwrap();
    assert_eq!(state["state"], "stale");

    // A lease that expires and is taken over by a new fencing token cannot
    // keep writing.
    let _ = original_tree;
    let change3 = prepared(
        &mut fixture,
        "plan.b021.cas3",
        "changeset.b021.cas3",
        &["src"],
        &[],
        &[("src/a.py", "VALUE = 4\n")],
        "manual_edit",
        json!([]),
    );
    common::write_file(
        &fixture.root,
        "src/a.py",
        "VALUE = 1
",
    );
    let before_lease = common::tree_hashes(&fixture.root);
    let project = fixture.project.clone();
    let mut hook = |store: &mut archmeld_core::store::Store, event: &ApplyEvent| {
        if matches!(event, ApplyEvent::PreflightPassed) {
            std::thread::sleep(Duration::from_millis(2200));
            let taken = acquire_writer_lease(store, &project, "intruder", 60).unwrap();
            assert!(taken["fencing_token"].as_i64().unwrap() >= 2);
        }
        Ok(())
    };
    let error = common::apply_with_hook(
        &mut fixture,
        &change3.changeset_id,
        change3.revision,
        &change3.prepared_digest,
        &change3.approval_id,
        "apply-old-lease",
        1,
        &mut hook,
    )
    .unwrap_err();
    assert!(error.to_string().contains("lease_lost"), "{error}");
    assert_eq!(common::tree_hashes(&fixture.root), before_lease);
    let state = get_changeset(&fixture.store, &fixture.project, &change3.changeset_id).unwrap();
    assert_eq!(state["state"], "failed");
    let lease = archmeld_core::apply::writer_lease(&fixture.store, &fixture.project)
        .unwrap()
        .unwrap();
    assert_eq!(lease["owner"], "intruder");
    assert!(lease["fencing_token"].as_i64().unwrap() >= 2);
    let _ = release_writer_lease(
        &mut fixture.store,
        &fixture.project,
        "intruder",
        lease["fencing_token"].as_i64().unwrap(),
    );
    assert!(fixture.store.integrity_check().unwrap().is_empty());
    write_evidence(
        &fixture,
        "AT046",
        json!({
            "target_external_edit": "stale, unchanged",
            "read_set_external_edit": "stale, unchanged",
            "lease_takeover": "old fencing token refused, no file written",
            "final_state": state["state"],
        }),
        None,
    );
}

#[test]
fn crash_after_partial_replacements_is_recovered_from_the_journal() {
    let mut fixture = common::setup("b021-crash");
    common::write_file(&fixture.root, "notes.txt", "unrelated user work\n");
    let change = prepared(
        &mut fixture,
        "plan.b021.crash",
        "changeset.b021.crash",
        &["src"],
        &[],
        &[
            ("src/a.py", "VALUE = 10\n"),
            ("src/b.py", "OTHER = 20\n"),
            ("src/new.py", "NEW = 30\n"),
        ],
        "manual_edit",
        json!([]),
    );
    // Add a deletion to the staged picture.
    fs::remove_file(change.staging.join("src/calc/keep.py")).unwrap();
    let crash_transform = common::transform(&fixture, "manual_edit", json!([]));
    let restaged = common::prepare(
        &mut fixture,
        &change.changeset_id,
        change.revision,
        Some(crash_transform),
        "crash-restage",
    )
    .unwrap();
    let approval = common::approve_prepared(
        &mut fixture,
        &change.changeset_id,
        restaged["revision"].as_i64().unwrap(),
        restaged["prepared_digest"].as_str().unwrap(),
    );
    let approval_id = approval["approval"]["approval_id"].as_str().unwrap();
    let revision = restaged["revision"].as_i64().unwrap();
    let digest = restaged["prepared_digest"].as_str().unwrap().to_string();
    let after_tree = common::tree_hashes(&fixture.root);

    let mut replaced = 0;
    let mut hook = |_store: &mut archmeld_core::store::Store, event: &ApplyEvent| {
        if matches!(event, ApplyEvent::AfterReplace { .. }) {
            replaced += 1;
            if replaced == 2 {
                return Err(archmeld_core::CoreError::Injected(
                    "simulated process crash after two replacements".into(),
                ));
            }
        }
        Ok(())
    };
    let error = common::apply_with_hook(
        &mut fixture,
        &change.changeset_id,
        revision,
        &digest,
        approval_id,
        "apply-crash",
        // A 1-second lease can self-expire while the apply is still writing
        // under parallel test load, masking the injected crash. Five seconds
        // keeps the write path inside its own lease; the recovery below waits
        // for that lease to expire before taking the next fencing token.
        5,
        &mut hook,
    )
    .unwrap_err();
    assert!(
        error.to_string().contains("simulated process crash"),
        "{error}"
    );

    let state = get_changeset(&fixture.store, &fixture.project, &change.changeset_id).unwrap();
    assert_eq!(state["state"], "applying");
    let interrupted = interrupted_changesets(&fixture.store, &fixture.project).unwrap();
    let record = interrupted
        .iter()
        .find(|item| item["changeset_id"] == change.changeset_id)
        .unwrap();
    assert_eq!(record["state"], "applying");
    assert!(record["journal_revision"].as_i64().unwrap() >= revision + 1);
    assert!(!record["pending_journal"].as_array().unwrap().is_empty());

    // Mid-apply state is visible: the first create and update were applied,
    // later intents were not.
    assert_eq!(common::read_text(&fixture.root, "src/new.py"), "NEW = 30\n");
    assert_eq!(common::read_text(&fixture.root, "src/a.py"), "VALUE = 10\n");
    assert_eq!(common::read_text(&fixture.root, "src/b.py"), "OTHER = 2\n");
    assert!(fixture.root.join("src/calc/keep.py").exists());

    // A real restart cannot take over the crashed writer's lease until it
    // expires; recovery then acquires the next fencing token.
    std::thread::sleep(Duration::from_millis(6200));
    let recovered = common::recover(
        &mut fixture,
        &change.changeset_id,
        state["revision"].as_i64().unwrap(),
        "recover-crash",
    )
    .unwrap();
    assert_eq!(recovered["state"], "recovered");
    assert_eq!(common::tree_hashes(&fixture.root), after_tree);
    assert_eq!(
        common::read_text(&fixture.root, "notes.txt"),
        "unrelated user work\n"
    );
    assert!(!fixture.root.join("src/new.py").exists());
    let journal = interrupted_changesets(&fixture.store, &fixture.project).unwrap();
    assert!(journal.is_empty(), "{journal:?}");
    assert!(fixture.store.integrity_check().unwrap().is_empty());
    write_evidence(
        &fixture,
        "AT047",
        json!({
            "fault": "injected after the second file replacement",
            "detected_state": "applying with a durable journal",
            "mid_apply_files": {"src/new.py": "postimage", "src/a.py": "postimage", "src/b.py": "preimage"},
            "recovery": "journal-driven, all owned writes restored",
            "unrelated_user_file_preserved": true,
            "full_reset_used": false,
        }),
        Some(&recovered),
    );
}

#[test]
fn recovery_preserves_a_later_user_edit_of_an_updated_file() {
    let mut fixture = common::setup("b021-preserve");
    let change = prepared(
        &mut fixture,
        "plan.b021.preserve",
        "changeset.b021.preserve",
        &["src"],
        &[],
        &[("src/a.py", "VALUE = 7\n"), ("src/b.py", "OTHER = 8\n")],
        "manual_edit",
        json!([]),
    );
    let applied = common::apply(
        &mut fixture,
        &change.changeset_id,
        change.revision,
        &change.prepared_digest,
        &change.approval_id,
        "apply-preserve",
    )
    .unwrap();
    assert_eq!(applied["state"], "applied");
    assert_eq!(common::read_text(&fixture.root, "src/a.py"), "VALUE = 7\n");

    // The user edits one file after apply. Recovery must keep it.
    common::write_file(&fixture.root, "src/a.py", "USER_EDIT = 7\n");
    let user_bytes = fs::read(fixture.root.join("src/a.py")).unwrap();
    let recovered = common::recover(
        &mut fixture,
        &change.changeset_id,
        applied["revision"].as_i64().unwrap(),
        "recover-preserve",
    )
    .unwrap();
    assert_eq!(recovered["state"], "conflicted");
    assert_eq!(
        common::read_text(&fixture.root, "src/a.py"),
        "USER_EDIT = 7\n"
    );
    assert_eq!(common::read_text(&fixture.root, "src/b.py"), "OTHER = 2\n");
    let conflicts = recovered["conflicts"].as_array().unwrap();
    assert_eq!(conflicts.len(), 1);
    assert_eq!(conflicts[0]["path"], "src/a.py");
    for operation in recovered["operations"].as_array().unwrap() {
        if operation["path"] == "src/a.py" {
            assert_eq!(operation["state"], "conflict", "{operation}");
        } else {
            assert_eq!(operation["state"], "restored", "{operation}");
        }
    }
    let artifact = conflicts[0]["user_version_artifact"].as_str().unwrap();
    let stored = fixture.store.read_blob_bytes(artifact).unwrap().unwrap();
    assert_eq!(stored, user_bytes);
    assert!(fixture.store.integrity_check().unwrap().is_empty());
    write_evidence(
        &fixture,
        "AT048",
        json!({
            "updated_file": "src/a.py",
            "user_edit_preserved": true,
            "other_file_restored": "src/b.py -> preimage",
            "user_version_artifact": artifact,
            "state": recovered["state"],
        }),
        Some(&recovered),
    );
}

#[test]
fn path_and_link_races_are_rechecked_before_any_write() {
    let mut fixture = common::setup("b021-link");
    let change = prepared(
        &mut fixture,
        "plan.b021.link",
        "changeset.b021.link",
        &["src"],
        &[],
        &[("src/a.py", "VALUE = 5\n")],
        "manual_edit",
        json!([]),
    );
    let outside = fixture._dir.path().join("outside");
    fs::create_dir_all(&outside).unwrap();
    fs::write(outside.join("a.py"), "VALUE = 1\n").unwrap();
    fs::write(outside.join("victim.py"), "VICTIM\n").unwrap();
    let victim_before = hash::digest_bytes(&fs::read(outside.join("victim.py")).unwrap());
    let victim_after_apply;
    let repo_src = fixture.root.join("src");
    let saved_src = fixture.root.join("src_real");
    fs::rename(&repo_src, &saved_src).unwrap();

    #[cfg(windows)]
    let linked = {
        let junction = repo_src.clone();
        let status = std::process::Command::new("cmd")
            .arg("/C")
            .arg("mklink")
            .arg("/J")
            .arg(&junction)
            .arg(&outside)
            .status()
            .unwrap();
        status.success()
    };
    #[cfg(unix)]
    let linked = std::os::unix::fs::symlink(&outside, &repo_src).is_ok();

    if linked {
        let result = common::apply(
            &mut fixture,
            &change.changeset_id,
            change.revision,
            &change.prepared_digest,
            &change.approval_id,
            "apply-link-race",
        );
        victim_after_apply = hash::digest_bytes(&fs::read(outside.join("victim.py")).unwrap());
        let state = get_changeset(&fixture.store, &fixture.project, &change.changeset_id).unwrap();
        assert!(result.is_err(), "link race must be rejected");
        let message = result.unwrap_err().to_string();
        assert!(
            message.contains("unsafe_change") || message.contains("symlink"),
            "{message}"
        );
        assert!(matches!(
            state["state"].as_str(),
            Some("stale" | "failed" | "conflicted")
        ));
        assert_eq!(victim_before, victim_after_apply);
    } else {
        victim_after_apply = victim_before.clone();
        eprintln!("link creation unsupported on this host; junction evidence skipped");
    }

    // Remove the reparse point itself, never its target, and restore the repo.
    #[cfg(windows)]
    {
        if linked {
            fs::remove_dir(&repo_src).unwrap();
        }
    }
    #[cfg(unix)]
    {
        if linked {
            fs::remove_file(&repo_src).unwrap();
        }
    }
    fs::rename(&saved_src, &repo_src).unwrap();
    assert_eq!(victim_before, victim_after_apply);
    assert!(fixture.store.integrity_check().unwrap().is_empty());
    write_evidence(
        &fixture,
        "AT049",
        json!({
            "platform": std::env::consts::OS,
            "attack": "target parent replaced by a junction/symlink after preparation",
            "linked_path_supported": linked,
            "outside_file_unchanged": true,
            "outside_victim_digest": victim_before,
        }),
        None,
    );
}

#[test]
fn case_only_move_new_file_and_delete_are_recoverable() {
    let mut fixture = common::setup("b021-case");
    // Prepare a case-only move through a real staging rename.
    common::approved_plan(&mut fixture, "plan.b021.case", &["src"]);
    common::create_changeset(&mut fixture, "changeset.b021.case", "plan.b021.case", &[]);
    let seeded = common::prepare(&mut fixture, "changeset.b021.case", 1, None, "seed").unwrap();
    let staging = common::staging_path(&fixture, &seeded);
    fs::rename(staging.join("src/Name.py"), staging.join("src/name.py")).unwrap();
    let case_move_transform = common::transform(
        &fixture,
        "native_rename",
        json!([{"from": "src/Name.py", "to": "src/name.py"}]),
    );
    let moved = common::prepare(
        &mut fixture,
        "changeset.b021.case",
        seeded["revision"].as_i64().unwrap(),
        Some(case_move_transform),
        "case-move",
    )
    .unwrap();
    let operations = moved["operations"].as_array().unwrap();
    assert_eq!(operations.len(), 1);
    assert_eq!(operations[0]["op"], "move");
    assert_eq!(operations[0]["case_only"], true);
    assert_eq!(
        operations[0]["before"]["digest"],
        operations[0]["after"]["digest"]
    );
    let approval = common::approve_prepared(
        &mut fixture,
        "changeset.b021.case",
        moved["revision"].as_i64().unwrap(),
        moved["prepared_digest"].as_str().unwrap(),
    );
    let applied = common::apply(
        &mut fixture,
        "changeset.b021.case",
        moved["revision"].as_i64().unwrap(),
        moved["prepared_digest"].as_str().unwrap(),
        approval["approval"]["approval_id"].as_str().unwrap(),
        "apply-case-move",
    )
    .unwrap();
    assert_eq!(applied["state"], "applied");
    let names = directory_names(&fixture.root.join("src"));
    assert!(names.contains("name.py"), "{names:?}");
    assert!(!names.contains("Name.py"), "{names:?}");
    assert_eq!(
        common::read_text(&fixture.root, "src/name.py"),
        "NAME = 'case'\n"
    );

    // Repeating the exact request is idempotent.
    let replay = common::apply(
        &mut fixture,
        "changeset.b021.case",
        moved["revision"].as_i64().unwrap(),
        moved["prepared_digest"].as_str().unwrap(),
        approval["approval"]["approval_id"].as_str().unwrap(),
        "apply-case-move",
    )
    .unwrap();
    assert_eq!(replay["state"], "applied");

    let recovered = common::recover(
        &mut fixture,
        "changeset.b021.case",
        applied["revision"].as_i64().unwrap(),
        "recover-case-move",
    )
    .unwrap();
    assert_eq!(recovered["state"], "recovered");
    let names = directory_names(&fixture.root.join("src"));
    assert!(names.contains("Name.py"), "{names:?}");
    assert!(!names.contains("name.py"), "{names:?}");

    // A move onto an already existing base path is refused during preparation.
    common::approved_plan(&mut fixture, "plan.b021.exists", &["src"]);
    common::create_changeset(
        &mut fixture,
        "changeset.b021.exists",
        "plan.b021.exists",
        &[],
    );
    let seeded = common::prepare(&mut fixture, "changeset.b021.exists", 1, None, "seed").unwrap();
    let staging = common::staging_path(&fixture, &seeded);
    fs::rename(staging.join("src/a.py"), staging.join("src/b.py")).unwrap();
    let exists_transform = common::transform(
        &fixture,
        "native_rename",
        json!([{"from": "src/a.py", "to": "src/b.py"}]),
    );
    let error = common::prepare(
        &mut fixture,
        "changeset.b021.exists",
        seeded["revision"].as_i64().unwrap(),
        Some(exists_transform),
        "exists",
    )
    .unwrap_err();
    assert!(error.to_string().contains("already exists"), "{error}");

    // A newly created file that the user later edits is preserved on recovery.
    let change = prepared(
        &mut fixture,
        "plan.b021.newfile",
        "changeset.b021.newfile",
        &["src"],
        &[],
        &[("src/created.py", "CREATED = 1\n")],
        "manual_edit",
        json!([]),
    );
    let applied = common::apply(
        &mut fixture,
        &change.changeset_id,
        change.revision,
        &change.prepared_digest,
        &change.approval_id,
        "apply-newfile",
    )
    .unwrap();
    assert_eq!(applied["state"], "applied");
    common::write_file(&fixture.root, "src/created.py", "USER_CHANGED = 1\n");
    let recovered = common::recover(
        &mut fixture,
        &change.changeset_id,
        applied["revision"].as_i64().unwrap(),
        "recover-newfile",
    )
    .unwrap();
    assert_eq!(recovered["state"], "conflicted");
    assert_eq!(
        common::read_text(&fixture.root, "src/created.py"),
        "USER_CHANGED = 1\n"
    );
    assert!(fixture.store.integrity_check().unwrap().is_empty());
    write_evidence(
        &fixture,
        "AT078",
        json!({
            "case_only_move_applied": true,
            "case_only_move_recovered": true,
            "move_to_existing_target": "rejected during preparation",
            "user_edited_new_file_preserved": true,
            "recovery_state": recovered["state"],
        }),
        Some(&recovered),
    );
}

fn directory_names(dir: &Path) -> BTreeSet<String> {
    fs::read_dir(dir)
        .unwrap()
        .map(|entry| entry.unwrap().file_name().to_string_lossy().to_string())
        .collect()
}

fn write_evidence(
    fixture: &common::Fixture,
    case_id: &str,
    case: Value,
    changeset: Option<&Value>,
) {
    if std::env::var("ARCHMELD_WRITE_EVIDENCE").ok().as_deref() != Some("1") {
        return;
    }
    let _guard = EVIDENCE_LOCK.lock().unwrap();
    let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../docs/evidence/B021-apply.json");
    let mut root = if path.exists() {
        serde_json::from_str::<Value>(&fs::read_to_string(&path).unwrap())
            .unwrap_or_else(|_| json!({}))
    } else {
        json!({
            "generated_by": "crates/archmeld-core/tests/apply_recovery.rs",
            "platform": std::env::consts::OS,
            "project_id": fixture.project,
            "snapshot_id": fixture.snapshot_id,
            "acceptance": {},
        })
    };
    root["project_id"] = json!(fixture.project);
    root["snapshot_id"] = json!(fixture.snapshot_id);
    root["acceptance"][case_id] = case;
    if let Some(changeset) = changeset {
        root["acceptance"][case_id]["changeset_state"] = changeset["state"].clone();
        root["acceptance"][case_id]["resulting_snapshot_id"] =
            changeset["resulting_snapshot_id"].clone();
    }
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(path, serde_json::to_string_pretty(&root).unwrap() + "\n").unwrap();
}
