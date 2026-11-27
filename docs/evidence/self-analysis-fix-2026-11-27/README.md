# Self-analysis repair acceptance — 2026-11-27

This bundle closes P01–P25, the real cognition map, and the release-bundle
revalidation. It uses the rebuilt helper (schema 1.2), the current
`archmeld`/`archmeld-mcp` binaries, and the `0.1.0` Windows/Linux bundles.

## Inputs

| Item | Value |
|---|---|
| Snapshot | `sha256:46fd4d94506e977d0879e6c70f13c07b776f502c00e5fc4bc062f8e103bc9ac5` |
| Snapshot HEAD | `f36c9870` |
| Run | `run.archmeld-self.index.1795732916` |
| Export digest | `sha256:f64a3cbc1d039f512e7b5822fe305d722580e859a292e5b61d0c89d693b3f961` |
| Export bytes | 18,510,414 |
| Manifest schema | 1.2 |
| `archmeld` | `0.1.0+dd124ca1`, `sha256:21ad4d...` |
| `archmeld-mcp` | `sha256:4a3a9c...` |
| Debug helper | `sha256:6d48c9...`, 567,959,605 B |
| Windows release helper | `sha256:1c0dec...`, schema 1.2, commit `dd124ca1` |
| Linux release helper | `sha256:268cb0...`, schema 1.2, commit `49683c1d` |

The debug/first release RocksDB builds used MSYS2 UCRT Mingw-w64 16.1.0 after
the earlier winlibs 13.2.0 installation disappeared. The dev profile keeps
RocksDB debuginfo off (833 MB archive → 228 MB); the release archive is 42 MB.

## Acceptance results

`acceptance.json` passed **24/24**:

- status/review `clean`, `needs_review=false`; brief has no stale-change
  finding; `files_origin={tracked:1868}`.
- `archmeld_context` with the `Store` type and 4,000 tokens returned 11 selected
  entities and 7 source snippets (3,856 estimated tokens).
- MCP `archmeld_status` summary is 11,773 bytes (<256 KB);
  `archmeld_review` summary is 1,880 bytes (<1 MB).
- languages: kotlin 318, toml 274, bash 14 entities; index parse counts include
  kotlin 39, toml 49, bash 8.
- 47 `implements` relations; deferred resolver added 25, 0 ambiguous.
- `examples/` has 65 entities; 3 Kotlin parser-error paths are recorded.
- skip reasons: exclude_pattern 10, excluded_dir 49, hidden 8, too_large 2,
  unsupported_extension 664.
- **P04**: `min_producer_resolution=direct` + `require_call_site=true` returned
  4 site relations, all `direct`, none `resolved`; relation filters on symbols
  return an explicit `invalid_request`.
- **P11**: Store impact contains test entities by default and none with
  `exclude_entity_roles=["test"]`.
- **P12**: `::Store` returns type/symbol/module/external with type first;
  module results are external and anchor-less, and the module kind filter is
  exclusive.
- **P16**: `cytoscape` is absent by default (minified pattern), and
  `Cargo.toml` config results drop from 20 to 0 with
  `exclude_entity_roles=["config"]`.
- **P23**: auto brief performs exactly one review (`review_calls=1`).
- **P25**: helper `--schema` is 1.2, `--version` launches, the 0xC0000135
  missing-DLL exit is induced, and the exit-hint unit test covers
  0xC0000139/0xC0000135.

## Timings

| Command | Before | Now |
|---|---|---|
| `status --workspace` | 3m57s | 7.98 s |
| `review-diff` | 2m38s | 11.79 s |
| `brief auto` | 6m02s | 14.43 s (one review, 11.83 s) |

## Review cursor paging

`PAGING-RESULT.json` uses a synthetic 600-file workspace: summary page 1 is
500 files / 534,980 B with `next_cursor`; page 2 is 100 files / 396,901 B;
paged unique = full unique = 600, all pages <1 MB. This run found the
`sha256:` colon cursor bug (`5012abef`) and the oversized 500-file summary page
(`c0280a8f`).

## Cognition map (P17)

`archmeld.map.txt` is committed with 18 entries and 23 confirmed refs:
`map check/status` state `ok`, 0 orphan/ambiguous; `brief --recipe map` reports
`map ok`; MCP `inspect(map_entry)` returns the entry. The map header binds
snapshot `3f7fef` and is checked against it; the final snapshot includes the map
file but is one re-baseline newer.

## Release bundle

`dist/release-manifest.json` records product commit `af8fa2f9`, both helper
schema/hash/commit fields, and GCC 16.1.0. `test_release.py` passed Windows,
Ubuntu 24.04, VSIX, AT068, AT069 and AT077. Old artifacts are archived under
`dist/archive/2026-10-31-590c2ca7/`.

## Command records

`COMMANDS.json` lists the `baseline/commands.ndjson` ids, argv, exit codes,
durations and log hashes. Key ids:

- `self-fix-helper-build-3-2026-11-27`,
  `self-fix-helper-build-final-2026-11-27`;
- `self-fix-core-tests-3-2026-11-27`, `self-fix-export-tests-2026-11-27`,
  `self-fix-api-tests-cursor-2026-11-27`,
  `self-fix-server-indexer-tests-2026-11-27`,
  `self-fix-server-integration-tests-2026-11-27`;
- `self-fix-index-2026-11-27`, `self-fix-acceptance-cursor-2026-11-27`.

The final index and acceptance were run directly so the workspace stayed
byte-identical while status/review observed it; recording them in
`baseline/commands.ndjson` happens after observation, as it must.

## Limitations

1. The snapshot precedes the evidence-only commits that record this run.
   Clean status was observed at snapshot time, not against the final evidence
   commit.
2. `archmeld`/helper were built at `dd124ca1`; later commits are tests, docs,
   map and release tooling. The analyzed product source is the same.
3. The Linux release helper was built at `49683c1d`; its source is identical to
   the final helper source (only snapshot-policy/docs/tests changed afterwards).
4. The full `codegraph-server` Windows suite still has pre-existing
   backend/path/ROCKSDB failures; this bundle runs the indexer target, the two
   new integration targets, and the release CLI/helper tests.
5. B000/B030 evidence predates 0.21.2/P01–P25; re-running those on the current
   pin remains a release-evidence task, not a hidden pass.
