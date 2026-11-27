# Structured upstream version probes (2026-11-26)

Scope: U4b narrow. Adds registry `probes`, the read-only `upstream.py probes`
command, and the weekly workflow step. No pin was changed, no source was
fetched into the worktree, and no GitHub REST API or token is used.

## What changed

- `upstreams.lock.json`: 16 component probes across SCIP, LSP, traces,
  Cytoscape, AOCI and the toolchain. `pin_from` reads existing lock files where
  possible; unsupported components are explicit `none`/`local_lock`.
- `tooling/upstream.py`: `git ls-remote --tags --refs`, npm registry, PyPI and
  Go proxy providers; semver ordering; `behind: null`; per-probe error isolation.
- `.github/workflows/archmeld-upstream.yml`: `upstream-probes.json` artifact.
- `tooling/portable_env.py`: MinGW sysroot include for bindgen and pinned Python
  support required by the older-tree rollback drill.

## Results

| Check | Result | Log |
|---|---|---|
| `tooling/test_upstream.py` | 43 passed / 0 failed | `upstream-tests.stderr.log` |
| `upstream.py verify --skip-cargo` | `passed_with_warnings`, 0 fail | `verify.json` |
| `upstream.py probes` | 16 probes: 8 update, 3 ok, 5 skipped, 0 warnings | `probes.json` |

Updates observed on 2026-11-26 (informational, not pins): scip-clang 0.4.1→0.4.3,
scip format 0.3→0.3.3, pyright 1.1.408→1.1.414,
typescript-language-server 4.3.4→6.1.0, gopls 0.20.0→v0.24.0,
csharp-ls 0.14.0→0.29.0, Tracy 0.13.1→0.15.0, Cytoscape 3.30.2→3.35.0.

Skipped with reason: clangd (LLVM tag stream), rust-analyzer (rustup
toolchain), VizTracer (no pinned fixture), AOCI (identifier is not a Git URL)
and the toolchain (environment pin).

## Boundaries

- Version updates do not change `--fail-on-update` (commit pins only), do not
  fail the weekly job and never update a lock or pin.
- `behind` is `null`; no commit-count or GitHub API query.
- Output contains public package/module URLs and versions only; no absolute
  cwd, argv or token.
- SCIP format `v0.3` compares as equal to a `v0.3.0` tag; prereleases are
  excluded unless a probe explicitly opts in.
