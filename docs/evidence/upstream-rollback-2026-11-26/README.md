# Upstream rollback drill (2026-11-26)

Scope: U5. Builds the pre-0.21.2 fork state and probes the source rollback path.
No pin, lock, NOTICE or patch manifest was changed; no tag was created; no
source was pushed.

## Target

- Current HEAD at the full run: `48d1005836db4f07310a1021d663613a620801e1`.
- Rollback target: `903bfe0a888e290a8618c714ffc1a11d27390261`, the first parent
  of merge `1ca0ae8b85a013ce9ce85af78a39d257d6332c0d` and the fork state before
  CodeGraph 0.21.2. `590c2ca7` is the pure upstream commit and is not a source
  rollback target.
- Revert probe: `git revert --no-commit -m 1 1ca0ae8b` on a temporary worktree
  at HEAD. Conflicts: `UPSTREAM_PATCHES.md` and
  `crates/codegraph-server/src/watcher.rs`; the probe was aborted, not resolved.

## Results

| Run | Result | Detail |
|---|---|---|
| full matrix | **failed** | core/API 211/0, bridge 3/0, probes 3/0, export example build 0; `codegraph-php` exit `0xC0000139` (STATUS_ENTRYPOINT_NOT_FOUND) and `archmeld_state_root` "server failed" — the old winlibs/Strawberry test binaries expect `__emutls_v` libstdc++ symbols that UCRT64 no longer exports; not a RocksDB runtime limitation |
| quick matrix | **passed** | core/API 211/0, bridge 3/0, probes 3/0; same target and revert probe |
| revert probe | **conflicted (recorded)** | `UPSTREAM_PATCHES.md`, `crates/codegraph-server/src/watcher.rs`; expected after later local commits touched merged files |

The full matrix is not green because the drill reused test binaries built with
the removed winlibs/Strawberry 13.2.0 toolchain, not because the old tree fails
its rollback contract. Rebuilding with UCRT64 16.1.0 gives PHP 89/89 and
`archmeld_state_root` 1/1; `codegraph --lib` with RocksDB is 59/59. The quick
matrix remains the Windows rollback gate; a Linux worker is optional, not
required by a RocksDB limitation. No test was removed or weakened.

## Files

- `full-matrix.json` / `.stdout.log`: full `rollback-drill` evidence.
- `quick-matrix.json` / `.stdout.log`: passing quick matrix.
- `SHA256SUMS`: hashes for this directory.

Command records: `#u5-rollback-full-matrix`, `#u5-rollback-quick-matrix`.

## Boundaries

- The drill uses temporary worktrees and removes them; the main worktree stayed
  clean.
- `rollback-drill` never calls `upstream.py record`; a real rollback must update
  pins/locks/NOTICE/patch manifest manually as described in
  `docs/upstream/ROLLBACK.md`.
- Evidence is named `ROLLBACK-DRILL-*`, not `UPSTREAM-*`, and contains no
  `AT077` block.
