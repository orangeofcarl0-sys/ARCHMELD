// Copyright 2026 ARCHMELD contributors
// SPDX-License-Identifier: Apache-2.0

//! B023 / AT054: bounded ContextPacks carry snapshot/version/evidence
//! references, explicit omissions and a resume cursor; counterexamples stay
//! visible or are listed as omissions.

mod common;

use archmeld_core::bridge::{import_codegraph_export, BridgeConfig};
use archmeld_core::context::create_context_pack;
use archmeld_core::hash;
use archmeld_core::interpret::{propose_claim, review_claim};
use archmeld_core::snapshot::{capture_snapshot, SnapshotInputs, SnapshotOptions};
use serde_json::{json, Value};
use std::fs;
use std::path::Path;

fn import_graph(fixture: &mut common::Fixture) -> (String, String) {
    let captured = capture_snapshot(
        &mut fixture.store,
        &fixture.project,
        &fixture.root,
        &SnapshotInputs {
            build_config: fixture.build_config.clone(),
            environment: fixture.environment.clone(),
        },
        &SnapshotOptions {
            include_untracked: true,
            ..SnapshotOptions::default()
        },
    )
    .unwrap();
    assert_eq!(captured.snapshot_id, fixture.snapshot_id);
    let node = |id: i64, path: &str, name: &str, line: i64| {
        json!({"type":"node","id":id,"node_type":"Function","properties":{
            "path": fixture.root.join(path).to_string_lossy(),
            "name": name,
            "line_start": line,
            "line_end": line,
        }})
    };
    let nodes = [
        node(1, "src/a.py", "a", 1),
        node(2, "src/b.py", "b", 1),
        node(3, "src/calc/keep.py", "keep", 1),
    ];
    let mut lines = vec![json!({
        "type":"manifest","format":"archmeld-export-v1","source":"codegraph",
        "version":"0.2.0","node_count":3,"edge_count":2,"iteration":"real-iterators"
    })
    .to_string()];
    for node in &nodes {
        lines.push(node.to_string());
    }
    lines.push(
        json!({"type":"edge","id":1,"source":1,"target":2,"edge_type":"Calls","properties":{"line":1}})
            .to_string(),
    );
    lines.push(
        json!({"type":"edge","id":2,"source":1,"target":3,"edge_type":"References","properties":{"line":1}})
            .to_string(),
    );
    let export = lines.join("\n") + "\n";
    import_codegraph_export(
        &mut fixture.store,
        &BridgeConfig {
            project_id: &fixture.project,
            snapshot_id: &fixture.snapshot_id,
            config_digest: &fixture.config_digest,
            run_id: "run.context.pack",
            workspace_root: Some(&fixture.root),
            max_entities_per_batch: 500,
            max_relations_per_batch: 2000,
        },
        &captured.files,
        &export,
    )
    .unwrap();
    let entity_ids = fixture
        .store
        .entity_ids(&fixture.project, &fixture.snapshot_id)
        .unwrap();
    let a_id = entity_ids
        .iter()
        .find(|id| {
            fixture
                .store
                .get_entity(&fixture.project, &fixture.snapshot_id, id)
                .unwrap()
                .and_then(|entity| entity["qualified_name"].as_str().map(str::to_string))
                .is_some_and(|name| name.ends_with("::a"))
        })
        .unwrap()
        .clone();
    let evidence_id = "evidence.codegraph.export.run.context.pack".to_string();
    (a_id, evidence_id)
}

fn rejected_claim(fixture: &mut common::Fixture, evidence_id: &str) -> String {
    let claim = json!({
        "id": "claim.context.dominant",
        "project_id": fixture.project,
        "snapshot_id": fixture.snapshot_id,
        "claim_type": "hypothesis",
        "text": "The dominant behavior is in helper b and must be rewritten.",
        "status": "proposed",
        "support_ids": [],
        "counterevidence_ids": [],
        "scope": "context.pack",
        "producer": {
            "name": "human",
            "version": "manual-1",
            "source_ref": "operator-note",
            "run_id": "run.context.claim",
        },
        "unresolved_questions": ["is helper b reached at runtime?"],
    });
    let proposed = propose_claim(
        &mut fixture.store,
        &json!({
            "project_id": fixture.project,
            "claim": claim,
            "expected_revision": 1,
            "idempotency_key": "claim-context-1",
        }),
    )
    .unwrap();
    let claim_id = proposed["id"].as_str().unwrap().to_string();
    review_claim(
        &mut fixture.store,
        &fixture.project,
        &claim_id,
        1,
        "rejected",
        "the imported static graph contradicts the hypothesis",
        &json!({"name": "operator"}),
        &[],
        &[evidence_id.to_string()],
        "review-context-1",
    )
    .unwrap();
    claim_id
}

fn pack(fixture: &mut common::Fixture, entity_id: &str, budget: u64) -> Value {
    create_context_pack(
        &mut fixture.store,
        &json!({
            "project_id": fixture.project,
            "snapshot_id": fixture.snapshot_id,
            "objective": "decide whether helper b is the dominant behavior without changing source",
            "non_goals": ["no source mutation", "no model calls"],
            "entity_ids": [entity_id],
            "budget_tokens": budget,
        }),
    )
    .unwrap()
}

