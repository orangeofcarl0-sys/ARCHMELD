// Copyright 2026 ARCHMELD contributors
// SPDX-License-Identifier: Apache-2.0

//! Low-friction bootstrap helpers: default state/project discovery, MCP client
//! configuration rendering, agent rule installation and a real MCP handshake.
//!
//! Nothing here uploads source, writes a repository source file, or changes a
//! git ref. `--write` is always explicit and only touches the user's client
//! configuration or the requested agent-rule file.

use archmeld_core::hash;
use archmeld_core::sandbox::sanitize_id;
use serde_json::{json, Value};
use std::env;
use std::fs;
use std::io::{BufRead, BufReader, Write};
use std::path::{Component, Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::mpsc;
use std::time::Duration;

const RULES_AGENTS: &str = include_str!("../../../docs/agent-rules/AGENTS.md");
const RULES_CLAUDE: &str = include_str!("../../../docs/agent-rules/claude.md");
const RULE_BEGIN: &str = "<!-- ARCHMELD:BEGIN -->";
const RULE_END: &str = "<!-- ARCHMELD:END -->";

/// A user-level state root for one workspace. The path is derived from the
/// canonical workspace so two repos never share a ledger by accident.
pub fn default_state_root(workspace: &Path) -> Result<PathBuf, String> {
    let base = if let Some(value) = env::var_os("LOCALAPPDATA") {
        PathBuf::from(value).join("ARCHMELD")
    } else if let Some(value) = env::var_os("XDG_DATA_HOME") {
        PathBuf::from(value).join("archmeld")
    } else if let Some(value) = env::var_os("HOME") {
        PathBuf::from(value)
            .join(".local")
            .join("share")
            .join("archmeld")
    } else {
        return Err(
            "cannot determine a user data directory; pass --state <dir> explicitly".to_string(),
        );
    };
    let digest = hash::digest_bytes(workspace.to_string_lossy().as_bytes());
    let short = digest
        .strip_prefix("sha256:")
        .unwrap_or(&digest)
        .chars()
        .take(16)
        .collect::<String>();
    Ok(base.join("projects").join(short))
}

/// Stable, human-readable project id. Re-running init on the same workspace
/// keeps the id; moving the repo to another path creates a new project.
pub fn default_project_id(workspace: &Path) -> String {
    let name = workspace
        .file_name()
        .and_then(|value| value.to_str())
        .unwrap_or("workspace");
    let clean = sanitize_id(name);
    let clean = clean.trim_matches(['.', '-', '_']);
    let clean = if clean.is_empty() { "workspace" } else { clean };
    let clean: String = clean.chars().take(48).collect();
    let digest = hash::digest_bytes(workspace.to_string_lossy().as_bytes());
    let short = digest
        .strip_prefix("sha256:")
        .unwrap_or(&digest)
        .chars()
        .take(8)
        .collect::<String>();
    format!("project.{clean}.{short}")
}

/// The MCP binary that ships next to the current CLI. Falls back to a PATH
/// lookup so a source checkout still works.
pub fn mcp_binary_path() -> PathBuf {
    if let Some(value) = env::var_os("ARCHMELD_MCP_BINARY") {
        return PathBuf::from(value);
    }
    let name = if cfg!(windows) {
        "archmeld-mcp.exe"
    } else {
        "archmeld-mcp"
    };
    env::current_exe()
        .ok()
        .and_then(|exe| exe.parent().map(|dir| dir.join(name)))
        .filter(|path| path.exists())
        .unwrap_or_else(|| PathBuf::from(name))
}

fn quote(arg: &str) -> String {
    if arg.is_empty() || arg.contains([' ', '\t', '"']) {
        format!("\"{}\"", arg.replace('"', "\\\""))
    } else {
        arg.to_string()
    }
}

/// Render the exact client command and config snippet. This function never
/// writes anything; `execute_mcp_install` performs an explicit `--write`.
pub fn plan_mcp_install(client: &str, state: &Path, project: &str) -> Result<Value, String> {
    let binary = mcp_binary_path();
    let binary_text = binary.to_string_lossy().to_string();
    let state_text = state.to_string_lossy().to_string();
    let args = vec![
        "--state".to_string(),
        state_text.clone(),
        "--project".to_string(),
        project.to_string(),
    ];
    let (command, target, snippet, snippet_format) = match client {
        "claude" => {
            let mut command = vec![
                "claude".to_string(),
                "mcp".to_string(),
                "add".to_string(),
                "archmeld".to_string(),
                "--".to_string(),
                binary_text.clone(),
            ];
            command.extend(args.clone());
            (
                command,
                "Claude Code user scope via `claude mcp add` (or a project .mcp.json)".to_string(),
                json!({
                    "mcpServers": {
                        "archmeld": {"command": binary_text.clone(), "args": args.clone()}
                    }
                }),
                "json",
            )
        }
        "codex" => {
            let mut command = vec![
                "codex".to_string(),
                "mcp".to_string(),
                "add".to_string(),
                "archmeld".to_string(),
                "--".to_string(),
                binary_text.clone(),
            ];
            command.extend(args.clone());
            (
                command,
                "Codex user config via `codex mcp add`".to_string(),
                json!(format!(
                    "[mcp_servers.archmeld]\ncommand = \"{}\"\nargs = [{}]\n",
                    binary_text.replace('\\', "\\\\"),
                    args.iter()
                        .map(|arg| format!("\"{}\"", arg.replace('\\', "\\\\")))
                        .collect::<Vec<_>>()
                        .join(", ")
                )),
                "toml",
            )
        }
        "generic" => (
            Vec::new(),
            "Any MCP client: add this server entry".to_string(),
            json!({
                "mcpServers": {
                    "archmeld": {"command": binary_text.clone(), "args": args.clone()}
                }
            }),
            "json",
        ),
        other => {
            return Err(format!(
                "unknown client {other}; supported clients: claude, codex, generic"
            ))
        }
    };
    let command_line = command
        .iter()
        .map(|arg| quote(arg))
        .collect::<Vec<_>>()
        .join(" ");
    Ok(json!({
        "client": client,
        "binary": binary_text,
        "state": state_text,
        "project": project,
        "target": target,
        "command": command,
        "command_line": command_line,
        "snippet": snippet,
        "snippet_format": snippet_format,
        "read_only": true,
        "next_action": if command.is_empty() {
            json!("add the snippet to the client's MCP configuration")
        } else {
            json!(format!("re-run with --write to execute the shown command, or run it manually"))
        }
    }))
}

/// Execute an explicit `--write` plan. The client CLI is the only writer; we
/// never parse or rewrite a global config file ourselves.
pub fn execute_mcp_install(plan: &Value) -> Result<Value, String> {
    let command = plan["command"]
        .as_array()
        .ok_or_else(|| "install plan has no command".to_string())?
        .iter()
        .filter_map(|value| value.as_str().map(str::to_string))
        .collect::<Vec<_>>();
    if command.is_empty() {
        return Err("this client has no CLI; add the printed snippet manually".to_string());
    }
    let output = Command::new(&command[0])
        .args(&command[1..])
        .output()
        .map_err(|error| {
            format!(
                "cannot run {}: {error}; run the printed command manually",
                command[0]
            )
        })?;
    Ok(json!({
        "executed": true,
        "command": command,
        "exit_code": output.status.code(),
        "stdout": String::from_utf8_lossy(&output.stdout).trim(),
        "stderr": String::from_utf8_lossy(&output.stderr).trim(),
    }))
}

/// Install or preview the ARCHMELD rule block. Existing content is preserved;
/// the block is appended once and replaced in place on later runs.
pub fn install_rules(workspace: &Path, file_name: &str, write: bool) -> Result<Value, String> {
    let source = match file_name {
        "CLAUDE.md" => RULES_CLAUDE,
        "AGENTS.md" => RULES_AGENTS,
        other => return Err(format!("unsupported rule file {other}")),
    };
    let target = workspace.join(file_name);
    let existing = fs::read_to_string(&target).unwrap_or_default();
    let merged = merge_rule_block(&existing, source);
    if write {
        fs::write(&target, &merged)
            .map_err(|error| format!("cannot write {}: {error}", target.display()))?;
    }
    let mut result = json!({
        "path": target.to_string_lossy(),
        "written": write,
        "changed": merged != existing,
    });
    if !write {
        result["content"] = json!(merged);
    }
    Ok(result)
}

fn merge_rule_block(existing: &str, block: &str) -> String {
    let block = block.trim();
    if let (Some(start), Some(end)) = (existing.find(RULE_BEGIN), existing.find(RULE_END)) {
        if end > start {
            let mut merged = String::new();
            merged.push_str(&existing[..start]);
            merged.push_str(block);
            merged.push_str(&existing[end + RULE_END.len()..]);
            return merged;
        }
    }
    if existing.trim().is_empty() {
        format!("{block}\n")
    } else {
        format!("{}\n\n{block}\n", existing.trim_end())
    }
}

/// A real MCP stdio handshake: initialize, tools/list and one status call.
/// This catches the "indexed but no workspace root" class of first-run bugs
/// that a `--help` check cannot see.
pub fn health_check(state: &Path, project: &str) -> Result<Value, String> {
    health_check_with_binary(&mcp_binary_path(), state, project)
}

/// Same handshake as [`health_check`] with an explicit MCP binary. Used by the
/// packaged CLI and by integration tests that know the built binary path.
pub fn health_check_with_binary(
    binary: &Path,
    state: &Path,
    project: &str,
) -> Result<Value, String> {
    let mut child = Command::new(binary)
        .arg("--state")
        .arg(state)
        .arg("--project")
        .arg(project)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|error| format!("cannot start {}: {error}", binary.display()))?;
    let mut stdin = child
        .stdin
        .take()
        .ok_or_else(|| "MCP process has no stdin".to_string())?;
    let stdout = child
        .stdout
        .take()
        .ok_or_else(|| "MCP process has no stdout".to_string())?;
    let (sender, receiver) = mpsc::channel();
    std::thread::spawn(move || {
        for line in BufReader::new(stdout).lines().map_while(Result::ok) {
            if sender.send(line).is_err() {
                break;
            }
        }
    });
    let result = (|| -> Result<Value, String> {
        send_line(
            &mut stdin,
            json!({
                "jsonrpc": "2.0",
                "id": 1,
                "method": "initialize",
                "params": {
                    "protocolVersion": "2024-11-05",
                    "capabilities": {},
                    "clientInfo": {"name": "archmeld-init", "version": env!("CARGO_PKG_VERSION")}
                }
            }),
        )?;
        let initialized = read_response(&receiver, 1, Duration::from_secs(5))?;
        if initialized.get("error").is_some() {
            return Err(format!("MCP initialize failed: {}", initialized["error"]));
        }
        send_line(
            &mut stdin,
            json!({"jsonrpc": "2.0", "method": "notifications/initialized"}),
        )?;
        send_line(
            &mut stdin,
            json!({"jsonrpc": "2.0", "id": 2, "method": "tools/list", "params": {}}),
        )?;
        let tools = read_response(&receiver, 2, Duration::from_secs(5))?;
        if tools.get("error").is_some() {
            return Err(format!("MCP tools/list failed: {}", tools["error"]));
        }
        let names = tools["result"]["tools"]
            .as_array()
            .map(|list| {
                list.iter()
                    .filter_map(|tool| tool["name"].as_str().map(str::to_string))
                    .collect::<Vec<_>>()
            })
            .unwrap_or_default();
        if names.is_empty() {
            return Err("MCP tools/list returned no tools".to_string());
        }
        send_line(
            &mut stdin,
            json!({
                "jsonrpc": "2.0",
                "id": 3,
                "method": "tools/call",
                "params": {"name": "archmeld_status", "arguments": {}}
            }),
        )?;
        let status = read_response(&receiver, 3, Duration::from_secs(10))?;
        if status.get("error").is_some() || status["result"]["isError"].as_bool() == Some(true) {
            return Err(format!(
                "MCP archmeld_status failed: {}",
                status
                    .get("error")
                    .cloned()
                    .unwrap_or_else(|| status["result"]["content"][0]["text"].clone())
            ));
        }
        let text = status["result"]["content"][0]["text"]
            .as_str()
            .unwrap_or_default();
        let status_json: Value =
            serde_json::from_str(text).unwrap_or_else(|_| json!({"raw": text}));
        Ok(json!({
            "state": "ok",
            "binary": binary.to_string_lossy(),
            "project": project,
            "tools": names.len(),
            "tool_names": names,
            "initialize": initialized["result"]["serverInfo"],
            "status": status_json,
        }))
    })();
    let _ = child.kill();
    let _ = child.wait();
    result
}

