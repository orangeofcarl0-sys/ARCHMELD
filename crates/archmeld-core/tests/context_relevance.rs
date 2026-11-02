// Copyright 2026 ARCHMELD contributors
// SPDX-License-Identifier: Apache-2.0

//! B023 follow-up: ContextPack selection follows the objective, irrelevant
//! claims do not crowd out the implementation, and a resumed pack continues the
//! same deterministic candidate order.

mod common;

use archmeld_core::bridge::{import_codegraph_export, BridgeConfig};
use archmeld_core::context::create_context_pack;
use archmeld_core::interpret::{propose_claim, review_claim};
use archmeld_core::snapshot::{capture_snapshot, SnapshotInputs, SnapshotOptions};
use serde_json::{json, Value};
use std::collections::{BTreeMap, BTreeSet};
use std::fs;

fn setup() -> (common::Fixture, BTreeMap<String, String>) {
    let files: Vec<(String, String)> = std::iter::once((
        "src/target.py".to_string(),
        "def target_widget():\n    unique_widget_marker = 1\n".to_string(),
    ))
    .chain(std::iter::once((
        "src/alpha.py".to_string(),
        "def alpha():\n    alpha_marker = 1\n".to_string(),
    )))
    .chain((0..12).map(|index| {
        (
            format!("src/common_{index:02}.py"),
            format!("def common_{index:02}():\n    common_marker = {index}\n"),
        )
    }))
    .collect();
    let mut fixture = common::setup_prepared("b023-context-relevance", |root| {
        fs::create_dir_all(root.join("src")).unwrap();
        for (path, content) in &files {
            fs::write(root.join(path), content).unwrap();
        }
    });
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
    let mut lines = Vec::new();
    let mut next_id = 1u64;
    let mut expected = BTreeMap::new();
    for (path, _) in &files {
        let name = match path.as_str() {
            "src/target.py" => "target_widget".to_string(),
            "src/alpha.py" => "alpha".to_string(),
            other => other
                .rsplit('/')
                .next()
                .unwrap()
                .trim_end_matches(".py")
                .to_string(),
        };
        let node = json!({
            "type":"node","id":next_id,"node_type":"Function","properties":{
                "path": fixture.root.join(path).to_string_lossy(),
                "name": name, "line_start": 1, "line_end": 2
            }
        });
        expected.insert(name, next_id);
        next_id += 1;
        lines.push(node);
    }
    let mut export_lines = vec![json!({
        "type":"manifest","format":"archmeld-export-v1","source":"codegraph",
        "version":"0.2.0","node_count":lines.len(),"edge_count":0,"iteration":"real-iterators"
    })
    .to_string()];
    for line in &lines {
        export_lines.push(line.to_string());
    }
    import_codegraph_export(
        &mut fixture.store,
        &BridgeConfig {
            project_id: &fixture.project,
            snapshot_id: &fixture.snapshot_id,
            config_digest: &fixture.config_digest,
            run_id: "run.context.relevance",
            workspace_root: Some(&fixture.root),
            max_entities_per_batch: 500,
            max_relations_per_batch: 2000,
        },
        &captured.files,
        &(export_lines.join("\n") + "\n"),
    )
    .unwrap();
    let mut ids = BTreeMap::new();
    for id in fixture
        .store
        .entity_ids(&fixture.project, &fixture.snapshot_id)
        .unwrap()
    {
        let name = fixture
            .store
            .get_entity(&fixture.project, &fixture.snapshot_id, &id)
            .unwrap()
            .unwrap()["qualified_name"]
            .as_str()
            .unwrap()
            .rsplit("::")
            .next()
            .unwrap()
            .to_string();
        ids.insert(name, id);
    }
    (fixture, ids)
}

fn pack(
    fixture: &mut common::Fixture,
    objective: &str,
    entity_ids: &[String],
    budget: u64,
    cursor: Option<&str>,
) -> Value {
    create_context_pack(
        &mut fixture.store,
        &json!({
            "project_id": fixture.project,
            "snapshot_id": fixture.snapshot_id,
            "objective": objective,
            "entity_ids": entity_ids,
            "budget_tokens": budget,
            "cursor": cursor,
        }),
    )
    .unwrap()
}

fn selected_ids(pack: &Value) -> BTreeSet<String> {
    pack["selected_entities"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|entity| entity["entity_id"].as_str().map(str::to_string))
        .collect()
}

