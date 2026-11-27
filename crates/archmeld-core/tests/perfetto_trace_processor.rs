// Copyright 2026 ARCHMELD contributors
// SPDX-License-Identifier: Apache-2.0

//! Optional Perfetto trace-processor backend. The test runs only when
//! `ARCHMELD_TRACE_PROCESSOR` names a real `trace_processor_shell`; it compares
//! the SQL backend with the in-repo protobuf decoder on the pinned fixture.

use archmeld_core::perfetto::perfetto_trace;
use archmeld_core::trace_adapters::{TraceLocator, TraceMeta};
use archmeld_core::trace_processor::{
    perfetto_trace_via_processor_checked, verify_processor_identity, TraceProcessorExpectation,
};
use serde_json::{json, Value};
use std::fs;
use std::path::Path;

fn pinned_processor() -> TraceProcessorExpectation {
    let path = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../adapters/traces/fixtures.lock.json");
    let lock: Value = serde_json::from_str(&fs::read_to_string(path).unwrap()).unwrap();
    let tool = lock["tools"]
        .as_array()
        .unwrap()
        .iter()
        .find(|tool| tool["trace_processor_version"].as_str().is_some())
        .expect("trace processor tool");
    TraceProcessorExpectation {
        version_prefix: tool["trace_processor_version"]
            .as_str()
            .unwrap()
            .to_string(),
        sha256: tool["trace_processor_sha256"].as_str().map(str::to_string),
    }
}

#[test]
fn processor_identity_mismatch_fails_closed() {
    let expected = TraceProcessorExpectation {
        version_prefix: "Perfetto v59.1".into(),
        sha256: Some(format!("sha256:{}", "a".repeat(64))),
    };
    assert!(verify_processor_identity(
        "Perfetto v59.1-1e9c61e98",
        &format!("sha256:{}", "a".repeat(64)),
        &expected
    )
    .is_ok());
    let version_error =
        verify_processor_identity("Perfetto v60.0", &format!("sha256:{}", "a".repeat(64)), &expected)
            .unwrap_err();
    assert!(
        version_error
            .to_string()
            .contains("trace_processor_version_mismatch"),
        "{version_error}"
    );
    let hash_error =
        verify_processor_identity("Perfetto v59.1-1e9c61e98", &format!("sha256:{}", "b".repeat(64)), &expected)
            .unwrap_err();
    assert!(
        hash_error.to_string().contains("trace_processor_sha256_mismatch"),
        "{hash_error}"
    );
}

#[test]
fn trace_processor_query_matches_the_in_process_decoder() {
    let Some(processor) = std::env::var_os("ARCHMELD_TRACE_PROCESSOR") else {
        return;
    };
    let trace_path = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../adapters/traces/fixtures/perfetto-counter-state.pftrace");
    let meta = TraceMeta::new(
        "",
        &format!("sha256:{}", "a".repeat(64)),
        json!({"kind": "native", "variant": "trace-processor-test"}),
        "perfetto-trace-processor",
        "v59.1",
    );
    let locator = TraceLocator::new();
    let expected = pinned_processor();
    let capture = perfetto_trace_via_processor_checked(
        &trace_path,
        Path::new(&processor),
        &locator,
        &meta,
        Some(&expected),
    )
    .unwrap();
    assert_eq!(capture.slice_rows, 2);
    assert_eq!(capture.counter_rows, 4);
    assert!(capture.tool_version.starts_with("Perfetto v59.1"));
    assert!(capture.tool_sha256.starts_with("sha256:"));

    let spans = capture.trace["spans"].as_array().unwrap();
    assert_eq!(spans.len(), 2);
    assert_eq!(spans[0]["name"], "stage.queue");
    assert_eq!(spans[0]["start_ns"], "99579362228500");
    assert_eq!(spans[0]["end_ns"], "99579469010400");
    assert_eq!(spans[1]["name"], "stage.compute");
    assert_eq!(spans[1]["parent_span_id"], "slice.1");
    assert_eq!(spans[1]["start_ns"], "99579406554800");
    assert_eq!(spans[1]["end_ns"], "99579438173900");
    let counters = capture.trace["counters"].as_array().unwrap();
    assert_eq!(counters.len(), 1);
    let values: Vec<f64> = counters[0]["samples"]
        .as_array()
        .unwrap()
        .iter()
        .map(|sample| sample["value"].as_f64().unwrap())
        .collect();
    assert_eq!(values, vec![0.0, 3.0, 1.0, 0.0]);

    // The in-process decoder sees the same slices and samples.
    let bytes = fs::read(&trace_path).unwrap();
    let built_in = perfetto_trace(&bytes, &locator, &meta).unwrap();
    let mut built_in_names: Vec<&str> = built_in["spans"]
        .as_array()
        .unwrap()
        .iter()
        .map(|span| span["name"].as_str().unwrap())
        .collect();
    built_in_names.sort_unstable();
    let mut sql_names: Vec<&str> = spans.iter().map(|span| span["name"].as_str().unwrap()).collect();
    sql_names.sort_unstable();
    assert_eq!(built_in_names, sql_names);
    assert_eq!(
        built_in["counters"][0]["samples"].as_array().unwrap().len(),
        4
    );
}
