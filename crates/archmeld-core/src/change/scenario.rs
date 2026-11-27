// Copyright 2026 ARCHMELD contributors
// SPDX-License-Identifier: Apache-2.0

//! Controlled scenario execution with an explicit grant and a capture manifest.
//! No shell, bounded output, hard timeout, process-tree audit and cancellation.
//!
//! A scenario's `succeeded` state only means the process exited as requested;
//! it never means the business contract passed. Capture overhead is reported as
//! `unknown` unless a capture adapter measured it.

use crate::error::{CoreError, Result};
use crate::store::{validate, Store};
use serde::Deserialize;
use serde_json::{json, Map, Value};
use std::collections::BTreeMap;
use std::fs;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

const MAX_STREAM: usize = 4 * 1024 * 1024;
const PROCESS_AUDIT_INTERVAL: Duration = Duration::from_millis(500);
const ALLOWED_ENV: &[&str] = &[
    "PATH", "PATHEXT", "SYSTEMROOT", "SYSTEMDRIVE", "WINDIR", "TEMP", "TMP",
    "HOME", "USERPROFILE", "APPDATA", "LOCALAPPDATA", "NUMBER_OF_PROCESSORS",
];

#[derive(Debug, Clone)]
pub struct ExecutionGrant {
    pub granted: bool,
    pub grant_id: String,
    pub reason: String,
}

impl ExecutionGrant {
    pub fn grant(id: &str, reason: &str) -> Self {
        Self {
            granted: true,
            grant_id: id.into(),
            reason: reason.into(),
        }
    }
    pub fn deny(reason: &str) -> Self {
        Self {
            granted: false,
            grant_id: String::new(),
            reason: reason.into(),
        }
    }
}

#[derive(Debug, Clone)]
pub struct ScenarioSpec {
    pub project_id: String,
    pub run_id: String,
    pub snapshot_id: String,
    pub config_digest: String,
    pub argv: Vec<String>,
    /// Authorized workspace cwd used by the default `workspace` mode. A
    /// sandboxed run ignores it and reads the snapshot copy instead.
    pub cwd: PathBuf,
    pub env: Vec<(String, String)>,
    pub timeout: Duration,
    /// Optional isolation. `workspace_copy` materializes the captured snapshot
    /// and runs the command there; `container` additionally runs it in a
    /// digest-pinned local OCI image without network.
    pub sandbox: Option<ScenarioSandbox>,
}

/// Declaration of one scenario sandbox. The shape intentionally matches the
/// per-check sandbox plus a relative `cwd` inside the snapshot copy.
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ScenarioSandbox {
    /// `workspace`, `workspace_copy` or `container`.
    pub mode: String,
    #[serde(default)]
    pub include: Vec<String>,
    #[serde(default)]
    pub exclude: Vec<String>,
    #[serde(default)]
    pub image: Option<String>,
    #[serde(default)]
    pub network: Option<String>,
    /// Relative path inside the snapshot copy; defaults to its root.
    #[serde(default)]
    pub cwd: Option<String>,
}

#[derive(Debug, Clone)]
struct ProcessEntry {
    pid: u32,
    parent: u32,
    name: String,
}

#[derive(Debug, Clone)]
struct SeenProcess {
    pid: u32,
    parent: u32,
    name: String,
    first_seen_ms: i64,
}

#[derive(Default)]
struct ProcessAudit {
    available: bool,
    tool: String,
    error: Option<String>,
    processes: BTreeMap<u32, SeenProcess>,
    table: Vec<ProcessEntry>,
}

impl ProcessAudit {
    fn to_json(&self, survivors: &[u32], cleanup: &[Value]) -> Value {
        json!({
            "state": if self.available { "complete" } else { "unavailable" },
            "tool": self.tool,
            "error": self.error,
            "observed": self.processes.len(),
            "processes": self.processes.values().map(|process| json!({
                "pid": process.pid,
                "parent_pid": process.parent,
                "name": process.name,
                "first_seen_ms": process.first_seen_ms,
            })).collect::<Vec<_>>(),
            "survivors": survivors,
            "cleanup": cleanup,
            "method": cleanup_method(),
        })
    }