fn add_claim(
    fixture: &mut common::Fixture,
    id: &str,
    text: &str,
    key: &str,
    evidence_id: &str,
) {
    let claim = json!({
        "id": id,
        "project_id": fixture.project,
        "snapshot_id": fixture.snapshot_id,
        "claim_type": "hypothesis",
        "text": text,
        "status": "proposed",
        "support_ids": [],
        "counterevidence_ids": [],
        "scope": "context.relevance",
        "producer": {
            "name": "human",
            "version": "manual-1",
            "source_ref": "operator-note",
            "run_id": "run.context.relevance.claim"
        },
        "unresolved_questions": [],
    });
    propose_claim(
        &mut fixture.store,
        &json!({
            "project_id": fixture.project,
            "claim": claim,
            "expected_revision": 1,
            "idempotency_key": key,
        }),
    )
    .unwrap();
    review_claim(
        &mut fixture.store,
        &fixture.project,
        id,
        1,
        "rejected",
        "the fixture marks this claim rejected",
        &json!({"name": "operator"}),
        &[],
        &[evidence_id.to_string()],
        &format!("{key}-review"),
    )
    .unwrap();
}

#[test]
fn objective_selects_the_target_and_unrelated_claims_do_not_crowd_it_out() {
    let (mut fixture, ids) = setup();
    let evidence_id = "evidence.codegraph.export.run.context.relevance";
    add_claim(
        &mut fixture,
        "claim.relevance.target",
        "the target_widget marker is the important behavior",
        "claim-relevance-target",
        evidence_id,
    );
    for index in 0..30 {
        add_claim(
            &mut fixture,
            &format!("claim.relevance.unrelated.{index:02}"),
            &format!("unrelated claim {index} about bananas"),
            &format!("claim-relevance-unrelated-{index}"),
            evidence_id,
        );
    }
    let result = pack(
        &mut fixture,
        "locate the unique widget marker implementation",
        &[],
        4000,
        None,
    );
    assert_eq!(result["relevance"]["seed_source"], "objective");
    assert_eq!(result["relevance"]["fallback"], false);
    let selected = selected_ids(&result);
    let target = ids["target_widget"].clone();
    assert!(selected.contains(&target), "{selected:?}");
    assert_eq!(
        result["selected_entities"][0]["entity_id"], target,
        "the objective-matched entity should be selected first"
    );
    assert_eq!(
        result["selected_entities"][0]["relevance"]["seed"],
        true
    );
    let claims = result["claims"].as_array().unwrap();
    assert!(claims
        .iter()
        .any(|claim| claim["claim_id"] == "claim.relevance.target"));
    let preserved_unrelated = claims
        .iter()
        .filter(|claim| {
            claim["claim_id"]
                .as_str()
                .unwrap_or("")
                .starts_with("claim.relevance.unrelated.")
        })
        .count();
    assert!(
        preserved_unrelated <= 2,
        "unrelated rejected claims must not crowd out the relevant slice: {preserved_unrelated}"
    );
    assert!(result["omissions"].as_array().unwrap().iter().any(|item| {
        item["kind"] == "claim"
            && item["reason"] == "irrelevant_to_objective"
            && item["count"].as_u64().unwrap_or(0) >= 28
    }));
}

#[test]
fn small_budget_cursor_pages_cover_the_same_entities_as_a_large_budget() {
    let (mut fixture, ids) = setup();
    let explicit: Vec<String> = ids.values().cloned().collect();
    let large = pack(&mut fixture, "inspect common files", &explicit, 64_000, None);
    let large_ids = selected_ids(&large);
    assert_eq!(large_ids.len(), explicit.len());

    let mut page = pack(&mut fixture, "inspect common files", &explicit, 1024, None);
    let cursor = page["next_cursor"]
        .as_str()
        .expect("tight budget must page")
        .to_string();
    let mut paged_ids = selected_ids(&page);
    let mut pages = 1;
    while let Some(token) = page["next_cursor"].as_str().map(str::to_string) {
        assert!(pages < 32, "cursor does not terminate");
        page = pack(
            &mut fixture,
            "inspect common files",
            &explicit,
            1024,
            Some(&token),
        );
        paged_ids.extend(selected_ids(&page));
        pages += 1;
    }
    assert_eq!(paged_ids, large_ids);

    // The cursor is bound to the objective and snapshot.
    let conflict = create_context_pack(
        &mut fixture.store,
        &json!({
            "project_id": fixture.project,
            "snapshot_id": fixture.snapshot_id,
            "objective": "a different objective",
            "entity_ids": explicit,
            "budget_tokens": 1024,
            "cursor": cursor,
        }),
    )
    .expect_err("cursor scope");
    assert!(
        conflict.to_string().contains("context cursor"),
        "{conflict}"
    );
}

#[test]
fn unmatched_objective_falls_back_and_says_so() {
    let (mut fixture, _ids) = setup();
    let result = pack(
        &mut fixture,
        "zzzz no such symbol",
        &[],
        2000,
        None,
    );
    assert_eq!(result["relevance"]["seed_source"], "fallback");
    assert_eq!(result["relevance"]["fallback"], true);
    assert!(!result["selected_entities"].as_array().unwrap().is_empty());
    assert!(result["unresolved_questions"]
        .as_array()
        .unwrap()
        .iter()
        .any(|question| question
            .as_str()
            .unwrap()
            .contains("objective matched no symbol")));
}