fn send_line(stdin: &mut std::process::ChildStdin, value: Value) -> Result<(), String> {
    writeln!(stdin, "{value}").map_err(|error| format!("MCP write failed: {error}"))?;
    stdin
        .flush()
        .map_err(|error| format!("MCP flush failed: {error}"))
}

fn read_response(
    receiver: &mpsc::Receiver<String>,
    id: u64,
    timeout: Duration,
) -> Result<Value, String> {
    loop {
        let line = receiver
            .recv_timeout(timeout)
            .map_err(|_| format!("MCP response {id} timed out"))?;
        let value: Value = serde_json::from_str(&line)
            .map_err(|error| format!("MCP returned non-JSON: {error}"))?;
        if value["id"].as_u64() == Some(id) {
            return Ok(value);
        }
    }
}

/// Count candidate source files without reading their bytes. This is only a
/// preflight hint; the snapshot capture remains the authority on what is
/// indexed.
pub fn count_source_files(workspace: &Path, max_files: usize) -> Result<Value, String> {
    count_source_files_with(workspace, max_files, &[], &[], None)
}

/// Count files using the same include/exclude/size options the index helper
/// receives, so `init --dry-run` does not promise a different scope.
pub fn count_source_files_with(
    workspace: &Path,
    max_files: usize,
    include_dirs: &[String],
    exclude_dirs: &[String],
    max_file_size: Option<u64>,
) -> Result<Value, String> {
    let mut count = 0usize;
    let mut truncated = false;
    walk_files(
        workspace,
        max_files,
        include_dirs,
        exclude_dirs,
        max_file_size,
        &mut count,
        &mut truncated,
    )?;
    Ok(json!({
        "files": count,
        "truncated": truncated,
        "max_files": max_files,
    }))
}

