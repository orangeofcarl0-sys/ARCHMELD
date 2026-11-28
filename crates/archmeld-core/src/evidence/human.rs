// Copyright 2026 ARCHMELD contributors
// SPDX-License-Identifier: Apache-2.0

//! Human-attested structural relations.
//!
//! This importer accepts a person's declaration that one entity reads, writes
//! or owns another. It never turns that declaration into a runtime observation
//! or a resolved call: relations carry `basis=human`, `resolution=declared`,
//! and `conditions` that say so. The attestation file is stored in the CAS so
//! the claim can be re-read later.

use crate::error::{CoreError, Result};
use crate::hash;
use crate::store::{validate, Store};
use serde_json::{json, Value};
use std::collections::BTreeMap;

const SCHEMA_VERSION: &str = "1.0";
const MAX_OBSERVATIONS: usize = 1000;

pub fn import_human_evidence(store: &mut Store, input: &Value) -> Result<Value> {
    if input["schema_version"].as_str() != Some(SCHEMA_VERSION) {
        return Err(CoreError::invalid("human evidence schema_version must be 1.0"));
    }
    let project_id = input["project_id"]
        .as_str()
        .ok_or_else(|| CoreError::invalid("human evidence requires project_id"))?;
    let snapshot_id = input["snapshot_id"]
        .as_str()
        .ok_or_else(|| CoreError::invalid("human evidence requires snapshot_id"))?;
    validate::require_id("project_id", project_id)?;
    validate::require_digest("snapshot_id", snapshot_id)?;
    store.check_snapshot_scope(project_id, snapshot_id)?;
    let payload = store
        .snapshot_payload(project_id, snapshot_id)?
        .ok_or_else(|| CoreError::not_found(format!("snapshot {snapshot_id} is missing")))?;
    let config_digest = payload["config_digest"]
        .as_str()
        .ok_or_else(|| CoreError::Storage("snapshot has no config_digest".into()))?
        .to_string();

    let producer = &input["producer"];
    for field in ["name", "version", "source_ref"] {
        let value = producer[field]
            .as_str()
            .ok_or_else(|| CoreError::invalid(format!("producer.{field} is required")))?;
        validate::require_non_empty(&format!("producer.{field}"), value)?;
    }
    let run_id = producer["run_id"]
        .as_str()
        .ok_or_else(|| CoreError::invalid("producer.run_id is required"))?;
    validate::require_id("producer.run_id", run_id)?;
    let attestor = input["attestor"]
        .as_str()
        .ok_or_else(|| CoreError::invalid("attestor is required"))?;
    validate::require_non_empty("attestor", attestor)?;

    let observations = input["observations"]
        .as_array()
        .ok_or_else(|| CoreError::invalid("observations must be an array"))?;
    if observations.is_empty() {
        return Err(CoreError::invalid("observations must not be empty"));
    }
    if observations.len() > MAX_OBSERVATIONS {
        return Err(CoreError::invalid(format!(
            "resource_exhausted: observations has more than {MAX_OBSERVATIONS} items"
        )));
    }

    let attestation = serde_json::to_vec(input)
        .map_err(|err| CoreError::invalid(format!("attestation encode: {err}")))?;
    let artifact_digest = store.put_blob(&attestation, "application/json")?;
    let evidence_id = format!(
        "evidence.human.{}",
        hash::short_value(
            &json!({
                "project_id": project_id,
                "snapshot_id": snapshot_id,
                "run_id": run_id,
                "artifact_digest": artifact_digest,
            }),
            32,
        )
        .map_err(|err| CoreError::invalid(format!("human evidence identity: {err}")))?
    );

    let mut relations = Vec::new();
    let mut relation_payloads: BTreeMap<String, String> = BTreeMap::new();
    for (index, observation) in observations.iter().enumerate() {
        let kind = match observation["kind"].as_str() {
            Some("read") | Some("reads") => "reads",
            Some("write") | Some("writes") => "writes",
            Some("own") | Some("owns") => "owns",
            Some(other) => {
                return Err(CoreError::invalid(format!(
                    "observation {index}: kind must be read, write or own, got {other:?}"
                )))
            }
            None => {
                return Err(CoreError::invalid(format!(
                    "observation {index}: kind is required"
                )))
            }
        };
        let source_id = observation["source_entity_id"]
            .as_str()
            .ok_or_else(|| CoreError::invalid(format!("observation {index}: source_entity_id is required")))?;
        let target_id = observation["target_entity_id"]
            .as_str()
            .ok_or_else(|| CoreError::invalid(format!("observation {index}: target_entity_id is required")))?;
        let Some(source) = store.get_entity(project_id, snapshot_id, source_id)? else {
            return Err(CoreError::not_found(format!(
                "observation {index}: source entity {source_id} is not in snapshot {snapshot_id}"
            )));
        };
        let Some(target) = store.get_entity(project_id, snapshot_id, target_id)? else {
            return Err(CoreError::not_found(format!(
                "observation {index}: target entity {target_id} is not in snapshot {snapshot_id}"
            )));
        };
        let path = observation["path"].as_str();
        let line = observation["line"].as_u64();
        if let Some(path) = path {
            validate::validate_path("observation.path", path)?;
            if !anchor_matches(&source, path, line) && !anchor_matches(&target, path, line) {
                return Err(CoreError::invalid(format!(
                    "observation {index}: {path} does not match an anchor of the source or target entity"
                )));
            }
        } else if line.is_some() {
            return Err(CoreError::invalid(format!(
                "observation {index}: line requires path"
            )));
        }
        let rationale = observation["rationale"].as_str().unwrap_or("");
        let relation_id = format!(
            "relation.human.{}",
            hash::short_value(
                &json!({
                    "project_id": project_id,
                    "snapshot_id": snapshot_id,
                    "attestor": attestor,
                    "kind": kind,
                    "source_id": source_id,
                    "target_id": target_id,
                    "path": path,
                    "line": line,
                }),
                32,
            )
            .map_err(|err| CoreError::invalid(format!("human evidence identity: {err}")))?
        );
        let mut conditions = vec![
            "human-attested".to_string(),
            format!("attestor:{attestor}"),
            "not-runtime-observation".to_string(),
        ];
        if !rationale.trim().is_empty() {
            conditions.push(format!("rationale:{rationale}"));
        }
        let relation = json!({
            "id": relation_id,
            "project_id": project_id,
            "snapshot_id": snapshot_id,
            "config_digest": config_digest,
            "source_id": source_id,
            "target_id": target_id,
            "kind": kind,
            "basis": "human",
            "resolution": "declared",
            "evidence_ids": [evidence_id],
            "conditions": conditions,
            "call_site": path.map(|path| json!({
                "path": path,
                "line": line,
            })),
            "occurrences": 1,
            "evidence_scope": if path.is_some() { "site" } else { "artifact" },
        });
        let canonical = hash::canonical_json(&relation)
            .map_err(|err| CoreError::invalid(format!("relation encode: {err}")))?;
        if let Some(existing) = relation_payloads.get(&relation_id) {
            if existing != &canonical {
                return Err(CoreError::conflict(format!(
                    "observation {index} duplicates {relation_id} with different content"
                )));
            }
            continue;
        }
        relation_payloads.insert(relation_id, canonical);
        relations.push(relation);
    }

    let evidence = json!({
        "id": evidence_id,
        "project_id": project_id,
        "snapshot_id": snapshot_id,
        "config_digest": config_digest,
        "kind": "human",
        "producer": {
            "name": producer["name"],
            "version": producer["version"],
            "source_ref": producer["source_ref"],
            "run_id": run_id,
        },
        "artifact_digest": artifact_digest,
        "locator": format!("human-attestation:{attestor}"),
        "source_anchor": null,
        "scenario_id": null,
        "runtime_run_id": null,
        "coverage": {
            "state": "partial",
            "scope": format!("human-attested reads/writes/owns for snapshot {snapshot_id}"),
            "expected_count": relations.len(),
            "observed_count": relations.len(),
            "omitted_count": 0,
            "unresolved_count": 0,
            "limitations": [
                "human declaration, not a runtime observation",
                "no executed command, trace or scenario backs these relations",
            ],
        },
        "freshness": "current",
    });

    let batch = json!({
        "schema_version": "1.0",
        "project_id": project_id,
        "snapshot_id": snapshot_id,
        "config_digest": config_digest,
        "run_id": run_id,
        "adapter_id": "archmeld.human-attestation",
        "adapter_version": "1.0",
        "sequence": "0",
        "final": true,
        "coverage": evidence["coverage"],
        "entities": [],
        "evidence": [evidence],
        "relations": relations,
        "diagnostics": [],
    });
    let outcome = store.import_batch(&batch)?;
    Ok(json!({
        "schema_version": "1.0",
        "project_id": project_id,
        "snapshot_id": snapshot_id,
        "attestor": attestor,
        "run_id": run_id,
        "evidence_id": evidence_id,
        "artifact_digest": artifact_digest,
        "relations": relations.len(),
        "observations": observations.len(),
        "basis": "human",
        "resolution": "declared",
        "coverage": "partial",
        "import": {
            "status": format!("{:?}", outcome.status),
            "published": outcome.published,
            "relations": outcome.relations,
        },
        "read_with": {
            "review_diff": "--candidate-snapshot <snapshot_id> --base-snapshot <base>",
        },
    }))
}


fn anchor_matches(entity: &Value, path: &str, line: Option<u64>) -> bool {
    entity["anchors"]
        .as_array()
        .into_iter()
        .flatten()
        .any(|anchor| {
            if anchor["path"].as_str() != Some(path) {
                return false;
            }
            let Some(line) = line else {
                return true;
            };
            let start = anchor["line_start"].as_u64();
            let end = anchor["line_end"].as_u64().or(start);
            match (start, end) {
                (Some(start), Some(end)) => start <= line && line <= end,
                _ => false,
            }
        })
}
