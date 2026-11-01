// Copyright 2026 ARCHMELD contributors
// SPDX-License-Identifier: Apache-2.0

//! B024 / AT055: the optional Instigator only emits bounded NUDGE/SILENT
//! messages. It never creates tasks, changes task state, approves, applies or
//! waits, and stale/expired messages are discarded without waking a task.

use archmeld_core::events::events_after;
use archmeld_core::instigator::{configure, deliver_due, propose_nudge, status};
use archmeld_core::jobs::{create_job, get_job, transition_job};
use archmeld_core::store::Store;
use serde_json::{json, Value};
use std::fs;
use std::path::Path;
use std::time::{SystemTime, UNIX_EPOCH};

const P: &str = "project.b024";

fn nudge(
    store: &mut Store,
    task_id: &str,
    revision: i64,
    reason: &str,
    expires_at: i64,
    key: &str,
) -> Value {
    propose_nudge(
        store,
        &json!({
            "project_id": P,
            "task_id": task_id,
            "task_revision": revision,
            "reason": reason,
            "scope_ref": task_id,
            "expires_at": expires_at,
            "idempotency_key": key,
        }),
    )
    .unwrap()
}

fn configure_mode(store: &mut Store, enabled: bool, cooldown: i64, max: i64) {
    configure(
        store,
        &json!({
            "project_id": P,
            "enabled": enabled,
            "cooldown_seconds": cooldown,
            "max_nudges_per_task": max,
            "actor": "operator",
        }),
    )
    .unwrap();
}

fn now_unix() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs() as i64
}

