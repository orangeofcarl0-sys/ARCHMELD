# E1 `map check` evidence (2026-11-25)

The map is read-only input. `archmeld map check` parses the header, sections
and file-level Entries, resolves `file:`/`entity:` refs against the snapshot,
and exits 1 when the report state is `invalid`. `map status` reports the same
facts (and supports `--markdown`), with the same invalid/ok exit rule; absent
or stale maps exit 0.

## Commands

```sh
# valid map: exit 0, refs confirmed=3
archmeld map check --state <state> --project project.m1 --workspace <repo>

# one orphan entity ref: exit 1, refs.orphan=1
archmeld map check --state <state> --project project.m1 --workspace <repo>

# markdown status: exit 1 on the same invalid map, 0 for ok/stale/absent
archmeld map status --state <state> --project project.m1 --workspace <repo> --markdown
```

## Files

- `map-ok.txt` / `check-ok.json`: valid map and report.
- `map-orphan.txt` / `check-orphan.json`: `entity:function:src/a.py::missing`;
  exit 1, `ref_orphan`.
- `status-orphan.md`: markdown report for the same invalid map.

The fixture is the M1 4-file repository; `README.md` and `main.py` have no
Entry and are reported as coverage gaps, not errors. The map file itself is
metadata and is skipped by coverage.

## Limits

E1 does not resolve `relation:` refs (deferred until the Python same-name
binding is fixed), does not write maps, and does not run `map maintain` or
`brief(recipe=map)`.
