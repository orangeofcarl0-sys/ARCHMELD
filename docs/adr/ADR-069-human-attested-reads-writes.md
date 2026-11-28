# ADR-069: Human-attested reads/writes evidence

Status: accepted for implementation (2026-11-28).

## Context

`reads` and `writes` are relation kinds the review surface can display, but
CodeGraph 0.21.2 does not emit them and ARCHMELD executes no repository code.
Without a producer those relation kinds were unreachable, so ownership in a
structural diff could only ever be `coverage=none`. Palimpsest asked for a
declared source that never pretends to be runtime observation.

## Decision

1. Add `archmeld evidence import-human --state <dir> --input <attestation.json>`.
   The input names a project, one snapshot, a producer, an attestor and a list
   of observations (`kind: read|write|own`, source/target entity id, optional
   path/line, rationale).
2. Every imported relation is written with:
   - `basis=human`,
   - `resolution=declared`,
   - `conditions` containing `human-attested`, `attestor:<id>` and
     `not-runtime-observation`.
   The importer has no code path that writes `observed`, `runtime`, or
   `resolved_call`.
3. The exact attestation JSON is stored in the CAS and referenced as
   `artifact_digest`. The evidence object has `kind=human` and
   `coverage.state=partial`, with limitations that say it is a declaration.
4. Project/snapshot scope, entity existence, relation kind and optional path
   anchor are validated before any write. The import uses the ordinary evidence
   batch transaction, so a failed observation writes nothing.
5. Relation ids are deterministic over project, snapshot, attestor, kind,
   endpoints, path and line. Re-importing the same attestation is idempotent;
   reusing an id with different content is a conflict.
6. `diff_snapshots` shows these edges in `structural.ownership` with
   `basis=human` and `resolution=declared`. When no ownership edge differs,
   ownership coverage stays `none`; the presence of a declaration is never
   reported as proof that a write occurred at runtime.

## Consequences

- A team can record a human review finding as first-class, snapshot-bound
  evidence without a tracing tool.
- The core stays honest: human declarations cannot satisfy a runtime-evidence
  requirement, and an absent declaration still means ownership is unknown.
- Producing real `reads`/`writes` from a language server or tracer remains a
  separate external producer; this ADR does not change candidate/resolved
  semantics for call edges.

## Evidence

- `crates/archmeld-core/tests/human_evidence.rs`:
  `human_writes_are_imported_as_declared_human_evidence`,
  `human_evidence_rejects_missing_entities_and_wrong_snapshot_scope`,
  `human_writes_appear_in_structural_ownership_as_declared`.
- `contracts/archmeld/human-evidence.patch.json`.
