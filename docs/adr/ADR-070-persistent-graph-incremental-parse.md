# ADR-070: Project-level persistent graph and stable materialization (I1)

Status: accepted for implementation (2026-11-28).

## Context

`archmeld index` captured a snapshot and materialized it into a fresh
`index-sandboxes/<project>/<run>` directory per invocation. The helper built an
in-memory CodeGraph, so every run parsed every file even though
`IndexState` hashes were written to disk: the absolute paths in the hash map
changed with the random run directory, and a new process never called
`IndexState::load()`. ADR-036 and ADR-066 therefore stated, correctly, that
indexing was a full parse and that "incremental" would be a false claim.

## Decision

1. Materialization moves to a stable per-project root
   `<state>/index-work/<project>`. A run materializes into
   `<state>/index-work-tmp/<project>`, replaces the stable root, and only then
   runs the helper. IndexState keys are therefore the same absolute paths on
   every run.
2. The helper opens a project-level persistent CodeGraph at
   `<state>/index-graph/<key>` (key = digest of the stable materialization
   path), loads the previous IndexState, and lets the existing indexer skip
   files whose content hash is unchanged. Unchanged nodes stay in the graph.
3. Before export the helper scans IndexState for paths that no longer exist in
   the workspace, deletes their nodes and edges, and removes the hashes. The
   export therefore describes the current workspace, not a union of every run.
4. The export manifest gains additive fields, not a new line type or format:
   `index_mode` (`incremental_parse` or `full_parse`), `files_reused`,
   `files_deleted`, `storage` and `state_loaded`. `archmeld index` and
   `refresh` surface `index_mode`, `files_reused` and `files_deleted`.
5. If the persistent graph cannot be opened (lock, corrupt directory) or no
   graph is requested, the helper falls back to an in-memory graph, **clears**
   IndexState so unchanged files are not skipped against an empty graph, and
   reports `storage=in_memory_fallback` with `index_mode=full_parse` and the
   storage error. A failed persistent open must not leave a half graph.
6. The export and publish remain full: every run writes the complete graph to
   NDJSON and imports a complete snapshot generation. I1 is incremental
   *parsing*, not a delta ledger. I2 (a persisted delta format) remains out of
   scope.

## Consequences

- A second run after a one-file edit parses only that file; the exported node
  set is identical to a full parse (verified by comparing node keys).
- Deleted and renamed files disappear from the export; old paths are not left
  behind by the persistent graph.
- `IndexState` and the graph are still derived caches. The ledger snapshot and
  CAS remain the authority; deleting `<state>/index-work` or
  `<state>/index-graph` causes a safe full re-parse.
- The stable work directory contains the last materialized snapshot until the
  next run replaces it. It is inside the ledger state root, not the user's
  workspace, and is not a second source of truth.

## Evidence

- `crates/archmeld-api/src/cli.rs`:
  `index_parses_the_captured_snapshot_not_a_concurrent_save`,
  `index_reports_incremental_mode_reuse_and_deletions_from_the_helper`.
- Two-run/delete/rename/fallback command records under
  `docs/evidence/i1-incremental-2026-11-28/` (generated from the real helper).
