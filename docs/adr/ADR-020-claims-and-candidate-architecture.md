# ADR-020 · Claims, investigations and candidate-only architecture imports

Status: Accepted for implementation; contract revision pending.

## Context

Contract 1.0 defines `POST /v1/claims`, `GET /v1/claims/{claim_id}`,
`POST /v1/architectures` and `GET /v1/architectures/{architecture_id}`, and the
`Claim`, `ClaimProposal`, `Architecture` and `ArchitectureProposal` schemas. It
does not define how a claim is scoped to a project from its bare id, how a claim
is reviewed after counterevidence appears, how an investigation is persisted,
how CodeBoarding output is imported, or how a candidate's verification state is
read. S07 requires claims to link evidence, rejected hypotheses to retain their
counterexamples, and model output to stay a candidate.

## Decision

1. `GET /v1/claims/{claim_id}` and `GET /v1/architectures/{architecture_id}`
   require the `project_id` query parameter, matching ADR-018's scoping rule.
   Missing scope is a 400; a different project is a 404.
2. New local-core extension routes are introduced:
   - `POST /v1/claims/{claim_id}/review`
   - `POST /v1/investigations`, `GET /v1/investigations/{investigation_id}`
   - `GET /v1/architectures/{architecture_id}/review`
   - `POST /v1/architectures/codeboarding`
   - `GET /v1/capabilities`
3. Claim proposals accept only `status=proposed|needs_review`. A model or tool
   cannot accept its own claim. Reviewing to `rejected` requires at least one
   counterevidence id; reviewing to `accepted` requires at least one support id.
   Every review appends a claim revision and an audit row.
4. Architecture proposals accept only `status=candidate|needs_review` and
   dependency status `candidate`. An `observed` dependency is rejected rather
   than silently downgraded. Imported components/dependencies are candidates;
   no entity or relation row is written to the evidence ledger.
5. Missing member entities, stale source references, unknown dependency
   endpoints and missing claim ids are retained in the candidate and reported
   by the review endpoint as `unverified`/`partial`. They are never dropped and
   never materialized as ledger facts.
6. The CodeBoarding adapter reads its real `analysis.json` (components, nested
   components, `key_entities`, `components_relations`). Source references are
   checked against the bound snapshot's file blobs and line counts; exact
   qualified-name matches may link a component to an existing entity, otherwise
   `member_entity_ids` stays empty and the reference is still visible.
7. Provider commands are stored in the provenance artifact and counted as
   `commands_ignored`; they are never executed. The model provider is optional:
   `/v1/capabilities` reports `unconfigured`, while manual claims and
   architectures remain available offline.

Schema version 9 adds `claims`, `claim_reviews`, `investigations`,
`architectures` and `architecture_reviews`.

## Consequences

- Contract 1.0 is knowingly incomplete for project scoping and review; this ADR
  and `contracts/archmeld/openapi-b017-extensions.patch.json` are the traceable
  deviation until the next pack revision.
- Candidate review UI can be implemented against the stable review payload
  without changing the sealed contract object.
