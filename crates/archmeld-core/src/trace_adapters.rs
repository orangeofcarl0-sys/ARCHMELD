// Copyright 2026 ARCHMELD contributors
// SPDX-License-Identifier: Apache-2.0

//! Native capture adapters for the normalized runtime trace (S06).
//!
//! Each adapter consumes a real tool's output and produces the trace envelope
//! accepted by [`crate::runtime::import_trace`]. Source attribution is never
//! guessed from a function name: callers must declare exact
//! `(path, line) -> entity` locations, or an explicit symbol probe for tools
//! whose output carries no file/line (for example EventPipe speedscope).
//!
//! The adapters preserve the native capture semantics:
//! * Chrome Trace / VizTracer complete events keep their measured timestamps;
//! * stack profilers (JFR, V8, speedscope, pprof) build parent links from the
//!   recorded call stack, not from timestamp proximity;
//! * aggregated pprof data is placed on an explicitly labelled CPU-time axis;
//! * sampling and export losses are reported through `loss_notes`.

use crate::error::{CoreError, Result};
use serde_json::{json, Map, Value};
use std::collections::{BTreeMap, HashMap};

/// Exact source attribution supplied by the caller after static entities exist.
#[derive(Debug, Clone, Default)]
pub struct TraceLocator {
    locations: HashMap<(String, u64), String>,
    ranges: Vec<(String, u64, u64, String)>,
    symbols: HashMap<String, String>,
}

impl TraceLocator {
    pub fn new() -> Self {
        Self::default()
    }

    /// Declare an entity for a source file/line. Paths are matched after
    /// normalizing separators and `file://` prefixes.
    pub fn add_location(&mut self, path: &str, line: u64, entity_id: &str) {
        self.locations
            .insert((normalize_path(path), line), entity_id.to_string());
    }

    /// Declare a source range that belongs to one entity. This covers
    /// profilers that report the executed line inside a function rather than
    /// the declaration line.
    pub fn add_range(&mut self, path: &str, start_line: u64, end_line: u64, entity_id: &str) {
        self.ranges.push((
            normalize_path(path),
            start_line.min(end_line),
            start_line.max(end_line),
            entity_id.to_string(),
        ));
    }

    /// Declare an entity for an exact tool symbol. This is an explicit probe
    /// declaration, not an automatic name match.
    pub fn add_symbol(&mut self, symbol: &str, entity_id: &str) {
        self.symbols.insert(symbol.to_string(), entity_id.to_string());
    }

    pub fn location(&self, path: &str, line: u64) -> Option<String> {
        let path = normalize_path(path);
        if let Some(entity_id) = self.locations.get(&(path.clone(), line)) {
            return Some(entity_id.clone());
        }
        let mut best: Option<(&u64, &u64, &String)> = None;
        for (range_path, start, end, entity_id) in &self.ranges {
            if range_path != &path || line < *start || line > *end {
                continue;
            }
            if best
                .map(|(best_start, best_end, _)| (end - start) < (best_end - best_start))
                .unwrap_or(true)
            {
                best = Some((start, end, entity_id));
            }
        }
        best.map(|(_, _, entity_id)| entity_id.clone())
    }

    pub fn symbol(&self, symbol: &str) -> Option<String> {
        self.symbols.get(symbol).cloned()
    }
}

/// Provenance and clocks for one native capture.
#[derive(Debug, Clone)]
pub struct TraceMeta {
    pub snapshot_id: String,
    pub config_digest: String,
    pub build_identity: Value,
    pub scenario_id: Option<String>,
    pub tool: String,
    pub version: String,
    pub clock_kind: String,
    pub wall_clock: bool,
    pub lossy: bool,
    pub loss_notes: Vec<String>,
}

impl TraceMeta {
    pub fn new(
        snapshot_id: &str,
        config_digest: &str,
        build_identity: Value,
        tool: &str,
        version: &str,
    ) -> Self {
        Self {
            snapshot_id: snapshot_id.to_string(),
            config_digest: config_digest.to_string(),
            build_identity,
            scenario_id: None,
            tool: tool.to_string(),
            version: version.to_string(),
            clock_kind: "monotonic".to_string(),
            wall_clock: false,
            lossy: false,
            loss_notes: Vec::new(),
        }
    }

    pub fn scenario(mut self, scenario_id: &str) -> Self {
        self.scenario_id = Some(scenario_id.to_string());
        self
    }

    pub fn clock(mut self, kind: &str, wall_clock: bool) -> Self {
        self.clock_kind = kind.to_string();
        self.wall_clock = wall_clock;
        self
    }

    pub fn lossy(mut self, notes: &[&str]) -> Self {
        self.lossy = !notes.is_empty();
        self.loss_notes = notes.iter().map(|note| note.to_string()).collect();
        self
    }
}

pub(crate) struct SpanDraft {
    pub id: String,
    pub parent: Option<String>,
    pub name: String,
    pub kind: String,
    pub start_ns: i128,
    pub end_ns: i128,
    pub process_id: Option<String>,
    pub thread_id: Option<String>,
    pub entity_id: Option<String>,
    pub attributes: Map<String, Value>,
}

impl SpanDraft {
    pub(crate) fn to_json(&self) -> Value {
        let mut attributes = self.attributes.clone();
        if let Some(entity_id) = &self.entity_id {
            attributes.insert("archmeld.entity_id".to_string(), json!(entity_id));
        }
        json!({
            "span_id": self.id,
            "parent_span_id": self.parent,
            "name": self.name,
            "kind": self.kind,
            "start_ns": self.start_ns.to_string(),
            "end_ns": self.end_ns.to_string(),
            "process_id": self.process_id,
            "thread_id": self.thread_id,
            "attributes": attributes,
        })
    }
}

