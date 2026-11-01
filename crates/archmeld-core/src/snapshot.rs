// Copyright 2026 ARCHMELD contributors
// SPDX-License-Identifier: Apache-2.0

//! Snapshot capture (S02): enumerate a workspace, hash raw bytes into the CAS,
//! record the manifest, and never publish a mixed-generation snapshot.
//!
//! Content identity excludes the absolute workspace path, capture time and
//! machine identity. The environment and build config are hashed separately so
//! two build variants of the same bytes are distinct analysis units.

use crate::error::{CoreError, Result};
use crate::hash;
use crate::store::{
    Coverage, CoverageState, FileEntry, FileOrigin, FileRole, SnapshotInsert, Store,
};
use serde_json::{json, Value};
use std::collections::{BTreeMap, BTreeSet, HashSet};
use std::fs;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

const SNAPSHOT_VERSION: &str = "1.0";
const MAX_DEPTH: usize = 64;

#[derive(Debug, Clone)]
pub struct SnapshotOptions {
    /// Include untracked files that are not gitignored. Tracked files are
    /// always included.
    pub include_untracked: bool,
    pub max_file_bytes: u64,
    /// Directory names skipped at any depth.
    pub excluded_dirs: Vec<String>,
    /// Whole-capture retries when the workspace changes under us.
    pub max_retries: u32,
}

impl Default for SnapshotOptions {
    fn default() -> Self {
        Self {
            include_untracked: false,
            max_file_bytes: 8 * 1024 * 1024,
            excluded_dirs: vec![
                ".git".into(),
                ".hg".into(),
                ".svn".into(),
                "node_modules".into(),
                "target".into(),
                ".venv".into(),
                "__pycache__".into(),
                ".mypy_cache".into(),
                ".pytest_cache".into(),
                ".tox".into(),
                ".gradle".into(),
                ".dart_tool".into(),
            ],
            max_retries: 2,
        }
    }
}

/// Build and environment manifests supplied by the caller. Both are hashed
/// with a version prefix; secrets must never be placed here (S02 §2).
#[derive(Debug, Clone)]
pub struct SnapshotInputs {
    pub build_config: Value,
    pub environment: Value,
}

#[derive(Debug, Clone)]
pub struct OmittedFile {
    pub path: String,
    pub reason: String,
}