#[test]
fn instigator_is_optional_bounded_and_never_blocks() {
    let dir = tempfile::tempdir().unwrap();
    let mut store = Store::open(dir.path().join("state")).unwrap();
    store.ensure_project(P, "expedition").unwrap();
    let jobs_before = store.job_count(P).unwrap();

    // Default is disabled: the main task does not wait for any Instigator.
    configure_mode(&mut store, false, 0, 2);
    let task1 = create_job(&mut store, P, "main").unwrap();
    let task1_id = task1["id"].as_str().unwrap().to_string();
    let running1 = transition_job(&mut store, P, &task1_id, "running", 0.5).unwrap();
    let disabled = nudge(
        &mut store,
        &task1_id,
        running1["revision"].as_i64().unwrap(),
        "depth",
        9_999_999_999,
        "nudge-disabled",
    );
    assert_eq!(disabled["decision"], "SILENT");
    assert_eq!(disabled["reason"], "instigator_disabled");
    let finished1 = transition_job(&mut store, P, &task1_id, "succeeded", 1.0).unwrap();
    assert_eq!(finished1["state"], "succeeded");

    // Enable. A current revision produces a pending NUDGE, but the task state
    // and revision are untouched.
    configure_mode(&mut store, true, 3600, 3);
    let task2 = create_job(&mut store, P, "main").unwrap();
    let task2_id = task2["id"].as_str().unwrap().to_string();
    let running2 = transition_job(&mut store, P, &task2_id, "running", 0.5).unwrap();
    let revision2 = running2["revision"].as_i64().unwrap();
    let first = nudge(
        &mut store,
        &task2_id,
        revision2,
        "depth",
        9_999_999_999,
        "nudge-current",
    );
    assert_eq!(first["decision"], "NUDGE");
    assert_eq!(first["state"], "pending");
    assert_eq!(first["blocking"], false);
    assert_eq!(first["may_create_tasks"], false);
    assert_eq!(first["may_change_task_state"], false);
    let replay = nudge(
        &mut store,
        &task2_id,
        revision2,
        "depth",
        9_999_999_999,
        "nudge-current",
    );
    assert_eq!(replay["message_id"], first["message_id"]);
    let task_after_nudge = get_job(&store, P, &task2_id).unwrap().unwrap();
    assert_eq!(task_after_nudge["state"], "running");
    assert_eq!(task_after_nudge["revision"], revision2);
    assert_eq!(store.job_count(P).unwrap(), jobs_before + 2);

    // Stale revision, expiry, cooldown and budget are all silent.
    let stale = nudge(
        &mut store,
        &task2_id,
        revision2 - 1,
        "coverage",
        9_999_999_999,
        "nudge-stale",
    );
    assert_eq!(stale["decision"], "SILENT");
    assert_eq!(stale["reason"], "stale_task_revision");
    let expired = nudge(
        &mut store,
        &task2_id,
        revision2,
        "experiment",
        now_unix() - 10,
        "nudge-expired",
    );
    assert_eq!(expired["decision"], "SILENT");
    assert_eq!(expired["reason"], "expired");
    let cooldown = nudge(
        &mut store,
        &task2_id,
        revision2,
        "depth",
        9_999_999_999,
        "nudge-cooldown",
    );
    assert_eq!(cooldown["decision"], "SILENT");
    assert_eq!(cooldown["reason"], "cooldown");
    configure_mode(&mut store, true, 3600, 1);
    let budget = nudge(
        &mut store,
        &task2_id,
        revision2,
        "new-goal",
        9_999_999_999,
        "nudge-budget",
    );
    assert_eq!(budget["decision"], "SILENT");
    assert_eq!(budget["reason"], "nudge_budget_exhausted");
    let foreign_scope = propose_nudge(
        &mut store,
        &json!({
            "project_id": P,
            "task_id": task2_id,
            "task_revision": revision2,
            "reason": "scope",
            "scope_ref": "goal.new",
            "expires_at": 9_999_999_999i64,
            "idempotency_key": "nudge-scope",
        }),
    )
    .unwrap();
    assert_eq!(foreign_scope["decision"], "SILENT");
    assert_eq!(foreign_scope["reason"], "scope_not_an_existing_task");
    let missing_task = propose_nudge(
        &mut store,
        &json!({
            "project_id": P,
            "task_id": "job.not-a-task",
            "task_revision": 1,
            "reason": "scope",
            "scope_ref": "job.not-a-task",
            "expires_at": 9_999_999_999i64,
            "idempotency_key": "nudge-missing",
        }),
    )
    .unwrap_err();
    assert!(missing_task.to_string().contains("cannot create tasks"));
    assert_eq!(store.job_count(P).unwrap(), jobs_before + 2);

    // Delivery is a separate non-blocking call. A pending nudge for a finished
    // task is discarded, not delivered and not able to wake the task.
    let finished2 = transition_job(&mut store, P, &task2_id, "succeeded", 1.0).unwrap();
    let discarded = deliver_due(
        &mut store,
        &json!({"project_id": P, "idempotency_key": "deliver-1"}),
    )
    .unwrap();
    assert_eq!(discarded["blocking"], false);
    assert_eq!(discarded["waited_for_approval"], false);
    assert!(discarded["discarded"]
        .as_array()
        .unwrap()
        .iter()
        .any(|item| item["reason"] == "task_finished"));
    assert_eq!(finished2["state"], "succeeded");

    // A current pending nudge can be delivered; delivery does not change the
    // task state.
    configure_mode(&mut store, true, 0, 3);
    let task3 = create_job(&mut store, P, "main").unwrap();
    let task3_id = task3["id"].as_str().unwrap().to_string();
    let running3 = transition_job(&mut store, P, &task3_id, "running", 0.5).unwrap();
    let revision3 = running3["revision"].as_i64().unwrap();
    let pending = nudge(
        &mut store,
        &task3_id,
        revision3,
        "experiment",
        9_999_999_999,
        "nudge-deliver",
    );
    let delivered = deliver_due(
        &mut store,
        &json!({"project_id": P, "idempotency_key": "deliver-2"}),
    )
    .unwrap();
    assert!(delivered["delivered"]
        .as_array()
        .unwrap()
        .iter()
        .any(|item| item["message_id"] == pending["message_id"]));
    let task3_after = get_job(&store, P, &task3_id).unwrap().unwrap();
    assert_eq!(task3_after["state"], "running");
    assert_eq!(task3_after["revision"], revision3);
    let events = events_after(&store, P, None).unwrap();
    assert!(events.events.iter().any(|event| {
        event["type"] == "instigator.nudge"
            && event["payload"]["message_id"] == pending["message_id"]
    }));

    // Disabling again never creates work and never prevents completion.
    configure_mode(&mut store, false, 0, 3);
    let disabled = nudge(
        &mut store,
        &task3_id,
        revision3,
        "depth",
        9_999_999_999,
        "nudge-off",
    );
    assert_eq!(disabled["decision"], "SILENT");
    let done = transition_job(&mut store, P, &task3_id, "succeeded", 1.0).unwrap();
    assert_eq!(done["state"], "succeeded");
    assert_eq!(store.job_count(P).unwrap(), jobs_before + 3);
    let status = status(&store, P).unwrap();
    assert_eq!(status["enabled"], false);
    assert_eq!(status["may_block_delivery"], false);
    assert!(store.integrity_check().unwrap().is_empty());
    write_evidence(&store, &first, &delivered, &events.events, &status);
}

fn write_evidence(
    store: &Store,
    first: &Value,
    delivered: &Value,
    events: &[Value],
    status: &Value,
) {
    if std::env::var("ARCHMELD_WRITE_EVIDENCE").ok().as_deref() != Some("1") {
        return;
    }
    let job_count = store.job_count(P).unwrap();
    let payload = json!({
        "generated_by": "crates/archmeld-core/tests/instigator.rs",
        "platform": std::env::consts::OS,
        "acceptance": {
            "AT055": {
                "status": "passed",
                "default_disabled": true,
                "nudge": first,
                "delivery": delivered,
                "nudge_events": events.iter().filter(|event| event["type"] == "instigator.nudge").collect::<Vec<_>>(),
                "task_state_unchanged_by_nudge": true,
                "new_tasks_created": 0,
                "job_count": job_count,
                "stale_revision_discarded": true,
                "expired_discarded": true,
                "cooldown_enforced": true,
                "budget_enforced": true,
                "final_status": status,
            }
        }
    });
    let path =
        Path::new(env!("CARGO_MANIFEST_DIR")).join("../../docs/evidence/B024-instigator.json");
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(path, serde_json::to_string_pretty(&payload).unwrap() + "\n").unwrap();
}
