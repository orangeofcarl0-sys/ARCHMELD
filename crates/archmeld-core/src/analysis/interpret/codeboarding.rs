// Copyright 2026 ARCHMELD contributors
// SPDX-License-Identifier: Apache-2.0

//! CodeBoarding import and reference verification.

use super::*;
use super::architecture::insert_architecture;

pub struct CodeBoardingRequest<'a> {
    pub project_id: &'a str,
    pub snapshot_id: &'a str,
    pub expected_revision: i64,
    pub idempotency_key: &'a str,
    pub provider_version: &'a str,
    pub run_id: &'a str,
    pub commands: Vec<String>,
    /// Model interpretation provenance. Never carries an API key.
    pub model: Option<ModelProvenance<'a>>,
}

#[derive(Debug, Clone)]
pub struct ModelProvenance<'a> {
    pub provider: &'a str,
    pub model: &'a str,
    /// Host only, for example `opencode.ai`; no credentials or query string.
    pub base_url_host: &'a str,
    pub calls: u64,
    pub input_tokens: Option<u64>,
    pub output_tokens: Option<u64>,
}

impl ModelProvenance<'_> {
    fn to_json(&self) -> Value {
        json!({
            "provider": self.provider,
            "model": self.model,
            "base_url_host": self.base_url_host,
            "calls": self.calls,
            "input_tokens": self.input_tokens,
            "output_tokens": self.output_tokens,
        })
    }
}

/// Read the optional CodeBoarding contract version. The pinned 0.15.3 output
/// has no `analysis_schema_version`, so a missing field is recorded as
/// `unversioned` instead of rejecting the only real artifact. An explicit
/// unknown major version is rejected before it can be cached as an idempotent
/// result.
fn codeboarding_contract(analysis: &Value) -> Result<Value> {
    let (status, declared_version, declared_major) =
        match analysis.get("analysis_schema_version") {
            None | Some(Value::Null) => ("unversioned", Value::Null, Value::Null),
            Some(Value::String(text)) => {
                let text = text.trim();
                let major_text = text.split('.').next().unwrap_or("");
                let major: u64 = major_text.parse().map_err(|_| {
                    CoreError::invalid(
                        "CodeBoarding analysis_schema_version must start with a numeric major version",
                    )
                })?;
                if major > 1 {
                    return Err(CoreError::invalid(format!(
                        "unsupported CodeBoarding analysis_schema_version {text}; supported major version is 1"
                    )));
                }
                ("declared", json!(text), json!(major))
            }
            Some(_) => {
                return Err(CoreError::invalid(
                    "CodeBoarding analysis_schema_version must be a string",
                ))
            }
        };
    Ok(json!({
        "declared_version": declared_version,
        "declared_major": declared_major,
        "status": status,
        "observed_tree_spec_version": analysis["metadata"]["tree_spec"]["version"].as_u64(),
    }))
}

