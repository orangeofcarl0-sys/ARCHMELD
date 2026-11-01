# ADR-021 · Target-graph diff and executable rule checks

Status: Accepted for implementation; contract revision pending.

## Context

S07 requires As-Is, To-Be and Transition layers to be independent versions, and
a target-graph edit must not modify source. It also requires architecture rules
to declare whether a checker exists; a required rule without a checker must not
pass acceptance. Contract 1.0 has an `Architecture` object with `rules`, but no
route for diffing a target model or running the rule registry.

## Decision

Two local-core extension routes are added:

- `POST /v1/architectures/{architecture_id}/diff` with
  `{project_id, target}`. The base must be `layer=as_is`, the target
  `layer=to_be`, status `candidate|needs_review`, same snapshot. The response
  contains component/dependency diffs, typed intents
  (`remove_dependency`, `introduce_boundary`, `retire_component`,
  `split_responsibility`), affected ledger paths, `source_mutation.state=none`,
  and a candidate plan. No ChangeSet is created and no file is written.
- `POST /v1/architectures/{architecture_id}/check` with
  `{project_id, rule_ids?}`. The response contains the registry, per-rule
  verdicts and coverage, blocking rules and an aggregate state.

Rule semantics:

1. Only `forbid_dependency` with `checker_id=ledger.dependency_path.v1` is
   executable in this slice. It searches ledger relations of dependency-like
   kinds from source-component members to the forbidden target-component
   members, directly and through declared-scope intermediates, depth <= 6.
2. A path search over the static ledger is `coverage.state=partial`; even with
   no path the verdict is `no_violation_found`, never `passed`. A required rule
   with partial coverage blocks acceptance.
3. `allowed_layers`, `acyclic`, `interface_only`, `ownership` and
   `custom_contract` are `declared_only` until a checker is registered. A
   required declared-only rule blocks acceptance (AT075).
4. A rule that claims `checkability=executable` but names an unregistered
   checker is treated as `declared_only`; human intent is not executable.
5. Violations are candidate paths backed by relation ids, not resolved calls or
   observed facts. The source tree and ledger relations are never modified.

## Consequences

- The candidate review UI can display check results later; this slice exposes
  the API and core result. A To-Be graph editor remains future work.
- The pack should absorb the two operations and the `intents`/`coverage` shapes
  in the next contract revision; the deviation is recorded in
  `contracts/archmeld/openapi-b018-extensions.patch.json`.
