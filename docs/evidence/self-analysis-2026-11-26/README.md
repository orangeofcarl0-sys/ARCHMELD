# ARCHMELD self-analysis evidence, 2026-11-26

Raw outputs from indexing `<path> (HEAD
`7b1dc280`) with the product's own CLI and reading it back through
`archmeld-mcp`. The report is
[`docs/reviews/ARCHMELD-SELF-ANALYSIS-2026-11-26.md`](../../reviews/ARCHMELD-SELF-ANALYSIS-2026-11-26.md).

## Provenance

| Item | Value |
|---|---|
| Product | `target/debug/archmeld.exe` 0.1.0, sha256 `af27ed13…` |
| MCP | `target/debug/archmeld-mcp.exe`, sha256 `ad2892e1…` |
| Helper | `target/debug/examples/archmeld_export.exe`, sha256 `abb2b64d…` |
| Snapshot | `sha256:72a51d623dd67ab3d6adf1dbaebefc1796a6f6f078432025f028a0181994e6b2` |
| Export | 18 MB NDJSON, sha256 `317cf17f84d891bc9db9e7f5d8a66516fc3df2b81beba5b1a1baed3f368503f6` |
| State | isolated temp ledger, not committed |

Exact argv, exit codes, durations and artifact hashes are in
[`COMMANDS.json`](COMMANDS.json). The ledger and binaries are intentionally not
committed; the raw outputs below are the durable evidence.

## Files

| File | Content |
|---|---|
| `index.json` | CLI index summary: 1,828 snapshot files, 742 parsed, 13,979 nodes, 51,518 edges, 11,043 entities, 46,244 relations, helper 33.1 s |
| `status.summary.json` | snapshot coverage, counts, languages, freshness counts (6,623 added, needs_review=false) |
| `status.raw.json.gz` | full `archmeld status --workspace` output (1.3 MB raw) |
| `brief-auto.md` | `brief --markdown` with the auto recipe; the tree is reported as changed |
| `brief-project.md` | `brief --recipe project --markdown`; includes the false "uncommitted changes" finding |
| `review-diff.summary.json` | state, file counts, coverage limitations and unmapped-file count |
| `review-diff.raw.json.gz` | full `review-diff` output (5.6 MB raw, 2 m 57 s) |
| `map-status.json` | `map status` when no `archmeld.map.txt` exists |
| `mcp-session-1.jsonl` | initialize, tools/list, capabilities, three symbol/text queries |
| `mcp-probe-core.json.gz` | inspect/neighbors/impact/context/source around `Store` |
| `mcp-probe-path-source.json.gz` | bounded path query, literal source search, snapshot inspect |
| `context-type-seed.json` | the empty-context reproduction at 4k/8k/16k tokens |
| `mcp-status.jsonl` | MCP `archmeld_status` (2 m 56 s, same drift as CLI) |
| `aggregates.txt` | read-only ledger aggregates: areas, cross-crate candidates, ambiguity, fan-in/out |
| `model-probe.txt` | edge multiplicity/property counts, coverage states, language/role/module model gaps |
| `coverage.txt` | tracked vs parsed files by extension and default-excluded directories |
| `scripts/*.py` | as-run MCP/model probe drivers; they contain the temp state path |

Gzip files are byte-identical to the raw outputs they replace; the SHA-256 in
`COMMANDS.json` is of the stored `.gz`.

## What this evidence does not show

- No build, test, runtime trace, SCIP, LSP or scenario was executed against
  this snapshot. Candidate relations are not resolved calls.
- Only 1,828 of 8,458 walked files were captured; 742 were parsed.
  `examples/` and `fixtures/` are excluded by the indexer defaults.
- No `archmeld.map.txt`, contract, claim or architecture was present, so those
  surfaces are exercised only in their empty state.
- `status.summary.json` and `review-diff.summary.json` are derived views; the
  `.raw.json.gz` files are authoritative.
- The report's "core API surface" table is a filtered view (generic method
  names removed), not a compiler-resolved call graph.
