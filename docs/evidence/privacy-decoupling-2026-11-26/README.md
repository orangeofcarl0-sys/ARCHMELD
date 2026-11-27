# Privacy decoupling (2026-11-26)

Scope: P0–P3 decoupling of machine-specific paths and identifiers from the
product/test/tooling layers. No pin changed, no private history was rewritten,
and `docs/archmeld/**` was copied byte-for-byte.

## Results

| Check | Result | Log |
|---|---|---|
| `tooling/test_portable_env.py` | 14 passed | committed with P0 |
| `tooling/test_upstream.py` | 23 passed | `upstream-tests.log` |
| `tooling/test_public_snapshot.py` | 9 passed | `public-snapshot-tests.log` |
| `cargo test -p archmeld-core` without tools root | hermetic tests pass; external-tool tests print `SKIP` | run locally |
| `public_snapshot.py check --mode private` | `passed_with_warnings` (1631 historical findings) | `private-scan.json` |
| `public_snapshot.py export` | 1781 files, 28 omitted, temp output only | `export.log` |
| `public_snapshot.py verify-export` | `passed`, 0 unclassified | `export-scan.json` |
| `upstream.py verify` | `passed_with_warnings`, no fail | upstream evidence |

The private tree intentionally contains historical local paths; they are
classified as warnings, not failures. The exported tree has zero private
identifiers/paths after the redaction pass. The export remains a temp artifact;
publication still requires a separate explicit push.

## Boundaries

- External-tool tests use `ARCHMELD_TOOLS_ROOT`; when it is set, a missing tool
  is a hard failure. `ARCHMELD_REQUIRE_TOOLS=1` forces strict mode.
- Locks keep `path`/`source_path`/`archive.path` for old readers and add
  `observed_path`, `path_env`, `relative_path`, `strategy` and `path_kind`.
- `docs/archmeld/` is sealed; the three pre-existing SHA256SUMS mismatches
  remain warnings and were not edited.
