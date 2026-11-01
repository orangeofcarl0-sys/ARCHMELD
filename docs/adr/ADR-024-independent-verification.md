# ADR-024 · Independent verification and the accepted gate

Status: accepted for the R1 verification slice (B022).

## Decision

- Verification is a separate immutable run bound to one ChangeSet revision,
  one resulting snapshot, one frozen contract digest and one toolchain digest.
  A caller cannot substitute the staging snapshot or an older snapshot: the
  request is rejected before any check starts, and the applied workspace is
  compared against the resulting snapshot before and after execution.
- The executor reads checks from the frozen contract. It never lets an agent
  add, remove, reorder or mark a check optional. Missing, extra, unsupported,
  inconclusive, cancelled and timed-out results are recorded as such and never
  rewritten to `passed`.
- Commands run without a shell, with an allowlisted environment, captured
  stdout/stderr, a declared per-check timeout, and process-tree cleanup on
  timeout. There is no retry: a timeout is final and the verification is not
  accepted.
- Predicates implemented in this slice: `exit_code`, `test_report`
  (JUnit/JSON with a minimum test count and a maximum failure count),
  `file_digest`, and `architecture_rules`. ADR-028 adds executable
  `numeric_tolerance`, `invariant` and `performance_budget` engines, including
  baseline-relative performance comparison.
- A `test_report` only passes when the command exited zero, the report parses,
  the test count is at least the declared minimum, and failures+errors do not
  exceed the declared maximum. A zero-test run is a failure, not a pass.
- The final verdict is computed by deterministic Rust code, not a model. It is
  `accepted` only when every required contract check passed, no result is
  missing or extra, the snapshot/contract/toolchain bindings match, and no
  source drift was observed.
- `accept` is a separate action. It rechecks the workspace against the verified
  snapshot and appends `accepted`. A failed or drifted verification leaves the
  ChangeSet `failed`; it cannot be accepted by retrying the accept call.

## Residual limits

- Command checks receive workspace read access and the allowlisted process
  environment; finer per-check filesystem confinement is part of the security
  work (B028).
- Numeric tolerance, invariant and performance-budget predicates are
  implemented by deterministic engines (ADR-028). A missing artifact, shape or
  unit mismatch, insufficient repetitions, environment mismatch or missing
  baseline is `failed`/`inconclusive`, never a silent pass.
- Baseline/candidate performance comparison is available only when both sides
  carry the same metric, unit, environment digest and enough samples; other
  differential reports remain future work.
- A verification run interrupted by process death remains `running` for
  inspection; it is never treated as accepted.
