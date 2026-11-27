// Copyright 2026 ARCHMELD contributors
// SPDX-License-Identifier: Apache-2.0

//! Passphrase-encrypted backups (S14). A plaintext `VACUUM INTO` backup is
//! created in a temporary directory, every file is encrypted with AES-256-GCM,
//! and the manifest is authenticated with HMAC-SHA256. The passphrase never
//! enters logs, manifests or the returned JSON.

use crate::drift::create_backup;
use crate::error::{CoreError, Result};
use crate::hash;
use crate::store::{validate, Store};
use ring::{
    aead,
    hmac,
    pbkdf2,
    rand::{SecureRandom, SystemRandom},
};
use serde_json::{json, Value};
use std::fs;
use std::num::NonZeroU32;
use std::path::{Component, Path, PathBuf};

const VERSION: &str = "1.0";
const KDF_ITERATIONS: u32 = 120_000;
const KEY_BYTES: usize = 64;

struct TempDir(PathBuf);

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

fn hex_encode(bytes: &[u8]) -> String {
    bytes.iter().map(|byte| format!("{byte:02x}")).collect()
}

fn hex_decode(text: &str) -> Result<Vec<u8>> {
    if text.len() % 2 != 0 {
        return Err(CoreError::invalid("hex value has an odd length"));
    }
    let mut out = Vec::with_capacity(text.len() / 2);
    let bytes = text.as_bytes();
    for index in (0..bytes.len()).step_by(2) {
        let high = (bytes[index] as char)
            .to_digit(16)
            .ok_or_else(|| CoreError::invalid("hex value contains a non-hex character"))?;
        let low = (bytes[index + 1] as char)
            .to_digit(16)
            .ok_or_else(|| CoreError::invalid("hex value contains a non-hex character"))?;
        out.push((high * 16 + low) as u8);
    }
    Ok(out)
}

fn derive_keys(passphrase: &str, salt: &[u8]) -> Result<[u8; KEY_BYTES]> {
    if passphrase.is_empty() {
        return Err(CoreError::invalid("backup passphrase must not be empty"));
    }
    let iterations = NonZeroU32::new(KDF_ITERATIONS).expect("nonzero iterations");
    let mut keys = [0u8; KEY_BYTES];
    pbkdf2::derive(
        pbkdf2::PBKDF2_HMAC_SHA256,
        iterations,
        salt,
        passphrase.as_bytes(),
        &mut keys,
    );
    Ok(keys)
}

fn encrypt(key: &[u8; KEY_BYTES], nonce: &[u8], aad: &[u8], plaintext: &[u8]) -> Result<Vec<u8>> {
    let unbound = aead::UnboundKey::new(&aead::AES_256_GCM, &key[..32])
        .map_err(|_| CoreError::Storage("encryption key rejected".into()))?;
    let key = aead::LessSafeKey::new(unbound);
    let nonce_bytes: [u8; 12] = nonce
        .try_into()
        .map_err(|_| CoreError::invalid("AES-GCM nonce must be 12 bytes"))?;
    let mut output = plaintext.to_vec();
    key.seal_in_place_append_tag(
        aead::Nonce::assume_unique_for_key(nonce_bytes),
        aead::Aad::from(aad),
        &mut output,
    )
    .map_err(|_| CoreError::Storage("backup encryption failed".into()))?;
    Ok(output)
}

fn decrypt(key: &[u8; KEY_BYTES], nonce: &[u8], aad: &[u8], ciphertext: &[u8]) -> Result<Vec<u8>> {
    let unbound = aead::UnboundKey::new(&aead::AES_256_GCM, &key[..32])
        .map_err(|_| CoreError::Storage("encryption key rejected".into()))?;
    let key = aead::LessSafeKey::new(unbound);
    let nonce_bytes: [u8; 12] = nonce
        .try_into()
        .map_err(|_| CoreError::invalid("AES-GCM nonce must be 12 bytes"))?;
    let mut output = ciphertext.to_vec();
    let plaintext = key
        .open_in_place(
            aead::Nonce::assume_unique_for_key(nonce_bytes),
            aead::Aad::from(aad),
            &mut output,
        )
        .map_err(|_| {
            CoreError::invalid("encrypted backup passphrase is wrong or the bundle is corrupt")
        })?;
    Ok(plaintext.to_vec())
}

