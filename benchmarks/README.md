# B026 benchmark suite

This directory contains the reproducible T0/T1/T2 measurements and the
deterministic UI-render benchmark used by AT058.

## Data levels

| Level | Source | Purpose |
|---|---|---|
| T0 | `crates/archmeld-core/tests/common` fixture (5 files) | deterministic smoke timing and cold snapshot capture |
| T1 | fixed `upstream-probes/real-export/archmeld-core.ndjson` (335 nodes / 972 edges) imported over a copy of `crates/archmeld-core` | real-source query, source read and drift timing |
| T2 | deterministic generator in `tests/benchmark.rs` (100,000 entities / 99,500 relations) | scale import, query, database size and working-set measurement |

T2 is synthetic scale data. It is never used to claim business correctness;
T1 carries the real-source checks. The T1 export and its format are documented
in `UPSTREAM_PATCHES.md`; tests do not clone a moving branch.

## Command

```sh
RUSTUP_TOOLCHAIN=1.94.1-x86_64-pc-windows-gnu \
PATH=/d/Strawberry/c/bin:/c/Users/<local-user>/.cargo/bin:$PATH \
cargo test -p archmeld-core --test benchmark -- --nocapture
```

Set `ARCHMELD_WRITE_EVIDENCE=1` to also write
`docs/evidence/B026-benchmark.json`. The command record
`#b026-benchmark` in `baseline/commands.ndjson` stores cwd, argv, exit code,
log hashes, git state and the environment/tool versions.

## Budgets and reporting

The benchmark records raw samples and p50/p95/max, the dataset sizes, database
size, working set (Windows), cancellation process audit, and the environment
digest. Every run writes `target/b026-benchmark-latest.json` before any budget
assertion, so a failed run keeps the problem data. With
`ARCHMELD_WRITE_EVIDENCE=1` it also writes the durable
`benchmarks/results/B026-latest.json` and `docs/evidence/B026-benchmark.json`.
No sample is dropped or replaced by a favorable retry.

Budgets currently come from S16's starting SLO and the declared reference
machine:

- T1 local symbol query p95 ≤ 300 ms
- T1 source read p95 ≤ 500 ms
- cancellation returns and reports within 3 s
- T1 drift/stale analysis p95 ≤ 1 s
- deterministic webview HTML generation p95 ≤ 500 ms

The webview number is renderer cost only; VS Code host and paint latency are
not measured. Cancellation uses the cooperative flag and process-tree
termination. On Windows the process table uses native Toolhelp32 enumeration,
so the pre-kill and post-kill survivor audits stay cheap; PowerShell is only a
fallback.

## Resource guards

The same test exercises the hard limits: query `max_nodes` 1..=2000,
`max_depth` 0..=8, source `max_bytes` 1..=1,048,576, batch limits of 500
entities and 2,000 relations, and the 4 MiB stdout cap. Over-limit requests
return explicit `invalid`/`resource_exhausted` and are never silently skipped.
