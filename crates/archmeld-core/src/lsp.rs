// Copyright 2026 ARCHMELD contributors
// SPDX-License-Identifier: Apache-2.0

//! Real LSP process round-trip with explicit spawn permission and an audit log
//! (AT023, AT024).
//!
//! Starting a language server executes external code, so it is refused unless
//! the caller presents an explicit grant. The session records the spawn argv,
//! cwd, environment *names* (never values), pid, request methods, shutdown exit
//! status and any surviving descendant pids. Only a process tree this session
//! spawned is ever terminated.

use crate::error::{CoreError, Result};
use serde_json::{json, Value};
use std::io::{BufRead, BufReader, Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Child, ChildStdin, ChildStdout, Command, Stdio};
use std::sync::atomic::{AtomicI64, Ordering};
use std::sync::{mpsc, Arc, Mutex};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

/// Environment variables forwarded to a language server. Values are never put
/// into the audit log; only names are recorded.
const ALLOWED_ENV: &[&str] = &[
    "PATH",
    "PATHEXT",
    "SYSTEMROOT",
    "SYSTEMDRIVE",
    "WINDIR",
    "TEMP",
    "TMP",
    "HOME",
    "USERPROFILE",
    "APPDATA",
    "LOCALAPPDATA",
    "RUSTUP_HOME",
    "CARGO_HOME",
    "NUMBER_OF_PROCESSORS",
    "PROGRAMDATA",
];

pub struct LspServerSpec {
    pub id: String,
    pub program: PathBuf,
    pub args: Vec<String>,
    pub cwd: PathBuf,
    pub env: Vec<(String, String)>,
    /// Optional `initializationOptions` payload passed to the server.
    pub initialization_options: Option<Value>,
}

pub struct SpawnPermission {
    pub granted: bool,
    pub grant_id: String,
    pub reason: String,
}

impl SpawnPermission {
    pub fn grant(grant_id: impl Into<String>, reason: impl Into<String>) -> Self {
        Self {
            granted: true,
            grant_id: grant_id.into(),
            reason: reason.into(),
        }
    }

    pub fn deny(reason: impl Into<String>) -> Self {
        Self {
            granted: false,
            grant_id: String::new(),
            reason: reason.into(),
        }
    }
}

#[derive(Debug, Clone)]
pub struct AuditSummary {
    pub events: u64,
    pub exit_code: Option<i32>,
    pub orphan_pids: Vec<u32>,
    pub descendants_seen: Vec<u32>,
    pub audit_available: bool,
}

pub struct LspSession {
    child: Child,
    stdin: Arc<Mutex<ChildStdin>>,
    rx: mpsc::Receiver<Value>,
    next_id: AtomicI64,
    pub pid: u32,
    audit_path: PathBuf,
    session_id: String,
    events: u64,
    stderr_log: PathBuf,
    initialization_options: Option<Value>,
}

