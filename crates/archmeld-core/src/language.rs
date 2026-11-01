// Copyright 2026 ARCHMELD contributors
// SPDX-License-Identifier: Apache-2.0

//! Language broker: capability negotiation, LSP position conversion and
//! read-only rename previews (AT023, AT024).
//!
//! This module never starts a language server, executes a command or installs
//! a tool. It decides whether a capability is verified, converts an LSP
//! WorkspaceEdit into a reviewable patch against snapshot bytes, and refuses
//! edits outside the authorized scope.

use crate::error::{CoreError, Result};
use crate::hash;
use crate::source::LineIndex;
use crate::store::{validate, FileEntry, Store};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::collections::BTreeMap;
use std::path::Path;

const MAX_FILES: usize = 32;
const MAX_EDITS_PER_FILE: usize = 2000;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CapabilityState {
    Verified,
    Unverified,
}

#[derive(Debug, Clone)]
pub struct Profile {
    pub id: String,
    pub languages: Vec<String>,
    capabilities: BTreeMap<String, CapabilityState>,
}

impl Profile {
    pub fn capability(&self, name: &str) -> CapabilityState {
        self.capabilities
            .get(name)
            .copied()
            .unwrap_or(CapabilityState::Unverified)
    }

    pub fn supports(&self, name: &str) -> bool {
        self.capability(name) == CapabilityState::Verified
    }
}

#[derive(Debug, Clone)]
pub struct Registry {
    profiles: Vec<Profile>,
}

impl Registry {
    /// Parse `contracts/language-profiles.json`. A capability is only `Verified`
    /// when its state literally says `verified`; every target/not_run state
    /// stays unavailable instead of being silently promoted.
    pub fn from_contract_json(text: &str) -> Result<Self> {
        let value: Value = serde_json::from_str(text)
            .map_err(|err| CoreError::invalid(format!("language profiles: {err}")))?;
        let profiles = value["profiles"]
            .as_array()
            .ok_or_else(|| CoreError::invalid("language profiles must contain a profiles array"))?;
        let mut parsed = Vec::new();
        for profile in profiles {
            let id = profile["id"]
                .as_str()
                .ok_or_else(|| CoreError::invalid("language profile without id"))?
                .to_string();
            let languages = profile["languages"]
                .as_array()
                .map(|items| {
                    items
                        .iter()
                        .filter_map(Value::as_str)
                        .map(str::to_string)
                        .collect()
                })
                .unwrap_or_default();
            let mut capabilities = BTreeMap::new();
            if let Some(object) = profile["capabilities"].as_object() {
                for (name, state) in object {
                    let state = match state.as_str().unwrap_or("") {
                        "verified" => CapabilityState::Verified,
                        _ => CapabilityState::Unverified,
                    };
                    capabilities.insert(name.clone(), state);
                }
            }
            parsed.push(Profile {
                id,
                languages,
                capabilities,
            });
        }
        Ok(Self { profiles: parsed })
    }

    pub fn profile(&self, id: &str) -> Option<&Profile> {
        self.profiles.iter().find(|profile| profile.id == id)
    }