#[derive(Debug, Clone)]
pub struct CapturedSnapshot {
    pub snapshot_id: String,
    pub snapshot: Value,
    pub files: Vec<FileEntry>,
    pub git_commit: Option<String>,
    pub coverage: Coverage,
    pub omitted: Vec<OmittedFile>,
    /// Result of the opted-in capture-time retention pass, if any.
    pub retention: Option<Value>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct MetaFile {
    path: String,
    size: u64,
    mtime_ns: u128,
}

enum ReadOutcome {
    Read(Vec<u8>),
    TooLarge,
    ChangedDuringRead,
}

#[derive(Default)]
struct WalkResult {
    files: Vec<MetaFile>,
    omitted: Vec<OmittedFile>,
    case_collisions: Vec<String>,
}

/// Capture and register a snapshot. The ledger row and file list are inserted
/// only after the workspace is observed stable across two passes.
pub fn capture_snapshot(
    store: &mut Store,
    project_id: &str,
    root: &Path,
    inputs: &SnapshotInputs,
    options: &SnapshotOptions,
) -> Result<CapturedSnapshot> {
    capture_snapshot_with_hook(store, project_id, root, inputs, options, None)
}

/// Same as [`capture_snapshot`] with a deterministic mutation hook between the
/// read pass and the verification pass. Used by AT012 fault injection.
pub fn capture_snapshot_with_hook(
    store: &mut Store,
    project_id: &str,
    root: &Path,
    inputs: &SnapshotInputs,
    options: &SnapshotOptions,
    mut hook: Option<&mut dyn FnMut()>,
) -> Result<CapturedSnapshot> {
    if !root.is_dir() {
        return Err(CoreError::invalid(format!(
            "snapshot root is not a directory: {}",
            root.display()
        )));
    }
    let config_digest = manifest_digest("build", &inputs.build_config)?;
    let environment_digest = manifest_digest("environment", &inputs.environment)?;
    let exclusions_digest = exclusions_digest(options)?;
    let is_git = is_git_repository(root);
    let (tracked, ignored, git_commit) = git_sets(root);

    let mut last_reason = String::new();
    for _attempt in 0..=options.max_retries {
        let first = walk(root, options)?;
        if !first.case_collisions.is_empty() {
            return Err(CoreError::invalid(format!(
                "case-insensitive path collision: {}",
                first.case_collisions.join(", ")
            )));
        }
        let mut files = Vec::new();
        let mut omitted: Vec<OmittedFile> = Vec::new();
        let mut changed = false;
        for meta in &first.files {
            let path = PathBuf::from(&meta.path);
            if !tracked.contains(&meta.path) {
                if is_git {
                    if ignored.contains(&meta.path) {
                        omitted.push(OmittedFile {
                            path: meta.path.clone(),
                            reason: "gitignored".into(),
                        });
                        continue;
                    }
                    if !options.include_untracked {
                        omitted.push(OmittedFile {
                            path: meta.path.clone(),
                            reason: "untracked_not_included".into(),
                        });
                        continue;
                    }
                } else if !options.include_untracked {
                    // Outside git there is no ignore information; the caller
                    // must opt in to an untracked tree explicitly.
                    omitted.push(OmittedFile {
                        path: meta.path.clone(),
                        reason: "untracked_not_included".into(),
                    });
                    continue;
                }
            }
            match read_file(root, &path, meta, options.max_file_bytes)? {
                ReadOutcome::TooLarge => omitted.push(OmittedFile {
                    path: meta.path.clone(),
                    reason: "too_large".into(),
                }),
                ReadOutcome::ChangedDuringRead => {
                    changed = true;
                    last_reason = format!("file changed while reading: {}", meta.path);
                    break;
                }
                ReadOutcome::Read(bytes) => {
                    if is_binary(&bytes) {
                        omitted.push(OmittedFile {
                            path: meta.path.clone(),
                            reason: "binary".into(),
                        });
                        continue;
                    }
                    let media_type = media_type_for(&meta.path);
                    let blob_digest = store.put_blob(&bytes, media_type)?;
                    let origin = if tracked.contains(&meta.path) {
                        FileOrigin::Tracked
                    } else {
                        FileOrigin::Untracked
                    };
                    files.push(FileEntry {
                        path: meta.path.clone(),
                        blob_digest,
                        size_bytes: bytes.len() as u64,
                        role: role_for(&meta.path),
                        executable: is_executable(root, &path),
                        origin,
                    });
                }
            }
        }
        if changed {
            continue;
        }
        if let Some(hook) = hook.as_mut() {
            hook();
        }
        let second = walk(root, options)?;
        if first.files != second.files {
            last_reason = "workspace metadata changed between passes".into();
            continue;
        }
        files.sort_by(|a, b| a.path.as_bytes().cmp(b.path.as_bytes()));
        omitted.extend(first.omitted);
        omitted.sort_by(|a, b| a.path.cmp(&b.path).then(a.reason.cmp(&b.reason)));
        omitted.dedup_by(|a, b| a.path == b.path && a.reason == b.reason);

        let coverage = coverage_for(&files, &omitted);
        let snapshot = build_snapshot(
            project_id,
            &config_digest,
            &environment_digest,
            &exclusions_digest,
            git_commit.clone(),
            &files,
            &coverage,
        )?;
        let snapshot_id = snapshot["id"].as_str().unwrap().to_string();
        let manifest_bytes = hash::canonical_json(&snapshot)
            .map_err(|err| CoreError::invalid(format!("snapshot manifest: {err}")))?;
        let manifest_artifact = store.put_blob(manifest_bytes.as_bytes(), "application/json")?;
        store.insert_snapshot(SnapshotInsert {
            snapshot_id: snapshot_id.clone(),
            project_id: project_id.to_string(),
            config_digest: config_digest.clone(),
            environment_digest: environment_digest.clone(),
            exclusions_digest: exclusions_digest.clone(),
            manifest_artifact,
            stable: true,
            coverage: coverage.clone(),
            payload: snapshot.clone(),
            files: files.clone(),
        })?;
        let retention = match crate::retention::auto_prune(store, project_id) {
            Ok(value) => value,
            Err(error) => Some(json!({"state": "error", "reason": error.to_string()})),
        };
        return Ok(CapturedSnapshot {
            snapshot_id,
            snapshot,
            files,
            git_commit,
            coverage,
            omitted,
            retention,
        });
    }
    Err(CoreError::conflict(format!(
        "unstable_workspace: {last_reason}; no snapshot was published"
    )))
}

fn manifest_digest(kind: &str, manifest: &Value) -> Result<String> {
    hash::digest_value(&json!({
        "version": SNAPSHOT_VERSION,
        "manifest_kind": kind,
        "manifest": manifest,
    }))
    .map_err(|err| CoreError::invalid(format!("{kind} manifest is outside the hash domain: {err}")))
}

fn exclusions_digest(options: &SnapshotOptions) -> Result<String> {
    let mut dirs = options.excluded_dirs.clone();
    dirs.sort();
    hash::digest_value(&json!({
        "version": SNAPSHOT_VERSION,
        "excluded_dirs": dirs,
        "max_file_bytes": options.max_file_bytes,
        "include_untracked": options.include_untracked,
    }))
    .map_err(|err| CoreError::invalid(format!("exclusion manifest: {err}")))
}

fn build_snapshot(
    project_id: &str,
    config_digest: &str,
    environment_digest: &str,
    exclusions_digest: &str,
    git_commit: Option<String>,
    files: &[FileEntry],
    coverage: &Coverage,
) -> Result<Value> {
    let without_id = json!({
        "schema_version": "1.0",
        "project_id": project_id,
        "created_at": now_rfc3339(),
        "kind": "disk",
        "base_snapshot_id": null,
        "git_commit": git_commit,
        "config_digest": config_digest,
        "environment_digest": environment_digest,
        "exclusions_digest": exclusions_digest,
        "files": files,
        "coverage": coverage,
        "stable": true,
    });
    let mut identity = without_id.clone();
    identity.as_object_mut().unwrap().remove("created_at");
    identity.as_object_mut().unwrap().remove("id");
    let id = hash::digest_value(&identity)
        .map_err(|err| CoreError::invalid(format!("snapshot identity: {err}")))?;
    let mut snapshot = without_id;
    snapshot["id"] = Value::String(id.clone());
    Ok(snapshot)
}

fn coverage_for(files: &[FileEntry], omitted: &[OmittedFile]) -> Coverage {
    let mut reasons: BTreeMap<&str, usize> = BTreeMap::new();
    for item in omitted {
        *reasons.entry(item.reason.as_str()).or_default() += 1;
    }
    let limitations: Vec<String> = reasons
        .iter()
        .map(|(reason, count)| format!("{reason}: {count} file(s)"))
        .collect();
    let state = if omitted.is_empty() {
        CoverageState::Complete
    } else {
        CoverageState::Partial
    };
    Coverage {
        state,
        scope: "workspace manifest under the declared root; content only".into(),
        expected_count: Some((files.len() + omitted.len()) as u64),
        observed_count: files.len() as u64,
        omitted_count: omitted.len() as u64,
        unresolved_count: 0,
        limitations,
    }
}

fn walk(root: &Path, options: &SnapshotOptions) -> Result<WalkResult> {
    let mut result = WalkResult::default();
    let excluded: HashSet<&str> = options.excluded_dirs.iter().map(String::as_str).collect();
    walk_dir(root, root, &excluded, 0, options.max_file_bytes, &mut result)?;
    let mut seen_lower: HashSet<String> = HashSet::new();
    for file in &result.files {
        let lower = file.path.to_ascii_lowercase();
        if !seen_lower.insert(lower) {
            result.case_collisions.push(file.path.clone());
        }
    }
    result.files.sort_by(|a, b| a.path.cmp(&b.path));
    Ok(result)
}

fn walk_dir(
    root: &Path,
    dir: &Path,
    excluded: &HashSet<&str>,
    depth: usize,
    max_file_bytes: u64,
    result: &mut WalkResult,
) -> Result<()> {
    if depth > MAX_DEPTH {
        return Err(CoreError::invalid(format!(
            "directory nesting exceeds {MAX_DEPTH} at {}",
            dir.display()
        )));
    }
    let entries = fs::read_dir(dir)?;
    for entry in entries {
        let entry = entry?;
        let path = entry.path();
        let metadata = fs::symlink_metadata(&path)?;
        let file_type = metadata.file_type();
        let relative = match path.strip_prefix(root) {
            Ok(relative) => relative,
            Err(_) => continue,
        };
        let Some(relative_str) = relative.to_str() else {
            result.omitted.push(OmittedFile {
                path: relative.to_string_lossy().into_owned(),
                reason: "non_utf8_path".into(),
            });
            continue;
        };
        let posix = relative_str.replace('\\', "/");
        if file_type.is_symlink() {
            result.omitted.push(OmittedFile {
                path: posix,
                reason: "symlink_not_followed".into(),
            });
            continue;
        }
        if file_type.is_dir() {
            let name = entry.file_name();
            let name = name.to_string_lossy();
            if excluded.contains(name.as_ref()) {
                result.omitted.push(OmittedFile {
                    path: format!("{posix}/"),
                    reason: "excluded_dir".into(),
                });
                continue;
            }
            walk_dir(root, &path, excluded, depth + 1, max_file_bytes, result)?;
            continue;
        }
        if !file_type.is_file() {
            result.omitted.push(OmittedFile {
                path: posix,
                reason: "not_regular_file".into(),
            });
            continue;
        }
        if metadata.len() > max_file_bytes {
            result.omitted.push(OmittedFile {
                path: posix,
                reason: "too_large".into(),
            });
            continue;
        }
        let mtime_ns = metadata
            .modified()
            .ok()
            .and_then(|time| time.duration_since(UNIX_EPOCH).ok())
            .map(|duration| duration.as_nanos())
            .unwrap_or(0);
        result.files.push(MetaFile {
            path: posix,
            size: metadata.len(),
            mtime_ns,
        });
    }
    Ok(())
}

fn read_file(root: &Path, relative: &Path, meta: &MetaFile, max: u64) -> Result<ReadOutcome> {
    let full = root.join(relative);
    let bytes = fs::read(&full)?;
    if bytes.len() as u64 > max {
        return Ok(ReadOutcome::TooLarge);
    }
    if bytes.len() as u64 != meta.size {
        return Ok(ReadOutcome::ChangedDuringRead);
    }
    Ok(ReadOutcome::Read(bytes))
}

fn is_binary(bytes: &[u8]) -> bool {
    bytes.iter().take(8192).any(|byte| *byte == 0)
}

fn is_executable(root: &Path, relative: &Path) -> bool {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::metadata(root.join(relative))
            .map(|m| m.permissions().mode() & 0o111 != 0)
            .unwrap_or(false)
    }
    #[cfg(not(unix))]
    {
        let _ = (root, relative);
        false
    }
}

