// Copyright 2026 ARCHMELD contributors
// SPDX-License-Identifier: Apache-2.0

use archmeld_core::bridge::{import_codegraph_export, BridgeConfig};
use archmeld_core::brief::{
    build_brief, build_brief_with_budget, BriefBudget, BriefRecipe, BriefRequest,
};
use archmeld_core::manager::{open_project, OpenRequest};
use archmeld_core::snapshot::{capture_snapshot, SnapshotInputs, SnapshotOptions};
use archmeld_core::store::Store;
use serde_json::json;
use std::fs;
use std::path::PathBuf;

struct Fixture {
    _dir: tempfile::TempDir,
    store: Store,
    root: PathBuf,
    project: String,
    snapshot_id: String,
}

fn setup() -> Fixture {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().join("repo");
    fs::create_dir_all(root.join("src")).unwrap();
    fs::write(root.join("src/a.py"), "def a():\n    return b()\n").unwrap();
    fs::write(root.join("src/b.py"), "def b():\n    return 1\n").unwrap();
    let state = dir.path().join("state");
    let mut store = Store::open(&state).unwrap();
    open_project(
        &mut store,
        OpenRequest {
            project_id: "project.brief",
            mode: "expedition",
            workspace_root: &root,
            state_root: &state.join("sessions"),
            config: json!({"profile": "brief"}),
        },
    )
    .unwrap();
    let captured = capture_snapshot(
        &mut store,
        "project.brief",
        &root,
        &SnapshotInputs {
            build_config: json!({"variant": "brief"}),
            environment: json!({"os": "test"}),
        },
        &SnapshotOptions {
            include_untracked: true,
            ..SnapshotOptions::default()
        },
    )
    .unwrap();
    let config_digest = captured.snapshot["config_digest"]
        .as_str()
        .unwrap()
        .to_string();
    let nodes = [
        json!({"type":"node","id":1,"node_type":"Function","properties":{
            "path": root.join("src/a.py").to_string_lossy(),"name":"a",
            "line_start":1,"line_end":2}}),
        json!({"type":"node","id":2,"node_type":"Function","properties":{
            "path": root.join("src/b.py").to_string_lossy(),"name":"b",
            "line_start":1,"line_end":2}}),
    ];
    let mut lines = vec![json!({
        "type":"manifest","format":"archmeld-export-v1","source":"codegraph",
        "version":"0.2.0","node_count":2,"edge_count":1,"iteration":"real-iterators"
    })
    .to_string()];
    for node in &nodes {
        lines.push(node.to_string());
    }
    lines.push(
        json!({"type":"edge","id":1,"source":1,"target":2,"edge_type":"Calls","properties":{"line":2}})
            .to_string(),
    );
    let export = format!("{}\n", lines.join("\n"));
    import_codegraph_export(
        &mut store,
        &BridgeConfig {
            project_id: "project.brief",
            snapshot_id: &captured.snapshot_id,
            config_digest: &config_digest,
            run_id: "run.brief.001",
            workspace_root: Some(&root),
            max_entities_per_batch: 500,
            max_relations_per_batch: 2000,
        },
        &captured.files,
        &export,
    )
    .unwrap();
    Fixture {
        _dir: dir,
        store,
        root,
        project: "project.brief".to_string(),
        snapshot_id: captured.snapshot_id,
    }
}

#[test]
fn project_brief_reports_counts_and_next_actions() {
    let fixture = setup();
    let brief = build_brief(
        &fixture.store,
        &BriefRequest {
            project_id: &fixture.project,
            snapshot_id: Some(&fixture.snapshot_id),
            workspace: Some(&fixture.root),
            recipe: BriefRecipe::Project,
        },
    )
    .unwrap();
    assert_eq!(brief["state"], "ok");
    assert_eq!(brief["level"], "module");
    assert_eq!(brief["scope"]["snapshot_id"], fixture.snapshot_id);
    assert_eq!(brief["counts"]["candidate"], 1);
    assert_eq!(brief["counts"]["confirmed"], 0);
    assert_eq!(brief["counts"]["declared"], 0);
    assert_eq!(
        brief["counts"]["unknown"],
        brief["unknowns"].as_array().unwrap().len() as u64
    );
    assert!(brief["coverage"]["state"].is_string(), "{brief}");
    assert_eq!(brief["verification"]["state"], "not_run", "{brief}");
    assert!(brief["budget"].is_object(), "{brief}");
    assert!(brief["context"].as_array().unwrap().is_empty());
    assert!(brief["gaps"].as_array().unwrap().is_empty());
    assert!(brief["summary"].as_str().unwrap().contains("entities"));
    assert!(brief["next_actions"].as_array().unwrap().len() >= 2);
}

