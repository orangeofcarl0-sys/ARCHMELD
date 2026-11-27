// Copyright 2026 ARCHMELD contributors
// SPDX-License-Identifier: Apache-2.0

//! B018 target architecture diffing and executable architecture rules (S07/S10).
//!
//! Editing a To-Be graph is not a source edit: the diff produces typed intents
//! and a candidate plan, never a ChangeSet. Rule checks only read the evidence
//! ledger; when coverage is partial they report `no_violation_found` instead of
//! claiming `passed`.

use crate::error::{CoreError, Result};
use crate::interpret::{get_architecture, validate_architecture_document};
use crate::store::{validate, Store};
use serde_json::{json, Value};
use std::collections::{BTreeMap, BTreeSet, HashMap};

const MAX_PATHS: usize = 100;
const MAX_DEPTH: usize = 6;
const CHECKER_DEPENDENCY: &str = "ledger.dependency_path.v1";
const CHECKER_ACYCLIC: &str = "architecture.acyclic.v1";
const CHECKER_ALLOWED_LAYERS: &str = "architecture.allowed_layers.v1";
const CHECKER_INTERFACE_ONLY: &str = "architecture.interface_only.v1";
const CHECKER_OWNERSHIP: &str = "architecture.ownership.v1";
const CHECKER_CUSTOM_CONTRACT: &str = "architecture.custom_contract.v1";

pub fn rule_registry() -> Value {
    json!({
        "version": "1",
        "rules": {
            "forbid_dependency": {
                "checker_id": CHECKER_DEPENDENCY,
                "checkability": "executable",
                "coverage": "static ledger relations (imports, references, calls, observed calls)",
                "max_depth": MAX_DEPTH,
                "limitations": [
                    "dynamic dispatch and unresolved calls are not traversed",
                    "a missing path is only no_violation_found, not proof of absence"
                ]
            },
            "allowed_layers": {
                "checker_id": CHECKER_ALLOWED_LAYERS,
                "checkability": "executable",
                "coverage": "declared candidate dependencies against declared layer order",
                "parameters": "layers: [[component_id,...], ...] ordered outer to inner; allow_same_layer: boolean (default true)",
                "limitations": ["declared candidate dependencies only; runtime edges are out of scope"]
            },
            "acyclic": {
                "checker_id": CHECKER_ACYCLIC,
                "checkability": "executable",
                "coverage": "candidate dependency graph declared in the architecture",
                "limitations": ["declared candidate dependencies only; runtime cycles are out of scope"]
            },
            "interface_only": {
                "checker_id": CHECKER_INTERFACE_ONLY,
                "checkability": "executable",
                "coverage": "declared candidate dependencies crossing a component boundary",
                "parameters": "interfaces: [component_id, ...]; a cross-component dependency must touch a declared interface",
                "limitations": ["interface membership is declared on the architecture; no runtime dispatch proof"]
            },
            "ownership": {
                "checker_id": CHECKER_OWNERSHIP,
                "checkability": "executable",
                "coverage": "declared owned_state/owned_resources across components",
                "parameters": "exclusive: boolean (default true)",
                "limitations": ["declared ownership only; actual write paths and runtime ownership are not proven"]
            },
            "custom_contract": {
                "checker_id": CHECKER_CUSTOM_CONTRACT,
                "checkability": "executable",
                "coverage": "declared candidate graph",
                "parameters": "assertion: no_self_dependencies | max_dependencies_per_component (+limit) | require_dependency_reason",
                "limitations": ["small registered assertion set; arbitrary scripts are not executed"]
            }
        }
    })
}

