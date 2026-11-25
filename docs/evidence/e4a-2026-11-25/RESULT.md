# E4a result: no-go (2026-11-25)

Verdict: **no-go**. The evidence-bound map did not change decisions enough
on the non-lexical fixture. E stays at the read-only E2 surface; `map apply`
(E3), the 50-100 file pilot (E4b) and rule v3 are not promoted.

## Design iterations

| Draft | Fixture | Problem | A result |
|---|---|---|---|
| 1 | 7 files; README named invoice and analytics and said derived values existed | leaked the coupling | `nonlexical-a1` passed without the map |
| 2 | 25 files with decoys; `invoice.py`/`analytics.py` comments named the pricing rate | leaked the coupling in comments | `nonlexical2-a1` passed without the map |
| 3 | 26 files (7 core + 18 service decoys + README); generic README; no coupling comments; `INVOICE_ADJUSTMENT_BPS` | the clean test | A 2/3, B 1/1 |

Only draft 3 is used for the verdict; drafts 1-2 are design failures, not
no-go evidence.

## Clean nonlexical runs

Backend: `pi` 1.0.4, `omni-anthropic/deepseek-v4.1-flash`, thinking max,
durable sessions. A has no map; B has the map and the rule to inspect each
target Entry.

| Run | Arm | Behavior | Files changed | Map inspected | Tokens | Duration |
|---|---|---|---|---|---|---|
| `nonlexical3-a1b` | A | pass | pricing, invoice, analytics | 0 | 94,909 | 325.7 s |
| `nonlexical3-a2` | A | fail | pricing, analytics (invoice missed) | 0 | 149,577 | 76.7 s |
| `nonlexical3-a3` | A | pass | pricing, invoice, analytics | 0 | 213,085 | 121.6 s |
| `nonlexical3-b1` | B | pass | pricing, invoice, analytics | 3 Entry reads | 74,792 | 87.2 s |

Aggregate:

- A: 2/3 correct; mean 152,524 tokens.
- B: 1/1 correct; 74,792 tokens, 3 `map_entry` inspect calls.
- Go/no-go required B ≥2/3 and A ≤1/3. A=2/3 violates the gate, so the
  experiment is no-go. B2/B3 were not run after the gate failed; the result
  is directional and B is not a full 3-run arm.

## Why this is a valid no-go

The A arm found the two coupled modules without the map in two of three
runs. It used ARCHMELD `query`/`review` and ordinary reading; it left the 18
service decoys unchanged. The map's `S` names the coupling, but on this
fixture the rest of ARCHMELD plus the task instruction was sufficient. B was
cheaper in its one run, but "cheaper when it works" does not offset the fact
that A already made the right decision most of the time.

The first two drafts were invalid because their comments/README stated the
coupling. A pass on those drafts is not evidence against the map.

## Consequences

- E remains read-only: `map check|status|maintain`, `brief(recipe=map)`,
  `status.map`, `inspect(kind=map_entry)`.
- `map apply` (E3) is not implemented.
- E4b (50-100 files) and rule v3 are not started.
- The map stays optional; no session rule requires reading it.
- The `relation:` ref and per-symbol alias gaps remain separate items.

## Limitations

- One model family (the map was authored by the same family), n=3 A / n=1 B.
- 26-file synthetic Python fixture, not a real repository.
- A had full ARCHMELD read tools; the result compares the map to the rest of
  ARCHMELD, not to grep alone.
- The task explicitly asks to keep behavior correct, which prompts a thorough
  search; a different task wording could change the decision economics.
- This does not prove maps never help; it shows this design did not justify
  the write path or a rule mandate.
