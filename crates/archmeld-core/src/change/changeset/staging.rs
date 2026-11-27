// Copyright 2026 ARCHMELD contributors
// SPDX-License-Identifier: Apache-2.0

//! Staging scan, diff, seed and walk helpers.

use super::*;

pub(super) struct Staged {
    pub(super) path: PathBuf,
    pub(super) files: BTreeMap<String, StagedFile>,
}

pub(super) fn scan_staging(
    store: &mut Store,
    payload: &Value,
    transform: &Option<TransformRecord>,
) -> Result<Staged> {
    let project_id = payload["project_id"].as_str().unwrap_or("");
    let changeset_id = payload["id"].as_str().unwrap_or("");
    let base_snapshot_id = payload["base_snapshot_id"].as_str().unwrap_or("");
    let staging = staging_root(store, project_id, changeset_id)?;
    ensure_staging_outside_workspace(store, project_id, &staging)?;
    seed_staging(store, payload, &staging)?;
    let options: SnapshotCaptureOptions =
        serde_json::from_value(payload["snapshot_options"].clone()).unwrap_or_default();
    let files = walk_staging(&staging, options.max_file_bytes)?;
    let base = snapshot_files(store, project_id, base_snapshot_id)?;
    let allowed: Vec<String> = payload["allowed_paths"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|value| value.as_str().map(str::to_string))
        .collect();
    if allowed.is_empty() {
        return Err(CoreError::invalid("changeset has no allowed_paths"));
    }
    let read_set: Vec<String> = payload["read_set"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|value| value.as_str().map(str::to_string))
        .collect();

    // Every staged path must be inside the authorized write scope, or an
    // unchanged read-set file that was materialized only as context.
    for (path, file) in &files {
        if in_scope(path, &allowed) {
            continue;
        }
        if read_set.iter().any(|item| item == path) {
            let Some(base_file) = base.get(path) else {
                return Err(CoreError::invalid(format!(
                    "read_set path {path} has no base snapshot entry"
                )));
            };
            if base_file.digest != file.digest {
                return Err(CoreError::invalid(format!(
                    "out_of_scope: read_set file {path} was modified in staging"
                )));
            }
            continue;
        }
        return Err(CoreError::invalid(format!(
            "out_of_scope: staged path {path} is not within allowed_paths"
        )));
    }

    // Read-set context must still be present and unchanged; deleting it would
    // silently change the preparation context.
    for path in &read_set {
        let Some(base_file) = base.get(path) else {
            return Err(CoreError::not_found(format!(
                "read_set path {path} is missing from base snapshot"
            )));
        };
        if in_scope(path, &allowed) {
            continue;
        }
        match files.get(path) {
            Some(staged) if staged.digest == base_file.digest => {}
            Some(_) => {
                return Err(CoreError::invalid(format!(
                    "out_of_scope: read_set file {path} was modified in staging"
                )))
            }
            None => {
                return Err(CoreError::invalid(format!(
                    "read_set file {path} was deleted from staging"
                )))
            }
        }
    }
    let _ = transform;
    Ok(Staged {
        path: staging,
        files,
    })
}

