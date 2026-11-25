# Public snapshot

This repository is a sanitized public snapshot of ARCHMELD, exported from a
private development repository on 2026-11-25. It is a single snapshot commit,
not the private development history.

This snapshot includes the U0/U1 upstream-governance slice:

- `upstreams.lock.json`, read-only `tooling/upstream.py` status/verify/check,
  and U1 `patches`/`diff`/`drill`/`record`;
- `archmeld-export-v1` `schema_version` 1.1 with `producer_commit` and
  `producer_dirty`, while legacy 1.0 remains importable;
- `patches/codegraph/manifest.json`: a path-level audit of every changed
  upstream file, with conflict hotspots and per-entry diff hashes;
- the Python per-symbol import alias fix and its evidence;
- the E4a no-go evidence; E remains at the read-only E2 map surface.

Redactions applied to this snapshot:

- authorized remote workstation host and user names;
- local user paths, workspace paths and the local machine hostname;
- the identity, byte/row counts, selected columns, digest and aggregate values
  of the private recorded dataset used for B029 evidence. The dataset itself and
  its derived tables are not included;
- E4a run directories and temporary benchmark fixture names.

Local paths in the retained evidence are replaced with `<repo>`, `<workspace>`,
`<temp>`, `<user>` and `<remote-host>` placeholders. Paths inside code or
evidence may therefore not be directly runnable from the public snapshot.

Other intentional differences:

- transient C# fixture build outputs under
  `adapters/languages/fixtures/csharp/{bin,obj}` were omitted; run the fixture
  build to regenerate them;
- the SCIP fixtures use the already-neutralized public index URIs;
- artifact hashes in `baseline/commands.ndjson` refer to the private originals
  before redaction. The public evidence files differ from those originals only
  by the redactions listed above.

No credentials, bearer tokens or private keys are included. The secret-like
strings under `crates/codegraph-harness/fixtures/` and the detector patterns in
`archmeld-core` are deliberately fake examples used by the secret-detection
tests.

The upstream CodeGraph license, NOTICE and provenance files are retained.