fn role_for(path: &str) -> FileRole {
    let lower = path.to_ascii_lowercase();
    let name = lower.rsplit('/').next().unwrap_or(&lower);
    if lower.contains("/test/")
        || lower.contains("/tests/")
        || name.starts_with("test_")
        || name.ends_with("_test.py")
        || name.ends_with(".test.ts")
        || name.ends_with(".test.tsx")
        || name.ends_with(".test.js")
        || name.ends_with("_test.go")
    {
        return FileRole::Test;
    }
    if lower.contains("/generated/")
        || lower.contains("/gen/")
        || name.contains(".generated.")
        || name.ends_with(".g.dart")
        || name.ends_with(".pb.go")
        || name.ends_with(".pb.cc")
        || name.ends_with(".pb.h")
    {
        return FileRole::Generated;
    }
    if matches!(
        extension(name),
        "proto" | "graphql" | "gql" | "thrift" | "avsc"
    ) {
        return FileRole::Protocol;
    }
    if matches!(
        name,
        "cargo.toml"
            | "package.json"
            | "pyproject.toml"
            | "setup.py"
            | "go.mod"
            | "go.sum"
            | "pom.xml"
            | "build.gradle"
            | "settings.gradle"
            | "cmakelists.txt"
            | "makefile"
            | "dockerfile"
    ) || matches!(
        extension(name),
        "json"
            | "toml"
            | "yaml"
            | "yml"
            | "ini"
            | "cfg"
            | "conf"
            | "cmake"
            | "csproj"
            | "sln"
            | "props"
            | "targets"
            | "xml"
            | "lock"
    ) {
        return FileRole::Config;
    }
    if matches!(
        extension(name),
        "rs" | "py"
            | "pyi"
            | "js"
            | "jsx"
            | "ts"
            | "tsx"
            | "mjs"
            | "cjs"
            | "c"
            | "h"
            | "cc"
            | "cpp"
            | "cxx"
            | "hpp"
            | "hh"
            | "hxx"
            | "java"
            | "cs"
            | "go"
            | "kt"
            | "kts"
            | "swift"
            | "rb"
            | "php"
            | "scala"
            | "m"
            | "mm"
            | "lua"
            | "sh"
            | "bash"
            | "ps1"
            | "sql"
            | "html"
            | "css"
            | "scss"
            | "vue"
            | "svelte"
            | "dart"
            | "ex"
            | "exs"
            | "erl"
            | "hrl"
            | "hs"
            | "ml"
            | "mli"
            | "clj"
            | "cljs"
            | "fs"
            | "fsx"
            | "r"
            | "jl"
            | "zig"
            | "v"
            | "sv"
            | "vhd"
            | "vhdl"
            | "tcl"
            | "pl"
            | "pm"
            | "lisp"
            | "el"
            | "scm"
            | "sol"
    ) {
        return FileRole::Source;
    }
    FileRole::Other
}

