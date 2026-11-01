# ADR-045: Snapshot source jump from a retained Tracy zone

Status: accepted for the project testing phase.

## Context

The Tracy adapter retains each zone's `tracy.src_file` and `tracy.src_line` in
the span attributes, but the workbench had no way to open that location. The
existing `/v1/source` route only reads entity anchors, and the runtime data had
no HTTP surface at all, so a zone could not reach its source line.

## Decision

1. `POST /v1/source/file` reads a line window from one snapshot file:
   `{project_id, snapshot_id, path, line?, context_lines?, max_bytes?}`.
   The path is a validated relative POSIX path resolved only against the
   snapshot `files` table and CAS blob. It never reads the current workspace,
   and the response includes the recorded blob digest and the actual window.
2. `GET /v1/runtime/{run_id}` returns the existing runtime timeline, and
   `GET /v1/runtime/{run_id}/summary` returns the bounded summary. Both require
   `project_id`; `limit` is bounded by the core timeline cap.
3. The VS Code command `archmeld.openTraceFrame` takes `{runId, spanId?,
   spanIndex?}`. Without a span selector it lists only zones that retain a Tracy
   source location. It fetches the excerpt, opens it in the read-only
   `archmeld-snapshot-file` virtual document, places the cursor on the exact
   line, and shows the historical snapshot id in the status bar.
4. Absent or non-numeric source attributes are skipped; a run with no located
   zones fails with an explicit message. The command never guesses a location
   from a zone name or a nearby symbol.

## Evidence

- Fixture `docs/evidence/ui-trace-state`: one snapshot of
  `docs/evidence/ui-trace-repo` and one run imported from the pinned
  `tracy-zones-plots.tracy` capture. Both zones retain `tracy_probe.cc:14` and
  `tracy_probe.cc:19`; the fixture generator asserts the snapshot excerpt
  contains the zone lines.
- Real extension-host run: `archmeld.openTraceFrame` returned in 164 ms, opened
  the snapshot document at `tracy_probe.cc:14`, placed the cursor on the
  expected excerpt line, and the CDP screenshot shows
  `ZoneScopedN("stage.queue");` selected.
- `docs/release-evidence/B016-trace-source-jump.json`, raw result
  `docs/evidence/ui-result-b016-trace-jump.json`, screenshot
  `docs/evidence/b016-trace-source-jump.png`, commands
  `#b016-trace-ui-fixture` and `#b016-trace-jump-host`.
- Core tests: `crates/archmeld-core/tests/source_file_api.rs` (3 cases) cover
  line accuracy, snapshot binding, traversal/missing path rejection and
  truncation. Extension unit tests cover the new client routes.

## Consequences

- A Tracy zone is now navigable to the exact historical snapshot line, with no
  entity mapping required and no workspace read.
- The source-file route is a general snapshot-file reader; callers must still
  have the snapshot id. It does not expose current working-tree files.
- Tracy native protocol decoding and trace-processor queries remain out of
  scope; only the retained CSV summary and its source columns are used.
