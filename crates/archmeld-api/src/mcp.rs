// Copyright 2026 ARCHMELD contributors
// SPDX-License-Identifier: Apache-2.0

//! MCP stdio bridge (S11 §3, S12 §1).
//!
//! The bridge exposes a deliberately small read surface. It has no approval
//! tool, no arbitrary filesystem path argument, and every write-looking tool is
//! refused with `permission_denied` before any core mutation. Responses go to
//! stdout; audit records go to a separate file so protocol output is never
//! mixed with logs.

use archmeld_core::brief::{build_brief, render_markdown, BriefRecipe, BriefRequest};
use archmeld_core::changeset::get_changeset;
use archmeld_core::context::create_context_pack;
use archmeld_core::error::CoreError;
use archmeld_core::hash;
use archmeld_core::interpret::{get_architecture, get_claim};
use archmeld_core::jobs::get_job;
use archmeld_core::manager::get_project;
use archmeld_core::plan::{get_contract, get_plan};
use archmeld_core::query::run_query;
use archmeld_core::review::review_workspace_diff;
use archmeld_core::source_api::read_source;
use archmeld_core::status::status_report;
use archmeld_core::store::Store;
use archmeld_core::verification::get_verification;
use serde_json::{json, Value};
use std::fs::OpenOptions;
use std::io::{BufRead, BufReader, Write};
use std::path::PathBuf;

