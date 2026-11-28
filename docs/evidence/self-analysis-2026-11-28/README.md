# Self-analysis acceptance — 2026-11-28

Clean-worktree self-analysis at HEAD `e3f9ad2b` using the release binaries built
at `fd5c8758`. The wrapper ran index, status and the 24-check acceptance before
`run_cmd` appended its own record, so the snapshot and the observed status are
clean (`status_before=[]`, `status_after=[]`).

## Inputs

- Snapshot: `sha256:df7eb174e9700e0865843b6f05da7c0d466c20ee69beee8496e0ef0449143f15`
- Run: `run.archmeld-self.index...` (`RUN.json`), `index_mode=full_parse`
- Files: 2017; entities: 11,286; relations: 47,531
- Parse: 803 files (rust 576, python 74, typescript 53, kotlin 39, toml 49,
  bash 8, cpp 4), 0 diagnostics, 47 implements (25 deferred resolved, 0 ambiguous)
- Examples: `--include-dir examples` retained 69 example-anchored entities
- Kotlin: 3 recorded parser errors under `jetbrains/`

## Acceptance

`acceptance.json`: **24 passed / 0 failed**, including:

- status/review clean, no stale-change finding;
- MCP status/review summary stay under the declared byte budgets;
- `archmeld_context`, impact/filter semantics, `Store` lookup and role filters;
- auto brief performs exactly one review;
- helper `--schema=1.2` and the missing-DLL exit hint.

## Limitations

- The snapshot describes product source at `fd5c8758`; commits after it are
  tests/tooling/docs and do not change the analyzed crates. The snapshot itself
  is at `e3f9ad2b` (clean tree).
- The `run_cmd` record is appended after the batch, so it is not part of the
  observed clean state.
- Self-analysis measures the current repository, not a general performance or
  correctness guarantee.

## Command

`self-analysis-2026-11-28-clean` (see `COMMANDS.json`).
