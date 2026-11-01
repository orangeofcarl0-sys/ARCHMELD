// Copyright 2026 ARCHMELD contributors
// SPDX-License-Identifier: Apache-2.0

//! B025 / AT056: drift makes affected decisions/plans reviewable without
//! deleting history; a reopened ledger still serves the old snapshot and the
//! prior decision. Backups are consistent and GC is pin-aware.

mod common;

use archmeld_core::bridge::{import_codegraph_export, BridgeConfig};
use archmeld_core::drift::{
    analyze_drift, create_backup, gc, list_drift_reports, pin, record_drift, unpin,
};
use archmeld_core::interpret::{get_claim, propose_claim, review_claim};
use archmeld_core::retention::{configure_retention, list_retention_runs, prune_snapshots};
use archmeld_core::snapshot::{capture_snapshot, SnapshotInputs, SnapshotOptions};
use archmeld_core::store::Store;
use serde_json::{json, Value};
use std::fs;
use std::path::Path;
use std::sync::Mutex;

static EVIDENCE_LOCK: Mutex<()> = Mutex::new(());

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
    let node = |id: i64, path: &str, name: &str, line: i64| {
        json!({"type":"node","id":id,"node_type":"Function","properties":{
            "path": fixture.root.join(path).to_string_lossy(),
            "name": name,
            "line_start": line,
            "line_end": line,
        }})
    };
    let nodes = [node(1, "src/a.py", "a", 1), node(2, "src/b.py", "b", 1)];
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
        &mut fixture.store,
        &BridgeConfig {
            project_id: &fixture.project,
            snapshot_id: &fixture.snapshot_id,
            config_digest: &fixture.config_digest,
            run_id: "run.drift",
            workspace_root: Some(&fixture.root),
            max_entities_per_batch: 500,
            max_relations_per_batch: 2000,
        },
        &captured.files,
        &export,
    )
    .unwrap();
    let entity_id = fixture
        .store
        .entity_ids(&fixture.project, &fixture.snapshot_id)
        .unwrap()
        .into_iter()
        .find(|id| {
            fixture
                .store
                .get_entity(&fixture.project, &fixture.snapshot_id, id)
                .unwrap()
                .and_then(|entity| entity["qualified_name"].as_str().map(str::to_string))
                .is_some_and(|name| name.ends_with("::a"))
        })
        .unwrap();
    (entity_id, "evidence.codegraph.export.run.drift".to_string())
}

