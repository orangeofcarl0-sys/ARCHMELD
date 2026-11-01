// Copyright 2026 ARCHMELD contributors
// SPDX-License-Identifier: Apache-2.0

//! B018: To-Be diff produces intents/affected paths without source mutation,
//! executable forbid_dependency checks find direct and indirect paths, and
//! required declared-only rules block acceptance.

use archmeld_core::blueprint::{check_architecture_rules, diff_architecture};
use archmeld_core::hash;
use archmeld_core::identity::line_entity_id;
use archmeld_core::interpret::propose_architecture;
use archmeld_core::snapshot::{capture_snapshot, SnapshotInputs, SnapshotOptions};
use archmeld_core::store::Store;
use serde_json::{json, Value};
use std::collections::BTreeMap;
use std::fs;
use std::path::Path;
use std::sync::Mutex;

static EVIDENCE_LOCK: Mutex<()> = Mutex::new(());

const P: &str = "project.b018";

struct Fixture {
    _dir: tempfile::TempDir,
    store: Store,
    root: std::path::PathBuf,
    snapshot_id: String,
    config_digest: String,
    ids: BTreeMap<&'static str, String>,
    evidence_id: String,
}

fn anchor(path: &str, digest: &str, line: u64) -> Value {
    json!({
        "path": path,
        "blob_digest": digest,
        "precision": "line_only",
        "byte_start": null,
        "byte_end": null,
        "line_start": line,
        "line_end": line,
        "encoding": "utf-8",
    })
}

fn setup() -> Fixture {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().join("repo");
    fs::create_dir_all(root.join("src")).unwrap();
    let files = [
        ("src/ui.py", "from adapter import read_state\n\ndef render():\n    return read_state()\n"),
        ("src/adapter.py", "from estimator import state\n\ndef read_state():\n    return state()\n"),
        ("src/estimator.py", "def state():\n    return 42\n"),
    ];
    for (path, text) in files {
        fs::write(root.join(path), text).unwrap();
    }
    let mut store = Store::open(dir.path().join("state")).unwrap();
    store.ensure_project(P, "expedition").unwrap();
    let snap = capture_snapshot(
        &mut store,
        P,
        &root,
        &SnapshotInputs {
            build_config: json!({"variant": "b018"}),
            environment: json!({"os": "test"}),
        },
        &SnapshotOptions {
            include_untracked: true,
            ..SnapshotOptions::default()
        },
    )
    .unwrap();
    let snapshot_id = snap.snapshot_id.clone();
    let config_digest = snap.snapshot["config_digest"].as_str().unwrap().to_string();
    let mut ids = BTreeMap::new();
    let mut entities = Vec::new();
    for (key, path, name, line) in [
        ("ui", "src/ui.py", "render", 3),
        ("adapter", "src/adapter.py", "read_state", 3),
        ("estimator", "src/estimator.py", "state", 1),
    ] {
        let file = snap
            .files
            .iter()
            .find(|entry| entry.path == path)
            .unwrap_or_else(|| panic!("missing {path}"));
        let id = line_entity_id(
            &file.blob_digest,
            "symbol",
            line,
            line,
            &format!("{path}::symbol::{name}"),
        )
        .unwrap();
        ids.insert(key, id.clone());
        entities.push(json!({
            "id": id,
            "project_id": P,
            "snapshot_id": snapshot_id,
            "config_digest": config_digest,
            "kind": "symbol",
            "language": "python",
            "qualified_name": format!("{path}::{name}"),
            "anchors": [anchor(path, &file.blob_digest, line)],
            "external": false,
            "backend_refs": [],
        }));
    }
    let evidence_id = "evidence.syntax.b018";
    let artifact = snap.files[0].blob_digest.clone();
    let relation = |id: &str, source: &str, target: &str, line: u64| {
        json!({
            "id": id,
            "project_id": P,
            "snapshot_id": snapshot_id,
            "config_digest": config_digest,
            "source_id": source,
            "target_id": target,
            "kind": "references",
            "basis": "syntax",
            "evidence_ids": [evidence_id],
            "conditions": [format!("fixture-reference-line:{line}")],
            "resolution": "candidate",
        })
    };
    let batch = json!({
        "schema_version": "1.0",
        "project_id": P,
        "snapshot_id": snapshot_id,
        "config_digest": config_digest,
        "run_id": "run.b018.syntax",
        "adapter_id": "fixture.syntax",
        "adapter_version": "1",
        "sequence": "0",
        "final": true,
        "coverage": {
            "state": "complete",
            "scope": "b018 fixture relations",
            "expected_count": 3,
            "observed_count": 3,
            "omitted_count": 0,
            "unresolved_count": 0,
            "limitations": [],
        },
        "entities": entities,
        "evidence": [{
            "id": evidence_id,
            "project_id": P,
            "snapshot_id": snapshot_id,
            "config_digest": config_digest,
            "kind": "syntax",
            "producer": {"name": "fixture.syntax", "version": "1", "source_ref": "line-anchor", "run_id": "run.b018.syntax"},
            "artifact_digest": artifact,
            "locator": "src/",
            "source_anchor": null,
            "scenario_id": null,
            "runtime_run_id": null,
            "coverage": {
                "state": "complete",
                "scope": "b018 fixture relations",
                "expected_count": 3,
                "observed_count": 3,
                "omitted_count": 0,
                "unresolved_count": 0,
                "limitations": [],
            },
            "freshness": "current",
        }],
        "relations": [
            relation("rel.b018.ui.estimator", &ids["ui"], &ids["estimator"], 4),
            relation("rel.b018.ui.adapter", &ids["ui"], &ids["adapter"], 4),
            relation("rel.b018.adapter.estimator", &ids["adapter"], &ids["estimator"], 4),
        ],
        "diagnostics": [],
    });
    store.import_batch(&batch).unwrap();
    Fixture {
        _dir: dir,
        store,
        root,
        snapshot_id,
        config_digest,
        ids,
        evidence_id: evidence_id.to_string(),
    }
}

