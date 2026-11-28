# ADR-068: MCP status paging, brief counts and review-report lifecycle

Status: accepted for implementation (2026-11-28).

## Context

The Palimpsest closure slice added bounded briefs, structural diff, rule
exceptions, provenance and persisted review reports. An audit found four
remaining contract gaps: MCP status could page `changed`/`deleted` only via the
`added` cursor, brief recipes disagreed on `counts`, review reports had no
delete path or GC root, and provenance used `false`/`null` for git states it
could not know.

## Decision

1. `archmeld_status` summary uses a versioned cursor bound to project, base
   snapshot, workspace content digest and page size. `changed`, `added` and
   `deleted` page independently; each reports `*_total`, `*_returned`,
   `*_truncated` and `*_next_cursor`. The combined cursor advances all three.
   A stale cursor (different project/snapshot/workspace/page size) is rejected
   instead of skipping or repeating files. Auxiliary freshness arrays
   (moved/omitted/affected entities/relations/claims/architectures/plans/
   changesets) are compacted to a count plus 20 items.
2. Brief recipes share one count vocabulary: `confirmed`
   (declared/resolved/observed), `candidate`, `unknown` (open questions),
   `stale`, `findings`, `context`, `gaps`. `declared` remains an alias of
   `confirmed` for older clients. Every finding carries
   `confidence: confirmed|candidate`. `map` applies `max_findings`/`max_gaps`,
   reports invalid-map errors as findings and uncovered files as gaps, and
   never returns `counts.findings > 0` with an empty array. Project, change and
   map briefs all expose `coverage`, `verification`, `context`, `gaps` and
   `budget`.
3. Review reports become first-class evidence objects:
   - `archmeld review-report list|delete`;
   - report id/identity covers base, candidate, mode and stable output;
   - reads verify both the output digest and the identity columns;
   - persist writes the report and its snapshot pins in one transaction;
   - the size limit is 8 MiB;
   - delete removes the report, its snapshot pins, and freeze pins that no
     other report still cites;
   - GC treats `blob_digest` values inside any report as roots.
   `archmeld report` does not include review reports in this slice.
4. Workspace provenance distinguishes what it knows: non-git workspaces report
   `git_state=unavailable`, `dirty=null` and a limitation; a repository
   subdirectory reports a subtree tree oid plus `workspace_subdir`; snapshot
   diffs without a workspace report `head_tree_basis=unavailable_no_workspace`.
   Duplicate-content renames are marked `ambiguous_move` instead of being
   paired arbitrarily. The identical-snapshot early return reports
   `structural.paths.coverage=not_run` and `schema_version`.
5. Rule exception keys stay stable across content re-indexing and name the
   assertion/limit for `custom_contract`; `scope_ids` are audit-only and
   documented as such; duplicate rule ids are rejected; a path-search result cap
   is reported separately from the expansion cap.

## Consequences

- A status client can reach every changed file even when only one array is
  large.
- A brief's counts mean the same thing in every recipe, and truncation is an
  explicit open question.
- A report that is readable has its pinned snapshots and frozen bytes; deleting
  it releases what GC may reclaim.
- Unknown git/analyser provenance is visible instead of being reported as a
  clean/current value.

## Evidence

- `crates/archmeld-api/src/mcp.rs` cursor tests.
- `crates/archmeld-core/tests/brief.rs` budget/count/confidence/map tests.
- `crates/archmeld-core/src/evidence/store/mod.rs` report size/tamper/rollback
  tests; `crates/archmeld-core/tests/diff_review.rs` lifecycle and provenance
  tests.
- `crates/archmeld-core/tests/blueprint_rules.rs` exception edge tests.
