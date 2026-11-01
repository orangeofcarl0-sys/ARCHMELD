// Copyright 2026 ARCHMELD contributors
// SPDX-License-Identifier: Apache-2.0

//! B025: passphrase-encrypted backup creation, authentication, tamper
//! detection and restore. The passphrase is never written to the bundle.

use archmeld_core::encrypted_backup::{
    create_encrypted_backup, restore_encrypted_backup, retain_encrypted_backup,
    rotate_encrypted_backup, verify_encrypted_backup,
};
use archmeld_core::store::Store;
use serde_json::json;
use std::fs;

const PASSPHRASE: &str = "correct horse battery staple";
const PROJECT: &str = "project.backup";

#[test]
fn encrypted_backup_authenticates_and_restores() {
    let dir = tempfile::tempdir().unwrap();
    let state = dir.path().join("state");
    let mut store = Store::open(&state).unwrap();
    store.ensure_project(PROJECT, "expedition").unwrap();
    let blob = store
        .put_blob(b"source text that must not appear in the manifest", "text/plain")
        .unwrap();

    let destination = dir.path().join("backup-enc");
    let created = create_encrypted_backup(&mut store, PROJECT, &destination, PASSPHRASE).unwrap();
    assert_eq!(created["encrypted"], true);
    assert_eq!(created["cipher"], "aes-256-gcm");
    assert!(created["file_count"].as_u64().unwrap() >= 2);
    assert_eq!(created["passphrase_recorded"], false);

    let manifest = fs::read_to_string(destination.join("manifest.json")).unwrap();
    assert!(!manifest.contains(PASSPHRASE));
    assert!(!manifest.contains("source text that must not appear"));
    assert!(!manifest.contains(&format!("sha256:{}", "0".repeat(64))));

    let verified = verify_encrypted_backup(&destination, PASSPHRASE).unwrap();
    assert_eq!(verified["encrypted"], true);
    assert_eq!(verified["checked_files"], created["file_count"]);
    assert!(verify_encrypted_backup(&destination, "wrong passphrase").is_err());

    // Flip one ciphertext byte: the per-file digest and GCM tag must reject it.
    let encrypted_file = destination.join("files").join("backup.json.enc");
    let mut bytes = fs::read(&encrypted_file).unwrap();
    bytes[16] ^= 0x01;
    fs::write(&encrypted_file, &bytes).unwrap();
    assert!(verify_encrypted_backup(&destination, PASSPHRASE).is_err());

    // The restore target must not exist yet; a failed verification must not create it.
    let restore = dir.path().join("restored");
    assert!(restore_encrypted_backup(&destination, PASSPHRASE, &restore).is_err());
    assert!(!restore.exists());

    // Recreate and restore successfully.
    let destination = dir.path().join("backup-enc-2");
    create_encrypted_backup(&mut store, PROJECT, &destination, PASSPHRASE).unwrap();
    let restored = restore_encrypted_backup(&destination, PASSPHRASE, &restore).unwrap();
    assert_eq!(restored["encrypted"], true);
    assert!(restored["restored_files"].as_u64().unwrap() >= 2);
    let restored_store = Store::open(&restore).unwrap();
    assert!(restored_store.integrity_check().unwrap().is_empty());
    assert_eq!(
        restored_store
            .read_blob_bytes(&blob)
            .unwrap()
            .unwrap()
            .as_slice(),
        b"source text that must not appear in the manifest"
    );
    assert!(archmeld_core::manager::get_project(&restored_store, PROJECT)
        .unwrap()
        .is_some());
    assert!(restore_encrypted_backup(&destination, PASSPHRASE, &restore).is_err());
    let _ = json!({"checked": verified["checked_files"]});
}