#[test]
fn change_brief_is_static_and_keeps_candidate_resolution() {
    let fixture = setup();
    fs::write(fixture.root.join("src/b.py"), "def b():\n    return 5\n").unwrap();
    let brief = build_brief(
        &fixture.store,
        &BriefRequest {
            project_id: &fixture.project,
            snapshot_id: Some(&fixture.snapshot_id),
            workspace: Some(&fixture.root),
            recipe: BriefRecipe::Change,
        },
    )
    .unwrap();
    assert_eq!(brief["state"], "ok");
    assert_eq!(brief["level"], "change");
    assert_eq!(brief["counts"]["stale"], 1);
    let findings = brief["findings"].as_array().unwrap();
    assert!(!findings.is_empty());
    for finding in findings {
        assert_eq!(finding["verification"], "not_run");
    }
    assert!(brief["findings"]
        .as_array()
        .unwrap()
        .iter()
        .any(|finding| finding["evidence"]
            .as_array()
            .unwrap()
            .iter()
            .any(|evidence| evidence["resolution"] == "candidate")));
    let candidate_finding = brief["findings"]
        .as_array()
        .unwrap()
        .iter()
        .find(|finding| finding["title"].as_str().unwrap_or("").contains("b changed"))
        .expect("changed entity finding");
    assert_eq!(candidate_finding["confidence"], "candidate", "{brief}");
    assert_eq!(brief["counts"]["confirmed"], 0, "{brief}");
    assert!(!brief["unknowns"]
        .as_array()
        .unwrap()
        .iter()
        .any(|unknown| unknown["question"]
            == "line overlap is static text evidence, not semantic equivalence"));
}

#[test]
fn brief_returns_needs_init_without_a_snapshot() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().join("repo");
    fs::create_dir_all(&root).unwrap();
    let state = dir.path().join("state");
    let mut store = Store::open(&state).unwrap();
    store.ensure_project("project.empty", "expedition").unwrap();
    let brief = build_brief(
        &store,
        &BriefRequest {
            project_id: "project.empty",
            snapshot_id: None,
            workspace: Some(&root),
            recipe: BriefRecipe::Auto,
        },
    )
    .unwrap();
    assert_eq!(brief["state"], "needs_init");
    assert_eq!(brief["next_actions"][0]["tool"], "archmeld_init");
}

#[test]
fn briefs_include_a_read_only_aoci_cognition_map() {
    let fixture = setup();
    fs::write(
        fixture.root.join("aoci.txt"),
        "#AOCI-ROOT-MANIFEST: 1\n\
         #Volume: id=code kind=code path=aoci.code.txt format=object-fras-v2 depends=meta state=enabled\n",
    )
    .unwrap();
    fs::write(
        fixture.root.join("aoci.code.txt"),
        "#AOCI-CODE-VOLUME: 1\n\
         ===/<repository>/src/===\n\
         a.py[CG5T]: F:Entry A | R:code:src/b.py | A:- | S:-\n\
         b.py[CG7M]: F:Entry B | R:- | A:- | S:-\n",
    )
    .unwrap();

    let project = build_brief(
        &fixture.store,
        &BriefRequest {
            project_id: &fixture.project,
            snapshot_id: Some(&fixture.snapshot_id),
            workspace: Some(&fixture.root),
            recipe: BriefRecipe::Project,
        },
    )
    .unwrap();
    assert_eq!(project["cognition"]["present"], true, "{project}");
    assert_eq!(project["cognition"]["entries_total"], 2, "{project}");
    let markdown = archmeld_core::brief::render_markdown(&project);
    assert!(
        markdown.contains("Cognition map (AOCI, optional)"),
        "{markdown}"
    );

    fs::write(
        fixture.root.join("src/a.py"),
        "def a():\n    return b() + 1\n",
    )
    .unwrap();
    let change = build_brief(
        &fixture.store,
        &BriefRequest {
            project_id: &fixture.project,
            snapshot_id: Some(&fixture.snapshot_id),
            workspace: Some(&fixture.root),
            recipe: BriefRecipe::Change,
        },
    )
    .unwrap();
    assert_eq!(change["cognition"]["present"], true, "{change}");
    let affected = change["cognition"]["affected"].as_array().unwrap();
    assert!(
        affected.iter().any(|entry| entry["path"] == "src/a.py"),
        "{change}"
    );
}

