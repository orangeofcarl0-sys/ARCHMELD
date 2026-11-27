# `archmeld-export-v1` contract

Status: additive revisions (`schema_version` 1.1 and 1.2), 2026-11-26.
1.1 adds producer provenance; 1.2 adds the optional `index` manifest block.

The producer is the pinned CodeGraph fork (`crates/codegraph/src/export/ndjson.rs`);
the consumer is the private evidence core (`crates/archmeld-core/src/bridge.rs`).
The format tag stays `archmeld-export-v1`: it names the protocol family, while
`schema_version` names an additive revision inside that family. Do not rename the
format tag for additive changes.

## Manifest line

The first non-empty line is a JSON object with `"type": "manifest"`. Fields:

| Field | Required | Meaning |
|---|---|---|
| `type` | yes | Always `"manifest"`. |
| `format` | yes | Always `"archmeld-export-v1"`; exact string match. |
| `schema_version` | no | `"1.0"` when absent. Producer emits `"1.1"`, or `"1.2"` when the `index` block is present. Consumers accept any `1.x` and reject another major. |
| `source` | producer | `"codegraph"`. The consumer does not currently validate it. |
| `version` | yes | The **`codegraph` crate version** (for example `0.2.0`), not the upstream release (`0.21.2`) and not the git commit. |
| `producer_commit` | no | Full 40- or 64-character lowercase/uppercase hex commit. Omitted when the build had no git checkout. |
| `producer_dirty` | no | Boolean; emitted only together with `producer_commit`. True when the build's git worktree was dirty. |
| `node_count` | yes | Number of node lines. The consumer uses it as the coverage denominator. |
| `edge_count` | producer | Number of edge lines. The consumer does not use it for coverage today. |
| `iteration` | producer | `"real-iterators"`. The consumer does not currently validate it. |
| `index` | no | 1.2+: `{files_total, files_parsed, files_skipped, by_language{}, skipped_by_reason{}, parser_errors[{path,language,message}], max_files_reached, max_depth_reached, cross_file_implements{resolved,ambiguous}, config{max_files,max_file_size_bytes,max_depth,exclude_dirs,include_dirs}}`. `parser_errors[].path` is relative to the indexed workspace. The consumer records this block as index coverage and as the source of the ledger's `parse` counters; unknown subfields are ignored. |

`producer_commit` is the only reliable upstream revision identifier. `version`
is a crate version and may stay unchanged across local patches. The commit is also
recorded in `upstreams.lock.json` for the pinned release.

## Node and edge lines

Unchanged from v1: each line is one JSON object with `"type": "node"` or
`"type": "edge"`; numeric backend IDs are local to the export; node `properties`
carry `path`, `name`, `line_start`, `line_end`; edge lines carry `source`,
`target`, `edge_type`, and optional `properties.line`. Producers may also emit
`properties.call_site_line`, `properties.is_direct` (boolean or `"true"`/`"false"`)
and `properties.resolved_by`; the consumer preserves them on the relation
(`call_site`, `producer_resolution`, `conditions`) without promoting a candidate
to a resolved call. An edge without a call-site line is imported with
`evidence_scope: artifact`; one with a line is `evidence_scope: site`.

## Compatibility policy

- The `format` tag is exact. A different tag fails closed with an `unsupported`
  error.
- `schema_version` is optional. Absent means `1.0`; any `1.x` is accepted; a
  different major fails closed.
- Unknown **manifest fields** are ignored. This is how additive revisions work.
- Unknown **line types** are rejected. A new line kind requires a new format
  version and a dual-read window.
- `producer_commit` is optional. When present it must be 40 or 64 hex characters.
  A malformed value fails closed.
- No timestamp is emitted. Export bytes stay deterministic for the same graph and
  build so repeated imports remain idempotent.

## Producer obligations

1. Emit `schema_version: "1.1"` (or a later `1.x`) when adding fields.
2. Emit `producer_commit` only when a full git hash is available and valid;
   otherwise omit it rather than writing `"unknown"`.
3. Emit `producer_dirty` only with `producer_commit`.
4. Never add a NDJSON line type inside `v1`; that is a format change.
5. Keep node/edge backend IDs stable within one export and sparse-ID safe.

## Consumer obligations

1. Parse fully before writing to the ledger; a bad manifest must not partially
   import.
2. Validate `format`, the schema major, and `producer_commit` shape.
3. Default an absent `schema_version` to `1.0` so historical exports remain
   importable.
4. Keep syntax candidates as candidates; the export does not resolve calls.
5. Record the artifact digest. The raw export (including `producer_commit`) is
   stored as the evidence artifact.

## Provenance boundary (U0)

`producer_commit` is visible in:

- the export manifest and the stored artifact blob;
- `BridgeReport` (`export_schema_version`, `producer_commit`, `producer_dirty`);
- the `archmeld index` JSON summary.

It is **not** yet a field of the core `Producer` evidence record. That record is
covered by the sealed `docs/archmeld/contracts/schemas/archmeld.schema.json`;
changing it requires re-sealing the specification pack and a separate contract
revision. Until then, consumers must read the commit from the artifact or the
index summary, not from the evidence `producer` object.

## Examples

Legacy `1.0` (still accepted):

```json
{"type":"manifest","format":"archmeld-export-v1","source":"codegraph","version":"0.2.0","node_count":1,"edge_count":0,"iteration":"real-iterators"}
```

Current `1.1`:

```json
{"type":"manifest","format":"archmeld-export-v1","schema_version":"1.1","source":"codegraph","version":"0.2.0","producer_commit":"ae4b2e54fc8ce1c074523943fc750b20f68bf8aa","producer_dirty":false,"node_count":1,"edge_count":0,"iteration":"real-iterators"}
```

## Tests

- Producer: `crates/codegraph/src/export/ndjson.rs` (`export_includes_sparse_ids`)
  and `crates/codegraph/tests/archmeld_probes.rs`
  (`export_ndjson_v1_matches_real_iterators`).
- Consumer: `crates/archmeld-core/tests/bridge_import.rs` covers legacy `1.0`,
  `1.1` with commit, unknown `1.x`, rejected `2.0`, malformed commit, and unknown
  manifest fields.
- Historical byte-level fixture:
  `upstream-probes/real-export/archmeld-core.ndjson` (no new fields) must keep
  importing.