    fn refresh(&mut self, root: u32, started: Instant) {
        match collect_process_table() {
            Ok((table, tool)) => {
                self.available = true;
                if self.tool.is_empty() {
                    self.tool = tool;
                }
                let descendants = descendants(root, &table);
                let elapsed = started.elapsed().as_millis() as i64;
                for entry in descendants {
                    self.processes.entry(entry.pid).or_insert(SeenProcess {
                        pid: entry.pid,
                        parent: entry.parent,
                        name: entry.name,
                        first_seen_ms: elapsed,
                    });
                }
                self.table = table;
            }
            Err(error) => {
                if self.error.is_none() {
                    self.error = Some(error);
                }
            }
        }
    }

    fn survivors(&self, root: u32) -> Vec<u32> {
        if self.table.is_empty() {
            return Vec::new();
        }
        let live: BTreeMap<u32, &ProcessEntry> =
            self.table.iter().map(|entry| (entry.pid, entry)).collect();
        self.processes
            .values()
            .filter(|seen| {
                seen.pid != root
                    && live
                        .get(&seen.pid)
                        .is_some_and(|entry| entry.name == seen.name)
            })
            .map(|seen| seen.pid)
            .collect()
    }
}

pub fn run_scenario(
    store: &mut Store,
    spec: &ScenarioSpec,
    grant: &ExecutionGrant,
) -> Result<Value> {
    run_scenario_with_cancel(store, spec, grant, None)
}

/// Run a scenario and cooperatively cancel it when `cancel` flips to true. The
/// whole spawned process tree is terminated; a cancelled run is never reported
/// as complete.
///
/// With `sandbox.mode=workspace_copy` or `container` the command reads the
/// captured snapshot, not the live workspace. A container additionally has no
/// network, a read-only root filesystem and a writable `/tmp`; it is removed
/// even when the run times out or is cancelled.
pub struct PreparedScenario {
    spec: ScenarioSpec,
    grant: ExecutionGrant,
    exec_argv: Vec<String>,
    command_cwd: PathBuf,
    sandbox_detail: Value,
    target_env: Vec<(String, String)>,
    materialized: Option<crate::sandbox::Materialized>,
    container: Option<crate::sandbox::ContainerRun>,
}

pub struct ScenarioOutcome {
    spec: ScenarioSpec,
    grant: ExecutionGrant,
    capture: CommandCapture,
    sandbox_detail: Value,
    env_names: Vec<String>,
    workdir: String,
}

