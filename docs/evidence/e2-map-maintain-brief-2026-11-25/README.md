# E2 `map maintain` / `brief(recipe=map)` evidence (2026-11-25)

E2 keeps the map read-only and adds the maintenance/read surfaces:

- `archmeld map maintain`: compares the map header snapshot with the requested
  snapshot, lists stale/unbaselined entries, missing files, orphan refs, and
  whether the map file itself is tracked and dirty.
- `archmeld brief --recipe map`: a one-screen map brief with state, entry
  counts, confirmed/candidate/unknown refs and next actions.
- `archmeld status`: each project with a workspace gets a `map` summary.
- `archmeld_inspect(kind=map_entry, id=<path>)`: one Entry with its refs and
  check issues.
- Project/change briefs add a `cognition_map` block (summary or affected
  entries) only when `archmeld.map.txt` exists.

## Commands on the M1 fixture

```sh
archmeld map maintain --state <state> --project project.m1 --workspace <repo>
archmeld brief --state <state> --project project.m1 --workspace <repo> --recipe map
archmeld status --state <state> --project project.m1 --workspace <repo>
```

Results:

- `maintain.json`: state `ok`, 2 entries, 2 unchanged, 0 stale, 0 orphan,
  `check_state=ok`; `missing=2` because `README.md` and `main.py` have no Entry
  and are not observed/excluded; `map_dirty=false`, `map_tracked=false` because
  the map was written after the M1 snapshot.
- `brief-map.json`: state `ok`, level `map`, summary
  `map ok: 2 entries, 3 confirmed refs, 0 candidate, 0 unknown`.
- `status.json`: `projects[0].map` reports `present=true`, `entries=2`,
  `snapshot_match=true`, `state=ok`, `tokens_estimate=63`.

All three commands exited 0. The map was not written.

## Tests

- `crates/archmeld-core/tests/cognition_map.rs`: 10/10, including
  `maintain_marks_changed_files_after_a_new_snapshot`.
- `crates/archmeld-core/tests/brief.rs`: 5/5, including
  `map_recipe_and_cognition_map_are_read_only`.
- `crates/archmeld-api/tests/mcp_stdio.rs`: 1/1, now also inspects a
  `map_entry` through the real MCP process.
- Full core/API regression is recorded in
  `docs/release-evidence/E2-MAP-MAINTAIN-BRIEF-2026-11-25.json`.

## Limits

`map maintain` compares snapshots, not the live working tree; use `review` for
uncommitted edits. `map_dirty` only applies when the map is in the snapshot.
E3 (`map apply`) and E4 (real-repository pilot) are not implemented.
