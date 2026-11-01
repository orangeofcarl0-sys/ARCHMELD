# ADR-036 · Offline index and report CLI

Status: accepted (2026-10-31)

## Context

The first release bundles contained `archmeld-serve` and `archmeld-mcp` only.
AT068 requires a clean machine to install, index a workspace, open an offline
report, upgrade and uninstall without a model provider or network. The pinned
CodeGraph `codegraph-server` one-shot export path crashes after indexing on the
Windows GNU build (`0xC0000005`), including with an in-memory graph attempt.
The crash was traced to the server wrapper, not to the parser/indexer: a direct
in-memory `ParserRegistry` + `Indexer` + `export_ndjson_v1` path succeeds on the
same workspace.

## Decision

Ship two additional binaries in both CLI bundles:

1. `archmeld` — offline operator CLI:
   - `index --workspace <dir> --state <dir> [--project id]`: runs the sibling
     `archmeld-index` helper, captures a snapshot, imports the
     `archmeld-export-v1` NDJSON through the evidence bridge, and prints one
     JSON result.
   - `report --state ... --project ... --snapshot ... --out ...`: exports the
     checksummed offline report bundle.
   - `verify-report --dir ...`: re-verifies every manifest file and the bundle
     digest.
2. `archmeld-index` — a one-shot in-memory bridge built from
   `crates/codegraph-server/examples/archmeld_export.rs`. It uses the pinned
   CodeGraph parsers and indexer directly, does not open RocksDB, does not load
   an embedding model, and does not start a watcher.

The raw NDJSON export is kept under `<state>/index-runs/<run-id>.ndjson`; the
snapshot, config digest and ledger counts are part of the `archmeld index`
result. The report remains the existing static, script-free, checksummed bundle.

## Consequences

- Clean-machine indexing no longer depends on the crashing server wrapper.
- The bundle grows by the helper binary and its parser dependencies.
- Indexing is a full structural parse per invocation; incremental state is
  written but not resumed by the one-shot helper.
- The helper does not replace CodeGraph; it is a thin invocation of the pinned
  parser/indexer and export API.
- Linux and Windows bundles must both carry the four binaries; install,
  rollback and uninstall enumerate them.
- The offline report path is now exercised from the installed prefix, not only
  from the source tree.

## Rejected alternatives

- Packaging the crashing `codegraph-server` binary and hoping the export path
  works: rejected because the crash is reproducible.
- Making the help text say “index manually with Cargo”: rejected because a
  clean machine has no toolchain.
- Copying a prebuilt ledger into the bundle: rejected because it is not the
  user's workspace and would fake the acceptance.
