# ADR-061: Core module groups, root re-exports and in-place file splits

Status: accepted for implementation (2026-11-27). This ADR records the T3
structure refactor: visibility narrowing, four in-place file splits, and six
private module groups with root re-exports.

## Context

`archmeld-core` exposes 43 `pub mod`s from one flat `lib.rs` and several files
exceed 1,700 lines (`predicate.rs` 2,867, `runtime.rs` 2,276,
`changeset.rs` 2,060, `interpret.rs` 1,913). Most of the public module names are
also used by `archmeld-api` and by 63 integration-test files, so a move that
changes paths would create hundreds of edits and break third-party readers.

The project also keeps a committed cognition map (`archmeld.map.txt`) whose
`R:file:` references are validated against a snapshot. A physical move is only
safe if the map is regenerated in the same change.

## Decision

### 1. Split first, move second

The four large files are split in place before any directory move:

| Old file | New files |
|---|---|
| `predicate.rs` | `predicate/{mod,numeric,invariant,performance}.rs` |
| `runtime.rs` | `runtime/{mod,import,timeline,causal,metrics}.rs` |
| `changeset.rs` | `changeset/{mod,prepare,staging}.rs` |
| `interpret.rs` | `interpret/{mod,claims,investigations,architecture,codeboarding}.rs` |

`mod.rs` keeps shared types/helpers and re-exports every item that was public
before. Test modules stay with the code they test. This keeps
`archmeld_core::<module>::...` and `crate::<module>::...` paths unchanged.

### 2. Six private groups with root re-exports

The 44 modules are grouped by concern:

| Group | Modules |
|---|---|
| `evidence` | store, events, hash, identity, snapshot, source, source_api, anchor, bridge, query, retention |
| `change` | changeset, apply, plan, verification, predicate, sandbox, scenario, units |
| `trace` | runtime, trace_adapters, trace_processor, perfetto, tracy_csv |
| `analysis` | interpret, blueprint, context, cognition, cognition_map |
| `ops` | manager, jobs, instigator, encrypted_backup, migrate, drift, watch, report |
| `surface` | error, language, lsp, scip, brief, status, review |

The group modules are **private** (`mod evidence { ... }`), and every public
module is re-exported at the crate root (`pub use evidence::store;`). This keeps
the public API exactly the pre-refactor surface: no new `archmeld_core::evidence`
path is advertised, and rustdoc keeps the old canonical paths. `pub mod` groups
were rejected because they would add six public paths and expose every module
twice.

`anchor` and `units` are `pub(crate)` inside their groups and re-exported with
`pub(crate) use`; they had no external consumers. `predicate` remains
`pub(crate)` (it already was) and is re-exported with `pub(crate) use
change::predicate;`; `pub use` would be E0365.

### 3. Group placement notes

- `retention` sits in `evidence` because `snapshot` calls `retention::auto_prune`;
  this removes an evidence→ops inversion.
- `units` sits in `change` next to `predicate`, its only consumer.
- `error` stays in `surface`; it is used by every group. The resulting
  group-level cycles (`surface ↔ analysis ↔ change`, `evidence ↔ ops`,
  `change ↔ analysis`) are accepted. Rust allows module cycles; this ADR does
  not claim an acyclic group DAG.
- `trace_processor` imports `scenario` from `change`; trace is not a pure leaf.

### 4. Map and documentation

`archmeld.map.txt` is rewritten in the same change: one `===<directory>===`
section per group directory, bare entry names, updated `R:file:` targets, and a
new `#snapshot:` id from a real `archmeld refresh`. `map check` must report
`state=ok`, `errors=[]`, `snapshot.match=true`. `map_dirty=true` is expected for
the snapshot in which the map is edited because the map is its own baseline.

Historical references in accepted ADRs, `docs/reviews/` and evidence stay
untouched; this ADR is the mapping record. `ADR-053`'s
`crates/archmeld-core/src/predicate.rs` now resolves to `predicate/mod.rs`.

### 5. No `#[path]`

`#[path]` would preserve file locations while pretending to group them; it
defeats navigation and the map. Physical moves are used, one group per commit,
with root re-exports preserving every old path.

## Consequences

- Public API shrinks by exactly two modules (`anchor`, `units`); no public path
  is added or renamed.
- `lib.rs` stays at `src/lib.rs` (`benchmark.rs` reads it; Cargo `[lib] path`
  is unchanged) and `CONTRACT_VERSION` stays `1.1`.
- Internal `crate::<module>` paths keep working through the root re-exports.
  `query.rs`'s `super::store` uses are normalized to `crate::store` before the
  move so they do not depend on group placement.
- The repository map must be regenerated; a stale map is `invalid`, not merely
  a warning.
- Each split and each group move is a separate commit so a failure can be
  reverted without a data migration.

## Acceptance

- `cargo check -p archmeld-core -p archmeld-api --all-targets --locked --offline`
- `cargo doc -p archmeld-core --no-deps`
- `test_lane.py --lane fast` and `--lane full`
- `archmeld map check` with `state=ok`, `errors=[]`, `snapshot.match=true`
- No evidence, sealed-pack, contract-version or test-expectation changes.
