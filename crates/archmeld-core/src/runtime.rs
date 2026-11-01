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

pub fn import_trace(
    store: &mut Store,
    request: &RuntimeImport<'_>,
    trace: &Value,
) -> Result<Value> {
    validate::require_id("project_id", request.project_id)?;
    validate::require_id("run_id", request.run_id)?;
    require_decimal("clock.origin_ns", trace["clock"]["origin_ns"].as_str().unwrap_or(""))?;
    let snapshot_config = load_snapshot_config(store, request.snapshot_id)?;
    let bound = match (request.snapshot_id, &snapshot_config) {
        (Some(snapshot), Some(config)) => {
            if config != request.config_digest {
                if !request.allow_unbound {
                    return Err(CoreError::conflict(
                        "stale_snapshot: trace config does not match the snapshot build",
                    ));
                }
                false
            } else {
                let _ = snapshot;
                true
            }
        }
        _ => false,
    };
    let trace_config = trace["config_digest"]
        .as_str()
        .ok_or_else(|| CoreError::invalid("trace config_digest is required"))?;
    let mut bound = bound;
    if trace_config != request.config_digest {
        if !request.allow_unbound {
            return Err(CoreError::conflict(
                "trace was produced by a different build config",
            ));
        }
        bound = false;
    }
    if let Some(trace_snapshot) = trace["snapshot_id"].as_str() {
        if Some(trace_snapshot) != request.snapshot_id {
            if !request.allow_unbound {
                return Err(CoreError::conflict(
                    "trace names a different snapshot than the import request",
                ));
            }
            bound = false;
        }
    }
    if !bound && !request.allow_unbound {
        return Err(CoreError::conflict(
            "trace is unbound to this snapshot/build; pass allow_unbound to import it explicitly",
        ));
    }
    if trace["version"].as_str() != Some("1.0") {
        return Err(CoreError::invalid("trace version must be 1.0"));
    }
    let tool = non_empty(&trace["capture"]["tool"], "capture.tool")?;
    let version = non_empty(&trace["capture"]["version"], "capture.version")?;
    let lossy = trace["capture"]["lossy"]
        .as_bool()
        .ok_or_else(|| CoreError::invalid("capture.lossy must be boolean"))?;
    let loss_notes = trace["capture"]["loss_notes"]
        .as_array()
        .ok_or_else(|| CoreError::invalid("capture.loss_notes must be an array"))?
        .iter()
        .map(|value| {
            value
                .as_str()
                .filter(|text| !text.is_empty())
                .map(str::to_string)
                .ok_or_else(|| CoreError::invalid("loss_notes entries must be non-empty strings"))
        })
        .collect::<Result<Vec<_>>>()?;
    let (domains, default_clock_id) = parse_clock_domains(trace)?;
    let clock_kind = non_empty(&trace["clock"]["kind"], "clock.kind")?;
    let clock_unit = non_empty(&trace["clock"]["unit"], "clock.unit")?;
    if clock_unit != "ns" {
        return Err(CoreError::unsupported("only nanosecond clocks are supported"));
    }
    let origin_ns = trace["clock"]["origin_ns"].as_str().unwrap().to_string();
    let build_identity = trace["build_identity"]
        .as_object()
        .ok_or_else(|| CoreError::invalid("build_identity must be an object"))?
        .clone();
    let scenario =
        resolve_scenario_association(store, request, trace["scenario_id"].as_str())?;
    let verified_scenario_id = scenario
        .verified
        .as_ref()
        .map(|value| value.run_id.clone());
    let probes = parse_probes(trace)?;
    let spans_value = trace["spans"]
        .as_array()
        .ok_or_else(|| CoreError::invalid("spans must be an array"))?;
    if spans_value.len() > MAX_SPANS {
        return Err(CoreError::invalid(format!(
            "resource_exhausted: {} spans exceed {MAX_SPANS}",
            spans_value.len()
        )));
    }

    let mut spans: BTreeMap<String, RawSpan> = BTreeMap::new();
    for raw in spans_value {
        let span = parse_span(raw, &default_clock_id, &domains)?;
        if spans.contains_key(&span.span_id) {
            return Err(CoreError::invalid(format!(
                "duplicate span_id {}",
                span.span_id
            )));
        }
        spans.insert(span.span_id.clone(), span);
    }
    for span in spans.values() {
        if let Some(parent) = &span.parent_id {
            if !spans.contains_key(parent) {
                return Err(CoreError::invalid(format!(
                    "span {} references missing parent {parent}",
                    span.span_id
                )));
            }
        }
        for link in &span.span_links {
            if !spans.contains_key(&link.source_span_id) {
                return Err(CoreError::invalid(format!(
                    "link {} references missing source span {}",
                    link.link_id, link.source_span_id
                )));
            }
            if !spans.contains_key(&link.target_span_id) {
                return Err(CoreError::invalid(format!(
                    "link {} references missing target span {}",
                    link.link_id, link.target_span_id
                )));
            }
        }
    }
    let mut links = parse_links(trace, &spans)?;
    links.extend(
        spans
            .values()
            .flat_map(|span| span.span_links.iter().cloned()),
    );
    links.sort_by(|a, b| a.link_id.cmp(&b.link_id));
    for pair in links.windows(2) {
        if pair[0].link_id == pair[1].link_id {
            return Err(CoreError::invalid(format!(
                "duplicate link_id {}",
                pair[0].link_id
            )));
        }
    }
    detect_cycles(&spans)?;
    let metrics = parse_metrics(trace, &default_clock_id, &domains)?;

    let mapping = resolve_mappings(store, request, &spans, &probes);
    let metric_report = resolve_metric_mappings(store, request, &metrics, &probes);
    let causal = build_causal_report(
        &mapping,
        &links,
        &spans,
        &domains,
        bound,
        &metric_report,
    );

    let raw_bytes = serde_json::to_vec(trace)
        .map_err(|err| CoreError::Storage(format!("trace encode: {err}")))?;
    let artifact = store.put_blob(&raw_bytes, "application/json")?;
    let digest = hash::digest_bytes(&raw_bytes);
    let existing: Option<String> = store
        .conn
        .query_row(
            "SELECT artifact_digest FROM runtime_runs WHERE project_id=?1 AND run_id=?2",
            (request.project_id, request.run_id),
            |row| row.get(0),
        )
        .ok();
    if let Some(existing) = existing {
        if existing == artifact {
            let recorded: Option<Option<String>> = store
                .conn
                .query_row(
                    "SELECT scenario_run_id FROM runtime_runs WHERE project_id=?1 AND run_id=?2",
                    (request.project_id, request.run_id),
                    |row| row.get(0),
                )
                .ok();
            if recorded.flatten() != verified_scenario_id {
                return Err(CoreError::conflict(
                    "scope_conflict: run_id already imported with a different scenario association",
                ));
            }
            return timeline(store, request.project_id, request.run_id, None, None, MAX_TIMELINE);
        }
        let _ = digest;
        return Err(CoreError::conflict(
            "scope_conflict: run_id already imported with different content",
        ));
    }

    let tx = store.conn.transaction()?;
    let domains_json =
        serde_json::to_string(&domains.iter().map(ClockDomain::to_json).collect::<Vec<_>>())
            .map_err(|err| CoreError::Storage(format!("clock domains encode: {err}")))?;
    let causal_json = serde_json::to_string(&causal)
        .map_err(|err| CoreError::Storage(format!("causal report encode: {err}")))?;
    tx.execute(
        "INSERT INTO runtime_runs
         (project_id,run_id,snapshot_id,config_digest,build_identity_json,capture_tool,capture_version,
          clock_kind,clock_unit,clock_origin_ns,lossy,loss_notes_json,artifact_digest,bound,span_count,
          created_at,metric_count,scenario_run_id)
         VALUES (?1,?2,?3,?4,?5,?6,?7,?8,'ns',?9,?10,?11,?12,?13,?14,?15,?16,?17)",
        rusqlite::params![
            request.project_id,
            request.run_id,
            if bound { request.snapshot_id } else { None },
            request.config_digest,
            serde_json::to_string(&build_identity).unwrap(),
            &tool,
            &version,
            &clock_kind,
            &origin_ns,
            if lossy { 1 } else { 0 },
            serde_json::to_string(&loss_notes).unwrap(),
            &artifact,
            if bound { 1 } else { 0 },
            spans.len() as i64,
            now_unix(),
            metrics.len() as i64,
            verified_scenario_id.as_deref(),
        ],
    )?;
    tx.execute(
        "UPDATE runtime_runs SET clock_domains_json=?1, causal_json=?2
         WHERE project_id=?3 AND run_id=?4",
        (
            &domains_json,
            &causal_json,
            request.project_id,
            request.run_id,
        ),
    )?;
    for span in spans.values() {
        tx.execute(
            "INSERT INTO runtime_spans
             (project_id,run_id,span_id,parent_id,name,kind,start_ns,end_ns,process_id,thread_id,
              attributes_json,clock_id)
             VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12)",
            (
                request.project_id,
                request.run_id,
                &span.span_id,
                &span.parent_id,
                &span.name,
                &span.kind,
                &span.start_ns,
                &span.end_ns,
                &span.process_id,
                &span.thread_id,
                serde_json::to_string(&span.attributes).unwrap(),
                &span.clock_id,
            ),
        )?;
    }
    for metric in &metrics {
        tx.execute(
            "INSERT INTO runtime_metrics
             (project_id,run_id,metric_id,kind,name,unit,clock_id,entity_id,attributes_json)
             VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9)",
            (
                request.project_id,
                request.run_id,
                &metric.metric_id,
                metric.kind,
                &metric.name,
                &metric.unit,
                &metric.clock_id,
                metric_report.resolved.get(&metric.metric_id),
                serde_json::to_string(&metric.attributes).unwrap(),
            ),
        )?;
        for (seq, sample) in metric.samples.iter().enumerate() {
            tx.execute(
                "INSERT INTO runtime_metric_samples
                 (project_id,run_id,metric_id,seq,start_ns,end_ns,value_json,process_id,thread_id)
                 VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9)",
                (
                    request.project_id,
                    request.run_id,
                    &metric.metric_id,
                    seq as i64,
                    &sample.start_ns,
                    &sample.end_ns,
                    serde_json::to_string(&sample.value).unwrap(),
                    &sample.process_id,
                    &sample.thread_id,
                ),
            )?;
        }
    }
    for link in &links {
        let cross_clock = span_clock(&spans, &link.source_span_id)
            != span_clock(&spans, &link.target_span_id);
        tx.execute(
            "INSERT INTO runtime_links
             (project_id,run_id,link_id,source_span_id,target_span_id,kind,correlation_id,
              cross_clock,attributes_json)
             VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9)",
            (
                request.project_id,
                request.run_id,
                &link.link_id,
                &link.source_span_id,
                &link.target_span_id,
                &link.kind,
                &link.correlation_id,
                if cross_clock { 1 } else { 0 },
                serde_json::to_string(&link.attributes).unwrap(),
            ),
        )?;
    }

    if let Some(snapshot_id) = request.snapshot_id {
        let evidence_id = insert_runtime_evidence(
            &tx,
            request,
            snapshot_id,
            &tool,
            &version,
            &artifact,
            &scenario,
            bound,
            &causal,
            lossy,
            &loss_notes,
            spans.len(),
            &metric_report,
        )?;
        if bound {
            insert_observed_relations(
                &tx,
                request,
                snapshot_id,
                &evidence_id,
                &spans,
                &links,
                &mapping,
            )?;
        }
    }
    tx.commit()?;
    timeline(store, request.project_id, request.run_id, None, None, MAX_TIMELINE)
}

