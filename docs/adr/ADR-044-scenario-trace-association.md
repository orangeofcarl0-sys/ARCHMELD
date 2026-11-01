# ADR-044: Explicit scenario-run association for trace imports

Status: accepted for the project testing phase.

## Context

Normalized traces carry an optional `scenario_id`, and adapters can set it from
producer metadata. The importer stored that string in the runtime evidence but
never checked that a scenario run with that id existed, nor that its snapshot and
build config matched. A trace could therefore claim an association that the
ledger could not substantiate, and there was no way to tell an execution-linked
trace from an annotation.

## Decision

1. Schema 18 adds `runtime_runs.scenario_run_id`. It is populated only for a
   verified association.
2. `archmeld trace-import` gains `--scenario-run-id <id>` and `RuntimeImport`
   gains `scenario_run_id`. When supplied, the importer requires:
   - the id exists in `scenario_runs` for the same project;
   - the scenario run's snapshot equals the import snapshot when one is given;
   - the scenario run's config digest equals the import config digest;
   - an embedded `trace.scenario_id`, if present, is identical.
   Any mismatch is rejected; nothing is written.
3. A verified association is recorded in the runtime evidence as
   `scenario: {run_id, snapshot_id, config_digest, exit_code, timed_out,
   association: "explicit: ..."}` and in the timeline/summary as
   `scenario_run_id`.
4. Without `--scenario-run-id`, an embedded `scenario_id` remains
   producer-declared metadata. It is recorded with
   `association: "producer_declared_unverified"`, the evidence coverage gains an
   explicit limitation, and `scenario_run_id` stays null. It never becomes a
   link to a run.
5. Re-importing the same trace artifact with a different scenario association
   is a conflict; the original association is not silently replaced.

## Evidence

- `docs/release-evidence/B016-scenario-trace-link.json`: a real scenario run
  (`python -c print`) linked to a normalized trace; the timeline and summary
  report `run.link.ok`, and the evidence records exit code 0 and the matching
  snapshot.
- Missing, mismatched and different-snapshot scenario runs are rejected.
- A producer-declared `scenario_id` is recorded as unverified with an explicit
  coverage limitation and no `scenario_run_id`.
- CLI coverage: `--scenario-run-id` succeeds for a real run and fails for an
  absent id.
- Tests: `crates/archmeld-core/tests/scenario_trace_link.rs` (4 cases),
  `crates/archmeld-api/tests/trace_import_cli.rs` (association case).

## Consequences

- A trace's scenario link is now a verifiable relationship or explicitly
  unverified metadata, never an unchecked string.
- Callers must run the scenario before importing its trace; an import cannot
  create a scenario record.
- The association is not a causal edge. It does not change span mapping,
  observed relations or static candidates.
