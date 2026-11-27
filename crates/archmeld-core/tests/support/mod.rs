// Copyright 2026 ARCHMELD contributors
// SPDX-License-Identifier: Apache-2.0

//! Portable test support for external language tools and temporary workspaces.
//!
//! Resolution is environment-first: no test encodes a machine-specific path.
//! `ARCHMELD_TOOLS_ROOT` is authoritative when set; a missing tool is a hard
//! failure. Without it, external-tool tests print `SKIP:` and return, unless
//! `ARCHMELD_REQUIRE_TOOLS=1` forces strict mode for evidence runs.

#![allow(dead_code)]

use serde_json::{json, Value};
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

pub fn env_path(name: &str) -> Option<PathBuf> {
    std::env::var_os(name)
        .filter(|value| !value.is_empty())
        .map(PathBuf::from)
}

pub fn home() -> Option<PathBuf> {
    env_path("USERPROFILE").or_else(|| env_path("HOME"))
}

pub fn temp_root() -> PathBuf {
    env_path("ARCHMELD_TEMP")
        .or_else(|| env_path("TMP"))
        .or_else(|| env_path("TEMP"))
        .or_else(|| env_path("TMPDIR"))
        .unwrap_or_else(std::env::temp_dir)
}

pub fn tempdir(prefix: &str) -> tempfile::TempDir {
    let root = temp_root();
    let _ = fs::create_dir_all(&root);
    tempfile::Builder::new()
        .prefix(prefix)
        .tempdir_in(&root)
        .or_else(|_| tempfile::tempdir())
        .expect("create test tempdir")
}

pub fn tools_root() -> Option<PathBuf> {
    env_path("ARCHMELD_TOOLS_ROOT")
}

/// Seconds since the Unix epoch, recorded in evidence files so a verifier can
/// reject a rerun that silently reused an older output.
pub fn recorded_at_unix() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}

/// Short product commit compiled/tested here, or `unknown` when git is absent.
pub fn product_head() -> String {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    let output = Command::new("git")
        .arg("-C")
        .arg(&root)
        .args(["rev-parse", "--short", "HEAD"])
        .output();
    match output {
        Ok(output) if output.status.success() => {
            String::from_utf8_lossy(&output.stdout).trim().to_string()
        }
        _ => "unknown".to_string(),
    }
}

fn executable_extensions() -> Vec<String> {
    if cfg!(windows) {
        std::env::var("PATHEXT")
            .unwrap_or_else(|_| ".COM;.EXE;.BAT;.CMD".to_string())
            .split(';')
            .filter(|item| !item.is_empty())
            .map(|item| item.to_ascii_lowercase())
            .collect()
    } else {
        vec![String::new()]
    }
}

pub fn which(name: &str) -> Option<PathBuf> {
    let path = std::env::var_os("PATH")?;
    let extensions = executable_extensions();
    for directory in std::env::split_paths(&path) {
        let direct = directory.join(name);
        if direct.is_file() {
            return Some(direct);
        }
        for extension in &extensions {
            if extension.is_empty() {
                continue;
            }
            let candidate = directory.join(format!("{name}{extension}"));
            if candidate.is_file() {
                return Some(candidate);
            }
        }
    }
    None
}

pub fn tool(env_name: &str, relative: &str) -> Option<PathBuf> {
    if let Some(explicit) = env_path(env_name) {
        if explicit.exists() {
            return Some(explicit);
        }
    }
    if let Some(root) = tools_root() {
        let candidate = root.join(relative);
        if candidate.exists() {
            return Some(candidate);
        }
    }
    let relative_path = Path::new(relative);
    let basename = relative_path
        .file_name()
        .map(|name| name.to_string_lossy().to_string())?;
    if let Some(strawberry) = env_path("ARCHMELD_STRAWBERRY_BIN") {
        let candidate = strawberry.join(&basename);
        if candidate.is_file() {
            return Some(candidate);
        }
    }
    let nested = relative_path
        .parent()
        .map(|parent| !parent.as_os_str().is_empty())
        .unwrap_or(false);
    if nested {
        return None;
    }
    which(&basename)
}

pub fn rust_toolchain() -> String {
    std::env::var("ARCHMELD_RUST_TOOLCHAIN")
        .unwrap_or_else(|_| "1.94.1-x86_64-pc-windows-gnu".to_string())
}

pub fn require_program(env_name: &str, relative: &str) -> Option<String> {
    require_tool(env_name, relative).map(|path| path.to_string_lossy().to_string())
}

pub fn require_tool(env_name: &str, relative: &str) -> Option<PathBuf> {
    if let Some(found) = tool(env_name, relative) {
        return Some(found);
    }
    let reason = format!("external tool missing: {env_name} or {relative}");
    let strict_root = std::env::var_os("ARCHMELD_TOOLS_ROOT").is_some();
    let strict_flag = std::env::var("ARCHMELD_REQUIRE_TOOLS").ok().as_deref() == Some("1");
    if strict_root || strict_flag {
        panic!("{reason}; ARCHMELD_TOOLS_ROOT is configured, so this is a hard failure");
    }
    eprintln!("SKIP: {reason}; set ARCHMELD_TOOLS_ROOT or ARCHMELD_REQUIRE_TOOLS=1 to run it");
    None
}