/// Diff a stored As-Is architecture against a submitted To-Be model. The
/// function performs no file writes, creates no ChangeSet and does not modify
/// either architecture.
pub fn diff_architecture(
    store: &Store,
    project_id: &str,
    base_architecture_id: &str,
    target: &Value,
) -> Result<Value> {
    validate::require_id("project_id", project_id)?;
    validate::require_id("base_architecture_id", base_architecture_id)?;
    let base = get_architecture(store, project_id, base_architecture_id)?;
    if base["layer"] != "as_is" {
        return Err(CoreError::invalid(
            "base architecture for a diff must have layer=as_is",
        ));
    }
    let target = validate_architecture_document(store, project_id, target)?;
    if target["layer"] != "to_be" {
        return Err(CoreError::invalid(
            "target architecture for a diff must have layer=to_be",
        ));
    }
    if target["status"] != "candidate" && target["status"] != "needs_review" {
        return Err(CoreError::invalid(
            "proposal-only: target architecture status must be candidate or needs_review",
        ));
    }
    if target["snapshot_id"] != base["snapshot_id"] {
        return Err(CoreError::conflict(
            "target architecture must bind the same snapshot as the as-is model",
        ));
    }
    let base_components = component_map(&base);
    let target_components = component_map(&target);
    let base_dependencies = dependency_map(&base);
    let target_dependencies = dependency_map(&target);

    let added_components: Vec<&str> = target_components
        .keys()
        .filter(|id| !base_components.contains_key(*id))
        .map(String::as_str)
        .collect();
    let removed_components: Vec<&str> = base_components
        .keys()
        .filter(|id| !target_components.contains_key(*id))
        .map(String::as_str)
        .collect();
    let mut changed_components = Vec::new();
    for (id, base_component) in &base_components {
        if let Some(target_component) = target_components.get(id) {
            if component_projection(base_component) != component_projection(target_component) {
                changed_components.push(json!({
                    "component_id": id,
                    "base": component_projection(base_component),
                    "target": component_projection(target_component),
                }));
            }
        }
    }

    let added_dependencies: Vec<Value> = target_dependencies
        .iter()
        .filter(|(key, _)| !base_dependencies.contains_key(*key))
        .map(|(_, dependency)| dependency_summary(dependency))
        .collect();
    let removed_dependencies: Vec<Value> = base_dependencies
        .iter()
        .filter(|(key, _)| !target_dependencies.contains_key(*key))
        .map(|(_, dependency)| dependency_summary(dependency))
        .collect();

    let members = all_members(&base);
    let mut intents = Vec::new();
    for dependency in &removed_dependencies {
        let source_members = component_members(&base_components, dependency["source_id"].as_str());
        let target_members = component_members(&base_components, dependency["target_id"].as_str());
        let (paths, _) = find_paths(
            store,
            project_id,
            base["snapshot_id"].as_str().unwrap_or(""),
            &source_members,
            &target_members,
            &members,
            MAX_DEPTH,
        )?;
        intents.push(json!({
            "type": "remove_dependency",
            "source_component": dependency["source_id"],
            "target_component": dependency["target_id"],
            "reason": dependency["reason"],
            "affected_paths": paths,
            "path_state": if paths.is_empty() { "no_ledger_path_found" } else { "observed_ledger_paths" },
            "contracts_to_preserve": ["public interfaces of both components", "runtime behaviour of existing callers"],
            "questions": if paths.is_empty() {
                json!(["no ledger path was found; confirm the dependency is not dynamic or unresolved"])
            } else {
                json!(["confirm each observed path is acceptable to remove"])
            }
        }));
    }
    for dependency in &added_dependencies {
        intents.push(json!({
            "type": "introduce_boundary",
            "source_component": dependency["source_id"],
            "target_component": dependency["target_id"],
            "reason": dependency["reason"],
            "affected_paths": [],
            "path_state": "candidate_new_dependency",
            "contracts_to_preserve": [],
            "questions": ["define the allowed interface for the new dependency"]
        }));
    }
    for removed in &removed_components {
        intents.push(json!({
            "type": "retire_component",
            "component_id": removed,
            "reason": "component is absent from the target model",
            "affected_paths": [],
            "contracts_to_preserve": [],
            "questions": ["where do the retired component's responsibilities and owned state go?"]
        }));
    }
    if !changed_components.is_empty() {
        intents.push(json!({
            "type": "split_responsibility",
            "components": changed_components,
            "reason": "responsibilities, ownership or membership changed in the target model",
            "affected_paths": [],
            "contracts_to_preserve": ["public interfaces of changed components"],
            "questions": ["is each new responsibility covered by an owner?"]
        }));
    }

    let snapshot_files: i64 = store
        .conn
        .query_row(
            "SELECT COUNT(*) FROM files WHERE project_id=?1 AND snapshot_id=?2",
            (project_id, base["snapshot_id"].as_str().unwrap_or("")),
            |row| row.get(0),
        )
        .unwrap_or(0);
    let plan_steps: Vec<Value> = intents
        .iter()
        .enumerate()
        .map(|(index, intent)| {
            json!({
                "step": index + 1,
                "intent": intent["type"],
                "state": "candidate",
                "requires_change_set": true,
            })
        })
        .collect();
    Ok(json!({
        "base_architecture_id": base_architecture_id,
        "base_revision": base["revision"],
        "target_architecture_id": target["id"],
        "target_revision": target["revision"],
        "snapshot_id": base["snapshot_id"],
        "components": {
            "added": added_components,
            "removed": removed_components,
            "changed": changed_components,
        },
        "dependencies": {
            "added": added_dependencies,
            "removed": removed_dependencies,
        },
        "intents": intents,
        "affected_paths": intents
            .iter()
            .flat_map(|intent| intent["affected_paths"].as_array().cloned().unwrap_or_default())
            .collect::<Vec<Value>>(),
        "source_mutation": {
            "state": "none",
            "files_written": 0,
            "changesets_created": 0,
            "repository_files_in_snapshot": snapshot_files,
        },
        "plan_candidate": {
            "state": "candidate",
            "steps": plan_steps,
            "non_goals": ["automatic source edits", "applying a ChangeSet"],
        },
    }))
}