impl LspSession {
    pub fn start(
        spec: &LspServerSpec,
        permission: &SpawnPermission,
        audit_path: &Path,
    ) -> Result<Self> {
        if !spec.program.is_file() {
            return Err(CoreError::not_found(format!(
                "language server binary {} not found",
                spec.program.display()
            )));
        }
        if !spec.cwd.is_dir() {
            return Err(CoreError::invalid(format!(
                "language server cwd {} is not a directory",
                spec.cwd.display()
            )));
        }
        let mut audit = Audit::new(audit_path);
        if !permission.granted {
            audit.record(
                "spawn_denied",
                json!({
                    "server": spec.id,
                    "program": spec.program.to_string_lossy(),
                    "reason": permission.reason,
                    "note": "language-server execution requires an explicit grant",
                }),
            )?;
            return Err(CoreError::Unsupported(format!(
                "permission_denied: LSP spawn refused: {}",
                permission.reason
            )));
        }

        let mut command = Command::new(&spec.program);
        command
            .args(&spec.args)
            .current_dir(&spec.cwd)
            .env_clear()
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        let mut env_names = Vec::new();
        for name in ALLOWED_ENV {
            if let Some(value) = std::env::var_os(name) {
                command.env(name, value);
                env_names.push((*name).to_string());
            }
        }
        for (name, value) in &spec.env {
            if name.is_empty() || name.contains('=') {
                return Err(CoreError::invalid(format!("invalid env name {name:?}")));
            }
            command.env(name, value);
            env_names.push(name.clone());
        }
        let mut child = command.spawn().map_err(|err| {
            CoreError::Storage(format!(
                "failed to spawn language server {}: {err}",
                spec.program.display()
            ))
        })?;
        let pid = child.id();
        let stdin = child
            .stdin
            .take()
            .ok_or_else(|| CoreError::Storage("LSP stdin unavailable".to_string()))?;
        let stdout = child
            .stdout
            .take()
            .ok_or_else(|| CoreError::Storage("LSP stdout unavailable".to_string()))?;
        let stderr = child.stderr.take();
        let stdin = Arc::new(Mutex::new(stdin));
        let (tx, rx) = mpsc::channel();
        let reader_writer = Arc::clone(&stdin);
        std::thread::Builder::new()
            .name("archmeld-lsp-reader".into())
            .spawn(move || read_loop(stdout, reader_writer, tx))
            .map_err(|err| CoreError::Storage(format!("LSP reader thread: {err}")))?;
        let stderr_log = audit_path.with_extension("stderr.log");
        if let Some(stderr) = stderr {
            let log = stderr_log.clone();
            std::thread::Builder::new()
                .name("archmeld-lsp-stderr".into())
                .spawn(move || capture_stderr(stderr, &log))
                .map_err(|err| CoreError::Storage(format!("LSP stderr thread: {err}")))?;
        }
        audit.record(
            "spawn",
            json!({
                "server": spec.id,
                "program": spec.program.to_string_lossy(),
                "args": spec.args,
                "cwd": spec.cwd.to_string_lossy(),
                "pid": pid,
                "grant_id": permission.grant_id,
                "reason": permission.reason,
                "env_names": env_names,
            }),
        )?;
        Ok(Self {
            child,
            stdin,
            rx,
            next_id: AtomicI64::new(1),
            pid,
            audit_path: audit_path.to_path_buf(),
            session_id: spec.id.clone(),
            events: audit.events,
            stderr_log,
            initialization_options: spec.initialization_options.clone(),
        })
    }

    fn send(&self, value: &Value) -> Result<()> {
        let body = serde_json::to_vec(value)
            .map_err(|err| CoreError::Storage(format!("LSP encode: {err}")))?;
        let mut guard = self
            .stdin
            .lock()
            .map_err(|_| CoreError::Storage("LSP stdin lock poisoned".to_string()))?;
        write!(guard, "Content-Length: {}\r\n\r\n", body.len())?;
        guard.write_all(&body)?;
        guard.flush()?;
        Ok(())
    }

    pub fn request(&self, method: &str, params: Value, timeout: Duration) -> Result<Value> {
        let id = self.next_id.fetch_add(1, Ordering::SeqCst);
        self.send(&json!({
            "jsonrpc": "2.0",
            "id": id,
            "method": method,
            "params": params,
        }))?;
        let deadline = Instant::now() + timeout;
        loop {
            let remaining = deadline.saturating_duration_since(Instant::now());
            if remaining.is_zero() {
                return Err(CoreError::Unsupported(format!(
                    "tool_unavailable: LSP request {method} timed out"
                )));
            }
            let message = self.rx.recv_timeout(remaining).map_err(|_| {
                CoreError::Unsupported(format!(
                    "tool_unavailable: language server closed while waiting for {method}"
                ))
            })?;
            if message["id"].as_i64() == Some(id) {
                if let Some(error) = message.get("error") {
                    return Err(CoreError::Unsupported(format!(
                        "tool_unavailable: LSP {method} failed: {error}"
                    )));
                }
                return Ok(message["result"].clone());
            }
        }
    }

    pub fn notify(&self, method: &str, params: Value) -> Result<()> {
        self.send(&json!({
            "jsonrpc": "2.0",
            "method": method,
            "params": params,
        }))
    }

    pub fn initialize(&self, root: &Path) -> Result<Value> {
        let root_uri = file_uri(root)?;
        let mut params = json!({
                "processId": std::process::id(),
                "clientInfo": {"name": "archmeld-language-broker", "version": env!("CARGO_PKG_VERSION")},
                "rootUri": root_uri,
                "workspaceFolders": [{"uri": root_uri, "name": "workspace"}],
                "capabilities": {
                    "workspace": {
                        "workspaceEdit": {"documentChanges": true},
                        "configuration": false
                    },
                    "textDocument": {
                        "rename": {"prepareSupport": false},
                        "synchronization": {"didSave": false}
                    }
                }
            });
        if let Some(options) = &self.initialization_options {
            params["initializationOptions"] = options.clone();
        }
        let result = self.request("initialize", params, Duration::from_secs(90))?;
        self.notify("initialized", json!({}))?;
        self.audit("initialize", json!({"root": root.to_string_lossy()}))?;
        Ok(result)
    }