fn resolve_scenario_association(
    store: &Store,
    request: &RuntimeImport<'_>,
    trace_scenario: Option<&str>,
) -> Result<ScenarioResolution> {
    let requested = request.scenario_run_id.filter(|value| !value.is_empty());
    let embedded = trace_scenario.filter(|value| !value.is_empty());
    let Some(requested) = requested else {
        // Without an explicit declaration the trace's scenario_id stays
        // producer metadata; it is recorded and marked unverified instead of
        // silently becoming a link to a run that may not exist.
        if let Some(value) = embedded {
            validate::require_id("trace.scenario_id", value)?;
            return Ok(ScenarioResolution {
                declared_id: Some(value.to_string()),
                verified: None,
            });
        }
        return Ok(ScenarioResolution::default());
    };
    validate::require_id("scenario_run_id", requested)?;
    if let Some(trace_value) = embedded {
        validate::require_id("trace.scenario_id", trace_value)?;
        if trace_value != requested {
            return Err(CoreError::conflict(
                "trace scenario_id does not match the declared scenario run",
            ));
        }
    }
    let row: Option<(String, String, Option<i64>, i64)> = store
        .conn
        .query_row(
            "SELECT snapshot_id, config_digest, exit_code, timed_out
             FROM scenario_runs WHERE project_id=?1 AND run_id=?2",
            (request.project_id, requested),
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?)),
        )
        .ok();
    let Some((snapshot_id, config_digest, exit_code, timed_out)) = row else {
        return Err(CoreError::invalid(format!(
            "scenario run {requested} not found in this project; run it before associating a trace"
        )));
    };
    if let Some(requested_snapshot) = request.snapshot_id {
        if requested_snapshot != snapshot_id {
            return Err(CoreError::conflict(
                "scenario run snapshot differs from the trace snapshot",
            ));
        }
    }
    if config_digest != request.config_digest {
        return Err(CoreError::conflict(
            "scenario run config differs from the trace build config",
        ));
    }
    Ok(ScenarioResolution {
        declared_id: Some(requested.to_string()),
        verified: Some(ScenarioAssociation {
            run_id: requested.to_string(),
            snapshot_id,
            config_digest,
            exit_code,
            timed_out: timed_out == 1,
        }),
    })
}

