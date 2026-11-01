# Upstream patches and local additions

Base: `codegraph-ai/CodeGraph` @ `590c2ca7dd7b37bd1d41caf45fac535b6522f969`
(branch `archmeld/initial`). Remote `origin` renamed to `upstream`.

Each entry records necessity, affected paths, whether it should be offered
upstream, and the acceptance tests it supports. No upstream test was deleted or
weakened.

| # | Change | Paths | Necessity | Upstream? | AT |
|---|---|---|---|---|---|
| 1 | Workspace member for the evidence core | `Cargo.toml` | ARCHMELD crate must build with the pinned workspace | No (product code) | B003+ |
| 2 | `archmeld-export-v1` NDJSON export using the graph's real node/edge iterators | `crates/codegraph/src/export/ndjson.rs` (new), `crates/codegraph/src/export/mod.rs` | `export_json` iterates `0..node_count` and drops live objects after deletions (proved in `tests/archmeld_probes.rs`) | Candidate: a correctness fix for upstream | AT004, AT013 |
| 3 | Sparse-ID / persistence / cache probes | `crates/codegraph/tests/archmeld_probes.rs` (new) | Reproducible boundary evidence; upstream tests untouched | No (fork-only probes) | AT004, AT005 |
| 4 | `--archmeld-export-v1` / `--archmeld-state-root` one-shot export | `crates/codegraph-server/src/main.rs` | Structural export and isolated state root are product requirements, not upstream features | No (fork-only flags) | AT005, AT013 |
| 5 | `McpServer::run_archmeld_export` | `crates/codegraph-server/src/mcp/server.rs` | Index once, export the complete graph, exit without serving | No | AT013 |
| 6 | State root override | `crates/codegraph-server/src/memory.rs` (`codegraph_home_dir`, `project_data_dir`) | Writes must be isolated per project/snapshot; never touch the user's real `~/.codegraph` | Candidate: configurable state root is generally useful | AT005 |
| 7 | Daemon state directory follows the state root | `crates/codegraph-server/src/daemon.rs` | Same isolation for daemon heartbeats | Candidate | AT005 |
| 8 | Startup crash marker honours the state root (parse args before `crash_phase::mark`) | `crates/codegraph-server/src/main.rs` | First marker otherwise wrote `~/.codegraph/last-phase.<pid>.json` before the flag was applied | Candidate (arg parsing before crash instrumentation is generally correct) | AT005 |
| 9 | Process-level state isolation test | `crates/codegraph-server/tests/archmeld_state_root.rs` (new) | Proves no `~/.codegraph` write in bridge mode without needing RocksDB | No (fork-only test) | AT005 |
| 10 | Spec pack copied as baseline | `docs/archmeld/**`, `AGENTS.md` | Required by FIRST_SESSION; upstream has no AGENTS.md to merge | No | — |
| 11 | Lockfile entries for `archmeld-core`, `sha2`, `rusqlite` | `Cargo.lock` | New crate dependencies, resolved with `--locked` discipline | No | B003+ |

## Build-environment workarounds (not source changes)

These are recorded because the baseline is not reproducible without them on this
machine; they do not alter upstream source:

- `CXXFLAGS="-include cstdint"` for RocksDB 8.10 under GCC 13 (missing transitive
  `<cstdint>` include).
- `LIBCLANG_PATH` pointing at a local libclang 18.1.1 bundle for bindgen.
- `PATH` must put the MinGW runtime DLL directory (Strawberry) first; otherwise
  the Rust GNU test binaries fail with `STATUS_ENTRYPOINT_NOT_FOUND`.
- `TMP`/`TEMP` redirected to `F:\Codex_Work_Space\_archmeld_temp` because the
  system drive was full.
- RocksDB still crashes at runtime under Windows GNU
  (`STATUS_ACCESS_VIOLATION` in `storage::rocksdb_backend::tests::test_delete`),
  so the persistent backend is exercised in the Linux container.
