# Upstream patches and local additions

Base: `codegraph-ai/CodeGraph` @ `ae4b2e54fc8ce1c074523943fc750b20f68bf8aa`
(upstream 0.21.2; previous base
`590c2ca7dd7b37bd1d41caf45fac535b6522f969`). Branch `archmeld/initial`.
Remote `origin` renamed to `upstream`.

Each entry records necessity, affected paths, whether it should be offered
upstream, and the acceptance tests it supports. No upstream test was deleted or
weakened.

## Merge history

- **2026-11-24 — upstream 0.21.2 (`ae4b2e54`).** Four upstream commits:
  thread budgeting and engine-download hardening (`008cb5d8`), LSP
  `--max-files`/`--exclude` (`16c9aa0b`), PHP receiver-class method resolution
  (`064befd2`), and the version bump (`ae4b2e54`). The only content conflict
  was `crates/codegraph-server/src/main.rs`: upstream moved `Args::parse()`
  into `main()` so the thread budget is known before any pool exists, while the
  local patch parsed arguments and installed `ARCHMELD_STATE_ROOT` inside
  `run()`. Resolution keeps upstream's structure and installs the state root in
  `main()` immediately after `Args::parse()` and before the crash marker, lock
  or cache path. `DaemonConfig` gained upstream's `max_files` field. The local
  `--archmeld-export-v1`, `--archmeld-state-root`, export/test modes and worker
  root registration are unchanged. Evidence:
  `docs/release-evidence/UPSTREAM-0.21.2-2026-11-24.json`.

## Patch surface reconciliation (2026-11-25)

The authoritative path-level manifest is `patches/codegraph/manifest.json`;
`python tooling/upstream.py patches --check` fails if an upstream-owned path is
unaccounted for. The table below records paths that were missing from the
original 11-entry table. It does not replace the manifest and does not change the
pin.

| ID | Change | Paths | Upstream? | AT / evidence |
|---|---|---|---|---|
| P003 | One-shot export entry points | `crates/codegraph-server/src/mcp/server.rs`, `crates/codegraph-server/examples/archmeld_export.rs` | No (fork entry) | AT013, B031 |
| P005 | State-root isolation helpers | `crates/codegraph-server/src/crash_phase.rs`, `crates/codegraph-server/src/index_state.rs`; `memory.rs`/`daemon.rs` already listed above | Candidate | AT005 |
| P007 | Python per-symbol alias parsing | `crates/codegraph-python/src/extractor.rs`, `crates/codegraph-parser-api/src/relationships/imports.rs` | Candidate | `docs/release-evidence/PYTHON-ALIAS-BINDING-2026-11-25.json` |
| P008 | Python per-file import binding resolver | `crates/codegraph-server/src/watcher.rs` | Candidate | `PYTHON-IMPORT-BINDING-2026-11-25.json`, `PYTHON-ALIAS-BINDING-2026-11-25.json` |
| P009 | Upstream parser test addition | `crates/codegraph-python/tests/parser_impl_tests.rs` | Candidate | alias edge assertion; no upstream test removed or weakened |
| P010 | Product README | `README.md` | No (product doc) | M1 |

Conflict hotspots are `Cargo.toml`, `main.rs`, `memory.rs`, `daemon.rs`,
`crash_phase.rs`, `index_state.rs`, `watcher.rs` and the export files. The
manifest records a SHA-256 of each path-scoped diff; refresh it with
`python tooling/upstream.py patches --write` after a local change.

## Windows test portability and RocksDB feature split (2026-11-27)

