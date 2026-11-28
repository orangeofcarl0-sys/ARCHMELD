# Self-architecture audit — 2026-11-28

Clean self-analysis at HEAD `e3f9ad2b` (snapshot `sha256:df7eb174…143f15`) plus
static inspection of `crates/archmeld-core/src` and `crates/archmeld-api/src`.
Evidence: `docs/evidence/self-analysis-2026-11-28/`.

**Verdict:** no known functional failure at HEAD (24/24 acceptance, fast lane,
B031/AT067 pass). The debt is structural: a shared `Store` kernel with leaked
SQL, two monolithic entry-point files, several very large functions, duplicated
helpers and a pervasively untyped JSON interface. The Rust module import graph
has **no cycle**, so this is coupling/complexity, not a cyclic architecture.

## Measurements

| Metric | Value |
|---|---|
| core+API source | 51,258 lines in 69 files (average ~743 lines/file) |
| files ≥800 lines / ≥1,200 lines | 26 / 15 |
| entities / relations in self snapshot | 11,286 / 47,531 |
| `Store` public methods | 44 in `store/mod.rs` plus impls in other files |
| SQL calls outside `evidence/store/` | 87 calls in 19 files |
| function signatures containing `Value` | 233 |
| production `unwrap/expect` (tests excluded) | 67 |
| `#[allow(clippy::too_many_arguments)]` | 6 |
| duplicate `short_hash` definitions | 8 |
| CLI / API entry files | `cli.rs` 3,054 lines / 48 commands; `lib.rs` 1,862 lines / 57 handlers |

## Findings

### F1 (high): `Store` is a god object and schema knowledge leaks

`evidence/store/mod.rs` is 1,567 lines with 44 public methods; `query.rs`
adds another 1,521 lines of `impl Store`, `import.rs` 1,000 lines. Outside the
store module there are 87 direct `store.conn.prepare/execute/query` calls in 19
files, led by `ops/drift.rs` (14), `change/changeset/mod.rs` (8),
`ops/instigator.rs` (8), `trace/runtime/timeline.rs` (8), `change/apply.rs`
(7), `change/verification.rs` (7). A schema or transaction change therefore
requires edits across persistence, ops, change and analysis.

Recommendation: keep SQL in `Store` and expose typed query methods for these
call sites; do it incrementally by table (relations, snapshots, retention pins,
verification rows), not as a rewrite.

### F2 (high): monolithic entry points

`archmeld-api/src/cli.rs` contains 48 commands and ~3,054 lines;
`archmeld-api/src/lib.rs` contains the router, auth, 57 handlers and lifecycle
logic in ~1,862 lines. Both mix argument parsing, policy checks, persistence,
JSON shaping and error formatting (`cli.rs` has 71
`map_err(|error| error.to_string())` calls).

Recommendation: split `cli.rs` into `commands/<domain>.rs` behind a small
dispatch enum, and `api/lib.rs` into `routes/<domain>.rs` with one handler
module per resource. Keep the public command/route names unchanged.

### F3 (medium): the core error type lives under `surface`

`CoreError`/`Result` are defined in `surface/error.rs`, although 42 core files
import `crate::error`. The physical path suggests a layer dependency on
`surface` that does not exist at the Rust import level, and it inflates
call-graph coupling metrics (`evidence → surface`).

Recommendation: move the file to `src/error.rs`, keep the root re-export, and
update the module list. This is mechanical and low-risk.

### F4 (medium): duplicated identity/diff helpers

There are eight `short_hash` implementations in core (plus a variant in
`human.rs`) and two content-tree digest implementations: inline in
`ops/drift.rs:285` and `surface/review.rs:379`. Workspace review, snapshot diff
and drift also rebuild file/entity summaries independently. This class of
duplication already produced real divergences (omitted-vs-deleted, ambiguous
moves, report identity), so it is a correctness risk, not only style.

Recommendation: one `hash::short` helper; one `content_tree_digest` function
shared by drift/review; eventually one file-diff model consumed by all three
surfaces.

### F5 (medium): JSON `Value` is the internal interface

233 production signatures take or return `Value`. Schemas are re-stated in
hand-written extraction code across CLI, MCP, HTTP and core. The recent closure
found three concrete consequences: inconsistent brief counts, cursor/page shape
drift, and report identity that did not cover its columns.

Recommendation: introduce typed request/result structs at the highest-risk
boundaries first — review report identity, status page, brief counts — while
keeping JSON at the MCP/HTTP edge. Do not attempt a blanket `Value` removal.

### F6 (medium): giant functions

Largest core functions (line span from the ledger):

| Lines | Function |
|---:|---|
| 650 | `analysis/context.rs::create_context_pack` |
| 626 | `analysis/blueprint.rs::check_rule` |
| 531 | `surface/brief.rs::change_brief` |
| 442 | `evidence/bridge.rs::import_codegraph_export` |
| 427 | `surface/review.rs::diff_snapshots` |
| 373 | `evidence/store/import.rs::publish_generation` |
| 352 | `trace/runtime/import.rs::import_trace` |
| 348 | `surface/scip.rs::import_scip` |
| 344 | `analysis/cognition_map.rs::check_map` |
| 330 | `change/apply.rs::apply_changeset_with_hook` |

