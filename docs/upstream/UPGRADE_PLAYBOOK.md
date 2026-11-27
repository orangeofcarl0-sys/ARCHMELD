# CodeGraph upstream upgrade playbook

Status: U5 governance, 2026-11-26. This is the operational procedure behind the
policy in [`README.md`](README.md). It is read-only until step 4; no command
here auto-follows, auto-merges or auto-updates a pin.

Only CodeGraph is a code-level thin fork. CodeBoarding/SCIP/traces/AOCI/LSP/
Cytoscape are artifact or process boundaries; update them by regenerating their
fixtures and running their contract tests, not by merging their source trees.

## Release-only rule

CodeGraph is consumed only from a release: a git tag (`refs/tags/v*`) or, when
a release is untagged, the version-bump commit plus its release notes recorded
in `upstreams.lock.json`. Never merge `upstream/main` (or any other branch
head) into `archmeld/initial`.

- `upstream.py check` compares the pinned release tag only. For an untagged
  release it returns the pinned commit with `release_only=true` and states that
  branch head is not tracked.
- `upstream.py diff` / `drill` refuse `refs/heads/main`, `main`, `master` and
  `upstream/main` for a release-only upstream.
- `0.21.2` is the current untagged exception: the release identity is
  `ae4b2e54` plus its release notes, because the latest git tag is `v0.21.0`.
  This exception is explicit in `upstreams.lock.json`; do not infer a tag.

## Preflight

- [ ] Main worktree is clean (`git status --porcelain` is empty).
- [ ] Record the current branch, HEAD and `upstreams.lock.json` pin in the
      upgrade evidence.
- [ ] Keep the current release artifacts and `SHA256SUMS` (the previous
      Windows/Linux bundles) so a binary rollback stays possible.
- [ ] Create an annotated pre-upgrade tag, for example
      `git tag -a archmeld/pre-upstream-0.21.2 -m "state before upstream 0.21.2"`.
      No such tag exists today; create one before the next upgrade, not during
      the drill.
- [ ] Confirm `docs/archmeld/**` is clean. Do not edit the sealed pack.

## 1. Check and diff

```sh
python tooling/upstream.py check codegraph --json
python tooling/upstream.py diff codegraph --to <release-tag-or-release-commit> --json
python tooling/upstream.py patches --check
```

`diff` reports commits, changed paths, conflict hotspots and a `merge-tree`
preflight. `check` never fetches, merges or changes a pin.

## 2. Drill

```sh
python tooling/upstream.py drill codegraph --ref <release-tag-or-release-commit> --run --matrix quick --json
python tooling/upstream.py drill codegraph --ref <release-tag-or-release-commit> --run --matrix full --json
```

The drill creates a temporary worktree at HEAD, performs `git merge --no-ff`,
optionally runs the test matrix, and removes the worktree. It does not push and
does not update the pin. `--matrix full` adds PHP, server contract tests and the
export example build.

## 3. Merge and resolve conflicts

Perform the merge manually on `archmeld/initial` only after the drill is
reviewed. Rules:

- keep upstream structure; adapt the local patch to it;
- do not delete upstream tests, weaken assertions or change tolerances;
- keep `--archmeld-export-v1`, `--archmeld-state-root`, export entry points and
  state-root isolation intact;
- resolve conflicts in `main.rs`, `daemon.rs`, `memory.rs`, export files and
  `patches/codegraph/manifest.json` deliberately; record each resolution.

Run at least the matrix from step 2, plus the export/state-root smoke:

```sh
cargo test -p archmeld-core -p archmeld-api --offline --no-fail-fast -j 2
cargo test -p archmeld-core --test bridge_import --offline -j 2
cargo test -p codegraph --test archmeld_probes --no-default-features --offline -j 2
```

## 4. Record

```sh
python tooling/upstream.py record codegraph --evidence <passed-drill-evidence> --dry-run
python tooling/upstream.py record codegraph --evidence <passed-drill-evidence> --yes
```

`record` refuses non-`passed` evidence, updates the registry/locks/NOTICE and
refreshes the patch manifest base. Commit the evidence and the pin update
together. Do not use `record` for a rollback (see [`ROLLBACK.md`](ROLLBACK.md)).

## 5. Post-upgrade

- [ ] `python tooling/upstream.py verify --skip-cargo --json`
- [ ] `python tooling/upstream.py patches --check`
- [ ] `python tooling/upstream.py status --remote --json`
- [ ] Record the release/export smoke and hashes in evidence.
- [ ] Update `docs/archmeld-progress.md` and the handoff record.

## Rollback trigger

Stop the upgrade and go to [`ROLLBACK.md`](ROLLBACK.md) when any of these fail:

- compile or link of the pinned workspace;
- `archmeld-export-v1` contract/consumer tests;
- state-root isolation or MCP handshake;
- release/install smoke on Windows or Ubuntu.

Do not partially merge and do not leave the registry pin ahead of the source
tree.

## Prohibited

- auto-follow `latest`, auto-merge or auto-change a pin;
- `git reset --hard` or `git clean -fd` as recovery;
- merging the public snapshot branch (it is a sanitized snapshot, not history);
- deleting a failing test or lowering an assertion/tolerance to make the drill
  green;
- editing `docs/archmeld/**` without re-sealing the pack.