fn component(id: &str, name: &str, members: &[String]) -> Value {
    json!({
        "id": id,
        "name": name,
        "purpose": format!("{name} responsibility"),
        "responsibilities": [format!("{name} responsibility")],
        "non_responsibilities": [],
        "member_entity_ids": members,
        "owned_state": [],
        "owned_resources": [],
        "runtime_placement": [],
        "evidence_ids": [],
        "open_questions": [],
    })
}

fn rule(id: &str, kind: &str, scope: &[&str], required: bool, checker: Option<&str>, checkability: &str) -> Value {
    json!({
        "id": id,
        "kind": kind,
        "description": format!("{id} rule"),
        "scope_ids": scope,
        "required": required,
        "checker_id": checker,
        "checkability": checkability,
    })
}

#[test]
fn target_graph_diff_never_touches_source_and_rules_find_direct_and_indirect_paths() {
    let mut fixture = setup();
    let before: Vec<String> = ["src/ui.py", "src/adapter.py", "src/estimator.py"]
        .iter()
        .map(|path| hash::digest_bytes(&fs::read(fixture.root.join(path)).unwrap()))
        .collect();
    let relations_before = fixture
        .store
        .relation_payloads(P, &fixture.snapshot_id)
        .unwrap()
        .len();

    let base_model = json!({
        "schema_version": "1.0",
        "id": "arch.base.b018",
        "project_id": P,
        "snapshot_id": fixture.snapshot_id,
        "revision": 1,
        "layer": "as_is",
        "status": "candidate",
        "components": [
            component(
                "component.ui",
                "UI",
                &[fixture.ids["ui"].clone(), fixture.ids["adapter"].clone()],
            ),
            component("component.estimator", "Estimator", &[fixture.ids["estimator"].clone()]),
        ],
        "dependencies": [{
            "source_id": "component.ui",
            "target_id": "component.estimator",
            "reason": "UI reads estimator internal state through a helper",
            "evidence_ids": [fixture.evidence_id],
            "status": "candidate",
        }],
        "rules": [
            rule(
                "rule.no-ui-estimator",
                "forbid_dependency",
                &["component.ui", "component.estimator"],
                true,
                Some("ledger.dependency_path.v1"),
                "executable",
            ),
            rule(
                "rule.no-reverse",
                "forbid_dependency",
                &["component.estimator", "component.ui"],
                true,
                Some("ledger.dependency_path.v1"),
                "executable",
            ),
            rule(
                "rule.declared-only",
                "custom_contract",
                &["component.ui"],
                true,
                None,
                "declared_only",
            ),
            rule(
                "rule.human-intent",
                "forbid_dependency",
                &["component.ui", "component.estimator"],
                false,
                Some("human.opinion"),
                "executable",
            ),
        ],
        "claim_ids": [],
    });
    let base = propose_architecture(
        &mut fixture.store,
        &json!({
            "project_id": P,
            "model": base_model,
            "expected_revision": 1,
            "idempotency_key": "b018-base-1",
        }),
    )
    .unwrap();

    let checks = check_architecture_rules(
        &fixture.store,
        P,
        base["id"].as_str().unwrap(),
        None,
    )
    .unwrap();
    assert_eq!(checks["state"], "failed", "{checks}");
    let results = checks["results"].as_array().unwrap();
    let find = |id: &str| results.iter().find(|result| result["rule_id"] == id).unwrap();
    let violation = find("rule.no-ui-estimator");
    assert_eq!(violation["verdict"], "violated");
    assert_eq!(violation["passed"], false);
    let paths = violation["violations"].as_array().unwrap();
    assert_eq!(paths.iter().map(|path| path["depth"].as_u64()).flatten().min(), Some(1));
    assert_eq!(paths.iter().map(|path| path["depth"].as_u64()).flatten().max(), Some(2));
    assert_eq!(violation["coverage"]["members_found"], 3);
    assert_eq!(violation["coverage"]["edges_considered"], 3);
    assert_eq!(violation["coverage"]["state"], "partial");
    let reverse = find("rule.no-reverse");
    assert_eq!(reverse["verdict"], "no_violation_found");
    assert_eq!(reverse["passed"], false, "partial coverage never passes");
    let declared = find("rule.declared-only");
    assert_eq!(declared["verdict"], "declared_only");
    assert_eq!(declared["blocks_acceptance"], true);
    let human = find("rule.human-intent");
    assert_eq!(human["verdict"], "declared_only");
    assert!(human["reason"]
        .as_str()
        .unwrap()
        .contains("human intent"));
    assert!(checks["blocking_rules"]
        .as_array()
        .unwrap()
        .iter()
        .any(|id| id == "rule.declared-only"));

    let mut target = base.clone();
    target["id"] = json!("arch.target.b018");
    target["revision"] = json!(1);
    target["layer"] = json!("to_be");
    target["status"] = json!("candidate");
    target["dependencies"] = json!([]);
    target["rules"] = json!([]);
    let diff = diff_architecture(
        &fixture.store,
        P,
        base["id"].as_str().unwrap(),
        &target,
    )
    .unwrap();
    assert_eq!(diff["source_mutation"]["state"], "none");
    assert_eq!(diff["source_mutation"]["files_written"], 0);
    assert_eq!(diff["source_mutation"]["changesets_created"], 0);
    assert_eq!(diff["dependencies"]["removed"].as_array().unwrap().len(), 1);
    let intents = diff["intents"].as_array().unwrap();
    let removal = intents
        .iter()
        .find(|intent| intent["type"] == "remove_dependency")
        .expect("remove_dependency intent");
    let affected = removal["affected_paths"].as_array().unwrap();
    assert!(affected.len() >= 2, "{affected:?}");
    assert_eq!(
        affected
            .iter()
            .map(|path| path["depth"].as_u64())
            .flatten()
            .max(),
        Some(2)
    );
    assert_eq!(diff["plan_candidate"]["state"], "candidate");
    let after: Vec<String> = ["src/ui.py", "src/adapter.py", "src/estimator.py"]
        .iter()
        .map(|path| hash::digest_bytes(&fs::read(fixture.root.join(path)).unwrap()))
        .collect();
    assert_eq!(before, after, "target graph diff must not modify source files");
    assert_eq!(
        fixture
            .store
            .relation_payloads(P, &fixture.snapshot_id)
            .unwrap()
            .len(),
        relations_before
    );
    assert!(fixture.store.integrity_check().unwrap().is_empty());

    // A target model cannot reuse an As-Is layer or accepted status.
    let mut wrong_layer = target.clone();
    wrong_layer["layer"] = json!("as_is");
    assert!(diff_architecture(&fixture.store, P, base["id"].as_str().unwrap(), &wrong_layer).is_err());
    let mut wrong_status = target.clone();
    wrong_status["status"] = json!("accepted");
    assert!(diff_architecture(&fixture.store, P, base["id"].as_str().unwrap(), &wrong_status).is_err());

    write_b018_evidence(json!({
        "generated_by": "crates/archmeld-core/tests/blueprint_rules.rs",
        "snapshot_id": fixture.snapshot_id,
        "config_digest": fixture.config_digest,
        "acceptance": {
            "AT040": {
                "status": "passed",
                "base_architecture_id": base["id"],
                "target_architecture_id": target["id"],
                "source_hashes_before": before,
                "source_hashes_after": after,
                "diff": diff,
            },
            "AT041": {
                "status": "passed",
                "checks": checks,
            },
            "AT075": {
                "status": "passed",
                "required_declared_only_blocks": declared["blocks_acceptance"],
                "partial_coverage_passes": reverse["passed"],
            }
        },
    }));
}