#[allow(clippy::too_many_arguments)]
fn insert_runtime_evidence(
    tx: &rusqlite::Transaction<'_>,
    request: &RuntimeImport<'_>,
    snapshot_id: &str,
    tool: &str,
    version: &str,
    artifact: &str,
    scenario: &ScenarioResolution,
    bound: bool,
    causal: &Value,
    lossy: bool,
    loss_notes: &[String],
    span_count: usize,
    metrics: &MetricReport,
) -> Result<String> {
    let unmapped = causal["unmapped_spans"].as_u64().unwrap_or(0)
        + causal["unmapped_links"].as_u64().unwrap_or(0);
    let metric_unresolved = metrics.unresolved() as u64;
    let metric_count = metrics.metric_count();
    let mut limitations = Vec::new();
    if lossy {
        limitations.push("source export is lossy; see loss_notes".to_string());
        limitations.extend(loss_notes.iter().cloned());
    }
    if unmapped > 0 {
        limitations.push(format!(
            "{unmapped} spans/links have no explicit entity mapping"
        ));
    }
    if metrics.process_scope > 0 {
        limitations.push(format!(
            "{} counters/states are process-scope observations without an explicit source entity",
            metrics.process_scope
        ));
    }
    if metric_unresolved > 0 {
        limitations.push(format!(
            "{metric_unresolved} counters/states declare a source mapping that did not resolve"
        ));
    }
    if !bound {
        limitations.push("trace is unbound; it cannot verify a ChangeSet".to_string());
    }
    if let (None, Some(run_id)) = (&scenario.verified, &scenario.declared_id) {
        limitations.push(format!(
            "scenario_id {run_id} is producer-declared and was not linked to a recorded scenario run"
        ));
    }
    let complete = !lossy && unmapped == 0 && metric_unresolved == 0;
    let mut scope = "normalized trace spans and explicit causal links".to_string();
    if metric_count > 0 {
        scope.push_str("; counters and state intervals are observations only");
    }
    let coverage = Coverage {
        state: if complete {
            CoverageState::Complete
        } else {
            CoverageState::Partial
        },
        scope,
        expected_count: Some((span_count + metric_count) as u64),
        observed_count: (span_count + metric_count) as u64,
        omitted_count: 0,
        unresolved_count: unmapped + metric_unresolved,
        limitations,
    };
    let coverage_json = serde_json::to_string(&coverage)
        .map_err(|err| CoreError::Storage(format!("coverage encode: {err}")))?;
    let evidence_id = runtime_evidence_id(request.run_id);
    let freshness = if bound { "current" } else { "unbound" };
    let evidence = json!({
        "id": &evidence_id,
        "project_id": request.project_id,
        "snapshot_id": snapshot_id,
        "config_digest": request.config_digest,
        "kind": "runtime",
        "producer": {
            "name": tool,
            "version": version,
            "source_ref": "archmeld-trace-v1",
            "run_id": request.run_id,
        },
        "artifact_digest": artifact,
        "locator": format!("runtime:{}", request.run_id),
        "source_anchor": null,
        "scenario_id": scenario.declared_id,
        "scenario": match (&scenario.verified, &scenario.declared_id) {
            (Some(value), _) => json!({
                "run_id": value.run_id,
                "snapshot_id": value.snapshot_id,
                "config_digest": value.config_digest,
                "exit_code": value.exit_code,
                "timed_out": value.timed_out,
                "association": "explicit: existing scenario run with matching snapshot and build config",
            }),
            (None, Some(run_id)) => json!({
                "run_id": run_id,
                "association": "producer_declared_unverified",
                "reason": "no scenario_run_id was supplied; the declared run was not checked against scenario_runs",
            }),
            (None, None) => Value::Null,
        },
        "runtime_run_id": request.run_id,
        "coverage": coverage,
        "freshness": freshness,
    });
    let payload_json = hash::canonical_json(&evidence)
        .map_err(|err| CoreError::invalid(format!("runtime evidence is outside the hash domain: {err}")))?;
    tx.execute(
        "INSERT INTO evidence
         (project_id, snapshot_id, evidence_id, config_digest, kind, producer_name, producer_version,
          artifact_digest, coverage_json, freshness, payload_json)
         VALUES (?1,?2,?3,?4,'runtime',?5,?6,?7,?8,?9,?10)",
        (
            request.project_id,
            snapshot_id,
            &evidence_id,
            request.config_digest,
            tool,
            version,
            artifact,
            &coverage_json,
            freshness,
            &payload_json,
        ),
    )?;
    tx.execute(
        "INSERT OR IGNORE INTO artifact_refs (owner_type, owner_id, digest) VALUES ('evidence', ?1, ?2)",
        (&evidence_id, artifact),
    )?;
    Ok(evidence_id)
}

