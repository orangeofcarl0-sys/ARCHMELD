# ADR-071: Internal structure maintenance and the debt ratchet

Status: accepted for implementation (2026-11-28).

## Context

The 2026-11-28 self-architecture audit found no known functional failure but
substantial structural debt: a shared `Store` whose SQL leaked into 19 files,
3,054-line `cli.rs` and 1,862-line `api/lib.rs` entry points, eight copies of
`short_hash`, two content-tree digest implementations, ten 300–650-line
functions and 67 production `unwrap`/`expect` calls. A follow-up inspection
also found a reachable panic in the SCIP length-delimited reader: a crafted
varint lets `self.pos + length` overflow, so debug builds panic on the addition
and release builds panic on a reversed slice range.

The project had just closed its R1 evidence path. A rewrite would put accepted
brief/context/review contracts at risk, so this ADR records a deliberately
incremental debt-reduction policy instead.

## Decision

1. **Behaviour is frozen during the refactor.** Public CLI command names, MCP
   tools, HTTP routes, JSON keys and contract/schema versions do not change.
   A slice that cannot stay behaviour-preserving is stopped, not forced.
2. **Correctness before style.** The SCIP length overflow is fixed first, then
   guard-dependent unwraps on imported input are converted to explicit errors.
   Infallible typed `serde_json` serialization stays as `expect`; it is not
   rewritten for appearance.
3. **One home for shared primitives.** `CoreError`/`Result` move to
   `src/error.rs`; `hash::short_hex`/`short_bytes`/`short_value` own short ids;
   `snapshot::content_tree_digest` owns the tree digest shared by drift and
   review. The public `sandbox::short_hash` remains as a delegating wrapper.
4. **SQL lives behind `Store`.** New code uses typed methods. This slice adds
   `idempotency_put_raw`/`idempotency_put_tx`, `snapshot_file_digests`,
   `snapshot_file_metadata`, `RevisionTable`, `latest_revision_ids`,
   `latest_revision_rows` and `latest_revision_payload`. The remaining direct
   queries are known and counted; `conn` stays `pub(crate)` until they reach
   zero.
5. **Entry points are thin.** `cli.rs` keeps the clap parser, top-level
   `Commands` enum, dispatch and `main`; command arguments and implementations
   live under `src/commands/<domain>.rs`. `api/lib.rs` keeps `ApiState`, the
   route table, auth and shared error/query plumbing; handlers live under
   `src/routes/<resource>.rs`. A clap command-tree test pins the 22 top-level
   commands and 7 nested leaves.
6. **Large functions are split by stage, not rewritten.** `check_rule` becomes
   one function per rule kind; `diff_snapshots` becomes provenance/file/
   entity/relation/structural stages plus a result builder; `change_brief`
   becomes review resolution, entity collection, gap/relation stages;
   `create_context_pack` becomes request parsing, header construction, claim/
   entity/graph packing, budget enforcement, unresolved-question attachment and
   sealing. Each stage keeps the existing JSON shape and tests.
7. **The ratchet is monotonic.** `tooling/check_architecture_debt.py` records
   single-line `store.conn` calls, multiline `.conn` lines, `tx.*` calls,
   idempotency inserts outside `Store`, production `unwrap/expect`, duplicate
   helper definitions, files over 800 lines, and the two entry-point sizes.
   The fast lane fails if any metric grows above
   `docs/assessments/DEBT-BASELINE-2026-11-28.json`; the baseline may only be
   lowered deliberately.

## Consequences

At the end of this slice the measured values are:

| Metric | Before | After |
|---|---:|---:|
| `short_hash` definitions | 8 | 1 (delegating wrapper) |
| content-tree digest definitions | 2 | 1 |
| production `unwrap/expect` | 62 (ratchet count) | 49 |
| `INSERT INTO idempotency` outside `Store` | 18 | 0 |
| single-line `store.conn` outside `Store` | 87 | 71 |
| `tx.*` calls outside `Store` | 70 | 61 |
| `cli.rs` lines | 3,054 | 64 |
| `api/lib.rs` lines | 1,862 | 350 |
| files ≥800 lines | 26 | 24 |
| `check_rule` | 626 | 32 |
| `diff_snapshots` | 427 | 106 |
| `create_context_pack` | 650 | 148 |
| `change_brief` | 531 | 228 |

The 160-line function cap and the ≤60 single-line Store-SQL cap are warning
lines, not hard gates. `change_brief` ends at 228 lines and the remaining
single-line Store queries at 71; both are below the pre-slice values, and the
actual split follows cohesion. The `change_brief` residue is output assembly
plus the budget/unknown tail; the expensive collection loops are already
separate. Splitting that tail further would add a 20-field transport struct
without reducing coupling, so it is deferred rather than forced. The remaining
Store sites are listed in `docs/REMAINING_BLOCKERS.md`.

Deferred with prerequisites:

- typed DTOs for review/status/brief beyond the existing typed keys;
- one shared file-diff model behind drift, workspace review and snapshot diff;
- the six remaining large import/apply/map functions;
- `Store` `conn` privatisation once the remaining 71 single-line and 61
  transaction sites are typed;
- an import-based self-architecture rule (candidate call edges are not
  architecture evidence).

## Evidence

- `tooling/check_architecture_debt.py` and its baseline JSON.
- Fast-lane records `debt-phase1-fast-2026-11-28` through
  `debt-phase4-fast-2026-11-28` in `baseline/commands.ndjson`.
- Targeted suites: `hash_vectors`, `scip_import`, `blueprint_rules`,
  `diff_review`, `context_pack`, `context_relevance`, `brief`, `session_card`,
  `drift_policy`, `drift_recovery`, `changeset_stage`, `apply_recovery`,
  `verification`, and the API HTTP/MCP suites.