/// Run the executable rule registry over a stored architecture. A rule without
/// registered coverage cannot pass acceptance, and a static path search alone
/// reports `no_violation_found`, never `passed`.
pub fn check_architecture_rules(
    store: &Store,
    project_id: &str,
    architecture_id: &str,
    rule_ids: Option<&[String]>,
) -> Result<Value> {
    validate::require_id("project_id", project_id)?;
    validate::require_id("architecture_id", architecture_id)?;
    let architecture = get_architecture(store, project_id, architecture_id)?;
    let snapshot_id = architecture["snapshot_id"]
        .as_str()
        .ok_or_else(|| CoreError::Storage("architecture snapshot is missing".into()))?;
    let components = component_map(&architecture);
    let dependencies = architecture["dependencies"]
        .as_array()
        .cloned()
        .unwrap_or_default();
    let rules = architecture["rules"].as_array().cloned().unwrap_or_default();
    let selected: BTreeSet<&str> = rule_ids
        .unwrap_or(&[])
        .iter()
        .map(String::as_str)
        .collect();
    let mut results = Vec::new();
    let mut blocking = Vec::new();
    for rule in &rules {
        let rule_id = rule["id"].as_str().unwrap_or("unknown");
        if !selected.is_empty() && !selected.contains(rule_id) {
            continue;
        }
        let result = check_rule(
            store,
            project_id,
            snapshot_id,
            &components,
            &dependencies,
            rule,
        );
        if result["blocks_acceptance"] == true {
            blocking.push(rule_id.to_string());
        }
        results.push(result);
    }
    let state = if results
        .iter()
        .any(|result| result["verdict"] == "violated")
    {
        "failed"
    } else if !blocking.is_empty() {
        "blocked"
    } else if !results.is_empty() && results.iter().all(|result| result["verdict"] == "passed") {
        "passed"
    } else if results.iter().any(|result| result["verdict"] == "no_violation_found") {
        "incomplete_coverage"
    } else {
        "unverified"
    };
    Ok(json!({
        "architecture_id": architecture_id,
        "snapshot_id": snapshot_id,
        "registry": rule_registry(),
        "results": results,
        "blocking_rules": blocking,
        "state": state,
    }))
}

