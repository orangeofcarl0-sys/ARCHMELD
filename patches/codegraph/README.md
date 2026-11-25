# CodeGraph patch manifest

`manifest.json` is a path-level audit of every change under upstream-owned trees
between the pinned commit and `HEAD`. It is **not** a rebased patch series.

Why not a series: the fork branch contains many mixed local commits (features,
evidence, docs and product code), so `git format-patch` would produce a noisy
dump and `git range-diff` has no comparable predecessor. Splitting history into
topic branches is a separate, risky operation and is not required for U0/U1.

What the manifest guarantees:

- every changed upstream path appears in exactly one entry;
- each entry records its necessity, acceptance IDs, whether it is a candidate for
  upstream, and a SHA-256 of the path-scoped diff;
- `patches --check` fails when `git diff --name-status <base>..HEAD` contains an
  unaccounted path or a manifest path that no longer differs.

Conflict hotspots (`main.rs`, `memory.rs`, `daemon.rs`, `crash_phase.rs`,
`index_state.rs`, `watcher.rs`, export files, `Cargo.toml`) are the first places
to inspect during an upstream merge. The manifest does not replace a real merge
drill.

When a path changes:

1. Run `python tooling/upstream.py patches --write` to refresh `diff_sha256` and
   `unaccounted_paths`.
2. If an unaccounted path appears, classify it in `entries` before recording any
   upstream update.
3. Update `UPSTREAM_PATCHES.md` with the same classification.

`format-patch`/`range-diff` become useful only after the fork is reorganized into
topic branches; that is deliberately out of scope here.
