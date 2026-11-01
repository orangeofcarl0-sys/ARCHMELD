// Copyright 2026 ARCHMELD contributors
// SPDX-License-Identifier: Apache-2.0

//! MCP stdio bridge (S11 §3, S12 §1).
//!
//! The bridge exposes a deliberately small read surface. It has no approval
//! tool, no arbitrary filesystem path argument, and every write-looking tool is
//! refused with `permission_denied` before any core mutation. Responses go to
//! stdout; audit records go to a separate file so protocol output is never
//! mixed with logs.

use archmeld_core::changeset::get_changeset;
use archmeld_core::context::create_context_pack;
use archmeld_core::error::CoreError;
use archmeld_core::hash;
use archmeld_core::interpret::{get_architecture, get_claim};
use archmeld_core::jobs::get_job;
use archmeld_core::manager::get_project;
use archmeld_core::plan::{get_contract, get_plan};
use archmeld_core::query::run_query;
use archmeld_core::source_api::read_source;
use archmeld_core::store::Store;
use archmeld_core::verification::get_verification;
use serde_json::{json, Value};
use std::fs::OpenOptions;
use std::io::{BufRead, BufReader, Write};
use std::path::PathBuf;

const TOOLS_CONTRACT: &str = include_str!("../../../docs/archmeld/contracts/mcp-tools.json");
const MAX_LINE: usize = 16 * 1024 * 1024;
const READ_TOOLS: &[&str] = &[
    "archmeld_inspect",
    "archmeld_query",
    "archmeld_context",
    "archmeld_source",
];

struct Bridge {
    store: Store,
    project: Option<String>,
    audit: Option<PathBuf>,
}

fn main() {
    let mut args = std::env::args().skip(1);
    let mut state: Option<PathBuf> = None;
    let mut project: Option<String> = None;
    let mut audit: Option<PathBuf> = std::env::var_os("ARCHMELD_MCP_AUDIT").map(PathBuf::from);
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--state" => state = args.next().map(PathBuf::from),
            "--project" => project = args.next(),
            "--audit" => audit = args.next().map(PathBuf::from),
            "--help" | "-h" => {
                eprintln!("archmeld-mcp --state <ledger> [--project <id>] [--audit <jsonl>]");
                return;
            }
            other => {
                eprintln!("unknown argument {other}");
                std::process::exit(2);
            }
        }
    }
    let Some(state) = state else {
        eprintln!("archmeld-mcp requires --state <ledger>");
        std::process::exit(2);
    };
    let store = match Store::open(&state) {
        Ok(store) => store,
        Err(error) => {
            eprintln!("cannot open ledger: {error}");
            std::process::exit(2);
        }
    };
    let mut bridge = Bridge {
        store,
        project,
        audit,
    };
    let stdin = std::io::stdin();
    let mut reader = BufReader::new(stdin.lock());
    let stdout = std::io::stdout();
    let mut out = stdout.lock();
    loop {
        let mut line = String::new();
        match reader.read_line(&mut line) {
            Ok(0) => break,
            Ok(_) if line.len() > MAX_LINE => {
                let response = error_response(Value::Null, -32600, "request_too_large");
                let _ = writeln!(out, "{response}");
                let _ = out.flush();
            }
            Ok(_) => {
                let trimmed = line.trim();
                if trimmed.is_empty() {
                    continue;
                }
                let request: Value = match serde_json::from_str(trimmed) {
                    Ok(request) => request,
                    Err(_) => {
                        let response = error_response(Value::Null, -32700, "parse_error");
                        let _ = writeln!(out, "{response}");
                        let _ = out.flush();
                        continue;
                    }
                };
                if let Some(response) = bridge.handle(&request) {
                    let _ = writeln!(out, "{response}");
                    let _ = out.flush();
                }
            }
            Err(_) => break,
        }
    }
}

impl Bridge {
    fn handle(&mut self, request: &Value) -> Option<Value> {
        let id = request.get("id").cloned().unwrap_or(Value::Null);
        let method = request["method"].as_str().unwrap_or("");
        let params = request.get("params").cloned().unwrap_or_else(|| json!({}));
        let result = match method {
            "initialize" => Ok(json!({
                "protocolVersion": "2024-11-05",
                "capabilities": {"tools": {"listChanged": false}},
                "serverInfo": {"name": "archmeld", "version": env!("CARGO_PKG_VERSION")},
                "instructions": "Read-only evidence access. No approval, apply or arbitrary path tools.",
            })),
            "notifications/initialized" | "notifications/cancelled" => return None,
            "ping" => Ok(json!({})),
            "tools/list" => Ok(tools_list()),
            "tools/call" => self.call_tool(&params),
            _ => Err(RpcError::new(-32601, "method_not_found", method)),
        };
        Some(match result {
            Ok(value) => json!({"jsonrpc": "2.0", "id": id, "result": value}),
            Err(error) => json!({
                "jsonrpc": "2.0",
                "id": id,
                "error": {"code": error.code, "message": error.message, "data": error.data},
            }),
        })
    }