fn declared_dependency(source: &str, target: &str) -> Value {
    json!({
        "source_id": source,
        "target_id": target,
        "reason": "declared To-Be dependency",
        "evidence_ids": [],
        "status": "candidate",
    })
}

fn acyclic_model(fixture: &Fixture, id: &str, dependencies: Vec<Value>, checker: Option<&str>) -> Value {
    json!({
        "schema_version": "1.0",
        "id": id,
        "project_id": P,
        "snapshot_id": fixture.snapshot_id,
        "revision": 1,
        "layer": "to_be",
        "status": "candidate",
        "components": [
            component("component.ui", "UI", &[fixture.ids["ui"].clone()]),
            component("component.adapter", "Adapter", &[fixture.ids["adapter"].clone()]),
            component("component.estimator", "Estimator", &[fixture.ids["estimator"].clone()]),
        ],
        "dependencies": dependencies,
        "rules": [rule(
            "rule.acyclic",
            "acyclic",
            &["component.ui", "component.adapter", "component.estimator"],
            true,
            checker,
            if checker.is_some() { "executable" } else { "declared_only" },
        )],
        "claim_ids": [],
    })
}

#[test]
fn acyclic_rule_passes_only_on_a_complete_declared_graph() {
    let mut fixture = setup();
    let before: Vec<String> = ["src/ui.py", "src/adapter.py", "src/estimator.py"]
        .iter()
        .map(|path| hash::digest_bytes(&fs::read(fixture.root.join(path)).unwrap()))
        .collect();

    let acyclic = acyclic_model(
        &fixture,
        "arch.acyclic.ok",
        vec![
            declared_dependency("component.ui", "component.adapter"),
            declared_dependency("component.adapter", "component.estimator"),
        ],
        Some("architecture.acyclic.v1"),
    );
    let stored = propose_architecture(
        &mut fixture.store,
        &json!({
            "project_id": P,
            "model": acyclic,
            "expected_revision": 1,
            "idempotency_key": "b018-acyclic-ok",
        }),
    )
    .unwrap();
    let passed = check_architecture_rules(&fixture.store, P, stored["id"].as_str().unwrap(), None).unwrap();
    assert_eq!(passed["state"], "passed", "{passed}");
    let result = &passed["results"][0];
    assert_eq!(result["verdict"], "passed");
    assert_eq!(result["passed"], true);
    assert_eq!(result["blocks_acceptance"], false);
    assert_eq!(result["coverage"]["state"], "complete_declared_graph");
    assert_eq!(result["coverage"]["edges_considered"], 2);

    let cyclic = acyclic_model(
        &fixture,
        "arch.acyclic.bad",
        vec![
            declared_dependency("component.ui", "component.adapter"),
            declared_dependency("component.adapter", "component.estimator"),
            declared_dependency("component.estimator", "component.ui"),
        ],
        Some("architecture.acyclic.v1"),
    );
    let stored = propose_architecture(
        &mut fixture.store,
        &json!({
            "project_id": P,
            "model": cyclic,
            "expected_revision": 1,
            "idempotency_key": "b018-acyclic-bad",
        }),
    )
    .unwrap();
    let failed = check_architecture_rules(&fixture.store, P, stored["id"].as_str().unwrap(), None).unwrap();
    assert_eq!(failed["state"], "failed", "{failed}");
    let result = &failed["results"][0];
    assert_eq!(result["verdict"], "violated");
    assert_eq!(result["blocks_acceptance"], true);
    let cycle = result["violations"][0]["cycle"].as_array().unwrap();
    assert!(cycle.len() >= 4, "{cycle:?}");
    assert_eq!(cycle.first(), cycle.last());

    let unregistered = acyclic_model(
        &fixture,
        "arch.acyclic.unregistered",
        vec![declared_dependency("component.ui", "component.adapter")],
        Some("architecture.cycle.unknown"),
    );
    let stored = propose_architecture(
        &mut fixture.store,
        &json!({
            "project_id": P,
            "model": unregistered,
            "expected_revision": 1,
            "idempotency_key": "b018-acyclic-unregistered",
        }),
    )
    .unwrap();
    let blocked = check_architecture_rules(&fixture.store, P, stored["id"].as_str().unwrap(), None).unwrap();
    assert_eq!(blocked["state"], "blocked");
    assert_eq!(blocked["results"][0]["verdict"], "declared_only");
    assert_eq!(blocked["results"][0]["blocks_acceptance"], true);

    let after: Vec<String> = ["src/ui.py", "src/adapter.py", "src/estimator.py"]
        .iter()
        .map(|path| hash::digest_bytes(&fs::read(fixture.root.join(path)).unwrap()))
        .collect();
    assert_eq!(before, after);
    assert!(fixture.store.integrity_check().unwrap().is_empty());

    write_b018_evidence(json!({
        "acceptance": {"AT041_acyclic": {
            "status": "passed",
            "acyclic_check": passed,
            "cyclic_check": failed,
            "unregistered_checker": blocked,
            "source_unchanged": true,
        }}
    }));
}