const TOOLS_CONTRACT: &str = include_str!("../../../docs/archmeld/contracts/mcp-tools.json");
const MAX_LINE: usize = 16 * 1024 * 1024;
const IMPLEMENTED_INSPECT_KINDS: &[&str] = &[
    "project",
    "snapshot",
    "entity",
    "architecture",
    "plan",
    "contract",
    "change",
    "verification",
    "claim",
    "job",
    "map_entry",
];
/// The single source of truth for `tools/list`. A tool is advertised only when
/// this process has a handler for it; the broader contract is reported by
/// `archmeld_capabilities` with an explicit unavailable status.
const IMPLEMENTED_TOOLS: &[&str] = &[
    "archmeld_inspect",
    "archmeld_query",
    "archmeld_context",
    "archmeld_source",
    "archmeld_status",
    "archmeld_brief",
    "archmeld_review",
    "archmeld_capabilities",
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
                "instructions": "Read-only ARCHMELD evidence access. Start with archmeld_status or archmeld_brief. Indexing is a CLI step (`archmeld init`); this process never creates a snapshot. Workflow: status -> brief/review -> query/context -> source. Candidate relations are not resolved calls, and review never executes tests.",
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
        let outcome = if IMPLEMENTED_TOOLS.contains(&name) {
            self.read_tool(name, &arguments)
        } else {
            Err(unavailable_tool(&name))
        };
        self.audit(&json!({
            "tool": name,
            "arguments_digest": arguments_digest,
            "outcome": if outcome.is_ok() { "ok" } else { "error" },
            "code": outcome.as_ref().err().map(error_code),
            "at": now_unix(),
        }));
        match outcome {
            Ok(value) => {
                let mut value = decorate(name, value);
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
                let mut content = vec![json!({"type": "text", "text": value.to_string()})];
                // A one-screen card is the readable surface; the JSON item
                // stays first so existing clients keep parsing the same shape.
                if name == "archmeld_brief" && value["summary"].is_string() {
                    content.push(json!({"type": "text", "text": render_markdown(&value)}));
                }
                Ok(json!({
                    "content": content,
                    "isError": false,
                }))
            }
            Err(error) => {
                let message = error.to_string();
                let next_action = if message.contains("no snapshot") {
                    "run archmeld init (or archmeld index) before querying"
                } else if message.contains("multiple projects") {
                    "call archmeld_status to list projects, then pass project_id"
                } else if message.contains("workspace root") {
                    "run archmeld init to register the workspace root"
                } else if message.contains("entity_id is required") {
                    "call archmeld_query with kind=symbols first"
                } else {
                    "call archmeld_capabilities for the implemented surface"
                };
                Ok(json!({
                    "content": [{"type": "text", "text": json!({
                        "code": error_code(&error),
                        "message": message,
                        "status": tool_status(&name),
                        "next_action": next_action,
                    }).to_string()}],
                    "isError": true,
                }))
            }
        }
    }

    fn read_tool(&mut self, name: &str, arguments: &Value) -> Result<Value, CoreError> {
        match name {
            "archmeld_query" => {
                let filled = self.fill_query(arguments)?;
                run_query(&self.store, &filled)
            }
            "archmeld_source" => {
                let filled = self.fill_source(arguments)?;
                read_source(&self.store, &filled)
            }
            "archmeld_context" => {
                let filled = self.fill_context(arguments)?;
                create_context_pack(&mut self.store, &filled)
            }
            "archmeld_inspect" => self.inspect(arguments),
            "archmeld_status" => {
                if !arguments.as_object().is_none_or(|object| object.is_empty()) {
                    return Err(CoreError::invalid("archmeld_status takes no arguments"));
                }
                let root = match &self.project {
                    Some(project) => self.store.active_project_root(project)?,
                    None => {
                        let projects = self.store.list_projects()?;
                        if projects.len() == 1 {
                            self.store.active_project_root(&projects[0])?
                        } else {
                            None
                        }
                    }
                };
                status_report(&self.store, self.project.as_deref(), root.as_deref())
            }
            "archmeld_brief" => self.brief(arguments),
            "archmeld_review" => {
                if let Some(object) = arguments.as_object() {
                    if object.keys().any(|key| key != "base_snapshot_id") {
                        return Err(CoreError::invalid(
                            "archmeld_review accepts only base_snapshot_id",
                        ));
                    }
                }
                let project = self.project()?;
                let root = self.store.active_project_root(&project)?.ok_or_else(|| {
                    CoreError::unsupported(
                        "project has no active workspace root; run archmeld init to register it",
                    )
                })?;
                let base = match arguments["base_snapshot_id"].as_str() {
                    Some(snapshot_id) if !snapshot_id.is_empty() => snapshot_id.to_string(),
                    _ => self.store.latest_snapshot_id(&project)?.ok_or_else(|| {
                        CoreError::not_found("project has no snapshot; run archmeld init")
                    })?,
                };
                review_workspace_diff(&self.store, &project, &base, &root)
            }
            "archmeld_capabilities" => {
                if !arguments.as_object().is_none_or(|object| object.is_empty()) {
                    return Err(CoreError::invalid(
                        "archmeld_capabilities takes no arguments",
                    ));
                }
                Ok(capabilities())
            }
            _ => Err(CoreError::invalid("unknown read tool")),
        }
    }

    fn project_and_snapshot(&self, arguments: &Value) -> Result<(String, String), CoreError> {
        let project = match arguments["project_id"].as_str() {
            Some(project) if !project.is_empty() => project.to_string(),
            _ => self.project()?,
        };
        let snapshot = match arguments["snapshot_id"].as_str() {
            Some(snapshot) if !snapshot.is_empty() => snapshot.to_string(),
            _ => self.store.latest_snapshot_id(&project)?.ok_or_else(|| {
                CoreError::not_found("project has no snapshot; run archmeld init")
            })?,
        };
        Ok((project, snapshot))
    }

    fn fill_query(&self, arguments: &Value) -> Result<Value, CoreError> {
        let (project, snapshot) = self.project_and_snapshot(arguments)?;
        let mut object = arguments.as_object().cloned().unwrap_or_default();
        object.insert("project_id".to_string(), json!(project));
        object.insert("snapshot_id".to_string(), json!(snapshot));
        object
            .entry("kind".to_string())
            .or_insert_with(|| json!("symbols"));
        object
            .entry("query".to_string())
            .or_insert_with(|| json!(""));
        object
            .entry("entity_ids".to_string())
            .or_insert_with(|| json!([]));
        object
            .entry("relation_kinds".to_string())
            .or_insert_with(|| json!([]));
        object
            .entry("max_nodes".to_string())
            .or_insert_with(|| json!(100));
        object
            .entry("max_depth".to_string())
            .or_insert_with(|| json!(1));
        object.entry("cursor".to_string()).or_insert(Value::Null);
        Ok(Value::Object(object))
    }

    fn fill_context(&self, arguments: &Value) -> Result<Value, CoreError> {
        let objective = arguments["objective"].as_str().unwrap_or("").trim();
        if objective.is_empty() {
            return Err(CoreError::invalid(
                "objective is required; describe what to summarize or investigate",
            ));
        }
        let (project, snapshot) = self.project_and_snapshot(arguments)?;
        let mut object = arguments.as_object().cloned().unwrap_or_default();
        object.insert("project_id".to_string(), json!(project));
        object.insert("snapshot_id".to_string(), json!(snapshot));
        object
            .entry("entity_ids".to_string())
            .or_insert_with(|| json!([]));
        object
            .entry("budget_tokens".to_string())
            .or_insert_with(|| json!(4000));
        Ok(Value::Object(object))
    }

    fn fill_source(&self, arguments: &Value) -> Result<Value, CoreError> {
        if arguments["entity_id"]
            .as_str()
            .is_none_or(|entity_id| entity_id.is_empty())
        {
            return Err(CoreError::invalid(
                "entity_id is required; call archmeld_query kind=symbols first",
            ));
        }
        let (project, snapshot) = self.project_and_snapshot(arguments)?;
        let mut object = arguments.as_object().cloned().unwrap_or_default();
        object.insert("project_id".to_string(), json!(project));
        object.insert("snapshot_id".to_string(), json!(snapshot));
        object
            .entry("anchor_index".to_string())
            .or_insert_with(|| json!(0));
        object
            .entry("max_bytes".to_string())
            .or_insert_with(|| json!(200000));
        Ok(Value::Object(object))
    }

    fn brief(&self, arguments: &Value) -> Result<Value, CoreError> {
        let recipe = BriefRecipe::parse(arguments["recipe"].as_str().unwrap_or("auto"))?;
        let project = match arguments["project_id"].as_str() {
            Some(project) if !project.is_empty() => project.to_string(),
            _ => match self.project() {
                Ok(project) => project,
                Err(error) => {
                    let projects = self.store.list_projects()?;
                    if projects.len() > 1 {
                        return Ok(choice_brief(&projects, recipe));
                    }
                    return Err(error);
                }
            },
        };
        let root = self.store.active_project_root(&project)?;
        let mut value = build_brief(
            &self.store,
            &BriefRequest {
                project_id: &project,
                snapshot_id: arguments["snapshot_id"].as_str(),
                workspace: root.as_deref(),
                recipe,
            },
        )?;
        if arguments["format"].as_str() == Some("human") {
            value["human_text"] = json!(render_markdown(&value));
        }
        Ok(value)
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
            "map_entry" => {
                let root = self.store.active_project_root(&project)?.ok_or_else(|| {
                    CoreError::unsupported(
                        "project has no active workspace root; run archmeld init to register it",
                    )
                })?;
                let snapshot = self.store.latest_snapshot_id(&project)?.ok_or_else(|| {
                    CoreError::not_found("project has no snapshot; run archmeld init")
                })?;
                archmeld_core::cognition_map::entry_report(
                    &self.store,
                    &project,
                    &root,
                    Some(&snapshot),
                    None,
                    id,
                )?
            }
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

fn contract() -> Value {
    serde_json::from_str(TOOLS_CONTRACT).unwrap_or_else(|_| json!({"tools": []}))
}

fn contract_tool(name: &str) -> Option<Value> {
    contract()["tools"]
        .as_array()
        .and_then(|tools| tools.iter().find(|tool| tool["name"] == name))
        .cloned()
}

/// `implemented`, `requires_grant`, `not_implemented` or `unknown`. A tool is
/// only `implemented` when this process has a handler for it; the declaration
/// in the contract alone is never advertised as callable.
fn tool_status(name: &str) -> &'static str {
    if IMPLEMENTED_TOOLS.contains(&name) {
        return "implemented";
    }
    match contract_tool(name) {
        Some(tool) if tool["annotations"]["readOnlyHint"].as_bool() == Some(false) => {
            "requires_grant"
        }
        Some(_) => "not_implemented",
        None => "unknown",
    }
}