| ID | Change | Paths | Upstream? | AT / evidence |
|---|---|---|---|---|
| P015 | Platform-absolute test fixtures (tempdir-style logical root, `Url::from_file_path`) and native-separator adjacent-test lookup; no assertion weakened | `crates/codegraph-server/src/handlers/custom.rs`, `handlers/navigation.rs`, `domain/unused_code.rs`; `backend.rs`/`memory.rs` are accounted under P005/P011 | Candidate: upstream Windows portability fix | Windows server-lib suite 426/0/1; `WINDOWS-VERIFICATION-2026-11-27` |
| P016 | Parser-only crates no longer enable the RocksDB default backend; `codegraph-server` opts in; examples/bench/test modules gated; build-time commit overrides and corrected `.git` rerun paths | `crates/codegraph-server/Cargo.toml`, `crates/codegraph/Cargo.toml`, `crates/codegraph-server/build.rs`, `crates/codegraph/build.rs`, `crates/codegraph/tests/integration/mod.rs`, `crates/codegraph/tests/unit/export_test.rs`; `crates/archmeld-api/build.rs` is product-owned | Candidate for the parser split; build overrides are generally useful | parser suites; `cargo tree`; release provenance |
| P017 | `[profile.test] debug = 1` (line tables) with an explicit `librocksdb-sys` `debug = false`; test binaries keep file/line backtraces but shrink enough to cut link time. `dev` debuginfo is unchanged | `Cargo.toml` | No (local build profile; upstream may prefer its own default) | fast lane; `WINDOWS` perf evidence; AT067 re-run |

These changes are test/build-graph only; no product runtime behaviour is
changed. The fixture patch preserves every exact path, count, line and column
assertion.

## Retired upstream surfaces (2026-11-27)

| ID | Change | Paths | Upstream? | AT / evidence |
|---|---|---|---|---|
| P018 | Retire the stale Visual Studio extension (manifest 0.13.1, .NET Framework csproj, no build/test/release reference) | `vs-extension/DEVELOPMENT.md`, `vs-extension/src/CodegraphExtension/CodegraphExtension.csproj`, `CodegraphLanguageClient.cs`, `CodegraphPackage.cs`, `source.extension.vsixmanifest` | No — out-of-scope surface removal; no upstream code change | `docs/release-evidence/SURFACE-RETIRE-2026-11-27.json` |

This is an explicit deviation from the default "preserve upstream surfaces"
rule, authorized with the T4 archive/retire scope. The path is deliberately not
added to `patches/codegraph/manifest.json`: the manifest only covers
`Cargo.toml`, `Cargo.lock`, `README.md` and `crates/codegraph*`, and adding a
deleted path would make `upstream.py patches --check` report a missing path.
A release-only upstream merge does not reintroduce an unchanged deleted path;
an upstream edit to these files would surface as a delete/modify conflict.


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
| 12 | `[profile.dev.package.librocksdb-sys] debug = false`; helper manifest `index` includes `by_language`/`cross_file_implements`; deferred `Implements` post-pass | `Cargo.toml`, `crates/codegraph-rust/src/{mapper.rs,lib.rs}`, `crates/codegraph-server/src/{indexer.rs,watcher.rs}`, `crates/codegraph-server/examples/archmeld_export.rs` | The pinned RocksDB archive is 833 MB with debuginfo and cannot link on the 16 GB host; the helper needs no RocksDB debuginfo. The index block and cross-file trait resolution are required by P13–P15; ambiguous trait names are never guessed | Candidate: parser crates do not need the persistent backend | P13, P14, P15 |

## Build-environment workarounds (not source changes)

These are recorded because the baseline is not reproducible without them on this
machine; they do not alter upstream source:

- `CXXFLAGS="-include cstdint"` for RocksDB 8.10 (missing transitive
  `<cstdint>` include). The current host uses MSYS2 UCRT Mingw-w64 16.1.0;
  the earlier winlibs 13.2.0 installation is no longer present.
- The root `Cargo.toml` profile override in patch 12 keeps debuginfo off the
  322 RocksDB C++ translation units. Without it the debug archive is 833 MB and
  the helper link does not complete reliably on this host; it does not remove
  debuginfo from Rust code.
- `LIBCLANG_PATH` pointing at a local libclang 18.1.1 bundle for bindgen.
- `PATH` must put the MinGW runtime DLL directory first; `ARCHMELD_MINGW_BIN`
  (or its retained alias `ARCHMELD_STRAWBERRY_BIN`) is set to
  `<path> Otherwise Rust GNU test binaries fail with
  `STATUS_ENTRYPOINT_NOT_FOUND`.
- `TMP`/`TEMP` redirected to `<path> because the
  system drive was full.
- RocksDB still crashes at runtime under Windows GNU
  (`STATUS_ACCESS_VIOLATION` in `storage::rocksdb_backend::tests::test_delete`),
  so the persistent backend is exercised in the Linux container.