fn extension(name: &str) -> &str {
    name.rsplit_once('.').map(|(_, ext)| ext).unwrap_or("")
}

fn media_type_for(path: &str) -> &'static str {
    match extension(&path.to_ascii_lowercase()) {
        "json" => "application/json",
        "py" => "text/x-python",
        "rs" => "text/x-rust",
        "ts" | "tsx" => "text/typescript",
        "js" | "jsx" => "text/javascript",
        "c" | "h" => "text/x-c",
        "cc" | "cpp" | "cxx" | "hpp" | "hh" | "hxx" => "text/x-c++",
        "java" => "text/x-java",
        "cs" => "text/x-csharp",
        "go" => "text/x-go",
        _ => "text/plain",
    }
}

/// Tracked / untracked-nonignored / ignored sets and HEAD commit. Missing git
/// is not an error: content hashes still describe the bytes.
fn git_sets(root: &Path) -> (BTreeSet<String>, BTreeSet<String>, Option<String>) {
    let tracked = git_z(root, &["ls-files", "-z"]);
    let ignored = git_z(root, &["ls-files", "-z", "--others", "--ignored", "--exclude-standard"]);
    let commit = std::process::Command::new("git")
        .arg("-C")
        .arg(root)
        .args(["rev-parse", "--verify", "HEAD"])
        .output()
        .ok()
        .filter(|output| output.status.success())
        .map(|output| String::from_utf8_lossy(&output.stdout).trim().to_string())
        .filter(|commit| commit.len() >= 40);
    (tracked, ignored, commit)
}
fn is_git_repository(root: &Path) -> bool {
    std::process::Command::new("git")
        .arg("-C")
        .arg(root)
        .args(["rev-parse", "--is-inside-work-tree"])
        .output()
        .map(|output| {
            output.status.success()
                && String::from_utf8_lossy(&output.stdout).trim() == "true"
        })
        .unwrap_or(false)
}