    pub fn did_open(&self, path: &Path, language_id: &str, text: &str) -> Result<()> {
        self.notify(
            "textDocument/didOpen",
            json!({
                "textDocument": {
                    "uri": file_uri(path)?,
                    "languageId": language_id,
                    "version": 1,
                    "text": text,
                }
            }),
        )
    }

    pub fn rename(&self, path: &Path, line: u64, character: u64, new_name: &str, timeout: Duration) -> Result<Value> {
        let edit = self.request(
            "textDocument/rename",
            json!({
                "textDocument": {"uri": file_uri(path)?},
                "position": {"line": line, "character": character},
                "newName": new_name,
            }),
            timeout,
        )?;
        self.audit(
            "rename",
            json!({"path": path.to_string_lossy(), "line": line, "character": character, "new_name": new_name}),
        )?;
        Ok(edit)
    }

    pub fn shutdown(mut self) -> Result<AuditSummary> {
        let _ = self.request("shutdown", Value::Null, Duration::from_secs(10));
        let _ = self.notify("exit", json!({}));
        let deadline = Instant::now() + Duration::from_secs(15);
        let mut exit_code = None;
        while Instant::now() < deadline {
            if let Ok(Some(status)) = self.child.try_wait() {
                exit_code = status.code();
                break;
            }
            std::thread::sleep(Duration::from_millis(100));
        }
        if exit_code.is_none() {
            let _ = self.audit("kill", json!({"reason": "shutdown timeout"}));
            let _ = self.child.kill();
            let _ = self.child.wait();
        }
        // Give short-lived cargo metadata children a moment to exit, then audit
        // and clean only this session's own process tree. The root may already
        // have exited while its children survive with a stale parent id, so
        // surviving descendants are terminated by pid as well.
        std::thread::sleep(Duration::from_millis(800));
        let descendants = descendant_pids(self.pid);
        let audit_available = descendants.is_some();
        let mut seen = descendants.clone().unwrap_or_default();
        if let Some(remaining) = descendants {
            if !remaining.is_empty() {
                let _ = self.audit("orphans_detected", json!({"pids": remaining}));
                let _ = Command::new("taskkill")
                    .args(["/PID", &self.pid.to_string(), "/T", "/F"])
                    .stdout(Stdio::null())
                    .stderr(Stdio::null())
                    .status();
                for pid in &remaining {
                    let _ = Command::new("taskkill")
                        .args(["/PID", &pid.to_string(), "/T", "/F"])
                        .stdout(Stdio::null())
                        .stderr(Stdio::null())
                        .status();
                }
            }
        }
        let cleanup_deadline = Instant::now() + Duration::from_secs(15);
        let mut remaining = descendant_pids(self.pid).unwrap_or_default();
        while !remaining.is_empty() && Instant::now() < cleanup_deadline {
            std::thread::sleep(Duration::from_millis(200));
            remaining = descendant_pids(self.pid).unwrap_or_default();
        }
        if !remaining.is_empty() {
            seen.extend(remaining.iter().copied());
        }
        let summary = AuditSummary {
            events: self.events,
            exit_code,
            orphan_pids: remaining,
            descendants_seen: seen,
            audit_available,
        };
        self.audit(
            "shutdown",
            json!({
                "exit_code": summary.exit_code,
                "orphan_pids": summary.orphan_pids,
                "descendants_seen": summary.descendants_seen,
                "audit_available": summary.audit_available,
                "stderr_log": self.stderr_log.to_string_lossy(),
            }),
        )?;
        Ok(summary)
    }

    fn audit(&self, event: &str, detail: Value) -> Result<()> {
        let mut audit = Audit::resume(&self.audit_path, self.events);
        audit.record(event, detail)?;
        Ok(())
    }
}

impl Drop for LspSession {
    fn drop(&mut self) {
        if matches!(self.child.try_wait(), Ok(None)) {
            let _ = self.child.kill();
            let _ = self.child.wait();
        }
    }
}

struct Audit {
    path: PathBuf,
    events: u64,
}

impl Audit {
    fn new(path: &Path) -> Self {
        Self {
            path: path.to_path_buf(),
            events: 0,
        }
    }

    fn resume(path: &Path, events: u64) -> Self {
        Self {
            path: path.to_path_buf(),
            events,
        }
    }

    fn record(&mut self, event: &str, detail: Value) -> Result<()> {
        if let Some(parent) = self.path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let timestamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|duration| duration.as_millis())
            .unwrap_or(0);
        let line = json!({
            "timestamp_ms": timestamp,
            "event": event,
            "detail": detail,
        });
        let mut file = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&self.path)?;
        writeln!(file, "{line}")?;
        file.sync_data()?;
        self.events += 1;
        Ok(())
    }
}