fn unavailable_tool(name: &str) -> CoreError {
    match tool_status(name) {
        "requires_grant" => CoreError::unsupported(format!(
            "permission_denied: tool {name} requires an external grant; this read-only MCP \
             process cannot self-approve. Call archmeld_capabilities for the grant path. \
             status=requires_grant"
        )),
        "not_implemented" => CoreError::unsupported(format!(
            "permission_denied: tool {name} is declared but has no handler in this build. \
             status=not_implemented"
        )),
        _ => CoreError::unsupported(format!(
            "permission_denied: tool {name} is not exposed by the read-only MCP surface. \
             status=unknown"
        )),
    }
}

fn capabilities_tool() -> Value {
    json!({
        "name": "archmeld_capabilities",
        "description": "Report the tools and operations this process can actually execute, and distinguish external-grant requirements from unimplemented declarations. Read-only; grants nothing.",
        "inputSchema": {
            "type": "object",
            "properties": {},
            "required": [],
            "additionalProperties": false
        },
        "annotations": {
            "readOnlyHint": true,
            "destructiveHint": false,
            "idempotentHint": true,
            "openWorldHint": false
        },
        "x-implementation": {
            "status": "implemented",
            "handler": "mcp::capabilities",
            "surface": "read_only"
        }
    })
}

