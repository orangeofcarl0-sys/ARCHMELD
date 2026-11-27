// Copyright 2026 ARCHMELD contributors
// SPDX-License-Identifier: Apache-2.0

//! Normalized runtime traces (S06) with nanosecond decimal strings, explicit
//! clock domains and loss metadata.
//!
//! A trace is only `bound` when its declared build config matches the target
//! snapshot. Causal edges are created exclusively from an explicit
//! `parent_span_id`, flow/message/task identifier or link; two spans that merely
//! overlap in time are never connected. A span is attributed to a source
//! entity only through an explicit `archmeld.entity_id` or an explicit probe
//! id; function-name matching never creates an observed edge.
//!
//! Optional `counters` and `states` carry real runtime measurements (probe
//! events, Chrome/Perfetto counter tracks, native trace packets). They are
//! observations only: no metric creates a causal edge, an observed call or an
//! architecture fact. A metric is attributed to a source entity only through
//! an explicit entity id or probe; absent declarations are recorded as
//! process-scope metrics rather than guessed.

use crate::error::{CoreError, Result};
use crate::hash;
use crate::store::{validate, Coverage, CoverageState, Store};
use serde_json::{json, Map, Value};
use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::time::{SystemTime, UNIX_EPOCH};

const MAX_SPANS: usize = 100_000;
const MAX_LINKS: usize = 100_000;
const MAX_TIMELINE: usize = 10_000;
const MAX_CAUSAL_SAMPLES: usize = 100;
const MAX_EDGE_SAMPLES: usize = 8;
const MAX_METRICS: usize = 10_000;
// T3 stress traces carry one million events. Samples are far cheaper than
// spans, so the sample budget is larger while spans stay bounded at 100k.
const MAX_METRIC_SAMPLES: usize = 1_000_000;
const MAX_METRIC_SAMPLES_TIMELINE: usize = 10_000;
const MAX_STATE_VALUE: usize = 256;

pub struct RuntimeImport<'a> {
    pub project_id: &'a str,
    pub run_id: &'a str,
    pub snapshot_id: Option<&'a str>,
    pub config_digest: &'a str,
    pub allow_unbound: bool,
    /// Explicit scenario run association. A trace may also carry
    /// `scenario_id`; both must agree when present, and the run must exist
    /// with the same snapshot and build config.
    pub scenario_run_id: Option<&'a str>,
}

#[derive(Debug, Clone)]
struct ScenarioAssociation {
    run_id: String,
    snapshot_id: String,
    config_digest: String,
    exit_code: Option<i64>,
    timed_out: bool,
}

#[derive(Debug, Clone, Default)]
struct ScenarioResolution {
    /// Id carried in the evidence: an explicitly verified run id or a
    /// producer-declared annotation.
    declared_id: Option<String>,
    /// Present only when an explicit scenario run matched snapshot and build.
    verified: Option<ScenarioAssociation>,
}

#[derive(Debug, Clone)]
struct ClockDomain {
    clock_id: String,
    kind: String,
    unit: String,
    origin_ns: String,
    calibrated: bool,
    uncertainty_ns: Option<String>,
    wall_clock: bool,
}

impl ClockDomain {
    fn to_json(&self) -> Value {
        json!({
            "clock_id": self.clock_id,
            "kind": self.kind,
            "unit": self.unit,
            "origin_ns": self.origin_ns,
            "calibrated": self.calibrated,
            "uncertainty_ns": self.uncertainty_ns,
            "wall_clock": self.wall_clock,
        })
    }
}

struct RawSpan {
    span_id: String,
    parent_id: Option<String>,
    name: String,
    kind: String,
    clock_id: String,
    start_ns: String,
    end_ns: String,
    process_id: Option<String>,
    thread_id: Option<String>,
    attributes: Value,
    span_links: Vec<RawLink>,
}