fn walk_files(
    dir: &Path,
    max_files: usize,
    include_dirs: &[String],
    exclude_dirs: &[String],
    max_file_size: Option<u64>,
    count: &mut usize,
    truncated: &mut bool,
) -> Result<(), String> {
    if *truncated {
        return Ok(());
    }
    let entries = match fs::read_dir(dir) {
        Ok(entries) => entries,
        Err(error) => return Err(format!("cannot read {}: {error}", dir.display())),
    };
    for entry in entries {
        if *truncated {
            return Ok(());
        }
        let entry = entry.map_err(|error| format!("cannot read {}: {error}", dir.display()))?;
        let file_type = entry
            .file_type()
            .map_err(|error| format!("cannot stat {}: {error}", entry.path().display()))?;
        if file_type.is_symlink() {
            continue;
        }
        let name = entry.file_name();
        let name = name.to_string_lossy();
        if file_type.is_dir() {
            let included = include_dirs.iter().any(|dir| dir == name.as_ref());
            if !included
                && (matches!(
                    name.as_ref(),
                    ".git"
                        | ".hg"
                        | ".svn"
                        | "node_modules"
                        | "target"
                        | ".venv"
                        | "__pycache__"
                        | ".mypy_cache"
                        | ".pytest_cache"
                        | "dist"
                        | "build"
                ) || exclude_dirs.iter().any(|dir| dir == name.as_ref()))
            {
                continue;
            }
            walk_files(
                &entry.path(),
                max_files,
                include_dirs,
                exclude_dirs,
                max_file_size,
                count,
                truncated,
            )?;
        } else if file_type.is_file() {
            if let Some(limit) = max_file_size {
                if entry
                    .metadata()
                    .map(|metadata| metadata.len() > limit)
                    .unwrap_or(false)
                {
                    continue;
                }
            }
            *count += 1;
            if *count >= max_files {
                *truncated = true;
                return Ok(());
            }
        }
    }
    Ok(())
}