fn read_loop(stdout: ChildStdout, stdin: Arc<Mutex<ChildStdin>>, tx: mpsc::Sender<Value>) {
    let mut reader = BufReader::new(stdout);
    while let Some(message) = read_message(&mut reader) {
        let is_request = message.get("method").is_some() && message.get("id").is_some();
        if is_request {
            let id = message["id"].clone();
            let method = message["method"].as_str().unwrap_or("");
            let result = match method {
                "workspace/configuration" => {
                    let count = message["params"]["items"]
                        .as_array()
                        .map(|items| items.len())
                        .unwrap_or(0);
                    Value::Array(vec![Value::Null; count])
                }
                "workspace/applyEdit" => json!({"applied": false}),
                _ => Value::Null,
            };
            if let Ok(mut guard) = stdin.lock() {
                let body = json!({"jsonrpc": "2.0", "id": id, "result": result}).to_string();
                let _ = write!(guard, "Content-Length: {}\r\n\r\n{}", body.len(), body);
                let _ = guard.flush();
            }
            continue;
        }
        if message.get("id").is_some() {
            if tx.send(message).is_err() {
                return;
            }
        }
    }
}

fn read_message(reader: &mut BufReader<ChildStdout>) -> Option<Value> {
    let mut length: Option<usize> = None;
    loop {
        let mut line = String::new();
        if reader.read_line(&mut line).ok()? == 0 {
            return None;
        }
        let trimmed = line.trim_end_matches(['\r', '\n']);
        if trimmed.is_empty() {
            break;
        }
        if let Some(value) = trimmed.strip_prefix("Content-Length:") {
            length = value.trim().parse().ok();
        }
    }
    let length = length?;
    let mut buffer = vec![0u8; length];
    reader.read_exact(&mut buffer).ok()?;
    serde_json::from_slice(&buffer).ok()
}

fn capture_stderr(mut stderr: impl Read, path: &Path) {
    if let Some(parent) = path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    let mut file = match std::fs::File::create(path) {
        Ok(file) => file,
        Err(_) => return,
    };
    let mut buffer = [0u8; 8192];
    loop {
        match stderr.read(&mut buffer) {
            Ok(0) | Err(_) => break,
            Ok(read) => {
                let _ = file.write_all(&buffer[..read]);
            }
        }
    }
}

pub fn file_uri(path: &Path) -> Result<String> {
    let absolute = std::fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf());
    let raw = absolute.to_string_lossy().replace('\\', "/");
    // Windows canonicalize returns an extended-length prefix (//?/C:/...).
    // LSP clients reject it as "url is not a file".
    let raw = raw
        .strip_prefix("//?/")
        .or_else(|| raw.strip_prefix("//./"))
        .unwrap_or(&raw);
    let mut text = raw.to_string();
    if !text.starts_with('/') {
        text.insert(0, '/');
    }
    let encoded = text
        .replace('%', "%25")
        .replace(' ', "%20")
        .replace('#', "%23");
    Ok(format!("file://{encoded}"))
}

/// Descendant pids of an owned process tree. Returns `None` when the audit
/// mechanism itself is unavailable; that is recorded, never treated as "clean".
fn descendant_pids(root: u32) -> Option<Vec<u32>> {
    let output = Command::new("powershell")
        .args([
            "-NoProfile",
            "-Command",
            "Get-CimInstance Win32_Process | Select-Object ProcessId,ParentProcessId | ConvertTo-Json -Compress",
        ])
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    let text = String::from_utf8_lossy(&output.stdout);
    let value: Value = serde_json::from_str(text.trim()).ok()?;
    let array = match value {
        Value::Array(items) => items,
        value @ Value::Object(_) => vec![value],
        _ => return None,
    };
    let mut children: Vec<(u32, u32)> = Vec::new();
    for item in array {
        let pid = item["ProcessId"].as_u64().unwrap_or(0) as u32;
        let parent = item["ParentProcessId"].as_u64().unwrap_or(0) as u32;
        if pid != 0 {
            children.push((pid, parent));
        }
    }
    let mut result = Vec::new();
    let mut frontier = vec![root];
    while let Some(parent) = frontier.pop() {
        for (pid, ppid) in &children {
            if *ppid == parent && !result.contains(pid) {
                result.push(*pid);
                frontier.push(*pid);
            }
        }
    }
    Some(result)
}
