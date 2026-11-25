# Python cross-module same-name binding fix (2026-11-25)

The M2.5-D fixture found that `from pricing import apply_discount` in
`checkout.py`, `wrapper.py`, and `analytics.py` was bound to
`legacy.apply_discount` (an unrelated function). `impact` on the real
function therefore under-reported callers.

## Root cause

`codegraph-server`'s `resolve_cross_file_imports` built one global
`name -> NodeId` map. Duplicate free functions were collapsed to the
highest-weight/first declaration, and import-symbol edges also used that
global map. The caller's own import statement was never consulted.

## Fix

In `crates/codegraph-server/src/watcher.rs`:

- index file paths, path suffixes, Python files, and module-level symbols per
  file;
- build per-file import bindings for `from module import name` and
  `import module [as alias]`;
- resolve Python calls in this order: qualified/inherited -> imported symbol
  -> module alias -> ambiguous method -> duplicate free function unresolved
  -> global fallback;
- use the same binding when creating the importing file's symbol edge;
- leave duplicate free-function names unresolved when no local/import binding
  exists, rather than guessing a homonym.

Non-Python resolution keeps the previous global fallback.

## Evidence

- Focused test `test_python_import_binds_call_to_module_not_same_name_homonym`:
  1/1, log `focused-test.log`.
- Watcher module: 29/29, log `watcher-tests.log`.
- Full server lib suite: 402 passed / 17 failed / 1 ignored, log
  `server-lib-tests.log`; the failures are backend workspace-path helpers,
  handler fixtures, the ai_query performance threshold and the memory path
  format. None are watcher or cross-file resolution tests.
- Minimal export: `checkout -> pricing.apply_discount` (id 6), not
  `legacy.apply_discount` (id 4); `assert.json`.
- M2.5-D fixture export: `checkout`, `summarize_order`, and `check_price` all
  call `pricing.apply_discount` (id 15), not `legacy` (id 13);
  `m2.5d-bindings.json`.
- ARCHMELD ledger impact on the same fixture: `pricing.apply_discount`
  returns `analytics.check_price`, `checkout.checkout`, `pricing.list_price`
  and `wrapper.summarize_order` as candidate callers; no legacy caller.
  `e2e-impact-summary.json`.
- Structured record: `docs/release-evidence/PYTHON-IMPORT-BINDING-2026-11-25.json`.

## Boundaries

- Calls remain `static_call_candidate`; this does not claim a runtime call.
- Per-symbol aliases (`from m import x as y`) were not extracted at this
  commit. A call could stay unresolved, or bind a unique global homonym named
  `y`; the follow-up `PYTHON-ALIAS-BINDING-2026-11-25` closes that gap.
- `relation:` refs in `archmeld.map.txt` are still unimplemented. The Python
  binding prerequisite is met, but adding relation syntax/resolution is a
  separate E3/E5 task.