/// Validate a scenario and materialize its sandbox. This is the only part that
/// needs the Store; execution and finalization are split so a long child
/// process never holds the global ledger mutex.
pub fn prepare_scenario(
    store: &mut Store,
    spec: ScenarioSpec,
    grant: ExecutionGrant,
) -> Result<PreparedScenario> {
    validate::require_id("project_id", &spec.project_id)?;
    validate::require_id("run_id", &spec.run_id)?;
    validate::require_digest("snapshot_id", &spec.snapshot_id)?;
    store.check_snapshot_scope(&spec.project_id, &spec.snapshot_id)?;
    if spec.argv.is_empty() || spec.argv[0].is_empty() {
        return Err(CoreError::invalid("scenario argv must not be empty"));
    }
    if !grant.granted {
        return Err(CoreError::Unsupported(format!(
            "permission_denied: scenario execution refused: {}",
            grant.reason
        )));
    }
    let mode = spec
        .sandbox
        .as_ref()
        .map(|sandbox| sandbox.mode.as_str())
        .unwrap_or("workspace");
    let request = spec
        .sandbox
        .as_ref()
        .map(|sandbox| crate::sandbox::SandboxRequest {
            include: sandbox.include.clone(),
            exclude: sandbox.exclude.clone(),
            image: sandbox.image.clone(),
            network: sandbox.network.clone(),
            cwd: sandbox.cwd.clone(),
        });
    if let Some(request) = &request {
        request.validate(mode)?;
    }
    let scope_project = crate::sandbox::sanitize_id(&spec.project_id);
    let scope_owner = crate::sandbox::sanitize_id(&spec.run_id);
    let scope = [
        "scenario-sandboxes",
        scope_project.as_str(),
        scope_owner.as_str(),
    ];
    let mut materialized: Option<crate::sandbox::Materialized> = None;
    let mut container: Option<crate::sandbox::ContainerRun> = None;
    let (exec_argv, command_cwd, sandbox_detail, target_env) = match (mode, request) {
        ("workspace_copy", Some(request)) => {
            let sandbox = crate::sandbox::materialize(
                store,
                &spec.project_id,
                &spec.snapshot_id,
                &scope,
                &request,
            )?;
            let mut detail = sandbox.detail.clone();
            detail["mode"] = json!("workspace_copy");
            detail["declared"] = json!(true);
            detail["isolated"] = json!(true);
            detail["enforcement"] = json!("materialized_snapshot_copy");
            detail["limitations"] = json!([
                "the process can still address absolute paths outside the sandbox copy; OS-level confinement is not applied"
            ]);
            let cwd = sandbox.cwd.clone();
            materialized = Some(sandbox);
            (spec.argv.clone(), cwd, detail, spec.env.clone())
        }
        ("container", Some(request)) => {
            let (sandbox, running, argv) = crate::sandbox::prepare_container(
                store,
                &spec.project_id,
                &spec.snapshot_id,
                &scope,
                &request,
                &spec.env,
                &spec.argv,
            )?;
            let mut detail = sandbox.detail.clone();
            detail["mode"] = json!("container");
            detail["declared"] = json!(true);
            detail["isolated"] = json!(true);
            detail["enforcement"] = json!("oci_container");
            detail["network"] = json!("none");
            detail["read_only"] = json!(true);
            detail["tmpfs"] = json!("/tmp:rw,size=64m");
            detail["image"] = json!(running.image);
            detail["image_id"] = json!(running.image_id);
            detail["repo_digests"] = json!(running.repo_digests);
            detail["platform"] = json!(running.platform);
            detail["workdir"] = json!(running.workdir);
            detail["container_name"] = json!(running.name);
            detail["exec_argv"] = json!(crate::sandbox::redact_exec_argv(&argv));
            detail["limitations"] = json!([
                "Docker/Linux containers only; the host must provide a local pinned image (no pull)",
                "the process runs as the image default user and can write the bind-mounted snapshot copy",
                "the container is removed by name after the run, including timeout and cancellation",
                "container processes are not visible to the host process audit; the docker client is audited"
            ]);
            let host_cwd = sandbox.root.clone();
            materialized = Some(sandbox);
            container = Some(running);
            (argv, host_cwd, detail, Vec::new())
        }
        _ => {
            if !spec.cwd.is_dir() {
                return Err(CoreError::invalid(format!(
                    "cwd {} is not a directory",
                    spec.cwd.display()
                )));
            }
            (
                spec.argv.clone(),
                spec.cwd.clone(),
                json!({
                    "mode": "workspace",
                    "declared": spec.sandbox.is_some(),
                    "isolated": false,
                    "enforcement": "none",
                    "limitations": ["the process runs directly on the host with the caller's workspace access"],
                }),
                spec.env.clone(),
            )
        }
    };
    Ok(PreparedScenario {
        spec,
        grant,
        exec_argv,
        command_cwd,
        sandbox_detail,
        target_env,
        materialized,
        container,
    })
}

