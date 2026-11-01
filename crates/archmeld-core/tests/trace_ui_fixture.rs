// Copyright 2026 ARCHMELD contributors
// SPDX-License-Identifier: Apache-2.0

//! Prepares the extension-host fixture for the Tracy source jump.
//!
//! The test is a no-op unless `ARCHMELD_TRACE_UI_STATE` and
//! `ARCHMELD_TRACE_UI_REPO` are set. With both set it writes a ledger with one
//! snapshot of the fixture repository and one runtime run imported from the
//! real Tracy capture, then verifies the retained source locations can be read
//! from the snapshot CAS at the exact lines.

use archmeld_core::runtime::RuntimeImport;
use archmeld_core::snapshot::{capture_snapshot, SnapshotInputs, SnapshotOptions};
use archmeld_core::source_api::read_source_file;
use archmeld_core::store::Store;
use archmeld_core::tracy_csv::import_tracy_capture;
use archmeld_core::trace_adapters::{TraceLocator, TraceMeta};
use serde_json::{json, Value};
use std::fs;
use std::path::{Path, PathBuf};

const P: &str = "project.traceui";

fn fixtures() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../adapters/traces/fixtures")
}

#[test]
fn write_trace_ui_fixture_when_requested() {
    let Some(state) = std::env::var_os("ARCHMELD_TRACE_UI_STATE") else {
        return;
    };
    let Some(repo) = std::env::var_os("ARCHMELD_TRACE_UI_REPO") else {
        return;
    };
    let state = PathBuf::from(state);
    let repo = PathBuf::from(repo);
    assert!(repo.is_dir(), "fixture repository is missing");
    assert!(
        !state.join("state.sqlite3").exists(),
        "trace UI state already exists; remove it before regenerating"
    );
    let mut store = Store::open(&state).unwrap();
    store.ensure_project(P, "expedition").unwrap();
    let snapshot = capture_snapshot(
        &mut store,
        P,
        &repo,
        &SnapshotInputs {
            build_config: json!({"variant": "tracy-ui"}),
            environment: json!({"os": "windows", "profile": "trace-ui"}),
        },
        &SnapshotOptions {
            include_untracked: true,
            ..SnapshotOptions::default()
        },
    )
    .unwrap();
    let config_digest = snapshot.snapshot["config_digest"]
        .as_str()
        .unwrap()
        .to_string();
    let capture = fs::read(fixtures().join("tracy-zones-plots.tracy")).unwrap();
    let csv_bytes = fs::read(fixtures().join("tracy-zones-plots.csv")).unwrap();
    let csv = String::from_utf8(csv_bytes).unwrap();
    let meta = TraceMeta::new(
        &snapshot.snapshot_id,
        &config_digest,
        json!({"kind": "native", "variant": "tracy-ui"}),
        "tracy",
        "0.13.1",
    );
    let report = import_tracy_capture(
        &mut store,
        &RuntimeImport {
            project_id: P,
            run_id: "run.tracy.ui",
            snapshot_id: Some(&snapshot.snapshot_id),
            config_digest: &config_digest,
            allow_unbound: false,
            scenario_run_id: None,
        },
        &capture,
        &csv,
        &TraceLocator::new(),
        &meta,
    )
    .unwrap();
    let located: Vec<Value> = report["spans"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|span| span["attributes"]["tracy.src_file"].is_string())
        .map(|span| {
            json!({
                "span_id": span["span_id"],
                "name": span["name"],
                "file": span["attributes"]["tracy.src_file"],
                "line": span["attributes"]["tracy.src_line"],
            })
        })
        .collect();
    assert_eq!(located.len(), 2, "both real Tracy zones retain a source location");
    for span in &located {
        let file = span["file"].as_str().unwrap();
        let line = span["line"]
            .as_i64()
            .or_else(|| span["line"].as_str().and_then(|text| text.parse().ok()))
            .expect("Tracy source line is numeric");
        let excerpt = read_source_file(
            &store,
            &json!({
                "project_id": P,
                "snapshot_id": snapshot.snapshot_id,
                "path": file,
                "line": line,
                "context_lines": 2,
            }),
        )
        .unwrap();
        assert!(
            excerpt["line_start"].as_i64().unwrap() <= line
                && line <= excerpt["line_end"].as_i64().unwrap(),
            "line {line} is inside the excerpt for {file}"
        );
        assert!(
            excerpt["text"].as_str().unwrap().contains("ZoneScopedN"),
            "excerpt for {file} contains the zone line"
        );
    }
    let manifest = json!({
        "schema_version": "1.0",
        "project_id": P,
        "snapshot_id": snapshot.snapshot_id,
        "config_digest": config_digest,
        "run_id": "run.tracy.ui",
        "spans": located,
    });
    fs::write(
        state.join("ui-trace-fixture.json"),
        serde_json::to_string_pretty(&manifest).unwrap() + "\n",
    )
    .unwrap();
}