pub(super) fn diff_operations(
    store: &mut Store,
    payload: &Value,
    staged: &Staged,
    transform: &Option<TransformRecord>,
) -> Result<Vec<Operation>> {
    let project_id = payload["project_id"].as_str().unwrap_or("");
    let base_snapshot_id = payload["base_snapshot_id"].as_str().unwrap_or("");
    let base = snapshot_files(store, project_id, base_snapshot_id)?;
    let allowed: Vec<String> = payload["allowed_paths"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|value| value.as_str().map(str::to_string))
        .collect();
    let declared = transform
        .as_ref()
        .map(|record| record.declared_moves.clone())
        .unwrap_or_default();
    // Persist every staged byte sequence as a postimage candidate. A digest
    // alone is not evidence; the CAS must actually hold the bytes.
    for (path, file) in &staged.files {
        let bytes = fs::read(staged.path.join(path))?;
        let digest = store.put_blob(&bytes, "text/plain")?;
        if digest != file.digest {
            return Err(CoreError::Storage(format!(
                "staged file {path} changed while being recorded"
            )));
        }
    }
    let mut move_sources = BTreeSet::new();
    let mut move_targets = BTreeSet::new();
    let mut operations: Vec<Operation> = Vec::new();
    for movement in &declared {
        validate::validate_path("declared_moves[].from", &movement.from)?;
        validate::validate_path("declared_moves[].to", &movement.to)?;
        if movement.from == movement.to {
            return Err(CoreError::invalid("declared move has identical from/to"));
        }
        if !in_scope(&movement.from, &allowed) || !in_scope(&movement.to, &allowed) {
            return Err(CoreError::invalid(format!(
                "declared move {} -> {} is outside allowed_paths",
                movement.from, movement.to
            )));
        }
        if !move_sources.insert(movement.from.clone()) || !move_targets.insert(movement.to.clone())
        {
            return Err(CoreError::invalid(
                "a declared move reuses a source or target",
            ));
        }
        let Some(before) = base.get(&movement.from).cloned() else {
            return Err(CoreError::not_found(format!(
                "declared move source {} is not in the base snapshot",
                movement.from
            )));
        };
        if movement.from != movement.to && base.contains_key(&movement.to) {
            return Err(CoreError::conflict(format!(
                "declared move target {} already exists in the base snapshot",
                movement.to
            )));
        }
        if staged.files.contains_key(&movement.from) {
            return Err(CoreError::invalid(format!(
                "declared move {} -> {} has not been performed in staging",
                movement.from, movement.to
            )));
        }
        let Some(after) = staged.files.get(&movement.to) else {
            return Err(CoreError::invalid(format!(
                "declared move target {} is missing from staging",
                movement.to
            )));
        };
        if after.digest != before.digest || after.size != before.size {
            return Err(CoreError::invalid(format!(
                "v1 move {} -> {} must preserve content; split it into move then update",
                movement.from, movement.to
            )));
        }
        let case_only = movement.from != movement.to
            && movement.from.to_lowercase() == movement.to.to_lowercase();
        operations.push(Operation {
            op: "move".to_string(),
            path: movement.from.clone(),
            destination: Some(movement.to.clone()),
            before: Some(before),
            after: Some((after.digest.clone(), after.size)),
            case_only,
        });
    }

    // Staged files in write scope that are not move targets.
    for (path, staged_file) in &staged.files {
        if move_targets.contains(path) || !in_scope(path, &allowed) {
            continue;
        }
        if move_sources.contains(path) {
            continue;
        }
        match base.get(path) {
            Some(before)
                if before.digest == staged_file.digest && before.size == staged_file.size =>
            {
                // unchanged
            }
            Some(before) => operations.push(Operation {
                op: "update".to_string(),
                path: path.clone(),
                destination: None,
                before: Some(before.clone()),
                after: Some((staged_file.digest.clone(), staged_file.size)),
                case_only: false,
            }),
            None => operations.push(Operation {
                op: "create".to_string(),
                path: path.clone(),
                destination: None,
                before: None,
                after: Some((staged_file.digest.clone(), staged_file.size)),
                case_only: false,
            }),
        }
    }
    // Base files in write scope that are missing from staging (and are not
    // move sources).
    for (path, before) in &base {
        if !in_scope(path, &allowed) || move_sources.contains(path) || move_targets.contains(path) {
            continue;
        }
        if !staged.files.contains_key(path) {
            operations.push(Operation {
                op: "delete".to_string(),
                path: path.clone(),
                destination: None,
                before: Some(before.clone()),
                after: None,
                case_only: false,
            });
        }
    }
    for operation in &operations {
        if let Some((digest, size)) = &operation.after {
            let bytes = store.read_blob(digest)?.ok_or_else(|| {
                CoreError::Storage(format!(
                    "staged content {digest} is not in the object store"
                ))
            })?;
            if bytes.len() as u64 != *size {
                return Err(CoreError::Storage(format!(
                    "staged content {digest} size mismatch"
                )));
            }
        }
    }
    operations.sort_by(|left, right| {
        rank(&left.op)
            .cmp(&rank(&right.op))
            .then(left.path.as_bytes().cmp(right.path.as_bytes()))
            .then(left.destination.cmp(&right.destination))
    });
    Ok(operations)
}