/// Run the prepared command without touching the Store. Cleanup of containers
/// and snapshot copies is part of this phase.
pub fn execute_prepared(
    prepared: PreparedScenario,
    cancel: Option<&AtomicBool>,
) -> Result<ScenarioOutcome> {
    let PreparedScenario {
        spec,
        grant,
        exec_argv,
        command_cwd,
        mut sandbox_detail,
        target_env,
        materialized,
        container,
    } = prepared;
    let capture =
        match run_command_capture(&exec_argv, &command_cwd, &target_env, spec.timeout, cancel) {
            Ok(capture) => capture,
            Err(error) => {
                if let Some(running) = &container {
                    running.removed();
                }
                if let Some(sandbox) = &materialized {
                    let _ = fs::remove_dir_all(&sandbox.root);
                }
                return Err(error);
            }
        };
    let is_container = container.is_some();
    let container_removed = container.as_ref().map(|running| running.removed());
    if let (Some(running), Some(removed)) = (&container, container_removed) {
        sandbox_detail["container"] = running.detail(removed);
    }
    let succeeded = capture.completion == "succeeded";
    let retained = materialized.is_some() && !succeeded;
    if let Some(sandbox) = &materialized {
        if !retained {
            let _ = fs::remove_dir_all(&sandbox.root);
        }
    }
    sandbox_detail["retained"] = json!(retained);
    let env_names = if is_container {
        spec.env.iter().map(|(name, _)| name.clone()).collect()
    } else {
        capture.env_names.clone()
    };
    let workdir = container
        .as_ref()
        .map(|running| running.workdir.clone())
        .unwrap_or_else(|| command_cwd.to_string_lossy().to_string());
    Ok(ScenarioOutcome {
        spec,
        grant,
        capture,
        sandbox_detail,
        env_names,
        workdir,
    })
}

/// Same as [`execute_prepared`] with an owned cancellation flag, suitable for a
/// blocking worker or another thread.
pub fn execute_prepared_owned(
    prepared: PreparedScenario,
    cancel: Option<Arc<AtomicBool>>,
) -> Result<ScenarioOutcome> {
    execute_prepared(prepared, cancel.as_deref())
}

/// Persist the captured process as a scenario run. Only this short phase takes
/// the Store.
pub fn finalize_scenario(store: &mut Store, outcome: ScenarioOutcome) -> Result<Value> {
    let ScenarioOutcome {
        spec,
        grant,
        capture,
        sandbox_detail,
        env_names,
        workdir,
    } = outcome;
    let stdout_digest = store.put_blob(&capture.stdout, "text/plain")?;
    let stderr_digest = store.put_blob(&capture.stderr, "text/plain")?;
    let completion = capture.completion;
    let manifest = json!({
        "project_id": spec.project_id, "run_id": spec.run_id,
        "snapshot_id": spec.snapshot_id, "config_digest": spec.config_digest,
        "argv": spec.argv, "cwd": workdir,
        "env_names": env_names, "grant_id": grant.grant_id, "grant_reason": grant.reason,
        "pid": capture.pid,
        "started_ms": capture.started_ms, "finished_ms": capture.finished_ms,
        "exit_code": capture.exit_code, "timed_out": capture.timed_out, "cancelled": capture.cancelled,
        "completion": completion,
        "coverage": {"state": if completion == "succeeded" { "complete" } else { "partial" },
                     "scope": "scenario process exit, output capture and process-tree audit",
                     "limitations": if completion == "succeeded" { vec![] } else { vec![format!("completion={completion}")] }},
        "sandbox": sandbox_detail,
        "process_audit": capture.process_audit,
        "overhead": {
            "state": "unknown",
            "reason": "the scenario runner does not instrument the target; compare with a capture-enabled run before claiming a cost",
            "wall_ms": capture.wall_ms,
        },
        "stdout": {"digest": stdout_digest, "size": capture.stdout.len(), "truncated": capture.stdout_truncated},
        "stderr": {"digest": stderr_digest, "size": capture.stderr.len(), "truncated": capture.stderr_truncated},
    });
    store.conn.execute(
        "INSERT INTO scenario_runs (project_id,run_id,snapshot_id,config_digest,manifest_json,exit_code,timed_out,stdout_digest,stderr_digest,created_at)
         VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9,?10)",
        (&spec.project_id, &spec.run_id, &spec.snapshot_id, &spec.config_digest, manifest.to_string(),
         capture.exit_code, if capture.timed_out {1} else {0}, &stdout_digest, &stderr_digest, crate::store::now_unix()))?;
    Ok(manifest)
}

