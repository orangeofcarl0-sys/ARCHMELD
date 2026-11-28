# ADR-062: Change brief findings, context and output budget

Status: accepted for implementation (2026-11-28).

## Context

Palimpsest's PR #311 review found that the change brief described `context_only`
entities as changed, produced 51 findings for 31 affected entities, and required
a second pass to decide what mattered. The implementation also capped findings
at 50 before filtering state, so unchanged entities could crowd out changed
ones, and it promoted any incoming relation to `high` even when the relation was
a static candidate.

ADR-056 defined the brief as a one-screen composition of status and review
facts. It did not define what belongs in `findings`, how omitted items are
reported, or how callers are represented.

## Decision

1. `findings` contains only entities whose review state is `changed` or
   `deleted`. `context_only` entities never enter `findings`. An added file has
   no base entity, so it is represented as a `gaps` entry, not a synthetic
   entity finding.
2. Unchanged context is a separate `context` array. It includes:
   - entities in a changed file whose anchor was not touched, and
   - relation-derived callers whose target changed.
   Each entry cites the base snapshot and, where available, the relation.
3. Files without a complete review are separate per-file `gaps` entries with
   `kind`, `path` and `reason`. They are not collapsed into one anonymous
   finding.
4. Severity is deterministic and resolution-aware:
   - confirmed incoming (`resolved`, `declared`, `observed`) = `high`;
   - candidate incoming = `info` with `confidence=candidate`;
   - deleted = `low`, otherwise `info`; an added file is a medium gap.
   This narrows the ADR-056 mapping to changed entities.
5. `archmeld_brief` has bounded `max_findings`, `max_context`, `max_gaps` and
   `max_relation_items`. The default is 20/10/10/10. `budget` reports returned
   and omitted counts; `counts.findings` is the pre-truncation total.
6. Every `next_action` that cites evidence carries the brief's `snapshot_id`,
   so drill-down is bound to the same snapshot even after a refresh.
7. `coverage` and `verification` are top-level fields. Truncation adds explicit
   `unknowns` entries; it does not silently drop coverage.

## Consequences

- This is an intentional output-semantics change. Existing consumers that
  counted `findings` as "all entities in changed files" must read `findings`
  (changed only) plus `context` and `gaps`.
- The MCP input schema is frozen. The new budget fields are advertised by the
  runtime `tools_list()` override and recorded in
  `contracts/archmeld/mcp-brief-budget.patch.json` until the next pack
  revision.
- The brief still adds no evidence, infers no calls and executes nothing.

## Evidence

- `crates/archmeld-core/tests/brief.rs`:
  `change_brief_separates_changed_findings_from_unchanged_callers`,
  `change_brief_reports_unmapped_files_as_named_gaps`,
  `change_brief_budget_reports_omitted_items_without_losing_coverage`.
- MCP handshake: `crates/archmeld-api/tests/mcp_stdio.rs` verifies the
  advertised budget fields; handler budget behavior is covered by the core
  brief tests above.