fn check_rule(
    store: &Store,
    project_id: &str,
    snapshot_id: &str,
    components: &BTreeMap<String, Value>,
    dependencies: &[Value],
    rule: &Value,
) -> Value {
    let rule_id = rule["id"].as_str().unwrap_or("unknown");
    let kind = rule["kind"].as_str().unwrap_or("unknown");
    let required = rule["required"].as_bool().unwrap_or(false);
    let checker_id = rule["checker_id"].as_str().unwrap_or("");
    let declared_only = |reason: &str| {
        json!({
            "rule_id": rule_id,
            "kind": kind,
            "required": required,
            "checker_id": if checker_id.is_empty() { Value::Null } else { json!(checker_id) },
            "checkability": "declared_only",
            "verdict": "declared_only",
            "passed": false,
            "blocks_acceptance": required,
            "reason": reason,
            "coverage": {"state": "none", "reason": "no executable checker is registered"},
            "violations": [],
        })
    };
    if kind == "acyclic" {
        if checker_id != CHECKER_ACYCLIC {
            return declared_only(
                "acyclic requires the registered architecture.acyclic.v1 checker",
            );
        }
        let scope: Vec<&str> = rule["scope_ids"]
            .as_array()
            .map(|ids| ids.iter().filter_map(Value::as_str).collect())
            .unwrap_or_default();
        let allowed: BTreeSet<&str> = if scope.is_empty() {
            components.keys().map(String::as_str).collect()
        } else {
            scope.iter().copied().collect()
        };
        let missing: Vec<&str> = allowed
            .iter()
            .filter(|id| !components.contains_key(**id))
            .copied()
            .collect();
        if !missing.is_empty() {
            return json!({
                "rule_id": rule_id,
                "kind": kind,
                "required": required,
                "checker_id": CHECKER_ACYCLIC,
                "checkability": "executable",
                "verdict": "inconclusive",
                "passed": false,
                "blocks_acceptance": required,
                "reason": "scope component is not declared in the architecture",
                "coverage": {"state": "none", "missing_scope_ids": missing},
                "violations": [],
            });
        }
        let mut adjacency: BTreeMap<&str, BTreeSet<&str>> = BTreeMap::new();
        let mut edges_considered = 0usize;
        for dependency in dependencies {
            let (Some(source), Some(target)) = (
                dependency["source_id"].as_str(),
                dependency["target_id"].as_str(),
            ) else {
                continue;
            };
            if !allowed.contains(source) || !allowed.contains(target) {
                continue;
            }
            adjacency.entry(source).or_default().insert(target);
            edges_considered += 1;
        }
        if let Some(cycle) = find_declared_cycle(&adjacency, &allowed) {
            return json!({
                "rule_id": rule_id,
                "kind": kind,
                "required": required,
                "checker_id": CHECKER_ACYCLIC,
                "checkability": "executable",
                "verdict": "violated",
                "passed": false,
                "blocks_acceptance": true,
                "reason": "the declared candidate dependency graph contains a cycle",
                "coverage": {
                    "state": "complete_declared_graph",
                    "components": allowed.len(),
                    "edges_considered": edges_considered,
                    "limitations": ["declared candidate dependencies only; runtime cycles are out of scope"],
                },
                "violations": [{"cycle": cycle}],
            });
        }
        return json!({
            "rule_id": rule_id,
            "kind": kind,
            "required": required,
            "checker_id": CHECKER_ACYCLIC,
            "checkability": "executable",
            "verdict": "passed",
            "passed": true,
            "blocks_acceptance": false,
            "reason": "no cycle exists in the declared candidate dependency graph",
            "coverage": {
                "state": "complete_declared_graph",
                "components": allowed.len(),
                "edges_considered": edges_considered,
                "limitations": ["declared candidate dependencies only; runtime cycles are out of scope"],
            },
            "violations": [],
        });
    }
    if kind == "allowed_layers" {
        if checker_id != CHECKER_ALLOWED_LAYERS {
            return declared_only("allowed_layers requires the registered architecture.allowed_layers.v1 checker");
        }
        let parameters = rule.get("parameters").cloned().unwrap_or_else(|| json!({}));
        let Some(layers) = parameters["layers"].as_array() else {
            return json!({
                "rule_id": rule_id, "kind": kind, "required": required,
                "checker_id": CHECKER_ALLOWED_LAYERS, "checkability": "executable",
                "verdict": "inconclusive", "passed": false,
                "blocks_acceptance": required,
                "reason": "parameters.layers must be an array of component-id arrays",
                "coverage": {"state": "none", "reason": "missing layer declaration"},
                "violations": [],
            });
        };
        let allow_same = parameters["allow_same_layer"].as_bool().unwrap_or(true);
        let mut layer_of: BTreeMap<&str, usize> = BTreeMap::new();
        let mut duplicate = false;
        for (index, layer) in layers.iter().enumerate() {
            let Some(ids) = layer.as_array() else {
                duplicate = true;
                break;
            };
            for id in ids.iter().filter_map(Value::as_str) {
                if layer_of.insert(id, index).is_some() {
                    duplicate = true;
                }
            }
        }
        let missing: Vec<&str> = components
            .keys()
            .filter(|id| !layer_of.contains_key(id.as_str()))
            .map(String::as_str)
            .collect();
        if duplicate {
            return json!({
                "rule_id": rule_id, "kind": kind, "required": required,
                "checker_id": CHECKER_ALLOWED_LAYERS, "checkability": "executable",
                "verdict": "inconclusive", "passed": false,
                "blocks_acceptance": required,
                "reason": "a component is declared in more than one layer or a layer entry is invalid",
                "coverage": {"state": "none", "missing_components": missing},
                "violations": [],
            });
        }
        let mut violations = Vec::new();
        let mut edges_considered = 0usize;
        let mut uncovered_edges = 0usize;
        for dependency in dependencies {
            let (Some(source), Some(target)) = (
                dependency["source_id"].as_str(),
                dependency["target_id"].as_str(),
            ) else { continue };
            let (Some(source_layer), Some(target_layer)) = (
                layer_of.get(source).copied(),
                layer_of.get(target).copied(),
            ) else {
                uncovered_edges += 1;
                continue;
            };
            edges_considered += 1;
            if source_layer > target_layer || (!allow_same && source_layer == target_layer) {
                violations.push(json!({
                    "source_id": source,
                    "target_id": target,
                    "source_layer": source_layer,
                    "target_layer": target_layer,
                    "reason": if source_layer > target_layer {
                        "dependency points to an outer layer"
                    } else {
                        "same-layer dependency is not allowed"
                    },
                }));
            }
        }
        let complete = missing.is_empty() && uncovered_edges == 0;
        let verdict = if !violations.is_empty() {
            "violated"
        } else if complete {
            "passed"
        } else {
            "no_violation_found"
        };
        return json!({
            "rule_id": rule_id, "kind": kind, "required": required,
            "checker_id": CHECKER_ALLOWED_LAYERS, "checkability": "executable",
            "verdict": verdict, "passed": verdict == "passed",
            "blocks_acceptance": required && verdict != "passed" || verdict == "violated",
            "reason": if verdict == "violated" {
                "a declared dependency crosses layers in the forbidden direction"
            } else if verdict == "passed" {
                "every declared dependency follows the layer order"
            } else {
                "layer coverage is incomplete; absence of a violation is not proof"
            },
            "coverage": {
                "state": if complete { "complete_declared_graph" } else { "partial" },
                "components": components.len(),
                "layers": layers.len(),
                "edges_considered": edges_considered,
                "uncovered_edges": uncovered_edges,
                "missing_components": missing,
                "limitations": ["declared candidate dependencies only; runtime edges are out of scope"],
            },
            "violations": violations,
        });
    }
    if kind == "interface_only" {
        if checker_id != CHECKER_INTERFACE_ONLY {
            return declared_only("interface_only requires the registered architecture.interface_only.v1 checker");
        }
        let parameters = rule.get("parameters").cloned().unwrap_or_else(|| json!({}));
        let Some(interfaces) = parameters["interfaces"].as_array() else {
            return json!({
                "rule_id": rule_id, "kind": kind, "required": required,
                "checker_id": CHECKER_INTERFACE_ONLY, "checkability": "executable",
                "verdict": "inconclusive", "passed": false,
                "blocks_acceptance": required,
                "reason": "parameters.interfaces must be an array of component ids",
                "coverage": {"state": "none"}, "violations": [],
            });
        };
        let declared: BTreeSet<&str> = interfaces.iter().filter_map(Value::as_str).collect();
        let missing: Vec<&str> = declared
            .iter()
            .filter(|id| !components.contains_key(**id))
            .copied()
            .collect();
        if missing.len() == declared.len() {
            return json!({
                "rule_id": rule_id, "kind": kind, "required": required,
                "checker_id": CHECKER_INTERFACE_ONLY, "checkability": "executable",
                "verdict": "inconclusive", "passed": false,
                "blocks_acceptance": required,
                "reason": "no declared interface component exists",
                "coverage": {"state": "none", "missing_interfaces": missing},
                "violations": [],
            });
        }
        let mut violations = Vec::new();
        let mut edges_considered = 0usize;
        for dependency in dependencies {
            let (Some(source), Some(target)) = (
                dependency["source_id"].as_str(),
                dependency["target_id"].as_str(),
            ) else { continue };
            if source == target {
                continue;
            }
            edges_considered += 1;
            if !declared.contains(source) && !declared.contains(target) {
                violations.push(json!({
                    "source_id": source,
                    "target_id": target,
                    "reason": "cross-component dependency bypasses every declared interface",
                }));
            }
        }
        let complete = missing.is_empty();
        let verdict = if !violations.is_empty() {
            "violated"
        } else if complete {
            "passed"
        } else {
            "no_violation_found"
        };
        return json!({
            "rule_id": rule_id, "kind": kind, "required": required,
            "checker_id": CHECKER_INTERFACE_ONLY, "checkability": "executable",
            "verdict": verdict, "passed": verdict == "passed",
            "blocks_acceptance": required && verdict != "passed" || verdict == "violated",
            "reason": if verdict == "violated" {
                "a cross-component dependency does not touch a declared interface"
            } else if verdict == "passed" {
                "every cross-component dependency touches a declared interface"
            } else {
                "some declared interfaces are missing from the architecture"
            },
            "coverage": {
                "state": if complete { "complete_declared_graph" } else { "partial" },
                "interfaces": declared.len(),
                "edges_considered": edges_considered,
                "missing_interfaces": missing,
                "limitations": ["interface membership is declared; runtime dispatch is not proven"],
            },
            "violations": violations,
        });
    }
    if kind == "ownership" {
        if checker_id != CHECKER_OWNERSHIP {
            return declared_only("ownership requires the registered architecture.ownership.v1 checker");
        }
        let parameters = rule.get("parameters").cloned().unwrap_or_else(|| json!({}));
        let exclusive = parameters["exclusive"].as_bool().unwrap_or(true);
        let mut owners: BTreeMap<&str, BTreeSet<&str>> = BTreeMap::new();
        for (component_id, component) in components {
            for field in ["owned_state", "owned_resources"] {
                for item in component[field]
                    .as_array()
                    .into_iter()
                    .flatten()
                    .filter_map(Value::as_str)
                {
                    owners.entry(item).or_default().insert(component_id);
                }
            }
        }
        let violations: Vec<Value> = if exclusive {
            owners
                .iter()
                .filter(|(_, owners)| owners.len() > 1)
                .map(|(item, owners)| json!({
                    "item": item,
                    "owners": owners,
                    "reason": "declared ownership is not exclusive",
                }))
                .collect()
        } else {
            Vec::new()
        };
        let verdict = if !violations.is_empty() {
            "violated"
        } else {
            "passed"
        };
        return json!({
            "rule_id": rule_id, "kind": kind, "required": required,
            "checker_id": CHECKER_OWNERSHIP, "checkability": "executable",
            "verdict": verdict, "passed": verdict == "passed",
            "blocks_acceptance": required && verdict != "passed" || verdict == "violated",
            "reason": if verdict == "violated" {
                "a declared state or resource has more than one owner"
            } else {
                "declared ownership is exclusive"
            },
            "coverage": {
                "state": "complete_declared_graph",
                "owned_items": owners.len(),
                "exclusive": exclusive,
                "limitations": ["declared ownership only; actual writes are not proven"],
            },
            "violations": violations,
        });
    }
    if kind == "custom_contract" {
        if checker_id != CHECKER_CUSTOM_CONTRACT {
            return declared_only("custom_contract requires the registered architecture.custom_contract.v1 checker");
        }
        let parameters = rule.get("parameters").cloned().unwrap_or_else(|| json!({}));
        let assertion = parameters["assertion"].as_str().unwrap_or("");
        let mut violations = Vec::new();
        match assertion {
            "no_self_dependencies" => {
                for dependency in dependencies {
                    if dependency["source_id"] == dependency["target_id"] {
                        violations.push(json!({
                            "source_id": dependency["source_id"],
                            "reason": "self dependency",
                        }));
                    }
                }
            }
            "max_dependencies_per_component" => {
                let Some(limit) = parameters["limit"].as_u64() else {
                    return json!({
                        "rule_id": rule_id, "kind": kind, "required": required,
                        "checker_id": CHECKER_CUSTOM_CONTRACT, "checkability": "executable",
                        "verdict": "inconclusive", "passed": false,
                        "blocks_acceptance": required,
                        "reason": "max_dependencies_per_component requires parameters.limit",
                        "coverage": {"state": "none"}, "violations": [],
                    });
                };
                let mut counts: BTreeMap<&str, u64> = BTreeMap::new();
                for dependency in dependencies {
                    if let Some(source) = dependency["source_id"].as_str() {
                        *counts.entry(source).or_insert(0) += 1;
                    }
                }
                for (component_id, count) in counts {
                    if count > limit {
                        violations.push(json!({
                            "component_id": component_id,
                            "dependencies": count,
                            "limit": limit,
                        }));
                    }
                }
            }
            "require_dependency_reason" => {
                for dependency in dependencies {
                    if dependency["reason"]
                        .as_str()
                        .unwrap_or("")
                        .trim()
                        .is_empty()
                    {
                        violations.push(json!({
                            "source_id": dependency["source_id"],
                            "target_id": dependency["target_id"],
                            "reason": "missing dependency reason",
                        }));
                    }
                }
            }
            other => {
                return json!({
                    "rule_id": rule_id, "kind": kind, "required": required,
                    "checker_id": CHECKER_CUSTOM_CONTRACT, "checkability": "executable",
                    "verdict": "inconclusive", "passed": false,
                    "blocks_acceptance": required,
                    "reason": format!("unsupported custom assertion {other:?}"),
                    "coverage": {"state": "none"}, "violations": [],
                });
            }
        }
        let verdict = if !violations.is_empty() {
            "violated"
        } else {
            "passed"
        };
        return json!({
            "rule_id": rule_id, "kind": kind, "required": required,
            "checker_id": CHECKER_CUSTOM_CONTRACT, "checkability": "executable",
            "verdict": verdict, "passed": verdict == "passed",
            "blocks_acceptance": required && verdict != "passed" || verdict == "violated",
            "reason": if verdict == "violated" {
                "the declared custom assertion is violated"
            } else {
                "the declared custom assertion holds on the candidate graph"
            },
            "coverage": {
                "state": "complete_declared_graph",
                "assertion": assertion,
                "limitations": ["small registered assertion set; arbitrary scripts are not executed"],
            },
            "violations": violations,
        });
    }
    if kind != "forbid_dependency" || checker_id != CHECKER_DEPENDENCY {
        let reason = if checker_id.is_empty() {
            "rule has no checker; a human description is not an executable check"
        } else {
            "checker_id is not registered; human intent cannot be treated as executable"
        };
        return declared_only(reason);
    }
    let scope_ids: Vec<&str> = rule["scope_ids"]
        .as_array()
        .map(|ids| ids.iter().filter_map(Value::as_str).collect())
        .unwrap_or_default();
    if scope_ids.len() < 2 {
        return declared_only("forbid_dependency requires a source and a forbidden target component");
    }
    let source_component = scope_ids[0];
    let target_component = *scope_ids.last().unwrap();
    let (Some(source), Some(target)) = (
        components.get(source_component),
        components.get(target_component),
    ) else {
        return json!({
            "rule_id": rule_id,
            "kind": kind,
            "required": required,
            "checker_id": CHECKER_DEPENDENCY,
            "checkability": "executable",
            "verdict": "no_violation_found",
            "passed": false,
            "blocks_acceptance": required,
            "reason": "scope component is not declared in the architecture",
            "coverage": {"state": "none", "reason": "scope component missing"},
            "violations": [],
        });
    };
    let mut scope_members = BTreeSet::new();
    let mut missing_members = Vec::new();
    for scope_id in &scope_ids {
        let Some(component) = components.get(*scope_id) else {
            continue;
        };
        for member in component_members_map(component) {
            let exists: Option<String> = store
                .conn
                .query_row(
                    "SELECT entity_id FROM entities
                     WHERE project_id=?1 AND snapshot_id=?2 AND entity_id=?3",
                    (project_id, snapshot_id, &member),
                    |row| row.get(0),
                )
                .ok();
            if exists.is_some() {
                scope_members.insert(member);
            } else {
                missing_members.push(json!({"component_id": scope_id, "entity_id": member}));
            }
        }
    }
    let starts = component_member_set(source);
    let targets = component_member_set(target);
    if starts.is_empty() || targets.is_empty() {
        return json!({
            "rule_id": rule_id,
            "kind": kind,
            "required": required,
            "checker_id": CHECKER_DEPENDENCY,
            "checkability": "executable",
            "verdict": "no_violation_found",
            "passed": false,
            "blocks_acceptance": required,
            "reason": "source or target component has no member entities",
            "coverage": {"state": "none", "reason": "empty component membership"},
            "violations": [],
        });
    }
    let (paths, edges_considered) = match find_paths(
        store,
        project_id,
        snapshot_id,
        &starts,
        &targets,
        &scope_members,
        MAX_DEPTH,
    ) {
        Ok(paths) => paths,
        Err(error) => {
            return json!({
                "rule_id": rule_id,
                "kind": kind,
                "required": required,
                "checker_id": CHECKER_DEPENDENCY,
                "checkability": "executable",
                "verdict": "no_violation_found",
                "passed": false,
                "blocks_acceptance": required,
                "reason": error.to_string(),
                "coverage": {"state": "none", "reason": "path search failed"},
                "violations": [],
            })
        }
    };
    let verdict = if !paths.is_empty() {
        "violated"
    } else {
        // A static ledger path search is not a completeness proof.
        "no_violation_found"
    };
    json!({
        "rule_id": rule_id,
        "kind": kind,
        "required": required,
        "checker_id": CHECKER_DEPENDENCY,
        "checkability": "executable",
        "verdict": verdict,
        "passed": false,
        "blocks_acceptance": verdict == "violated" || required,
        "reason": if paths.is_empty() {
            "no path was found, but static coverage is partial"
        } else {
            "a dependency path reaches the forbidden component"
        },
        "coverage": {
            "state": "partial",
            "members_expected": scope_members.len() + missing_members.len(),
            "members_found": scope_members.len(),
            "missing_members": missing_members,
            "max_depth": MAX_DEPTH,
            "edges_considered": edges_considered,
            "limitations": [
                "static ledger relations only; dynamic dispatch and unresolved calls are not traversed",
                "absence of a path is not proof of absence"
            ],
        },
        "violations": paths,
    })
}

