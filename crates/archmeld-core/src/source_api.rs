// Copyright 2026 ARCHMELD contributors
// SPDX-License-Identifier: Apache-2.0

//! Historical source access (AT016).
//!
//! Source is always read from the snapshot's content-addressed blob. A newer
//! file on disk, or a newer snapshot, can never be substituted silently: the
//! response carries the snapshot id and the exact anchor that was used.

use crate::error::{CoreError, Result};
use crate::source::LineIndex;
use crate::store::{validate, Store};
use serde::Deserialize;
use serde_json::{json, Value};

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SourceRequest {
    pub project_id: String,
    pub snapshot_id: String,
    pub entity_id: String,
    pub anchor_index: u32,
    pub max_bytes: u32,
}

pub fn read_source(store: &Store, raw: &Value) -> Result<Value> {
    let request: SourceRequest = serde_json::from_value(raw.clone())
        .map_err(|err| CoreError::invalid(format!("source request: {err}")))?;
    validate::require_id("project_id", &request.project_id)?;
    validate::require_digest("snapshot_id", &request.snapshot_id)?;
    validate::require_id("entity_id", &request.entity_id)?;
    if request.max_bytes == 0 || request.max_bytes > 1_048_576 {
        return Err(CoreError::invalid("max_bytes must be 1..=1048576"));
    }
    store.check_snapshot_scope(&request.project_id, &request.snapshot_id)?;

    let entity = store
        .get_entity(&request.project_id, &request.snapshot_id, &request.entity_id)?
        .ok_or_else(|| CoreError::not_found(format!("entity {} not found", request.entity_id)))?;
    let anchors = entity["anchors"]
        .as_array()
        .ok_or_else(|| CoreError::Storage("entity has no anchors array".to_string()))?;
    let anchor = anchors
        .get(request.anchor_index as usize)
        .cloned()
        .ok_or_else(|| CoreError::not_found("anchor_index is out of range"))?;
    let path = anchor["path"]
        .as_str()
        .ok_or_else(|| CoreError::Storage("anchor has no path".to_string()))?
        .to_string();
    let blob_digest = anchor["blob_digest"]
        .as_str()
        .ok_or_else(|| CoreError::Storage("anchor has no blob digest".to_string()))?
        .to_string();

    // The anchor must still point at the blob the snapshot recorded for that
    // path; a mismatched anchor would silently read the wrong bytes.
    let recorded: Option<String> = store
        .conn
        .query_row(
            "SELECT blob_digest FROM files WHERE project_id=?1 AND snapshot_id=?2 AND path=?3",
            (&request.project_id, &request.snapshot_id, &path),
            |row| row.get(0),
        )
        .ok();
    if recorded.as_deref() != Some(blob_digest.as_str()) {
        return Err(CoreError::conflict(format!(
            "anchor blob for {path} does not match the snapshot file entry"
        )));
    }
    let bytes = store
        .read_blob(&blob_digest)?
        .ok_or_else(|| CoreError::not_found(format!("blob {blob_digest} is not stored")))?;
    let text = std::str::from_utf8(&bytes).map_err(|_| {
        CoreError::unsupported(format!("{path} is not valid UTF-8; source is unsupported"))
    })?;
    let range = crate::anchor::byte_range(&anchor, &bytes)?;
    let (start, end) = (range.start, range.end);
    let mut slice_end = end;
    let max = request.max_bytes as u64;
    let truncated = end - start > max;
    if truncated {
        slice_end = start + max;
        while slice_end > start && !text.is_char_boundary(slice_end as usize) {
            slice_end -= 1;
        }
    }
    let excerpt = &bytes[start as usize..slice_end as usize];
    let excerpt = std::str::from_utf8(excerpt)
        .map_err(|_| CoreError::invalid("anchor range splits a UTF-8 character".to_string()))?;
    Ok(json!({
        "snapshot_id": request.snapshot_id,
        "anchor": anchor,
        "text": excerpt,
        "redacted": false,
        "truncated": truncated,
    }))
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SourceFileRequest {
    pub project_id: String,
    pub snapshot_id: String,
    /// Snapshot-relative POSIX path. This is never resolved on disk.
    pub path: String,
    /// One-based line to center the excerpt on.
    pub line: Option<u64>,
    /// Lines of context on each side (default 3, at most 200).
    pub context_lines: Option<u64>,
    pub max_bytes: Option<u32>,
}

/// Read a line window from one snapshot file. Used by trace views that retain
/// an exact source location (for example a Tracy zone) but have no entity
/// anchor. The bytes still come only from the snapshot CAS, and the recorded
/// snapshot file digest is returned with the excerpt.
pub fn read_source_file(store: &Store, raw: &Value) -> Result<Value> {
    let request: SourceFileRequest = serde_json::from_value(raw.clone())
        .map_err(|err| CoreError::invalid(format!("source file request: {err}")))?;
    validate::require_id("project_id", &request.project_id)?;
    validate::require_digest("snapshot_id", &request.snapshot_id)?;
    validate::validate_path("path", &request.path)?;
    let max_bytes = request.max_bytes.unwrap_or(262_144);
    if max_bytes == 0 || max_bytes > 1_048_576 {
        return Err(CoreError::invalid("max_bytes must be 1..=1048576"));
    }
    let context_lines = request.context_lines.unwrap_or(3);
    if context_lines > 200 {
        return Err(CoreError::invalid("context_lines must be <= 200"));
    }
    store.check_snapshot_scope(&request.project_id, &request.snapshot_id)?;
    let blob_digest: Option<String> = store
        .conn
        .query_row(
            "SELECT blob_digest FROM files WHERE project_id=?1 AND snapshot_id=?2 AND path=?3",
            (&request.project_id, &request.snapshot_id, &request.path),
            |row| row.get(0),
        )
        .ok();
    let Some(blob_digest) = blob_digest else {
        return Err(CoreError::not_found(format!(
            "{} is not part of snapshot {}",
            request.path, request.snapshot_id
        )));
    };
    let bytes = store
        .read_blob(&blob_digest)?
        .ok_or_else(|| CoreError::not_found(format!("blob {blob_digest} is not stored")))?;
    let text = std::str::from_utf8(&bytes).map_err(|_| {
        CoreError::unsupported(format!(
            "{} is not valid UTF-8; source is unsupported",
            request.path
        ))
    })?;
    let index = LineIndex::new(&bytes);
    let total_lines = index.line_count();
    if total_lines == 0 {
        return Ok(json!({
            "snapshot_id": request.snapshot_id,
            "path": request.path,
            "blob_digest": blob_digest,
            "requested_line": request.line,
            "line_start": 0,
            "line_end": 0,
            "total_lines": 0,
            "text": "",
            "truncated": false,
        }));
    }
    let requested_line = request.line.unwrap_or(1).max(1).min(total_lines);
    let first_line = requested_line.saturating_sub(context_lines).max(1);
    let last_line = (requested_line + context_lines).min(total_lines);
    let start = index
        .line_start(first_line - 1)
        .ok_or_else(|| CoreError::invalid("line window start is out of range"))?;
    let end = index.line_start(last_line).unwrap_or(index.len());
    let mut slice_end = end;
    let truncated = end - start > max_bytes as u64;
    if truncated {
        slice_end = start + max_bytes as u64;
        while slice_end > start && !text.is_char_boundary(slice_end as usize) {
            slice_end -= 1;
        }
    }
    let excerpt = &bytes[start as usize..slice_end as usize];
    let excerpt = std::str::from_utf8(excerpt)
        .map_err(|_| CoreError::invalid("line window splits a UTF-8 character".to_string()))?;
    Ok(json!({
        "snapshot_id": request.snapshot_id,
        "path": request.path,
        "blob_digest": blob_digest,
        "requested_line": requested_line,
        "line_start": first_line,
        "line_end": last_line,
        "total_lines": total_lines,
        "text": excerpt,
        "truncated": truncated,
    }))
}