/// Return true when `child` is the same path as, or nested under, `parent`.
/// The state root must never be inside the repository being indexed.
pub fn state_inside_workspace(state: &Path, workspace: &Path) -> Result<bool, String> {
    let state = normalize_path(state)?;
    let workspace = normalize_path(workspace)?;
    let state = state.to_string_lossy().replace('\\', "/").to_lowercase();
    let state = state.strip_prefix("//?/").unwrap_or(&state).to_string();
    let workspace = workspace
        .to_string_lossy()
        .replace('\\', "/")
        .to_lowercase();
    let workspace = workspace
        .strip_prefix("//?/")
        .unwrap_or(&workspace)
        .to_string();
    Ok(state == workspace || state.starts_with(&format!("{workspace}/")))
}

fn normalize_path(path: &Path) -> Result<PathBuf, String> {
    if path.exists() {
        return path
            .canonicalize()
            .map_err(|error| format!("cannot resolve {}: {error}", path.display()));
    }
    let absolute = std::path::absolute(path)
        .map_err(|error| format!("cannot resolve {}: {error}", path.display()))?;
    let mut normalized = PathBuf::new();
    for component in absolute.components() {
        match component {
            Component::ParentDir => {
                normalized.pop();
            }
            Component::CurDir => {}
            other => normalized.push(other.as_os_str()),
        }
    }
    Ok(normalized)
}