fn choice_brief(projects: &[String], recipe: BriefRecipe) -> Value {
    json!({
        "schema_version": "1.0",
        "state": "needs_choice",
        "level": "project",
        "summary": "multiple projects are registered; choose one project_id",
        "counts": {
            "confirmed": 0,
            "candidate": 0,
            "unknown": projects.len() as u64,
            "stale": 0,
            "findings": 0
        },
        "findings": [],
        "unknowns": [{
            "question": "Which project should this brief describe?",
            "why": "the MCP process has no --project and multiple projects are registered",
            "how_to_resolve": "call archmeld_brief again with project_id from choices"
        }],
        "choices": projects.iter().map(|project| json!({
            "id": project,
            "label": project,
            "why": "registered project",
            "tool": "archmeld_brief",
            "args": {"project_id": project, "recipe": recipe.as_str()}
        })).collect::<Vec<_>>(),
        "next_actions": [{
            "tool": "archmeld_status",
            "args": {},
            "why": "list projects and their latest snapshots"
        }]
    })
}

fn decorate(tool: &str, mut value: Value) -> Value {
    if !value.is_object() {
        return value;
    }
    if value.get("next_actions").is_none() {
        let next = default_next_actions(tool, &value);
        value["next_actions"] = next;
    }
    if !value.get("summary").map(Value::is_string).unwrap_or(false) {
        let summary = agent_summary(tool, &value);
        value["agent_summary"] = json!(summary);
    }
    value
}

