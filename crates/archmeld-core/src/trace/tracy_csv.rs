// Copyright 2026 ARCHMELD contributors
// SPDX-License-Identifier: Apache-2.0

//! Native Tracy support: retain the raw `.tracy` capture and normalize the
//! explicit per-event summary exported by `tracy-csvexport -u -p`.
//!
//! Tracy's own profiler protocol is not reimplemented here. The capture is a
//! real tool artifact kept in the content-addressed store, and the CSV export
//! is the declared summary. Zones keep their measured `ns_since_start` and
//! `exec_time_ns`; plots become counters. Frame-rate rows are not application
//! observations and are ignored. Source attribution uses only an exact
//! `(src_file, src_line)` mapping supplied by the caller.

use crate::error::{CoreError, Result};
use crate::runtime::{import_trace, RuntimeImport};
use crate::store::Store;
use crate::trace_adapters::{base_trace_with_metrics, SpanDraft, TraceLocator, TraceMeta};
use serde_json::{json, Map, Value};
use std::collections::BTreeMap;

const HEADER: [&str; 7] = [
    "name",
    "src_file",
    "src_line",
    "ns_since_start",
    "exec_time_ns",
    "thread",
    "value",
];

pub fn tracy_csv(text: &str, locator: &TraceLocator, meta: &TraceMeta) -> Result<Value> {
    let normalized = text.replace("\r\n", "\n");
    let mut lines = normalized.lines();
    let header = lines
        .next()
        .ok_or_else(|| CoreError::invalid("Tracy CSV is empty"))?;
    let columns = parse_csv_line(header)?;
    if columns != HEADER {
        return Err(CoreError::unsupported(
            "Tracy CSV header must be name,src_file,src_line,ns_since_start,exec_time_ns,thread,value",
        ));
    }
    let mut spans = Vec::new();
    let mut counters: BTreeMap<String, Value> = BTreeMap::new();
    let mut frame_rows = 0usize;
    for (index, line) in lines.enumerate() {
        if line.trim().is_empty() {
            continue;
        }
        let row = parse_csv_line(line)?;
        if row.len() != HEADER.len() {
            return Err(CoreError::invalid(format!(
                "Tracy CSV row {} has {} columns, expected {}",
                index + 2,
                row.len(),
                HEADER.len()
            )));
        }
        let name = &row[0];
        let source_file = &row[1];
        let source_line = &row[2];
        let ns_since_start = &row[3];
        let exec_time_ns = &row[4];
        let thread = &row[5];
        let value = &row[6];
        if name == "Frame" {
            frame_rows += 1;
            continue;
        }
        if !value.is_empty() && exec_time_ns.is_empty() {
            let timestamp = parse_ns("ns_since_start", ns_since_start)?;
            let numeric: f64 = value
                .parse()
                .map_err(|_| CoreError::invalid(format!("Tracy plot {name} has a non-numeric value")))?;
            if !numeric.is_finite() {
                return Err(CoreError::invalid(format!(
                    "Tracy plot {name} has a non-finite value"
                )));
            }
            let entry = counters.entry(name.clone()).or_insert_with(|| {
                let mut attributes = Map::new();
                attributes.insert("capture.tool_format".to_string(), json!("tracy-plot"));
                json!({
                    "counter_id": format!("tracy.plot.{}", sanitize(name)),
                    "name": name,
                    "clock_id": "default",
                    "attributes": attributes,
                    "samples": [],
                })
            });
            entry["samples"]
                .as_array_mut()
                .unwrap()
                .push(json!({
                    "timestamp_ns": timestamp.to_string(),
                    "value": numeric,
                    "thread_id": if thread.is_empty() { Value::Null } else { json!(thread) },
                }));
            continue;
        }
        if exec_time_ns.is_empty() {
            continue;
        }
        let start = parse_ns("ns_since_start", ns_since_start)?;
        let duration = parse_ns("exec_time_ns", exec_time_ns)?;
        let line_number = source_line.parse::<u64>().ok();
        let entity_id = line_number
            .and_then(|line| locator.location(source_file, line));
        let mut attributes = Map::new();
        attributes.insert("capture.tool_format".to_string(), json!("tracy-zone"));
        attributes.insert("tracy.src_file".to_string(), json!(source_file));
        attributes.insert("tracy.src_line".to_string(), json!(source_line));
        spans.push(SpanDraft {
            id: format!("tracy.zone.{}", spans.len() + 1),
            parent: None,
            name: name.clone(),
            kind: "zone".to_string(),
            start_ns: start,
            end_ns: start.saturating_add(duration),
            process_id: None,
            thread_id: if thread.is_empty() {
                None
            } else {
                Some(thread.clone())
            },
            entity_id,
            attributes,
        });
    }
    let mut trace = base_trace_with_metrics(
        meta,
        spans,
        counters.into_values().collect(),
        Vec::new(),
    )?;
    if frame_rows > 0 {
        trace["capture"]["lossy"] = json!(true);
        trace["capture"]["loss_notes"]
            .as_array_mut()
            .unwrap()
            .push(json!(format!(
                "{frame_rows} frame-rate rows were ignored; they are not application events"
            )));
    }
    Ok(trace)
}

