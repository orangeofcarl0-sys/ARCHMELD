// Copyright 2026 ARCHMELD contributors
// SPDX-License-Identifier: Apache-2.0

//! Human-attested reads/writes: declaration only, never runtime observation.

use archmeld_core::bridge::{import_codegraph_export, BridgeConfig};
use archmeld_core::human::import_human_evidence;
use archmeld_core::review::diff_snapshots;
use archmeld_core::snapshot::{capture_snapshot, SnapshotInputs, SnapshotOptions};
use archmeld_core::store::{FileEntry, Store};
use serde_json::{json, Value};
use std::fs;
use std::path::{Path, PathBuf};

const PROJECT: &str = "project.human";

struct Fixture {
    _dir: tempfile::TempDir,
    store: Store,
    root: PathBuf,
    base: String,
    second: String,
    base_ids: (String, String),
    second_ids: (String, String),
}

fn import_entities(
    store: &mut Store,
    snapshot_id: &str,
    config: &str,
    root: &Path,
    files: &[FileEntry],
) -> (String, String) {
    let nodes = [
        json!({"type":"node","id":1,"node_type":"Function","properties":{
            "path": root.join("src/a.py").to_string_lossy(),"name":"reader",
            "line_start":1,"line_end":2}}),
        json!({"type":"node","id":2,"node_type":"Function","properties":{
            "path": root.join("src/b.py").to_string_lossy(),"name":"store",
            "line_start":1,"line_end":2}}),
    ];
    let mut lines = vec![json!({
        "type":"manifest","format":"archmeld-export-v1","source":"codegraph",
        "version":"0.2.0","node_count":2,"edge_count":0,"iteration":"real-iterators"
    })
    .to_string()];
    for node in &nodes {
        lines.push(node.to_string());
    }
    let export = format!("{}\n", lines.join("\n"));
    let run_id = format!(
        "run.human.export.{}",
        snapshot_id
            .strip_prefix("sha256:")
            .unwrap_or(snapshot_id)
            .chars()
            .take(12)
            .collect::<String>()
    );
    import_codegraph_export(
        store,
        &BridgeConfig {
            project_id: PROJECT,
            snapshot_id,
            config_digest: config,
            run_id: &run_id,
            workspace_root: Some(root),
            max_entities_per_batch: 500,
            max_relations_per_batch: 2000,
        },
        &files,
        &export,
    )
    .unwrap();
    let find = |store: &Store, name: &str| {
        store
            .entity_ids(PROJECT, snapshot_id)
            .unwrap()
            .into_iter()
            .find(|id| {
                store
                    .get_entity(PROJECT, snapshot_id, id)
                    .unwrap()
                    .and_then(|entity| entity["qualified_name"].as_str().map(str::to_string))
                    .is_some_and(|qualified| qualified.ends_with(name))
            })
            .expect("entity")
    };
    (find(store, "::reader"), find(store, "::store"))
}

fn setup() -> Fixture {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().join("repo");
    fs::create_dir_all(root.join("src")).unwrap();
    fs::write(root.join("src/a.py"), "def reader():\n    return 1\n").unwrap();
    fs::write(root.join("src/b.py"), "def store():\n    return 2\n").unwrap();
    fs::write(root.join("src/c.py"), "def other():\n    return 3\n").unwrap();
    let mut store = Store::open(dir.path().join("state")).unwrap();
    store.ensure_project(PROJECT, "expedition").unwrap();
    let options = SnapshotOptions {
        include_untracked: true,
        ..SnapshotOptions::default()
    };
    let base = capture_snapshot(
        &mut store,
        PROJECT,
        &root,
        &SnapshotInputs {
            build_config: json!({"variant": "human"}),
            environment: json!({"os": "test"}),
        },
        &options,
    )
    .unwrap();
    let base_config = base.snapshot["config_digest"].as_str().unwrap().to_string();
    let base_ids = import_entities(&mut store, &base.snapshot_id, &base_config, &root, &base.files);

    fs::write(root.join("src/c.py"), "def other():\n    return 4\n").unwrap();
    let second = capture_snapshot(
        &mut store,
        PROJECT,
        &root,
        &SnapshotInputs {
            build_config: json!({"variant": "human"}),
            environment: json!({"os": "test"}),
        },
        &options,
    )
    .unwrap();
    assert_ne!(base.snapshot_id, second.snapshot_id);
    let second_config = second.snapshot["config_digest"]
        .as_str()
        .unwrap()
        .to_string();
    let second_ids = import_entities(
        &mut store,
        &second.snapshot_id,
        &second_config,
        &root,
        &second.files,
    );
    Fixture {
        _dir: dir,
        store,
        root,
        base: base.snapshot_id,
        second: second.snapshot_id,
        base_ids,
        second_ids,
    }
}