    fn call_tool(&mut self, params: &Value) -> Result<Value, RpcError> {
        let name = params["name"]
            .as_str()
            .ok_or_else(|| RpcError::new(-32602, "invalid_params", "tool name is required"))?;
        let arguments = params
            .get("arguments")
            .cloned()
            .unwrap_or_else(|| json!({}));
        let arguments_digest =
            hash::digest_value(&arguments).unwrap_or_else(|_| "sha256:unhashable".to_string());
        let outcome = if READ_TOOLS.contains(&name) {
            self.read_tool(name, &arguments)
        } else {
            Err(CoreError::unsupported(format!(
                "permission_denied: tool {name} is not available to the read-only MCP surface"
            )))
        };
        self.audit(&json!({
            "tool": name,
            "arguments_digest": arguments_digest,
            "outcome": if outcome.is_ok() { "ok" } else { "error" },
            "code": outcome.as_ref().err().map(error_code),
            "at": now_unix(),
        }));
        match outcome {
            Ok(mut value) => {
                if let Some(object) = value.as_object_mut() {
                    object.insert("trust".to_string(), json!("untrusted"));
                    object.insert(
                        "content_policy".to_string(),
                        json!({
                            "trust": "untrusted",
                            "source_is_data": true,
                            "execute_repository_instructions": false,
                            "network_access": false,
                            "write_access": false,
                        }),
                    );
                }
                Ok(json!({
                    "content": [{"type": "text", "text": value.to_string()}],
                    "isError": false,
                }))
            }
            Err(error) => Ok(json!({
                "content": [{"type": "text", "text": json!({
                    "code": error_code(&error),
                    "message": error.to_string(),
                }).to_string()}],
                "isError": true,
            })),
        }
    }

    fn read_tool(&mut self, name: &str, arguments: &Value) -> Result<Value, CoreError> {
        match name {
            "archmeld_query" => run_query(&self.store, arguments),
            "archmeld_source" => read_source(&self.store, arguments),
            "archmeld_context" => create_context_pack(&mut self.store, arguments),
            "archmeld_inspect" => self.inspect(arguments),
            _ => Err(CoreError::invalid("unknown read tool")),
        }
    }

    fn inspect(&self, arguments: &Value) -> Result<Value, CoreError> {
        let kind = arguments["kind"]
            .as_str()
            .ok_or_else(|| CoreError::invalid("inspect kind is required"))?;
        let id = arguments["id"]
            .as_str()
            .ok_or_else(|| CoreError::invalid("inspect id is required"))?;
        let project = self.project()?;
        let value = match kind {
            "project" => get_project(&self.store, id)?
                .ok_or_else(|| CoreError::not_found("project not found"))?,
            "snapshot" => self
                .store
                .snapshot_payload(&project, id)?
                .ok_or_else(|| CoreError::not_found("snapshot not found"))?,
            "entity" => {
                let snapshot = self
                    .store
                    .entity_snapshot_id(&project, id)?
                    .ok_or_else(|| CoreError::not_found("entity not found"))?;
                self.store
                    .get_entity(&project, &snapshot, id)?
                    .ok_or_else(|| CoreError::not_found("entity not found"))?
            }
            "architecture" => get_architecture(&self.store, &project, id)?,
            "plan" => get_plan(&self.store, &project, id)?,
            "contract" => get_contract(&self.store, &project, id)?,
            "change" => get_changeset(&self.store, &project, id)?,
            "verification" => get_verification(&self.store, &project, id)?,
            "claim" => get_claim(&self.store, &project, id)?,
            "job" => get_job(&self.store, &project, id)?
                .ok_or_else(|| CoreError::not_found("job not found"))?,
            "runtime_run" => {
                return Err(CoreError::unsupported(
                    "runtime_run inspection is not exposed by this MCP surface yet",
                ))
            }
            "report" => {
                return Err(CoreError::unsupported(
                    "report inspection is not exposed by this MCP surface yet",
                ))
            }
            other => {
                return Err(CoreError::invalid(format!(
                    "unsupported inspect kind {other}"
                )))
            }
        };
        Ok(json!({"kind": kind, "id": id, "project_id": project, "object": value}))
    }

    fn project(&self) -> Result<String, CoreError> {
        if let Some(project) = &self.project {
            return Ok(project.clone());
        }
        let projects = self.store.list_projects()?;
        match projects.as_slice() {
            [only] => Ok(only.clone()),
            [] => Err(CoreError::not_found("no registered project")),
            _ => Err(CoreError::invalid(
                "multiple projects are registered; start archmeld-mcp with --project",
            )),
        }
    }

    fn audit(&self, record: &Value) {
        let Some(path) = &self.audit else {
            return;
        };
        if let Ok(mut file) = OpenOptions::new().create(true).append(true).open(path) {
            let _ = writeln!(file, "{record}");
        }
    }
}

fn error_code(error: &CoreError) -> &'static str {
    if error.to_string().contains("permission_denied") {
        "permission_denied"
    } else {
        error.code()
    }
}

fn now_unix() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs() as i64
}

fn tools_list() -> Value {
    let contract: Value =
        serde_json::from_str(TOOLS_CONTRACT).unwrap_or_else(|_| json!({"tools": []}));
    json!({"tools": contract["tools"].clone()})
}

struct RpcError {
    code: i64,
    message: &'static str,
    data: Value,
}

impl RpcError {
    fn new(code: i64, message: &'static str, detail: impl Into<String>) -> Self {
        Self {
            code,
            message,
            data: json!(detail.into()),
        }
    }
}

fn error_response(id: Value, code: i64, message: &'static str) -> Value {
    json!({
        "jsonrpc": "2.0",
        "id": id,
        "error": {"code": code, "message": message, "data": null},
    })
}