#[derive(Clone)]
struct RawLink {
    link_id: String,
    source_span_id: String,
    target_span_id: String,
    kind: String,
    correlation_id: Option<String>,
    attributes: Value,
}

struct CausalGroup {
    source_id: String,
    target_id: String,
    kind: &'static str,
    edges: BTreeSet<String>,
    correlations: BTreeSet<String>,
    cross_clock: bool,
    observations: u64,
}

struct MappingReport {
    mapped: HashMap<String, String>,
    missing_samples: Vec<Value>,
    mapped_spans: usize,
    unmapped_spans: usize,
    unknown_entity: usize,
    unknown_probe: usize,
    conflicts: usize,
}

/// A counter or state metric declared by an explicit runtime source. Metrics
/// are observations, never causality.
struct RawMetric {
    metric_id: String,
    kind: &'static str,
    name: String,
    unit: Option<String>,
    clock_id: String,
    declared_entity: Option<String>,
    attributes: Value,
    samples: Vec<RawMetricSample>,
}

struct RawMetricSample {
    start_ns: String,
    end_ns: Option<String>,
    value: Value,
    process_id: Option<String>,
    thread_id: Option<String>,
}

#[derive(Default)]
struct MetricReport {
    counters: usize,
    states: usize,
    samples: usize,
    mapped: usize,
    process_scope: usize,
    unknown_entity: usize,
    unknown_probe: usize,
    conflicts: usize,
    missing_samples: Vec<Value>,
    resolved: HashMap<String, String>,
}

impl MetricReport {
    fn metric_count(&self) -> usize {
        self.counters + self.states
    }

    fn unresolved(&self) -> usize {
        self.unknown_entity + self.unknown_probe + self.conflicts
    }
}

mod import;
mod timeline;
mod causal;
mod metrics;

pub use import::import_trace;
pub use timeline::{runtime_summary, timeline};

fn parse_probes(trace: &Value) -> Result<HashMap<String, String>> {
    let mut probes = HashMap::new();
    let Some(values) = trace.get("probes") else {
        return Ok(probes);
    };
    let values = values
        .as_array()
        .ok_or_else(|| CoreError::invalid("probes must be an array"))?;
    for value in values {
        let probe_id = non_empty(&value["probe_id"], "probes[].probe_id")?;
        validate::require_id("probes[].probe_id", &probe_id)?;
        let entity_id = non_empty(&value["entity_id"], "probes[].entity_id")?;
        if let Some(previous) = probes.insert(probe_id.clone(), entity_id.clone()) {
            if previous != entity_id {
                return Err(CoreError::conflict(format!(
                    "probe {probe_id} maps to both {previous} and {entity_id}"
                )));
            }
        }
    }
    Ok(probes)
}

