# ADR-018 · getEntity must carry project and snapshot scope

Status: Accepted for implementation; contract revision pending in the pack.

## Context

`contracts/openapi.json` 1.0 defines `GET /v1/entities/{entity_id}` with only the
path parameter, while S19 §3 requires every snapshot-bound query to carry
`project_id` and `snapshot_id`. Entity IDs are content-derived and the same ID
can appear in several snapshots; returning one without scope would either pick
an arbitrary snapshot or silently mix evidence. The current implementation
requires both query parameters and returns `invalid_request` (400) when they are
missing, which disagrees with the published operation signature.

## Decision

Add two required query parameters to `getEntity`:

- `project_id` (Id)
- `snapshot_id` (Digest)

The server returns 404 when the entity does not exist in that exact snapshot and
400 when either parameter is absent. No fallback to "current snapshot" is
allowed. The correction is recorded in
`contracts/archmeld/openapi-getentity-scope.patch.json` instead of editing the
sealed baseline pack, because `docs/archmeld/SHA256SUMS` must keep verifying the
original specification. The next pack revision (contract 1.1) should absorb the
patch.

## Consequences

- Clients must pass scope; the VS Code extension already queries through
  `/v1/query` with explicit scope.
- Contract 1.0 as shipped is knowingly under-specified; this ADR is the
  required trace for the deviation.
- `check_pack.py` still passes on the sealed baseline; the working contract is
  the original plus this patch.