fn insert_observed_relations(
    tx: &rusqlite::Transaction<'_>,
    request: &RuntimeImport<'_>,
    snapshot_id: &str,
    evidence_id: &str,
    spans: &BTreeMap<String, RawSpan>,
    links: &[RawLink],
    mapping: &MappingReport,
) -> Result<()> {
    let mut groups: BTreeMap<(String, String, &'static str), CausalGroup> = BTreeMap::new();
    let mut add = |source_span: &str,
                   target_span: &str,
                   link_kind: &str,
                   correlation: Option<&str>| {
        let (Some(source_id), Some(target_id)) =
            (mapping.mapped.get(source_span), mapping.mapped.get(target_span))
        else {
            return;
        };
        if source_id == target_id && link_kind != "parent" {
            return;
        }
        let (kind, _relation_kind) = link_relation_kind(link_kind);
        let cross = span_clock(spans, source_span) != span_clock(spans, target_span);
        let key = (source_id.clone(), target_id.clone(), kind);
        let group = groups.entry(key).or_insert_with(|| CausalGroup {
            source_id: source_id.clone(),
            target_id: target_id.clone(),
            kind,
            edges: BTreeSet::new(),
            correlations: BTreeSet::new(),
            cross_clock: false,
            observations: 0,
        });
        group.observations += 1;
        group.cross_clock |= cross;
        if let Some(correlation) = correlation.filter(|value| !value.is_empty()) {
            group.correlations.insert(correlation.to_string());
        }
        if group.edges.len() < MAX_EDGE_SAMPLES {
            group
                .edges
                .insert(format!("span:{source_span}->{target_span}"));
        }
    };

    for span in spans.values() {
        if let Some(parent) = &span.parent_id {
            add(parent, &span.span_id, "parent", None);
        }
    }
    for link in links {
        add(
            &link.source_span_id,
            &link.target_span_id,
            &link.kind,
            link.correlation_id.as_deref(),
        );
    }

    for group in groups.values() {
        let identity = json!({
            "version": "1.0",
            "run_id": request.run_id,
            "source": group.source_id,
            "target": group.target_id,
            "kind": group.kind,
        });
        let digest = hash::digest_value(&identity)
            .map_err(|err| CoreError::invalid(format!("relation identity: {err}")))?;
        let relation_id = format!(
            "rel.{}",
            digest.strip_prefix("sha256:").expect("sha256 prefix")
        );
        let mut conditions = vec![
            format!("runtime-run:{}", request.run_id),
            format!("causal-basis:explicit-{}", group.kind),
            "resolution:observed-at-runtime".to_string(),
        ];
        if group.kind == "observed_call" {
            conditions.push("not promoted from static_call_candidate".to_string());
        }
        if group.cross_clock {
            conditions.push("cross-clock:explicit-link; no total order claimed".to_string());
        }
        conditions.push(format!("observation-count:{}", group.observations));
        for correlation in &group.correlations {
            conditions.push(format!("correlation-id:{correlation}"));
        }
        for edge in &group.edges {
            conditions.push(edge.clone());
        }
        let relation = json!({
            "id": relation_id,
            "project_id": request.project_id,
            "snapshot_id": snapshot_id,
            "config_digest": request.config_digest,
            "source_id": group.source_id,
            "target_id": group.target_id,
            "kind": group.kind,
            "basis": "runtime",
            "evidence_ids": [evidence_id],
            "conditions": conditions,
            "resolution": "observed",
        });
        let payload_json = hash::canonical_json(&relation)
            .map_err(|err| CoreError::invalid(format!("relation is outside the hash domain: {err}")))?;
        // An existing relation with the same identity (for example a static
        // candidate) is never overwritten; observed edges are new rows.
        let existing: Option<String> = tx
            .query_row(
                "SELECT payload_json FROM relations
                 WHERE project_id=?1 AND snapshot_id=?2 AND relation_id=?3",
                (request.project_id, snapshot_id, &relation_id),
                |row| row.get(0),
            )
            .ok();
        if let Some(existing) = existing {
            if existing == payload_json {
                continue;
            }
            return Err(CoreError::conflict(format!(
                "relation {relation_id} already exists with different content"
            )));
        }
        tx.execute(
            "INSERT INTO relations
             (project_id, snapshot_id, relation_id, config_digest, source_id, target_id, kind,
              basis, resolution, payload_json)
             VALUES (?1,?2,?3,?4,?5,?6,?7,'runtime','observed',?8)",
            (
                request.project_id,
                snapshot_id,
                &relation_id,
                request.config_digest,
                &group.source_id,
                &group.target_id,
                group.kind,
                &payload_json,
            ),
        )?;
        tx.execute(
            "INSERT INTO relation_evidence (project_id, snapshot_id, relation_id, evidence_id)
             VALUES (?1,?2,?3,?4)",
            (request.project_id, snapshot_id, &relation_id, evidence_id),
        )?;
    }
    Ok(())
}

pub fn timeline(
    store: &Store,
    project_id: &str,
    run_id: &str,
    from_ns: Option<&str>,
    to_ns: Option<&str>,
    limit: usize,
) -> Result<Value> {
    validate::require_id("project_id", project_id)?;
    if limit == 0 || limit > MAX_TIMELINE {
        return Err(CoreError::invalid(format!("limit must be 1..={MAX_TIMELINE}")));
    }
    if let Some(value) = from_ns {
        require_decimal("from_ns", value)?;
    }
    if let Some(value) = to_ns {
        require_decimal("to_ns", value)?;
    }
    let from = from_ns.map(str::to_string);
    let to = to_ns.map(str::to_string);
    let run = store
        .conn
        .query_row(
            "SELECT snapshot_id, bound, lossy, loss_notes_json, clock_kind, clock_unit, clock_origin_ns,
                    capture_tool, capture_version, artifact_digest, span_count, clock_domains_json,
                    causal_json, metric_count, scenario_run_id
             FROM runtime_runs WHERE project_id=?1 AND run_id=?2",
            (project_id, run_id),
            |row| {
                Ok((
                    row.get::<_, Option<String>>(0)?,
                    row.get::<_, i64>(1)? == 1,
                    row.get::<_, i64>(2)? == 1,
                    row.get::<_, String>(3)?,
                    row.get::<_, String>(4)?,
                    row.get::<_, String>(5)?,
                    row.get::<_, String>(6)?,
                    row.get::<_, String>(7)?,
                    row.get::<_, String>(8)?,
                    row.get::<_, Option<String>>(9)?,
                    row.get::<_, i64>(10)?,
                    row.get::<_, String>(11)?,
                    row.get::<_, String>(12)?,
                    row.get::<_, i64>(13)?,
                    row.get::<_, Option<String>>(14)?,
                ))
            },
        )
        .ok();
    let Some((
        snapshot,
        bound,
        lossy,
        notes,
        clock_kind,
        unit,
        origin,
        tool,
        version,
        artifact,
        total,
        domains_json,
        causal_json,
        metric_count,
        scenario_run,
    )) = run
    else {
        return Err(CoreError::not_found(format!("runtime run {run_id} not found")));
    };
    let domains: Vec<Value> = serde_json::from_str(&domains_json).unwrap_or_else(|_| vec![]);
    let causal: Value = serde_json::from_str(&causal_json).unwrap_or_else(|_| json!({}));
    let total_order = domains.len() <= 1;
    let mut statement = store.conn.prepare(
        "SELECT span_id,parent_id,name,kind,start_ns,end_ns,process_id,thread_id,attributes_json,clock_id
         FROM runtime_spans WHERE project_id=?1 AND run_id=?2",
    )?;
    let rows = statement.query_map((project_id, run_id), |row| {
        Ok((
            row.get::<_, String>(0)?,
            row.get::<_, Option<String>>(1)?,
            row.get::<_, String>(2)?,
            row.get::<_, String>(3)?,
            row.get::<_, String>(4)?,
            row.get::<_, String>(5)?,
            row.get::<_, Option<String>>(6)?,
            row.get::<_, Option<String>>(7)?,
            row.get::<_, String>(8)?,
            row.get::<_, String>(9)?,
        ))
    })?;
    let from_value = from.and_then(|value| value.parse::<u128>().ok());
    let to_value = to.and_then(|value| value.parse::<u128>().ok());
    let mut spans = Vec::new();
    for row in rows {
        let (span_id, parent, name, kind, start, end, process, thread, attributes, clock_id) = row?;
        let start_value = start
            .parse::<u128>()
            .map_err(|_| CoreError::Storage("bad start_ns".into()))?;
        let end_value = end
            .parse::<u128>()
            .map_err(|_| CoreError::Storage("bad end_ns".into()))?;
        if let Some(from_value) = from_value {
            if end_value < from_value {
                continue;
            }
        }
        if let Some(to_value) = to_value {
            if start_value > to_value {
                continue;
            }
        }
        spans.push((
            start_value,
            end_value,
            span_id,
            parent,
            name,
            kind,
            start,
            end,
            process,
            thread,
            attributes,
            clock_id,
        ));
    }
    if total_order {
        spans.sort_by(|a, b| (a.0, a.1, &a.2).cmp(&(b.0, b.1, &b.2)));
    } else {
        // Separate clock domains are never interleaved by bare numbers: two
        // machine clocks with different origins cannot be totally ordered
        // without calibration evidence.
        spans.sort_by(|a, b| (a.11.as_str(), a.0, a.1, &a.2).cmp(&(b.11.as_str(), b.0, b.1, &b.2)));
    }
    let truncated = spans.len() > limit;
    spans.truncate(limit);
    let observed = spans.len();
    let spans: Vec<Value> = spans
        .into_iter()
        .map(
            |(
                _,
                _,
                span_id,
                parent,
                name,
                kind,
                start,
                end,
                process,
                thread,
                attributes,
                clock_id,
            )| {
                json!({
                    "span_id": span_id,
                    "parent_span_id": parent,
                    "name": name,
                    "kind": kind,
                    "start_ns": start,
                    "end_ns": end,
                    "clock_id": clock_id,
                    "process_id": process,
                    "thread_id": thread,
                    "attributes": serde_json::from_str::<Value>(&attributes).unwrap_or_else(|_| json!({})),
                })
            },
        )
        .collect();

    let mut link_statement = store.conn.prepare(
        "SELECT l.link_id,l.source_span_id,l.target_span_id,l.kind,l.correlation_id,
                l.cross_clock,l.attributes_json,s.clock_id,t.clock_id
         FROM runtime_links l
         JOIN runtime_spans s ON s.project_id=l.project_id AND s.run_id=l.run_id
              AND s.span_id=l.source_span_id
         JOIN runtime_spans t ON t.project_id=l.project_id AND t.run_id=l.run_id
              AND t.span_id=l.target_span_id
         WHERE l.project_id=?1 AND l.run_id=?2
         ORDER BY l.link_id",
    )?;
    let link_rows = link_statement.query_map((project_id, run_id), |row| {
        Ok((
            row.get::<_, String>(0)?,
            row.get::<_, String>(1)?,
            row.get::<_, String>(2)?,
            row.get::<_, String>(3)?,
            row.get::<_, Option<String>>(4)?,
            row.get::<_, i64>(5)? == 1,
            row.get::<_, String>(6)?,
            row.get::<_, String>(7)?,
            row.get::<_, String>(8)?,
        ))
    })?;
    let domain_lookup: HashMap<String, Value> = domains
        .iter()
        .filter_map(|domain| {
            domain
                .get("clock_id")
                .and_then(Value::as_str)
                .map(|id| (id.to_string(), domain.clone()))
        })
        .collect();
    let mut links = Vec::new();
    for row in link_rows {
        let (
            link_id,
            source,
            target,
            kind,
            correlation,
            cross_clock,
            attributes,
            source_clock,
            target_clock,
        ) = row?;
        let uncertainty = clock_uncertainty(&domain_lookup, &source_clock, &target_clock);
        links.push(json!({
            "link_id": link_id,
            "source_span_id": source,
            "target_span_id": target,
            "kind": kind,
            "correlation_id": correlation,
            "cross_clock": cross_clock,
            "source_clock_id": source_clock,
            "target_clock_id": target_clock,
            "clock_uncertainty_ns": uncertainty,
            "strength": link_strength(&kind),
            "attributes": serde_json::from_str::<Value>(&attributes).unwrap_or_else(|_| json!({})),
        }));
    }
    let links_truncated = links.len() > MAX_TIMELINE;
    links.truncate(MAX_TIMELINE);

    let (counters, states, metrics_truncated) =
        load_timeline_metrics(store, project_id, run_id, from_value, to_value, limit)?;
    let observed_metrics = counters.len().saturating_add(states.len());

    let notes: Value = serde_json::from_str(&notes).unwrap_or_else(|_| json!([]));
    Ok(json!({
        "run_id": run_id,
        "snapshot_id": snapshot,
        "scenario_run_id": scenario_run,
        "bound": bound,
        "lossy": lossy,
        "loss_notes": notes,
        "clock": {
            "kind": clock_kind,
            "unit": unit,
            "origin_ns": origin,
            "domains": domains,
            "total_order": total_order,
            "ordering": if total_order {
                "single-clock-start-time"
            } else {
                "declared-clock-domain-then-local-time; no cross-domain total order"
            },
        },
        "capture": {"tool": tool, "version": version},
        "artifact_digest": artifact,
        "span_count": total,
        "spans": spans,
        "links": links,
        "links_truncated": links_truncated,
        "metric_count": metric_count,
        "counters": counters,
        "states": states,
        "metrics_truncated": metrics_truncated,
        "causal": causal,
        "truncated": truncated,
        "coverage": {
            "state": if truncated || metrics_truncated { "partial" } else { "complete" },
            "scope": "runtime run spans, counters and state intervals in the requested window",
            "expected_count": total as u64 + metric_count as u64,
            "observed_count": observed as u64 + observed_metrics as u64,
            "omitted_count": 0,
            "unresolved_count": 0,
            "limitations": if lossy { vec!["source export is lossy; see loss_notes"] } else { vec![] }
        }
    }))
}

/// Aggregate counts for a run without materializing the timeline. Used by
/// resource-budget tests and review surfaces that only need sizes.
pub fn runtime_summary(store: &Store, project_id: &str, run_id: &str) -> Result<Value> {
    validate::require_id("project_id", project_id)?;
    validate::require_id("run_id", run_id)?;
    let run = store
        .conn
        .query_row(
            "SELECT span_count, metric_count, artifact_digest, bound, lossy, capture_tool,
                    capture_version, scenario_run_id
             FROM runtime_runs WHERE project_id=?1 AND run_id=?2",
            (project_id, run_id),
            |row| {
                Ok((
                    row.get::<_, i64>(0)?,
                    row.get::<_, i64>(1)?,
                    row.get::<_, Option<String>>(2)?,
                    row.get::<_, i64>(3)? == 1,
                    row.get::<_, i64>(4)? == 1,
                    row.get::<_, String>(5)?,
                    row.get::<_, String>(6)?,
                    row.get::<_, Option<String>>(7)?,
                ))
            },
        )
        .ok();
    let Some((span_count, metric_count, artifact, bound, lossy, tool, version, scenario_run)) = run else {
        return Err(CoreError::not_found(format!("runtime run {run_id} not found")));
    };
    let sample_count: i64 = store.conn.query_row(
        "SELECT COUNT(*) FROM runtime_metric_samples WHERE project_id=?1 AND run_id=?2",
        (project_id, run_id),
        |row| row.get(0),
    )?;
    let interval_count: i64 = store.conn.query_row(
        "SELECT COUNT(*) FROM runtime_metric_samples
         WHERE project_id=?1 AND run_id=?2 AND end_ns IS NOT NULL",
        (project_id, run_id),
        |row| row.get(0),
    )?;
    let counter_count: i64 = store.conn.query_row(
        "SELECT COUNT(*) FROM runtime_metrics
         WHERE project_id=?1 AND run_id=?2 AND kind='counter'",
        (project_id, run_id),
        |row| row.get(0),
    )?;
    let state_count: i64 = store.conn.query_row(
        "SELECT COUNT(*) FROM runtime_metrics
         WHERE project_id=?1 AND run_id=?2 AND kind='state'",
        (project_id, run_id),
        |row| row.get(0),
    )?;
    let artifact_size = match &artifact {
        Some(digest) => store.blob_size(digest)?,
        None => None,
    };
    Ok(json!({
        "run_id": run_id,
        "span_count": span_count,
        "metric_count": metric_count,
        "counter_count": counter_count,
        "state_count": state_count,
        "sample_count": sample_count,
        "state_interval_count": interval_count,
        "event_count": span_count as i64 + sample_count,
        "artifact_digest": artifact,
        "artifact_size": artifact_size,
        "bound": bound,
        "lossy": lossy,
        "capture": {"tool": tool, "version": version},
        "scenario_run_id": scenario_run,
    }))
}

/// Load counters and states for a timeline window. Each series is capped at
/// `MAX_METRIC_SAMPLES_TIMELINE` samples and the number of series at `limit`;
/// both truncations are explicit instead of silently dropping data.
fn load_timeline_metrics(
    store: &Store,
    project_id: &str,
    run_id: &str,
    from_value: Option<u128>,
    to_value: Option<u128>,
    limit: usize,
) -> Result<(Vec<Value>, Vec<Value>, bool)> {
    let mut statement = store.conn.prepare(
        "SELECT metric_id,kind,name,unit,clock_id,entity_id,attributes_json
         FROM runtime_metrics WHERE project_id=?1 AND run_id=?2
         ORDER BY kind, metric_id",
    )?;
    let rows = statement.query_map((project_id, run_id), |row| {
        Ok((
            row.get::<_, String>(0)?,
            row.get::<_, String>(1)?,
            row.get::<_, String>(2)?,
            row.get::<_, Option<String>>(3)?,
            row.get::<_, String>(4)?,
            row.get::<_, Option<String>>(5)?,
            row.get::<_, String>(6)?,
        ))
    })?;
    let mut counters = Vec::new();
    let mut states = Vec::new();
    let mut truncated = false;
    let mut sample_statement = store.conn.prepare(
        "SELECT start_ns,end_ns,value_json,process_id,thread_id
         FROM runtime_metric_samples
         WHERE project_id=?1 AND run_id=?2 AND metric_id=?3 ORDER BY seq",
    )?;
    for row in rows {
        let (metric_id, kind, name, unit, clock_id, entity_id, attributes) = row?;
        if counters.len() + states.len() >= limit {
            truncated = true;
            break;
        }
        let mut samples = Vec::new();
        let mut series_truncated = false;
        let sample_rows = sample_statement.query_map(
            (project_id, run_id, &metric_id),
            |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, Option<String>>(1)?,
                    row.get::<_, String>(2)?,
                    row.get::<_, Option<String>>(3)?,
                    row.get::<_, Option<String>>(4)?,
                ))
            },
        )?;
        for sample in sample_rows {
            let (start, end, value, process, thread) = sample?;
            let start_value = start
                .parse::<u128>()
                .map_err(|_| CoreError::Storage("bad metric start_ns".into()))?;
            let end_value = match &end {
                Some(end) => end
                    .parse::<u128>()
                    .map_err(|_| CoreError::Storage("bad metric end_ns".into()))?,
                None => start_value,
            };
            if let Some(from_value) = from_value {
                if end_value < from_value {
                    continue;
                }
            }
            if let Some(to_value) = to_value {
                if start_value > to_value {
                    continue;
                }
            }
            if samples.len() >= MAX_METRIC_SAMPLES_TIMELINE {
                series_truncated = true;
                truncated = true;
                break;
            }
            let value = serde_json::from_str::<Value>(&value).unwrap_or(Value::Null);
            if kind == "counter" {
                samples.push(json!({
                    "timestamp_ns": start,
                    "value": value,
                    "process_id": process,
                    "thread_id": thread,
                }));
            } else {
                samples.push(json!({
                    "start_ns": start,
                    "end_ns": end,
                    "value": value,
                    "process_id": process,
                    "thread_id": thread,
                }));
            }
        }
        let attributes: Value = serde_json::from_str(&attributes).unwrap_or_else(|_| json!({}));
        let sample_count = samples.len();
        if kind == "counter" {
            counters.push(json!({
                "counter_id": metric_id,
                "name": name,
                "unit": unit,
                "clock_id": clock_id,
                "entity_id": entity_id,
                "attributes": attributes,
                "samples": samples,
                "sample_count": sample_count,
                "samples_truncated": series_truncated,
            }));
        } else {
            states.push(json!({
                "state_id": metric_id,
                "name": name,
                "unit": unit,
                "clock_id": clock_id,
                "entity_id": entity_id,
                "attributes": attributes,
                "intervals": samples,
                "interval_count": sample_count,
                "intervals_truncated": series_truncated,
            }));
        }
    }
    Ok((counters, states, truncated))
}