fn parse_clock_domains(trace: &Value) -> Result<(Vec<ClockDomain>, String)> {
    let clock = trace["clock"]
        .as_object()
        .ok_or_else(|| CoreError::invalid("clock must be an object"))?;
    let kind = non_empty(&trace["clock"]["kind"], "clock.kind")?;
    let unit = non_empty(&trace["clock"]["unit"], "clock.unit")?;
    if unit != "ns" {
        return Err(CoreError::unsupported("only nanosecond clocks are supported"));
    }
    let origin_ns = non_empty(&trace["clock"]["origin_ns"], "clock.origin_ns")?;
    require_decimal("clock.origin_ns", &origin_ns)?;
    let Some(values) = clock.get("domains") else {
        let domain = ClockDomain {
            clock_id: "default".to_string(),
            kind,
            unit,
            origin_ns,
            calibrated: true,
            uncertainty_ns: Some("0".to_string()),
            wall_clock: false,
        };
        return Ok((vec![domain], "default".to_string()));
    };
    let values = values
        .as_array()
        .filter(|values| !values.is_empty())
        .ok_or_else(|| CoreError::invalid("clock.domains must be a non-empty array"))?;
    if values.len() > 64 {
        return Err(CoreError::invalid("resource_exhausted: more than 64 clock domains"));
    }
    let mut domains = Vec::with_capacity(values.len());
    let mut seen = BTreeSet::new();
    for value in values {
        let clock_id = non_empty(&value["clock_id"], "clock.domains[].clock_id")?;
        validate::require_id("clock.domains[].clock_id", &clock_id)?;
        if !seen.insert(clock_id.clone()) {
            return Err(CoreError::invalid(format!(
                "duplicate clock domain {clock_id}"
            )));
        }
        let domain_kind = non_empty(&value["kind"], "clock.domains[].kind")?;
        let domain_unit = non_empty(&value["unit"], "clock.domains[].unit")?;
        if domain_unit != "ns" {
            return Err(CoreError::unsupported("only nanosecond clocks are supported"));
        }
        let domain_origin = value["origin_ns"]
            .as_str()
            .ok_or_else(|| CoreError::invalid("clock.domains[].origin_ns must be a decimal string"))?
            .to_string();
        require_decimal("clock.domains[].origin_ns", &domain_origin)?;
        let calibrated = value["calibrated"]
            .as_bool()
            .ok_or_else(|| CoreError::invalid("clock.domains[].calibrated must be boolean"))?;
        let uncertainty_ns = match value.get("uncertainty_ns") {
            None | Some(Value::Null) => None,
            Some(Value::String(text)) => {
                require_decimal("clock.domains[].uncertainty_ns", text)?;
                Some(text.clone())
            }
            Some(_) => {
                return Err(CoreError::invalid(
                    "clock.domains[].uncertainty_ns must be a decimal string",
                ))
            }
        };
        let wall_clock = value["wall_clock"].as_bool().unwrap_or(false);
        domains.push(ClockDomain {
            clock_id,
            kind: domain_kind,
            unit: domain_unit,
            origin_ns: domain_origin,
            calibrated,
            uncertainty_ns,
            wall_clock,
        });
    }
    let default_clock_id = if seen.contains("default") {
        "default".to_string()
    } else {
        domains[0].clock_id.clone()
    };
    Ok((domains, default_clock_id))
}

fn parse_span(raw: &Value, default_clock_id: &str, domains: &[ClockDomain]) -> Result<RawSpan> {
    let span_id = non_empty(&raw["span_id"], "span_id")?;
    let parent = raw["parent_span_id"]
        .as_str()
        .filter(|value| !value.is_empty())
        .map(str::to_string);
    let name = non_empty(&raw["name"], "name")?;
    let kind = non_empty(&raw["kind"], "kind")?;
    let clock_id = raw["clock_id"]
        .as_str()
        .filter(|value| !value.is_empty())
        .unwrap_or(default_clock_id)
        .to_string();
    if !domains.iter().any(|domain| domain.clock_id == clock_id) {
        return Err(CoreError::invalid(format!(
            "span {span_id} references unknown clock domain {clock_id}"
        )));
    }
    let start = raw["start_ns"]
        .as_str()
        .ok_or_else(|| CoreError::invalid("start_ns must be a decimal string, not a number"))?
        .to_string();
    let end = raw["end_ns"]
        .as_str()
        .ok_or_else(|| CoreError::invalid("end_ns must be a decimal string, not a number"))?
        .to_string();
    let start_value = require_decimal("start_ns", &start)?;
    let end_value = require_decimal("end_ns", &end)?;
    if end_value < start_value {
        return Err(CoreError::invalid(format!(
            "span {span_id} ends before it starts"
        )));
    }
    let attributes = raw.get("attributes").cloned().unwrap_or_else(|| json!({}));
    if !attributes.is_object() {
        return Err(CoreError::invalid("attributes must be an object"));
    }
    let mut span_links = Vec::new();
    if let Some(values) = raw.get("links") {
        let values = values
            .as_array()
            .ok_or_else(|| CoreError::invalid("span.links must be an array"))?;
        for (index, value) in values.iter().enumerate() {
            let target = non_empty(&value["target_span_id"], "span.links[].target_span_id")?;
            let link_kind = value["kind"].as_str().unwrap_or("link");
            validate_link_kind(link_kind)?;
            let correlation_id = value["correlation_id"]
                .as_str()
                .filter(|value| !value.is_empty())
                .map(str::to_string);
            if matches!(link_kind, "message" | "flow") && correlation_id.is_none() {
                return Err(CoreError::invalid(format!(
                    "span {span_id} {link_kind} link requires correlation_id"
                )));
            }
            let attributes = value.get("attributes").cloned().unwrap_or_else(|| json!({}));
            if !attributes.is_object() {
                return Err(CoreError::invalid("span.links[].attributes must be an object"));
            }
            span_links.push(RawLink {
                link_id: format!("{}:links.{index}", span_id),
                source_span_id: span_id.clone(),
                target_span_id: target,
                kind: link_kind.to_string(),
                correlation_id,
                attributes,
            });
        }
    }
    Ok(RawSpan {
        span_id,
        parent_id: parent,
        name,
        kind,
        clock_id,
        start_ns: start,
        end_ns: end,
        process_id: raw["process_id"].as_str().map(str::to_string),
        thread_id: raw["thread_id"].as_str().map(str::to_string),
        attributes,
        span_links,
    })
}

