# E4a smoke payload archive

The seven-run `smoke/` series was moved out of the worktree on 2026-11-27.

- Moved: `docs/evidence/e4a-2026-11-25/smoke/` (195 files, 142 directories,
  5,102,076 bytes).
- Destination: repository-sibling
  `_archmeld_evidence_archive/e4a-2026-11-25/smoke/`.
- Integrity: `SHA256SUMS` at the destination root covers all 195 files.
- Nothing in this directory was deleted. The 51 tracked files (`RESULT.md`,
  `STATUS.md`, `PROTOCOL.md`, prompts and `runs/**`) remain in place because
  `handoff_check.py` and `baseline/commands.ndjson` cite them.

Why this payload is safe to move:

- `smoke/` is untracked and ignored by the directory-local `.gitignore`
  (`smoke/`, `runs/*/repo/`, `runs/*/state/`, `__pycache__/`).
- No gate, ledger command, release evidence or `RESULT.md` reference points into
  `smoke/`.
- The 5.2 MB contains seven exploratory smoke runs, 59 CAS objects and seven
  SQLite ledgers; the four clean runs cited by the E4a result are under
  `runs/`, not under `smoke/`.

Restore:

```sh
mv <archive-root>/e4a-2026-11-25/smoke docs/evidence/e4a-2026-11-25/smoke
cd docs/evidence/e4a-2026-11-25
sha256sum -c <archive-root>/e4a-2026-11-25/SHA256SUMS
```

The archive root is a sibling of this repository
(`_archmeld_evidence_archive`), not part of the public snapshot.
