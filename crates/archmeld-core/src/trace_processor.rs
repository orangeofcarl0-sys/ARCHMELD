// Copyright 2026 ARCHMELD contributors
// SPDX-License-Identifier: Apache-2.0

//! Optional Perfetto trace-processor query backend (S06).
//!
//! The default Perfetto path decodes a protobuf subset in process. This module
//! runs the pinned `trace_processor_shell` with explicit SQL and maps only the
//! rows it returns: slices become spans, counter tracks become counter series.
//! No state intervals, args, flows or process metadata are invented.

use crate::error::{CoreError, Result};
use crate::hash;
use crate::scenario::run_command_capture;
use crate::trace_adapters::{base_trace_with_metrics, SpanDraft, TraceLocator, TraceMeta};
use serde_json::{json, Map, Value};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::time::Duration;

const QUERY_TIMEOUT: Duration = Duration::from_secs(120);
const MAX_SLICES: usize = 1_000_000;
const MAX_COUNTERS: usize = 100_000;

const SLICE_QUERY: &str = "SELECT s.ts, s.dur, s.name, s.depth, s.track_id, \
     COALESCE(t.name, '') AS track_name, COALESCE(t.type, '') AS track_type, \
     COALESCE(s.category, '') AS category \
     FROM slice s LEFT JOIN track t ON t.id = s.track_id \
     ORDER BY s.ts, s.track_id, s.depth";

const COUNTER_QUERY: &str = "SELECT c.ts, c.value, c.track_id, \
     COALESCE(ct.name, '') AS track_name, COALESCE(ct.unit, '') AS unit \
     FROM counter c LEFT JOIN counter_track ct ON ct.id = c.track_id \
     ORDER BY c.ts, c.track_id";

pub struct TraceProcessorCapture {
    pub trace: Value,
    pub tool_path: PathBuf,
    pub tool_version: String,
    pub tool_sha256: String,
    pub slice_rows: usize,
    pub counter_rows: usize,
}

