# ADR-022 · Frozen contract digest preimage and plan approval

Status: Accepted for implementation; contract revision pending.

## Context

The 1.0 schemas define `BehaviorContract.digest`, `Plan.contract_digest`, and a
`Plan.status` of `approved`/`stale`, but do not define the digest preimage or how
an approval is recorded. Without a stable preimage, two implementations can
freeze the same contract with different digests; without an approval route, a
plan cannot become approved.

## Decision

1. `contract.digest` is AM-C14N-1 over the contract object **excluding the
   `digest` and `frozen` fields**. `frozen` is an acceptance state, not part of
   the frozen content. A proposal must supply the matching digest and
   `frozen=false`; the stored contract is `frozen=true`.
2. A contract revision must equal `latest_revision + 1`. Changing any check
   (including numeric tolerance), removing a required check, or changing an
   invariant changes the digest.
3. `POST /v1/plans/{plan_id}/approve` is added with
   `{project_id, expected_revision, contract_digest, actor, idempotency_key}`.
   It appends plan revision + 1 with `status=approved` and records the approval
   against the exact contract digest.
4. Approval fails if the plan is not `draft`, if the digest differs from the
   plan or from the latest frozen contract, or if the plan is stale. A plan
   whose contract digest no longer matches the latest contract is reported as
   `stale`; the old approval remains historical evidence but is not valid.
5. Plan permissions are still data: this ADR does not create a ChangeSet,
   staging area, or source mutation. Those remain B020/B021.
6. `GET /v1/contracts/{contract_id}` and `GET /v1/plans/{plan_id}` require
   `project_id` for the same reason as ADR-018/020.

## Consequences

- Contract and plan failures return explicit 409/400 errors before any row is
  written; AT042 evidence checks that no `changesets` table or plan row exists.
- The pack should absorb the preimage rule, `project_id` parameters and the
  approval route in the next contract revision; the patch is recorded in
  `contracts/archmeld/openapi-b019-extensions.patch.json`.