/// Retain a real `.tracy` capture and import its explicit CSV summary. The
/// native artifact is referenced from the ledger so retention can see it, and
/// its digest is recorded in the normalized trace capture block.
pub fn import_tracy_capture(
    store: &mut Store,
    request: &RuntimeImport<'_>,
    capture: &[u8],
    csv: &str,
    locator: &TraceLocator,
    meta: &TraceMeta,
) -> Result<Value> {
    if capture.is_empty() {
        return Err(CoreError::invalid("Tracy capture is empty"));
    }
    let digest = store.put_blob(capture, "application/vnd.tracy")?;
    let mut trace = tracy_csv(csv, locator, meta)?;
    trace["capture"]["native_artifact"] = json!({
        "tool": "tracy-capture",
        "format": "tracy",
        "artifact_digest": digest,
        "size": capture.len(),
    });
    let result = import_trace(store, request, &trace)?;
    store.reference_artifact("runtime_native", request.run_id, &digest)?;
    Ok(result)
}

fn parse_ns(field: &str, text: &str) -> Result<i128> {
    if text.is_empty() || !text.bytes().all(|byte| byte.is_ascii_digit()) {
        return Err(CoreError::invalid(format!(
            "Tracy CSV {field} must be a non-negative integer"
        )));
    }
    text.parse::<i128>()
        .map_err(|_| CoreError::invalid(format!("Tracy CSV {field} is out of range")))
}

fn parse_csv_line(line: &str) -> Result<Vec<String>> {
    let mut fields = Vec::new();
    let mut current = String::new();
    let mut quoted = false;
    let mut chars = line.chars().peekable();
    while let Some(ch) = chars.next() {
        match ch {
            '"' if quoted => {
                if chars.peek() == Some(&'"') {
                    current.push('"');
                    chars.next();
                } else {
                    quoted = false;
                }
            }
            '"' if current.is_empty() => quoted = true,
            ',' if !quoted => {
                fields.push(std::mem::take(&mut current));
            }
            other => current.push(other),
        }
    }
    if quoted {
        return Err(CoreError::invalid("unterminated quote in Tracy CSV"));
    }
    fields.push(current);
    Ok(fields)
}

fn sanitize(name: &str) -> String {
    let mut value: String = name
        .chars()
        .map(|ch| {
            if ch.is_ascii_alphanumeric() || matches!(ch, '_' | '.' | '-' | ':') {
                ch
            } else {
                '_'
            }
        })
        .collect();
    if value.len() > 120 {
        value.truncate(120);
    }
    if value.is_empty() {
        value.push_str("plot");
    }
    value
}
