# ADR-059: Evidence model 1.1 and pack re-seal

Status: accepted for implementation (2026-11-26). This ADR authorizes the
contract revision that absorbs the P01–P25 self-analysis repairs. It is the
first deliberate re-seal of `docs/archmeld` after the sealed pack drifted
without a matching `PACK_MANIFEST.json`/`SHA256SUMS` update.

## Context

The 2026-11-26 self-analysis (`docs/reviews/ARCHMELD-SELF-ANALYSIS-PROBLEMS-2026-11-26.md`)
found five shared root causes: the bridge is a lossy normalizer, capture and
drift use different workspace policies, ledger identity is not bound to VCS or
the producing build, the read model has no role/confidence/multiplicity
dimensions, and the context pack has no priority model.

The existing contracts cannot express the repairs:

- `Entity` has no `role`, so tests, config, generated and vendored code cannot
  be filtered from architecture queries.
- `Relation` drops upstream `resolved_by`/`is_direct`, has no call-site line,
  and collapses duplicate call sites without an occurrence count.
- `FileEntry` has no git blob, so two checkouts of the same commit with
  different line endings produce different snapshot evidence.
- `Snapshot` has no workspace policy, so drift cannot apply the capture policy.
- `QueryRequest` has no role/kind/confidence filters.
- MCP `status`/`review` return full arrays with no detail or cursor.
- The sealed pack already disagrees with three files:
  `contracts/mcp-tools.json`, `contracts/schemas/archmeld.schema.json` and
  `specs/S11_API_MCP_EVENTS.md`.

## Decision

1. Publish contract **1.1** as an additive revision. Old payloads remain valid:
   new fields are optional/defaulted; the JSON major stays `1`.
2. Add:
   - `Entity.role`: `source|test|config|generated|protocol|vendored|external|other`.
   - `Relation.call_site{path,line,column?}`, `occurrences>=1`,
     `producer_resolution{name_match|cross_file_resolution|direct|declared}`,
     `evidence_scope{artifact|site}`.
   - `FileEntry.git_blob` (tracked and clean only) and
     `Snapshot.git_tree_digest` (git content identity, separate from the
     byte-level `snapshot_id`).
   - `Snapshot.workspace_policy` so drift applies the capture policy.
   - `IndexCoverage` (offered/parsed/skipped-by-reason/parser errors) and
     `Snapshot.files_origin`.
   - `QueryRequest.entity_kinds`, `entity_roles`, `min_producer_resolution`,
     `require_call_site`.
   - MCP `status`/`review` `detail`, `max_files`, `cursor`.
3. `archmeld-export-v1` schema 1.2 adds an optional manifest `index` object
   with parse/skip counters and per-file parser errors. Consumers that see 1.1
   keep working; unknown manifest fields stay ignored.
4. Re-seal: update the three drifted files, add the new definitions, then
   regenerate `PACK_MANIFEST.json` and `SHA256SUMS` with
   `tooling/reseal_pack.py --write`. `tooling/upstream.py verify` must report
   zero sealed-pack digest mismatches afterwards.
5. Candidate relations never become `resolved_call`. `producer_resolution`
   records only what upstream claimed; it is not a promotion.
6. Cross-file `Implements`: when a Rust impl block is mapped before its trait
   file, the mapper records the trait name on the implementor
   (`unresolved_implements`). After the workspace is parsed,
   `resolve_pending_implements` adds `Implements` only when the name matches
   exactly one `Interface`; multiple matches stay pending and are counted as
   `ambiguous`. File order must not change the graph.
7. The 1.2 manifest `index` block is the ledger source of truth for
   parse/skip counters and parser-error paths. The helper's stdout summary is
   a legacy fallback for 1.1 producers only; it never overrides the manifest.
8. Query exclusion is additive: `exclude_entity_roles` and
   `exclude_entity_kinds` remove a relation when either endpoint matches. The
   historical `entity_roles`/`entity_kinds` inclusion semantics (both endpoints)
   are unchanged. This lets impact/neighbors exclude test, vendored or config
   entities without inventing a second query language.
9. Relation filters (`relation_kinds`, `min_producer_resolution`,
   `require_call_site`) are traversal-only. Passing them to symbols/text/source
   is an explicit `invalid` error, not a silent no-op. Source queries accept
   `entity_kinds=["file"]` only and filter files by role.
10. `archmeld init` and `archmeld refresh` accept the same include/exclude/size/
    depth/tracked-only options as `archmeld index`; refresh no longer silently
    drops the index policy. A committed map is authored against a snapshot N and
    may appear as `added` until the next re-baseline; it is never rebound to a
    snapshot that does not exist.

## Consequences

- Snapshot IDs change for new captures because `config_digest` includes the
  product build and `workspace_policy`; old snapshots remain readable.
- Query cursors created before 1.1 are invalid because the request digest
  includes the new fields. This is intentional and fails closed.
- MCP `status`/`review` default to `detail=summary`; clients that need the old
  full response pass `detail=full`.
- The pack revision is a single auditable commit; future deviations must use a
  patch file or a new ADR, not a silent edit.

## Verification

- `crates/archmeld-core/src/store/mod.rs` migration test upgrades a v18 ledger.
- `context_relevance`/`context_pack` cover explicit seed retention.
- `drift_recovery`/`diff_review` cover ignored-only clean trees and one
  `needs_review` definition.
- `bridge_import` covers call-site/occurrence/producer-resolution preservation.
- `snapshot_capture` covers LF/CRLF `git_tree_digest` equivalence.
- `mcp_stdio` covers `detail`/`max_files`/`cursor`.
- `tooling/upstream.py verify --skip-cargo` reports zero sealed-pack mismatches.