fn parse_links(trace: &Value, spans: &BTreeMap<String, RawSpan>) -> Result<Vec<RawLink>> {
    let Some(values) = trace.get("links") else {
        return Ok(Vec::new());
    };
    let values = values
        .as_array()
        .ok_or_else(|| CoreError::invalid("links must be an array"))?;
    if values.len() > MAX_LINKS {
        return Err(CoreError::invalid(format!(
            "resource_exhausted: {} links exceed {MAX_LINKS}",
            values.len()
        )));
    }
    let mut links = Vec::with_capacity(values.len());
    for (index, value) in values.iter().enumerate() {
        let link_id = value["link_id"]
            .as_str()
            .filter(|value| !value.is_empty())
            .map(str::to_string)
            .unwrap_or_else(|| format!("link.{index}"));
        let source = non_empty(&value["source_span_id"], "links[].source_span_id")?;
        let target = non_empty(&value["target_span_id"], "links[].target_span_id")?;
        if source == target {
            return Err(CoreError::invalid(format!(
                "link {link_id} connects span {source} to itself"
            )));
        }
        if !spans.contains_key(&source) {
            return Err(CoreError::invalid(format!(
                "link {link_id} references missing source span {source}"
            )));
        }
        if !spans.contains_key(&target) {
            return Err(CoreError::invalid(format!(
                "link {link_id} references missing target span {target}"
            )));
        }
        let kind = value["kind"]
            .as_str()
            .ok_or_else(|| CoreError::invalid("links[].kind must be a string"))?;
        validate_link_kind(kind)?;
        let correlation_id = value["correlation_id"]
            .as_str()
            .filter(|value| !value.is_empty())
            .map(str::to_string);
        if matches!(kind, "message" | "flow") && correlation_id.is_none() {
            return Err(CoreError::invalid(format!(
                "link {link_id} of kind {kind} requires an explicit correlation_id"
            )));
        }
        let attributes = value.get("attributes").cloned().unwrap_or_else(|| json!({}));
        if !attributes.is_object() {
            return Err(CoreError::invalid("links[].attributes must be an object"));
        }
        links.push(RawLink {
            link_id,
            source_span_id: source,
            target_span_id: target,
            kind: kind.to_string(),
            correlation_id,
            attributes,
        });
    }
    Ok(links)
}