pub fn perfetto_trace_via_processor(
    trace_path: &Path,
    processor_path: &Path,
    locator: &TraceLocator,
    meta: &TraceMeta,
) -> Result<TraceProcessorCapture> {
    if !trace_path.is_file() {
        return Err(CoreError::not_found(format!(
            "trace {} is not a file",
            trace_path.display()
        )));
    }
    if !processor_path.is_file() {
        return Err(CoreError::not_found(format!(
            "trace processor {} is not a file",
            processor_path.display()
        )));
    }
    let tool_version = processor_version(processor_path)?;
    let tool_sha256 = hash::digest_bytes(&std::fs::read(processor_path)?);
    let slice_csv = run_query(processor_path, trace_path, SLICE_QUERY)?;
    let counter_csv = run_query(processor_path, trace_path, COUNTER_QUERY)?;
    let slice_rows = parse_csv(&slice_csv)?;
    let counter_rows = parse_csv(&counter_csv)?;
    if slice_rows.is_empty() || counter_rows.is_empty() {
        return Err(CoreError::invalid(
            "trace processor returned no CSV header for an explicit query",
        ));
    }
    if slice_rows.len() > 1 + MAX_SLICES {
        return Err(CoreError::invalid(format!(
            "resource_exhausted: {} slices exceed {MAX_SLICES}",
            slice_rows.len() - 1
        )));
    }

    let mut losses: Vec<String> = Vec::new();
    let mut spans: Vec<SpanDraft> = Vec::new();
    // (track_id, depth) -> (span id, end_ns)
    let mut stack: HashMap<(String, i64), (String, i128)> = HashMap::new();
    for (index, row) in slice_rows.iter().skip(1).enumerate() {
        let (ts, dur, name, depth, track_id, track_name, track_type, category) =
            match row.as_slice() {
                [ts, dur, name, depth, track_id, track_name, track_type, category] => {
                    (ts, dur, name, depth, track_id, track_name, track_type, category)
                }
                _ => {
                    return Err(CoreError::invalid(
                        "trace processor slice row does not have eight columns",
                    ))
                }
            };
        let start = parse_i128("slice.ts", ts)?;
        let duration = parse_i128("slice.dur", dur)?;
        if duration < 0 {
            losses.push(format!(
                "unfinished slice {name} at {start} ns was skipped"
            ));
            continue;
        }
        let depth = depth.parse::<i64>().map_err(|_| {
            CoreError::invalid(format!("slice.depth {depth:?} is not an integer"))
        })?;
        let parent = if depth > 0 {
            stack
                .get(&(track_id.clone(), depth - 1))
                .filter(|(_, end)| *end >= start)
                .map(|(id, _)| id.clone())
        } else {
            None
        };
        let id = format!("slice.{}", index + 1);
        let mut attributes = Map::new();
        attributes.insert("perfetto.track_id".into(), json!(track_id));
        attributes.insert("perfetto.track_name".into(), json!(track_name));
        attributes.insert("perfetto.track_type".into(), json!(track_type));
        attributes.insert("perfetto.category".into(), json!(category));
        attributes.insert("perfetto.depth".into(), json!(depth));
        attributes.insert("perfetto.source".into(), json!("trace_processor_sql"));
        spans.push(SpanDraft {
            id: id.clone(),
            parent,
            name: name.clone(),
            kind: "slice".to_string(),
            start_ns: start,
            end_ns: start + duration,
            process_id: None,
            thread_id: Some(track_id.clone()),
            entity_id: locator.symbol(name),
            attributes,
        });
        stack.insert((track_id.clone(), depth), (id, start + duration));
    }

    let mut counters: Vec<Value> = Vec::new();
    let mut by_track: HashMap<String, (String, String, Vec<Value>)> = HashMap::new();
    for row in counter_rows.iter().skip(1) {
        let (ts, value, track_id, track_name, unit) = match row.as_slice() {
            [ts, value, track_id, track_name, unit] => (ts, value, track_id, track_name, unit),
            _ => {
                return Err(CoreError::invalid(
                    "trace processor counter row does not have five columns",
                ))
            }
        };
        if unit.eq_ignore_ascii_case("state") {
            losses.push(format!(
                "state counter on track {track_id} ({track_name}) was not imported"
            ));
            continue;
        }
        let timestamp = parse_i128("counter.ts", ts)?;
        let numeric: f64 = value.parse().map_err(|_| {
            CoreError::invalid(format!("counter.value {value:?} is not a number"))
        })?;
        if !numeric.is_finite() {
            return Err(CoreError::invalid(format!(
                "counter.value {value:?} is not finite"
            )));
        }
        let entry = by_track.entry(track_id.clone()).or_insert_with(|| {
            let name = if track_name.is_empty() || track_name == "[NULL]" {
                format!("counter.track.{track_id}")
            } else {
                track_name.clone()
            };
            let unit = if unit.is_empty() {
                None
            } else {
                Some(unit.clone())
            };
            (name, unit.unwrap_or_default(), Vec::new())
        });
        entry.2.push(json!({
            "timestamp_ns": timestamp.to_string(),
            "value": numeric,
        }));
        if by_track.len() > MAX_COUNTERS {
            return Err(CoreError::invalid(format!(
                "resource_exhausted: more than {MAX_COUNTERS} counter tracks"
            )));
        }
    }
    for (track_id, (name, unit, samples)) in by_track {
        let mut attributes = Map::new();
        attributes.insert("perfetto.track_id".into(), json!(track_id));
        attributes.insert("perfetto.source".into(), json!("trace_processor_sql"));
        counters.push(json!({
            "counter_id": format!("counter.{track_id}"),
            "name": name,
            "unit": if unit.is_empty() { Value::Null } else { json!(unit) },
            "attributes": attributes,
            "samples": samples,
        }));
    }

    let mut meta = meta.clone();
    meta.tool = "perfetto-trace-processor".to_string();
    meta.version = tool_version.clone();
    meta.loss_notes.extend(losses.clone());
    if !losses.is_empty() {
        meta.lossy = true;
    }
    let trace = base_trace_with_metrics(&meta, spans, counters, Vec::new())?;
    Ok(TraceProcessorCapture {
        trace,
        tool_path: processor_path.to_path_buf(),
        tool_version,
        tool_sha256,
        slice_rows: slice_rows.len() - 1,
        counter_rows: counter_rows.len() - 1,
    })
}

