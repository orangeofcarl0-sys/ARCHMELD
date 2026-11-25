# E2.2 read-surface fixes (2026-11-25)

Scope: make the existing read-only map surfaces usable by E4a and E3 without
adding writes or tools.

- `affected()` now returns each Entry's `api`, `constraint`, and `refs`, plus
  top-level `state`, `snapshot_match`, `map_digest`, and `binding` counts.
- `check_map`, `summarize`, `maintain`, and `entry_report` expose `map_digest`;
  `check_map`/`summarize`/`maintain` also expose `freshness`.
- `entry_report` returns `absent` for a missing map instead of a storage error;
  `maintain` returns `needs_snapshot` instead of failing.
- `maintain --markdown` uses `current_snapshot`/`map_snapshot` and the maintain
  entries block.
- The MCP contract's `implemented_tools` now includes `archmeld_brief`.

Evidence:

- `final-core-api.log`: 240 passed / 0 failed / 0 ignored (78 sections),
  exit 0, sha256
  `19048831b0569660f7239d4a3003503514765ecd301d8c165e2bbfa085b8f4b8`.
- `docs/release-evidence/E2.2-READ-SURFACES-2026-11-25.json`.
- Smoke map: `smoke/runs/smoke-nonlexical-3` (`map check` state ok, 7 entries,
  7 confirmed refs; `inspect map_entry` returns the pricing `S`).

Boundaries: no map writes, no MCP tools added, `semantic_claims=unverified`.
E4a execution is not included; Claude Code is installed but not authenticated
in this environment.