#[test]
fn map_recipe_and_cognition_map_are_read_only() {
    let fixture = setup();
    fs::write(
        fixture.root.join("archmeld.map.txt"),
        format!(
            "#ARCHMELD-MAP: 1\n#snapshot: {}\n===src/===\n\
             a.py[L1D2C5M]: F:Entry A | R:file:src/b.py | A:- | S:-\n\
             b.py: F:Entry B | R:- | A:- | S:-\n",
            fixture.snapshot_id
        ),
    )
    .unwrap();

    let map = build_brief(
        &fixture.store,
        &BriefRequest {
            project_id: &fixture.project,
            snapshot_id: Some(&fixture.snapshot_id),
            workspace: Some(&fixture.root),
            recipe: BriefRecipe::Map,
        },
    )
    .unwrap();
    assert_eq!(map["level"], "map", "{map}");
    assert_eq!(map["state"], "ok", "{map}");
    assert_eq!(map["map_state"], "ok", "{map}");
    assert_eq!(map["map"]["state"], "ok", "{map}");
    assert_eq!(map["semantic_claims"], "unverified", "{map}");
    assert_eq!(map["map"]["entries"]["total"], 2, "{map}");
    assert_eq!(map["map"]["freshness"], "latest", "{map}");
    assert!(
        map["map"]["map_digest"]
            .as_str()
            .unwrap()
            .starts_with("sha256:"),
        "{map}"
    );

    let project = build_brief(
        &fixture.store,
        &BriefRequest {
            project_id: &fixture.project,
            snapshot_id: Some(&fixture.snapshot_id),
            workspace: Some(&fixture.root),
            recipe: BriefRecipe::Project,
        },
    )
    .unwrap();
    assert_eq!(project["cognition_map"]["present"], true, "{project}");
    assert_eq!(project["cognition_map"]["entries"], 2, "{project}");
    assert_eq!(
        project["cognition_map"]["semantic_claims"], "unverified",
        "{project}"
    );
    assert_eq!(
        project["cognition_map"]["validation"], "not_run",
        "{project}"
    );
    assert_eq!(project["cognition_map"]["freshness"], "latest", "{project}");

    fs::write(
        fixture.root.join("src/a.py"),
        "def a():\n    return b() + 1\n",
    )
    .unwrap();
    let change = build_brief(
        &fixture.store,
        &BriefRequest {
            project_id: &fixture.project,
            snapshot_id: Some(&fixture.snapshot_id),
            workspace: Some(&fixture.root),
            recipe: BriefRecipe::Change,
        },
    )
    .unwrap();
    assert_eq!(change["cognition_map"]["present"], true, "{change}");
    assert_eq!(change["cognition_map"]["state"], "ok", "{change}");
    assert_eq!(change["cognition_map"]["snapshot_match"], true, "{change}");
    assert!(
        change["cognition_map"]["map_digest"]
            .as_str()
            .unwrap()
            .starts_with("sha256:"),
        "{change}"
    );
    assert!(
        change["cognition_map"]["affected"]
            .as_array()
            .unwrap()
            .iter()
            .any(|entry| entry["path"] == "src/a.py"),
        "{change}"
    );
}

