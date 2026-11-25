# M2.5-B session card evidence (2026-11-24)

`archmeld brief --write` writes `.archmeld/SESSION.md` with a header carrying
the snapshot, commit, coverage, workspace state, counts and
`generated_at_unix`. `archmeld status` reports `session_card.present`,
`marker`, `snapshot_match` and `stale` for the active workspace.

## Clean fixture

`<temp>/m2.5b-clean` (2 Python files, indexed with
`archmeld init`):

- `clean-brief-write.json`: card written.
- `clean-status.json`: `session_card.present=true`, `snapshot_match=true`,
  `stale=false`.
- `dirty-status.json`: after appending a comment to `src/a.py`,
  `stale=true` while `snapshot_match` stays true.
- `clean-SESSION.md`: the written card.

The snapshot/drift walkers now skip `.archmeld/`, so the generated card itself
does not make the workspace stale. `crates/archmeld-core/tests/session_card.rs`
covers this with a real store and snapshot.

## M1 fixture

- `status-before.json`: before the card was written.
- `brief-write.json` / `SESSION.md`: card written on the M1 fixture.
- `status-with-card.json`: `present=true`, `marker=true`,
  `snapshot_match=true`, `stale=true` because that fixture already had a real
  `src/b.py` modification.

## Limits

The card is a projection of the brief, not a source of truth and not a call
graph. A missing card is not stale; a non-ARCHMELD file is only overwritten
with `--force`.