fn build_causal_report(
    mapping: &MappingReport,
    links: &[RawLink],
    spans: &BTreeMap<String, RawSpan>,
    domains: &[ClockDomain],
    bound: bool,
    metrics: &MetricReport,
) -> Value {
    let mut unmapped_links = 0u64;
    let mut samples = mapping.missing_samples.clone();
    let mut explicit_edges = 0u64;
    for span in spans.values() {
        if span.parent_id.is_some() {
            explicit_edges += 1;
            if span.parent_id.as_ref().is_some_and(|parent| {
                !mapping.mapped.contains_key(parent) || !mapping.mapped.contains_key(&span.span_id)
            }) {
                unmapped_links += 1;
                push_sample(
                    &mut samples,
                    json!({
                        "span_id": span.span_id,
                        "reason": "parent_edge_endpoint_unmapped",
                    }),
                );
            }
        }
    }
    for link in links {
        explicit_edges += 1;
        if !mapping.mapped.contains_key(&link.source_span_id)
            || !mapping.mapped.contains_key(&link.target_span_id)
        {
            unmapped_links += 1;
            push_sample(
                &mut samples,
                json!({
                    "span_id": link.source_span_id,
                    "target_span_id": link.target_span_id,
                    "reason": "link_endpoint_unmapped",
                }),
            );
        }
    }
    json!({
        "state": if !bound {
            "unbound"
        } else if mapping.unmapped_spans > 0
            || unmapped_links > 0
            || metrics.unresolved() > 0
        {
            "partial"
        } else {
            "complete"
        },
        "bound": bound,
        "mapped_spans": mapping.mapped_spans,
        "unmapped_spans": mapping.unmapped_spans,
        "mapped_entities": mapping.mapped.values().collect::<BTreeSet<_>>().len(),
        "unknown_entity": mapping.unknown_entity,
        "unknown_probe": mapping.unknown_probe,
        "mapping_conflicts": mapping.conflicts,
        "causal_edges": explicit_edges,
        "unmapped_links": unmapped_links,
        "link_count": links.len(),
        "correlation_links": links.iter().filter(|link| link.correlation_id.is_some()).count(),
        "missing_association": samples,
        "clock_domains": domains.iter().map(ClockDomain::to_json).collect::<Vec<_>>(),
        "metrics": {
            "counters": metrics.counters,
            "states": metrics.states,
            "samples": metrics.samples,
            "mapped": metrics.mapped,
            "process_scope": metrics.process_scope,
            "unknown_entity": metrics.unknown_entity,
            "unknown_probe": metrics.unknown_probe,
            "mapping_conflicts": metrics.conflicts,
            "missing_samples": metrics.missing_samples,
            "causality": "none: counters and states never create causal edges",
        },
    })
}