#[test]
fn a_map_brief_keeps_brief_state_separate_from_map_health() {
    let fixture = setup();
    fs::write(
        fixture.root.join("archmeld.map.txt"),
        format!(
            "#ARCHMELD-MAP: 1\n#snapshot: {}\n===src/===\n\
             a.py: F:Entry A | R:entity:function:src/a.py::missing | A:- | S:-\n\
             b.py: F:Entry B | R:- | A:- | S:-\n",
            fixture.snapshot_id
        ),
    )
    .unwrap();
    let brief = build_brief(
        &fixture.store,
        &BriefRequest {
            project_id: &fixture.project,
            snapshot_id: Some(&fixture.snapshot_id),
            workspace: Some(&fixture.root),
            recipe: BriefRecipe::Map,
        },
    )
    .unwrap();
    // The brief itself was produced; the map is still invalid. The two states
    // must not collapse into one "ok".
    assert_eq!(brief["state"], "ok", "{brief}");
    assert_eq!(brief["map_state"], "invalid", "{brief}");
    assert_eq!(brief["map"]["state"], "invalid", "{brief}");
    assert_eq!(brief["semantic_claims"], "unverified", "{brief}");
    // Invalid maps produce findings; an empty findings array with a non-zero
    // count would make the finding unreachable.
    assert_eq!(brief["counts"]["findings"], 1, "{brief}");
    let findings = brief["findings"].as_array().unwrap();
    assert_eq!(findings.len(), 1, "{brief}");
    assert_eq!(findings[0]["category"], "map-invalid", "{brief}");
    assert_eq!(findings[0]["confidence"], "confirmed", "{brief}");
    assert!(brief["coverage"].is_object(), "{brief}");
    assert_eq!(brief["verification"]["state"], "not_run", "{brief}");
}

#[test]
fn map_brief_applies_finding_and_gap_budgets() {
    let fixture = setup();
    fs::write(
        fixture.root.join("archmeld.map.txt"),
        format!(
            "#ARCHMELD-MAP: 1\n#snapshot: {}\n===src/===\n\
             a.py: F:Entry A | R:entity:function:src/a.py::missing_a | A:- | S:-\n\
             b.py: F:Entry B | R:entity:function:src/b.py::missing_b | A:- | S:-\n",
            fixture.snapshot_id
        ),
    )
    .unwrap();
    let brief = build_brief_with_budget(
        &fixture.store,
        &BriefRequest {
            project_id: &fixture.project,
            snapshot_id: Some(&fixture.snapshot_id),
            workspace: Some(&fixture.root),
            recipe: BriefRecipe::Map,
        },
        BriefBudget {
            max_findings: 1,
            max_context: 2,
            max_gaps: 1,
            max_relation_items: 2,
        },
    )
    .unwrap();
    assert_eq!(brief["map_state"], "invalid", "{brief}");
    assert_eq!(brief["counts"]["findings"], 2, "{brief}");
    assert_eq!(brief["findings"].as_array().unwrap().len(), 1, "{brief}");
    assert_eq!(brief["budget"]["omitted_findings"], 1, "{brief}");
    assert_eq!(brief["budget"]["truncated"], true, "{brief}");
    assert_eq!(brief["map"]["errors"].as_array().unwrap().len(), 1, "{brief}");
    assert_eq!(brief["map"]["errors_total"], 2, "{brief}");
    assert_eq!(brief["map"]["truncated"], true, "{brief}");
    // Truncation is an open question, never a silent omission.
    assert!(
        brief["unknowns"]
            .as_array()
            .unwrap()
            .iter()
            .any(|unknown| unknown["question"] == "Are there more map errors?"),
        "{brief}"
    );
}

#[test]
fn auto_brief_reviews_the_workspace_once_and_reports_timings() {
    let fixture = setup();
    let brief = build_brief(
        &fixture.store,
        &BriefRequest {
            project_id: &fixture.project,
            snapshot_id: Some(&fixture.snapshot_id),
            workspace: Some(&fixture.root),
            recipe: BriefRecipe::Auto,
        },
    )
    .unwrap();
    assert_eq!(brief["timings"]["review_calls"], 1, "{brief}");
    assert!(
        brief["timings"]["review_ms"].as_u64().is_some(),
        "review_ms must be recorded: {brief}"
    );
    // The auto recipe falls back to the project shape; it must still carry the
    // same coverage/verification/budget fields as every other recipe.
    assert!(brief["coverage"].is_object(), "{brief}");
    assert_eq!(brief["verification"]["state"], "not_run", "{brief}");
    assert!(brief["budget"].is_object(), "{brief}");
    assert!(brief["context"].as_array().unwrap().is_empty(), "{brief}");
    assert!(brief["gaps"].as_array().unwrap().is_empty(), "{brief}");
    // The auto recipe must not report a stale change after its single review.
    if brief["state"] == "ok" {
        assert_eq!(brief["counts"]["stale"], 0, "{brief}");
    }
}