These functions mix validation, graph traversal, persistence and output
shaping. Recommendation: extract stage functions with typed inputs, starting
with `create_context_pack` and `check_rule` (the two largest and most
branch-heavy).

### F7 (medium): panic surface in JSON/import paths

67 production `unwrap`/`expect` calls remain after excluding `#[cfg(test)]`.
The largest concentrations are `analysis/context.rs` (10),
`trace/runtime/import.rs` (8), `analysis/interpret/claims.rs` (8),
`change/predicate/mod.rs` (6), `analysis/interpret/investigations.rs` (6).
Several are invariant unwraps on locally built objects, but
`runtime/import.rs:88` unwraps `trace["clock"]["origin_ns"]` from an imported
trace. These are the first candidates for `?` plus an explicit invalid-input
error when hardening untrusted traces.

### F8 (info): candidate edges are not architecture evidence

The self ledger's `static_call_candidate` graph collapses 61 source files into
one SCC, but inspection shows false positives from name collisions:
`store::put_blob → fs::rename` is attributed to `surface/lsp.rs::rename`, and a
test `drop` to `lsp::drop`. Candidate edges are syntax hypotheses by design.

The Rust `use crate::…` module graph is acyclic (no SCC). Its shape is a hub:
`evidence` is imported by `ops` (18), `change` (17), `surface` (14) and
`analysis` (9) files; one-directional edges `surface → ops` (4),
`change → analysis` (2), `ops → change` (1), `trace → change` (1) exist but do
not cycle. There is one real within-module cycle,
`store/mod.rs ↔ store/validate.rs`, which is benign mutual validation.

Recommendation: if ARCHMELD is used to check its own architecture, use
imports/resolved relations or a manually keyed rule, not candidate calls.

### F9 (low): test and helper complexity

Evidence tests contain 500–850-line functions
(`language_runtime_linkage.rs` 844 lines, `e2e_case_b.rs` 735). They are
valuable acceptance records but hard to diagnose when they fail. Extract the
setup/assert stages, not the assertions.

`memory_limit.rs:114` calls `std::mem::forget` on a `Copy` raw handle; it has no
effect (the handle stays open because it is a raw pointer) and only generates a
compiler warning. Replace it with an explicit `let _ = job;` and a comment.

## Priorities

1. **P0, low risk:** move `error` out of `surface`; dedupe `short_hash` and
   `content_tree_digest`; replace the no-op `forget`; audit the untrusted-trace
   unwraps.
2. **P1:** typed `Store` methods for the top SQL-leak call sites; split
   `cli.rs` and `api/lib.rs` by domain.
3. **P2:** split `create_context_pack`, `check_rule`, `change_brief`,
   `diff_snapshots`; typed DTOs for report/status/brief; one shared file-diff
   model.
4. **P3:** add an ARCHMELD architecture rule for its own module layering using
   import-based relations, with the error module treated as its own layer.

Do not do a wholesale `Value` removal, a rewrite of `Store`, or treat candidate
call edges as proven cycles. The current behavior is accepted; these changes
are debt reduction with regression tests, not a rescue.

## Follow-up (2026-11-28, same day)

The original measurements above were checked line by line. Three corrections
and one new finding:

- There are **8** `short_hash` definitions in core, not 9; `human.rs` is one of
  the eight.
- `cli.rs` has **22 top-level clap commands and 25 leaves** (7 nested leaves),
  not 48. The "48" counted implementation functions, not the command surface.
- "87 SQL calls" is the one-line `store.conn.{prepare,execute,query}` literal
  count. Formatted `.conn` continuations and `tx.*` calls add roughly the same
  again; the ratchet therefore records all three separately.
- New: `surface/scip.rs::Reader::length_delimited` had a reachable panic. A
  varint length near `u64::MAX` overflows `self.pos + length`; debug builds
  panic on the addition, release builds wrap and panic on a reversed slice.
  Fixed with `checked_add` and an explicit error, with unit and import-level
  tests.

Implemented in the same day's P0–P2 slice (ADR-071): the SCIP fix, error module
move, shared short/tree hashes, guard-dependent unwrap removal, typed Store
idempotency/snapshot/revision methods, CLI and HTTP entry-point splits, and
stage splits for `check_rule`, `diff_snapshots`, `change_brief` and
`create_context_pack`.

Final ratchet values: `short_hash` 8→1, content-tree digest 2→1, production
`unwrap/expect` 62→49, idempotency inserts outside `Store` 18→0, one-line
Store SQL 87→71, `tx.*` 70→61, `cli.rs` 3,054→64, `api/lib.rs` 1,862→350,
files ≥800 lines 26→24. Function sizes: `check_rule` 626→32,
`diff_snapshots` 427→106, `create_context_pack` 650→148, `change_brief`
531→228.

`change_brief` remains above the 160-line warning line. The remaining body is output
assembly plus the budget/unknown tail; the collection loops are already
separate. Further splitting would add a wide transport struct without reducing
coupling, so it is deferred with the typed-DTO work rather than forced.
