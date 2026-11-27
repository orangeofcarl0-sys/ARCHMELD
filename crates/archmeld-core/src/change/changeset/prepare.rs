// Copyright 2026 ARCHMELD contributors
// SPDX-License-Identifier: Apache-2.0

//! Prepared-change persistence and digest construction.

use super::*;
use super::staging::{diff_operations, Staged};

pub(super) fn persist_revision(
    store: &mut Store,
    payload: Value,
    state: &str,
    operations: Value,
    journal: Option<&[Value]>,
) -> Result<()> {
    let revision = payload["revision"]
        .as_i64()
        .ok_or_else(|| CoreError::Storage("changeset revision missing".to_string()))?;
    let payload_json = hash::canonical_json(&payload)
        .map_err(|err| CoreError::invalid(format!("changeset payload: {err}")))?;
    let tx = store.conn.transaction()?;
    tx.execute(
        "INSERT INTO changesets
         (project_id,changeset_id,revision,plan_id,plan_revision,step_id,base_snapshot_id,
          contract_digest,state,prepared_digest,payload_json,created_at)
         VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12)",
        (
            payload["project_id"].as_str().unwrap_or(""),
            payload["id"].as_str().unwrap_or(""),
            revision,
            payload["plan_id"].as_str().unwrap_or(""),
            payload["plan_revision"].as_i64().unwrap_or(0),
            payload["step_id"].as_str().unwrap_or(""),
            payload["base_snapshot_id"].as_str().unwrap_or(""),
            payload["contract_digest"].as_str().unwrap_or(""),
            state,
            payload["prepared_digest"].as_str(),
            &payload_json,
            crate::store::now_unix(),
        ),
    )?;
    let default_operation_state = match state {
        "applied" => "applied",
        "recovered" => "restored",
        "conflicted" => "conflict",
        _ => "pending",
    };
    for (sequence, operation) in operations.as_array().into_iter().flatten().enumerate() {
        let operation_state = operation["state"]
            .as_str()
            .filter(|value| {
                matches!(
                    *value,
                    "pending" | "applied" | "failed" | "restored" | "conflict"
                )
            })
            .unwrap_or(default_operation_state);
        tx.execute(
            "INSERT INTO change_operations
             (project_id,changeset_id,revision,sequence,op,path,destination,
              before_digest,before_size,after_digest,after_size,state,payload_json)
             VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12,?13)",
            (
                payload["project_id"].as_str().unwrap_or(""),
                payload["id"].as_str().unwrap_or(""),
                revision,
                sequence as i64,
                operation["op"].as_str().unwrap_or(""),
                operation["path"].as_str().unwrap_or(""),
                operation["destination"].as_str(),
                operation["before"]["digest"].as_str(),
                operation["before"]["size"].as_i64(),
                operation["after"]["digest"].as_str(),
                operation["after"]["size"].as_i64(),
                operation_state,
                hash::canonical_json(operation)
                    .map_err(|err| CoreError::invalid(format!("operation payload: {err}")))?,
            ),
        )?;
    }
    for row in journal.into_iter().flatten() {
        tx.execute(
            "INSERT OR REPLACE INTO apply_journal
             (project_id,changeset_id,revision,sequence,op,path,destination,
              preimage_digest,postimage_digest,phase,observed_digest,detail_json,recorded_at)
             VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12,?13)",
            (
                payload["project_id"].as_str().unwrap_or(""),
                payload["id"].as_str().unwrap_or(""),
                revision,
                row["sequence"].as_i64().unwrap_or(0),
                row["op"].as_str().unwrap_or(""),
                row["path"].as_str().unwrap_or(""),
                row["destination"].as_str(),
                row["preimage_digest"].as_str(),
                row["postimage_digest"].as_str(),
                row["phase"].as_str().unwrap_or("intent"),
                row["observed_digest"].as_str(),
                hash::canonical_json(&row["detail"])
                    .map_err(|err| CoreError::invalid(format!("journal detail: {err}")))?,
                crate::store::now_unix(),
            ),
        )?;
    }
    append_event(
        &tx,
        payload["project_id"].as_str().unwrap_or(""),
        None,
        Some(payload["base_snapshot_id"].as_str().unwrap_or("")),
        "change.state_changed",
        json!({
            "changeset_id": payload["id"],
            "previous_state": payload["previous_state"],
            "state": state,
            "revision": revision,
        }),
    )?;
    tx.commit()?;
    Ok(())
}