#[test]
fn drift_is_reported_and_old_snapshot_and_decisions_survive_restart() {
    let mut fixture = common::setup("b025-drift");
    let (entity_id, evidence_id) = import_graph(&mut fixture);
    let claim = json!({
        "id": "claim.b025.interface",
        "project_id": fixture.project,
        "snapshot_id": fixture.snapshot_id,
        "claim_type": "decision",
        "text": "a.py exposes the stable helper interface.",
        "status": "proposed",
        "support_ids": [evidence_id],
        "counterevidence_ids": [],
        "scope": "src/a.py",
        "producer": {"name":"human","version":"manual-1","source_ref":"note","run_id":"run.drift"},
        "unresolved_questions": [],
    });
    let proposed = propose_claim(
        &mut fixture.store,
        &json!({
            "project_id": fixture.project,
            "claim": claim,
            "expected_revision": 1,
            "idempotency_key": "claim-drift-1",
        }),
    )
    .unwrap();
    let claim_id = proposed["id"].as_str().unwrap().to_string();
    review_claim(
        &mut fixture.store,
        &fixture.project,
        &claim_id,
        1,
        "accepted",
        "the interface is used by b",
        &json!({"name": "operator"}),
        &[evidence_id.clone()],
        &[],
        "review-drift-1",
    )
    .unwrap();
    common::approved_plan(&mut fixture, "plan.b025.drift", &["src/a.py"]);

    // Back up while decisions are intact and verify the backup reopens.
    let backup_dir = fixture._dir.path().join("backup");
    let backup = create_backup(&mut fixture.store, &fixture.project, &backup_dir).unwrap();
    assert_eq!(backup["verified"], true);
    let backup_store = Store::open(&backup_dir).unwrap();
    assert_eq!(
        get_claim(&backup_store, &fixture.project, &claim_id).unwrap()["status"],
        "accepted"
    );
    assert!(backup_store
        .get_entity(&fixture.project, &fixture.snapshot_id, &entity_id)
        .unwrap()
        .is_some());

    // The public interface changes under the base snapshot.
    common::write_file(
        &fixture.root,
        "src/a.py",
        "def a1(renamed: int):\n    return b()\n",
    );
    let report = analyze_drift(
        &fixture.store,
        &fixture.project,
        &fixture.snapshot_id,
        &fixture.root,
        None,
    )
    .unwrap();
    assert_eq!(report["state"], "changed");
    assert_eq!(report["needs_review"], true);
    assert!(report["changed"]
        .as_array()
        .unwrap()
        .iter()
        .any(|item| item["path"] == "src/a.py"));
    assert!(report["affected_entities"]
        .as_array()
        .unwrap()
        .iter()
        .any(|item| item["entity_id"] == entity_id));
    assert!(!report["affected_relations"].as_array().unwrap().is_empty());
    assert!(report["affected_claims"]
        .as_array()
        .unwrap()
        .iter()
        .any(|item| item["claim_id"] == claim_id));
    assert!(report["stale_plans"]
        .as_array()
        .unwrap()
        .iter()
        .any(|item| item["plan_id"] == "plan.b025.drift"));
    assert_eq!(report["source_mutation"], "none");
    assert_eq!(report["history_mutation"], "none");

    let new_snapshot = capture_snapshot(
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
    assert_ne!(new_snapshot.snapshot_id, fixture.snapshot_id);
    let recorded = record_drift(
        &mut fixture.store,
        &json!({
            "project_id": fixture.project,
            "base_snapshot_id": fixture.snapshot_id,
            "workspace_handle": fixture.handle,
            "new_snapshot_id": new_snapshot.snapshot_id,
            "idempotency_key": "drift-record-1",
        }),
    )
    .unwrap();
    assert_eq!(recorded["summary"]["state"], "changed");

    // Restart: close the store and reopen the same ledger.
    let project = fixture.project.clone();
    let base_snapshot = fixture.snapshot_id.clone();
    let state_dir = fixture._dir.path().join("state");
    drop(fixture.store);
    let mut store = Store::open(&state_dir).unwrap();
    let old_entity = store
        .get_entity(&project, &base_snapshot, &entity_id)
        .unwrap()
        .expect("old snapshot entity survives");
    assert_eq!(old_entity["id"], entity_id);
    let old_claim = get_claim(&store, &project, &claim_id).unwrap();
    assert_eq!(old_claim["status"], "accepted");
    assert_eq!(old_claim["support_ids"][0], evidence_id);
    let reports = list_drift_reports(&store, &project).unwrap();
    assert!(!reports.is_empty());
    assert_eq!(
        reports[0]["summary"]["new_snapshot_id"],
        new_snapshot.snapshot_id
    );
    assert!(store
        .latest_snapshot_id(&project)
        .unwrap()
        .is_some_and(|latest| latest == new_snapshot.snapshot_id));

    // GC: an unreferenced blob is a candidate, a pin protects it, and the
    // snapshot manifest stays referenced.
    let orphan = store
        .put_blob(b"orphan-object-b025", "application/octet-stream")
        .unwrap();
    let dry = gc(&mut store, true).unwrap();
    assert!(dry["candidates"]
        .as_array()
        .unwrap()
        .iter()
        .any(|item| item["digest"] == orphan));
    let manifest = store
        .snapshot_manifest_digest(&project, &base_snapshot)
        .unwrap()
        .unwrap();
    assert!(!dry["candidates"]
        .as_array()
        .unwrap()
        .iter()
        .any(|item| item["digest"] == manifest));
    pin(
        &mut store,
        &json!({"project_id": project, "pin_id": "pin.orphan", "kind": "object", "target": orphan}),
    )
    .unwrap();
    let pinned = gc(&mut store, true).unwrap();
    assert!(!pinned["candidates"]
        .as_array()
        .unwrap()
        .iter()
        .any(|item| item["digest"] == orphan));
    unpin(&mut store, &project, "pin.orphan").unwrap();
    let swept = gc(&mut store, false).unwrap();
    assert!(swept["candidates"]
        .as_array()
        .unwrap()
        .iter()
        .any(|item| item["digest"] == orphan));
    assert!(!store.blob_exists(&orphan).unwrap());
    assert!(store.integrity_check().unwrap().is_empty());
    write_evidence(
        &report,
        &recorded,
        &backup,
        &swept,
        &project,
        &base_snapshot,
        &entity_id,
        &claim_id,
    );
}

fn write_evidence(
    report: &Value,
    recorded: &Value,
    backup: &Value,
    swept: &Value,
    project: &str,
    base_snapshot: &str,
    entity_id: &str,
    claim_id: &str,
) {
    if std::env::var("ARCHMELD_WRITE_EVIDENCE").ok().as_deref() != Some("1") {
        return;
    }
    let _guard = EVIDENCE_LOCK.lock().unwrap();
    let path =
        Path::new(env!("CARGO_MANIFEST_DIR")).join("../../docs/evidence/B025-drift-recovery.json");
    let mut root = if path.exists() {
        serde_json::from_str::<Value>(&fs::read_to_string(&path).unwrap())
            .unwrap_or_else(|_| json!({}))
    } else {
        json!({})
    };
    root["generated_by"] = json!("crates/archmeld-core/tests/drift_recovery.rs");
    root["platform"] = json!(std::env::consts::OS);
    root["acceptance"]["AT056"] = json!({
        "status": "passed",
        "base_snapshot_id": base_snapshot,
        "new_snapshot_id": recorded["summary"]["new_snapshot_id"],
        "drift": report,
        "recorded_report_id": recorded["report_id"],
        "old_snapshot_entity_still_queryable": entity_id,
        "claim_decision_preserved": claim_id,
        "reopened_ledger": true,
        "backup": backup,
        "gc": swept,
        "project_id": project,
    });
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(path, serde_json::to_string_pretty(&root).unwrap() + "
").unwrap();
}

fn capture_after_change(
    fixture: &mut common::Fixture,
    content: &str,
) -> (String, Option<Value>) {
    fs::write(fixture.root.join("src/a.py"), content).unwrap();
    std::thread::sleep(std::time::Duration::from_millis(1100));
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
    (captured.snapshot_id, captured.retention)
}

#[test]
fn count_based_retention_preserves_referenced_history_and_honors_pins() {
    let mut fixture = common::setup("b025-retention");
    let initial = fixture.snapshot_id.clone();
    let status = configure_retention(
        &mut fixture.store,
        &json!({
            "project_id": fixture.project,
            "keep_snapshots": 2,
            "auto_prune": false,
        }),
    )
    .unwrap();
    assert_eq!(status["keep_snapshots"], 2);
    assert_eq!(status["auto_prune"], false);

    let (s1, _) = capture_after_change(&mut fixture, "VALUE = 11\n");
    let (s2, _) = capture_after_change(&mut fixture, "VALUE = 12\n");
    let (s3, _) = capture_after_change(&mut fixture, "VALUE = 13\n");
    assert_ne!(s1, s2);
    assert_ne!(s2, s3);

    let dry_run = prune_snapshots(
        &mut fixture.store,
        &json!({
            "project_id": fixture.project,
            "keep_snapshots": 2,
            "dry_run": true,
        }),
    )
    .unwrap();
    assert_eq!(dry_run["kept_count"], 2);
    assert_eq!(dry_run["candidate_count"], 1);
    assert_eq!(dry_run["deleted_count"], 0);
    assert_eq!(dry_run["candidates"][0]["snapshot_id"], s1);
    let retained = dry_run["retained"].as_array().unwrap();
    assert_eq!(retained.len(), 1);
    assert_eq!(retained[0]["snapshot_id"], initial);
    let reasons: Vec<&str> = retained[0]["reasons"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(Value::as_str)
        .collect();
    assert!(
        reasons.contains(&"plan") || reasons.contains(&"architecture"),
        "{reasons:?}"
    );
    assert!(fixture
        .store
        .snapshot_payload(&fixture.project, &s1)
        .unwrap()
        .is_some());

    let applied = prune_snapshots(
        &mut fixture.store,
        &json!({
            "project_id": fixture.project,
            "keep_snapshots": 2,
            "dry_run": false,
        }),
    )
    .unwrap();
    assert_eq!(applied["deleted_count"], 1);
    assert!(fixture
        .store
        .snapshot_payload(&fixture.project, &s1)
        .unwrap()
        .is_none());
    assert!(fixture
        .store
        .snapshot_payload(&fixture.project, &initial)
        .unwrap()
        .is_some());
    assert!(fixture
        .store
        .snapshot_payload(&fixture.project, &s3)
        .unwrap()
        .is_some());

    // A pin wins over the count policy even when the snapshot is old.
    pin(
        &mut fixture.store,
        &json!({
            "project_id": fixture.project,
            "pin_id": "pin.s2",
            "kind": "snapshot",
            "target": s2,
        }),
    )
    .unwrap();
    configure_retention(
        &mut fixture.store,
        &json!({
            "project_id": fixture.project,
            "keep_snapshots": 2,
            "auto_prune": true,
        }),
    )
    .unwrap();
    let (s4, retention) = capture_after_change(&mut fixture, "VALUE = 14\n");
    let retention = retention.expect("auto_prune was enabled");
    assert_eq!(retention["mode"], "automatic");
    assert_eq!(retention["deleted_count"], 0);
    let pinned_retained = retention["retained"]
        .as_array()
        .unwrap()
        .iter()
        .find(|item| item["snapshot_id"] == s2)
        .unwrap();
    assert!(pinned_retained["reasons"]
        .as_array()
        .unwrap()
        .iter()
        .any(|reason| reason == "pinned"));
    assert!(fixture
        .store
        .snapshot_payload(&fixture.project, &s2)
        .unwrap()
        .is_some());

    // After unpinning, the next capture prunes both unreferenced old
    // snapshots automatically; the architecture-referenced snapshot stays.
    unpin(&mut fixture.store, &fixture.project, "pin.s2").unwrap();
    let (s5, retention) = capture_after_change(&mut fixture, "VALUE = 15\n");
    let retention = retention.expect("auto_prune is still enabled");
    assert_eq!(retention["mode"], "automatic");
    assert_eq!(retention["deleted_count"], 2);
    for old in [&s2, &s3] {
        assert!(
            fixture
                .store
                .snapshot_payload(&fixture.project, old)
                .unwrap()
                .is_none(),
            "{old} should be pruned"
        );
    }
    for kept in [&initial, &s4, &s5] {
        assert!(
            fixture
                .store
                .snapshot_payload(&fixture.project, kept)
                .unwrap()
                .is_some(),
            "{kept} should be retained"
        );
    }
    let runs = list_retention_runs(&fixture.store, &fixture.project).unwrap();
    assert_eq!(runs.len(), 3);
    assert_eq!(runs[0]["mode"], "automatic");
    assert_eq!(runs[0]["deleted_count"], 2);
    assert_eq!(runs[1]["mode"], "automatic");
    assert_eq!(runs[1]["deleted_count"], 0);
    assert_eq!(runs[2]["mode"], "manual");
    assert_eq!(runs[2]["deleted_count"], 1);
    assert!(fixture.store.integrity_check().unwrap().is_empty());

    write_retention_evidence(
        &fixture,
        json!({
            "initial_snapshot_id": initial,
            "kept_latest_two": [s5, s4],
            "dry_run": dry_run,
            "manual_delete": applied,
            "pinned_auto_run": retention,
            "runs": runs,
            "project_id": fixture.project,
        }),
    );
}

fn write_retention_evidence(fixture: &common::Fixture, case: Value) {
    if std::env::var("ARCHMELD_WRITE_EVIDENCE").ok().as_deref() != Some("1") {
        return;
    }
    let _guard = EVIDENCE_LOCK.lock().unwrap();
    let path =
        Path::new(env!("CARGO_MANIFEST_DIR")).join("../../docs/evidence/B025-drift-recovery.json");
    let mut root = if path.exists() {
        serde_json::from_str::<Value>(&fs::read_to_string(&path).unwrap())
            .unwrap_or_else(|_| json!({}))
    } else {
        json!({})
    };
    root["generated_by"] = json!("crates/archmeld-core/tests/drift_recovery.rs");
    root["platform"] = json!(std::env::consts::OS);
    root["acceptance"]["AT057"] = json!({
        "status": "passed",
        "fixture": fixture.project,
        "retention": case,
    });
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(path, serde_json::to_string_pretty(&root).unwrap() + "\n").unwrap();
}
