# ADR-034 · Scientific refactor boundary

Status: accepted for the R1 scientific slice (B029 / AT064–AT065).

## Context

The fixture is a first-order IIR estimator with explicit units, one scalar state
and a phase-wrap helper. The mathematics are clear; the function is longer than
a trivial expression but not tangled. A refactor must not change expression
order, state ownership or units.

## Options compared

| Option | Behavior risk | State ownership | Added surface | Measured benefit |
|---|---|---|---|---|
| A. Do not refactor | none | already local to the function | none | none needed; remains a legal result |
| B. Extract one pure update helper (chosen) | expression order preserved; full trace and boundaries checked | `run_filter` still owns `previous` | one function, no class | measured 100k-sample medians: baseline 13.5 ms, refactored 16.8 ms; no speed claim |
| C. Introduce a filter class/strategy hierarchy | state lifecycle and update order must be re-proven | moves to an object | classes, configuration, lifecycle | no measured benefit; rejected |

The chosen extraction is a vehicle for the numeric acceptance test, not a
claimed performance or maintenance improvement. The measured runtime did not
improve, so no benefit is claimed. Option C is not implemented and cannot count
as complete without evidence. Option A remains legitimate for code whose math is
already clear.

## Numerical acceptance

- Golden truth is frozen in `adapters/scientific/fixtures/filter/golden.json`
  with units (`V`, `rad`), shape (`[8,1]`, `[6,1]`), dtype `float64`, seed and
  decimal `atol/rtol = 1e-12`.
- The frozen BehaviorContract checks units and shape before values, compares the
  full output trace (not only RMS), checks the phase boundaries at `+pi/-pi`,
  and runs empty-input, `alpha=0`, `alpha=1` and determinism checks.
- The baseline and refactored implementations both match the golden trace. The
  verification run binds the resulting snapshot, contract digest and toolchain
  digest; acceptance is a separate action.
- The profile is `scientific.numeric.v1`; it supports units, shape, dtype,
  state/update order, full dynamic traces and phase wrap. It is not a full
  dimensional-analysis engine, does not promise MATLAB semantics, and does not
  claim a statistical test.

## Consequences

- Scientific refactors must carry a frozen numeric contract and explicit
  boundary checks; an unchanged final RMS is not sufficient.
- A class/strategy refactor must provide its own evidence and cannot inherit
  credit from this minimal extraction.
- User-recorded real data was used after explicit authorization (ADR-053):
  reproducibility gates passed on the recorded file, while model-vs-Monte-Carlo
  differences remain an ungated observation.
- ULP, statistical, dimensional and MATLAB-comparison semantics (ignore_nan,
  sample/population variance, matlab_eps) are implemented in ADR-053.
  MATLAB expression-language semantics remain out of scope and are not claimed.