fn find_declared_cycle<'a>(
    adjacency: &BTreeMap<&'a str, BTreeSet<&'a str>>,
    allowed: &BTreeSet<&'a str>,
) -> Option<Vec<String>> {
    #[derive(Clone, Copy, PartialEq)]
    enum Mark {
        New,
        Visiting,
        Done,
    }
    fn visit<'a>(
        node: &'a str,
        adjacency: &BTreeMap<&'a str, BTreeSet<&'a str>>,
        marks: &mut BTreeMap<&'a str, Mark>,
        stack: &mut Vec<&'a str>,
    ) -> Option<Vec<String>> {
        marks.insert(node, Mark::Visiting);
        stack.push(node);
        if let Some(neighbors) = adjacency.get(node) {
            for next in neighbors {
                match marks.get(next).copied().unwrap_or(Mark::New) {
                    Mark::Visiting => {
                        let position = stack.iter().position(|item| item == next).unwrap_or(0);
                        let mut cycle: Vec<String> =
                            stack[position..].iter().map(|item| item.to_string()).collect();
                        cycle.push(next.to_string());
                        return Some(cycle);
                    }
                    Mark::New => {
                        if let Some(cycle) = visit(next, adjacency, marks, stack) {
                            return Some(cycle);
                        }
                    }
                    Mark::Done => {}
                }
            }
        }
        stack.pop();
        marks.insert(node, Mark::Done);
        None
    }
    let mut marks: BTreeMap<&str, Mark> = BTreeMap::new();
    let mut stack: Vec<&str> = Vec::new();
    for node in allowed {
        if marks.get(node).copied().unwrap_or(Mark::New) == Mark::New {
            if let Some(cycle) = visit(node, adjacency, &mut marks, &mut stack) {
                return Some(cycle);
            }
        }
    }
    None
}

