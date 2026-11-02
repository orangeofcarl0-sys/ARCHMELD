// Copyright 2026 ARCHMELD contributors
// SPDX-License-Identifier: Apache-2.0

//! B026 / AT058: T0/T1/T2 scale measurements, cancellation latency and
//! resource guards. Results are written before budget assertions so a failing
//! sample set is preserved instead of being deleted.

mod common;

use archmeld_core::bridge::{import_codegraph_export, BridgeConfig};
use archmeld_core::drift::analyze_drift;
use archmeld_core::hash;
use archmeld_core::query::run_query;
use archmeld_core::scenario::run_command_capture;
use archmeld_core::snapshot::{capture_snapshot, SnapshotInputs, SnapshotOptions};
use archmeld_core::source_api::read_source;
use archmeld_core::store::Store;
use serde_json::{json, Value};
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

fn bench_tempdir(prefix: &str) -> tempfile::TempDir {
    let base = std::env::var_os("ARCHMELD_BENCH_TEMP")
        .map(PathBuf::from)
        .or_else(|| {
            let candidate = PathBuf::from("<workspace>/_archmeld_temp");
            candidate.is_dir().then_some(candidate)
        });
    match base {
        Some(base) => tempfile::Builder::new()
            .prefix(prefix)
            .tempdir_in(base)
            .unwrap(),
        None => tempfile::tempdir().unwrap(),
    }
}

fn copy_dir(source: &Path, destination: &Path) {
    fs::create_dir_all(destination).unwrap();
    for entry in fs::read_dir(source).unwrap() {
        let entry = entry.unwrap();
        let target = destination.join(entry.file_name());
        if entry.file_type().unwrap().is_dir() {
            copy_dir(&entry.path(), &target);
        } else {
            fs::copy(entry.path(), target).unwrap();
        }
    }
}

fn percentile(samples: &[f64], percentile: f64) -> f64 {
    let mut sorted = samples.to_vec();
    sorted.sort_by(|left, right| left.total_cmp(right));
    let rank = ((percentile / 100.0) * sorted.len() as f64).ceil() as usize;
    sorted[rank.saturating_sub(1).min(sorted.len() - 1)]
}

fn stats(samples: &[f64]) -> Value {
    if samples.is_empty() {
        return json!({"count": 0});
    }
    let mut sorted = samples.to_vec();
    sorted.sort_by(|left, right| left.total_cmp(right));
    json!({
        "count": samples.len(),
        "min_ms": round3(sorted[0]),
        "median_ms": round3(percentile(samples, 50.0)),
        "p95_ms": round3(percentile(samples, 95.0)),
        "max_ms": round3(sorted[sorted.len() - 1]),
        "samples_ms": samples.iter().map(|value| round3(*value)).collect::<Vec<_>>(),
    })
}

fn round3(value: f64) -> f64 {
    (value * 1000.0).round() / 1000.0
}

fn query(
    store: &Store,
    project_id: &str,
    snapshot_id: &str,
    kind: &str,
    text: &str,
    max_nodes: u32,
    max_depth: u32,
) -> archmeld_core::Result<Value> {
    run_query(
        store,
        &json!({
            "project_id": project_id,
            "snapshot_id": snapshot_id,
            "kind": kind,
            "query": text,
            "entity_ids": [],
            "relation_kinds": [],
            "max_nodes": max_nodes,
            "max_depth": max_depth,
            "cursor": null,
        }),
    )
}

fn query_timings(
    store: &Store,
    project_id: &str,
    snapshot_id: &str,
    kind: &str,
    text: &str,
    repetitions: usize,
) -> Vec<f64> {
    for _ in 0..3 {
        query(store, project_id, snapshot_id, kind, text, 200, 2).unwrap();
    }
    (0..repetitions)
        .map(|_| {
            let started = Instant::now();
            query(store, project_id, snapshot_id, kind, text, 200, 2).unwrap();
            started.elapsed().as_secs_f64() * 1000.0
        })
        .collect()
}

