# E4a fixtures

Two disposable Python fixtures for the E4a semantic-value experiment. Both
start at a 10% discount and ask the agent to move to 15% while keeping the
modules consistent.

## nonlexical

The coupling is not a copied `0.9` literal, and 18 decoy modules under
`src/services/` contain similar constants (`1000`, `11110`, `0.9`, `90`) for
shipping, tax, experiments, inventory and reporting. They are unrelated to
the storefront rate and must not be changed.

- `src/pricing.py` returns `value * 0.9`.
- `src/invoice.py` stores `INVOICE_ADJUSTMENT_BPS = 1000` (basis points) and
  computes `value * (10000 - INVOICE_ADJUSTMENT_BPS) / 10000`.
- `src/analytics.py` checks `round(apply_discount(12345)) == EXPECTED_TOTAL`
  with `EXPECTED_TOTAL = 11110`.

Correct 15% state: `pricing` returns `0.85`;
`INVOICE_ADJUSTMENT_BPS = 1500`; `EXPECTED_TOTAL = 10493`. The map Entry for `pricing.py` states this
obligation in `S`; the decoys make plain constant-grepping noisy, so the map
is the only place that names the two real dependents.

## lexical

The control fixture copies the old rate into greppable literals:

- `src/invoice.py` has `EXPECTED_DISCOUNT = 0.9`.
- `src/analytics.py` checks `apply_discount(100) == 90`.

Correct 15% state: both become 0.85 / 85.

## Map

`map.template.txt` is not copied into the run repo. The launcher substitutes
`{{SNAPSHOT}}` with the `archmeld init` snapshot id and writes
`archmeld.map.txt` after indexing, so the map is not part of the snapshot.
Entry references use only `file:`; `relation:` remains deferred.

## Ground truth

- A correct run changes `src/pricing.py`, `src/invoice.py`, and
  `src/analytics.py`.
- `src/legacy.py` has a same-named `apply_discount` and must not be changed.
- `src/dynamic.py` resolves the function at runtime; it does not encode the
  rate and does not need to change.