#[test]
fn change_brief_separates_changed_findings_from_unchanged_callers() {
    let fixture = setup();
    fs::write(fixture.root.join("src/b.py"), "def b():\n    return 5\n").unwrap();
    let brief = build_brief(
        &fixture.store,
        &BriefRequest {
            project_id: &fixture.project,
            snapshot_id: Some(&fixture.snapshot_id),
            workspace: Some(&fixture.root),
            recipe: BriefRecipe::Change,
        },
    )
    .unwrap();
    let findings = brief["findings"].as_array().unwrap();
    assert_eq!(findings.len(), 1, "{brief}");
    assert!(
        findings
            .iter()
            .all(|finding| !finding["title"].as_str().unwrap_or("").contains("context_only")),
        "{brief}"
    );
    assert_eq!(brief["counts"]["findings"], 1);
    let context = brief["context"].as_array().unwrap();
    assert!(
        context
            .iter()
            .any(|entry| entry["entity"]["qualified_name"]
                .as_str()
                .unwrap_or("")
                .ends_with("::a")),
        "{brief}"
    );
    assert!(
        context
            .iter()
            .any(|entry| entry["relation"]["resolution"] == "candidate"),
        "{brief}"
    );
    assert_eq!(
        brief["findings"][0]["next_action"]["args"]["snapshot_id"],
        fixture.snapshot_id
    );
}

#[test]
fn change_brief_reports_unmapped_files_as_named_gaps() {
    let fixture = setup();
    fs::write(fixture.root.join("handoff.md"), "# handoff\n").unwrap();
    let brief = build_brief(
        &fixture.store,
        &BriefRequest {
            project_id: &fixture.project,
            snapshot_id: Some(&fixture.snapshot_id),
            workspace: Some(&fixture.root),
            recipe: BriefRecipe::Change,
        },
    )
    .unwrap();
    let gaps = brief["gaps"].as_array().unwrap();
    assert!(
        gaps.iter().any(|gap| gap["path"] == "handoff.md"
            && gap["kind"] == "unmapped"),
        "{brief}"
    );
    assert!(brief["coverage"].is_object(), "{brief}");
    assert_eq!(brief["verification"]["state"], "not_run", "{brief}");
}

#[test]
fn change_brief_budget_reports_omitted_items_without_losing_coverage() {
    let fixture = setup();
    fs::write(fixture.root.join("src/a.py"), "def a():\n    return b() + 1\n").unwrap();
    fs::write(fixture.root.join("src/b.py"), "def b():\n    return 5\n").unwrap();
    let brief = build_brief_with_budget(
        &fixture.store,
        &BriefRequest {
            project_id: &fixture.project,
            snapshot_id: Some(&fixture.snapshot_id),
            workspace: Some(&fixture.root),
            recipe: BriefRecipe::Change,
        },
        BriefBudget {
            max_findings: 1,
            max_context: 1,
            max_gaps: 1,
            max_relation_items: 1,
        },
    )
    .unwrap();
    assert_eq!(brief["findings"].as_array().unwrap().len(), 1, "{brief}");
    assert_eq!(brief["counts"]["findings"], 2, "{brief}");
    assert_eq!(brief["budget"]["omitted_findings"], 1, "{brief}");
    assert_eq!(brief["budget"]["truncated"], true, "{brief}");
    assert!(brief["coverage"].is_object(), "{brief}");
    assert_eq!(brief["verification"]["state"], "not_run", "{brief}");
    assert_eq!(
        brief["findings"][0]["next_action"]["args"]["snapshot_id"],
        fixture.snapshot_id
    );
}