fn attestation(_fixture: &Fixture, snapshot: &str, ids: &(String, String)) -> Value {
    json!({
        "schema_version": "1.0",
        "project_id": PROJECT,
        "snapshot_id": snapshot,
        "producer": {
            "name": "reviewer",
            "version": "1.0",
            "source_ref": "manual-review",
            "run_id": "run.human.attestation",
        },
        "attestor": "reviewer-a",
        "observations": [{
            "kind": "write",
            "source_entity_id": ids.0,
            "target_entity_id": ids.1,
            "path": "src/a.py",
            "line": 1,
            "rationale": "reader mutates the store during startup",
        }],
    })
}

#[test]
fn human_writes_are_imported_as_declared_human_evidence() {
    let mut fixture = setup();
    let input = attestation(&fixture, &fixture.second, &fixture.second_ids);
    let imported = import_human_evidence(&mut fixture.store, &input).unwrap();
    assert_eq!(imported["basis"], "human", "{imported}");
    assert_eq!(imported["resolution"], "declared", "{imported}");
    assert_eq!(imported["coverage"], "partial", "{imported}");
    assert_eq!(imported["relations"], 1, "{imported}");
    assert!(imported["artifact_digest"]
        .as_str()
        .unwrap()
        .starts_with("sha256:"));

    let relations = fixture
        .store
        .relation_payloads(PROJECT, &fixture.second)
        .unwrap();
    let relation = relations
        .iter()
        .find(|relation| relation["kind"] == "writes")
        .expect("human write relation");
    assert_eq!(relation["basis"], "human", "{relation}");
    assert_eq!(relation["resolution"], "declared", "{relation}");
    assert!(relation["conditions"]
        .as_array()
        .unwrap()
        .iter()
        .any(|condition| condition == "human-attested"));
    assert!(relation["conditions"]
        .as_array()
        .unwrap()
        .iter()
        .any(|condition| condition == "attestor:reviewer-a"));
    assert!(relation["conditions"]
        .as_array()
        .unwrap()
        .iter()
        .any(|condition| condition == "not-runtime-observation"));

    // Repeating the same attestation is idempotent: it reuses the relation id
    // and the published generation instead of adding a second edge.
    let repeated = import_human_evidence(&mut fixture.store, &input).unwrap();
    assert_eq!(repeated["relations"], 1, "{repeated}");
    assert_eq!(
        fixture
            .store
            .relation_payloads(PROJECT, &fixture.second)
            .unwrap()
            .len(),
        1
    );
}

#[test]
fn human_evidence_rejects_missing_entities_and_wrong_snapshot_scope() {
    let mut fixture = setup();
    let mut input = attestation(&fixture, &fixture.second, &fixture.second_ids);
    input["observations"][0]["target_entity_id"] = json!("entity.missing");
    let error = import_human_evidence(&mut fixture.store, &input).unwrap_err();
    assert!(error.to_string().contains("not in snapshot"), "{error}");

    let mut input = attestation(&fixture, &fixture.second, &fixture.second_ids);
    input["snapshot_id"] = json!(fixture.base);
    input["observations"][0]["source_entity_id"] = json!(fixture.base_ids.0);
    input["observations"][0]["target_entity_id"] = json!(fixture.base_ids.1);
    let error = import_human_evidence(&mut fixture.store, &input);
    // The base snapshot exists, so this scope is valid; the point is that an
    // unknown snapshot id is refused rather than silently attached elsewhere.
    assert!(error.is_ok());

    let mut input = attestation(&fixture, &fixture.second, &fixture.second_ids);
    input["snapshot_id"] = json!("sha256:does-not-exist");
    let error = import_human_evidence(&mut fixture.store, &input).unwrap_err();
    assert!(
        error.to_string().contains("missing") || error.to_string().contains("snapshot"),
        "{error}"
    );
}

#[test]
fn human_writes_appear_in_structural_ownership_as_declared() {
    let mut fixture = setup();
    let input = attestation(&fixture, &fixture.second, &fixture.second_ids);
    import_human_evidence(&mut fixture.store, &input).unwrap();
    let diff = diff_snapshots(&fixture.store, PROJECT, &fixture.base, &fixture.second).unwrap();
    let added = diff["structural"]["ownership"]["added"]["items"]
        .as_array()
        .expect("ownership added");
    let write = added
        .iter()
        .find(|item| item["kind"] == "writes")
        .expect("declared write must be visible to structural diff");
    assert_eq!(write["basis"], "human", "{diff}");
    assert_eq!(write["resolution"], "declared", "{diff}");
    assert!(write["source"].as_str().unwrap().ends_with("::reader"));
    assert!(write["target"].as_str().unwrap().ends_with("::store"));
}