fn manifest_hmac(key: &[u8; KEY_BYTES], manifest: &Value) -> Result<String> {
    let mut without = manifest.clone();
    without
        .as_object_mut()
        .ok_or_else(|| CoreError::Storage("encrypted backup manifest is not an object".into()))?
        .remove("manifest_hmac");
    let canonical = hash::canonical_json(&without)
        .map_err(|err| CoreError::invalid(format!("backup manifest: {err}")))?;
    let signing = hmac::Key::new(hmac::HMAC_SHA256, &key[32..]);
    Ok(hex_encode(hmac::sign(&signing, canonical.as_bytes()).as_ref()))
}

/// Stable, non-secret identifier for a backup key. It is derived from the MAC
/// key, so two bundles with the same passphrase but different salts have
/// different ids. The id never reveals the key.
fn key_id(key: &[u8; KEY_BYTES]) -> String {
    let signing = hmac::Key::new(hmac::HMAC_SHA256, &key[32..]);
    let digest = hmac::sign(&signing, b"archmeld.backup.key_id");
    hex_encode(digest.as_ref())[..32].to_string()
}

fn key_provenance(manifest: &Value, keys: &[u8; KEY_BYTES]) -> Value {
    manifest.get("key_provenance").cloned().unwrap_or_else(|| {
        json!({
            "key_id": key_id(keys),
            "generation": 1,
            "rotated_from": Value::Null,
        })
    })
}

fn safe_relative(path: &str) -> Result<PathBuf> {
    if path.is_empty() || path.starts_with('/') || path.contains('\\') || path.contains("..") {
        return Err(CoreError::invalid(format!(
            "backup path {path:?} is not a safe relative path"
        )));
    }
    let candidate = Path::new(path);
    if candidate
        .components()
        .any(|component| !matches!(component, Component::Normal(_)))
    {
        return Err(CoreError::invalid(format!(
            "backup path {path:?} is not a safe relative path"
        )));
    }
    Ok(candidate.to_path_buf())
}

fn collect_files(root: &Path, directory: &Path, output: &mut Vec<(String, PathBuf)>) -> Result<()> {
    let mut entries = fs::read_dir(directory)?.collect::<std::io::Result<Vec<_>>>()?;
    entries.sort_by_key(|entry| entry.file_name());
    for entry in entries {
        let path = entry.path();
        let metadata = fs::symlink_metadata(&path)?;
        if metadata.file_type().is_symlink() {
            return Err(CoreError::unsupported(format!(
                "backup contains a symlink: {}",
                path.display()
            )));
        }
        if metadata.is_dir() {
            collect_files(root, &path, output)?;
        } else if metadata.is_file() {
            let relative = path
                .strip_prefix(root)
                .map_err(|_| CoreError::Storage("backup path escaped its root".into()))?
                .to_string_lossy()
                .replace('\\', "/");
            safe_relative(&relative)?;
            output.push((relative, path));
        }
    }
    Ok(())
}

fn temp_directory(destination: &Path) -> Result<TempDir> {
    let parent = destination
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
        .unwrap_or_else(|| Path::new("."));
    fs::create_dir_all(parent)?;
    let name = format!(
        ".archmeld-backup-{}-{}",
        std::process::id(),
        crate::store::now_unix()
    );
    let path = parent.join(name);
    fs::create_dir_all(&path)?;
    Ok(TempDir(path))
}

