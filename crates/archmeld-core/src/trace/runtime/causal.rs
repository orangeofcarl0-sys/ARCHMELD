// Copyright 2026 ARCHMELD contributors
// SPDX-License-Identifier: Apache-2.0

//! Causal report construction and mapping resolution.

use super::*;

pub(super) fn build_causal_report(
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

pub(super) fn resolve_mappings(
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