fn source_timings(
    store: &Store,
    project_id: &str,
    snapshot_id: &str,
    entity_id: &str,
    repetitions: usize,
) -> Vec<f64> {
    let request = json!({
        "project_id": project_id,
        "snapshot_id": snapshot_id,
        "entity_id": entity_id,
        "anchor_index": 0,
        "max_bytes": 65536,
    });
    for _ in 0..3 {
        read_source(store, &request).unwrap();
    }
    (0..repetitions)
        .map(|_| {
            let started = Instant::now();
            read_source(store, &request).unwrap();
            started.elapsed().as_secs_f64() * 1000.0
        })
        .collect()
}

fn working_set_mb(cwd: &Path) -> Option<f64> {
    if !cfg!(windows) {
        return None;
    }
    let pid = std::process::id();
    let argv = vec![
        "powershell".to_string(),
        "-NoProfile".to_string(),
        "-Command".to_string(),
        format!("(Get-Process -Id {pid}).WorkingSet64"),
    ];
    let capture = run_command_capture(&argv, cwd, &[], Duration::from_secs(10), None).ok()?;
    let text = String::from_utf8_lossy(&capture.stdout).trim().to_string();
    text.parse::<f64>()
        .ok()
        .map(|bytes| (bytes / 1_048_576.0 * 10.0).round() / 10.0)
}