fn resolve_mappings(
    store: &Store,
    request: &RuntimeImport<'_>,
    spans: &BTreeMap<String, RawSpan>,
    probes: &HashMap<String, String>,
) -> MappingReport {
    let mut report = MappingReport {
        mapped: HashMap::new(),
        missing_samples: Vec::new(),
        mapped_spans: 0,
        unmapped_spans: 0,
        unknown_entity: 0,
        unknown_probe: 0,
        conflicts: 0,
    };
    let Some(snapshot_id) = request.snapshot_id else {
        report.unmapped_spans = spans.len();
        push_sample(
            &mut report.missing_samples,
            json!({"reason": "trace_has_no_bound_snapshot"}),
        );
        return report;
    };
    for span in spans.values() {
        let direct = span.attributes.get("archmeld.entity_id").and_then(Value::as_str);
        let probe = span.attributes.get("archmeld.probe_id").and_then(Value::as_str);
        let mut entity_id = direct.map(str::to_string);
        if let Some(probe) = probe {
            match probes.get(probe) {
                Some(mapped) => {
                    if let Some(direct) = &entity_id {
                        if direct != mapped {
                            report.conflicts += 1;
                            report.unmapped_spans += 1;
                            push_sample(
                                &mut report.missing_samples,
                                json!({
                                    "span_id": span.span_id,
                                    "reason": "entity_id_and_probe_disagree",
                                    "entity_id": direct,
                                    "probe_entity_id": mapped,
                                }),
                            );
                            continue;
                        }
                    } else {
                        entity_id = Some(mapped.clone());
                    }
                }
                None => {
                    report.unknown_probe += 1;
                    report.unmapped_spans += 1;
                    push_sample(
                        &mut report.missing_samples,
                        json!({
                            "span_id": span.span_id,
                            "reason": "unknown_probe",
                            "probe_id": probe,
                        }),
                    );
                    continue;
                }
            }
        }
        let Some(entity_id) = entity_id else {
            report.unmapped_spans += 1;
            push_sample(
                &mut report.missing_samples,
                json!({
                    "span_id": span.span_id,
                    "reason": "no_explicit_entity_or_probe",
                }),
            );
            continue;
        };
        if validate::require_id("archmeld.entity_id", &entity_id).is_err() {
            report.unknown_entity += 1;
            report.unmapped_spans += 1;
            push_sample(
                &mut report.missing_samples,
                json!({
                    "span_id": span.span_id,
                    "reason": "invalid_entity_id",
                }),
            );
            continue;
        }
        let exists: Option<String> = store
            .conn
            .query_row(
                "SELECT entity_id FROM entities
                 WHERE project_id=?1 AND snapshot_id=?2 AND entity_id=?3",
                (request.project_id, snapshot_id, &entity_id),
                |row| row.get(0),
            )
            .ok();
        if exists.is_none() {
            report.unknown_entity += 1;
            report.unmapped_spans += 1;
            push_sample(
                &mut report.missing_samples,
                json!({
                    "span_id": span.span_id,
                    "reason": "unknown_entity",
                    "entity_id": entity_id,
                }),
            );
            continue;
        }
        report.mapped.insert(span.span_id.clone(), entity_id);
        report.mapped_spans += 1;
    }
    report
}

