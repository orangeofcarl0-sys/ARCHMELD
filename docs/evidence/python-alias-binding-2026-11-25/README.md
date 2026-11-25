# Python per-symbol import aliases (2026-11-25)

Scope: `from m import x as y` safety and full mapping. This is a Python
parser/resolver correctness slice; E remains at E2 no-go and is untouched.

## Root cause

`extract_import_from` in `codegraph-python/src/extractor.rs` read only the
`name` child of `aliased_import`; the local `alias` child was dropped. The
graph edge carried `symbols=["x"]` and no alias. `python_import_bindings`
therefore mapped `(file, "x")` but not `(file, "y")`.

On a call `y()`, `resolve_callee` fell through to the global bare-name map.
If exactly one global `y` existed (function, method, class, or a non-Python
symbol), the call was bound to it — a wrong-target `static_call_candidate`,
not merely a missing edge. The earlier statement "aliases stay unresolved
rather than wrong" was too strong; it only held when no unique homonym
existed.

## Fix

- Parser: `from m import x as y` becomes one `ImportRelation` with
  `symbols=["x"]` and `alias=Some("y")`; plain names stay in a multi-symbol
  relation. Parenthesized import lists are collected recursively.
  `symbols` always contains exported names, never `"x as y"`.
- Resolver: `PythonImportBindings` now separates
  - `imported_symbols` (exported -> target, Phase 1 import edges),
  - `imported_locals` (local -> target, Phase 2 calls),
  - `module_aliases` (unchanged),
  - `shadowed_locals` (a bound/claimed local with no unique target).
  A two-pass unique-target collection prevents conflicting imports from
  overwriting each other.
- Safety: `resolve_callee` checks `imported_locals`, then `shadowed_locals`
  (including the receiver of `y.attr()`), before any module alias, duplicate
  free-function check, or global fallback. An unresolved alias can never
  borrow a global homonym.
- Phase 1 keeps using exported names so file -> symbol `Imports` edges still
  resolve. Non-Python languages are unchanged; the new maps are populated
  only for `python_files`.

## Tests

| Test | Result | Log |
|---|---|---|
| Extractor alias unit | 1/1 | `extractor-test.log` |
| Parser graph alias edge | 1/1 | `parser-test.log` |
| Watcher module (29 existing + 5 alias) | 34/34 | `watcher-tests.log` |
| Full `codegraph-python` | 122/0/1 ignored | `codegraph-python-tests.log` |
| Full `codegraph-server --lib` | 408 passed / 16 failed / 1 ignored | `server-lib-tests.log` |

The 16 server failures are the known backend workspace-path, handler,
memory-path and performance set; the `ai_query` performance test passed this
run. The watcher module and all cross-file (PHP/TS/C) tests pass.

Alias cases covered: function alias, class alias, unique global homonym,
missing target, ambiguous target (two `pricing.py` candidates), and
file-scoped shadowing.

## Not done

- The 1 GB `archmeld_export` example was not rebuilt (~22 min); the watcher
  tests exercise the same parser + resolver path. An export/ARCHMELD impact
  check would be a separate e2e pass.
- Alias to a re-exported symbol, relative-import package ambiguity,
  `import a.b` binding the first segment, star-import per-name binding, and
  Python method `parent_class` marking remain separate limitations.
- `relation:` refs remain unimplemented; E stays at E2 read-only.