fn write_b018_evidence(value: Value) {
    if std::env::var("ARCHMELD_WRITE_EVIDENCE").ok().as_deref() != Some("1") {
        return;
    }
    let _guard = EVIDENCE_LOCK.lock().unwrap();
    let path =
        Path::new(env!("CARGO_MANIFEST_DIR")).join("../../docs/evidence/B018-rules.json");
    let mut root = if path.exists() {
        serde_json::from_str::<Value>(&fs::read_to_string(&path).unwrap())
            .unwrap_or_else(|_| json!({}))
    } else {
        json!({})
    };
    merge_json(&mut root, value);
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(path, serde_json::to_string_pretty(&root).unwrap() + "\n").unwrap();
}

fn merge_json(target: &mut Value, source: Value) {
    match (target, source) {
        (Value::Object(target), Value::Object(source)) => {
            for (key, value) in source {
                merge_json(target.entry(key).or_insert(Value::Null), value);
            }
        }
        (target, source) => *target = source,
    }
}
fn rule_with_parameters(
    id: &str,
    kind: &str,
    scope: &[&str],
    required: bool,
    checker: &str,
    parameters: Value,
) -> Value {
    json!({
        "id": id,
        "kind": kind,
        "description": format!("{id} rule"),
        "scope_ids": scope,
        "required": required,
        "checker_id": checker,
        "checkability": "executable",
        "parameters": parameters,
    })
}

