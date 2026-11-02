# ADR-054: Session status, explicit refresh, and task-level diff review

Status: accepted for the 2026-11-02 cross-review closure.

## Context

A fresh agent could discover the MCP tools and snapshots but had no single
read-only entry that reported the project, latest snapshot, languages and
working-tree freshness. The old snapshot could not be compared with the current
working tree, and "refresh" was only implicit in `archmeld index`.

The review required conclusions bound to the current diff, explicit impact and
confidence, and no replacement of old evidence.

## Decision

1. `archmeld status --state <ledger> [--project <id>] [--workspace <repo>]`
   reports registered projects, the latest snapshot, stored counts, per-language
   entity counts and the allowed/read-only action surface. With `--workspace` it
   calls `analyze_drift` and reports changed/added/deleted files and affected
   entities, claims, architectures, plans and changesets. It creates no snapshot
   and records no drift report.
2. `archmeld review-diff --state --project --workspace [--base-snapshot]`
   compares the current workspace bytes with the frozen snapshot. It reports:
   per-file line changes, base/current digests, base-anchor entities whose line
   ranges overlap, static relation impact, a trigger and confidence label, and
   `verification.state=not_run`. It never executes a repository command.
3. `archmeld refresh` captures and indexes a new snapshot, then calls
   `diff_snapshots` against the previous snapshot. The output carries both ids
   and the file/entity/relation delta; the old snapshot remains addressable.
4. A clean review is not a correctness claim. Binary/oversized files and added
   files without base anchors make coverage `partial` and are named.

## Consequences

- The working-tree review is deterministic and offline, but line overlap is not
  semantic impact and new calls are not inferred.
- Refresh is explicit; nothing silently advances the snapshot used by earlier
  evidence.
- The CLI is the agent entry; the HTTP/MCP surface still exposes query/context/
  source/inspect/capabilities. A future status route can reuse the same core
  logic without changing the review facts.