fn git_z(root: &Path, args: &[&str]) -> BTreeSet<String> {
    let output = match std::process::Command::new("git")
        .arg("-C")
        .arg(root)
        .args(args)
        .output()
    {
        Ok(output) if output.status.success() => output,
        _ => return BTreeSet::new(),
    };
    String::from_utf8_lossy(&output.stdout)
        .split('\0')
        .filter(|item| !item.is_empty())
        .map(|item| item.replace('\\', "/"))
        .collect()
}

fn now_rfc3339() -> String {
    rfc3339_from_unix(now_unix())
}

fn now_unix() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs() as i64
}

pub(crate) fn rfc3339_from_unix(seconds: i64) -> String {
    let days = seconds.div_euclid(86_400);
    let rem = seconds.rem_euclid(86_400);
    let (year, month, day) = civil_from_days(days);
    format!(
        "{year:04}-{month:02}-{day:02}T{:02}:{:02}:{:02}Z",
        rem / 3600,
        (rem % 3600) / 60,
        rem % 60
    )
}

/// Howard Hinnant's civil_from_days.
fn civil_from_days(days: i64) -> (i64, u32, u32) {
    let z = days + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let year = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = doy - (153 * mp + 2) / 5 + 1;
    let month = mp + if mp < 10 { 3 } else { -9 };
    (
        year + if month <= 2 { 1 } else { 0 },
        month as u32,
        day as u32,
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rfc3339_formats_epoch_and_known_instant() {
        assert_eq!(rfc3339_from_unix(0), "1970-01-01T00:00:00Z");
        assert_eq!(rfc3339_from_unix(1_700_000_000), "2023-11-14T22:13:20Z");
    }

    #[test]
    fn roles_classify_paths() {
        assert_eq!(role_for("src/main.rs"), FileRole::Source);
        assert_eq!(role_for("tests/test_app.py"), FileRole::Test);
        assert_eq!(role_for("Cargo.toml"), FileRole::Config);
        assert_eq!(role_for("api/service.proto"), FileRole::Protocol);
        assert_eq!(role_for("src/generated/model.ts"), FileRole::Generated);
        assert_eq!(role_for("LICENSE"), FileRole::Other);
    }
}