/// Parse optional `counters` and `states`. Samples are explicit observations:
/// a counter sample is a finite number at a timestamp, a state interval is a
/// categorical value between two timestamps. Functional names never select a
/// clock or an entity.
fn parse_metrics(
    trace: &Value,
    default_clock_id: &str,
    domains: &[ClockDomain],
) -> Result<Vec<RawMetric>> {
    let mut metrics = Vec::new();
    let mut ids = BTreeSet::new();
    let mut total_samples = 0usize;
    for (field, kind, id_field, samples_field) in [
        ("counters", "counter", "counter_id", "samples"),
        ("states", "state", "state_id", "intervals"),
    ] {
        let Some(list) = trace.get(field) else {
            continue;
        };
        let list = list
            .as_array()
            .ok_or_else(|| CoreError::invalid(format!("{field} must be an array")))?;
        if list.len() > MAX_METRICS {
            return Err(CoreError::invalid(format!(
                "resource_exhausted: {} {field} exceed {MAX_METRICS}",
                list.len()
            )));
        }
        for raw in list {
            let metric_id = non_empty(&raw[id_field], &format!("{field}[].{id_field}"))?;
            require_metric_id(&metric_id)?;
            if !ids.insert(metric_id.clone()) {
                return Err(CoreError::invalid(format!(
                    "duplicate metric id {metric_id}"
                )));
            }
            let name = non_empty(&raw["name"], &format!("{field}[].name"))?;
            let unit = raw["unit"]
                .as_str()
                .filter(|unit| !unit.is_empty())
                .map(str::to_string);
            let clock_id = raw["clock_id"]
                .as_str()
                .filter(|value| !value.is_empty())
                .unwrap_or(default_clock_id)
                .to_string();
            if !domains.iter().any(|domain| domain.clock_id == clock_id) {
                return Err(CoreError::invalid(format!(
                    "{field} {metric_id} references unknown clock domain {clock_id}"
                )));
            }
            let declared_entity = match raw.get("entity_id") {
                None | Some(Value::Null) => None,
                Some(value) => Some(non_empty(value, &format!("{field}[].entity_id"))?),
            };
            let attributes = raw.get("attributes").cloned().unwrap_or_else(|| json!({}));
            if !attributes.is_object() {
                return Err(CoreError::invalid(format!(
                    "{field}[].attributes must be an object"
                )));
            }
            let samples = raw[samples_field]
                .as_array()
                .ok_or_else(|| CoreError::invalid(format!("{field}[].{samples_field} must be an array")))?;
            if samples.is_empty() {
                return Err(CoreError::invalid(format!(
                    "{field} {metric_id} declares no {samples_field}"
                )));
            }
            total_samples = total_samples.saturating_add(samples.len());
            if total_samples > MAX_METRIC_SAMPLES {
                return Err(CoreError::invalid(format!(
                    "resource_exhausted: more than {MAX_METRIC_SAMPLES} metric samples"
                )));
            }
            let mut parsed = Vec::with_capacity(samples.len());
            for sample in samples {
                let (start_ns, end_ns, value) = if kind == "counter" {
                    let timestamp = decimal_text(
                        &format!("{field}[].samples[].timestamp_ns"),
                        &sample["timestamp_ns"],
                    )?;
                    let value = numeric_metric_value(
                        &format!("{field}[].samples[].value"),
                        &sample["value"],
                    )?;
                    (timestamp, None, value)
                } else {
                    let start = decimal_text(&format!("{field}[].intervals[].start_ns"), &sample["start_ns"])?;
                    let end = decimal_text(&format!("{field}[].intervals[].end_ns"), &sample["end_ns"])?;
                    let start_value = require_decimal("start_ns", &start)?;
                    let end_value = require_decimal("end_ns", &end)?;
                    if end_value < start_value {
                        return Err(CoreError::invalid(format!(
                            "{field} {metric_id} has an interval that ends before it starts"
                        )));
                    }
                    let value = state_metric_value(
                        &format!("{field}[].intervals[].value"),
                        &sample["value"],
                    )?;
                    (start, Some(end), value)
                };
                parsed.push(RawMetricSample {
                    start_ns,
                    end_ns,
                    value,
                    process_id: optional_string(&sample["process_id"]),
                    thread_id: optional_string(&sample["thread_id"]),
                });
            }
            metrics.push(RawMetric {
                metric_id,
                kind,
                name,
                unit,
                clock_id,
                declared_entity,
                attributes,
                samples: parsed,
            });
        }
    }
    Ok(metrics)
}