pub fn nuget_root() -> Option<PathBuf> {
    env_path("NUGET_PACKAGES")
        .or_else(|| home().map(|home| home.join(".nuget").join("packages")))
        .filter(|path| path.is_dir())
}

pub fn tool_env(home_dir: &Path) -> Vec<(String, String)> {
    let home = home_dir.to_string_lossy().to_string();
    vec![
        ("HOME".to_string(), home.clone()),
        ("USERPROFILE".to_string(), home.clone()),
        ("HOMEPATH".to_string(), home.clone()),
        (
            "APPDATA".to_string(),
            home_dir.join("AppData/Roaming").to_string_lossy().to_string(),
        ),
        (
            "LOCALAPPDATA".to_string(),
            home_dir.join("AppData/Local").to_string_lossy().to_string(),
        ),
    ]
}

fn rust_channel() -> Option<String> {
    let manifest = Path::new(env!("CARGO_MANIFEST_DIR"));
    let text = fs::read_to_string(manifest.join("../../rust-toolchain.toml")).ok()?;
    for line in text.lines() {
        let line = line.trim();
        if let Some(rest) = line.strip_prefix("channel") {
            let value = rest.trim_start().strip_prefix('=')?.trim();
            return Some(value.trim_matches('"').to_string());
        }
    }
    None
}

pub fn rust_tool(name: &str) -> Option<PathBuf> {
    let normalized: String = name
        .chars()
        .map(|character| {
            if character == '-' || character == '.' {
                '_'
            } else {
                character
            }
        })
        .collect();
    let env_name = format!("ARCHMELD_{}", normalized.to_ascii_uppercase());
    if let Some(explicit) = env_path(&env_name) {
        if explicit.is_file() {
            return Some(explicit);
        }
    }
    let strict = tools_root().is_some()
        || std::env::var("ARCHMELD_REQUIRE_TOOLS").ok().as_deref() == Some("1");
    if !strict {
        return None;
    }
    if let Some(rustup) = which("rustup") {
        if let Ok(output) = Command::new(rustup).args(["which", name]).output() {
            if output.status.success() {
                let candidate = PathBuf::from(String::from_utf8_lossy(&output.stdout).trim());
                if candidate.is_file() {
                    return Some(candidate);
                }
            }
        }
    }
    let channel = rust_channel()?;
    let rustup_home = env_path("RUSTUP_HOME").or_else(|| home().map(|home| home.join(".rustup")))?;
    let toolchains = rustup_home.join("toolchains");
    let mut directories: Vec<PathBuf> = fs::read_dir(toolchains)
        .ok()?
        .filter_map(|entry| entry.ok().map(|entry| entry.path()))
        .filter(|path| {
            path.file_name()
                .map(|name| name.to_string_lossy().starts_with(&channel))
                .unwrap_or(false)
        })
        .collect();
    directories.sort();
    for directory in directories {
        let direct = directory.join("bin").join(name);
        if direct.is_file() {
            return Some(direct);
        }
        if cfg!(windows) {
            let exe = directory.join("bin").join(format!("{name}.exe"));
            if exe.is_file() {
                return Some(exe);
            }
        }
    }
    which(name)
}

pub fn audit_path(fixture: &Path, name: &str) -> PathBuf {
    fixture.join("audit").join(format!("{name}.ndjson"))
}

/// Rewrite a clangd compile database to point at a copied workspace. The
/// tracked fixture may contain the reference machine's directory; tests must
/// not depend on it.
pub fn rewrite_compile_commands(source: &Path, workspace: &Path) -> PathBuf {
    let text = fs::read_to_string(source).expect("read compile_commands");
    let mut value: Value = serde_json::from_str(&text).expect("parse compile_commands");
    if let Some(entries) = value.as_array_mut() {
        for entry in entries {
            let object = entry.as_object_mut().expect("compile_commands entry");
            let old_directory = object
                .get("directory")
                .and_then(Value::as_str)
                .unwrap_or("")
                .to_string();
            object.insert(
                "directory".to_string(),
                json!(workspace.to_string_lossy().to_string()),
            );
            if let Some(file) = object.get("file").and_then(Value::as_str) {
                let file = Path::new(file);
                let absolute = if file.is_absolute() {
                    workspace.join(file.file_name().unwrap_or_default())
                } else {
                    workspace.join(file)
                };
                object.insert("file".to_string(), json!(absolute.to_string_lossy().to_string()));
            }
            if let Some(command) = object.get("command").and_then(Value::as_str) {
                object.insert(
                    "command".to_string(),
                    json!(command.replace(&old_directory, &workspace.to_string_lossy())),
                );
            }
        }
    }
    let destination = workspace.join(
        source
            .file_name()
            .unwrap_or_else(|| std::ffi::OsStr::new("compile_commands.json")),
    );
    fs::write(&destination, serde_json::to_string_pretty(&value).unwrap()).expect("write compile_commands");
    destination
}
