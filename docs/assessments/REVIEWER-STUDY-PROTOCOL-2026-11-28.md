# Reviewer study protocol (not yet run)

Status: the human study has not been run. An agent-only adaptation (two
durable `pi` sessions, 12 sessions) is recorded in
`docs/evidence/reviewer-pilot-2026-11-28/`; it does not support a human-review
claim.

## Question

Does a bounded ARCHMELD change brief help a reviewer locate the correct owner
and detect redundant execution paths faster than a Git diff alone, without
increasing unsupported or unreproducible conclusions?

## Arms

1. Git diff only.
2. Git diff plus a bounded `archmeld_brief` and, where relevant, a persisted
   `review-diff` structural result.

The brief budget is fixed before the study (`findings=20`, `context=10`,
`gaps=10`, `relation_items=10`). Reviewers may open cited source and run the
project's own tests; they must not treat `candidate` relations as resolved.

## Fixtures

| Fixture | Purpose |
|---|---|
| File move with unchanged callers | Move is not a new implementation |
| Changed function with an untouched caller in another file | Context is distinguished from change |
| Two acyclic paths converging on one settlement target | Redundant path detection |
| Removed entry point whose callers use a shared implementation | Convergence and unresolved callers |
| Ambiguous same-name call | Candidate remains candidate |
| Stale/partial index | Coverage and verification notices survive |
| Unmapped file | Gap is named, not hidden |

Fixtures must be synthetic or public; do not use a private repository without
written permission. Each fixture records the base snapshot, candidate snapshot
or workspace digest, and the command that produced the diff.

## Measures

- Time to the correct owner (first answer judged by a pre-written key).
- Missed affected entities.
- Conclusions not supported by the cited source or project tests.
- Whether each conclusion names a reproducible source location and snapshot.
- For structural fixtures: whether both paths are found, whether a write claim
  is marked unavailable when no writes evidence exists, and whether a
  candidate path is labelled a hypothesis.

## Procedure

1. Pre-register the fixture key and the acceptable owner/path answer.
2. Randomize arm order per reviewer; do not reveal the arm hypothesis.
3. Give the same task text, time limit and tools in both arms.
4. Record start/end timestamps, the final answer, cited evidence and any test
   run by the reviewer.
5. A second person scores the answer against the key without seeing the arm.
6. Report descriptive statistics and the raw records. Do not claim statistical
   significance with a small sample.

## Evidence shape

One directory per study batch:

```
docs/evidence/reviewer-study-YYYY-MM-DD/
  PROTOCOL.md
  key.json
  runs/<reviewer>-<fixture>-<arm>/run.json
  RESULT.md
```

`run.json` records reviewer id (pseudonymous), fixture, arm, start/end, answer,
cited snapshot/source references, test commands, and whether the answer matched
the key. `RESULT.md` is the aggregate table and limitations.

## Stop conditions

- A fixture answer is ambiguous or the key is disputed.
- The brief or review output exceeds the declared budget without reporting
  omissions.
- A reviewer cannot reproduce a cited source from the recorded snapshot.

Stop the batch and fix the fixture or output before collecting more data. Do not
discard failed runs; keep them in the record.