#[test]
fn rotation_reencrypts_with_a_new_key_and_records_provenance() {
    let dir = tempfile::tempdir().unwrap();
    let state = dir.path().join("state");
    let mut store = Store::open(&state).unwrap();
    store.ensure_project(PROJECT, "expedition").unwrap();
    let blob = store.put_blob(b"rotation payload", "text/plain").unwrap();
    let source = dir.path().join("backup-old");
    let created = create_encrypted_backup(&mut store, PROJECT, &source, PASSPHRASE).unwrap();
    let old_key = created["key_provenance"]["key_id"].as_str().unwrap().to_string();
    assert_eq!(created["key_provenance"]["generation"], 1);
    assert_eq!(created["key_provenance"]["rotated_from"], serde_json::Value::Null);
    assert_eq!(old_key.len(), 32);

    let rotated_dir = dir.path().join("backup-new");
    let new_passphrase = "a different long passphrase";
    let rotated = rotate_encrypted_backup(&source, PASSPHRASE, new_passphrase, &rotated_dir).unwrap();
    assert_eq!(rotated["rotated"], true);
    assert_eq!(rotated["key_provenance"]["generation"], 2);
    assert_eq!(rotated["key_provenance"]["rotated_from"], old_key.as_str());
    assert_ne!(rotated["key_provenance"]["key_id"], old_key.as_str());
    assert_eq!(rotated["file_count"], created["file_count"]);
    assert!(verify_encrypted_backup(&rotated_dir, PASSPHRASE).is_err());
    let verified = verify_encrypted_backup(&rotated_dir, new_passphrase).unwrap();
    assert_eq!(verified["key_provenance"]["generation"], 2);
    assert_eq!(verified["key_provenance"]["rotated_from"], old_key.as_str());

    let restore = dir.path().join("rotated-restore");
    restore_encrypted_backup(&rotated_dir, new_passphrase, &restore).unwrap();
    let restored = Store::open(&restore).unwrap();
    assert!(restored.integrity_check().unwrap().is_empty());
    assert_eq!(restored.read_blob_bytes(&blob).unwrap().unwrap(), b"rotation payload".to_vec());

    let wrong_source = rotate_encrypted_backup(
        &source,
        "wrong old passphrase",
        new_passphrase,
        &dir.path().join("never"),
    );
    assert!(wrong_source.is_err());
    assert!(!dir.path().join("never").exists());
    let occupied = dir.path().join("occupied");
    fs::create_dir_all(&occupied).unwrap();
    fs::write(occupied.join("keep"), b"keep").unwrap();
    assert!(rotate_encrypted_backup(&source, PASSPHRASE, new_passphrase, &occupied).is_err());
    assert_eq!(fs::read(occupied.join("keep")).unwrap(), b"keep");
}

#[test]
fn path_retention_copies_verifies_and_reports_key_provenance() {
    let dir = tempfile::tempdir().unwrap();
    let state = dir.path().join("state");
    let mut store = Store::open(&state).unwrap();
    store.ensure_project(PROJECT, "expedition").unwrap();
    store.put_blob(b"retention payload", "text/plain").unwrap();
    let source = dir.path().join("backup-source");
    let created = create_encrypted_backup(&mut store, PROJECT, &source, PASSPHRASE).unwrap();
    let target_root = dir.path().join("offhost");
    let retained = retain_encrypted_backup(&source, &target_root, "retention.2026-10-31", PASSPHRASE).unwrap();
    assert_eq!(retained["verified"], true);
    assert_eq!(retained["target_kind"], "path");
    assert_eq!(retained["checked_files"], created["file_count"]);
    assert_eq!(retained["key_provenance"]["key_id"], created["key_provenance"]["key_id"]);
    let target = target_root.join("retention.2026-10-31");
    assert!(target.join("retention.json").is_file());
    assert_eq!(
        verify_encrypted_backup(&target, PASSPHRASE).unwrap()["checked_files"],
        created["file_count"]
    );
    assert!(retain_encrypted_backup(&source, &target_root, "retention.2026-10-31", PASSPHRASE).is_err());
    assert!(retain_encrypted_backup(&source, &target_root, "../escape", PASSPHRASE).is_err());
    assert!(retain_encrypted_backup(&source, &target_root, "retention.wrong", "wrong").is_err());
    assert!(!target_root.join("retention.wrong").exists());

    // Tamper with the retained copy: verification must fail even though the
    // retention record claims success at copy time.
    let encrypted_file = target.join("files").join("backup.json.enc");
    let mut bytes = fs::read(&encrypted_file).unwrap();
    bytes[20] ^= 0x01;
    fs::write(&encrypted_file, &bytes).unwrap();
    assert!(verify_encrypted_backup(&target, PASSPHRASE).is_err());
}