fn find_paths(
    store: &Store,
    project_id: &str,
    snapshot_id: &str,
    starts: &BTreeSet<String>,
    targets: &BTreeSet<String>,
    traversable: &BTreeSet<String>,
    max_depth: usize,
) -> Result<(Vec<Value>, usize)> {
    let relations = store.relation_payloads(project_id, snapshot_id)?;
    let mut adjacency: HashMap<String, Vec<(String, String, String)>> = HashMap::new();
    let mut edges_considered = 0usize;
    for relation in relations {
        let (Some(source), Some(target), Some(kind), Some(id)) = (
            relation["source_id"].as_str(),
            relation["target_id"].as_str(),
            relation["kind"].as_str(),
            relation["id"].as_str(),
        ) else {
            continue;
        };
        if !dependency_relation_kind(kind) {
            continue;
        }
        edges_considered += 1;
        adjacency
            .entry(source.to_string())
            .or_default()
            .push((target.to_string(), kind.to_string(), id.to_string()));
    }
    for edges in adjacency.values_mut() {
        edges.sort();
    }
    let mut paths: Vec<Value> = Vec::new();
    for start in starts {
        let mut visited = BTreeSet::new();
        visited.insert(start.clone());
        let mut steps = Vec::new();
        walk(
            &adjacency,
            start,
            targets,
            traversable,
            &mut visited,
            &mut steps,
            0,
            max_depth,
            &mut paths,
        );
        if paths.len() >= MAX_PATHS {
            break;
        }
    }
    Ok((paths, edges_considered))
}

