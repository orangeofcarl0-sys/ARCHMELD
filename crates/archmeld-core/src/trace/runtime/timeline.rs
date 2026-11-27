// Copyright 2026 ARCHMELD contributors
// SPDX-License-Identifier: Apache-2.0

//! Runtime timeline and summary projections.

use super::*;

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