/// Import a real CodeBoarding `analysis.json`. Components, dependencies and
/// source references are mapped to candidates; missing references are recorded
/// as unverified and no ledger entity/relation is created.
pub fn import_codeboarding(
    store: &mut Store,
    request: &CodeBoardingRequest<'_>,
    analysis: &Value,
) -> Result<Value> {
    validate::require_id("project_id", request.project_id)?;
    validate::require_id("idempotency_key", request.idempotency_key)?;
    validate::require_id("run_id", request.run_id)?;
    validate::require_non_empty("provider_version", request.provider_version)?;
    if let Some(model) = &request.model {
        validate::require_non_empty("model.provider", model.provider)?;
        validate::require_non_empty("model.model", model.model)?;
        validate::require_non_empty("model.base_url_host", model.base_url_host)?;
        if model.calls == 0 {
            return Err(CoreError::invalid("model.calls must be >= 1"));
        }
        if model.base_url_host.contains('@')
            || model.base_url_host.contains('/')
            || model.base_url_host.contains('?')
        {
            return Err(CoreError::invalid(
                "model.base_url_host must be a host name without credentials or a path",
            ));
        }
    }
    snapshot_config(store, request.project_id, request.snapshot_id)?;
    let project_revision = project_revision(store, request.project_id)?;
    if request.expected_revision != project_revision {
        return Err(CoreError::conflict(format!(
            "stale_snapshot: expected_revision {} does not match project revision {project_revision}",
            request.expected_revision
        )));
    }
    if analysis["components"].as_array().is_none() {
        return Err(CoreError::invalid(
            "CodeBoarding analysis is missing the components array",
        ));
    }
    let analysis_contract = codeboarding_contract(analysis)?;
    if request.commands.len() > 100
        || request.commands.iter().any(|command| command.len() > 10_000)
    {
        return Err(CoreError::invalid(
            "resource_exhausted: provider command list exceeds limits",
        ));
    }
    let envelope = json!({"analysis": analysis, "commands": request.commands});
    let artifact_bytes = serde_json::to_vec(&envelope)
        .map_err(|err| CoreError::Storage(format!("provider envelope encode: {err}")))?;
    let artifact_digest = store.put_blob(&artifact_bytes, "application/json")?;
    let model_json = request.model.as_ref().map(ModelProvenance::to_json);
    let digest = hash::digest_value(&json!({
        "project_id": request.project_id,
        "snapshot_id": request.snapshot_id,
        "analysis": analysis,
        "commands": request.commands,
        "expected_revision": request.expected_revision,
        "model": model_json,
    }))
    .map_err(|err| CoreError::invalid(format!("provider output outside hash domain: {err}")))?;
    if let Some((recorded, _, result)) =
        store.idempotency_get(request.project_id, "importCodeBoarding", request.idempotency_key)?
    {
        if recorded != digest {
            return Err(CoreError::conflict(
                "scope_conflict: idempotency_key was used with different provider output",
            ));
        }
        return serde_json::from_str(&result)
            .map_err(|err| CoreError::Storage(format!("stored architecture corrupt: {err}")));
    }

    let mut components: Vec<ArchitectureComponent> = Vec::new();
    let mut dependencies: Vec<ArchitectureDependency> = Vec::new();
    let mut references: Vec<Value> = Vec::new();
    let mut component_ids: BTreeSet<String> = BTreeSet::new();
    collect_codeboarding_components(
        store,
        request,
        analysis["components"].as_array().unwrap(),
        &mut components,
        &mut dependencies,
        &mut references,
        &mut component_ids,
    )?;
    if components.is_empty() {
        return Err(CoreError::invalid(
            "CodeBoarding analysis produced no candidate components",
        ));
    }
    if components.len() > MAX_COMPONENTS || dependencies.len() > MAX_DEPENDENCIES {
        return Err(CoreError::invalid(
            "resource_exhausted: provider output exceeds component/dependency limits",
        ));
    }
    if let Some(relations) = analysis["components_relations"].as_array() {
        push_codeboarding_relations(relations, &mut dependencies);
    }
    let architecture_id = format!("arch.codeboarding.{}", &digest[7..23]);
    let mut architecture = Architecture {
        schema_version: "1.0".to_string(),
        id: architecture_id,
        project_id: request.project_id.to_string(),
        snapshot_id: request.snapshot_id.to_string(),
        revision: 1,
        layer: Layer::AsIs,
        status: ArchitectureStatus::Candidate,
        components,
        dependencies,
        rules: Vec::new(),
        claim_ids: Vec::new(),
    };
    validate_architecture(store, request.project_id, &architecture)?;
    let mut verification = verify_architecture_references_with_references(
        store,
        &architecture,
        references,
    );
    verification["commands_ignored"] = json!(request.commands.len());
    verification["commands_executed"] = json!(0);
    verification["analysis_contract"] = analysis_contract;
    let mut producer = json!({
        "name": "CodeBoarding",
        "version": request.provider_version,
        "source_ref": "analysis.json (isolated CLI output)",
        "run_id": request.run_id,
    });
    if let Some(model) = &request.model {
        producer["model"] = model.to_json();
        producer["source_ref"] =
            json!("analysis.json (model interpretation; provider and model recorded)");
    }
    insert_architecture(
        store,
        &architecture,
        &producer,
        Some(&artifact_digest),
        request.commands.len(),
        &verification,
        "importCodeBoarding",
        request.idempotency_key,
        &digest,
    )?;
    architecture.revision = 1;
    Ok(serde_json::to_value(&architecture)
        .map_err(|err| CoreError::Storage(format!("architecture encode: {err}")))?)
}