fn validate_link_kind(kind: &str) -> Result<()> {
    match kind {
        "call" | "parent" | "message" | "flow" | "link" | "await" | "awaits" | "schedule"
        | "schedules" | "publish" | "publishes" | "consume" | "consumes" => Ok(()),
        other => Err(CoreError::invalid(format!(
            "unsupported causal link kind {other}; temporal proximity is not a link"
        ))),
    }
}

fn link_relation_kind(kind: &str) -> (&'static str, crate::store::RelationKind) {
    use crate::store::RelationKind;
    match kind {
        "call" | "parent" => ("observed_call", RelationKind::ObservedCall),
        "await" | "awaits" => ("awaits", RelationKind::Awaits),
        "schedule" | "schedules" => ("schedules", RelationKind::Schedules),
        "publish" | "publishes" => ("publishes", RelationKind::Publishes),
        "consume" | "consumes" => ("consumes", RelationKind::Consumes),
        _ => ("communicates_with", RelationKind::CommunicatesWith),
    }
}

fn link_strength(kind: &str) -> &'static str {
    match kind {
        "parent" => "explicit-parent-span",
        "message" | "flow" => "explicit-correlation-id",
        "call" => "explicit-call-link",
        _ => "explicit-link",
    }
}

fn span_clock<'a>(spans: &'a BTreeMap<String, RawSpan>, span_id: &str) -> &'a str {
    spans
        .get(span_id)
        .map(|span| span.clock_id.as_str())
        .unwrap_or("default")
}

fn clock_uncertainty(
    domains: &HashMap<String, Value>,
    source: &str,
    target: &str,
) -> Option<String> {
    let read = |clock_id: &str| {
        domains
            .get(clock_id)
            .and_then(|domain| domain.get("uncertainty_ns"))
            .and_then(Value::as_str)
            .and_then(|value| value.parse::<u128>().ok())
    };
    match (read(source), read(target)) {
        (Some(first), Some(second)) => {
            if source == target {
                Some(first.to_string())
            } else {
                Some(first.saturating_add(second).to_string())
            }
        }
        _ => None,
    }
}

fn push_sample(samples: &mut Vec<Value>, sample: Value) {
    if samples.len() < MAX_CAUSAL_SAMPLES {
        samples.push(sample);
    }
}

fn runtime_evidence_id(run_id: &str) -> String {
    format!("evidence.runtime.{run_id}")
}

fn detect_cycles(spans: &BTreeMap<String, RawSpan>) -> Result<()> {
    for span in spans.values() {
        let mut seen = HashMap::new();
        let mut current = span.parent_id.as_deref();
        while let Some(parent) = current {
            if seen.insert(parent.to_string(), ()).is_some() {
                return Err(CoreError::invalid("span parent cycle detected"));
            }
            current = spans.get(parent).and_then(|item| item.parent_id.as_deref());
        }
    }
    Ok(())
}

fn load_snapshot_config(store: &Store, snapshot_id: Option<&str>) -> Result<Option<String>> {
    let Some(snapshot_id) = snapshot_id else {
        return Ok(None);
    };
    Ok(store
        .conn
        .query_row(
            "SELECT config_digest FROM snapshots WHERE snapshot_id=?1",
            (snapshot_id,),
            |row| row.get(0),
        )
        .ok())
}

fn non_empty(value: &Value, field: &str) -> Result<String> {
    value
        .as_str()
        .filter(|text| !text.is_empty())
        .map(str::to_string)
        .ok_or_else(|| CoreError::invalid(format!("{field} must be a non-empty string")))
}

fn require_decimal(field: &str, value: &str) -> Result<u128> {
    if value.is_empty() || !value.bytes().all(|b| b.is_ascii_digit()) {
        return Err(CoreError::invalid(format!(
            "{field} must be a non-negative decimal integer string"
        )));
    }
    value
        .parse::<u128>()
        .map_err(|_| CoreError::invalid(format!("{field} is out of range")))
}

fn now_unix() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs() as i64
}

#[allow(dead_code)]
fn empty_attributes() -> Map<String, Value> {
    Map::new()
}
