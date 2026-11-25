# E4a protocol: constraint-seeded semantic value

Date: 2026-11-25. Status: **run and no-go**. See `RESULT.md`.

## Question

Does a change-scoped, evidence-bound map Entry change an agent's decision
when the constraint is not recoverable from grep or source alone?

## Design

- Two fixtures: `nonlexical` (map-only coupling) and `lexical` (greppable
  control). See `fixtures/e4a/README.md`.
- The first nonlexical draft named the coupled modules in `README.md` and had
  only 7 files; `nonlexical-a1` passed without the map, so that draft is
  invalid for the map-only claim. The revised nonlexical fixture has 26 files
  (7 core + 18 service decoys + README), a generic README, and no comments
  in `invoice.py`/`analytics.py` that name the pricing rate. Runs named
  `nonlexical3-*` use it.
- Two arms per fixture:
  - A: `archmeld_brief(recipe=change)` + `source` + read-only shell; no map.
  - B: map present; rule requires `archmeld_brief(recipe=change)` and
    `archmeld_inspect(kind=map_entry)` for each file about to be edited,
    before editing. The change brief can be empty before the first edit
    because no code has changed yet; the Entry read is the load-bearing step.
- Task: change the discount from 10% to 15%, keep invoice and analytics
  consistent, do not run tests, return a fixed JSON object.
- n=3 runs per cell initially. Nonlexical runs first; if the result is mixed,
  pre-register an extension to 6 per cell. The lexical control runs only if
  nonlexical passes, to interpret whether the map adds value beyond grep.

## Execution

Backend: `pi` 1.0.4 with `omni-anthropic/deepseek-v4.1-flash`, thinking max,
durable session in `<run>/pi-sessions`. `.pi/mcp.json` uses `exposure:
direct` and `toolExposure: {"*":"direct"}`; the launcher passes
`--no-extensions -e builtin:mcp` so broken global extensions do not block the
run and MCP stays enabled.

Run root: `<workspace>/<e4a-run>` (outside the git
worktree). A run repo inside `docs/evidence/` would be ignored by the
evidence `.gitignore`, and `archmeld init` would capture no files.

```sh
python tooling/e4a_launcher.py --probe
python tooling/e4a_launcher.py --backend pi --fixture nonlexical --arm a \
  --out <workspace>/<e4a-run>
python tooling/e4a_launcher.py --backend pi --fixture nonlexical --arm b \
  --out <workspace>/<e4a-run>
```

The launcher:

1. copies the fixture to `runs/<id>/repo`;
2. runs `archmeld init` with an isolated `--state`;
3. for B, writes `archmeld.map.txt` with the run snapshot and runs
   `map check` (`state=ok` required);
4. writes a per-run `mcp.json` and runs Claude Code with
   `--strict-mcp-config`, `--output-format stream-json`,
   `--permission-mode bypassPermissions`, and no session persistence;
5. records `stdout.jsonl`, `stderr.log`, `calls.jsonl`, `shell.jsonl`,
   `edits.jsonl`, `audit.jsonl`, tree hashes, and `run.json`.

The server audit log is authoritative for MCP outcomes. Tokens/cost are
recorded only when the client reports them; otherwise they are `null`.

## Grading

`python tooling/e4a_grade.py --run runs/<id> --fixture nonlexical`

- Behavior: `apply_discount(100) == 85`, `invoice_total(100) == 85`, and the
  analytics check passes. This accepts equivalent implementations.
- Files: `src/pricing.py`, `src/invoice.py`, `src/analytics.py` must change;
  `src/legacy.py` must not.
- Map use: count `archmeld_inspect(kind=map_entry)` and
  `archmeld_brief(recipe=change)` from the client stream; record whether the
  pricing Entry was inspected before the first edit. A change brief before
  the first edit may contain no affected entries; that is expected.
- Final answer: record `constraints_applied` and `map_used`; do not trust
  them over behavior.

## Go / no-go

Nonlexical:

- B updates invoice and analytics in >=2/3 runs; A <=1/3.
- B declares both constraints in >=2/3 runs.
- B tokens <=1.3x A (when both are reported).
- 0 candidate promotions; 0 unsupported verification claims.

No-go: no behavior difference. Then E stays at E2; do not build E3 and do not
install rule v3.

## Boundaries

- The map is model-authored; `semantic_claims=unverified` is expected.
- The agent may read `archmeld.map.txt` directly; the transcript records
  shell commands when the client exposes them. Do not claim the MCP surface
  was the only path.
- The map file is added to the repo after indexing; `review` shows it as a
  new file and the grader ignores it.
- No network upload, no source upload, no keys in argv or logs.
