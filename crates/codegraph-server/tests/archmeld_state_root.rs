// Copyright 2026 ARCHMELD contributors
// SPDX-License-Identifier: Apache-2.0

//! Bridge-mode isolation: `--archmeld-state-root` must take effect before the
//! startup crash marker, socket path or index state resolve, and the process
//! must never create `~/.codegraph`.
//!
//! Uses `--info`, which exits before any graph backend is opened, so this runs
//! on platforms where the persistent RocksDB backend is unavailable.

use std::path::Path;

#[test]
fn state_root_precedes_the_startup_crash_marker() {
    let dir = tempfile::tempdir().expect("tempdir");
    let home = dir.path().join("home");
    let state = dir.path().join("state");
    std::fs::create_dir_all(&home).expect("home");

    let output = std::process::Command::new(env!("CARGO_BIN_EXE_codegraph-server"))
        .args(["--info", "--archmeld-state-root"])
        .arg(&state)
        .env("HOME", &home)
        .env("USERPROFILE", &home)
        .env_remove("ARCHMELD_STATE_ROOT")
        .output()
        .expect("run codegraph-server");
    assert!(
        output.status.success(),
        "server failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );

    let marker = std::fs::read_dir(&state)
        .map(|entries| {
            entries
                .filter_map(Result::ok)
                .any(|entry| entry.file_name().to_string_lossy().starts_with("last-phase."))
        })
        .unwrap_or(false);
    assert!(marker, "startup marker must live under the state root");

    let leaked = home.join(".codegraph");
    assert!(
        !Path::new(&leaked).exists(),
        "~/.codegraph must not be created in bridge mode: {}",
        leaked.display()
    );
}