pub fn run_scenario_with_cancel(
    store: &mut Store,
    spec: &ScenarioSpec,
    grant: &ExecutionGrant,
    cancel: Option<&AtomicBool>,
) -> Result<Value> {
    let prepared = prepare_scenario(store, spec.clone(), grant.clone())?;
    let outcome = execute_prepared(prepared, cancel)?;
    finalize_scenario(store, outcome)
}

/// Read a previously recorded scenario run by project and run id.
pub fn get_scenario_run(store: &Store, project_id: &str, run_id: &str) -> Result<Value> {
    validate::require_id("project_id", project_id)?;
    validate::require_id("run_id", run_id)?;
    let text: Option<String> = store
        .conn
        .query_row(
            "SELECT manifest_json FROM scenario_runs WHERE project_id=?1 AND run_id=?2",
            (project_id, run_id),
            |row| row.get(0),
        )
        .ok();
    let Some(text) = text else {
        return Err(CoreError::not_found(format!(
            "scenario run {run_id} not found"
        )));
    };
    serde_json::from_str(&text)
        .map_err(|err| CoreError::Storage(format!("scenario manifest corrupt: {err}")))
}

/// Byte-level command capture for the verification engine. It shares the
/// scenario runner's no-shell, bounded-output, timeout and process-tree audit
/// rules, but performs no scenario bookkeeping.
#[derive(Debug)]
pub struct CommandCapture {
    pub pid: u32,
    pub exit_code: Option<i32>,
    pub timed_out: bool,
    pub cancelled: bool,
    pub completion: &'static str,
    pub stdout: Vec<u8>,
    pub stderr: Vec<u8>,
    pub stdout_truncated: bool,
    pub stderr_truncated: bool,
    pub process_audit: Value,
    pub env_names: Vec<String>,
    pub started_ms: i64,
    pub finished_ms: i64,
    pub wall_ms: i64,
}

