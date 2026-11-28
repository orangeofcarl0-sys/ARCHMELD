// Copyright 2026 ARCHMELD contributors
// SPDX-License-Identifier: Apache-2.0

//! Small review-cost benchmark. This is deliberately not an index benchmark:
//! it exercises snapshot capture, static workspace review and snapshot diff.
//! Graph indexing/parsing is a separate helper process and is not measured
//! here. Run with `cargo run -p archmeld-core --example review_benchmark`.

use archmeld_core::review::{diff_snapshots, review_workspace_diff};
use archmeld_core::snapshot::{capture_snapshot, SnapshotInputs, SnapshotOptions};
use archmeld_core::store::Store;
use serde_json::{json, Value};
use std::fs;
use std::path::Path;
use std::process;
use std::time::{Instant, SystemTime, UNIX_EPOCH};

fn dir_bytes(path: &Path) -> u64 {
    let mut total = 0;
    let Ok(entries) = fs::read_dir(path) else {
        return 0;
    };
    for entry in entries.flatten() {
        let Ok(metadata) = entry.metadata() else {
            continue;
        };
        if metadata.is_dir() {
            total += dir_bytes(&entry.path());
        } else {
            total += metadata.len();
        }
    }
    total
}

fn write_tree(root: &Path) -> Value {
    let mut files = Vec::new();
    for index in 0..200 {
        let path = root.join("src").join(format!("module_{index}.py"));
        let mut body = String::new();
        for function in 0..10 {
            body.push_str(&format!(
                "def fn_{index}_{function}():\n    return {function}\n\n"
            ));
        }
        fs::write(&path, body).unwrap();
        files.push(path);
    }
    json!(files.len())
}

fn main() {
    let root = std::env::temp_dir().join(format!(
        "archmeld-review-bench-{}-{}",
        process::id(),
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos()
    ));
    let state = root.join("state");
    let repo = root.join("repo");
    fs::create_dir_all(repo.join("src")).unwrap();
    let file_count = write_tree(&repo);
    let mut store = Store::open(&state).unwrap();
    store
        .ensure_project("project.review.bench", "expedition")
        .unwrap();
    let inputs = SnapshotInputs {
        build_config: json!({"variant": "review-bench", "index": "not_run"}),
        environment: json!({"os": std::env::consts::OS, "arch": std::env::consts::ARCH}),
    };
    let options = SnapshotOptions {
        include_untracked: true,
        ..SnapshotOptions::default()
    };

    let capture_started = Instant::now();
    let baseline = capture_snapshot(
        &mut store,
        "project.review.bench",
        &repo,
        &inputs,
        &options,
    )
    .unwrap();
    let baseline_capture_ms = capture_started.elapsed().as_millis() as u64;
    let baseline_bytes = dir_bytes(&state);

    let mut candidates = Vec::new();
    for candidate in 0..3 {
        for offset in 0..5 {
            let index = candidate * 5 + offset;
            let path = repo.join("src").join(format!("module_{index}.py"));
            fs::write(
                &path,
                format!("def fn_{index}_{candidate}():\n    return {candidate}\n"),
            )
            .unwrap();
        }
        let review_started = Instant::now();
        let review = review_workspace_diff(
            &store,
            "project.review.bench",
            &baseline.snapshot_id,
            &repo,
        )
        .unwrap();
        let review_ms = review_started.elapsed().as_millis() as u64;

        let capture_started = Instant::now();
        let captured = capture_snapshot(
            &mut store,
            "project.review.bench",
            &repo,
            &inputs,
            &options,
        )
        .unwrap();
        let candidate_capture_ms = capture_started.elapsed().as_millis() as u64;
        let diff_started = Instant::now();
        let diff = diff_snapshots(
            &store,
            "project.review.bench",
            &baseline.snapshot_id,
            &captured.snapshot_id,
        )
        .unwrap();
        let diff_ms = diff_started.elapsed().as_millis() as u64;
        candidates.push(json!({
            "candidate": candidate + 1,
            "changed_files": 5,
            "review_ms": review_ms,
            "review_state": review["state"],
            "review_coverage": review["coverage"]["state"],
            "diff_ms": diff_ms,
            "diff_state": diff["state"],
            "diff_reuse": diff["reuse"],
            "diff_invalidation": diff["invalidation"],
            "candidate_capture_ms": candidate_capture_ms,
            "snapshot_bytes_after": dir_bytes(&state),
        }));
        for offset in 0..5 {
            let index = candidate * 5 + offset;
            let path = repo.join("src").join(format!("module_{index}.py"));
            let mut body = String::new();
            for function in 0..10 {
                body.push_str(&format!(
                    "def fn_{index}_{function}():\n    return {function}\n\n"
                ));
            }
            fs::write(path, body).unwrap();
        }
    }

    let result = json!({
        "schema": "archmeld-review-bench/1",
        "recorded_at": SystemTime::now().duration_since(UNIX_EPOCH).unwrap_or_default().as_secs(),
        "host": {
            "platform": std::env::consts::OS,
            "arch": std::env::consts::ARCH,
        },
        "fixture": {
            "files": file_count,
            "functions_per_file": 10,
            "changed_files_per_candidate": 5,
            "project": "project.review.bench",
        },
        "baseline": {
            "capture_ms": baseline_capture_ms,
            "snapshot_bytes": baseline_bytes,
            "snapshot_id": baseline.snapshot_id,
        },
        "candidates": candidates,
        "index_mode": "capture_only_no_graph_index",
        "limitations": [
            "graph indexing/parsing is not measured; no helper was run",
            "capture is a full re-capture; incremental indexing is not implemented",
            "one host and one fixture; numbers are descriptive, not a guarantee",
            "review has no base entities in this fixture, so graph impact is empty"
        ],
    });
    println!("{}", serde_json::to_string_pretty(&result).unwrap());
    let _ = fs::remove_dir_all(root);
}
