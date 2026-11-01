// Copyright 2026 ARCHMELD contributors
// SPDX-License-Identifier: Apache-2.0

//! B030 / AT066 residuals: Case B scenario matrix for normal flow, slow
//! consumer, bounded-queue backpressure, cancellation and a simulated device
//! disconnect. No hardware is accessed.

use archmeld_core::hash;
use archmeld_core::scenario::run_command_capture;
use serde_json::{json, Map, Value};
use std::fs;
use std::path::Path;
use std::time::Duration;

fn run(mode: &str) -> Value {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../examples/case-b");
    let capture = run_command_capture(
        &[
            "python".to_string(),
            "scenarios.py".to_string(),
            mode.to_string(),
        ],
        &root,
        &[],
        Duration::from_secs(120),
        None,
    )
    .unwrap();
    assert_eq!(
        capture.exit_code,
        Some(0),
        "{mode} failed: {}",
        String::from_utf8_lossy(&capture.stderr)
    );
    let value: Value = serde_json::from_slice(&capture.stdout).unwrap();
    json!({
        "argv": ["python", "scenarios.py", mode],
        "cwd": root.to_string_lossy(),
        "exit_code": capture.exit_code,
        "timed_out": capture.timed_out,
        "wall_ms": capture.wall_ms,
        "stdout_digest": hash::digest_bytes(&capture.stdout),
        "stderr_digest": hash::digest_bytes(&capture.stderr),
        "process_audit": capture.process_audit,
        "result": value,
    })
}

#[test]
fn case_b_scenarios_expose_backpressure_cancellation_and_disconnect() {
    let mut scenarios = Map::new();

    let normal = run("normal");
    assert_eq!(normal["result"]["produced"], 32);
    assert_eq!(normal["result"]["consumed"], 32);
    assert!(normal["result"]["error"].is_null());
    scenarios.insert("normal".into(), normal);

    let slow = run("slow_consumer");
    assert_eq!(slow["result"]["produced"], 32);
    assert_eq!(slow["result"]["consumed"], 32);
    assert!(slow["result"]["error"].is_null());
    scenarios.insert("slow_consumer".into(), slow);

    let saturated = run("queue_saturation");
    assert_eq!(saturated["result"]["produced"], 400);
    assert_eq!(saturated["result"]["consumed"], 400);
    assert!(
        saturated["result"]["blocked"].as_u64().unwrap_or(0) > 0,
        "bounded queue must report backpressure: {saturated}"
    );
    assert!(saturated["result"]["silent_drop"].is_null());
    scenarios.insert("queue_saturation".into(), saturated);

    let cancelled = run("cancel");
    assert_eq!(cancelled["result"]["consumed"], 8);
    assert_eq!(cancelled["result"]["cancelled"], true);
    assert_eq!(cancelled["result"]["pending"], 0);
    scenarios.insert("cancel".into(), cancelled);

    let disconnected = run("disconnect");
    assert_eq!(disconnected["result"]["produced"], 10);
    assert_eq!(disconnected["result"]["consumed"], 10);
    assert_eq!(
        disconnected["result"]["error"], "simulated device disconnected"
    );
    scenarios.insert("disconnect".into(), disconnected);

    let evidence = json!({
        "schema_version": "1.0",
        "generated_by": "crates/archmeld-core/tests/case_b_scenarios.rs",
        "device": "simulated; no hardware access",
        "scenarios": scenarios,
        "note": "Queue backpressure is measured and reported; cancellation stops the producer; disconnect carries an explicit error instead of a green success.",
    });
    if std::env::var("ARCHMELD_WRITE_EVIDENCE").ok().as_deref() == Some("1") {
        for path in [
            "docs/release-evidence/B030-case-b-scenarios.json",
            "docs/evidence/B030-case-b-scenarios.json",
        ] {
            let full = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..").join(path);
            fs::create_dir_all(full.parent().unwrap()).unwrap();
            fs::write(&full, serde_json::to_string_pretty(&evidence).unwrap() + "\n").unwrap();
        }
    }
}
