# Upstream rollback procedure

Status: U5 governance, 2026-11-26. Read [`UPGRADE_PLAYBOOK.md`](UPGRADE_PLAYBOOK.md)
first. A rollback is a new commit that reverses an upgrade; it is never a
history rewrite.

## Three meanings

| Type | Use when | Mechanism | Evidence |
|---|---|---|---|
| Binary rollback | a release regression is found | reinstall the previous release bundle; project data is untouched | B031 install/upgrade/rollback evidence |
| Source rollback | an upstream merge broke build/contract/state-root | `git revert -m 1 <merge-commit>` on `archmeld/initial` | new rollback evidence |
| Pin correction | only the registry pointer is wrong | edit the registry/locks/NOTICE together | only valid with source provenance |

Changing only `upstreams.lock.json` is **not** a rollback: the source tree,
patch manifest and lock would disagree.

## Preconditions

- [ ] Previous release artifacts and `SHA256SUMS` are retained.
- [ ] The pre-upgrade commit/tag and `upstreams.lock.json` pin are recorded.
- [ ] The current worktree is clean; no user changes are overwritten.
- [ ] `docs/archmeld/**` is clean and remains sealed.
- [ ] The old upstream state is the merge's **first parent**, not the bare
      upstream commit. For 0.21.2 that is `903bfe0a` (fork with local patches);
      `590c2ca7` is the pure upstream commit and has no ARCHMELD patches.

## Source rollback (CodeGraph)

Use the merge commit from the upgrade evidence. For the 0.21.2 upgrade the merge
is `1ca0ae8b`; a future upgrade must use its own merge commit.

```sh
git switch archmeld/initial
git status --porcelain            # must be empty
git revert -m 1 1ca0ae8b          # creates a new revert commit; never reset
```

If the revert conflicts:

1. List conflicts with `git diff --name-only --diff-filter=U`.
2. Resolve them deliberately; preserve upstream tests, local patches and
   `--archmeld-export-v1`/`--archmeld-state-root`.
3. Do not use `git reset --hard`, `git clean -fd`, force push or checkout of
   the public snapshot.
4. Record every conflict and resolution in the rollback evidence.

For the 0.21.2 rollback, expected conflicts include
`crates/codegraph-server/src/watcher.rs` and `UPSTREAM_PATCHES.md`; later local
commits touched those files. A conflicted revert is a finding, not a failed
drill.

## Verify

```sh
cargo test -p archmeld-core --test bridge_import --offline -j 2
cargo test -p codegraph --test archmeld_probes --no-default-features --offline -j 2
cargo test -p archmeld-core -p archmeld-api --offline --no-fail-fast -j 2
```

Also run the export/state-root/MCP smoke used by the upgrade. Unit tests alone
are not a rollback verification.

## Reconcile provenance

After the revert:

```sh
python tooling/upstream.py patches --write
python tooling/upstream.py patches --check
python tooling/upstream.py verify --skip-cargo --json
```

Update, in the same commit:

- `upstreams.lock.json` pin/version/checked date;
- `toolchains.lock.json` `codegraph_commit`;
- `AGENTS.md`, `UPSTREAM_PATCHES.md`, `third_party/NOTICE.md`;
- `patches/codegraph/manifest.json` `base_commit` and diff hashes.

Do **not** run `upstream.py record` for a rollback. It assumes a forward pin and
would write the local fork commit `903bfe0a` into the patch manifest as if it
were the upstream base, and mark `last_drill` as passed AT077.

## Rollback drill

The drill builds the old tree and probes the revert path without changing the
main worktree:

```sh
python tooling/upstream.py rollback-drill codegraph \
  --to 903bfe0a --run --matrix full \
  --probe-revert 1ca0ae8b \
  --json --out <temp>/rollback-drill.json
```

- `--to` must be an ancestor of HEAD; the command refuses otherwise.
- The old tree is built in a temporary worktree; `--matrix full` includes
  core/API, bridge, `archmeld_probes --no-default-features`, PHP, server
  contract tests and the export example.
- `--probe-revert` runs `git revert --no-commit -m 1 <merge>` in a second
  temporary worktree, records conflicts, aborts and removes it. Conflicts do not
  change the drill `status`.
- The command never updates the registry, locks, NOTICE or patch manifest and
  never calls `record`.

On Windows GNU, replaying an old tree built before the UCRT64 migration can make
`codegraph-php` and `archmeld_state_root` fail with `0xC0000139`
(STATUS_ENTRYPOINT_NOT_FOUND): the old test binaries expect winlibs/Strawberry
`__emutls_v` libstdc++ symbols. Rebuild with UCRT64 16.1.0 before replaying;
this is not a RocksDB runtime limitation. Use `--matrix quick` as the Windows
gate, and run the PHP/server part of `--matrix full` on the Linux worker only
when a Linux worker is otherwise required. Record which matrix ran and every
failure; do not drop a test to make the drill green.

## Evidence naming

Use `docs/release-evidence/ROLLBACK-DRILL-YYYY-MM-DD.json` and
`docs/evidence/upstream-rollback-YYYY-MM-DD/`. Do **not** name it
`UPSTREAM-*.json` with an `AT077` block: `handoff_check.py` and
`test_release.py` select the newest such file and it would shadow the real
upgrade record.

## Prohibited

- `git reset --hard`, `git clean -fd`, force push;
- deleting or weakening tests to make the rollback green;
- merging or checking out the public snapshot as history;
- editing `docs/archmeld/**` without re-sealing;
- treating B031's binary installer rollback as proof of a source rollback.
