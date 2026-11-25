# M2.5-D adherence experiment prompts (2026-11-24)

Both conditions received the same task, the same repository snapshot and the
same tool shim. Only the rule block differed. No HTTP endpoint list or JSON
call sheet was supplied.

## Fixture

- 17 files, 16 Python modules; indexed snapshot
  `sha256:d13abca22ecdf740ff386a423cb282de3e2b38e7dffccbaa80640d3d090aef6e`
  (17 files, 44 entities, 49 relations, coverage complete).
- Ground truth callers: `pricing.list_price`, `checkout.checkout`,
  `wrapper.summarize_order`, `analytics.check_price`, and the indirect
  `reporting.build_report`; `dynamic.dynamic_discount` is dynamic/unknown.
- `legacy.apply_discount` is a same-named homonym, not a caller;
  `invoice.invoice_total` imports but never calls `apply_discount`.
- Known stale-value assumptions: `analytics.check_price` compares to 90;
  `invoice.EXPECTED_DISCOUNT = 0.9`.

## Common task block

```text
You are working in a disposable copy of a Python repository. Work only inside <run>/repo.

TASK
1. Apply this change in src/pricing.py: `return value * 0.9` -> `return value * 0.8`.
   Use `"<python>" "<tools>/apply_fix.py" "<run>/repo"` to apply it. Do not edit
   src/pricing.py directly.
2. Determine every function that can reach `apply_discount`, directly or
   indirectly, with the file and 1-based line of the call or definition.
3. Identify any code that assumes the old 0.9 value.
4. State whether each relation is confirmed, candidate, or unknown.

You may run read-only shell commands (rg, sed, find, git) and the ARCHMELD
tools. Do not run the project's tests.

ARCHMELD tools:
`"<python>" "<tools>/archmeld_tool.py" --config "<run>/tool.json" <tool> '<json-args>'`
Available tools: status, brief, review, query, source, context, inspect,
capabilities.

<RULES>

Return ONLY one JSON object as your final message, no markdown fence:
{"edit_applied":true,"callers":[{"function":"...","file":"...","line":0,"kind":"direct|indirect|dynamic","status":"confirmed|candidate|unknown"}],"assumptions":[{"file":"...","line":0,"what":"..."}],"archmeld_used":true,"notes":"..."}
```

## Condition A rules (pre-M2.5)

```text
ARCHMELD is available as MCP tools. Use it before multi-file grep when the task
needs call relationships, snapshot-bound changes, or evidence instead of guesses.

Rules:
- Call `archmeld_status` first when the snapshot or working-tree state is unknown.
- `verification=not_run` means the review is static; ARCHMELD never claims tests passed.
- `candidate` relations are syntax evidence, not resolved calls. Never present them as confirmed.
- `coverage=partial` or `unknown` means unknown, not absent.
- Read exact bytes with `archmeld_source`; a context snippet is not the full body.
- ARCHMELD is read-only: it does not edit, apply, run tests, or approve changes.
```

## Condition B rules (M2.5-A session contract)

```text
ARCHMELD is the default evidence source for this repository. Do not skip it
because a task looks local: the snapshot records the working tree and the
change's affected callers.

### Session start (before planning or editing)
1. Call `archmeld_status` once. If a snapshot exists, call `archmeld_brief`
   with `recipe=auto`.
2. Put the brief's counts (confirmed/candidate/unknown/stale), findings,
   unknowns and next_actions into your plan. Cite the snapshot id and coverage
   when they change the answer.
3. Before editing a symbol, call `archmeld_query` with `kind=impact` for that
   symbol.

### Before you finish
4. Call `archmeld_review` and `archmeld_brief` with `recipe=change`.
5. Report the changed entities, affected callers/relations, and any candidate,
   unknown or stale facts. Never claim verification you did not run.

### Rules
- `candidate` relations are syntax evidence, not resolved calls. Never present
  them as confirmed.
- `coverage=partial` or `unknown` means unknown, not absent.
- `verification=not_run` means static review; ARCHMELD never claims tests passed.
- Read exact bytes with `archmeld_source`; a context snippet is not the body.
- ARCHMELD is read-only: it does not edit, apply, run tests, or approve.
- If the tools are unavailable, say so explicitly; do not imply ARCHMELD was
  consulted.
```

## Measurement

`archmeld_tool.py` spawns the real `archmeld-mcp` process for each call and
appends `{ts_ms, tool, args, ok}` to `calls.jsonl`. `apply_fix.py` appends the
edit timestamp to `edit.jsonl`. A call is "before edit" only if its timestamp
is strictly earlier than the first edit timestamp. `results.json` contains the
per-run metrics and aggregate table; `analyze.py` regenerates it from the logs.

## Result

- Full ritual (status + brief + impact before edit; review + change brief
  after): **B 3/3, A 0/3**.
- Brief before edit: **B 3/3, A 1/3**. Impact before edit: **B 3/3, A 2/3**.
- Change brief after edit: **B 3/3, A 1/3**.
- Mean tokens: **B 680,358 vs A 722,590 (0.942x)**. Mean ARCHMELD calls:
  B 25.0 vs A 39.0, with B shifting work before the edit (18.7 vs 12.7) and
  doing less after (6.3 vs 26.3).
- Correctness: 6/6 runs reported the five real callers and the dynamic
  unknown; no run presented a candidate edge as confirmed.
- Real defect found: 5/6 runs detected that `from pricing import apply_discount`
  is mis-bound to `legacy.apply_discount` in the graph (B 3/3, A 2/3). This is
  recorded as a product gap, not counted as an agent error.
- Verdict: the ADR-057 success line is met (B >=2/3 before-edit brief;
  tokens <=1.3x; no correctness/candidate regression). n=3 per arm, one model;
  the result is directional, not a general benchmark.