#[test]
fn context_pack_is_bounded_traceable_and_preserves_counterexamples() {
    let mut fixture = common::setup("b023-context");
    let (entity_id, evidence_id) = import_graph(&mut fixture);
    let claim_id = rejected_claim(&mut fixture, &evidence_id);

    let large = pack(&mut fixture, &entity_id, 4000);
    assert_eq!(large["schema_version"], "1.0");
    assert_eq!(large["project_id"], fixture.project);
    assert_eq!(large["snapshot_id"], fixture.snapshot_id);
    assert_eq!(large["permissions_summary"]["mode"], "read_only");
    assert_eq!(large["permissions_summary"]["approval_tools"], false);
    assert_eq!(large["budget"]["truncated"], false);
    assert!(large["budget"]["estimated_tokens"].as_u64().unwrap() <= 4000);
    let claims = large["claims"].as_array().unwrap();
    assert!(claims.iter().any(|claim| claim["claim_id"] == claim_id));
    let counter = claims
        .iter()
        .find(|claim| claim["claim_id"] == claim_id)
        .unwrap();
    assert_eq!(counter["status"], "rejected");
    assert!(counter["counterevidence_ids"]
        .as_array()
        .unwrap()
        .iter()
        .any(|id| id == &json!(evidence_id)));
    assert!(large["evidence_refs"]
        .as_array()
        .unwrap()
        .iter()
        .any(|item| item["evidence_id"] == evidence_id));
    assert!(!large["selected_entities"].as_array().unwrap().is_empty());
    let snippets = large["source_snippets"].as_array().unwrap();
    assert!(!snippets.is_empty());
    for snippet in snippets {
        assert!(snippet["path"].as_str().unwrap().starts_with("src/"));
        assert!(snippet["blob_digest"]
            .as_str()
            .unwrap()
            .starts_with("sha256:"));
        assert!(snippet["anchor"]["path"].is_string());
        assert!(!snippet["text"].as_str().unwrap().is_empty());
    }
    assert!(!large["graph_slices"].as_array().unwrap().is_empty());
    for slice in large["graph_slices"].as_array().unwrap() {
        assert!(slice["basis"].is_string());
        assert!(slice["resolution"].is_string());
    }
    let mut digest_preimage = large.clone();
    digest_preimage
        .as_object_mut()
        .unwrap()
        .remove("context_digest");
    digest_preimage
        .as_object_mut()
        .unwrap()
        .remove("pack_artifact");
    assert_eq!(
        hash::digest_value(&digest_preimage).unwrap(),
        large["context_digest"].as_str().unwrap()
    );
    assert!(fixture
        .store
        .blob_exists(large["pack_artifact"].as_str().unwrap())
        .unwrap());

    // A tight budget must truncate explicitly, list omissions, and either keep
    // the rejected counterexample or name it as omitted.
    let small = pack(&mut fixture, &entity_id, 512);
    assert_eq!(small["budget"]["truncated"], true);
    assert!(small["budget"]["estimated_tokens"].as_u64().unwrap() <= 512);
    assert!(!small["omissions"].as_array().unwrap().is_empty());
    let kept = small["claims"]
        .as_array()
        .unwrap()
        .iter()
        .any(|claim| claim["claim_id"] == claim_id && claim["status"] == "rejected");
    let omitted = small["omissions"]
        .as_array()
        .unwrap()
        .iter()
        .any(|item| item["kind"] == "claim" && item["id"] == claim_id);
    assert!(
        kept || omitted,
        "counterexample must be kept or listed: {small}"
    );
    assert!(small["unresolved_questions"]
        .as_array()
        .unwrap()
        .iter()
        .any(|question| question.as_str().unwrap().contains("omitted")));
    assert!(small["budget"]["tokens"].as_u64().unwrap() == 512);

    write_evidence(&fixture, &large, &small, &claim_id, &evidence_id);
}

fn write_evidence(
    fixture: &common::Fixture,
    large: &Value,
    small: &Value,
    claim_id: &str,
    evidence_id: &str,
) {
    if std::env::var("ARCHMELD_WRITE_EVIDENCE").ok().as_deref() != Some("1") {
        return;
    }
    let payload = json!({
        "generated_by": "crates/archmeld-core/tests/context_pack.rs",
        "platform": std::env::consts::OS,
        "project_id": fixture.project,
        "snapshot_id": fixture.snapshot_id,
        "config_digest": fixture.config_digest,
        "acceptance": {
            "AT054": {
                "status": "passed",
                "large_budget": {
                    "tokens": large["budget"]["tokens"],
                    "estimated_tokens": large["budget"]["estimated_tokens"],
                    "truncated": large["budget"]["truncated"],
                    "claims": large["claims"].as_array().map(Vec::len),
                    "snippets": large["source_snippets"].as_array().map(Vec::len),
                    "relations": large["graph_slices"].as_array().map(Vec::len),
                    "evidence_refs": large["evidence_refs"],
                    "context_digest": large["context_digest"],
                    "pack_artifact": large["pack_artifact"],
                },
                "small_budget": {
                    "tokens": small["budget"]["tokens"],
                    "estimated_tokens": small["budget"]["estimated_tokens"],
                    "truncated": small["budget"]["truncated"],
                    "omissions": small["omissions"],
                    "next_cursor": small["next_cursor"],
                    "unresolved_questions": small["unresolved_questions"],
                },
                "counterexample_claim": claim_id,
                "counterevidence_id": evidence_id,
                "counterexample_kept_or_omitted": true,
            }
        }
    });
    let path =
        Path::new(env!("CARGO_MANIFEST_DIR")).join("../../docs/evidence/B023-context-pack.json");
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(path, serde_json::to_string_pretty(&payload).unwrap() + "\n").unwrap();
}
