# Tooling inventory

`tooling/` contains the maintained entry points, the self-tests for those entry
points, and the task-specific evidence generators kept for reproducibility.
This file is the map; use it before adding another script.

## Canonical entry points

| Concern | Entry point | Use |
|---|---|---|
| Shared primitives | `common.py` | `ROOT`, `sha256_*`, JSON read/write and subprocess capture |
| Resolve tools and temp paths | `portable_env.py --json` | Inspect what the machine will actually use |
| Canonical Cargo environment | `build_helper.cargo_env()` | The single source of `CXXFLAGS`, PATH, temp and toolchain pins |
| Daily tests | `test_lane.py --lane fast` | Core/API/parser/stable-server tests with prebuilt RocksDB, lld and sccache |
| Arbitrary Cargo test | `cargo_test.py -- -p <crate> ...` | Canonical env for one-off `cargo test` invocations |
| Evidence tests | `test_lane.py --lane evidence` | AT067 four commands through `baseline/run_cmd.py` plus the verifier |
| Prebuilt RocksDB | `rocksdb_cache.py status\|verify\|create` | Fast-lane archive; fail-closed |
| Linker probe | `link_probe.py --probe --workspace` | Verify lld before the fast lane uses it |
| sccache | `install_sccache.py install\|smoke` | Pinned install with cache-hit smoke test |
| Toolchain lock | `lock_toolchains.py` | Regenerate `toolchains.lock.json` from measured facts |
| Toolchain health | `doctor.py` | Compare the lock against this machine |
| Upstream registry | `upstream.py status\|verify\|check\|diff\|drill` | Release-only intake, pins, drills |
| Upstream compatibility | `upstream_delta_drill.py` | Synthetic adapter-interface drill |
| Release package | `package_release.py` | Offline CLI bundles, VSIX, SBOM and checksums |
| Release tests | `test_release.py` | Checksums, install/upgrade/rollback/uninstall, VSIX, Linux smoke |
| B030/AT067 verifier | `verify_b030_evidence.py` | Fail closed on an evidence bundle |
| Pack seal | `reseal_pack.py --check\|--write` | Only writer for `PACK_MANIFEST.json` and `SHA256SUMS` |
| Handoff | `handoff_check.py` | Regenerate `B032-handoff.json` |
| Notices | `gen_notices.py` | Regenerate third-party notices from `Cargo.lock` |
| Public export | `public_snapshot.py check\|export\|verify-export` | Sanitized single-commit snapshot |
| Public push guard | `publication_guard.py --hook\|--ci\|--install` | Block private-history branches on the public remote |
| Self-analysis acceptance | `self_analysis_acceptance.py` | Read-only MCP + ledger checks against a real state directory |

## Full inventory

### Build and environment

- `common.py` — stdlib-only primitives: repo root, SHA-256, JSON and subprocess
  capture. It owns no Cargo or environment logic.
- `portable_env.py` — resolves tools, temp root and include paths without
  encoding machine paths.
- `build_helper.py` — builds the index helper and owns `cargo_env()`.
- `cargo_test.py` — thin canonical wrapper around `cargo test`.
- `test_lane.py` — lane runner (`fast`, `core`, `server`, `parser`, `full`,
  `evidence`).
- `rocksdb_cache.py` — prebuilt `librocksdb.a` cache with provenance.
- `link_probe.py` — C/Rust/workspace lld probe.
- `install_sccache.py` — pinned sccache install and smoke test.
- `lock_toolchains.py` — writes `toolchains.lock.json` and
  `platform-matrix.json`.
- `doctor.py` — verifies the lock against the current host.

### Upstream

- `upstream.py` — registry, release-only checks, probes, diff, drill, rollback
  and guarded record updates.
- `upstream_delta_drill.py` — adapter compatibility drill.
- `test_upstream.py` — 44 registry/check/diff/drill self-tests.

### Release and handoff

- `package_release.py` — deterministic bundles and VSIX.
- `test_release.py` — packaged-artifact tests.
- `verify_b030_evidence.py` — B030/AT067 fail-closed verifier.
- `gen_notices.py` — third-party notices.
- `reseal_pack.py` — sealed pack manifest and checksums.
- `handoff_check.py` — B032 handoff evidence.

### Publication and security

- `public_snapshot.py` — scan, redact and export a sanitized snapshot.
- `publication_guard.py` — pre-push and CI guard for the public remote.
- `test_public_snapshot.py` — 9 scanner/export tests.
- `test_publication_guard.py` — 12 guard tests.

### Self-analysis and experiments

- `self_analysis_acceptance.py` — read-only MCP/ledger acceptance.
- `b016_trace_evidence.py`, `b016_trace_jump_evidence.py` — B016 trace runs.
- `b017_model_analysis_run.py`, `b017_model_proxy.py` — authorized model
  interpretation.
- `b026_webview_paint_evidence.py` — webview paint measurement.
- `b028_unix_socket_remote.py` — Unix socket remote proof.
- `b029_real_data.py` — real-data numeric evidence.
- `e4a_launcher.py`, `e4a_grade.py`, `e4a_mcp_shim.py` — E4a A/B experiment.
- `codeboarding_static_probe.py` — pinned static analyzer probe.

Task-specific scripts (`b0xx_*`, `e4a_*`, `codeboarding_static_probe.py`,
`b028_unix_socket_remote.py`) exist so a past acceptance can be reproduced.
They are not general-purpose entry points and should not grow new modes.

### Self-tests

- `test_common.py`, `test_portable_env.py`, `test_upstream.py`,
  `test_public_snapshot.py`, `test_publication_guard.py`, `test_release.py`.

Run one directly, for example:

```sh
py -3.11 tooling/test_common.py
py -3.11 tooling/test_upstream.py
py -3.11 tooling/test_publication_guard.py
```

## Conventions

- **Do not add a script for a flag.** Extend an existing entry point first; a
  new top-level script needs a distinct lifecycle or a different trust boundary.
- **One environment source.** Any new build/test script must call
  `build_helper.cargo_env()` rather than copying `CXXFLAGS`, PATH or temp logic.
  A new tool resolves through `portable_env.py`.
- **Record, do not reconstruct.** Evidence runs go through
  `baseline/run_cmd.py` so `baseline/commands.ndjson` has cwd, argv, exit code,
  duration and log hash.
- **Evidence scripts are append-only.** Keep `b0xx_*` and `e4a_*` working; do
  not delete them because a task is closed.
- **No secrets.** Logs must not contain model keys, service tokens or private
  dataset identifiers.
- **Public safety.** A new tool that can read private files must be covered by
  `public_snapshot_policy.json` or redacted by the exporter.
