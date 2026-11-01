# ADR-028 · Deterministic numeric, invariant and performance predicates

Status: accepted for the R1 verification slice (B022 residual, used by B026/B029).

## Decision

- `numeric_tolerance` requires `expected_artifact`, `actual_artifact`,
  decimal-string `atol`/`rtol`, an explicit `nonfinite_policy`
  (`reject`/`match`) and an optional `columns` selection. Artifacts are either
  CAS digests or workspace-relative files produced by the check; file bytes are
  stored in the CAS before comparison.
- Shape, column identity and units are checked before any numeric value. Unit
  metadata on one side only is a failure, never an inferred conversion. The
  comparison is `abs(expected - actual) <= atol + rtol * max(abs(expected),
  abs(actual))`, evaluated with IEEE-754 doubles after parsing the frozen
  decimal strings.
- `invariant` requires the registered `numeric.invariants.v1` checker with
  version `1` and an explicit `configuration`. Registered rule kinds are
  `finite`, `nonnegative`, `range`, `monotonic`, `constant` and `sum`; an
  unregistered checker or rule returns `unsupported`, so a model/human text
  claim can never become a pass.
- `performance_budget` requires `metric`, `statistic` (`min`, `max`, `mean`,
  `median`, `p95`, `p99`, `sum`, `count`), decimal-string `threshold`, `unit`,
  `comparison`, `min_repetitions`, `environment_digest` and a measurements
  artifact. Samples must be finite and non-negative. Too few samples, a wrong
  metric/unit/environment, or a missing baseline return `inconclusive`, never
  `passed`.
- `comparison=baseline_max_ratio` requires `baseline_artifact` with the same
  metric, unit, environment digest and at least `min_repetitions` samples. The
  result reports the candidate and baseline distributions (count, min, max,
  mean, median, p95, p99), the raw samples (bounded with an explicit truncation
  flag), ratio and delta. A missing baseline cannot prove absence of
  regression.
- Predicate details are AM-C14N-1-compatible: measured values and thresholds
  are emitted as decimal strings; only counts remain integers.
- Every command-based predicate still checks the command exit code unless the
  frozen predicate explicitly sets `allow_nonzero_exit`. Checker-only
  predicates have no process exit code.
- The existing acceptance rule is unchanged: optional failures remain visible,
  required `failed`/`inconclusive`/`unsupported`/`error` results block
  `accepted`.

## Residual limits

- Statistical comparison currently uses deterministic summary statistics and a
  ratio budget, not a hypothesis test. A statistical test must be added with a
  frozen method, seed and outlier rule before it can be claimed.
- Per-check filesystem confinement remains part of B028; the current executor
  isolates the process environment and argv, not the whole tree.
- Performance results are only meaningful for the declared environment digest;
  the product does not normalize results across machines.