fn rank(op: &str) -> u8 {
    match op {
        "move" => 0,
        "create" => 1,
        "update" => 2,
        "delete" => 3,
        _ => 4,
    }
}

fn seed_staging(store: &mut Store, payload: &Value, staging: &Path) -> Result<()> {
    ensure_staging_segments(store, staging)?;
    let project_id = payload["project_id"].as_str().unwrap_or("");
    let changeset_id = payload["id"].as_str().unwrap_or("");
    let base_snapshot_id = payload["base_snapshot_id"].as_str().unwrap_or("");
    let metadata_path = staging.join(STAGING_METADATA);
    let marker = staging.join(SEED_MARKER);
    if !metadata_path.exists() && marker.exists() {
        // A previous seed was interrupted. Preserve it for audit and start a
        // fresh staging directory; never delete it in place.
        let archived = staging.with_file_name(format!(
            "{changeset_id}.incomplete.{}",
            crate::store::now_unix()
        ));
        fs::rename(staging, &archived)?;
    }
    if metadata_path.exists() {
        let text = fs::read_to_string(&metadata_path)
            .map_err(|err| CoreError::Storage(format!("staging metadata unreadable: {err}")))?;
        let metadata: StagingMetadata = serde_json::from_str(&text)
            .map_err(|err| CoreError::Storage(format!("staging metadata corrupt: {err}")))?;
        if metadata.project_id != project_id
            || metadata.changeset_id != changeset_id
            || metadata.base_snapshot_id != base_snapshot_id
        {
            return Err(CoreError::conflict(
                "staging directory belongs to a different ChangeSet or base snapshot",
            ));
        }
        let _ = fs::remove_file(staging.join(SEED_MARKER));
        return Ok(());
    }
    if staging.exists() {
        let mut entries = fs::read_dir(staging)?;
        if entries.next().is_some() {
            return Err(CoreError::conflict(
                "staging directory exists without ARCHMELD metadata; refusing to overwrite it",
            ));
        }
    } else {
        fs::create_dir_all(staging)?;
    }
    fs::write(
        &marker,
        b"seeding
",
    )?;
    let base = snapshot_files(store, project_id, base_snapshot_id)?;
    let allowed: Vec<String> = payload["allowed_paths"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|value| value.as_str().map(str::to_string))
        .collect();
    let read_set: Vec<String> = payload["read_set"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|value| value.as_str().map(str::to_string))
        .collect();
    let mut seeded = Vec::new();
    for (path, file) in &base {
        let wanted = in_scope(path, &allowed) || read_set.iter().any(|item| item == path);
        if !wanted {
            continue;
        }
        let bytes = store.read_blob(&file.digest)?.ok_or_else(|| {
            CoreError::Storage(format!(
                "base snapshot blob {} for {path} is missing",
                file.digest
            ))
        })?;
        let target = staging.join(path);
        if let Some(parent) = target.parent() {
            fs::create_dir_all(parent)?;
        }
        fs::write(&target, &bytes)?;
        seeded.push(StagingMetadataFile {
            path: path.clone(),
            digest: file.digest.clone(),
            size: file.size,
        });
    }
    let metadata = StagingMetadata {
        version: CHANGESET_VERSION.to_string(),
        project_id: project_id.to_string(),
        changeset_id: changeset_id.to_string(),
        base_snapshot_id: base_snapshot_id.to_string(),
        files: seeded,
        created_at: crate::store::now_unix(),
    };
    let text = hash::canonical_json(
        &serde_json::to_value(&metadata)
            .map_err(|err| CoreError::Storage(format!("staging metadata encode: {err}")))?,
    )
    .map_err(|err| CoreError::invalid(format!("staging metadata: {err}")))?;
    fs::write(&metadata_path, text)?;
    let _ = fs::remove_file(&marker);
    Ok(())
}