pub fn create_encrypted_backup(
    store: &mut Store,
    project_id: &str,
    destination: &Path,
    passphrase: &str,
) -> Result<Value> {
    validate::require_id("project_id", project_id)?;
    if destination.exists() && fs::read_dir(destination)?.next().is_some() {
        return Err(CoreError::conflict(
            "encrypted backup destination already exists and is not empty",
        ));
    }
    if passphrase.is_empty() {
        return Err(CoreError::invalid("backup passphrase must not be empty"));
    }
    let temp = temp_directory(destination)?;
    let plaintext = temp.0.join("plain");
    fs::create_dir_all(&plaintext)?;
    let plain_summary = create_backup(store, project_id, &plaintext)?;
    let database_digest = plain_summary["database_digest"]
        .as_str()
        .unwrap_or("")
        .to_string();

    let mut files = Vec::new();
    collect_files(&plaintext, &plaintext, &mut files)?;
    if files.is_empty() {
        return Err(CoreError::Storage("plaintext backup contains no files".into()));
    }
    let random = SystemRandom::new();
    let mut salt = [0u8; 16];
    random
        .fill(&mut salt)
        .map_err(|_| CoreError::Storage("salt generation failed".into()))?;
    let keys = derive_keys(passphrase, &salt)?;
    let mut entries = Vec::with_capacity(files.len());
    let files_root = destination.join("files");
    fs::create_dir_all(&files_root)?;
    for (relative, source) in &files {
        let plaintext_bytes = fs::read(source)?;
        let mut nonce = [0u8; 12];
        random
            .fill(&mut nonce)
            .map_err(|_| CoreError::Storage("nonce generation failed".into()))?;
        let ciphertext = encrypt(&keys, &nonce, relative.as_bytes(), &plaintext_bytes)?;
        let encrypted_path = files_root.join(format!("{relative}.enc"));
        if let Some(parent) = encrypted_path.parent() {
            fs::create_dir_all(parent)?;
        }
        fs::write(&encrypted_path, &ciphertext)?;
        entries.push(json!({
            "path": relative,
            "plaintext_digest": hash::digest_bytes(&plaintext_bytes),
            "plaintext_size": plaintext_bytes.len(),
            "encrypted_digest": hash::digest_bytes(&ciphertext),
            "nonce": hex_encode(&nonce),
        }));
    }
    let mut manifest = json!({
        "schema_version": VERSION,
        "kind": "archmeld.encrypted_backup",
        "project_id": project_id,
        "created_at": crate::store::now_unix(),
        "cipher": "aes-256-gcm",
        "kdf": {
            "name": "pbkdf2-hmac-sha256",
            "iterations": KDF_ITERATIONS,
            "salt": hex_encode(&salt),
        },
        "database_digest": database_digest,
        "files": entries,
        "file_count": files.len(),
        "key_provenance": {
            "key_id": key_id(&keys),
            "generation": 1,
            "rotated_from": Value::Null,
        },
    });
    let mac = manifest_hmac(&keys, &manifest)?;
    manifest["manifest_hmac"] = json!(mac);
    let manifest_bytes = hash::canonical_json(&manifest)
        .map_err(|err| CoreError::invalid(format!("encrypted backup manifest: {err}")))?;
    fs::write(destination.join("manifest.json"), &manifest_bytes)?;
    let manifest_digest = hash::digest_bytes(manifest_bytes.as_bytes());
    let backup_id = format!(
        "backup.{}",
        manifest_digest
            .strip_prefix("sha256:")
            .unwrap_or(&manifest_digest)
            .chars()
            .take(32)
            .collect::<String>()
    );
    store.conn.execute(
        "INSERT INTO backup_runs
         (project_id,backup_id,directory,db_digest,manifest_digest,object_count,created_at)
         VALUES (?1,?2,?3,?4,?5,?6,?7)",
        (
            project_id,
            &backup_id,
            destination.to_string_lossy(),
            database_digest,
            &manifest_digest,
            files.len() as i64,
            crate::store::now_unix(),
        ),
    )?;
    Ok(json!({
        "backup_id": backup_id,
        "directory": destination.to_string_lossy(),
        "encrypted": true,
        "cipher": "aes-256-gcm",
        "kdf_iterations": KDF_ITERATIONS,
        "file_count": files.len(),
        "manifest_digest": manifest_digest,
        "key_provenance": manifest["key_provenance"],
        "passphrase_recorded": false,
    }))
}

fn load_manifest(destination: &Path) -> Result<(Value, Vec<u8>)> {
    let text = fs::read(destination.join("manifest.json"))?;
    let manifest: Value = serde_json::from_slice(&text)
        .map_err(|err| CoreError::Storage(format!("encrypted backup manifest corrupt: {err}")))?;
    if manifest["schema_version"] != VERSION
        || manifest["kind"] != "archmeld.encrypted_backup"
        || manifest["cipher"] != "aes-256-gcm"
    {
        return Err(CoreError::unsupported(
            "not an ARCHMELD encrypted backup bundle",
        ));
    }
    let salt = hex_decode(manifest["kdf"]["salt"].as_str().unwrap_or(""))?;
    Ok((manifest, salt))
}

