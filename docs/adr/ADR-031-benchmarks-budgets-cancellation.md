# ADR-031 · T0/T1/T2 benchmarks, budgets and cancellation

Status: accepted for the R1 scale slice (B026 / AT058).

## Decision

- Benchmarks use three declared levels: T0 is the deterministic five-file
  fixture; T1 is the fixed real CodeGraph export of `archmeld-core` imported
  over a source copy; T2 is a deterministic synthetic 100,000-entity /
  99,500-relation ledger. T2 measures resources only and never claims business
  correctness. The real-export fixture is pinned in the repository; tests do
  not clone a moving branch.
- Every reported percentile comes from real process samples. The result records
  the raw samples, p50/p95/max, dataset sizes, database size, working set on
  Windows, environment digest, cancellation process audit and command
  provenance. Synthetic data is labeled as such.
- The starting budgets are taken from S16, not invented per run: T1 symbol
  query p95 ≤ 300 ms, T1 source read p95 ≤ 500 ms, cancellation within 3 s,
  T1 drift/stale analysis p95 ≤ 1 s, and deterministic webview HTML generation
  p95 ≤ 500 ms. The webview number is renderer cost, not VS Code host or paint
  latency; that limitation is in the result.
- Every run writes `target/b026-benchmark-latest.json` before budget
  assertions; `ARCHMELD_WRITE_EVIDENCE=1` additionally writes the durable
  `benchmarks/results/B026-latest.json` and
  `docs/evidence/B026-benchmark.json`. A failing run preserves the samples;
  there is no favorable retry, sample deletion or threshold relaxation.
- Cancellation checks the cooperative flag before process cleanup. On Windows,
  the process table is read with native Toolhelp32 enumeration instead of
  spawning PowerShell, so the pre-kill, post-kill and final survivor audits
  remain cheap. The measured cancel-to-feedback time is about 0.6 s, with the
  observed child and an empty survivor list still recorded. The PowerShell
  query remains only as a fallback if the native call fails.
- Resource guards are explicit errors: query `max_nodes` 1..=2000 and
  `max_depth` 0..=8, source `max_bytes` 1..=1,048,576, import batches of at most
  500 entities / 2,000 relations, and a 4 MiB stdout cap with
  `truncated=true`. Exceeding a limit returns `invalid` or
  `resource_exhausted`; files are not silently skipped.
- T2 import is intentionally heavier (about 90 s and about 520 MB of SQLite on
  the reference machine). It is a real scale measurement, not a mock; temp data
  is placed on the F: scratch volume when available.

## Residual limits

- T3 (million-event traces and incremental change storms) is not part of this
  slice; its data and budgets belong with the runtime/report work.
- Memory is measured but has no hard threshold yet; S16 requires measuring
  before setting an engineering budget.
- UI latency is a renderer proxy, not an editor paint measurement. A VS Code
  host integration benchmark is still open.
- Results are specific to the recorded environment digest; the product does not
  normalize benchmark numbers across machines.
