// Copyright 2026 ARCHMELD contributors
// SPDX-License-Identifier: Apache-2.0

//! Runtime trace import, scenario binding and evidence persistence.

use super::*;
use super::causal::{build_causal_report, resolve_mappings};
use super::metrics::{parse_metrics, resolve_metric_mappings};

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