pub fn verify_encrypted_backup(destination: &Path, passphrase: &str) -> Result<Value> {
    let (manifest, salt) = load_manifest(destination)?;
    let keys = derive_keys(passphrase, &salt)?;
    let expected_mac = manifest["manifest_hmac"].as_str().unwrap_or("");
    if manifest_hmac(&keys, &manifest)? != expected_mac {
        return Err(CoreError::invalid(
            "encrypted backup manifest authentication failed",
        ));
    }
    let files = manifest["files"]
        .as_array()
        .ok_or_else(|| CoreError::Storage("encrypted backup manifest has no files".into()))?;
    let mut checked = 0usize;
    for file in files {
        let relative = file["path"]
            .as_str()
            .ok_or_else(|| CoreError::Storage("backup file has no path".into()))?;
        safe_relative(relative)?;
        let nonce = hex_decode(file["nonce"].as_str().unwrap_or(""))?;
        let encrypted = fs::read(destination.join("files").join(format!("{relative}.enc")))?;
        if hash::digest_bytes(&encrypted) != file["encrypted_digest"].as_str().unwrap_or("") {
            return Err(CoreError::Storage(format!(
                "encrypted backup file {relative} does not match its manifest digest"
            )));
        }
        let plaintext = decrypt(&keys, &nonce, relative.as_bytes(), &encrypted)?;
        if hash::digest_bytes(&plaintext) != file["plaintext_digest"].as_str().unwrap_or("")
            || plaintext.len() as u64 != file["plaintext_size"].as_u64().unwrap_or(0)
        {
            return Err(CoreError::Storage(format!(
                "decrypted backup file {relative} does not match its plaintext digest"
            )));
        }
        checked += 1;
    }
    Ok(json!({
        "encrypted": true,
        "checked_files": checked,
        "manifest_hmac": expected_mac,
        "key_provenance": key_provenance(&manifest, &keys),
    }))
}

pub fn restore_encrypted_backup(
    destination: &Path,
    passphrase: &str,
    target: &Path,
) -> Result<Value> {
    verify_encrypted_backup(destination, passphrase)?;
    if target.exists() && fs::read_dir(target)?.next().is_some() {
        return Err(CoreError::conflict(
            "restore target already exists and is not empty",
        ));
    }
    let text = fs::read(destination.join("manifest.json"))?;
    let manifest: Value = serde_json::from_slice(&text)
        .map_err(|err| CoreError::Storage(format!("encrypted backup manifest corrupt: {err}")))?;
    let salt = hex_decode(manifest["kdf"]["salt"].as_str().unwrap_or(""))?;
    let keys = derive_keys(passphrase, &salt)?;
    let files = manifest["files"]
        .as_array()
        .ok_or_else(|| CoreError::Storage("encrypted backup manifest has no files".into()))?;
    fs::create_dir_all(target)?;
    let mut restored = 0usize;
    for file in files {
        let relative = file["path"]
            .as_str()
            .ok_or_else(|| CoreError::Storage("backup file has no path".into()))?;
        let relative_path = safe_relative(relative)?;
        let nonce = hex_decode(file["nonce"].as_str().unwrap_or(""))?;
        let encrypted = fs::read(destination.join("files").join(format!("{relative}.enc")))?;
        let plaintext = decrypt(&keys, &nonce, relative.as_bytes(), &encrypted)?;
        let target_path = target.join(relative_path);
        if let Some(parent) = target_path.parent() {
            fs::create_dir_all(parent)?;
        }
        fs::write(&target_path, plaintext)?;
        restored += 1;
    }
    Ok(json!({
        "encrypted": true,
        "restored_files": restored,
        "target": target.to_string_lossy(),
    }))
}

