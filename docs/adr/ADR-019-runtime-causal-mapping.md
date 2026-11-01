# ADR-019 · Runtime causal mapping, clock domains and scenario cancellation

Status: Accepted for implementation; pack trace schema revision pending.

## Context

S06 requires observed causal edges only when a trace carries an explicit
`parent_span_id`, flow/message/task id or link, and forbids treating temporal
proximity as causality. AT036 additionally requires a producer/consumer handoff
with a correlation id, AT037 requires cancellation to clean child processes
without marking the run complete, and AT074 requires preserving multiple clock
domains without claiming a false total order. The 1.0 pack describes these
objects but does not version the concrete trace and scenario manifest fields.

## Decision

The normalized trace imported by `archmeld-core::runtime` accepts these optional
v1.0 extensions:

- `probes: [{probe_id, entity_id}]` and, per span,
  `attributes.archmeld.probe_id` or `attributes.archmeld.entity_id`;
- `links: [{link_id, source_span_id, target_span_id, kind, correlation_id,
  attributes}]` and per-span `links` whose owner span is the source;
- `clock.domains: [{clock_id, kind, unit, origin_ns, calibrated,
  uncertainty_ns, wall_clock}]` and per-span `clock_id`;
- optional `scenario_id`.

Rules:

1. A span→entity mapping is accepted only when the entity exists in the same
   snapshot and the mapping is explicit. Name or symbol similarity is never
   used. Unknown/absent mappings remain events but are reported in
   `causal.missing_association` and the evidence coverage.
2. `parent_span_id` produces `observed_call`; `call` links produce
   `observed_call`; `message`/`flow`/`link` produce `communicates_with`;
   `await`, `schedule`, `publish` and `consume` map to their relation kinds.
   `message`/`flow` links must carry a non-empty `correlation_id`.
3. Observed relations are new rows with `basis=runtime`,
   `resolution=observed` and a runtime evidence record. Existing
   `static_call_candidate` rows are never overwritten or promoted.
4. An unbound trace may be browsed and stored, but it cannot create observed
   relations or verify a ChangeSet. Its runtime evidence freshness is
   `unbound`.
5. More than one clock domain forces `clock.total_order=false`; spans are
   grouped by declared domain and sorted only within it. Cross-domain links
   expose `cross_clock=true` and the summed declared uncertainty. Missing
   uncertainty is `null`, never zero.
6. A scenario run reports `completion` (`succeeded` means only process success),
   records the observed process tree, and on cancel/timeout terminates the live
   tree. Survivors after cleanup are listed; a cancelled run is `partial` and
   never `complete`. A reparented process found after a normal exit is surfaced
   as an orphan candidate and is not killed by PID because of reuse risk.
7. Capture overhead is `unknown` unless a capture adapter measured it.

Schema version 8 adds `runtime_links`, `runtime_runs.clock_domains_json`,
`runtime_runs.causal_json` and `runtime_spans.clock_id`.

## Consequences

- `import_trace` returns causal coverage and the evidence provenance in the
  same transaction as the runtime rows.
- The pack should absorb these fields into the next trace/manifest contract
  revision; until then this ADR and the tests are the traceable deviation.
- The scenario runner does not yet integrate VizTracer/OTLP/Perfetto/Tracy
  adapters; overhead remains `unknown` for those runs.
