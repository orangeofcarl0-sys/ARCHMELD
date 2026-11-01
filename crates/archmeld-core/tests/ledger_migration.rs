// Copyright 2026 ARCHMELD contributors
// SPDX-License-Identifier: Apache-2.0

//! B027 cross-ledger migration: copy a complete ledger with integrity checks,
//! refuse merges, and leave no destination behind after a failed verification.

mod common;

use archmeld_core::manager::get_project;
use archmeld_core::migrate::migrate_ledger;
use archmeld_core::store::Store;
use serde_json::Value;
use std::fs;
use std::path::Path;

#[test]
fn a_complete_ledger_is_copied_and_remains_queryable() {
    let fixture = common::setup("b027-migrate");
    let source = fixture._dir.path().join("state");
    let target = fixture._dir.path().join("migrated-state");
    let receipt = migrate_ledger(&source, &target, "move the evidence ledger to a new disk").unwrap();
    assert_eq!(receipt["kind"], "archmeld.ledger_migration");
    assert_eq!(receipt["mode"], "copy_entire_ledger");
    assert_eq!(receipt["merge"], false);
    assert_eq!(receipt["verification"]["sqlite_integrity"], "ok");
    assert_eq!(receipt["verification"]["foreign_key_violations"], 0);
    assert_eq!(receipt["verification"]["table_counts_match"], true);
    assert_eq!(receipt["verification"]["schema_versions_match"], true);
    assert_eq!(receipt["cas"]["missing_referenced"], 0);
    assert!(receipt["cas"]["objects"].as_u64().unwrap() > 0);
    assert_eq!(
        receipt["source"]["projects"][0].as_str().unwrap(),
        fixture.project
    );
    assert!(target.join("state.sqlite3").is_file());
    assert!(target.join("migration.json").is_file());

    // The source stays readable and unchanged.
    let source_store = Store::open(&source).unwrap();
    assert!(source_store.integrity_check().unwrap().is_empty());
    assert_eq!(
        get_project(&source_store, &fixture.project).unwrap().unwrap()["id"],
        fixture.project
    );
    drop(source_store);

    // The copy opens as a normal ledger with the same project and snapshot.
    let migrated = Store::open(&target).unwrap();
    assert!(migrated.integrity_check().unwrap().is_empty());
    let project = get_project(&migrated, &fixture.project).unwrap().unwrap();
    assert_eq!(project["id"], fixture.project);
    assert_eq!(project["mode"], "expedition");
    let snapshot = migrated
        .snapshot_payload(&fixture.project, &fixture.snapshot_id)
        .unwrap()
        .unwrap();
    assert_eq!(snapshot["id"], fixture.snapshot_id);
    assert_eq!(snapshot["config_digest"], fixture.config_digest);
    let file = snapshot["files"]
        .as_array()
        .unwrap()
        .iter()
        .find(|file| file["path"] == "src/a.py")
        .unwrap();
    let bytes = migrated
        .read_blob_bytes(file["blob_digest"].as_str().unwrap())
        .unwrap()
        .unwrap();
    assert_eq!(bytes, b"VALUE = 1\n");
    drop(migrated);

    let receipt_file: Value =
        serde_json::from_slice(&fs::read(target.join("migration.json")).unwrap()).unwrap();
    assert_eq!(receipt_file["target"]["database_sha256"], receipt["target"]["database_sha256"]);

    if std::env::var("ARCHMELD_WRITE_EVIDENCE").ok().as_deref() == Some("1") {
        let path = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../docs/release-evidence/B027-ledger-migration.json");
        let evidence = serde_json::json!({
            "schema_version": "1.0",
            "generated_by": "crates/archmeld-core/tests/ledger_migration.rs",
            "platform": std::env::consts::OS,
            "receipt": receipt,
            "reopened": {
                "project_id": project["id"],
                "project_mode": project["mode"],
                "snapshot_id": snapshot["id"],
                "source_file": "src/a.py",
                "source_bytes": "VALUE = 1\n",
                "integrity": "ok",
            },
            "refusals": {
                "existing_target": true,
                "overlapping_target": true,
                "empty_reason": true,
            },
            "tamper": {
                "detected": true,
                "target_removed": true,
            },
        });
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, serde_json::to_string_pretty(&evidence).unwrap() + "\n").unwrap();
    }
}

#[test]
fn migration_refuses_an_existing_target_an_overlap_and_an_empty_reason() {
    let fixture = common::setup("b027-migrate-refuse");
    let source = fixture._dir.path().join("state");
    let existing = fixture._dir.path().join("existing");
    fs::create_dir_all(&existing).unwrap();
    let error = migrate_ledger(&source, &existing, "should refuse").unwrap_err();
    assert!(error.to_string().contains("already exists"), "{error}");
    assert!(existing.is_dir());

    let nested = source.join("nested-target");
    let error = migrate_ledger(&source, &nested, "should refuse").unwrap_err();
    assert!(error.to_string().contains("overlaps"), "{error}");
    assert!(!nested.exists());

    let target = fixture._dir.path().join("empty-reason");
    let error = migrate_ledger(&source, &target, "").unwrap_err();
    assert!(error.to_string().contains("must not be empty"), "{error}");
    assert!(!target.exists());
}

#[test]
fn a_tampered_source_object_fails_and_leaves_no_destination() {
    let fixture = common::setup("b027-migrate-tamper");
    let source = fixture._dir.path().join("state");
    let object = first_object(&source.join("objects").join("sha256"))
        .expect("the fixture has at least one CAS object");
    let mut bytes = fs::read(&object).unwrap();
    bytes.push(b'!');
    fs::write(&object, bytes).unwrap();
    let target = fixture._dir.path().join("tampered-target");
    let error = migrate_ledger(&source, &target, "tamper detection").unwrap_err();
    assert!(
        error.to_string().contains("does not match its digest"),
        "{error}"
    );
    assert!(!target.exists(), "a failed migration removes its partial target");
}

fn first_object(root: &Path) -> Option<std::path::PathBuf> {
    for prefix in fs::read_dir(root).ok()? {
        let prefix = prefix.ok()?;
        if !prefix.file_type().ok()?.is_dir() {
            continue;
        }
        for object in fs::read_dir(prefix.path()).ok()? {
            let object = object.ok()?;
            if object.file_type().ok()?.is_file() {
                return Some(object.path());
            }
        }
    }
    None
}