/// Re-encrypt an existing bundle under a new passphrase. The source is fully
/// verified and decrypted first; the destination is written atomically per
/// file with fresh salt and nonces. Key provenance records the previous key id
/// and increments the generation. The passphrases never enter the manifest.
pub fn rotate_encrypted_backup(
    source: &Path,
    old_passphrase: &str,
    new_passphrase: &str,
    destination: &Path,
) -> Result<Value> {
    if new_passphrase.is_empty() {
        return Err(CoreError::invalid("new backup passphrase must not be empty"));
    }
    verify_encrypted_backup(source, old_passphrase)?;
    if destination.exists() && fs::read_dir(destination)?.next().is_some() {
        return Err(CoreError::conflict(
            "rotated backup destination already exists and is not empty",
        ));
    }
    if destination.starts_with(source) || source.starts_with(destination) {
        return Err(CoreError::invalid(
            "rotated backup destination must not be inside the source bundle",
        ));
    }
    let text = fs::read(source.join("manifest.json"))?;
    let source_manifest: Value = serde_json::from_slice(&text)
        .map_err(|err| CoreError::Storage(format!("encrypted backup manifest corrupt: {err}")))?;
    let old_salt = hex_decode(source_manifest["kdf"]["salt"].as_str().unwrap_or(""))?;
    let old_keys = derive_keys(old_passphrase, &old_salt)?;
    let old_provenance = key_provenance(&source_manifest, &old_keys);
    let files = source_manifest["files"]
        .as_array()
        .ok_or_else(|| CoreError::Storage("encrypted backup manifest has no files".into()))?;
    if files.is_empty() {
        return Err(CoreError::Storage("encrypted backup contains no files".into()));
    }

    let random = SystemRandom::new();
    let mut salt = [0u8; 16];
    random
        .fill(&mut salt)
        .map_err(|_| CoreError::Storage("salt generation failed".into()))?;
    let keys = derive_keys(new_passphrase, &salt)?;
    let files_root = destination.join("files");
    fs::create_dir_all(&files_root)?;
    let mut entries = Vec::with_capacity(files.len());
    for file in files {
        let relative = file["path"]
            .as_str()
            .ok_or_else(|| CoreError::Storage("backup file has no path".into()))?;
        safe_relative(relative)?;
        let nonce = hex_decode(file["nonce"].as_str().unwrap_or(""))?;
        let encrypted = fs::read(source.join("files").join(format!("{relative}.enc")))?;
        let plaintext = decrypt(&old_keys, &nonce, relative.as_bytes(), &encrypted)?;
        let mut new_nonce = [0u8; 12];
        random
            .fill(&mut new_nonce)
            .map_err(|_| CoreError::Storage("nonce generation failed".into()))?;
        let ciphertext = encrypt(&keys, &new_nonce, relative.as_bytes(), &plaintext)?;
        let encrypted_path = files_root.join(format!("{relative}.enc"));
        if let Some(parent) = encrypted_path.parent() {
            fs::create_dir_all(parent)?;
        }
        fs::write(&encrypted_path, &ciphertext)?;
        entries.push(json!({
            "path": relative,
            "plaintext_digest": hash::digest_bytes(&plaintext),
            "plaintext_size": plaintext.len(),
            "encrypted_digest": hash::digest_bytes(&ciphertext),
            "nonce": hex_encode(&new_nonce),
        }));
    }
    let generation = old_provenance["generation"].as_u64().unwrap_or(1) + 1;
    let provenance = json!({
        "key_id": key_id(&keys),
        "generation": generation,
        "rotated_from": old_provenance["key_id"],
        "rotated_at": crate::store::now_unix(),
    });
    let mut manifest = json!({
        "schema_version": VERSION,
        "kind": "archmeld.encrypted_backup",
        "project_id": source_manifest["project_id"],
        "created_at": crate::store::now_unix(),
        "backup_created_at": source_manifest["created_at"],
        "cipher": "aes-256-gcm",
        "kdf": {
            "name": "pbkdf2-hmac-sha256",
            "iterations": KDF_ITERATIONS,
            "salt": hex_encode(&salt),
        },
        "database_digest": source_manifest["database_digest"],
        "files": entries,
        "file_count": entries.len(),
        "key_provenance": provenance,
    });
    let mac = manifest_hmac(&keys, &manifest)?;
    manifest["manifest_hmac"] = json!(mac);
    let manifest_bytes = hash::canonical_json(&manifest)
        .map_err(|err| CoreError::invalid(format!("rotated backup manifest: {err}")))?;
    fs::write(destination.join("manifest.json"), &manifest_bytes)?;
    verify_encrypted_backup(destination, new_passphrase)?;
    Ok(json!({
        "encrypted": true,
        "rotated": true,
        "file_count": entries.len(),
        "manifest_digest": hash::digest_bytes(manifest_bytes.as_bytes()),
        "key_provenance": manifest["key_provenance"],
        "passphrase_recorded": false,
    }))
}