pub fn run_command_capture(
    argv: &[String],
    cwd: &Path,
    env: &[(String, String)],
    timeout: Duration,
    cancel: Option<&AtomicBool>,
) -> Result<CommandCapture> {
    if argv.is_empty() || argv[0].is_empty() {
        return Err(CoreError::invalid("command argv must not be empty"));
    }
    if !cwd.is_dir() {
        return Err(CoreError::invalid(format!(
            "cwd {} is not a directory",
            cwd.display()
        )));
    }
    let started = SystemTime::now();
    let started_instant = Instant::now();
    let mut command = Command::new(&argv[0]);
    command
        .args(&argv[1..])
        .current_dir(cwd)
        .env_clear()
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        command.process_group(0);
    }
    let mut env_names = Vec::new();
    for name in ALLOWED_ENV {
        if let Some(value) = std::env::var_os(name) {
            command.env(name, value);
            env_names.push((*name).to_string());
        }
    }
    for (name, value) in env {
        if name.is_empty() || name.contains('=') {
            return Err(CoreError::invalid("invalid env name"));
        }
        command.env(name, value);
        env_names.push(name.clone());
    }
    let mut child = command
        .spawn()
        .map_err(|err| CoreError::Storage(format!("spawn failed: {err}")))?;
    let root_pid = child.id();
    let stdout = child
        .stdout
        .take()
        .ok_or_else(|| CoreError::Storage("stdout unavailable".into()))?;
    let stderr = child
        .stderr
        .take()
        .ok_or_else(|| CoreError::Storage("stderr unavailable".into()))?;
    let out_thread = std::thread::spawn(move || read_stream(stdout));
    let err_thread = std::thread::spawn(move || read_stream(stderr));
    let deadline = Instant::now() + timeout;
    let mut audit = ProcessAudit::default();
    let mut last_audit = Instant::now() - PROCESS_AUDIT_INTERVAL;
    let mut timed_out = false;
    let mut cancelled = false;
    let mut cleanup = Vec::new();
    let exit_code;
    loop {
        match child.try_wait()? {
            Some(status) => {
                exit_code = status.code();
                break;
            }
            None if cancel.is_some_and(|flag| flag.load(Ordering::SeqCst)) => {
                cancelled = true;
                audit.refresh(root_pid, started_instant);
                cleanup.extend(terminate_tree(root_pid, &mut child, &mut audit));
                exit_code = child.wait()?.code();
                break;
            }
            None if Instant::now() >= deadline => {
                timed_out = true;
                audit.refresh(root_pid, started_instant);
                cleanup.extend(terminate_tree(root_pid, &mut child, &mut audit));
                exit_code = child.wait()?.code();
                break;
            }
            None => {
                if last_audit.elapsed() >= PROCESS_AUDIT_INTERVAL {
                    audit.refresh(root_pid, started_instant);
                    last_audit = Instant::now();
                }
                std::thread::sleep(Duration::from_millis(50));
            }
        }
    }
    if cancelled || timed_out {
        std::thread::sleep(Duration::from_millis(150));
    }
    audit.refresh(root_pid, started_instant);
    let survivors = audit.survivors(root_pid);
    let (stdout_bytes, stdout_truncated) = out_thread
        .join()
        .map_err(|_| CoreError::Storage("stdout thread panicked".into()))?;
    let (stderr_bytes, stderr_truncated) = err_thread
        .join()
        .map_err(|_| CoreError::Storage("stderr thread panicked".into()))?;
    let finished = SystemTime::now();
    let completion = if cancelled {
        "cancelled"
    } else if timed_out {
        "timed_out"
    } else if exit_code == Some(0) {
        "succeeded"
    } else {
        "failed"
    };
    Ok(CommandCapture {
        pid: root_pid,
        exit_code,
        timed_out,
        cancelled,
        completion,
        stdout: stdout_bytes,
        stderr: stderr_bytes,
        stdout_truncated,
        stderr_truncated,
        process_audit: audit.to_json(&survivors, &cleanup),
        env_names,
        started_ms: unix_ms(started),
        finished_ms: unix_ms(finished),
        wall_ms: started_instant.elapsed().as_millis() as i64,
    })
}

fn terminate_tree(
    root_pid: u32,
    child: &mut std::process::Child,
    audit: &mut ProcessAudit,
) -> Vec<Value> {
    let mut results = Vec::new();
    #[cfg(windows)]
    {
        let status = Command::new("taskkill")
            .args(["/PID", &root_pid.to_string(), "/T", "/F"])
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status();
        results.push(json!({
            "pid": root_pid,
            "method": "taskkill /T /F",
            "result": match status {
                Ok(status) if status.success() => "terminated".to_string(),
                Ok(status) => format!("taskkill_exit_{:?}", status.code()),
                Err(error) => format!("taskkill_error: {error}"),
            }
        }));
    }
    #[cfg(unix)]
    {
        let status = Command::new("kill")
            .args(["-KILL", "--", &format!("-{root_pid}")])
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status();
        results.push(json!({
            "pid": root_pid,
            "method": "kill -KILL process group",
            "result": match status {
                Ok(status) if status.success() => "terminated".to_string(),
                Ok(status) => format!("kill_exit_{:?}", status.code()),
                Err(error) => format!("kill_error: {error}"),
            }
        }));
    }
    if !cfg!(windows) && !cfg!(unix) {
        let _ = child.kill();
        results.push(json!({"pid": root_pid, "method": "child.kill", "result": "requested"}));
    }
    audit.refresh(root_pid, Instant::now());
    results
}

