// Copyright 2026 ARCHMELD contributors
// SPDX-License-Identifier: Apache-2.0

//! Read-only interoperability with an AOCI-CODE cognition map.
//!
//! AOCI-CODE stores a model-authored, Git-versioned index as plain text:
//! `aoci.txt` declares Volumes, and a code Volume holds one FRAS line per
//! managed file. ARCHMELD does not write that format and does not treat its
//! entries as parsed relations. This module only reads the text, maps object
//! paths to repository-relative paths for a review, and reports parse problems
//! as warnings. A missing or malformed map never fails a brief.

use serde_json::{json, Value};
use std::fs;
use std::path::{Path, PathBuf};

const MAX_VOLUME_BYTES: u64 = 8 * 1024 * 1024;
const MAX_ENTRIES: usize = 20_000;
const MAX_AFFECTED: usize = 50;
const FUNCTION_PREVIEW_CHARS: usize = 200;

/// One Entry parsed from an AOCI code Volume. Only the fields needed to map a
/// changed file to its cognition line are retained.
struct Entry {
    path: String,
    name: String,
    tag: String,
    function: Option<String>,
    relocated: bool,
}

struct Volume {
    kind: String,
    path: String,
    format: String,
}

struct Map {
    root: Option<String>,
    volumes: Vec<Volume>,
    entries: Vec<Entry>,
    warnings: Vec<String>,
}

/// Detect and summarize an AOCI map in the workspace. The result is safe to
/// embed in a brief: `present=false` means no map was found, not an error.
pub fn summarize(workspace: Option<&Path>) -> Value {
    let map = load(workspace);
    if map.entries.is_empty() && map.root.is_none() && map.volumes.is_empty() {
        return json!({"provider": "aoci", "present": false});
    }
    json!({
        "provider": "aoci",
        "present": true,
        "root": map.root,
        "volumes": map.volumes.iter().map(|volume| json!({
            "kind": volume.kind,
            "path": volume.path,
            "format": volume.format,
        })).collect::<Vec<_>>(),
        "entries_total": map.entries.len(),
        "warnings": map.warnings,
    })
}

/// Summarize the map and list the entries whose object path matches a changed
/// file. Paths are compared exactly first, then by a `/`-suffix so an older
/// absolute section root still maps to a repository-relative change.
pub fn affected(workspace: Option<&Path>, changed_paths: &[String]) -> Value {
    let map = load(workspace);
    if map.entries.is_empty() && map.root.is_none() && map.volumes.is_empty() {
        return json!({
            "provider": "aoci",
            "present": false,
            "affected": [],
            "unmapped_changed_files": normalize_paths(changed_paths),
        });
    }
    let mut changed = normalize_paths(changed_paths);
    changed.sort();
    changed.dedup();
    let mut affected = Vec::new();
    let mut matched_changed = Vec::new();
    let mut truncated = false;
    for path in &changed {
        let mut matches = map
            .entries
            .iter()
            .filter(|entry| entry_matches(&entry.path, path))
            .collect::<Vec<_>>();
        if matches.is_empty() {
            continue;
        }
        matched_changed.push(path.clone());
        for entry in matches.drain(..) {
            if affected.len() >= MAX_AFFECTED {
                truncated = true;
                break;
            }
            let mut item = json!({
                "path": entry.path,
                "entry": entry.name,
                "tag": entry.tag,
                "relocated": entry.relocated,
            });
            if let Some(function) = &entry.function {
                item["function"] = json!(function);
            }
            affected.push(item);
        }
        if truncated {
            break;
        }
    }
    let unmapped = changed
        .into_iter()
        .filter(|path| !matched_changed.contains(path))
        .collect::<Vec<_>>();
    json!({
        "provider": "aoci",
        "present": true,
        "root": map.root,
        "volumes": map.volumes.iter().map(|volume| json!({
            "kind": volume.kind,
            "path": volume.path,
            "format": volume.format,
        })).collect::<Vec<_>>(),
        "entries_total": map.entries.len(),
        "affected": affected,
        "unmapped_changed_files": unmapped,
        "truncated": truncated,
        "warnings": map.warnings,
    })
}

fn load(workspace: Option<&Path>) -> Map {
    let mut map = Map {
        root: None,
        volumes: Vec::new(),
        entries: Vec::new(),
        warnings: Vec::new(),
    };
    let Some(workspace) = workspace else {
        return map;
    };
    let root_path = workspace.join("aoci.txt");
    let mut code_path: Option<PathBuf> = None;
    if root_path.is_file() {
        map.root = Some("aoci.txt".to_string());
        match read_text(&root_path) {
            Ok(text) => {
                map.volumes = parse_root(&text);
                if let Some(volume) = map.volumes.iter().find(|volume| volume.kind == "code") {
                    code_path = Some(workspace.join(&volume.path));
                }
            }
            Err(warning) => map.warnings.push(warning),
        }
    }
    if code_path.is_none() {
        let fallback = workspace.join("aoci.code.txt");
        if fallback.is_file() {
            if map.root.is_none() {
                map.warnings
                    .push("aoci.txt not found; parsed aoci.code.txt directly".to_string());
            } else {
                map.warnings
                    .push("aoci.txt declares no code Volume; parsed aoci.code.txt".to_string());
            }
            code_path = Some(fallback);
        }
    }
    let Some(code_path) = code_path else {
        if map.root.is_some() {
            map.warnings
                .push("aoci.txt declares no readable code Volume".to_string());
        }
        return map;
    };
    let text = match read_text(&code_path) {
        Ok(text) => text,
        Err(warning) => {
            map.warnings.push(warning);
            return map;
        }
    };
    let workspace_text = workspace.to_string_lossy().replace('\\', "/");
    let (entries, warnings) = parse_code_volume(&text, &workspace_text);
    map.entries = entries;
    map.warnings.extend(warnings);
    map
}