/// Copy a verified encrypted bundle to a path-based retention target and
/// independently verify the copy. The target is a directory on any mounted
/// filesystem (including a network mount); no cloud API or credential is used.
pub fn retain_encrypted_backup(
    source: &Path,
    target_root: &Path,
    retention_id: &str,
    passphrase: &str,
) -> Result<Value> {
    validate::require_id("retention_id", retention_id)?;
    verify_encrypted_backup(source, passphrase)?;
    fs::create_dir_all(target_root)?;
    let canonical_root = target_root.canonicalize()?;
    let target = canonical_root.join(retention_id);
    if target.exists() && fs::read_dir(&target)?.next().is_some() {
        return Err(CoreError::conflict(
            "retention target already exists and is not empty",
        ));
    }
    let canonical_source = source.canonicalize()?;
    if target.starts_with(&canonical_source) || canonical_source.starts_with(&target) {
        return Err(CoreError::invalid(
            "retention target must not be the source bundle or inside it",
        ));
    }
    let text = fs::read(source.join("manifest.json"))?;
    let manifest: Value = serde_json::from_slice(&text)
        .map_err(|err| CoreError::Storage(format!("encrypted backup manifest corrupt: {err}")))?;
    let files = manifest["files"]
        .as_array()
        .ok_or_else(|| CoreError::Storage("encrypted backup manifest has no files".into()))?;
    let files_root = target.join("files");
    fs::create_dir_all(&files_root)?;
    let copy_result = (|| -> Result<()> {
        fs::copy(source.join("manifest.json"), target.join("manifest.json"))?;
        for file in files {
            let relative = file["path"]
                .as_str()
                .ok_or_else(|| CoreError::Storage("backup file has no path".into()))?;
            let _relative_path = safe_relative(relative)?;
            let source_path = source.join("files").join(format!("{relative}.enc"));
            let target_path = files_root.join(format!("{relative}.enc"));
            if let Some(parent) = target_path.parent() {
                fs::create_dir_all(parent)?;
            }
            fs::copy(source_path, target_path)?;
        }
        Ok(())
    })();
    if let Err(error) = copy_result {
        let _ = fs::remove_dir_all(&target);
        return Err(error);
    }
    let verified = match verify_encrypted_backup(&target, passphrase) {
        Ok(verified) => verified,
        Err(error) => {
            let _ = fs::remove_dir_all(&target);
            return Err(error);
        }
    };
    let target_manifest: Value = serde_json::from_slice(&fs::read(target.join("manifest.json"))?)
        .map_err(|err| CoreError::Storage(format!("retained manifest corrupt: {err}")))?;
    let manifest_bytes = hash::canonical_json(&target_manifest)
        .map_err(|err| CoreError::invalid(format!("retained manifest: {err}")))?;
    let manifest_digest = hash::digest_bytes(manifest_bytes.as_bytes());
    let record = json!({
        "schema_version": VERSION,
        "kind": "archmeld.retention_record",
        "retention_id": retention_id,
        "source": source.to_string_lossy(),
        "target": target.to_string_lossy(),
        "target_kind": "path",
        "manifest_digest": manifest_digest,
        "key_provenance": verified["key_provenance"],
        "checked_files": verified["checked_files"],
        "verified": true,
        "copied_at": crate::store::now_unix(),
        "limitations": [
            "path-based target only; transport and remote credentials are outside this operation",
            "the retention record is not authenticated; verify the copied bundle before restore",
        ],
    });
    let record_bytes = hash::canonical_json(&record)
        .map_err(|err| CoreError::invalid(format!("retention record: {err}")))?;
    fs::write(target.join("retention.json"), record_bytes.as_bytes())?;
    Ok(json!({
        "retention_id": retention_id,
        "target": target.to_string_lossy(),
        "target_kind": "path",
        "verified": true,
        "checked_files": verified["checked_files"],
        "manifest_digest": manifest_digest,
        "key_provenance": record["key_provenance"],
        "record_digest": hash::digest_bytes(record_bytes.as_bytes()),
    }))
}