fn cleanup_method() -> &'static str {
    #[cfg(windows)]
    {
        "taskkill /PID <root> /T /F plus live process-tree audit"
    }
    #[cfg(unix)]
    {
        "kill -KILL -- -<process-group> plus live process-tree audit"
    }
    #[cfg(not(any(windows, unix)))]
    {
        "child.kill"
    }
}

fn descendants(root: u32, table: &[ProcessEntry]) -> Vec<ProcessEntry> {
    let mut children: BTreeMap<u32, Vec<&ProcessEntry>> = BTreeMap::new();
    for entry in table {
        children.entry(entry.parent).or_default().push(entry);
    }
    let mut result = Vec::new();
    let mut queue = vec![root];
    let mut visited = std::collections::BTreeSet::new();
    visited.insert(root);
    while let Some(parent) = queue.pop() {
        if let Some(entries) = children.get(&parent) {
            for entry in entries {
                if visited.insert(entry.pid) {
                    result.push((*entry).clone());
                    queue.push(entry.pid);
                }
            }
        }
    }
    result
}

#[cfg(windows)]
fn collect_process_table() -> std::result::Result<(Vec<ProcessEntry>, String), String> {
    match win32::process_table() {
        Ok(table) => Ok(table),
        Err(native_error) => collect_process_table_powershell()
            .map_err(|fallback_error| format!("{native_error}; fallback: {fallback_error}")),
    }
}

#[cfg(windows)]
fn collect_process_table_powershell() -> std::result::Result<(Vec<ProcessEntry>, String), String> {
    let output = Command::new("powershell.exe")
        .args([
            "-NoProfile",
            "-NonInteractive",
            "-Command",
            "Get-CimInstance Win32_Process | Select-Object ProcessId,ParentProcessId,Name | ConvertTo-Json -Compress",
        ])
        .stdin(Stdio::null())
        .output()
        .map_err(|error| format!("powershell spawn failed: {error}"))?;
    if !output.status.success() {
        return Err(format!(
            "powershell process query exited {:?}: {}",
            output.status.code(),
            String::from_utf8_lossy(&output.stderr).trim()
        ));
    }
    let text = String::from_utf8_lossy(&output.stdout);
    let text = text.trim();
    if text.is_empty() {
        return Err("process query returned no data".to_string());
    }
    let value: Value =
        serde_json::from_str(text).map_err(|error| format!("process query JSON: {error}"))?;
    let values = match value {
        Value::Array(values) => values,
        other => vec![other],
    };
    let mut table = Vec::with_capacity(values.len());
    for value in values {
        let (Some(pid), Some(parent), Some(name)) = (
            value["ProcessId"].as_u64(),
            value["ParentProcessId"].as_u64(),
            value["Name"].as_str(),
        ) else {
            continue;
        };
        table.push(ProcessEntry {
            pid: pid as u32,
            parent: parent as u32,
            name: name.to_string(),
        });
    }
    Ok((table, "powershell Get-CimInstance Win32_Process".to_string()))
}

#[cfg(windows)]
mod win32 {
    use super::ProcessEntry;
    use std::ffi::c_void;

    const TH32CS_SNAPPROCESS: u32 = 0x0000_0002;
    const INVALID_HANDLE_VALUE: *mut c_void = -1isize as *mut c_void;

    #[repr(C)]
    struct ProcessEntry32W {
        dw_size: u32,
        cnt_usage: u32,
        th32_process_id: u32,
        th32_default_heap_id: usize,
        th32_module_id: u32,
        cnt_threads: u32,
        th32_parent_process_id: u32,
        pc_pri_class_base: i32,
        dw_flags: u32,
        sz_exe_file: [u16; 260],
    }