fn resolve_metric_mappings(
    store: &Store,
    request: &RuntimeImport<'_>,
    metrics: &[RawMetric],
    probes: &HashMap<String, String>,
) -> MetricReport {
    let mut report = MetricReport::default();
    for metric in metrics {
        if metric.kind == "counter" {
            report.counters += 1;
        } else {
            report.states += 1;
        }
        report.samples += metric.samples.len();
        let attribute_entity = metric
            .attributes
            .get("archmeld.entity_id")
            .and_then(Value::as_str)
            .filter(|value| !value.is_empty());
        let mut entity_id = metric.declared_entity.clone();
        if let Some(attribute_entity) = attribute_entity {
            match &entity_id {
                Some(declared) if declared != attribute_entity => {
                    report.conflicts += 1;
                    push_sample(
                        &mut report.missing_samples,
                        json!({
                            "metric_id": metric.metric_id,
                            "reason": "metric_entity_declarations_disagree",
                            "entity_id": declared,
                            "attribute_entity_id": attribute_entity,
                        }),
                    );
                    continue;
                }
                Some(_) => {}
                None => entity_id = Some(attribute_entity.to_string()),
            }
        }
        let probe = metric
            .attributes
            .get("archmeld.probe_id")
            .and_then(Value::as_str)
            .filter(|value| !value.is_empty());
        if let Some(probe) = probe {
            match probes.get(probe) {
                Some(mapped) => {
                    if let Some(direct) = &entity_id {
                        if direct != mapped {
                            report.conflicts += 1;
                            push_sample(
                                &mut report.missing_samples,
                                json!({
                                    "metric_id": metric.metric_id,
                                    "reason": "entity_id_and_probe_disagree",
                                    "entity_id": direct,
                                    "probe_entity_id": mapped,
                                }),
                            );
                            continue;
                        }
                    } else {
                        entity_id = Some(mapped.clone());
                    }
                }
                None => {
                    report.unknown_probe += 1;
                    push_sample(
                        &mut report.missing_samples,
                        json!({
                            "metric_id": metric.metric_id,
                            "reason": "unknown_probe",
                            "probe_id": probe,
                        }),
                    );
                    continue;
                }
            }
        }
        let Some(entity_id) = entity_id else {
            // Process-scope metrics are complete observations; no source
            // entity is inferred from their name.
            report.process_scope += 1;
            continue;
        };
        let Some(snapshot_id) = request.snapshot_id else {
            report.unknown_entity += 1;
            push_sample(
                &mut report.missing_samples,
                json!({
                    "metric_id": metric.metric_id,
                    "reason": "trace_has_no_bound_snapshot",
                }),
            );
            continue;
        };
        if validate::require_id("archmeld.entity_id", &entity_id).is_err() {
            report.unknown_entity += 1;
            push_sample(
                &mut report.missing_samples,
                json!({
                    "metric_id": metric.metric_id,
                    "reason": "invalid_entity_id",
                }),
            );
            continue;
        }
        let exists: Option<String> = store
            .conn
            .query_row(
                "SELECT entity_id FROM entities
                 WHERE project_id=?1 AND snapshot_id=?2 AND entity_id=?3",
                (request.project_id, snapshot_id, &entity_id),
                |row| row.get(0),
            )
            .ok();
        if exists.is_none() {
            report.unknown_entity += 1;
            push_sample(
                &mut report.missing_samples,
                json!({
                    "metric_id": metric.metric_id,
                    "reason": "unknown_entity",
                    "entity_id": entity_id,
                }),
            );
            continue;
        }
        report.mapped += 1;
        report.resolved.insert(metric.metric_id.clone(), entity_id);
    }
    report
}

fn require_metric_id(value: &str) -> Result<()> {
    let bytes = value.as_bytes();
    let ok = (1..=160).contains(&bytes.len())
        && bytes[0].is_ascii_alphanumeric()
        && bytes
            .iter()
            .all(|b| b.is_ascii_alphanumeric() || matches!(*b, b'_' | b'.' | b':' | b'-'));
    if !ok {
        return Err(CoreError::invalid(format!(
            "metric id {value:?} is not a valid metric identifier"
        )));
    }
    Ok(())
}

fn decimal_text(field: &str, value: &Value) -> Result<String> {
    let text = value
        .as_str()
        .ok_or_else(|| CoreError::invalid(format!("{field} must be a decimal string, not a number")))?;
    require_decimal(field, text)?;
    Ok(text.to_string())
}

fn optional_string(value: &Value) -> Option<String> {
    value.as_str().filter(|text| !text.is_empty()).map(str::to_string)
}

fn numeric_metric_value(field: &str, value: &Value) -> Result<Value> {
    match value {
        Value::Number(number) => {
            let finite = number.as_f64().is_some_and(f64::is_finite);
            if !finite {
                return Err(CoreError::invalid(format!(
                    "{field} must be a finite number"
                )));
            }
            Ok(value.clone())
        }
        _ => Err(CoreError::invalid(format!(
            "{field} must be a finite JSON number"
        ))),
    }
}

fn state_metric_value(field: &str, value: &Value) -> Result<Value> {
    match value {
        Value::String(text)
            if !text.is_empty() && text.chars().count() <= MAX_STATE_VALUE =>
        {
            Ok(value.clone())
        }
        Value::Number(_) => numeric_metric_value(field, value),
        _ => Err(CoreError::invalid(format!(
            "{field} must be a non-empty string up to {MAX_STATE_VALUE} characters or a finite number"
        ))),
    }
}

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