fn dependency_relation_kind(kind: &str) -> bool {
    matches!(
        kind,
        "imports"
            | "references"
            | "static_call_candidate"
            | "resolved_call"
            | "observed_call"
            | "reads"
            | "writes"
            | "publishes"
            | "consumes"
            | "awaits"
            | "schedules"
            | "communicates_with"
    )
}

#[allow(clippy::too_many_arguments)]
fn walk(
    adjacency: &HashMap<String, Vec<(String, String, String)>>,
    current: &str,
    targets: &BTreeSet<String>,
    traversable: &BTreeSet<String>,
    visited: &mut BTreeSet<String>,
    steps: &mut Vec<Value>,
    depth: usize,
    max_depth: usize,
    paths: &mut Vec<Value>,
) {
    if paths.len() >= MAX_PATHS || depth >= max_depth {
        return;
    }
    let Some(edges) = adjacency.get(current) else {
        return;
    };
    for (target, kind, relation_id) in edges {
        if !visited.insert(target.clone()) {
            continue;
        }
        steps.push(json!({
            "from": current,
            "to": target,
            "kind": kind,
            "relation_id": relation_id,
            "basis": "ledger",
        }));
        if targets.contains(target) {
            paths.push(json!({
                "nodes": path_nodes(steps),
                "steps": steps.clone(),
                "depth": steps.len(),
                "state": "candidate_path",
            }));
        } else if traversable.contains(target) {
            walk(
                adjacency,
                target,
                targets,
                traversable,
                visited,
                steps,
                depth + 1,
                max_depth,
                paths,
            );
        }
        steps.pop();
        visited.remove(target);
        if paths.len() >= MAX_PATHS {
            return;
        }
    }
}

