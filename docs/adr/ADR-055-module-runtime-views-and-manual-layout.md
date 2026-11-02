# ADR-055: Module/runtime projections and stable manual layout

Status: accepted for the 2026-11-02 cross-review closure.

## Context

The main graph view filtered by entity kind and used identity-hash ring
positions. It did not express a component-level projection, preserve dragged
positions, or bind a runtime run to the displayed entities.

## Decision

1. The Modules view groups entities by the first path segment. It is explicitly
   labelled a path-prefix projection, not an accepted architecture. Aggregate
   edges retain `relation_ids`, relation kinds, evidence ids and up to 200
   underlying relation records; the UI renders them dotted and labelled
   `module projection`.
2. The Responsibilities view can load a candidate architecture. Components are
   built only from its real `member_entity_ids`; declared dependencies and
   underlying graph relations both retain their evidence, and the warning names
   the architecture status. The view is labelled `component projection`, never
   accepted architecture.
2. Module positions are assigned by dependency-depth columns. Existing module
   positions are cached and never moved by a later merge; new modules fill the
   next free row in their column.
3. Dragged node positions are stored per project/snapshot/view in VS Code
   `workspaceState`; the model applies them to both raw and module projections.
4. Clicking a module lists its member entities and opens the selected member's
   historical source. Clicking an aggregate edge lists the underlying relations
   and attached evidence.
5. The runtime filter reads `/v1/runtime/{runId}`, keeps only entities mapped by
   `archmeld.entity_id`, and projects each parent/child span pair with two
   mapped entities as an `observed_span_parent` edge. These edges are labelled
   `span parent` and are explicitly not confirmed calls; other displayed edges
   remain static graph adjacency, not runtime ordering.

## Consequences

- The projections are stable and inspectable, but path-prefix modules are not a
  semantic component model and runtime filtering does not prove execution order.
- Manual positions are local UI state, not evidence; they are keyed to a
  snapshot and do not claim that the graph itself changed.
- Aggregate edge truncation at 200 underlying records is explicit in the model.