fn agent_summary(tool: &str, value: &Value) -> String {
    match tool {
        "archmeld_status" => {
            let projects = value["projects"].as_array().map(Vec::len).unwrap_or(0);
            let latest = value["projects"]
                .as_array()
                .and_then(|projects| projects.first())
                .and_then(|project| project["latest_snapshot_id"].as_str())
                .unwrap_or("none");
            format!("{projects} project(s); latest snapshot {latest}")
        }
        "archmeld_review" => format!(
            "{} file(s) changed; {} affected entities; verification=not_run",
            value["summary"]["files"]["changed"].as_u64().unwrap_or(0)
                + value["summary"]["files"]["added"].as_u64().unwrap_or(0)
                + value["summary"]["files"]["deleted"].as_u64().unwrap_or(0),
            value["summary"]["affected_entities"].as_u64().unwrap_or(0)
        ),
        "archmeld_query" => format!(
            "{} entities; {} relations; coverage={}",
            value["entities"].as_array().map(Vec::len).unwrap_or(0),
            value["relations"].as_array().map(Vec::len).unwrap_or(0),
            value["coverage"]["state"].as_str().unwrap_or("unknown")
        ),
        "archmeld_context" => format!(
            "context pack: {} selected entities; {} source snippets; truncated={}",
            value["selected_entities"]
                .as_array()
                .map(Vec::len)
                .unwrap_or(0),
            value["source_snippets"]
                .as_array()
                .map(Vec::len)
                .unwrap_or(0),
            value["budget"]["truncated"].as_bool().unwrap_or(false)
        ),
        "archmeld_source" => {
            let path = value["anchor"]["path"].as_str().unwrap_or("source");
            let line = value["anchor"]["line_start"].as_u64().unwrap_or(0);
            let bytes = value["text"].as_str().map(str::len).unwrap_or(0);
            format!("source {path}:{line}; {bytes} byte(s) of text")
        }
        "archmeld_inspect" => format!(
            "inspected {} {}",
            value["kind"].as_str().unwrap_or("object"),
            value["id"].as_str().unwrap_or("")
        ),
        "archmeld_capabilities" => format!(
            "{} implemented tool(s)",
            value["implemented_tools"]
                .as_array()
                .map(Vec::len)
                .unwrap_or(0)
        ),
        _ => "ARCHMELD read-only result".to_string(),
    }
}

fn default_next_actions(tool: &str, value: &Value) -> Value {
    match tool {
        "archmeld_status" => json!([
            {"tool": "archmeld_brief", "args": {"recipe": "auto"}, "why": "get a one-screen summary"},
            {"tool": "archmeld_review", "args": {}, "why": "compare the working tree with the snapshot"},
            {"tool": "archmeld_query", "args": {"kind": "symbols", "query": ""}, "why": "list indexed symbols"}
        ]),
        "archmeld_review" => json!([
            {"tool": "archmeld_brief", "args": {"recipe": "change"}, "why": "condense the review into findings"},
            {"tool": "archmeld_status", "args": {}, "why": "check the snapshot and freshness"}
        ]),
        "archmeld_query" => {
            let entity = value["entities"]
                .as_array()
                .and_then(|entities| entities.first())
                .and_then(|entity| entity["id"].as_str())
                .unwrap_or("");
            if entity.is_empty() {
                json!([{"tool": "archmeld_status", "args": {}, "why": "check the snapshot"}])
            } else {
                json!([
                    {"tool": "archmeld_source", "args": {"entity_id": entity}, "why": "read the exact source range"},
                    {"tool": "archmeld_context", "args": {"objective": "explain this entity", "entity_ids": [entity]}, "why": "build a bounded evidence pack"}
                ])
            }
        }
        "archmeld_context" => {
            let entity = value["source_snippets"]
                .as_array()
                .and_then(|snippets| snippets.first())
                .and_then(|snippet| snippet["entity_id"].as_str())
                .unwrap_or("");
            if entity.is_empty() {
                json!([{"tool": "archmeld_status", "args": {}, "why": "check the snapshot"}])
            } else {
                json!([{"tool": "archmeld_source", "args": {"entity_id": entity}, "why": "read the exact source range"}])
            }
        }
        "archmeld_source" => {
            let entity = value["entity_id"].as_str().unwrap_or("");
            if entity.is_empty() {
                json!([{"tool": "archmeld_status", "args": {}, "why": "check the snapshot"}])
            } else {
                json!([
                    {"tool": "archmeld_query", "args": {"kind": "neighbors", "entity_ids": [entity], "max_depth": 1}, "why": "list outgoing relations"},
                    {"tool": "archmeld_context", "args": {"objective": "explain this entity", "entity_ids": [entity]}, "why": "build a bounded evidence pack"}
                ])
            }
        }
        "archmeld_inspect" => json!([
            {"tool": "archmeld_status", "args": {}, "why": "check the snapshot and freshness"},
            {"tool": "archmeld_capabilities", "args": {}, "why": "see which operations are implemented"}
        ]),
        "archmeld_capabilities" => json!([
            {"tool": "archmeld_status", "args": {}, "why": "see the registered project and snapshot"},
            {"tool": "archmeld_brief", "args": {"recipe": "auto"}, "why": "start with a one-screen summary"}
        ]),
        _ => json!([]),
    }
}