fn path_nodes(steps: &[Value]) -> Vec<String> {
    let mut nodes = Vec::new();
    for step in steps {
        if nodes.is_empty() {
            if let Some(from) = step["from"].as_str() {
                nodes.push(from.to_string());
            }
        }
        if let Some(to) = step["to"].as_str() {
            nodes.push(to.to_string());
        }
    }
    nodes
}

fn component_map(architecture: &Value) -> BTreeMap<String, Value> {
    architecture["components"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|component| {
            component["id"]
                .as_str()
                .map(|id| (id.to_string(), component.clone()))
        })
        .collect()
}

fn dependency_map(architecture: &Value) -> BTreeMap<String, Value> {
    architecture["dependencies"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|dependency| {
            let source = dependency["source_id"].as_str()?;
            let target = dependency["target_id"].as_str()?;
            Some((format!("{source}->{target}"), dependency.clone()))
        })
        .collect()
}

fn dependency_summary(dependency: &Value) -> Value {
    json!({
        "source_id": dependency["source_id"],
        "target_id": dependency["target_id"],
        "reason": dependency["reason"],
        "status": dependency["status"],
    })
}

fn component_projection(component: &Value) -> Value {
    json!({
        "name": component["name"],
        "purpose": component["purpose"],
        "responsibilities": component["responsibilities"],
        "non_responsibilities": component["non_responsibilities"],
        "member_entity_ids": component["member_entity_ids"],
        "owned_state": component["owned_state"],
        "owned_resources": component["owned_resources"],
        "runtime_placement": component["runtime_placement"],
    })
}

fn component_members(components: &BTreeMap<String, Value>, id: Option<&str>) -> BTreeSet<String> {
    id.and_then(|id| components.get(id))
        .map(component_member_set)
        .unwrap_or_default()
}

fn component_member_set(component: &Value) -> BTreeSet<String> {
    component_members_map(component).into_iter().collect()
}

fn component_members_map(component: &Value) -> Vec<String> {
    component["member_entity_ids"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(Value::as_str)
        .map(str::to_string)
        .collect()
}

fn all_members(architecture: &Value) -> BTreeSet<String> {
    architecture["components"]
        .as_array()
        .into_iter()
        .flatten()
        .flat_map(|component| component_member_set(component))
        .collect()
}