fn collect_codeboarding_components(
    store: &Store,
    request: &CodeBoardingRequest<'_>,
    raw_components: &[Value],
    components: &mut Vec<ArchitectureComponent>,
    dependencies: &mut Vec<ArchitectureDependency>,
    references: &mut Vec<Value>,
    component_ids: &mut BTreeSet<String>,
) -> Result<()> {
    for (index, raw) in raw_components.iter().enumerate() {
        let component_id = raw["component_id"]
            .as_str()
            .filter(|value| !value.is_empty())
            .map(str::to_string)
            .unwrap_or_else(|| format!("generated.{index}"));
        let name = raw["name"]
            .as_str()
            .filter(|value| !value.is_empty())
            .ok_or_else(|| CoreError::invalid("CodeBoarding component has no name"))?;
        let description = raw["description"]
            .as_str()
            .filter(|value| !value.is_empty())
            .unwrap_or(name);
        let candidate_id = format!("cb.{component_id}");
        validate::require_id("architecture.components[].id", &candidate_id)?;
        component_ids.insert(candidate_id.clone());
        let mut member_ids = BTreeSet::new();
        if let Some(entities) = raw["key_entities"].as_array() {
            for entity in entities {
                let Some(file) = entity["reference_file"].as_str() else {
                    references.push(json!({
                        "component_id": candidate_id,
                        "status": "unverified",
                        "reason": "missing reference_file",
                    }));
                    continue;
                };
                let line_start = entity["reference_start_line"].as_u64().unwrap_or(1);
                let line_end = entity["reference_end_line"]
                    .as_u64()
                    .unwrap_or(line_start);
                let qualified = entity["qualified_name"].as_str().unwrap_or("");
                match verify_source_reference(
                    store,
                    request.project_id,
                    request.snapshot_id,
                    &candidate_id,
                    file,
                    line_start,
                    line_end,
                    qualified,
                ) {
                    Ok((status, entity_id)) => {
                        if let Some(entity_id) = entity_id {
                            member_ids.insert(entity_id);
                        }
                        references.push(json!({
                            "component_id": candidate_id,
                            "path": file,
                            "line_start": line_start,
                            "line_end": line_end,
                            "qualified_name": qualified,
                            "status": status,
                        }));
                    }
                    Err(error) => references.push(json!({
                        "component_id": candidate_id,
                        "path": file,
                        "line_start": line_start,
                        "line_end": line_end,
                        "qualified_name": qualified,
                        "status": "unverified",
                        "reason": error.to_string(),
                    })),
                }
            }
        }
        if references.len() > MAX_SOURCE_REFERENCES {
            return Err(CoreError::invalid(
                "resource_exhausted: too many provider source references",
            ));
        }
        components.push(ArchitectureComponent {
            id: candidate_id,
            name: name.to_string(),
            purpose: description.to_string(),
            responsibilities: vec![description.to_string()],
            non_responsibilities: Vec::new(),
            member_entity_ids: member_ids.into_iter().collect(),
            owned_state: Vec::new(),
            owned_resources: Vec::new(),
            runtime_placement: Vec::new(),
            evidence_ids: Vec::new(),
            open_questions: vec![
                "CodeBoarding proposed this boundary; verify ownership and runtime coupling".to_string(),
            ],
        });

        for relation in raw["components_relations"]
            .as_array()
            .map(Vec::as_slice)
            .unwrap_or(&[])
        {
            let source_raw = relation["src_id"].as_str().unwrap_or("");
            let target_raw = relation["dst_id"].as_str().unwrap_or("");
            if source_raw.is_empty() || target_raw.is_empty() {
                continue;
            }
            let reason = relation["relation"]
                .as_str()
                .filter(|value| !value.is_empty())
                .unwrap_or("CodeBoarding candidate relation");
            dependencies.push(ArchitectureDependency {
                source_id: format!("cb.{source_raw}"),
                target_id: format!("cb.{target_raw}"),
                reason: reason.to_string(),
                evidence_ids: Vec::new(),
                status: DependencyStatus::Candidate,
            });
        }
        if let Some(nested) = raw["components"].as_array() {
            collect_codeboarding_components(
                store,
                request,
                nested,
                components,
                dependencies,
                references,
                component_ids,
            )?;
        }
    }
    Ok(())
}