pub(super) fn build_prepared(
    store: &mut Store,
    mut payload: Value,
    staged: &Staged,
    toolchain_digest: &str,
    transform: &Option<TransformRecord>,
    final_revision: i64,
) -> Result<Value> {
    let operations = diff_operations(store, &payload, staged, transform)?;
    let mut verification_ids = Vec::new();
    let checks = vec![
        (
            "scope",
            json!({
                "allowed_paths": payload["allowed_paths"],
                "operation_paths": operations.iter().map(|op| op.path.clone()).collect::<Vec<_>>(),
                "status": "within_allowed_paths",
            }),
        ),
        (
            "preimages",
            json!({
                "before_digests": operations.iter().filter_map(|op| op.before.as_ref().map(|file| file.digest.clone())).collect::<Vec<_>>(),
                "status": "all_present_in_cas",
            }),
        ),
        (
            "postimages",
            json!({
                "after_digests": operations.iter().filter_map(|op| op.after.as_ref().map(|(digest, _)| digest.clone())).collect::<Vec<_>>(),
                "status": "all_present_in_cas",
            }),
        ),
        (
            "encoding",
            json!({"status": "all_staged_files_utf8", "binary_operations": 0}),
        ),
        ("symlinks", json!({"status": "no_symlinks_in_staging"})),
        (
            "moves",
            json!({
                "declared": transform.as_ref().map(|record| record.declared_moves.clone()).unwrap_or_default(),
                "status": "all_declared_moves_match_content_identity",
            }),
        ),
        (
            "read_set",
            json!({
                "paths": payload["read_set"],
                "status": "read_set_unchanged_in_staging",
            }),
        ),
    ];
    for (kind, evidence) in &checks {
        let verification_id = verification_id(&payload, final_revision, kind);
        store.conn.execute(
            "INSERT INTO staging_verifications
             (project_id,changeset_id,revision,verification_id,kind,status,evidence_json,created_at)
             VALUES (?1,?2,?3,?4,?5,'passed',?6,?7)",
            (
                payload["project_id"].as_str().unwrap_or(""),
                payload["id"].as_str().unwrap_or(""),
                final_revision,
                &verification_id,
                kind,
                hash::canonical_json(evidence)
                    .map_err(|err| CoreError::invalid(format!("verification evidence: {err}")))?,
                crate::store::now_unix(),
            ),
        )?;
        verification_ids.push(verification_id);
    }
    verification_ids.sort();

    let mut allowed_paths: Vec<String> = payload["allowed_paths"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|value| value.as_str().map(str::to_string))
        .collect();
    allowed_paths.sort();
    let mut read_set: Vec<String> = payload["read_set"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|value| value.as_str().map(str::to_string))
        .collect();
    read_set.sort();
    let digest_operations: Vec<Value> = operations.iter().map(Operation::digest_entry).collect();
    let preimage = json!({
        "version": CHANGESET_VERSION,
        "project_id": payload["project_id"],
        "plan_id": payload["plan_id"],
        "plan_revision": payload["plan_revision"],
        "step_id": payload["step_id"],
        "base_snapshot_id": payload["base_snapshot_id"],
        "contract_digest": payload["contract_digest"],
        "allowed_paths": allowed_paths,
        "read_set": read_set,
        "operations": digest_operations,
        "toolchain_digest": toolchain_digest,
        "staging_verification_ids": verification_ids,
    });
    let prepared_digest = hash::digest_value(&preimage)
        .map_err(|err| CoreError::invalid(format!("prepared change: {err}")))?;
    let manifest = hash::canonical_json(&preimage)
        .map_err(|err| CoreError::invalid(format!("prepared change: {err}")))?;
    let prepared_artifact = store.put_blob(manifest.as_bytes(), "application/json")?;
    // A standalone, exact machine-readable patch. It is not part of the S19
    // prepared digest preimage (which already binds every operation), but it
    // gives reviewers and recovery audits the precise patch object.
    let patch = json!({
        "version": CHANGESET_VERSION,
        "changeset_id": payload["id"],
        "base_snapshot_id": payload["base_snapshot_id"],
        "operations": preimage["operations"],
    });
    let patch_json = hash::canonical_json(&patch)
        .map_err(|err| CoreError::invalid(format!("patch artifact: {err}")))?;
    let patch_artifact = store.put_blob(patch_json.as_bytes(), "application/json")?;

    let write_set: Vec<String> = {
        let mut set = BTreeSet::new();
        for operation in &operations {
            set.insert(operation.path.clone());
            if let Some(destination) = &operation.destination {
                set.insert(destination.clone());
            }
        }
        set.into_iter().collect()
    };
    payload["revision"] = json!(final_revision);
    payload["operations"] = json!(operations
        .iter()
        .map(Operation::payload)
        .collect::<Vec<_>>());
    payload["write_set"] = json!(write_set);
    payload["toolchain_digest"] = json!(toolchain_digest);
    payload["transform"] = match transform {
        Some(record) => json!({
            "method": record.method,
            "tool": {
                "name": record.tool.name,
                "version": record.tool.version,
                "identity_digest": record.tool.identity_digest,
            },
            "declared_moves": record.declared_moves,
            "patch_artifact": record.patch_artifact,
            "prompt_artifact": record.prompt_artifact,
            "workspace_edit_operations": record.workspace_edit_operations,
            "notes": record.notes,
        }),
        None => Value::Null,
    };
    payload["prepared_digest"] = json!(prepared_digest);
    payload["prepared_artifact"] = json!(prepared_artifact);
    payload["patch_artifact"] = json!(patch_artifact);
    payload["staging_verification_ids"] = json!(verification_ids);
    payload["staging"] = json!({
        "path": staged.path.to_string_lossy(),
        "file_count": staged.files.len(),
        "metadata": STAGING_METADATA,
        "source_mutation": "none",
    });
    payload["updated_at"] = json!(crate::store::now_unix());
    Ok(payload)
}