fn read_text(path: &Path) -> Result<String, String> {
    let metadata =
        fs::metadata(path).map_err(|error| format!("cannot read {}: {error}", path.display()))?;
    if metadata.len() > MAX_VOLUME_BYTES {
        return Err(format!(
            "{} is larger than {} bytes; cognition map skipped",
            path.display(),
            MAX_VOLUME_BYTES
        ));
    }
    fs::read_to_string(path)
        .map_err(|error| format!("{} is not UTF-8 text: {error}", path.display()))
}

fn parse_root(text: &str) -> Vec<Volume> {
    let mut volumes = Vec::new();
    for line in text.lines() {
        let Some(rest) = line.trim().strip_prefix("#Volume:") else {
            continue;
        };
        let mut kind = None;
        let mut path = None;
        let mut format = None;
        for field in rest.split_whitespace() {
            if let Some((key, value)) = field.split_once('=') {
                match key {
                    "kind" => kind = Some(value.to_string()),
                    "path" => path = Some(value.to_string()),
                    "format" => format = Some(value.to_string()),
                    _ => {}
                }
            }
        }
        if let (Some(kind), Some(path)) = (kind, path) {
            volumes.push(Volume {
                kind,
                path,
                format: format.unwrap_or_default(),
            });
        }
    }
    volumes
}

fn parse_code_volume(text: &str, workspace: &str) -> (Vec<Entry>, Vec<String>) {
    let mut entries = Vec::new();
    let mut warnings = Vec::new();
    let mut section = String::new();
    let mut section_relocated = true;
    let mut malformed = 0usize;
    let mut absolute_sections = 0usize;
    for line in text.lines() {
        let line = line.trim_end_matches('\r').trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        if line.starts_with("===") && line.ends_with("===") {
            let inner = line.trim_matches('=').trim();
            let (directory, relocated) = section_directory(inner, workspace);
            if !relocated {
                absolute_sections += 1;
            }
            section = directory;
            section_relocated = relocated;
            continue;
        }
        let Some(bracket) = line.find('[') else {
            malformed += 1;
            continue;
        };
        let Some(close) = line[bracket..].find("]:").map(|index| bracket + index) else {
            malformed += 1;
            continue;
        };
        let name = line[..bracket].trim();
        if name.is_empty() {
            malformed += 1;
            continue;
        }
        let tag = line[bracket + 1..close].trim();
        let body = line[close + 2..].trim();
        let function = body
            .split('|')
            .map(str::trim)
            .find_map(|field| field.strip_prefix("F:"))
            .map(|value| truncate(value.trim(), FUNCTION_PREVIEW_CHARS))
            .filter(|value| !value.is_empty() && value != "-");
        let path = if section.is_empty() {
            name.to_string()
        } else {
            format!("{}/{}", section.trim_end_matches('/'), name)
        };
        if entries.len() >= MAX_ENTRIES {
            warnings.push(format!("cognition map stopped after {MAX_ENTRIES} entries"));
            break;
        }
        entries.push(Entry {
            path: if section_relocated {
                normalize_path(&path)
            } else {
                // Keep the foreign root visible; the suffix match still finds
                // the repository-relative change, and `relocated=false` says
                // the path is historical.
                path.replace('\\', "/")
            },
            name: name.to_string(),
            tag: tag.to_string(),
            function,
            relocated: section_relocated,
        });
    }
    if malformed > 0 {
        warnings.push(format!(
            "{malformed} line(s) did not match the AOCI Entry grammar"
        ));
    }
    if absolute_sections > 0 {
        warnings.push(format!(
            "{absolute_sections} section(s) use an absolute or foreign root; paths are matched by suffix"
        ));
    }
    (entries, warnings)
}

/// Resolve a directory section header to a repository-relative directory.
/// `/<repository>` is the neutral root newer AOCI writers use; a recorded
/// machine root is stripped when it equals this workspace, and left as a
/// suffix-matched absolute path otherwise.
fn section_directory(inner: &str, workspace: &str) -> (String, bool) {
    let directory = inner.trim();
    if let Some(rest) = directory.strip_prefix("/<repository>") {
        return (
            rest.trim_start_matches('/')
                .trim_end_matches('/')
                .to_string(),
            true,
        );
    }
    let normalized = directory.replace('\\', "/");
    let workspace = workspace.trim_end_matches('/');
    if let Some(rest) = normalized.strip_prefix(workspace) {
        return (
            rest.trim_start_matches('/')
                .trim_end_matches('/')
                .to_string(),
            true,
        );
    }
    // Legacy Windows drive roots: keep a normalized absolute path so the
    // suffix match can still find the repository-relative change.
    (normalized.trim_end_matches('/').to_string(), false)
}

fn entry_matches(entry_path: &str, changed_path: &str) -> bool {
    if entry_path == changed_path {
        return true;
    }
    entry_path.ends_with(&format!("/{changed_path}"))
}

fn normalize_paths(paths: &[String]) -> Vec<String> {
    paths.iter().map(|path| normalize_path(path)).collect()
}

fn normalize_path(path: &str) -> String {
    let normalized = path.replace('\\', "/");
    let normalized = normalized.strip_prefix("./").unwrap_or(&normalized);
    normalized.trim_start_matches('/').to_string()
}

fn truncate(value: &str, limit: usize) -> String {
    if value.chars().count() <= limit {
        return value.to_string();
    }
    let mut out = value
        .chars()
        .take(limit.saturating_sub(1))
        .collect::<String>();
    out.push('…');
    out
}