fn push_codeboarding_relations(
    relations: &[Value],
    dependencies: &mut Vec<ArchitectureDependency>,
) {
    for relation in relations {
        let source_raw = relation["src_id"].as_str().unwrap_or("");
        let target_raw = relation["dst_id"].as_str().unwrap_or("");
        if source_raw.is_empty() || target_raw.is_empty() {
            continue;
        }
        let reason = relation["relation"]
            .as_str()
            .filter(|value| !value.is_empty())
            .unwrap_or("CodeBoarding candidate relation");
        dependencies.push(ArchitectureDependency {
            source_id: format!("cb.{source_raw}"),
            target_id: format!("cb.{target_raw}"),
            reason: reason.to_string(),
            evidence_ids: Vec::new(),
            status: DependencyStatus::Candidate,
        });
    }
}

#[allow(clippy::too_many_arguments)]
fn verify_source_reference(
    store: &Store,
    project_id: &str,
    snapshot_id: &str,
    _component_id: &str,
    path: &str,
    line_start: u64,
    line_end: u64,
    qualified_name: &str,
) -> Result<(String, Option<String>)> {
    validate::validate_path("provider.reference_file", path)?;
    let blob: Option<String> = store
        .conn
        .query_row(
            "SELECT blob_digest FROM files WHERE project_id=?1 AND snapshot_id=?2 AND path=?3",
            (project_id, snapshot_id, path),
            |row| row.get(0),
        )
        .ok();
    let Some(blob) = blob else {
        return Ok(("unverified".to_string(), None));
    };
    let bytes = store
        .read_blob(&blob)?
        .ok_or_else(|| CoreError::Storage(format!("snapshot blob {blob} is missing")))?;
    let text = String::from_utf8(bytes)
        .map_err(|_| CoreError::unsupported(format!("{path} is not valid UTF-8")))?;
    let lines = text.lines().count().max(1) as u64;
    if line_start == 0 || line_end < line_start || line_end > lines {
        return Ok(("unverified".to_string(), None));
    }
    let entity_id: Option<String> = store
        .conn
        .query_row(
            "SELECT entity_id FROM entities
             WHERE project_id=?1 AND snapshot_id=?2 AND qualified_name=?3
             ORDER BY entity_id LIMIT 1",
            (project_id, snapshot_id, qualified_name),
            |row| row.get(0),
        )
        .ok();
    Ok((
        if entity_id.is_some() {
            "resolved_with_entity".to_string()
        } else {
            "resolved_source_only".to_string()
        },
        entity_id,
    ))
}

pub(super) fn verify_architecture_references(store: &Store, architecture: &Architecture) -> Value {
    verify_architecture_references_with_references(store, architecture, Vec::new())
}