fn tools_list() -> Value {
    let mut tools = Vec::new();
    for name in IMPLEMENTED_TOOLS {
        if *name == "archmeld_capabilities" {
            tools.push(capabilities_tool());
            continue;
        }
        let Some(mut tool) = contract_tool(name) else {
            continue;
        };
        if *name == "archmeld_inspect" {
            tool["inputSchema"]["properties"]["kind"]["enum"] = json!(IMPLEMENTED_INSPECT_KINDS);
        }
        tool["x-implementation"] = json!({
            "status": "implemented",
            "handler": "mcp::read_tool",
            "surface": "read_only"
        });
        tools.push(tool);
    }
    json!({"tools": tools})
}

fn capabilities() -> Value {
    let declaration = contract();
    let mut tools = Vec::new();
    for tool in declaration["tools"].as_array().into_iter().flatten() {
        let Some(name) = tool["name"].as_str() else {
            continue;
        };
        let status = tool_status(name);
        let mut entry = json!({
            "name": name,
            "status": status,
            "read_only": tool["annotations"]["readOnlyHint"],
            "operations": tool["x-operations"],
        });
        if status == "requires_grant" {
            entry["authorization"] = json!({
                "required": true,
                "self_approval": false,
                "mcp_approval_tool": null,
                "via": "HTTP API or local CLI with an explicit execution/approval grant",
            });
        }
        tools.push(entry);
    }
    tools.push(json!({
        "name": "archmeld_capabilities",
        "status": "implemented",
        "read_only": true,
        "operations": ["capabilities"],
    }));
    let declared_kinds: Vec<String> = contract_tool("archmeld_inspect")
        .and_then(|tool| {
            tool["inputSchema"]["properties"]["kind"]["enum"]
                .as_array()
                .cloned()
        })
        .unwrap_or_default()
        .into_iter()
        .filter_map(|value| value.as_str().map(str::to_string))
        .collect();
    let not_implemented_kinds: Vec<String> = declared_kinds
        .iter()
        .filter(|kind| !IMPLEMENTED_INSPECT_KINDS.contains(&kind.as_str()))
        .cloned()
        .collect();
    json!({
        "surface": "mcp_stdio_read_only",
        "protocol_version": "2024-11-05",
        "contract_status": declaration["status"],
        "approval_exposed": false,
        "implemented_tools": IMPLEMENTED_TOOLS,
        "tools": tools,
        "inspect_kinds": {
            "implemented": IMPLEMENTED_INSPECT_KINDS,
            "not_implemented": not_implemented_kinds,
        },
        "grant_procedure": {
            "mcp_self_approval": false,
            "approval_tools": [],
            "via": "HTTP API or local CLI with an explicit execution/approval grant",
            "reason": "the stdio bridge is intentionally read-only so repository content cannot grant itself write or execution authority",
        },
        "content_policy": {
            "trust": "untrusted",
            "source_is_data": true,
            "execute_repository_instructions": false,
            "network_access": false,
            "write_access": false,
        },
    })
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
