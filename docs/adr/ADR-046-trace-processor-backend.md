# ADR-046: Optional Perfetto trace-processor query backend

Status: accepted for the project testing phase.

## Context

S06 expects Perfetto native traces to be queried by a local trace processor. The
default import path decodes a protobuf subset in process, which is precise for
slices, counters and states but is not the trace processor. The remaining B016
item asks for an optional query backend with a pinned tool identity; it must not
replace the in-process decoder or silently claim more coverage than it has.

## Decision

1. `archmeld trace-import --format perfetto --trace-processor <path>` runs the
   pinned `trace_processor_shell` with two explicit SQL queries:
   - slices: `ts, dur, name, depth, track_id, track name/type, category`;
   - counters: `ts, value, track_id, counter track name/unit`.
   The default path stays the in-repo decoder.
2. Mapping: each slice becomes a span. A parent is assigned only when the
   previous slice at `depth - 1` on the same track is still active; no parent is
   inferred from names or timestamps alone. Counter tracks become counter
   series. State tracks, flows, args and process/thread names are not imported.
3. Unfinished slices (duration < 0) and state counter tracks are skipped with an
   explicit loss note; the import is marked lossy.
4. The backend records the tool path, full version string and SHA-256 of the
   binary in the CLI result and in `adapters/traces/fixtures.lock.json`. The
   binary is extracted from the already pinned Perfetto v59.1 archive.
5. The trace processor's stdout uses the bounded command capture. A result over
   4 MiB fails explicitly instead of importing truncated CSV.
6. SQL output is parsed by a small quoted-CSV reader; an unterminated quote is
   an error, never a partial row.

## Evidence

- Real CLI run on `perfetto-counter-state.pftrace`:
  `trace_processor_shell` v59.1 SHA-256 `sha256:3acc5aca…`, 2 slice rows and
  4 counter rows; the timeline has 2 spans (parent `slice.1` for
  `slice.2`) and one counter series with values `[0, 3, 1, 0]`.
- The core test compares the SQL backend with the in-process decoder on the same
  fixture: identical slice names and four counter samples.
- `docs/release-evidence/B016-trace-processor.json`, result
  `docs/evidence/b016-trace-processor-result.json`, commands
  `#b016-trace-processor-cli`, `#b016-trace-processor-tests`,
  `#b016-trace-processor-cli-tests`.
- Tests: `crates/archmeld-core/tests/perfetto_trace_processor.rs`,
  `crates/archmeld-api/tests/trace_import_cli.rs` (gated on
  `ARCHMELD_TRACE_PROCESSOR`).

## Consequences

- S06's trace-processor expectation is now available as an explicit opt-in with
  a pinned tool identity, while the default remains deterministic and
  dependency-free.
- The SQL backend has narrower semantics than the in-process decoder (no state
  intervals, flows or debug annotations); the loss notes and evidence state
  that explicitly.
- Large query results are rejected rather than truncated; callers must narrow
  queries or extend the capture limit in a future revision.