#[test]
fn project_brief_uses_the_requested_snapshot_not_the_latest_one() {
    let mut fixture = setup();
    fs::write(fixture.root.join("src/a.py"), "def a():\n    return b() + 1\n").unwrap();
    let second = capture_snapshot(
        &mut fixture.store,
        &fixture.project,
        &fixture.root,
        &SnapshotInputs {
            build_config: json!({"variant": "brief-second"}),
            environment: json!({"os": "test"}),
        },
        &SnapshotOptions {
            include_untracked: true,
            ..SnapshotOptions::default()
        },
    )
    .unwrap();
    assert_ne!(second.snapshot_id, fixture.snapshot_id);
    let brief = build_brief(
        &fixture.store,
        &BriefRequest {
            project_id: &fixture.project,
            snapshot_id: Some(&fixture.snapshot_id),
            workspace: Some(&fixture.root),
            recipe: BriefRecipe::Project,
        },
    )
    .unwrap();
    assert_eq!(brief["scope"]["snapshot_id"], fixture.snapshot_id);
    assert_ne!(brief["scope"]["snapshot_id"], second.snapshot_id);
    assert!(
        brief["summary"].as_str().unwrap_or("").contains("2 entities"),
        "{brief}"
    );
}

#[test]
fn project_brief_rejects_a_snapshot_from_another_project() {
    let mut fixture = setup();
    fixture
        .store
        .ensure_project("project.brief.other", "expedition")
        .unwrap();
    let other = capture_snapshot(
        &mut fixture.store,
        "project.brief.other",
        &fixture.root,
        &SnapshotInputs {
            build_config: json!({"variant": "other"}),
            environment: json!({"os": "test"}),
        },
        &SnapshotOptions {
            include_untracked: true,
            ..SnapshotOptions::default()
        },
    )
    .unwrap();
    let result = build_brief(
        &fixture.store,
        &BriefRequest {
            project_id: &fixture.project,
            snapshot_id: Some(&other.snapshot_id),
            workspace: Some(&fixture.root),
            recipe: BriefRecipe::Project,
        },
    );
    assert!(result.is_err(), "foreign snapshot must be rejected: {result:?}");
}

#[test]
fn relation_budget_does_not_change_severity_or_context() {
    let fixture = setup();
    fs::write(fixture.root.join("src/b.py"), "def b():\n    return 5\n").unwrap();
    let brief = build_brief_with_budget(
        &fixture.store,
        &BriefRequest {
            project_id: &fixture.project,
            snapshot_id: Some(&fixture.snapshot_id),
            workspace: Some(&fixture.root),
            recipe: BriefRecipe::Change,
        },
        BriefBudget {
            max_findings: 10,
            max_context: 10,
            max_gaps: 10,
            max_relation_items: 0,
        },
    )
    .unwrap();
    assert_eq!(brief["relations"]["candidate"], 1, "{brief}");
    assert_eq!(brief["findings"][0]["severity"], "info", "{brief}");
    assert!(
        brief["context"]
            .as_array()
            .unwrap()
            .iter()
            .any(|entry| entry["entity"]["qualified_name"]
                .as_str()
                .unwrap_or("")
                .ends_with("::a")),
        "context must not depend on the relation evidence budget: {brief}"
    );
}

#[test]
fn change_brief_reports_omitted_base_files_as_gaps_not_deletions() {
    let fixture = setup();
    fs::write(fixture.root.join("src/a.py"), [0u8, 1, 2, 3]).unwrap();
    let brief = build_brief(
        &fixture.store,
        &BriefRequest {
            project_id: &fixture.project,
            snapshot_id: Some(&fixture.snapshot_id),
            workspace: Some(&fixture.root),
            recipe: BriefRecipe::Change,
        },
    )
    .unwrap();
    assert!(
        brief["gaps"]
            .as_array()
            .unwrap()
            .iter()
            .any(|gap| gap["path"] == "src/a.py" && gap["kind"] == "omitted"),
        "{brief}"
    );
    assert!(
        brief["findings"]
            .as_array()
            .unwrap()
            .iter()
            .all(|finding| !finding["title"].as_str().unwrap_or("").contains("deleted")),
        "{brief}"
    );
}