#[test]
fn declarative_architecture_checkers_are_executable_and_explain_coverage() {
    let mut fixture = setup();
    let before: Vec<String> = ["src/ui.py", "src/adapter.py", "src/estimator.py"]
        .iter()
        .map(|path| hash::digest_bytes(&fs::read(fixture.root.join(path)).unwrap()))
        .collect();
    let components = || {
        vec![
            component("component.ui", "UI", &[fixture.ids["ui"].clone()]),
            component("component.adapter", "Adapter", &[fixture.ids["adapter"].clone()]),
            component("component.estimator", "Estimator", &[fixture.ids["estimator"].clone()]),
        ]
    };
    let dependency = |source: &str, target: &str| {
        json!({
            "source_id": source,
            "target_id": target,
            "reason": "candidate dependency",
            "evidence_ids": [fixture.evidence_id],
            "status": "candidate",
        })
    };
    let model = |id: &str, dependencies: Vec<Value>, rules: Vec<Value>, components: Vec<Value>| {
        json!({
            "schema_version": "1.0",
            "id": id,
            "project_id": P,
            "snapshot_id": fixture.snapshot_id,
            "revision": 1,
            "layer": "to_be",
            "status": "candidate",
            "components": components,
            "dependencies": dependencies,
            "rules": rules,
            "claim_ids": [],
        })
    };
    let propose = |store: &mut Store, model: Value, key: &str| {
        propose_architecture(
            store,
            &json!({
                "project_id": P,
                "model": model,
                "expected_revision": 1,
                "idempotency_key": key,
            }),
        )
        .unwrap()
    };

    let layers = json!({
        "layers": [["component.ui"], ["component.adapter"], ["component.estimator"]],
        "allow_same_layer": true,
    });
    let passing_layers = propose(
        &mut fixture.store,
        model(
            "arch.layers.ok",
            vec![
                dependency("component.ui", "component.adapter"),
                dependency("component.adapter", "component.estimator"),
            ],
            vec![rule_with_parameters(
                "rule.layers",
                "allowed_layers",
                &["component.ui", "component.adapter", "component.estimator"],
                true,
                "architecture.allowed_layers.v1",
                layers.clone(),
            )],
            components(),
        ),
        "b018-layers-ok",
    );
    let passed = check_architecture_rules(
        &fixture.store,
        P,
        passing_layers["id"].as_str().unwrap(),
        None,
    )
    .unwrap();
    assert_eq!(passed["state"], "passed", "{passed}");
    assert_eq!(passed["results"][0]["verdict"], "passed");
    assert_eq!(
        passed["results"][0]["coverage"]["state"],
        "complete_declared_graph"
    );

    let violating_layers = propose(
        &mut fixture.store,
        model(
            "arch.layers.bad",
            vec![dependency("component.estimator", "component.ui")],
            vec![rule_with_parameters(
                "rule.layers",
                "allowed_layers",
                &["component.ui", "component.adapter", "component.estimator"],
                true,
                "architecture.allowed_layers.v1",
                layers,
            )],
            components(),
        ),
        "b018-layers-bad",
    );
    let violated = check_architecture_rules(
        &fixture.store,
        P,
        violating_layers["id"].as_str().unwrap(),
        None,
    )
    .unwrap();
    assert_eq!(violated["state"], "failed", "{violated}");
    assert_eq!(violated["results"][0]["verdict"], "violated");
    assert_eq!(violated["results"][0]["violations"][0]["source_id"], "component.estimator");

    let interface_pass = propose(
        &mut fixture.store,
        model(
            "arch.interface.ok",
            vec![
                dependency("component.ui", "component.adapter"),
                dependency("component.adapter", "component.estimator"),
            ],
            vec![rule_with_parameters(
                "rule.interfaces",
                "interface_only",
                &["component.ui", "component.adapter", "component.estimator"],
                true,
                "architecture.interface_only.v1",
                json!({"interfaces": ["component.adapter"]}),
            )],
            components(),
        ),
        "b018-interface-ok",
    );
    let interface_passed = check_architecture_rules(
        &fixture.store,
        P,
        interface_pass["id"].as_str().unwrap(),
        None,
    )
    .unwrap();
    assert_eq!(interface_passed["results"][0]["verdict"], "passed");
    let interface_bad = propose(
        &mut fixture.store,
        model(
            "arch.interface.bad",
            vec![dependency("component.ui", "component.estimator")],
            vec![rule_with_parameters(
                "rule.interfaces",
                "interface_only",
                &["component.ui", "component.adapter", "component.estimator"],
                true,
                "architecture.interface_only.v1",
                json!({"interfaces": ["component.adapter"]}),
            )],
            components(),
        ),
        "b018-interface-bad",
    );
    let interface_violated = check_architecture_rules(
        &fixture.store,
        P,
        interface_bad["id"].as_str().unwrap(),
        None,
    )
    .unwrap();
    assert_eq!(interface_violated["results"][0]["verdict"], "violated");

    let mut shared = components();
    shared[0]["owned_state"] = json!(["state.shared"]);
    shared[2]["owned_state"] = json!(["state.shared"]);
    let ownership_bad = propose(
        &mut fixture.store,
        model(
            "arch.ownership.bad",
            vec![],
            vec![rule_with_parameters(
                "rule.ownership",
                "ownership",
                &["component.ui", "component.estimator"],
                true,
                "architecture.ownership.v1",
                json!({"exclusive": true}),
            )],
            shared,
        ),
        "b018-ownership-bad",
    );
    let ownership_violated = check_architecture_rules(
        &fixture.store,
        P,
        ownership_bad["id"].as_str().unwrap(),
        None,
    )
    .unwrap();
    assert_eq!(ownership_violated["results"][0]["verdict"], "violated");
    assert_eq!(
        ownership_violated["results"][0]["violations"][0]["item"],
        "state.shared"
    );

    let custom_bad = propose(
        &mut fixture.store,
        model(
            "arch.custom.bad",
            vec![dependency("component.ui", "component.ui")],
            vec![rule_with_parameters(
                "rule.custom",
                "custom_contract",
                &["component.ui"],
                true,
                "architecture.custom_contract.v1",
                json!({"assertion": "no_self_dependencies"}),
            )],
            components(),
        ),
        "b018-custom-bad",
    );
    let custom_violated = check_architecture_rules(
        &fixture.store,
        P,
        custom_bad["id"].as_str().unwrap(),
        None,
    )
    .unwrap();
    assert_eq!(custom_violated["results"][0]["verdict"], "violated");

    let custom_unsupported = propose(
        &mut fixture.store,
        model(
            "arch.custom.unsupported",
            vec![],
            vec![rule_with_parameters(
                "rule.custom.unknown",
                "custom_contract",
                &["component.ui"],
                true,
                "architecture.custom_contract.v1",
                json!({"assertion": "run_arbitrary_script"}),
            )],
            components(),
        ),
        "b018-custom-unsupported",
    );
    let unsupported = check_architecture_rules(
        &fixture.store,
        P,
        custom_unsupported["id"].as_str().unwrap(),
        None,
    )
    .unwrap();
    assert_eq!(unsupported["results"][0]["verdict"], "inconclusive");
    assert_eq!(unsupported["state"], "blocked");

    let after: Vec<String> = ["src/ui.py", "src/adapter.py", "src/estimator.py"]
        .iter()
        .map(|path| hash::digest_bytes(&fs::read(fixture.root.join(path)).unwrap()))
        .collect();
    assert_eq!(before, after);
    assert!(fixture.store.integrity_check().unwrap().is_empty());
    write_b018_evidence(json!({
        "acceptance": {"AT041_declarative_checkers": {
            "status": "passed",
            "allowed_layers_pass": passed,
            "allowed_layers_violation": violated,
            "interface_pass": interface_passed,
            "interface_violation": interface_violated,
            "ownership_violation": ownership_violated,
            "custom_violation": custom_violated,
            "custom_unsupported": unsupported,
            "source_unchanged": true,
        }}
    }));
}