fn walk_staging(root: &Path, max_file_bytes: u64) -> Result<BTreeMap<String, StagedFile>> {
    if is_link_like(&fs::symlink_metadata(root)?) {
        return Err(CoreError::invalid(
            "the staging root itself is a link or reparse point",
        ));
    }
    let mut files = BTreeMap::new();
    let mut case_seen: BTreeMap<String, String> = BTreeMap::new();
    walk_dir(root, root, &mut files, &mut case_seen, max_file_bytes)?;
    Ok(files)
}

fn walk_dir(
    root: &Path,
    dir: &Path,
    files: &mut BTreeMap<String, StagedFile>,
    case_seen: &mut BTreeMap<String, String>,
    max_file_bytes: u64,
) -> Result<()> {
    for entry in fs::read_dir(dir)? {
        let entry = entry?;
        let path = entry.path();
        let relative = path
            .strip_prefix(root)
            .map_err(|_| CoreError::Storage("staging path escaped its root".to_string()))?;
        let rel_text = relative
            .to_string_lossy()
            .replace(std::path::MAIN_SEPARATOR, "/");
        if rel_text == STAGING_METADATA || rel_text == SEED_MARKER {
            continue;
        }
        if relative
            .file_name()
            .and_then(|name| name.to_str())
            .map(|name| name.starts_with(".archmeld."))
            .unwrap_or(false)
        {
            return Err(CoreError::invalid(format!(
                "reserved staging path {rel_text} is not a reviewable source file"
            )));
        }
        let metadata = fs::symlink_metadata(&path)?;
        let file_type = metadata.file_type();
        if is_link_like(&metadata) {
            return Err(CoreError::invalid(format!(
                "symlink_in_staging: {rel_text} is a symlink or reparse point"
            )));
        }
        if file_type.is_dir() {
            walk_dir(root, &path, files, case_seen, max_file_bytes)?;
            continue;
        }
        if !file_type.is_file() {
            return Err(CoreError::invalid(format!(
                "unsupported file type in staging: {rel_text}"
            )));
        }
        validate::validate_path("staging.path", &rel_text)?;
        if metadata.len() > max_file_bytes {
            return Err(CoreError::invalid(format!(
                "resource_exhausted: staged file {rel_text} exceeds {max_file_bytes} bytes"
            )));
        }
        if files.len() >= MAX_STAGED_FILES {
            return Err(CoreError::invalid(format!(
                "resource_exhausted: staging holds more than {MAX_STAGED_FILES} files"
            )));
        }
        let folded = rel_text.to_lowercase();
        if let Some(existing) = case_seen.insert(folded, rel_text.clone()) {
            return Err(CoreError::invalid(format!(
                "case-insensitive staging collision: {existing} and {rel_text}"
            )));
        }
        let bytes = fs::read(&path)?;
        if std::str::from_utf8(&bytes).is_err() {
            return Err(CoreError::unsupported(format!(
                "binary modifications are not enabled in v1: {rel_text}"
            )));
        }
        files.insert(
            rel_text,
            StagedFile {
                digest: hash::digest_bytes(&bytes),
                size: bytes.len() as u64,
            },
        );
    }
    Ok(())
}

pub(super) fn is_link_like(metadata: &fs::Metadata) -> bool {
    #[cfg(windows)]
    {
        use std::os::windows::fs::MetadataExt;
        const FILE_ATTRIBUTE_REPARSE_POINT: u32 = 0x0400;
        metadata.file_attributes() & FILE_ATTRIBUTE_REPARSE_POINT != 0
    }
    #[cfg(not(windows))]
    {
        metadata.file_type().is_symlink()
    }
}