fn normalize_path(path: &str) -> String {
    let mut value = path.trim().replace('\\', "/");
    for prefix in ["file:///", "file://"] {
        if let Some(rest) = value.strip_prefix(prefix) {
            value = rest.to_string();
            break;
        }
    }
    while let Some(rest) = value.strip_prefix("./") {
        value = rest.to_string();
    }
    percent_decode(&value)
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

fn microseconds_to_ns(value: f64) -> i128 {
    (value * 1000.0).round() as i128
}

fn number_as_f64(value: &Value) -> Option<f64> {
    value
        .as_f64()
        .or_else(|| value.as_str().and_then(|text| text.parse::<f64>().ok()))
}

fn string_number(value: &Value) -> Option<i128> {
    value
        .as_i64()
        .map(i128::from)
        .or_else(|| value.as_u64().map(i128::from))
        .or_else(|| value.as_str().and_then(|text| text.parse::<i128>().ok()))
}

fn base_trace(meta: &TraceMeta, spans: Vec<SpanDraft>) -> Result<Value> {
    base_trace_with_metrics(meta, spans, Vec::new(), Vec::new())
}

/// Build the normalized trace with explicit counter series and state
/// intervals. Both lists are observations; nothing here creates a span, a
/// link or a causal edge.
pub(crate) fn base_trace_with_metrics(
    meta: &TraceMeta,
    spans: Vec<SpanDraft>,
    counters: Vec<Value>,
    states: Vec<Value>,
) -> Result<Value> {
    let mut values = Vec::with_capacity(spans.len());
    for span in &spans {
        if span.id.is_empty() {
            return Err(CoreError::invalid("capture produced an empty span id"));
        }
        if span.name.is_empty() {
            return Err(CoreError::invalid(format!(
                "capture produced an empty span name for {}",
                span.id
            )));
        }
        if span.end_ns < span.start_ns {
            return Err(CoreError::invalid(format!(
                "capture produced a negative duration for {}",
                span.id
            )));
        }
        values.push(span.to_json());
    }
    let domain = json!({
        "clock_id": "default",
        "kind": meta.clock_kind,
        "unit": "ns",
        "origin_ns": "0",
        "calibrated": true,
        "uncertainty_ns": "0",
        "wall_clock": meta.wall_clock,
    });
    let mut trace = json!({
        "version": "1.0",
        "snapshot_id": meta.snapshot_id,
        "config_digest": meta.config_digest,
        "build_identity": meta.build_identity,
        "capture": {
            "tool": meta.tool,
            "version": meta.version,
            "lossy": meta.lossy,
            "loss_notes": meta.loss_notes,
        },
        "clock": {
            "kind": meta.clock_kind,
            "unit": "ns",
            "origin_ns": "0",
            "domains": [domain],
        },
        "probes": [],
        "spans": values,
        "links": [],
        "counters": counters,
        "states": states,
    });
    if let Some(scenario_id) = &meta.scenario_id {
        trace["scenario_id"] = json!(scenario_id);
    }
    Ok(trace)
}

/// Chrome Trace Event JSON and VizTracer. `parse_locations` reads VizTracer's
/// `function (path:line)` naming; plain Chrome traces keep the raw name.
pub fn chrome_trace(
    value: &Value,
    locator: &TraceLocator,
    meta: &TraceMeta,
    parse_locations: bool,
) -> Result<Value> {
    let events = value
        .get("traceEvents")
        .and_then(Value::as_array)
        .or_else(|| value.as_array())
        .ok_or_else(|| CoreError::invalid("Chrome trace must contain traceEvents"))?;
    let mut spans = Vec::new();
    let mut counter = 0usize;
    let mut counter_groups: Vec<Value> = Vec::new();
    let mut counter_index: HashMap<(String, String, String, String), usize> = HashMap::new();
    for event in events {
        let phase = event["ph"].as_str().unwrap_or("");
        // Counter tracks ("C") are real measurements exported by Chrome and
        // Perfetto. They become counter series, never spans or causal edges.
        if phase == "C" {
            let Some(start_us) = number_as_f64(&event["ts"]) else {
                continue;
            };
            let Some(value) = counter_event_value(&event["args"]) else {
                continue;
            };
            let name = event["name"].as_str().unwrap_or("counter");
            let category = event["cat"].as_str().unwrap_or("");
            let pid = event["pid"]
                .as_i64()
                .map(|value| value.to_string())
                .unwrap_or_else(|| "0".to_string());
            let tid = event["tid"]
                .as_i64()
                .map(|value| value.to_string())
                .unwrap_or_else(|| "0".to_string());
            let key = (
                name.to_string(),
                category.to_string(),
                pid.clone(),
                tid.clone(),
            );
            let index = match counter_index.get(&key) {
                Some(index) => *index,
                None => {
                    let mut attributes = Map::new();
                    attributes.insert("capture.phase".to_string(), json!("C"));
                    if !category.is_empty() {
                        attributes.insert("capture.category".to_string(), json!(category));
                    }
                    attributes.insert("capture.tool_format".to_string(), json!("chrome-counter"));
                    counter_groups.push(json!({
                        "counter_id": format!("chrome.counter.{}.{}", pid, counter_groups.len() + 1),
                        "name": name,
                        "clock_id": "default",
                        "attributes": attributes,
                        "samples": [],
                    }));
                    let index = counter_groups.len() - 1;
                    counter_index.insert(key, index);
                    index
                }
            };
            if let Some(samples) = counter_groups[index]["samples"].as_array_mut() {
                samples.push(json!({
                    "timestamp_ns": microseconds_to_ns(start_us).to_string(),
                    "value": value,
                    "process_id": pid,
                    "thread_id": tid,
                }));
            }
            continue;
        }
        // Only complete ("X") events are normalized here. Begin/end pairs
        // without an explicit parent id are left out rather than inventing a
        // parent from timestamp containment.
        if phase != "X" {
            continue;
        }
        let Some(start_us) = number_as_f64(&event["ts"]) else {
            continue;
        };
        let duration_us = number_as_f64(&event["dur"]).unwrap_or(0.0);
        let raw_name = event["name"].as_str().unwrap_or("event");
        let (name, location) = if parse_locations {
            parse_viz_name(raw_name)
        } else {
            (raw_name.to_string(), None)
        };
        let entity_id = location
            .as_ref()
            .and_then(|(path, line)| locator.location(path, *line));
        let mut attributes = Map::new();
        attributes.insert("capture.phase".to_string(), json!("X"));
        if let Some(category) = event["cat"].as_str() {
            attributes.insert("capture.category".to_string(), json!(category));
        }
        let pid = event["pid"]
            .as_i64()
            .map(|value| value.to_string())
            .unwrap_or_else(|| "0".to_string());
        let tid = event["tid"]
            .as_i64()
            .map(|value| value.to_string())
            .unwrap_or_else(|| "0".to_string());
        let start = microseconds_to_ns(start_us);
        counter += 1;
        spans.push(SpanDraft {
            id: format!("chrome.{counter}"),
            parent: None,
            name,
            kind: "function".to_string(),
            start_ns: start,
            end_ns: start + microseconds_to_ns(duration_us.max(0.0)),
            process_id: Some(pid),
            thread_id: Some(tid),
            entity_id,
            attributes,
        });
    }
    base_trace_with_metrics(meta, spans, counter_groups, Vec::new())
}

/// Chrome counter `args` may be `{"value": n}` or a single numeric entry.
/// Ambiguous multi-value objects are skipped; the adapter never picks one.
fn counter_event_value(args: &Value) -> Option<Value> {
    let object = args.as_object()?;
    if let Some(value) = object.get("value") {
        return numeric_json(value);
    }
    if object.len() == 1 {
        return object.values().next().and_then(numeric_json);
    }
    None
}

fn numeric_json(value: &Value) -> Option<Value> {
    match value {
        Value::Number(number) if number.as_f64().is_some_and(f64::is_finite) => {
            Some(Value::Number(number.clone()))
        }
        _ => None,
    }
}

fn parse_viz_name(name: &str) -> (String, Option<(String, u64)>) {
    if let Some(index) = name.rfind(" (") {
        if name.ends_with(')') {
            let location = &name[index + 2..name.len() - 1];
            if let Some(colon) = location.rfind(':') {
                if let Ok(line) = location[colon + 1..].parse::<u64>() {
                    return (
                        name[..index].to_string(),
                        Some((location[..colon].to_string(), line)),
                    );
                }
            }
        }
    }
    (name.to_string(), None)
}

/// V8 CPU profile (`node --cpu-prof`). Samples are real V8 stack samples at
/// the profiler interval; the parent chain comes from the profiler's call tree.
pub fn v8_cpuprofile(value: &Value, locator: &TraceLocator, meta: &TraceMeta) -> Result<Value> {
    let nodes = value["nodes"]
        .as_array()
        .ok_or_else(|| CoreError::invalid("V8 profile must contain nodes"))?;
    let mut frames: HashMap<i64, (Value, Option<i64>)> = HashMap::new();
    for node in nodes {
        let Some(id) = node["id"].as_i64() else {
            continue;
        };
        frames.insert(id, (node["callFrame"].clone(), None));
    }
    for node in nodes {
        let Some(id) = node["id"].as_i64() else {
            continue;
        };
        if let Some(children) = node["children"].as_array() {
            for child in children.iter().filter_map(Value::as_i64) {
                if let Some(entry) = frames.get_mut(&child) {
                    entry.1 = Some(id);
                }
            }
        }
    }
    let samples = value["samples"]
        .as_array()
        .ok_or_else(|| CoreError::invalid("V8 profile must contain samples"))?;
    let deltas = value["timeDeltas"]
        .as_array()
        .ok_or_else(|| CoreError::invalid("V8 profile must contain timeDeltas"))?;
    let mut cursor = microseconds_to_ns(
        value["startTime"]
            .as_f64()
            .ok_or_else(|| CoreError::invalid("V8 profile startTime is required"))?,
    );
    let mut spans = Vec::new();
    for (index, sample) in samples.iter().enumerate() {
        let Some(node_id) = sample.as_i64() else {
            continue;
        };
        let delta_us = deltas
            .get(index)
            .and_then(Value::as_f64)
            .unwrap_or(1000.0)
            .max(1.0);
        let start = cursor;
        let end = start + microseconds_to_ns(delta_us);
        cursor = end;
        let mut chain = Vec::new();
        let mut current = Some(node_id);
        let mut guard = 0;
        while let Some(id) = current {
            guard += 1;
            if guard > 512 {
                break;
            }
            let Some((frame, parent)) = frames.get(&id) else {
                break;
            };
            chain.push((id, frame.clone()));
            current = *parent;
        }
        // chain[0] is the leaf; chain[d+1] is its caller.
        for depth in 0..chain.len() {
            let (_, frame) = &chain[depth];
            let name = frame["functionName"]
                .as_str()
                .filter(|name| !name.is_empty())
                .unwrap_or("(anonymous)")
                .to_string();
            let url = frame["url"].as_str().unwrap_or("");
            let line = frame["lineNumber"].as_u64().map(|line| line + 1);
            let entity_id = line.and_then(|line| locator.location(url, line));
            let mut attributes = Map::new();
            attributes.insert("v8.url".to_string(), json!(url));
            attributes.insert(
                "v8.line".to_string(),
                json!(line.map(|line| line.to_string()).unwrap_or_default()),
            );
            attributes.insert("sample.delta_us".to_string(), json!(delta_us.to_string()));
            spans.push(SpanDraft {
                id: format!("v8.{index}.{depth}"),
                parent: if depth + 1 < chain.len() {
                    Some(format!("v8.{index}.{}", depth + 1))
                } else {
                    None
                },
                name,
                kind: "function_sample".to_string(),
                start_ns: start,
                end_ns: end,
                process_id: Some("v8".to_string()),
                thread_id: Some("main".to_string()),
                entity_id,
                attributes,
            });
        }
    }
    base_trace(meta, spans)
}

/// JFR `jfr print --json --events jdk.ExecutionSample`. Frames are ordered
/// leaf first and form an explicit sampled call stack.
pub fn jfr_execution_samples(
    value: &Value,
    locator: &TraceLocator,
    meta: &TraceMeta,
) -> Result<Value> {
    let events = value
        .get("recording")
        .and_then(|recording| recording["events"].as_array())
        .or_else(|| value["events"].as_array())
        .ok_or_else(|| CoreError::invalid("JFR print output must contain events"))?;
    let mut samples = Vec::new();
    for event in events {
        let values = &event["values"];
        let Some(start_text) = values["startTime"].as_str() else {
            continue;
        };
        let Some(start_ns) = rfc3339_ns(start_text) else {
            continue;
        };
        let thread = values["sampledThread"]["javaName"]
            .as_str()
            .or_else(|| values["sampledThread"]["osName"].as_str())
            .unwrap_or("unknown")
            .to_string();
        let frames = values["stackTrace"]["frames"]
            .as_array()
            .cloned()
            .unwrap_or_default();
        samples.push((start_ns, thread, frames));
    }
    samples.sort_by_key(|(start, _, _)| *start);
    let mut spans = Vec::new();
    for index in 0..samples.len() {
        let (start, thread, frames) = &samples[index];
        let next_start = samples
            .get(index + 1)
            .map(|(start, _, _)| *start)
            .unwrap_or(*start + 20_000_000);
        let end = if next_start > *start {
            next_start
        } else {
            *start + 1
        };
        for depth in 0..frames.len() {
            let frame = &frames[depth];
            let method = &frame["method"];
            let type_name = method["type"]["name"].as_str().unwrap_or("");
            let method_name = method["name"].as_str().unwrap_or("");
            if type_name.is_empty() || method_name.is_empty() {
                continue;
            }
            let symbol = format!("jfr:{type_name}#{method_name}");
            let name = format!("{type_name}#{method_name}");
            let mut attributes = Map::new();
            attributes.insert("jfr.type".to_string(), json!(type_name));
            attributes.insert("jfr.method".to_string(), json!(method_name));
            if let Some(line) = frame["lineNumber"].as_u64() {
                attributes.insert("jfr.line".to_string(), json!(line.to_string()));
            }
            if let Some(frame_type) = frame["type"].as_str() {
                attributes.insert("jfr.frame_type".to_string(), json!(frame_type));
            }
            spans.push(SpanDraft {
                id: format!("jfr.{index}.{depth}"),
                parent: if depth + 1 < frames.len() {
                    Some(format!("jfr.{index}.{}", depth + 1))
                } else {
                    None
                },
                name,
                kind: "function_sample".to_string(),
                start_ns: *start,
                end_ns: end,
                process_id: Some("jvm".to_string()),
                thread_id: Some(thread.clone()),
                entity_id: locator.symbol(&symbol),
                attributes,
            });
        }
    }
    base_trace(meta, spans)
}

/// Parse RFC3339 with a numeric offset into epoch nanoseconds.
pub fn rfc3339_ns(text: &str) -> Option<i128> {
    let (date, rest) = text.split_once('T')?;
    let mut pieces = date.split('-');
    let year: i64 = pieces.next()?.parse().ok()?;
    let month: i64 = pieces.next()?.parse().ok()?;
    let day: i64 = pieces.next()?.parse().ok()?;
    let (time, offset_seconds) = if let Some(rest) = rest.strip_suffix('Z') {
        (rest, 0i64)
    } else if let Some(index) = rest.rfind(['+', '-']) {
        let sign = if &rest[index..index + 1] == "+" { 1 } else { -1 };
        let offset = &rest[index + 1..];
        let (hours, minutes) = offset.split_once(':')?;
        (
            &rest[..index],
            sign * (hours.parse::<i64>().ok()? * 3600 + minutes.parse::<i64>().ok()? * 60),
        )
    } else {
        (rest, 0i64)
    };
    let mut time_pieces = time.split(':');
    let hour: i64 = time_pieces.next()?.parse().ok()?;
    let minute: i64 = time_pieces.next()?.parse().ok()?;
    let second_text = time_pieces.next()?;
    let (second_text, fraction) = match second_text.split_once('.') {
        Some((second, fraction)) => (second, fraction),
        None => (second_text, ""),
    };
    let second: i64 = second_text.parse().ok()?;
    let mut fraction_ns: i128 = 0;
    if !fraction.is_empty() {
        let digits: String = fraction.chars().take(9).collect();
        if digits.len() < 9 {
            return None;
        }
        fraction_ns = digits.parse().ok()?;
    }
    let days = days_from_civil(year, month, day);
    let seconds = days * 86_400 + hour * 3600 + minute * 60 + second - offset_seconds;
    Some(i128::from(seconds) * 1_000_000_000 + fraction_ns)
}

fn days_from_civil(year: i64, month: i64, day: i64) -> i64 {
    let year = if month <= 2 { year - 1 } else { year };
    let era = if year >= 0 { year } else { year - 399 } / 400;
    let yoe = year - era * 400;
    let month = month + if month > 2 { -3 } else { 9 };
    let doy = (153 * month + 2) / 5 + day - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146_097 + doe - 719_468
}

/// `go tool pprof -raw` text. pprof aggregates samples, so there is no
/// wall-clock timestamp per sample; durations are the measured CPU values and
/// are placed on an explicit `cpu` clock. Parent links come from sample stacks.
pub fn pprof_raw(text: &str, locator: &TraceLocator, meta: &TraceMeta) -> Result<Value> {
    let normalized = text.replace("\r\n", "\n");
    let mut locations: HashMap<u64, (String, u64, String)> = HashMap::new();
    let mut sample_lines = Vec::new();
    let mut section = "";
    for line in normalized.lines() {
        let trimmed = line.trim();
        if trimmed == "Samples:" {
            section = "samples";
            continue;
        }
        if trimmed == "Locations" {
            section = "locations";
            continue;
        }
        if trimmed == "Mappings" {
            section = "mappings";
            continue;
        }
        match section {
            "samples" => {
                if trimmed.is_empty()
                    || trimmed.starts_with("samples/")
                    || trimmed.starts_with("cpu/")
                    || trimmed.contains("nanoseconds")
                {
                    continue;
                }
                let Some((value_part, rest)) = trimmed.split_once(':') else {
                    continue;
                };
                let mut values = value_part.split_whitespace();
                let Some(_count) = values.next() else {
                    continue;
                };
                let Some(ns) = values.next().and_then(|text| text.parse::<i128>().ok()) else {
                    continue;
                };
                let ids: Vec<u64> = rest
                    .split_whitespace()
                    .filter_map(|text| text.parse::<u64>().ok())
                    .collect();
                if !ids.is_empty() {
                    sample_lines.push((ns, ids));
                }
            }
            "locations" => {
                let Some((id_text, rest)) = trimmed.split_once(':') else {
                    continue;
                };
                let Ok(id) = id_text.trim().parse::<u64>() else {
                    continue;
                };
                let Some(before_site) = rest.rsplit_once(" s=") else {
                    continue;
                };
                let mut position = before_site.0;
                // Trim the mapping marker (`M=1`) and address from the left,
                // then the symbol name so only `file:line:column` remains.
                if let Some(index) = position.rfind(" M=") {
                    position = &position[index + 3..];
                    if let Some(space) = position.rfind(' ') {
                        position = &position[space + 1..];
                    }
                }
                let Some((file_part, column)) = position.rsplit_once(':') else {
                    continue;
                };
                let Some((file, line_text)) = file_part.rsplit_once(':') else {
                    continue;
                };
                let Ok(line) = line_text.parse::<u64>() else {
                    continue;
                };
                if column.parse::<u64>().is_err() {
                    continue;
                }
                let name = position
                    .rsplit_once(' ')
                    .map(|(name, _)| name.to_string())
                    .unwrap_or_else(|| position.to_string());
                locations.insert(id, (file.to_string(), line, name));
            }
            _ => {}
        }
    }
    let mut spans = Vec::new();
    let mut cursor: i128 = 0;
    for (index, (ns, ids)) in sample_lines.iter().enumerate() {
        let duration = (*ns).max(0);
        let start = cursor;
        let end = start + duration;
        cursor = end;
        let mut chain: Vec<(u64, String, u64, String)> = Vec::new();
        for id in ids {
            if let Some((file, line, name)) = locations.get(id) {
                chain.push((*id, file.clone(), *line, name.clone()));
            }
        }
        for depth in 0..chain.len() {
            let (_, file, line, name) = &chain[depth];
            spans.push(SpanDraft {
                id: format!("pprof.{index}.{depth}"),
                parent: if depth + 1 < chain.len() {
                    Some(format!("pprof.{index}.{}", depth + 1))
                } else {
                    None
                },
                name: name.clone(),
                kind: "function_sample".to_string(),
                start_ns: start,
                end_ns: end,
                process_id: Some("go".to_string()),
                thread_id: None,
                entity_id: locator.location(file, *line),
                attributes: {
                    let mut attributes = Map::new();
                    attributes.insert("pprof.file".to_string(), json!(file));
                    attributes.insert("pprof.line".to_string(), json!(line.to_string()));
                    attributes
                        .insert("pprof.cpu_ns".to_string(), json!(duration.to_string()));
                    attributes
                },
            });
        }
    }
    base_trace(meta, spans)
}

/// EventPipe `dotnet-trace convert --format speedscope` output. Evented
/// profiles carry explicit open/close records; sampled profiles use
/// `weights` as measured sample intervals.
pub fn speedscope(value: &Value, locator: &TraceLocator, meta: &TraceMeta) -> Result<Value> {
    let frames = value["shared"]["frames"]
        .as_array()
        .ok_or_else(|| CoreError::invalid("speedscope must contain shared.frames"))?;
    let profiles = value["profiles"]
        .as_array()
        .ok_or_else(|| CoreError::invalid("speedscope must contain profiles"))?;
    let mut spans = Vec::new();
    for (profile_index, profile) in profiles.iter().enumerate() {
        let unit = profile["unit"].as_str().unwrap_or("milliseconds");
        let scale = match unit {
            "seconds" => 1_000_000_000.0,
            "microseconds" => 1_000.0,
            "nanoseconds" => 1.0,
            _ => 1_000_000.0,
        };
        if profile["type"].as_str() == Some("evented") {
            let events = profile["events"]
                .as_array()
                .ok_or_else(|| CoreError::invalid("evented speedscope needs events"))?;
            let mut stack: Vec<(String, String, i128)> = Vec::new();
            let mut counter = 0usize;
            for event in events {
                let kind = event["type"].as_str().unwrap_or("");
                let frame_index = event["frame"].as_u64().unwrap_or(u64::MAX) as usize;
                let Some(frame) = frames.get(frame_index) else {
                    continue;
                };
                let Some(at) = event["at"].as_f64() else {
                    continue;
                };
                let at_ns = (at * scale).round() as i128;
                let name = frame["name"].as_str().unwrap_or("frame").to_string();
                match kind {
                    "O" => {
                        counter += 1;
                        let id = format!("ss.{profile_index}.{counter}");
                        let parent = stack.last().map(|(id, _, _)| id.clone());
                        let entity_id = locator.symbol(&name);
                        stack.push((id.clone(), name.clone(), at_ns));
                        spans.push(SpanDraft {
                            id,
                            parent,
                            name,
                            kind: "function".to_string(),
                            start_ns: at_ns,
                            end_ns: at_ns,
                            process_id: Some(format!("speedscope.{profile_index}")),
                            thread_id: profile["name"].as_str().map(str::to_string),
                            entity_id,
                            attributes: {
                                let mut attributes = Map::new();
                                attributes
                                    .insert("speedscope.event".to_string(), json!("open"));
                                attributes
                            },
                        });
                    }
                    "C" => {
                        if let Some((id, _, _)) = stack.pop() {
                            if let Some(span) =
                                spans.iter_mut().find(|span| span.id == id)
                            {
                                span.end_ns = at_ns.max(span.start_ns);
                            }
                        }
                    }
                    _ => {}
                }
            }
        } else if profile["type"].as_str() == Some("sampled") {
            let stacks = profile["samples"]
                .as_array()
                .ok_or_else(|| CoreError::invalid("sampled speedscope needs samples"))?;
            let weights = profile["weights"].as_array();
            let mut cursor: i128 = 0;
            for (index, stack_value) in stacks.iter().enumerate() {
                let Some(stack) = stack_value.as_array() else {
                    continue;
                };
                let weight = weights
                    .and_then(|weights| weights.get(index))
                    .and_then(Value::as_f64)
                    .unwrap_or(1.0)
                    .max(0.0);
                let start = cursor;
                let end = start + (weight * scale).round() as i128;
                cursor = end;
                for depth in 0..stack.len() {
                    let Some(frame_index) = stack[depth].as_u64() else {
                        continue;
                    };
                    let Some(frame) = frames.get(frame_index as usize) else {
                        continue;
                    };
                    let name = frame["name"].as_str().unwrap_or("frame").to_string();
                    let entity_id = locator.symbol(&name);
                    spans.push(SpanDraft {
                        id: format!("ss.{profile_index}.{index}.{depth}"),
                        parent: if depth + 1 < stack.len() {
                            Some(format!("ss.{profile_index}.{index}.{}", depth + 1))
                        } else {
                            None
                        },
                        name,
                        kind: "function_sample".to_string(),
                        start_ns: start,
                        end_ns: end,
                        process_id: Some(format!("speedscope.{profile_index}")),
                        thread_id: profile["name"].as_str().map(str::to_string),
                        entity_id,
                        attributes: Map::new(),
                    });
                }
            }
        }
    }
    base_trace(meta, spans)
}

/// OTLP trace JSON (file export). Timestamps are epoch nanoseconds.
pub fn otlp_json(value: &Value, _locator: &TraceLocator, meta: &TraceMeta) -> Result<Value> {
    let resource_spans = value["resourceSpans"]
        .as_array()
        .ok_or_else(|| CoreError::invalid("OTLP JSON must contain resourceSpans"))?;
    let mut spans = Vec::new();
    let mut links = Vec::new();
    let mut seen = std::collections::BTreeSet::new();
    for resource in resource_spans {
        let service = resource["resource"]["attributes"]
            .as_array()
            .and_then(|attributes| {
                attributes.iter().find_map(|attribute| {
                    (attribute["key"].as_str() == Some("service.name"))
                        .then(|| attribute["value"]["stringValue"].as_str())
                        .flatten()
                })
            })
            .unwrap_or("service");
        for scope in resource["scopeSpans"]
            .as_array()
            .map(Vec::as_slice)
            .unwrap_or(&[])
        {
            for span in scope["spans"].as_array().map(Vec::as_slice).unwrap_or(&[]) {
                let Some(raw_id) = span["spanId"].as_str().filter(|id| !id.is_empty()) else {
                    continue;
                };
                let id = format!("otlp.{raw_id}");
                if !seen.insert(id.clone()) {
                    return Err(CoreError::invalid(format!(
                        "duplicate OTLP span id {raw_id}"
                    )));
                }
                let Some(start) = span["startTimeUnixNano"]
                    .as_str()
                    .and_then(|text| text.parse::<i128>().ok())
                    .or_else(|| string_number(&span["startTimeUnixNano"]))
                else {
                    continue;
                };
                let end = span["endTimeUnixNano"]
                    .as_str()
                    .and_then(|text| text.parse::<i128>().ok())
                    .or_else(|| string_number(&span["endTimeUnixNano"]))
                    .unwrap_or(start);
                let parent = span["parentSpanId"]
                    .as_str()
                    .filter(|parent| !parent.is_empty())
                    .map(|parent| format!("otlp.{parent}"));
                let mut attributes = Map::new();
                let mut entity_id = None;
                for attribute in span["attributes"].as_array().map(Vec::as_slice).unwrap_or(&[]) {
                    let Some(key) = attribute["key"].as_str() else {
                        continue;
                    };
                    let value = attribute["value"]
                        .get("stringValue")
                        .and_then(Value::as_str)
                        .map(|text| json!(text))
                        .or_else(|| {
                            attribute["value"]
                                .get("intValue")
                                .and_then(|value| {
                                    value
                                        .as_str()
                                        .map(|text| json!(text))
                                        .or_else(|| value.as_i64().map(|value| json!(value.to_string())))
                                })
                        })
                        .or_else(|| {
                            attribute["value"]
                                .get("boolValue")
                                .and_then(Value::as_bool)
                                .map(|value| json!(value))
                        })
                        .unwrap_or(Value::Null);
                    if key == "archmeld.entity_id" {
                        entity_id = value.as_str().map(str::to_string);
                    }
                    attributes.insert(key.to_string(), value);
                }
                let name = span["name"].as_str().unwrap_or("span").to_string();
                spans.push(SpanDraft {
                    id: id.clone(),
                    parent,
                    name,
                    kind: "otlp_span".to_string(),
                    start_ns: start,
                    end_ns: end.max(start),
                    process_id: Some(service.to_string()),
                    thread_id: None,
                    entity_id,
                    attributes,
                });
                for (index, link) in span["links"]
                    .as_array()
                    .map(Vec::as_slice)
                    .unwrap_or(&[])
                    .iter()
                    .enumerate()
                {
                    let Some(target) = link["spanId"].as_str().filter(|id| !id.is_empty()) else {
                        continue;
                    };
                    let mut correlation_id = None;
                    for attribute in
                        link["attributes"].as_array().map(Vec::as_slice).unwrap_or(&[])
                    {
                        if attribute["key"].as_str() == Some("correlation_id") {
                            correlation_id = attribute["value"]["stringValue"]
                                .as_str()
                                .map(str::to_string);
                        }
                    }
                    links.push(json!({
                        "link_id": format!("otlp.link.{raw_id}.{index}"),
                        "source_span_id": id,
                        "target_span_id": format!("otlp.{target}"),
                        "kind": "link",
                        "correlation_id": correlation_id,
                        "attributes": {"capture.tool": "otlp"},
                    }));
                }
            }
        }
    }
    let mut trace = base_trace(meta, spans)?;
    trace["links"] = json!(links);
    Ok(trace)
}

/// Explicit probe JSONL (C/C++ and Rust probes). Each line is one measured
/// span, counter sample or state transition. Counter/state lines are explicit
/// in-process observations; they are grouped by their declared id and never
/// become spans or causal edges.
pub fn probe_jsonl(text: &str, locator: &TraceLocator, meta: &TraceMeta) -> Result<Value> {
    let mut spans = Vec::new();
    let mut links = Vec::new();
    let mut counters: BTreeMap<String, CounterDraft> = BTreeMap::new();
    let mut states: BTreeMap<String, StateDraft> = BTreeMap::new();
    for (index, line) in text.replace("\r\n", "\n").lines().enumerate() {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        let value: Value = serde_json::from_str(line)
            .map_err(|err| CoreError::invalid(format!("probe line {}: {err}", index + 1)))?;
        match value["event"].as_str() {
            Some("counter") => {
                parse_probe_counter(&value, index + 1, &mut counters)?;
                continue;
            }
            Some("state") => {
                parse_probe_state(&value, index + 1, &mut states)?;
                continue;
            }
            Some(other) if other != "span" => {
                return Err(CoreError::invalid(format!(
                    "probe line {} has unknown event {other}",
                    index + 1
                )));
            }
            _ => {}
        }
        let Some(id) = value["span_id"].as_str().filter(|id| !id.is_empty()) else {
            return Err(CoreError::invalid(format!(
                "probe line {} has no span_id",
                index + 1
            )));
        };
        let name = value["name"].as_str().unwrap_or("span");
        let Some(start) = string_number(&value["start_ns"]) else {
            return Err(CoreError::invalid(format!(
                "probe span {id} has no start_ns"
            )));
        };
        let end = string_number(&value["end_ns"]).unwrap_or(start);
        let mut attributes = value["attributes"]
            .as_object()
            .cloned()
            .unwrap_or_default();
        let entity_id = value["entity_id"]
            .as_str()
            .filter(|entity| !entity.is_empty())
            .map(str::to_string)
            .or_else(|| locator.symbol(name));
        if let Some(entity_id) = &entity_id {
            attributes.insert("archmeld.entity_id".to_string(), json!(entity_id));
        }
        spans.push(SpanDraft {
            id: id.to_string(),
            parent: value["parent_span_id"]
                .as_str()
                .filter(|parent| !parent.is_empty())
                .map(str::to_string),
            name: name.to_string(),
            kind: value["kind"].as_str().unwrap_or("function").to_string(),
            start_ns: start,
            end_ns: end.max(start),
            process_id: value["process_id"].as_str().map(str::to_string),
            thread_id: value["thread_id"].as_str().map(str::to_string),
            entity_id,
            attributes,
        });
        if let Some(span_links) = value["links"].as_array() {
            for (link_index, link) in span_links.iter().enumerate() {
                let Some(target) = link["target_span_id"].as_str() else {
                    continue;
                };
                links.push(json!({
                    "link_id": format!("{id}.links.{link_index}"),
                    "source_span_id": id,
                    "target_span_id": target,
                    "kind": link["kind"].as_str().unwrap_or("link"),
                    "correlation_id": link["correlation_id"],
                    "attributes": link["attributes"].clone(),
                }));
            }
        }
    }
    let mut trace = base_trace_with_metrics(
        meta,
        spans,
        counters.into_values().map(CounterDraft::to_json).collect(),
        states.into_values().map(StateDraft::to_json).collect(),
    )?;
    trace["links"] = json!(links);
    Ok(trace)
}

#[derive(Default)]
struct CounterDraft {
    id: String,
    name: String,
    unit: Option<String>,
    attributes: Map<String, Value>,
    samples: Vec<Value>,
}

impl CounterDraft {
    fn to_json(self) -> Value {
        json!({
            "counter_id": self.id,
            "name": self.name,
            "unit": self.unit,
            "clock_id": "default",
            "attributes": self.attributes,
            "samples": self.samples,
        })
    }
}

#[derive(Default)]
struct StateDraft {
    id: String,
    name: String,
    attributes: Map<String, Value>,
    intervals: Vec<Value>,
}

impl StateDraft {
    fn to_json(self) -> Value {
        json!({
            "state_id": self.id,
            "name": self.name,
            "clock_id": "default",
            "attributes": self.attributes,
            "intervals": self.intervals,
        })
    }
}

fn parse_probe_counter(
    value: &Value,
    line: usize,
    counters: &mut BTreeMap<String, CounterDraft>,
) -> Result<()> {
    let id = value["counter_id"]
        .as_str()
        .filter(|id| !id.is_empty())
        .ok_or_else(|| {
            CoreError::invalid(format!("probe counter line {line} has no counter_id"))
        })?;
    let timestamp = string_number(&value["timestamp_ns"]).ok_or_else(|| {
        CoreError::invalid(format!(
            "probe counter {id} line {line} has no integer timestamp_ns"
        ))
    })?;
    let sample_value = match &value["value"] {
        Value::Number(number) if number.as_f64().is_some_and(f64::is_finite) => {
            Value::Number(number.clone())
        }
        _ => {
            return Err(CoreError::invalid(format!(
                "probe counter {id} line {line} value must be a finite number"
            )))
        }
    };
    let draft = counters.entry(id.to_string()).or_insert_with(|| {
        let mut attributes = value["attributes"]
            .as_object()
            .cloned()
            .unwrap_or_default();
        copy_probe_mapping(value, &mut attributes);
        CounterDraft {
            id: id.to_string(),
            name: value["name"].as_str().unwrap_or(id).to_string(),
            unit: value["unit"]
                .as_str()
                .filter(|unit| !unit.is_empty())
                .map(str::to_string),
            attributes,
            samples: Vec::new(),
        }
    });
    draft.samples.push(json!({
        "timestamp_ns": timestamp.to_string(),
        "value": sample_value,
        "process_id": value["process_id"].as_str(),
        "thread_id": value["thread_id"].as_str(),
    }));
    Ok(())
}

fn parse_probe_state(
    value: &Value,
    line: usize,
    states: &mut BTreeMap<String, StateDraft>,
) -> Result<()> {
    let id = value["state_id"]
        .as_str()
        .filter(|id| !id.is_empty())
        .ok_or_else(|| CoreError::invalid(format!("probe state line {line} has no state_id")))?;
    let start = string_number(&value["start_ns"]).ok_or_else(|| {
        CoreError::invalid(format!(
            "probe state {id} line {line} has no integer start_ns"
        ))
    })?;
    let end = string_number(&value["end_ns"]).ok_or_else(|| {
        CoreError::invalid(format!(
            "probe state {id} line {line} has no integer end_ns"
        ))
    })?;
    if end < start {
        return Err(CoreError::invalid(format!(
            "probe state {id} line {line} ends before it starts"
        )));
    }
    let state_value = match &value["value"] {
        Value::String(text) if !text.is_empty() => Value::String(text.clone()),
        Value::Number(number) if number.as_f64().is_some_and(f64::is_finite) => {
            Value::Number(number.clone())
        }
        _ => {
            return Err(CoreError::invalid(format!(
                "probe state {id} line {line} value must be a non-empty string or finite number"
            )))
        }
    };
    let draft = states.entry(id.to_string()).or_insert_with(|| {
        let mut attributes = value["attributes"]
            .as_object()
            .cloned()
            .unwrap_or_default();
        copy_probe_mapping(value, &mut attributes);
        StateDraft {
            id: id.to_string(),
            name: value["name"].as_str().unwrap_or(id).to_string(),
            attributes,
            intervals: Vec::new(),
        }
    });
    draft.intervals.push(json!({
        "start_ns": start.to_string(),
        "end_ns": end.to_string(),
        "value": state_value,
        "process_id": value["process_id"].as_str(),
        "thread_id": value["thread_id"].as_str(),
    }));
    Ok(())
}

/// Copy only explicit mapping fields. A metric name never selects an entity.
fn copy_probe_mapping(value: &Value, attributes: &mut Map<String, Value>) {
    for key in ["entity_id", "probe_id"] {
        if let Some(text) = value[key].as_str().filter(|text| !text.is_empty()) {
            attributes.insert(format!("archmeld.{key}"), json!(text));
        }
    }
    if let Some(probe) = attributes.get("probe_id").and_then(Value::as_str) {
        let probe = probe.to_string();
        attributes.insert("archmeld.probe_id".to_string(), json!(probe));
    }
}