fn processor_version(path: &Path) -> Result<String> {
    let capture = run_command_capture(
        &[path.display().to_string(), "--version".to_string()],
        path.parent().unwrap_or(Path::new(".")),
        &[],
        Duration::from_secs(30),
        None,
    )?;
    if capture.exit_code != Some(0) {
        return Err(CoreError::invalid(format!(
            "trace processor --version exited {:?}: {}",
            capture.exit_code,
            String::from_utf8_lossy(&capture.stderr).trim()
        )));
    }
    let text = String::from_utf8_lossy(&capture.stdout);
    text.lines()
        .map(str::trim)
        .find(|line| !line.is_empty())
        .map(str::to_string)
        .ok_or_else(|| CoreError::invalid("trace processor --version produced no output"))
}

fn run_query(path: &Path, trace: &Path, sql: &str) -> Result<String> {
    let capture = run_command_capture(
        &[
            path.display().to_string(),
            "--quiet".to_string(),
            "query".to_string(),
            trace.display().to_string(),
            sql.to_string(),
        ],
        path.parent().unwrap_or(Path::new(".")),
        &[],
        QUERY_TIMEOUT,
        None,
    )?;
    if capture.timed_out {
        return Err(CoreError::invalid(
            "trace processor query exceeded its timeout",
        ));
    }
    if capture.stdout_truncated {
        return Err(CoreError::invalid(
            "resource_exhausted: trace processor output exceeded the 4 MiB capture limit",
        ));
    }
    if capture.exit_code != Some(0) {
        return Err(CoreError::invalid(format!(
            "trace processor query exited {:?}: {}",
            capture.exit_code,
            String::from_utf8_lossy(&capture.stderr).trim()
        )));
    }
    String::from_utf8(capture.stdout)
        .map_err(|_| CoreError::invalid("trace processor output is not UTF-8"))
}

fn parse_i128(field: &str, text: &str) -> Result<i128> {
    text.parse::<i128>()
        .map_err(|_| CoreError::invalid(format!("{field} {text:?} is not an integer")))
}

/// Minimal quoted CSV parser for `trace_processor_shell` output. It supports
/// doubled quotes inside quoted cells and rejects an unterminated quote.
fn parse_csv(text: &str) -> Result<Vec<Vec<String>>> {
    let mut rows = Vec::new();
    let mut row: Vec<String> = Vec::new();
    let mut cell = String::new();
    let mut quoted = false;
    let mut chars = text.chars().peekable();
    while let Some(ch) = chars.next() {
        match ch {
            '"' if quoted => {
                if chars.peek() == Some(&'"') {
                    cell.push('"');
                    chars.next();
                } else {
                    quoted = false;
                }
            }
            '"' => quoted = true,
            ',' if !quoted => {
                row.push(std::mem::take(&mut cell));
            }
            '\r' if !quoted => {}
            '\n' if !quoted => {
                row.push(std::mem::take(&mut cell));
                if !(row.len() == 1 && row[0].is_empty()) {
                    rows.push(std::mem::take(&mut row));
                } else {
                    row.clear();
                }
            }
            other => cell.push(other),
        }
    }
    if quoted {
        return Err(CoreError::invalid(
            "trace processor CSV has an unterminated quoted cell",
        ));
    }
    if !cell.is_empty() || !row.is_empty() {
        row.push(cell);
        rows.push(row);
    }
    Ok(rows)
}