    #[link(name = "kernel32")]
    unsafe extern "system" {
        fn CreateToolhelp32Snapshot(flags: u32, process_id: u32) -> *mut c_void;
        fn Process32FirstW(snapshot: *mut c_void, entry: *mut ProcessEntry32W) -> i32;
        fn Process32NextW(snapshot: *mut c_void, entry: *mut ProcessEntry32W) -> i32;
        fn CloseHandle(handle: *mut c_void) -> i32;
    }

    pub(super) fn process_table() -> Result<(Vec<ProcessEntry>, String), String> {
        // SAFETY: the snapshot handle is checked against INVALID_HANDLE_VALUE,
        // the entry is a correctly sized POD structure, and the handle is
        // closed on every return path.
        unsafe {
            let snapshot = CreateToolhelp32Snapshot(TH32CS_SNAPPROCESS, 0);
            if snapshot == INVALID_HANDLE_VALUE {
                return Err(format!(
                    "CreateToolhelp32Snapshot failed: {}",
                    std::io::Error::last_os_error()
                ));
            }
            let mut entry: ProcessEntry32W = std::mem::zeroed();
            entry.dw_size = std::mem::size_of::<ProcessEntry32W>() as u32;
            let mut table = Vec::new();
            let mut ok = Process32FirstW(snapshot, &mut entry);
            while ok != 0 {
                let end = entry
                    .sz_exe_file
                    .iter()
                    .position(|value| *value == 0)
                    .unwrap_or(entry.sz_exe_file.len());
                table.push(ProcessEntry {
                    pid: entry.th32_process_id,
                    parent: entry.th32_parent_process_id,
                    name: String::from_utf16_lossy(&entry.sz_exe_file[..end]),
                });
                ok = Process32NextW(snapshot, &mut entry);
            }
            let error = std::io::Error::last_os_error();
            CloseHandle(snapshot);
            if table.is_empty() {
                return Err(format!("Toolhelp32 returned no processes: {error}"));
            }
            Ok((table, "Toolhelp32 CreateToolhelp32Snapshot".to_string()))
        }
    }
}

#[cfg(not(windows))]
fn collect_process_table() -> std::result::Result<(Vec<ProcessEntry>, String), String> {
    let output = Command::new("ps")
        .args(["-eo", "pid=,ppid=,comm="])
        .stdin(Stdio::null())
        .output()
        .map_err(|error| format!("ps spawn failed: {error}"))?;
    if !output.status.success() {
        return Err(format!("ps exited {:?}", output.status.code()));
    }
    let mut table = Vec::new();
    for line in String::from_utf8_lossy(&output.stdout).lines() {
        let mut parts = line.split_whitespace();
        let (Some(pid), Some(parent), Some(name)) = (parts.next(), parts.next(), parts.next())
        else {
            continue;
        };
        let (Ok(pid), Ok(parent)) = (pid.parse::<u32>(), parent.parse::<u32>()) else {
            continue;
        };
        table.push(ProcessEntry {
            pid,
            parent,
            name: name.to_string(),
        });
    }
    Ok((table, "ps -eo pid=,ppid=,comm=".to_string()))
}

fn read_stream(mut stream: impl Read) -> (Vec<u8>, bool) {
    let mut kept = Vec::new();
    let mut buffer = [0u8; 8192];
    let mut truncated = false;
    loop {
        match stream.read(&mut buffer) {
            Ok(0) | Err(_) => break,
            Ok(n) => {
                if kept.len() < MAX_STREAM {
                    let remaining = MAX_STREAM - kept.len();
                    kept.extend_from_slice(&buffer[..n.min(remaining)]);
                }
                if kept.len() >= MAX_STREAM {
                    truncated = true;
                }
            }
        }
    }
    (kept, truncated)
}

fn unix_ms(t: SystemTime) -> i64 {
    t.duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}

#[allow(dead_code)]
fn empty_attributes() -> Map<String, Value> {
    Map::new()
}
