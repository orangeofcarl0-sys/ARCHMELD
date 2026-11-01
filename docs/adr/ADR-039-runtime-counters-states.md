# ADR-039: runtime counters and state intervals

Status: accepted for the project testing phase.

## Context

S06 requires runtime state/counter observations in addition to spans, and
requires native Perfetto/Tracy captures to be preserved without copying every
event into the main graph. The normalized trace (`archmeld-trace-v1`) previously
carried only spans, links and causal reports. Probes could measure state and
counters, but had no explicit representation, so projects either dropped them or
would have to encode them as fake spans.

## Decision

1. The trace envelope gains two optional arrays, `counters` and `states`.
   - A counter is `{counter_id, name, unit?, clock_id?, entity_id?, attributes,
     samples:[{timestamp_ns, value, process_id?, thread_id?}]}`.
   - A state is `{state_id, name, clock_id?, entity_id?, attributes,
     intervals:[{start_ns, end_ns, value, process_id?, thread_id?}]}`.
   - Timestamps remain decimal strings, values are finite JSON numbers for
     counters and a bounded string or finite number for state values.
2. Storage uses schema 17: `runtime_metrics` plus `runtime_metric_samples`.
   Counters have no `end_ns`; state intervals do. `runtime_runs.metric_count`
   makes the presence explicit.
3. Metrics are observations only. They never create spans, parent edges, links,
   `observed` relations, or architecture facts. The causal report records
   metric counts and `"causality": "none"`.
4. Attribution is explicit. A metric maps to a source entity only through
   `entity_id`, an explicit `archmeld.entity_id`, or a declared probe id.
   Missing declarations are counted as `process_scope`; a declaration that does
   not resolve is `unknown_entity`/`unknown_probe` and makes coverage partial.
   Metric names never select entities.
5. Adapters:
   - probe JSONL: `event: counter` and `event: state` lines;
   - Chrome Trace JSON: `ph: "C"` counter tracks;
   - Perfetto protobuf: interned names, clock snapshots (union-find
     calibration), slices, counters and `TYPE_STATE` transitions. Open slices
     and the final open state are reported as losses, never closed with an
     invented end.
   - Tracy: the raw `.tracy` artifact is retained in the CAS and referenced from
     `artifact_refs`; zones and plots come from the official
     `tracy-csvexport -u -p` summary. Frame-rate rows are ignored and recorded
     as a loss.
6. Timeline queries return `counters`, `states`, `metric_count` and an explicit
   `metrics_truncated` flag. Each series is capped at 10,000 samples per query
   and series count by the query limit.

## S06 deviations

- S06 asks for Perfetto traces to be queried through a local trace processor.
  The implemented adapter decodes a declared protobuf subset in-process instead.
  It imports only explicit slices/counters/states, not the full event stream, but
  it does not invoke `trace_processor_shell`. An optional trace-processor query
  backend remains a tracked gap.
- Tracy source locations are retained in span attributes, but the VS Code
  jump-to-source action is not wired yet.

## Consequences

- State and counter evidence is queryable and bound to the same snapshot,
  build config and loss metadata as spans.
- Global (process-scope) metrics are represented as complete observations rather
  than being forced onto an arbitrary entity.
- Coverage stays partial whenever a declared source mapping fails to resolve,
  which prevents silent attribution.
- Native Perfetto/Tracy capabilities are bounded and recorded per fixture; no
  claim of full protocol support is made.
