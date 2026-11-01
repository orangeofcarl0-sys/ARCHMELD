// Copyright 2026 ARCHMELD contributors
// SPDX-License-Identifier: Apache-2.0

//! B028 / AT063: the workspace is explicitly externally writable. A shell-capable
//! external agent can change a file after preparation; the old prepared change
//! must be detected as stale and must not overwrite the external bytes.

mod common;

use archmeld_core::changeset::get_changeset;
use archmeld_core::drift::analyze_drift;
use archmeld_core::hash;
use archmeld_core::manager::get_project;
use serde_json::{json, Value};
use std::fs;
use std::path::Path;

fn write_b028_evidence(value: Value) {
    if std::env::var("ARCHMELD_WRITE_EVIDENCE").ok().as_deref() != Some("1") {
        return;
    }
    let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../docs/evidence/B028-security.json");
    let mut root: Value = if path.exists() {
        serde_json::from_str(&fs::read_to_string(&path).unwrap()).unwrap_or_else(|_| json!({}))
    } else {
        json!({
            "generated_by": "crates/archmeld-core/tests/security_external_write.rs",
            "platform": std::env::consts::OS,
            "acceptance": {},
        })
    };
    root["acceptance"]["AT063"] = value;
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(path, serde_json::to_string_pretty(&root).unwrap() + "\n").unwrap();
}

#[test]
fn external_write_is_visible_and_blocks_the_old_prepared_patch() {
    let mut fixture = common::setup("b028-external");
    let project = get_project(&fixture.store, &fixture.project)
        .unwrap()
        .unwrap();
    assert_eq!(project["workspace_security"]["externally_writable"], true);
    assert_eq!(project["workspace_security"]["isolation"], "none");
    assert_eq!(project["workspace_security"]["not_a_sandbox"], true);

    let plan_id = "plan.b028.external";
    common::approved_plan(&mut fixture, plan_id, &["src/a.py"]);
    let changeset_id = "changeset.b028.external";
    common::create_changeset(&mut fixture, changeset_id, plan_id, &[]);
    let seeded = common::prepare(&mut fixture, changeset_id, 1, None, "seed").unwrap();
    let staging = common::staging_path(&fixture, &seeded);
    common::write_file(&staging, "src/a.py", "VALUE = 77\n");
    let transform = common::transform(&mut fixture, "manual_edit", json!([]));
    let prepared = common::prepare(
        &mut fixture,
        changeset_id,
        seeded["revision"].as_i64().unwrap(),
        Some(transform),
        "transform",
    )
    .unwrap();
    let approval = common::approve_prepared(
        &mut fixture,
        changeset_id,
        prepared["revision"].as_i64().unwrap(),
        prepared["prepared_digest"].as_str().unwrap(),
    );

    // A separate shell-capable agent writes the same file after approval.
    let external = "EXTERNAL_AGENT = 99\n";
    common::write_file(&fixture.root, "src/a.py", external);
    let external_digest = hash::digest_bytes(&fs::read(fixture.root.join("src/a.py")).unwrap());

    let blocked = common::apply(
        &mut fixture,
        changeset_id,
        prepared["revision"].as_i64().unwrap(),
        prepared["prepared_digest"].as_str().unwrap(),
        approval["approval"]["approval_id"].as_str().unwrap(),
        "b028-external-apply",
    )
    .unwrap_err();
    let message = blocked.to_string();
    assert!(
        message.contains("stale") || message.contains("external") || message.contains("changed"),
        "{message}"
    );
    assert_eq!(
        hash::digest_bytes(&fs::read(fixture.root.join("src/a.py")).unwrap()),
        external_digest,
        "an old prepared patch must not overwrite externally written bytes"
    );
    let changeset =
        get_changeset(&fixture.store, &fixture.project, changeset_id).unwrap();
    assert_ne!(changeset["state"], "applied");

    let drift = analyze_drift(
        &fixture.store,
        &fixture.project,
        &fixture.snapshot_id,
        &fixture.root,
        None,
    )
    .unwrap();
    assert!(drift["changed"]
        .as_array()
        .unwrap()
        .iter()
        .any(|item| item["path"] == "src/a.py"));
    assert!(drift["stale_plans"]
        .as_array()
        .map(|items| !items.is_empty())
        .unwrap_or(false)
        || drift["stale_changesets"]
            .as_array()
            .map(|items| !items.is_empty())
            .unwrap_or(false));

    write_b028_evidence(json!({
        "status": "passed",
        "workspace_security": project["workspace_security"],
        "external_write_digest": external_digest,
        "blocked_error": message,
        "changeset_state_after_block": changeset["state"],
        "source_preserved": true,
        "drift_changed": drift["changed"],
        "stale_plans": drift["stale_plans"],
        "stale_changesets": drift["stale_changesets"],
        "claim_of_isolation": false,
    }));
}