fn verify_architecture_references_with_references(
    store: &Store,
    architecture: &Architecture,
    source_references: Vec<Value>,
) -> Value {
    let component_ids: BTreeSet<&str> = architecture
        .components
        .iter()
        .map(|component| component.id.as_str())
        .collect();
    let mut missing_members: Vec<Value> = Vec::new();
    let mut matched_members = 0usize;
    for component in &architecture.components {
        for entity_id in &component.member_entity_ids {
            let exists: Option<String> = store
                .conn
                .query_row(
                    "SELECT entity_id FROM entities
                     WHERE project_id=?1 AND snapshot_id=?2 AND entity_id=?3",
                    (&architecture.project_id, &architecture.snapshot_id, entity_id),
                    |row| row.get(0),
                )
                .ok();
            if exists.is_some() {
                matched_members += 1;
            } else {
                missing_members.push(json!({
                    "component_id": component.id,
                    "entity_id": entity_id,
                    "status": "unverified",
                }));
            }
        }
    }
    let mut missing_evidence: Vec<Value> = Vec::new();
    for component in &architecture.components {
        for evidence_id in &component.evidence_ids {
            let exists: Option<String> = store
                .conn
                .query_row(
                    "SELECT evidence_id FROM evidence
                     WHERE project_id=?1 AND snapshot_id=?2 AND evidence_id=?3",
                    (&architecture.project_id, &architecture.snapshot_id, evidence_id),
                    |row| row.get(0),
                )
                .ok();
            if exists.is_none() {
                missing_evidence.push(json!({
                    "component_id": component.id,
                    "evidence_id": evidence_id,
                    "status": "unverified",
                }));
            }
        }
    }
    for dependency in &architecture.dependencies {
        for evidence_id in &dependency.evidence_ids {
            let exists: Option<String> = store
                .conn
                .query_row(
                    "SELECT evidence_id FROM evidence
                     WHERE project_id=?1 AND snapshot_id=?2 AND evidence_id=?3",
                    (&architecture.project_id, &architecture.snapshot_id, evidence_id),
                    |row| row.get(0),
                )
                .ok();
            if exists.is_none() {
                missing_evidence.push(json!({
                    "source_id": dependency.source_id,
                    "target_id": dependency.target_id,
                    "evidence_id": evidence_id,
                    "status": "unverified",
                }));
            }
        }
    }
    let mut unresolved_dependencies: Vec<Value> = Vec::new();
    for dependency in &architecture.dependencies {
        if !component_ids.contains(dependency.source_id.as_str())
            || !component_ids.contains(dependency.target_id.as_str())
        {
            unresolved_dependencies.push(json!({
                "source_id": dependency.source_id,
                "target_id": dependency.target_id,
                "reason": dependency.reason,
                "status": "unverified",
            }));
        }
    }
    let known_claims: BTreeSet<String> = architecture
        .claim_ids
        .iter()
        .filter(|claim_id| {
            latest_claim(store, &architecture.project_id, claim_id)
                .ok()
                .flatten()
                .is_some()
        })
        .cloned()
        .collect();
    let missing_claims: Vec<&String> = architecture
        .claim_ids
        .iter()
        .filter(|claim_id| !known_claims.contains(*claim_id))
        .collect();
    let declared_only_required: Vec<String> = architecture
        .rules
        .iter()
        .filter(|rule| rule.required && rule.checkability == "declared_only")
        .map(|rule| rule.id.clone())
        .collect();
    let unverified_references = source_references
        .iter()
        .filter(|reference| reference["status"] == "unverified")
        .count();
    let resolved_references = source_references
        .iter()
        .filter(|reference| reference["status"] != "unverified")
        .count();
    json!({
        "candidate_only": true,
        "certain_calls_added": 0,
        "components": architecture.components.len(),
        "dependencies": architecture.dependencies.len(),
        "source_references": {
            "total": source_references.len(),
            "resolved": resolved_references,
            "unverified": unverified_references,
            "details": source_references,
        },
        "member_entities": {
            "matched": matched_members,
            "missing": missing_members,
        },
        "unresolved_dependency_endpoints": unresolved_dependencies,
        "missing_evidence": missing_evidence,
        "missing_claim_ids": missing_claims,
        "blocked_by_declared_only_required_rules": declared_only_required,
        "state": if missing_members.is_empty()
            && unresolved_dependencies.is_empty()
            && missing_claims.is_empty()
            && missing_evidence.is_empty()
            && unverified_references == 0
            && declared_only_required.is_empty()
        {
            "complete"
        } else {
            "partial"
        },
    })
}
