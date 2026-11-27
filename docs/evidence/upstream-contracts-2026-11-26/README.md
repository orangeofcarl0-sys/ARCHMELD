# Upstream contract gates and weekly check (2026-11-26)

Scope: U2–U4a. CodeBoarding/SCIP/AOCI contract gates, trace fixture audit,
LSP/trace-processor probes, and the read-only weekly upstream check. No pin was
changed, no upstream was merged, no push was performed, and `docs/archmeld/**`
was not edited.

## What changed

- CodeBoarding: `analysis_schema_version` is optional; missing is recorded as
  `unversioned`, an explicit unknown major is rejected before the idempotency
  digest, and `metadata.tree_spec.version` is recorded as an observation.
- SCIP: `Metadata.version` must be 0; non-zero is rejected, and missing
  `tool_info` is a warning.
- AOCI: `#Format-Version` and `#AOCI-ROOT-MANIFEST` are parsed; unknown majors
  and missing headers are warnings, and entries still parse.
- Traces: `verify` audits `adapters/traces/fixtures.lock.json` hashes, sizes,
  producers, limitations and consuming tests. No in-band format gate is invented.
- LSP: `probe_initialize_result` records `serverInfo` and advertised
  capabilities; `LspSession::capability_probe` reports version/capability
  downgrade. The sealed runtime profile is unchanged.
- Perfetto trace processor: `perfetto_trace_via_processor_checked` fails closed
  on a pinned version or sha256 mismatch.
- U4a: `status --remote --out --fail-on-update`, URL/HEAD fallback, `check --out`,
  weekly workflow, and `docs/upstream/WEEKLY_CHECK.md`.

## Results

| Check | Result | Log |
|---|---|---|
| contract tests (`interpretation`, `b017_model_analysis`, `scip_import`, `cognition`, `lsp_probe`, `lsp_roundtrip`, `perfetto_trace_processor`) | 32 passed / 0 failed (real LSP and trace processor ran) | `contract-tests.stdout.log` |
| `tooling/test_upstream.py` | 32 passed | `upstream-tests.stderr.log` |
| `upstream.py verify --skip-cargo` | `passed_with_warnings`, 0 fail | `upstream-verify.json` |
| `upstream.py status --remote` | codegraph up to date; codeboarding behind; no warnings | `upstream-status.json` |
| `upstream.py check codegraph` | pin = latest = `ae4b2e54`; exit 0 | `upstream-check.json` |
| public export | 1797 files, 28 omitted | `public-export.log` |
| `verify-export` | `passed`, 0 unclassified (6 allowed fake-secret fixtures) | `export-scan.json` |
| private scan | `passed_with_warnings` (historical local paths) | `private-scan.json` |

The CodeBoarding `update_available=true` is expected: its pin is a fixed commit
and the check compares against the default branch `HEAD`.

## Boundaries

- No automatic follow, merge, push, PR or pin update.
- `status`/`check` only use `git ls-remote`; no fetch into the working tree.
- The weekly workflow runs on the public snapshot's default branch with
  `contents: read`; the private tree uses the same local commands.
- `docs/archmeld/**` remains sealed; the runtime language profile is not
  promoted from a live probe.
- The weekly evidence template omits `AT077` so it cannot override a real merge
  record selected by `handoff_check.py`/`test_release.py`.
