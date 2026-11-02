# Scientific numeric profile

This directory holds the B029 golden-data contract and the
`scientific.numeric.v1` profile. The fixture is a real first-order IIR
estimator, not a mock: it has declared units, a scalar state, a documented
update order and a phase-wrap boundary.

## Contents

- `profiles/scientific.json` — supported capabilities and limitations.
- `fixtures/filter/estimator.py` — baseline implementation.
- `fixtures/filter/golden.json` — frozen golden truth, units, shape, dtype,
  seed, tolerance and boundary checks.
- `fixtures/filter/harness.py` — deterministic harness; writes the actual
  numeric artifacts and a test report.
- `fixtures/filter/baseline_estimator.py` — preserved baseline for the
  before/after comparison.

## Acceptance flow

`crates/archmeld-core/tests/scientific_profile.rs`:

1. Runs the harness against the baseline in a separate copy and compares the
   full output trace and phase values to the golden data.
2. Proposes a frozen contract with `test_report`, two `numeric_tolerance`
   checks and one `invariant` check. Units/shape are checked before values.
3. Applies a minimal pure `update` helper through staging, approval and the
   journaled writer.
4. Verifies the resulting snapshot; the full trace, phase boundaries and
   invariants must pass; then accepts the ChangeSet.
5. Measures baseline and refactored 100k-sample runs and records the option
   comparison. No speed or abstraction benefit is claimed.

## REST science profile status

- Comparison modes: `absolute_relative` (default), `ulp` (`max_ulps`) and
  `statistical` (`mean_abs_error_max`, `rms_error_max`, `max_abs_error_max`,
  `p95_abs_error_max`, `variance_max`, `stddev_max`). See ADR-053.
- `tolerance_model: "matlab_eps"` uses the IEEE-754 spacing at the compared
  magnitude; `normalization: "sample"` is the MATLAB N-1 convention.
- `nonfinite_policy: "ignore_nan"` skips NaN pairs and records `ignored_nan`;
  infinities still have to match.
- `unit_mode: "dimension"` checks the fixed SI/derived algebra with scale
  (for example `mm` equals `1e-3*m`). `unit_definitions` declares up to 64
  project-specific multiplicative units; unknown, affine and logarithmic units
  are `inconclusive`, never guessed.
- `semantics` is `ieee754_elementwise` only. MATLAB expression-language
  semantics (colon/linspace, implicit expansion, column-major reshape) are
  explicitly out of scope and rejected.
- Real authorized data evidence:
  `docs/release-evidence/B029-real-data.json`, produced by
  `tooling/b029_real_data.py`; raw data is not stored in this repository.

## Command

```sh
ARCHMELD_WRITE_EVIDENCE=1 \
RUSTUP_TOOLCHAIN=1.94.1-x86_64-pc-windows-gnu \
PATH=/d/Strawberry/c/bin:/c/Users/<user>/.cargo/bin:$PATH \
cargo test -p archmeld-core --test scientific_profile
```

Evidence: `docs/evidence/B029-scientific.json`.
ADR: `docs/adr/ADR-034-scientific-refactor-boundary.md`.
