# ADR-030 · Architecture review actions and the acyclic checker

Status: accepted for the R1 architecture slice (B017/B018 residuals).

## Decision

- An architecture is always proposed as `candidate` or `needs_review`. A
  provider or model cannot set `accepted` in a proposal.
- Human review is a separate action that appends a new architecture revision:
  `accept` maps to `accepted`, `reject`/`request_changes` map to
  `needs_review`, and `mark_reviewed` maps to `reviewed`. Every action requires
  an actor, a reason and an idempotency key, and writes an
  `architecture_reviews` row plus an `architecture.reviewed` event.
- Review never edits the architecture payload's components, dependencies,
  rules or claims, never writes ledger entities/relations, and never changes
  source bytes. It also does not turn a candidate into proof; the review row is
  an explicit human decision over the recorded verification.
- Stale `expected_revision` is rejected. Repeating the same idempotency key
  returns the recorded revision; using it with a different payload returns
  `scope_conflict`.
- The `acyclic` rule is now executable through
  `architecture.acyclic.v1`. It searches only the candidate dependency graph
  declared in the architecture, within `scope_ids` (all components when the
  scope is empty). A cycle is `violated` and blocks acceptance; no cycle is
  `passed` with `coverage.state=complete_declared_graph`. This completeness is
  limited to the declared graph: runtime cycles and undeclared dependencies are
  out of scope.
- `allowed_layers`, `interface_only`, `ownership` and `custom_contract` remain
  `declared_only` until a registered checker and the required model fields
  exist. A required declared-only rule still blocks acceptance.
- Architecture checks map `passed` to the verification state `passed`; the
  prior mapping made every complete rule inconclusive.

## Residual limits

- There is no To-Be graph editing UI yet; target graphs are submitted as
  complete architecture documents.
- Acyclic checking does not inspect runtime call cycles or dynamic dispatch.
- Architecture review does not sign approvals; local database access remains
  outside the non-repudiation boundary.
