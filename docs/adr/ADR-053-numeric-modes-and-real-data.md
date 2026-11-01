# ADR-053: ULP/statistical comparison, dimensional checks and authorized real data

Status: accepted for the project testing phase.

## Context

B029's first slice compared full traces with decimal `atol/rtol`, exact unit
strings and an invariant rule set. Three gaps remained: a statistical or ULP
comparison mode, dimensional reasoning beyond string equality, and real
user-authorized recorded data. MATLAB semantics were also listed as missing.

## Decision

### Comparison modes

1. `numeric_tolerance` keeps `mode: "absolute_relative"` as the default. It
   accepts `tolerance_model: "atol_rtol"` (required `atol`/`rtol`) or
   `tolerance_model: "matlab_eps"` with a non-negative `eps_multiplier`;
   `matlab_eps` adds `multiplier * eps(x)` where `eps(x)` is the actual IEEE-754
   spacing at the largest compared magnitude, plus optional `atol`/`rtol`.
2. Two additional modes are accepted:
   - `ulp`: requires `max_ulps` (non-negative integer) and compares IEEE-754
     bit distance. `-0.0` and `0.0` are distance 0; NaN/Inf follow the declared
     `nonfinite_policy`.
   - `statistical`: requires a `statistics` object with one or more of
     `mean_abs_error_max`, `rms_error_max`, `max_abs_error_max`,
     `p95_abs_error_max`, `variance_max`, `stddev_max`. `normalization` selects
     `sample` (N-1, the default and the MATLAB convention) or `population` (N);
     sample variance/stddev with fewer than two pairs is `inconclusive`.
3. `nonfinite_policy` accepts `reject`, `match` and `ignore_nan`. `ignore_nan`
   skips pairs where either side is NaN and records `ignored_nan`; infinities
   still have to match sign and type.
4. Shape, column names and unit metadata are checked before values in every
   mode. Unknown modes, thresholds, normalization values and tolerance models
   are contract errors, not silently ignored fields.

### Dimensional checks

5. `unit_mode: "dimension"` parses both unit strings with a bounded SI algebra
   (base units, common derived units, SI prefixes, products, quotients, integer
   powers and decimal scale constants) and requires both dimension exponents
   and scale to match. `mm` is equivalent to `1e-3*m`, while `mm` is not
   equivalent to `m`.
6. `unit_definitions` declares project-specific multiplicative units as
   expressions over the built-in table or other declared units (up to 64,
   cycle-checked, no shadowing of built-ins). This extends the fixed SI table
   without shipping a general conversion database.
7. An unknown symbol, an affine/logarithmic unit (`degC`, `dB`, `dBm`) or a
   missing unit on either side makes the check `inconclusive` with the reason;
   it never guesses a conversion. `unit_mode: "exact"` remains the default.

### MATLAB semantics

8. Comparison-relevant MATLAB semantics are implemented: `ignore_nan`,
   sample/population variance and stddev, and `matlab_eps` spacing tolerance.
9. `semantics` still accepts only `ieee754_elementwise`; `semantics: "matlab"`
   remains `unsupported`. Expression-language semantics (colon/linspace,
   implicit expansion, column-major reshape) are out of scope for a comparator:
   they belong to the producer that computes the artifacts. The unsupported
   response lists them and points at the implemented comparison features, so the
   boundary is explicit rather than a silent approximation.

### Real authorized data

6. The user authorized B029 real-data use. The evidence run reads
   an authorized private recorded dataset (identity and values withheld
   from the public snapshot), processes it locally and does not
   upload it or copy raw/derived data into the repository.
7. Positive gates are reproducibility on the recorded values:
   - two independent extraction/serialization paths (`repr` vs `.17g`)
     compared in statistical mode with all thresholds 0;
   - the same pair compared in ULP mode with `max_ulps = 0`;
   - the recorded millimetre unit compared with `1e-3*m` in dimension mode,
     with a negative control (`m`) that must fail.
8. The model-versus-Monte-Carlo difference is reported as an observation with
   `gate: "none"` because they are different estimators and no frozen tolerance
   exists. It is not turned into a pass by inventing a threshold.

### Operator surface

9. `archmeld numeric-check --state <ledger> --root <artifacts> --predicate
   <json>` evaluates the same engine directly. It stores exact artifact bytes
   in the CAS and reports state/detail, but creates no verification and accepts
   no ChangeSet.

## Evidence

- `crates/archmeld-core/src/predicate.rs` unit tests for ULP distance, statistical
  thresholds, sample/population variance, `ignore_nan`, `matlab_eps`, declared
  units, dimension equivalence/inconclusive/scale mismatch, MATLAB refusal and
  unknown mode.
- `crates/archmeld-core/tests/scientific_profile.rs` now freezes ULP
  (`max_ulps=0`), statistical (all metrics 0) and dimension-mode checks on the
  real IIR fixture in addition to the existing absolute tolerance.
- `crates/archmeld-api/tests/trace_import_cli.rs` exercises `numeric-check`.
- `docs/release-evidence/B029-real-data.json` records the source hash,
  extraction hashes, CLI argv/exit/stdout hashes, check outcomes and the ungated
  model-vs-MC distribution.
- `tooling/b029_real_data.py` reproduces the run on an authorized local copy.

## Consequences

- A stricter ULP gate is available for deterministic numeric code; a statistical
  gate is available when element ordering or aggregation is the declared
  contract, including sample/population variance.
- Dimensional checking is extensible through contract-declared multiplicative
  units. It still refuses unknown, affine and logarithmic units instead of
  silently treating them as dimensionless.
- MATLAB comparison semantics are implemented where they affect artifact
  comparison. MATLAB expression-language semantics are explicitly out of scope
  and are neither emulated nor claimed.
- The real-data positive checks prove extraction reproducibility, not that the
  optical model agrees with Monte Carlo. The observation is recorded without a
  gate.
