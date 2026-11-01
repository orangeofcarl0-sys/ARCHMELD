# ADR-037 · Declarative architecture rule parameters and checkers

Status: accepted (2026-10-31)

## Context

ADR-021 registered only `forbid_dependency` and `acyclic` as executable.
`allowed_layers`, `interface_only`, `ownership` and `custom_contract` were
`declared_only` because Contract 1.0's `ArchitectureRule` has no field for
checker inputs beyond `scope_ids`. That made the four rule kinds impossible to
execute without inventing meaning from component names.

## Decision

Add an optional `parameters` object to `ArchitectureRule`:

- omitted parameters leave existing documents byte-identical;
- at most 64 keys and 64 KiB encoded;
- required to be an object.

Register four new checker ids and execute them over the declared candidate
graph (never over source files or resolved calls):

| Kind | Checker | Parameters | Verdict |
|---|---|---|---|
| `allowed_layers` | `architecture.allowed_layers.v1` | `layers`, `allow_same_layer` | A dependency may point to the same or an inner layer; pointing outward (or same-layer when disallowed) is `violated`. Any unassigned component or endpoint makes coverage `partial`, so no violation is `no_violation_found`, not `passed`. |
| `interface_only` | `architecture.interface_only.v1` | `interfaces` | A cross-component dependency must touch a declared interface. A missing interface declaration makes coverage partial. |
| `ownership` | `architecture.ownership.v1` | `exclusive` | With `exclusive=true`, an `owned_state` or `owned_resources` item may have only one declaring component. This checks declarations, not runtime writes. |
| `custom_contract` | `architecture.custom_contract.v1` | `assertion` | Only `no_self_dependencies`, `max_dependencies_per_component` (+`limit`) and `require_dependency_reason` execute. Unknown assertions are `inconclusive` and required rules still block. No script engine is introduced. |

Coverage is reported honestly: complete for a fully declared graph, partial when
an id or endpoint is missing. Required partial/inconclusive rules still block
acceptance. The rule registry exposes the parameters and limitations; the
candidate review panel renders the per-rule results.

## Consequences

- The four kinds can be executable without changing component identity or
  inferring architecture from names.
- The sealed pack remains at 1.0; the deviation is recorded in
  `contracts/archmeld/architecture-rule-parameters.patch.json` and must be
  absorbed in the next pack revision.
- `custom_contract` is deliberately small. Adding assertions requires a new
  registry entry and tests, not an arbitrary embedded script.
- Existing rules without `parameters` keep their previous canonical bytes and
  declared-only behaviour.
