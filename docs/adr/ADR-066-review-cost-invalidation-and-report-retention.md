# ADR-066: Review cost, invalidation and report retention

Status: accepted for implementation (2026-11-28).

## Context

Palimpsest measured a cold index, one review, and one subsequent candidate, but
had no evidence about reuse, invalidation or the cost of successive candidates.
ARCHMELD already had `helper_elapsed_ms` and `review_ms` in one brief path, but
the CLI `index`, `refresh` and `review-diff` commands had no total timings,
reuse counters or invalidation reasons, and a review result was not persisted
or pinned.

Indexing is a full structural parse unless ADR-070's project-level persistent
graph is available and IndexState can be resumed. Reporting a full refresh as
"incremental" would be false.

## Decision

1. `index` reports `index_mode` (`full_parse` or `incremental_parse`), plus
   `files_reused`, `files_deleted` and timings for capture, helper, import and
   total. `diff_snapshots` derives `reuse.index_mode` from the index-run
   analyser's recorded mode (`full_parse`, `incremental_parse`) or
   `not_indexed` when there is no analyser.
2. `refresh` reports total timing; `review-diff` reports total timing.
3. `diff_snapshots` reports:
   - `reuse` (unchanged/changed/added/deleted/moved files, unique reused/new
     blobs, `index_mode=full_parse|not_indexed`);
   - `invalidation` derived from config, environment, exclusion/policy,
     analyser and tree identity changes.
4. `review-diff --persist` stores the result in `review_reports` and pins every
   cited snapshot via `retention_pins`. `review-diff --report <id>` reads it
   back. The default remains non-persisting and read-only.
5. Retention treats a snapshot referenced by `review_reports` as retained and
   reports the reason; pinning is explicit and visible.
6. Incremental parsing is implemented separately in ADR-070; it changes the
   parse step only. The export remains a complete snapshot generation.
7. The benchmark protocol is recorded separately; this ADR claims only what the
   implemented counters and tests prove.

## Consequences

- A caller can distinguish a clean reuse, a full re-index and an invalidated
  evidence set without reading logs.
- A persisted review is replayable after temporary artifacts are reclaimed
  because its base/candidate snapshots are pinned and reference-probed.
- Time and reuse numbers are descriptive; they are not a performance guarantee.
- No review result is persisted by MCP; the MCP surface remains read-only.

## Evidence

- `crates/archmeld-core/tests/diff_review.rs`:
  `persisted_review_report_pins_its_base_snapshot`.
- `crates/archmeld-api/src/cli.rs` CLI unit tests.
- Benchmark evidence is recorded separately under `docs/evidence/` when run.