#[test]
fn scale_cancellation_and_budget_measurements_are_repeatable() {
    let repository_root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    let environment_digest = hash::digest_value(&json!({
        "os": std::env::consts::OS,
        "arch": std::env::consts::ARCH,
        "cpus": std::thread::available_parallelism().map(|value| value.get()).unwrap_or(0),
        "profile": "b026-reference",
    }))
    .unwrap();

    // T0: deterministic small fixture, measured cold capture.
    let mut t0 = common::setup("b026-t0");
    let t0_payload = t0
        .store
        .snapshot_payload(&t0.project, &t0.snapshot_id)
        .unwrap()
        .unwrap();
    let t0_files = t0_payload["files"]
        .as_array()
        .map(|files| files.len())
        .unwrap_or(0);
    let mut t0_capture = Vec::new();
    for _ in 0..5 {
        let started = Instant::now();
        capture_snapshot(
            &mut t0.store,
            &t0.project,
            &t0.root,
            &SnapshotInputs {
                build_config: t0.build_config.clone(),
                environment: t0.environment.clone(),
            },
            &SnapshotOptions {
                include_untracked: true,
                ..SnapshotOptions::default()
            },
        )
        .unwrap();
        t0_capture.push(started.elapsed().as_secs_f64() * 1000.0);
    }

    // T1: a fixed real CodeGraph export of this crate, imported into a fresh
    // ledger. The source tree is a copy so a drift measurement cannot touch the
    // working repository.
    let t1_dir = bench_tempdir("b026-t1-");
    let t1_root = t1_dir.path().join("repo");
    copy_dir(
        &repository_root.join("crates/archmeld-core"),
        &t1_root.join("crates/archmeld-core"),
    );
    let mut t1_store = Store::open(t1_dir.path().join("state")).unwrap();
    t1_store.ensure_project("project.b026.t1", "expedition").unwrap();
    let t1_capture = capture_snapshot(
        &mut t1_store,
        "project.b026.t1",
        &t1_root,
        &SnapshotInputs {
            build_config: json!({"kind": "rust", "variant": "b026-t1"}),
            environment: json!({"os": "benchmark"}),
        },
        &SnapshotOptions {
            include_untracked: true,
            ..SnapshotOptions::default()
        },
    )
    .unwrap();
    let t1_config = t1_capture.snapshot["config_digest"].as_str().unwrap().to_string();
    let export = fs::read_to_string(
        repository_root.join("upstream-probes/real-export/archmeld-core.ndjson"),
    )
    .unwrap();
    let import_started = Instant::now();
    let t1_report = import_codegraph_export(
        &mut t1_store,
        &BridgeConfig {
            project_id: "project.b026.t1",
            snapshot_id: &t1_capture.snapshot_id,
            config_digest: &t1_config,
            run_id: "run.b026.t1",
            workspace_root: Some(Path::new("/work")),
            max_entities_per_batch: 500,
            max_relations_per_batch: 2000,
        },
        &t1_capture.files,
        &export,
    )
    .unwrap();
    let t1_import_ms = import_started.elapsed().as_secs_f64() * 1000.0;
    assert!(t1_report.entities > 0, "{:?}", t1_report);

    let mut source_entity = None;
    for entity_id in t1_store
        .entity_ids("project.b026.t1", &t1_capture.snapshot_id)
        .unwrap()
    {
        let entity = t1_store
            .get_entity("project.b026.t1", &t1_capture.snapshot_id, &entity_id)
            .unwrap()
            .unwrap();
        if entity["anchors"]
            .as_array()
            .map(|anchors| !anchors.is_empty())
            .unwrap_or(false)
        {
            source_entity = Some(entity_id);
            break;
        }
    }
    let source_entity = source_entity.expect("T1 has no anchored entity");
    let t1_query_samples = query_timings(
        &t1_store,
        "project.b026.t1",
        &t1_capture.snapshot_id,
        "symbols",
        "archmeld",
        30,
    );
    let t1_source_samples = source_timings(
        &t1_store,
        "project.b026.t1",
        &t1_capture.snapshot_id,
        &source_entity,
        30,
    );
    let t1_lib = t1_root.join("crates/archmeld-core/src/lib.rs");
    let t1_lib_original = fs::read(&t1_lib).unwrap();
    let mut t1_drift_samples = Vec::new();
    for index in 0..10 {
        let mut changed = t1_lib_original.clone();
        changed.extend_from_slice(format!("\n// b026 drift sample {index}\n").as_bytes());
        fs::write(&t1_lib, &changed).unwrap();
        let started = Instant::now();
        let report = analyze_drift(
            &t1_store,
            "project.b026.t1",
            &t1_capture.snapshot_id,
            &t1_root,
            None,
        )
        .unwrap();
        t1_drift_samples.push(started.elapsed().as_secs_f64() * 1000.0);
        assert!(!report["changed"].as_array().unwrap().is_empty());
    }
    fs::write(&t1_lib, &t1_lib_original).unwrap();

    // T2: deterministic 100k-entity / 99.5k-relation synthetic ledger.
    let t2_dir = bench_tempdir("b026-t2-");
    let t2_root = t2_dir.path().join("repo");
    fs::create_dir_all(&t2_root).unwrap();
    fs::write(t2_root.join("bench.py"), "def bench():\n    return 0\n").unwrap();
    let mut t2_store = Store::open(t2_dir.path().join("state")).unwrap();
    t2_store
        .ensure_project("project.b026.t2", "expedition")
        .unwrap();
    let t2_capture = capture_snapshot(
        &mut t2_store,
        "project.b026.t2",
        &t2_root,
        &SnapshotInputs {
            build_config: json!({"kind": "python", "variant": "b026-t2"}),
            environment: json!({"os": "benchmark"}),
        },
        &SnapshotOptions {
            include_untracked: true,
            ..SnapshotOptions::default()
        },
    )
    .unwrap();
    let memory_before = working_set_mb(&t2_root);
    let artifact = t2_capture.files[0].blob_digest.clone();
    let t2_config_digest = t2_config(&t2_capture.snapshot);
    let t2_snapshot_id = t2_capture.snapshot_id.clone();
    let total_entities = 100_000usize;
    let batch_size = 500usize;
    let total_batches = total_entities / batch_size;
    let import_started = Instant::now();
    for batch_index in 0..total_batches {
        let mut entities = Vec::with_capacity(batch_size);
        let mut relations = Vec::new();
        for offset in 0..batch_size {
            let index = batch_index * batch_size + offset;
            let entity_id = format!("entity.bench.{index:06}");
            entities.push(json!({
                "id": entity_id,
                "project_id": "project.b026.t2",
                "snapshot_id": t2_snapshot_id,
                "config_digest": t2_config_digest,
                "kind": "symbol",
                "language": "python",
                "qualified_name": format!("benchmark_fn_{index:06}"),
                "anchors": [{
                    "path": "bench.py",
                    "blob_digest": artifact,
                    "precision": "line_only",
                    "byte_start": null,
                    "byte_end": null,
                    "line_start": 1,
                    "line_end": 1,
                    "encoding": "utf-8",
                }],
                "external": false,
                "backend_refs": [],
            }));
            if index > 0 {
                relations.push(json!({
                    "id": format!("relation.bench.{index:06}"),
                    "project_id": "project.b026.t2",
                    "snapshot_id": t2_snapshot_id,
                    "config_digest": t2_config_digest,
                    "source_id": format!("entity.bench.{index:06}"),
                    "target_id": "entity.bench.000000",
                    "kind": "references",
                    "basis": "syntax",
                    "evidence_ids": ["evidence.bench.t2"],
                    "conditions": ["synthetic scale fixture"],
                    "resolution": "candidate",
                }));
            }
        }
        let evidence = if batch_index == 0 {
            json!([{
                "id": "evidence.bench.t2",
                "project_id": "project.b026.t2",
                "snapshot_id": t2_snapshot_id,
                "config_digest": t2_config_digest,
                "kind": "syntax",
                "producer": {
                    "name": "benchmark.synthetic",
                    "version": "1",
                    "source_ref": "deterministic-generator",
                    "run_id": "run.b026.t2",
                },
                "artifact_digest": artifact,
                "locator": "bench.py",
                "source_anchor": null,
                "scenario_id": null,
                "runtime_run_id": null,
                "coverage": {
                    "state": "complete",
                    "scope": "deterministic synthetic scale fixture",
                    "expected_count": total_entities,
                    "observed_count": total_entities,
                    "omitted_count": 0,
                    "unresolved_count": 0,
                    "limitations": ["synthetic scale data; business correctness is verified on T1"],
                },
                "freshness": "current",
            }])
        } else {
            json!([])
        };
        let batch = json!({
            "schema_version": "1.0",
            "project_id": "project.b026.t2",
            "snapshot_id": t2_snapshot_id,
            "config_digest": t2_config_digest,
            "run_id": "run.b026.t2",
            "adapter_id": "benchmark.synthetic",
            "adapter_version": "1",
            "sequence": batch_index.to_string(),
            "final": batch_index + 1 == total_batches,
            "coverage": {
                "state": "complete",
                "scope": "deterministic synthetic scale fixture",
                "expected_count": batch_size,
                "observed_count": batch_size,
                "omitted_count": 0,
                "unresolved_count": 0,
                "limitations": ["synthetic scale data"],
            },
            "entities": entities,
            "evidence": evidence,
            "relations": relations,
            "diagnostics": [],
        });
        t2_store.import_batch(&batch).unwrap();
    }
    let t2_import_ms = import_started.elapsed().as_secs_f64() * 1000.0;
    let memory_after = working_set_mb(&t2_root);
    let db_size = fs::metadata(t2_dir.path().join("state/state.sqlite3"))
        .unwrap()
        .len();
    let t2_symbol_samples = query_timings(
        &t2_store,
        "project.b026.t2",
        &t2_capture.snapshot_id,
        "symbols",
        "benchmark_fn_0001",
        20,
    );
    let t2_text_samples = query_timings(
        &t2_store,
        "project.b026.t2",
        &t2_capture.snapshot_id,
        "text",
        "benchmark_fn_",
        10,
    );

    // Cancellation: flip the cooperative flag while a real process tree is
    // running and measure how long the executor takes to return.
    let cancel = Arc::new(AtomicBool::new(false));
    let cancel_flag = Arc::clone(&cancel);
    let cancel_started = Instant::now();
    let cancel_cwd = t2_root.clone();
    let handle = std::thread::spawn(move || {
        run_command_capture(
            &[
                "python".to_string(),
                "-c".to_string(),
                "import subprocess,sys,time; subprocess.Popen([sys.executable,'-c','import time;time.sleep(60)']); time.sleep(60)"
                    .to_string(),
            ],
            &cancel_cwd,
            &[],
            Duration::from_secs(30),
            Some(&cancel_flag),
        )
        .unwrap()
    });
    std::thread::sleep(Duration::from_millis(300));
    let cancel_requested = Instant::now();
    cancel.store(true, Ordering::SeqCst);
    let cancelled = handle.join().unwrap();
    let cancel_latency_ms = cancel_requested.elapsed().as_secs_f64() * 1000.0;
    assert!(cancelled.cancelled);
    assert!(cancel_started.elapsed() < Duration::from_secs(30));

    // Resource guards: bounded query/source/import/output and no silent skip.
    let query_limit = query(
        &t2_store,
        "project.b026.t2",
        &t2_capture.snapshot_id,
        "symbols",
        "bench",
        2001,
        1,
    )
    .unwrap_err();
    assert!(query_limit.to_string().contains("max_nodes"));
    let depth_limit = query(
        &t2_store,
        "project.b026.t2",
        &t2_capture.snapshot_id,
        "neighbors",
        "",
        10,
        9,
    )
    .unwrap_err();
    assert!(depth_limit.to_string().contains("max_depth"));
    let source_limit = read_source(
        &t2_store,
        &json!({
            "project_id": "project.b026.t2",
            "snapshot_id": t2_capture.snapshot_id,
            "entity_id": "entity.bench.000000",
            "anchor_index": 0,
            "max_bytes": 0,
        }),
    )
    .unwrap_err();
    assert!(source_limit.to_string().contains("max_bytes"));
    let mut too_many = Vec::new();
    for index in 0..501 {
        too_many.push(json!({
            "id": format!("entity.guard.{index:03}"),
            "project_id": "project.b026.t2",
            "snapshot_id": t2_snapshot_id,
            "config_digest": t2_config_digest,
            "kind": "symbol",
            "language": "python",
            "qualified_name": format!("guard_{index}"),
            "anchors": [{
                "path": "bench.py",
                "blob_digest": artifact,
                "precision": "line_only",
                "byte_start": null,
                "byte_end": null,
                "line_start": 1,
                "line_end": 1,
                "encoding": "utf-8",
            }],
            "external": false,
            "backend_refs": [],
        }));
    }
    let import_limit = t2_store
        .import_batch(&json!({
            "schema_version": "1.0",
            "project_id": "project.b026.t2",
            "snapshot_id": t2_snapshot_id,
            "config_digest": t2_config_digest,
            "run_id": "run.b026.guard",
            "adapter_id": "benchmark.synthetic",
            "adapter_version": "1",
            "sequence": "0",
            "final": true,
            "coverage": {
                "state": "complete",
                "scope": "guard",
                "expected_count": 501,
                "observed_count": 501,
                "omitted_count": 0,
                "unresolved_count": 0,
                "limitations": [],
            },
            "entities": too_many,
            "evidence": [],
            "relations": [],
            "diagnostics": [],
        }))
        .unwrap_err();
    assert!(import_limit.to_string().contains("resource_exhausted"));
    let output_cap = run_command_capture(
        &[
            "python".to_string(),
            "-c".to_string(),
            "import sys;sys.stdout.write('x'*(5*1024*1024))".to_string(),
        ],
        &t2_root,
        &[],
        Duration::from_secs(30),
        None,
    )
    .unwrap();
    assert!(output_cap.stdout_truncated);
    assert_eq!(output_cap.stdout.len(), 4 * 1024 * 1024);

    // UI renderer: the same model/renderer used by the extension, without an
    // editor host. The limitation is recorded, not hidden.
    let ui_capture = run_command_capture(
        &[
            "node".to_string(),
            repository_root
                .join("benchmarks/ui-latency.js")
                .to_string_lossy()
                .to_string(),
            "500".to_string(),
            "1000".to_string(),
            "30".to_string(),
        ],
        &repository_root,
        &[],
        Duration::from_secs(60),
        None,
    )
    .unwrap();
    assert_eq!(
        ui_capture.exit_code,
        Some(0),
        "{}",
        String::from_utf8_lossy(&ui_capture.stderr)
    );
    let ui: Value = serde_json::from_slice(&ui_capture.stdout).unwrap();

    let t1_query_p95 = percentile(&t1_query_samples, 95.0);
    let t1_source_p95 = percentile(&t1_source_samples, 95.0);
    let t1_drift_p95 = percentile(&t1_drift_samples, 95.0);
    let ui_p95 = ui["p95_ms"].as_f64().unwrap_or(f64::INFINITY);
    let budgets = json!({
        "t1_query_p95_ms": 300.0,
        "t1_source_p95_ms": 500.0,
        "cancel_ms": 3000.0,
        "t1_stale_p95_ms": 1000.0,
        "ui_render_p95_ms": 500.0,
    });
    let passed = t1_query_p95 <= 300.0
        && t1_source_p95 <= 500.0
        && t1_drift_p95 <= 1000.0
        && cancel_latency_ms <= 3000.0
        && ui_p95 <= 500.0;
    let results = json!({
        "generated_by": "crates/archmeld-core/tests/benchmark.rs",
        "status": if passed { "passed" } else { "failed" },
        "environment": {
            "os": std::env::consts::OS,
            "arch": std::env::consts::ARCH,
            "cpus": std::thread::available_parallelism().map(|value| value.get()).unwrap_or(0),
            "environment_digest": environment_digest,
            "reference_profile": "this Windows reference machine; no cross-machine normalization",
        },
        "datasets": {
            "T0": {
                "kind": "deterministic fixture",
                "files": t0_files,
                "cold_capture": stats(&t0_capture),
            },
            "T1": {
                "kind": "fixed real CodeGraph export of archmeld-core",
                "source_files": t1_capture.files.len(),
                "export_nodes": 335,
                "export_edges": 972,
                "ledger_entities": t1_report.entities,
                "ledger_relations": t1_report.relations,
                "ledger_import_ms": round3(t1_import_ms),
                "coverage": t1_report.coverage,
                "symbol_query": stats(&t1_query_samples),
                "source_read": stats(&t1_source_samples),
                "drift_stale": stats(&t1_drift_samples),
            },
            "T2": {
                "kind": "deterministic synthetic scale data",
                "entities": total_entities,
                "relations": (total_batches - 1) * batch_size,
                "batches": total_batches,
                "batch_size": batch_size,
                "ledger_import_ms": round3(t2_import_ms),
                "symbol_query": stats(&t2_symbol_samples),
                "text_query": stats(&t2_text_samples),
                "db_size_bytes": db_size,
                "working_set_before_mb": memory_before,
                "working_set_after_mb": memory_after,
                "business_correctness": "not claimed from synthetic data; T1 carries real-source checks",
            },
        },
        "budgets": budgets,
        "results": {
            "t1_query_p95_ms": round3(t1_query_p95),
            "t1_source_p95_ms": round3(t1_source_p95),
            "t1_stale_p95_ms": round3(t1_drift_p95),
            "cancel_ms": round3(cancel_latency_ms),
            "ui_render_p95_ms": round3(ui_p95),
        },
        "cancellation": {
            "cancelled": cancelled.cancelled,
            "exit_code": cancelled.exit_code,
            "wall_ms": cancelled.wall_ms,
            "process_audit": cancelled.process_audit,
            "retried": false,
        },
        "guards": {
            "max_nodes_over_cap": "invalid",
            "max_depth_over_cap": "invalid",
            "source_max_bytes_zero": "invalid",
            "import_501_entities": "resource_exhausted",
            "stdout_cap_bytes": output_cap.stdout.len(),
            "stdout_truncated": output_cap.stdout_truncated,
            "silent_skip": false,
        },
        "ui": ui,
        "limitations": [
            "T2 is synthetic scale data; its business correctness is not claimed",
            "UI latency is webview HTML generation, not VS Code host paint latency",
            "no cross-machine normalization; budgets apply to the declared reference environment",
            "cancellation uses the declared cooperative flag and process-tree termination",
        ],
    });
    let serialized = serde_json::to_string_pretty(&results).unwrap() + "
";
    let scratch = repository_root.join("target/b026-benchmark-latest.json");
    fs::create_dir_all(scratch.parent().unwrap()).unwrap();
    fs::write(&scratch, &serialized).unwrap();
    if std::env::var("ARCHMELD_WRITE_EVIDENCE").ok().as_deref() == Some("1") {
        let latest = repository_root.join("benchmarks/results/B026-latest.json");
        fs::create_dir_all(latest.parent().unwrap()).unwrap();
        fs::write(&latest, &serialized).unwrap();
        let evidence = repository_root.join("docs/evidence/B026-benchmark.json");
        fs::create_dir_all(evidence.parent().unwrap()).unwrap();
        fs::write(&evidence, &serialized).unwrap();
    }
    assert!(
        passed,
        "budget exceeded; samples preserved in target/b026-benchmark-latest.json: {results}"
    );
}

fn t2_config(snapshot: &Value) -> String {
    snapshot["config_digest"].as_str().unwrap().to_string()
}
