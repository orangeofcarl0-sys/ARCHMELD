// Copyright 2026 ARCHMELD contributors
// SPDX-License-Identifier: Apache-2.0

//! Runtime metric parsing and entity mapping.

use super::*;

/// Parse optional `counters` and `states`. Samples are explicit observations:
/// a counter sample is a finite number at a timestamp, a state interval is a
/// categorical value between two timestamps. Functional names never select a
/// clock or an entity.
pub(super) fn parse_metrics(
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

pub(super) fn resolve_metric_mappings(
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
