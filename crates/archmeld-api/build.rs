// Copyright 2026 ARCHMELD contributors
// SPDX-License-Identifier: Apache-2.0

//! Records the product build commit so `--version`, MCP `serverInfo` and the
//! ledger's index-run provenance can name the exact binary.

use std::path::PathBuf;
use std::process::Command;

fn git(root: &PathBuf, args: &[&str]) -> Option<String> {
    let output = Command::new("git")
        .arg("-C")
        .arg(root)
        .args(args)
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    Some(String::from_utf8_lossy(&output.stdout).trim().to_string())
}

fn main() {
    let manifest = PathBuf::from(std::env::var("CARGO_MANIFEST_DIR").unwrap());
    let root = manifest
        .parent()
        .and_then(|path| path.parent())
        .map(PathBuf::from)
        .unwrap_or(manifest);
    println!(
        "cargo:rerun-if-changed={}",
        root.join(".git/HEAD").display()
    );
    println!(
        "cargo:rerun-if-changed={}",
        root.join(".git/refs").display()
    );
    println!("cargo:rerun-if-env-changed=ARCHMELD_BUILD_COMMIT");
    println!("cargo:rerun-if-env-changed=ARCHMELD_BUILD_DIRTY");
    let short = std::env::var("ARCHMELD_BUILD_COMMIT")
        .ok()
        .filter(|value| !value.is_empty())
        .unwrap_or_else(|| {
            git(&root, &["rev-parse", "--short", "HEAD"]).unwrap_or_else(|| "unknown".into())
        });
    let dirty = std::env::var("ARCHMELD_BUILD_DIRTY")
        .ok()
        .map(|value| value == "1" || value.eq_ignore_ascii_case("true"))
        .unwrap_or_else(|| {
            git(&root, &["status", "--porcelain"])
                .map(|value| !value.is_empty())
                .unwrap_or(false)
        });
    println!("cargo:rustc-env=ARCHMELD_GIT_SHORT={short}");
    println!(
        "cargo:rustc-env=ARCHMELD_GIT_DIRTY={}",
        if dirty { "true" } else { "false" }
    );
}