    pub fn for_language(&self, language: &str) -> Option<&Profile> {
        self.profiles
            .iter()
            .find(|profile| profile.languages.iter().any(|item| item == language))
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BrokerRequest {
    Definition,
    References,
    Hover,
    RenamePreview,
    ExecuteCommand,
    InstallTool,
    NetworkFetch,
}

#[derive(Debug, Clone, Copy)]
pub struct ExecutionPolicy {
    pub allow_read_only: bool,
    /// A grant still requires a per-call approval; the broker never executes.
    pub execute_command_permitted: bool,
}

impl Default for ExecutionPolicy {
    fn default() -> Self {
        Self {
            allow_read_only: true,
            execute_command_permitted: false,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Decision {
    Allow,
    NeedsPermission {
        capability: &'static str,
        reason: String,
    },
    Deny {
        code: &'static str,
        reason: String,
    },
}

pub fn decide(profile: &Profile, request: BrokerRequest, policy: &ExecutionPolicy) -> Decision {
    match request {
        BrokerRequest::Definition | BrokerRequest::References | BrokerRequest::Hover => {
            if !policy.allow_read_only {
                return Decision::Deny {
                    code: "read_only_not_permitted",
                    reason: "read-only language requests are disabled by policy".into(),
                };
            }
            if profile.supports("definition_reference") {
                Decision::Allow
            } else {
                Decision::Deny {
                    code: "unsupported_capability",
                    reason: format!(
                        "profile {} has no verified definition/reference capability",
                        profile.id
                    ),
                }
            }
        }
        BrokerRequest::RenamePreview => {
            if !policy.allow_read_only {
                return Decision::Deny {
                    code: "read_only_not_permitted",
                    reason: "preview requires reading source".into(),
                };
            }
            if profile.supports("bounded_refactor") {
                Decision::Allow
            } else {
                Decision::Deny {
                    code: "unsupported_capability",
                    reason: format!(
                        "profile {} has no verified bounded-refactor capability",
                        profile.id
                    ),
                }
            }
        }
        BrokerRequest::ExecuteCommand => {
            if policy.execute_command_permitted {
                Decision::NeedsPermission {
                    capability: "execute_command",
                    reason: "language servers may run repository code; each call needs explicit approval and is never auto-executed"
                        .into(),
                }
            } else {
                Decision::Deny {
                    code: "execution_not_permitted",
                    reason: "executeCommand is disabled; no language-server command is run automatically".into(),
                }
            }
        }
        BrokerRequest::InstallTool => Decision::Deny {
            code: "auto_install_forbidden",
            reason: "tool installation is a separate authorized step".into(),
        },
        BrokerRequest::NetworkFetch => Decision::Deny {
            code: "network_denied",
            reason: "the language broker performs no network requests".into(),
        },
    }
}

#[derive(Debug, Clone)]
pub struct PreviewRequest<'a> {
    pub project_id: &'a str,
    pub snapshot_id: &'a str,
    pub allowed_paths: &'a [String],
    pub max_files: usize,
    /// Authorized workspace root. When set, absolute `file://` URIs are
    /// accepted only if they resolve inside this root.
    pub workspace_root: Option<&'a Path>,
}

#[derive(Debug, Clone, Serialize)]
pub struct FilePreview {
    pub path: String,
    pub before_digest: String,
    pub after_digest: String,
    pub edits: usize,
}

#[derive(Debug, Clone, Serialize)]
pub struct Refusal {
    pub code: String,
    pub path: Option<String>,
    pub message: String,
}

#[derive(Debug, Clone)]
pub struct EditPreview {
    pub snapshot_id: String,
    pub files: Vec<FilePreview>,
    pub refusals: Vec<Refusal>,
    pub patch: String,
    pub total_edits: usize,
}

impl EditPreview {
    pub fn complete(&self) -> bool {
        self.refusals.is_empty() && !self.files.is_empty()
    }

    pub fn to_json(&self) -> Value {
        json!({
            "snapshot_id": self.snapshot_id,
            "files": self.files,
            "refusals": self.refusals,
            "patch": self.patch,
            "total_edits": self.total_edits,
            "complete": self.complete(),
            "writes_applied": false,
        })
    }
}

/// Convert an LSP WorkspaceEdit into a patch without writing anything.
/// Any refusal makes the preview incomplete and suppresses file output, so a
/// caller can never mistake a partially valid edit for an applicable one.
pub fn preview_workspace_edit(
    store: &Store,
    request: &PreviewRequest<'_>,
    edit: &Value,
) -> Result<EditPreview> {
    validate::require_id("project_id", request.project_id)?;
    validate::require_digest("snapshot_id", request.snapshot_id)?;
    store.check_snapshot_scope(request.project_id, request.snapshot_id)?;
    let max_files = request.max_files.clamp(1, MAX_FILES);

    let mut refusals = Vec::new();
    let mut per_file: BTreeMap<String, Vec<TextEdit>> = BTreeMap::new();
    let encoding = match edit["position_encoding"].as_str().unwrap_or("utf-16") {
        "utf-8" => Encoding::Utf8,
        "utf-16" => Encoding::Utf16,
        "utf-32" => Encoding::Utf32,
        other => {
            return Err(CoreError::invalid(format!(
                "unsupported position_encoding {other}"
            )))
        }
    };

    if let Some(changes) = edit["changes"].as_object() {
        for (uri, edits) in changes {
            let path = match normalize_uri(uri, request) {
                Ok(path) => path,
                Err(refusal) => {
                    refusals.push(refusal);
                    continue;
                }
            };
            let Some(edits_array) = edits.as_array() else {
                refusals.push(Refusal {
                    code: "invalid_edit".into(),
                    path: Some(path),
                    message: "changes entry is not an edit array".into(),
                });
                continue;
            };
            collect_edits(&path, edits_array, &mut per_file, &mut refusals);
        }
    }
    if let Some(document_changes) = edit["documentChanges"].as_array() {
        for change in document_changes {
            if change["kind"].is_string() {
                refusals.push(Refusal {
                    code: "resource_operation_not_supported".into(),
                    path: change["textDocument"]["uri"].as_str().map(str::to_string),
                    message: "create/rename/delete resource operations are not part of a v1 rename preview"
                        .into(),
                });
                continue;
            }
            let Some(uri) = change["textDocument"]["uri"].as_str() else {
                refusals.push(Refusal {
                    code: "invalid_document_change".into(),
                    path: None,
                    message: "documentChanges entry has no textDocument.uri".into(),
                });
                continue;
            };
            let path = match normalize_uri(uri, request) {
                Ok(path) => path,
                Err(refusal) => {
                    refusals.push(refusal);
                    continue;
                }
            };
            let empty: &[Value] = &[];
            collect_edits(
                &path,
                change["edits"].as_array().map(Vec::as_slice).unwrap_or(empty),
                &mut per_file,
                &mut refusals,
            );
        }
    }
    if per_file.is_empty() && refusals.is_empty() {
        return Err(CoreError::invalid("workspace edit contains no text edits"));
    }
    if per_file.len() > max_files {
        refusals.push(Refusal {
            code: "too_many_files".into(),
            path: None,
            message: format!(
                "edit touches {} files; the preview limit is {max_files}",
                per_file.len()
            ),
        });
        per_file.clear();
    }

    let mut files = Vec::new();
    let mut patch = String::new();
    let mut total_edits = 0usize;
    for (path, edits) in per_file {
        if !refusals.is_empty() {
            break;
        }
        let entry = match snapshot_file(store, request, &path)? {
            Some(entry) => entry,
            None => {
                refusals.push(Refusal {
                    code: "file_not_in_snapshot".into(),
                    path: Some(path.clone()),
                    message: "path is not part of the declared snapshot".into(),
                });
                break;
            }
        };
        if !allowed(request.allowed_paths, &path) {
            refusals.push(Refusal {
                code: "scope_violation".into(),
                path: Some(path.clone()),
                message: "path is outside allowed_paths".into(),
            });
            break;
        }
        let bytes = store
            .read_blob(&entry.blob_digest)?
            .ok_or_else(|| CoreError::Storage(format!("blob {} missing", entry.blob_digest)))?;
        let text = std::str::from_utf8(&bytes)
            .map_err(|_| CoreError::unsupported(format!("{path} is not UTF-8; preview unsupported")))?;
        let index = LineIndex::new(&bytes);
        let mut resolved = Vec::with_capacity(edits.len());
        for edit in &edits {
            match edit.into_bytes(&index, &bytes, encoding) {
                Ok((start, end)) => resolved.push((start, end, edit.new_text.clone())),
                Err(refusal) => {
                    refusals.push(Refusal {
                        code: refusal.into(),
                        path: Some(path.clone()),
                        message: "edit range is invalid for the snapshot bytes".into(),
                    });
                }
            }
        }
        if !refusals.is_empty() {
            break;
        }
        resolved.sort_by_key(|(start, end, _)| (*start, *end));
        for window in resolved.windows(2) {
            let (a_start, a_end, _) = &window[0];
            let (b_start, _b_end, _) = &window[1];
            if b_start < a_end || (b_start == a_start && a_start == a_end) {
                refusals.push(Refusal {
                    code: "overlapping_edits".into(),
                    path: Some(path.clone()),
                    message: "edits overlap or insert at the same position".into(),
                });
                break;
            }
        }
        if !refusals.is_empty() {
            break;
        }
        let mut after = String::with_capacity(text.len());
        let mut cursor = 0usize;
        for (start, end, replacement) in &resolved {
            after.push_str(&text[cursor..*start as usize]);
            after.push_str(replacement);
            cursor = *end as usize;
        }
        after.push_str(&text[cursor..]);
        total_edits += resolved.len();
        let diff = similar::TextDiff::from_lines(text, &after);
        let old_label = format!("a/{path}");
        let new_label = format!("b/{path}");
        patch.push_str(
            &diff
                .unified_diff()
                .header(&old_label, &new_label)
                .to_string(),
        );
        files.push(FilePreview {
            path,
            before_digest: hash::digest_bytes(&bytes),
            after_digest: hash::digest_bytes(after.as_bytes()),
            edits: resolved.len(),
        });
    }
    if !refusals.is_empty() {
        files.clear();
        patch.clear();
        total_edits = 0;
    }
    Ok(EditPreview {
        snapshot_id: request.snapshot_id.to_string(),
        files,
        refusals,
        patch,
        total_edits,
    })
}

/// Apply an LSP WorkspaceEdit to snapshot bytes without touching the disk.
///
/// This is the write-side counterpart of [`preview_workspace_edit`]: callers
/// use the returned bytes to populate a ChangeSet staging tree, which then goes
/// through prepared-digest approval and the journaled apply gate. The function
/// never starts a language server and never writes a repository file.
pub fn apply_workspace_edit(
    store: &Store,
    request: &PreviewRequest<'_>,
    edit: &Value,
) -> Result<BTreeMap<String, Vec<u8>>> {
    validate::require_id("project_id", request.project_id)?;
    validate::require_digest("snapshot_id", request.snapshot_id)?;
    store.check_snapshot_scope(request.project_id, request.snapshot_id)?;
    let max_files = request.max_files.clamp(1, MAX_FILES);
    let encoding = match edit["position_encoding"].as_str().unwrap_or("utf-16") {
        "utf-8" => Encoding::Utf8,
        "utf-16" => Encoding::Utf16,
        "utf-32" => Encoding::Utf32,
        other => {
            return Err(CoreError::invalid(format!(
                "unsupported position_encoding {other}"
            )))
        }
    };
    let mut refusals = Vec::new();
    let mut per_file: BTreeMap<String, Vec<TextEdit>> = BTreeMap::new();
    if let Some(changes) = edit["changes"].as_object() {
        for (uri, edits) in changes {
            match normalize_uri(uri, request) {
                Ok(path) => {
                    if let Some(edits) = edits.as_array() {
                        collect_edits(&path, edits, &mut per_file, &mut refusals);
                    } else {
                        refusals.push(Refusal {
                            code: "invalid_edit".into(),
                            path: Some(path),
                            message: "changes entry is not an edit array".into(),
                        });
                    }
                }
                Err(refusal) => refusals.push(refusal),
            }
        }
    }
    if let Some(document_changes) = edit["documentChanges"].as_array() {
        for change in document_changes {
            if change["kind"].is_string() {
                refusals.push(Refusal {
                    code: "resource_operation_not_supported".into(),
                    path: change["textDocument"]["uri"].as_str().map(str::to_string),
                    message: "create/rename/delete resource operations are not part of a v1 rename".into(),
                });
                continue;
            }
            let Some(uri) = change["textDocument"]["uri"].as_str() else {
                refusals.push(Refusal {
                    code: "invalid_document_change".into(),
                    path: None,
                    message: "documentChanges entry has no textDocument.uri".into(),
                });
                continue;
            };
            let path = match normalize_uri(uri, request) {
                Ok(path) => path,
                Err(refusal) => {
                    refusals.push(refusal);
                    continue;
                }
            };
            let empty: &[Value] = &[];
            collect_edits(
                &path,
                change["edits"].as_array().map(Vec::as_slice).unwrap_or(empty),
                &mut per_file,
                &mut refusals,
            );
        }
    }
    if !refusals.is_empty() {
        return Err(CoreError::invalid(format!(
            "workspace edit refused: {}",
            refusals[0].message
        )));
    }
    if per_file.is_empty() {
        return Err(CoreError::invalid("workspace edit contains no text edits"));
    }
    if per_file.len() > max_files {
        return Err(CoreError::invalid(format!(
            "edit touches {} files; the apply limit is {max_files}",
            per_file.len()
        )));
    }
    let mut output = BTreeMap::new();
    for (path, edits) in per_file {
        let entry = snapshot_file(store, request, &path)?.ok_or_else(|| {
            CoreError::invalid(format!("{path} is not part of the declared snapshot"))
        })?;
        if !allowed(request.allowed_paths, &path) {
            return Err(CoreError::invalid(format!(
                "{path} is outside allowed_paths"
            )));
        }
        let bytes = store
            .read_blob(&entry.blob_digest)?
            .ok_or_else(|| CoreError::Storage(format!("blob {} missing", entry.blob_digest)))?;
        let text = std::str::from_utf8(&bytes).map_err(|_| {
            CoreError::unsupported(format!("{path} is not UTF-8; rename unsupported"))
        })?;
        let index = LineIndex::new(&bytes);
        let mut resolved = Vec::with_capacity(edits.len());
        for edit in &edits {
            let (start, end) = edit
                .into_bytes(&index, &bytes, encoding)
                .map_err(|code| CoreError::invalid(format!("{path}: {code}")))?;
            resolved.push((start, end, edit.new_text.clone()));
        }
        resolved.sort_by_key(|(start, end, _)| (*start, *end));
        for window in resolved.windows(2) {
            let (a_start, a_end, _) = &window[0];
            let (b_start, _b_end, _) = &window[1];
            if b_start < a_end || (b_start == a_start && a_start == a_end) {
                return Err(CoreError::invalid(format!(
                    "{path}: edits overlap or insert at the same position"
                )));
            }
        }
        let mut after = String::with_capacity(text.len());
        let mut cursor = 0usize;
        for (start, end, replacement) in &resolved {
            after.push_str(&text[cursor..*start as usize]);
            after.push_str(replacement);
            cursor = *end as usize;
        }
        after.push_str(&text[cursor..]);
        output.insert(path, after.into_bytes());
    }
    Ok(output)
}

#[derive(Debug, Clone)]
struct TextEdit {
    start_line: u64,
    start_character: u64,
    end_line: u64,
    end_character: u64,
    new_text: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Encoding {
    Utf8,
    Utf16,
    Utf32,
}

impl TextEdit {
    fn into_bytes(
        &self,
        index: &LineIndex,
        bytes: &[u8],
        encoding: Encoding,
    ) -> std::result::Result<(u64, u64), &'static str> {
        let convert = |line: u64, character: u64| match encoding {
            Encoding::Utf8 => index
                .line_start(line)
                .and_then(|start| start.checked_add(character)),
            Encoding::Utf16 => index.utf16_col_to_byte(bytes, line, character),
            Encoding::Utf32 => index.utf32_col_to_byte(bytes, line, character),
        };
        let start = convert(self.start_line, self.start_character).ok_or("invalid_range")?;
        let end = convert(self.end_line, self.end_character).ok_or("invalid_range")?;
        if start > end || end > bytes.len() as u64 {
            return Err("invalid_range");
        }
        Ok((start, end))
    }
}

fn collect_edits(
    path: &str,
    edits: &[Value],
    per_file: &mut BTreeMap<String, Vec<TextEdit>>,
    refusals: &mut Vec<Refusal>,
) {
    if edits.is_empty() {
        return;
    }
    if edits.len() > MAX_EDITS_PER_FILE {
        refusals.push(Refusal {
            code: "too_many_edits".into(),
            path: Some(path.to_string()),
            message: format!("more than {MAX_EDITS_PER_FILE} edits for one file"),
        });
        return;
    }
    let mut collected = Vec::with_capacity(edits.len());
    for edit in edits {
        let range = &edit["range"];
        let (Some(start_line), Some(start_character), Some(end_line), Some(end_character)) = (
            range["start"]["line"].as_u64(),
            range["start"]["character"].as_u64(),
            range["end"]["line"].as_u64(),
            range["end"]["character"].as_u64(),
        ) else {
            refusals.push(Refusal {
                code: "invalid_edit".into(),
                path: Some(path.to_string()),
                message: "edit has no valid range".into(),
            });
            return;
        };
        let Some(new_text) = edit["newText"].as_str() else {
            refusals.push(Refusal {
                code: "invalid_edit".into(),
                path: Some(path.to_string()),
                message: "edit has no newText".into(),
            });
            return;
        };
        collected.push(TextEdit {
            start_line,
            start_character,
            end_line,
            end_character,
            new_text: new_text.to_string(),
        });
    }
    per_file.insert(path.to_string(), collected);
}

fn normalize_uri(uri: &str, request: &PreviewRequest<'_>) -> std::result::Result<String, Refusal> {
    let refusal = |code: &str, message: &str| Refusal {
        code: code.to_string(),
        path: Some(uri.to_string()),
        message: message.to_string(),
    };
    let without_scheme = if let Some(rest) = uri.strip_prefix("archmeld-snapshot:") {
        let rest = rest.trim_start_matches('/');
        let mut parts = rest.splitn(2, '/');
        let snapshot = parts.next().unwrap_or("");
        if snapshot != request.snapshot_id {
            return Err(refusal(
                "snapshot_uri_mismatch",
                "URI names a different snapshot than the preview request",
            ));
        }
        parts.next().unwrap_or("").to_string()
    } else if let Some(rest) = uri.strip_prefix("file://") {
        // Real LSP servers return absolute file URIs. They are accepted only
        // when the preview request names an authorized workspace root.
        let Some(root) = request.workspace_root else {
            return Err(refusal(
                "absolute_uri_not_authorized",
                "file URIs require an authorized workspace root on the preview request",
            ));
        };
        #[allow(unused_mut)]
        let mut decoded = percent_decode(rest.trim_start_matches('/'));
        #[cfg(windows)]
        if decoded.starts_with('/') && decoded.as_bytes().get(2) == Some(&b':') {
            decoded.remove(0);
        }
        let candidate = Path::new(&decoded);
        let root_canonical = std::fs::canonicalize(root).unwrap_or_else(|_| root.to_path_buf());
        let candidate_canonical =
            std::fs::canonicalize(candidate).unwrap_or_else(|_| candidate.to_path_buf());
        let relative = candidate_canonical
            .strip_prefix(&root_canonical)
            .map_err(|_| {
                refusal(
                    "absolute_uri_not_authorized",
                    "file URI resolves outside the authorized workspace root",
                )
            })?;
        relative.to_string_lossy().replace('\\', "/")
    } else if uri.contains(':') || uri.starts_with('/') || uri.starts_with('\\') {
        return Err(refusal(
            "absolute_uri_not_authorized",
            "only repository-relative paths, archmeld-snapshot URIs or authorized file URIs are accepted",
        ));
    } else {
        uri.to_string()
    };
    let path = without_scheme.replace('\\', "/");
    validate::validate_path("workspace_edit.uri", &path)
        .map_err(|_| refusal("invalid_path", "path is not a relative repository path"))?;
    Ok(path)
}

fn percent_decode(text: &str) -> String {
    let bytes = text.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut index = 0;
    while index < bytes.len() {
        if bytes[index] == b'%' && index + 2 < bytes.len() {
            if let (Some(high), Some(low)) = (
                (bytes[index + 1] as char).to_digit(16),
                (bytes[index + 2] as char).to_digit(16),
            ) {
                out.push((high * 16 + low) as u8);
                index += 3;
                continue;
            }
        }
        out.push(bytes[index]);
        index += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

fn allowed(allowed_paths: &[String], path: &str) -> bool {
    if allowed_paths.is_empty() {
        return true;
    }
    allowed_paths.iter().any(|allowed| {
        if allowed.ends_with('/') {
            path.starts_with(allowed)
        } else {
            path == allowed
        }
    })
}

fn snapshot_file(
    store: &Store,
    request: &PreviewRequest<'_>,
    path: &str,
) -> Result<Option<FileEntry>> {
    let row: Option<(String, i64, String, String, i64)> = store
        .conn
        .query_row(
            "SELECT blob_digest, size_bytes, role, origin, executable
             FROM files WHERE project_id=?1 AND snapshot_id=?2 AND path=?3",
            (request.project_id, request.snapshot_id, path),
            |row| {
                Ok((
                    row.get(0)?,
                    row.get(1)?,
                    row.get(2)?,
                    row.get(3)?,
                    row.get(4)?,
                ))
            },
        )
        .ok();
    Ok(row.map(
        |(blob_digest, size_bytes, role, origin, executable)| FileEntry {
            path: path.to_string(),
            blob_digest,
            size_bytes: size_bytes.max(0) as u64,
            role: parse_role(&role),
            origin: parse_origin(&origin),
            executable: executable != 0,
        },
    ))
}

fn parse_role(value: &str) -> crate::store::FileRole {
    use crate::store::FileRole::*;
    match value {
        "config" => Config,
        "generated" => Generated,
        "protocol" => Protocol,
        "test" => Test,
        "other" => Other,
        _ => Source,
    }
}

fn parse_origin(value: &str) -> crate::store::FileOrigin {
    use crate::store::FileOrigin::*;
    match value {
        "untracked" => Untracked,
        "generated" => Generated,
        "external" => External,
        _ => Tracked,
    }
}

/// Guard used by adapters before launching an LSP process. The broker itself
/// never spawns anything; this exists so call sites cannot forget the check.
pub fn require_readonly_profile(profile: &Profile) -> Result<()> {
    if profile.supports("definition_reference") {
        Ok(())
    } else {
        Err(CoreError::unsupported(format!(
            "profile {} has no verified definition/reference capability",
            profile.id
        )))
    }
}

/// Preview never touches the filesystem; this helper makes that explicit for
/// callers that hold a workspace path.
pub fn preview_is_read_only(workspace_root: &Path) -> bool {
    let _ = workspace_root;
    true
}
