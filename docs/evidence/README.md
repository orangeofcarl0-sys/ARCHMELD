# Execution evidence

This directory holds per-task execution evidence. It is a historical record, not
the current status. Current status lives in
[`../HANDOFF_CHECKLIST.md`](../HANDOFF_CHECKLIST.md); release gates live in
[`../release-evidence/`](../release-evidence/).

## Three evidence stores

| Store | Location | What it proves |
|---|---|---|
| Command ledger | [`../../baseline/commands.ndjson`](../../baseline/commands.ndjson) | Every recorded command with cwd, argv, exit code, duration, tool versions and log hash |
| Per-task evidence | `docs/evidence/` (this directory) | A task run: inputs, raw outputs, summaries and checksums |
| Release gates | [`../release-evidence/`](../release-evidence/) | The evidence a release or acceptance gate cites |

A task directory is self-contained. A release gate may cite a task directory,
but it must also have its own stable file so a later re-run does not change the
gate's meaning.

## Naming

- Dated task directory: `<task>-YYYY-MM-DD/`. One directory per run batch; a
  re-run on a different date gets a new directory.
- Gate file: `B0xx-<subject>.json` or `AT0xx-<subject>.json` at the top level or
  in `../release-evidence/`.
- A JSON file records its own `schema_version`, `recorded_at`, HEAD/snapshot and
  result. A `COMMANDS.json` or `SHA256SUMS` next to it binds the raw outputs.
- Screenshots are kept locally but are omitted from the public snapshot because
  they can contain workstation UI and paths.

## Dated task directories

| Directory | Records |
|---|---|
| `agent-comparison-2026-11-03/` | Same-agent comparison fixture and grade result |
| `m1-onboarding-2026-11-11/` | M1 onboarding (`init`, `mcp install`, `brief`) |
| `m2.5a-adherence-2026-11-24/` | M2.5-A session-start adherence |
| `m2.5b-session-card-2026-11-24/` | M2.5-B session card |
| `m2.5d-adherence-2026-11-24/` | M2.5-D adherence experiment |
| `e1-map-check-2026-11-25/` | E1 cognition-map `check` and `status` |
| `e2-map-maintain-brief-2026-11-25/` | E2 `map maintain` and `brief(recipe=map)` |
| `e2.1-semantic-labels-2026-11-25/` | E2.1 semantic labels |
| `e2.2-read-surfaces-2026-11-25/` | E2.2 read surfaces |
| `e4a-2026-11-25/` | E4a map/no-map experiment (result: no-go) |
| `python-binding-2026-11-25/` | Python binding evidence |
| `python-alias-binding-2026-11-25/` | Python alias binding evidence |
| `upstream-0.21.2-2026-11-24/` | 0.21.2 upstream upgrade drill |
| `upstream-registry-2026-11-25/` | Upstream registry checks |
| `upstream-contracts-2026-11-26/` | Upstream contract checks |
| `upstream-probes-2026-11-26/` | Upstream probe runs |
| `upstream-rollback-2026-11-26/` | Upstream rollback drill |
| `privacy-decoupling-2026-11-26/` | Privacy decoupling and public snapshot review |
| `self-analysis-2026-11-26/` | Self-analysis (P01–P25 discovery) |
| `self-analysis-fix-2026-11-27/` | Self-analysis repair acceptance |

## Current gates and machine readers

Most dated directories are historical. The files a current gate or tool reads
are:

- `B030-language-matrix.json`, `B030-runtime-linkage.json`,
  `B030-medium-projects.json`, `B030-rename-apply.json` — read by
  `tooling/verify_b030_evidence.py` and `test_lane.py --lane evidence`.
- `B030-case-b-scenarios.json`, `B031-install-tests.json`,
  `B031-release-notes.md`, `SELF-ANALYSIS-FIX-2026-11-27.json` — read by
  `handoff_check.py` / release tests.
- `UPSTREAM-*.json` — scanned by `handoff_check.py` and `test_release.py` for
  the AT077 status.
- `e4a-2026-11-25/RESULT.md` and its two `run.json` files — cited by
  `baseline/commands.ndjson` record `e4a-nogo-2026-11-25` and checked by
  `handoff_check.py`.

Everything else under `docs/evidence/` is a historical run. It stays available
for reproduction; it is not current truth.

## E4a smoke payload

`docs/evidence/e4a-2026-11-25/smoke/` (195 untracked files, 5.1 MB) was moved
to the repository-sibling `_archmeld_evidence_archive/e4a-2026-11-25/smoke/` on
2026-11-27. See
[`e4a-2026-11-25/SMOKE-ARCHIVE.md`](e4a-2026-11-25/SMOKE-ARCHIVE.md) for the
inventory, checksum manifest and restore command. The 51 tracked files in that
directory were not moved.

## Deliberate duplicates

- Six JSON files are mirrored between `docs/evidence/` and
  `docs/release-evidence/` (`B017-native-static`, five `B030-*`). The five B030
  tests write both paths on purpose; do not deduplicate them without changing
  those writers and re-running the gate.
- `archive/2026-10-31-b030/` duplicates the four superseded B030 files in
  `docs/release-evidence/archive/`; the archive is intentionally frozen.
- Three screenshots are byte-identical pairs (`b026-workbench-clean-2/3`,
  `b026-workbench-clean` and `b026-workbench-paint-1/3`). They are local-only
  and omitted from the public snapshot.

`ui-state/`, `ui-trace-state/`, `ui-trace-repo/` and `ui-export.ndjson` are
local-only or ignored. `ui-import.json` has no tool or document reference.

## Archive

`B0xx-*.json` files are task-level summaries. They complement the dated
directories; for example `B030-language-matrix.json` is the gate summary and
`self-analysis-fix-2026-11-27/` contains the repair run.

`perf-2026-11-27.json` records the Windows build/test performance baseline and
the effect of each optimization layer.

## Archive

`archive/` holds superseded evidence that a later run replaced, currently the
2026-10-31 B030 files. Do not move active evidence into the archive without
recording the superseding path in the task's progress entry.

## Rules

- Append, do not rewrite. A different result creates a new dated file or
  directory; it does not edit the old result.
- Every claimed pass needs a real command record. A screenshot or a prose note
  is not sufficient.
- Evidence must not contain model keys, service tokens or repository secrets.
- Public publication uses `tooling/public_snapshot.py`; the policy omits
  screenshots, local logs, SQLite ledgers, compressed files, trace binaries,
  content-addressed blobs and the E4a model transcripts.
- Do not move a tracked evidence path. `handoff_check.py` checks the paths cited
  in `docs/archmeld-progress.md`, and `baseline/commands.ndjson` records the
  original argv. Archive only untracked payload, as the E4a smoke archive does.
