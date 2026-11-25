# E2.1 semantic boundary labels (2026-11-25)

Scope: make every map read surface say what it did and did not verify.

- `map check` / `inspect(map_entry)` / `map maintain`: `semantic_claims=unverified`,
  `verification=not_run`, `validation=refs_checked`.
- `summarize` (status / project brief) and `affected` (change brief):
  `semantic_claims=unverified`, `verification=not_run`, `validation=not_run`.
- `brief(recipe=map)`: top-level `map_state` is separate from the brief's own
  `state`; an invalid map no longer hides behind `brief.state=ok`.
- Map markdown prints the labels explicitly.

Evidence:

- `final-core-api.log`: full core/API regression, 239 passed / 0 failed /
  0 ignored (78 sections), exit 0, sha256
  `f9bdf218f1ba3766b066c05aeee6d19650de11a7191517d6a63d6ca6df1771ce`.
- `docs/release-evidence/E2.1-SEMANTIC-LABELS-2026-11-25.json`: structured
  record, including the discarded first run (missing Python/libclang on PATH).

Boundaries: this does not validate F/A/S semantics, does not write the map,
does not add an MCP tool, and does not start E4a/E3/E4b.
