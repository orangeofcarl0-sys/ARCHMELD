# ADR-065: Structural snapshot diff, moves and convergent paths

Status: accepted for implementation (2026-11-28).

## Context

Palimpsest asked for a structural diff that shows added/removed cross-layer
dependencies, duplicate execution paths and ownership hypotheses. The existing
`diff_snapshots` compared files by path, entities by kind+qualified name and
relations by relation id. It could not distinguish a file move from a
delete+add, reported no structural dependencies, and could not name two paths
that converge on the same target.

The default CodeGraph 0.21.2 exporter emits calls, imports, contains, declares,
implements and references. It does not emit reads/writes/owns. The consumer can
now map those edge types when a producer supplies them, but they remain declared
syntax evidence, not runtime observations.

## Decision

1. `diff_snapshots` classifies deleted+added files with the same blob digest as
   `moved`; they leave the added/deleted lists. Workspace review applies the
   same pairing and reports `files[].state="moved"` with the old path's
   entities as unchanged context.
2. Relations are also compared by a stable structural key
   `(source qualified name, target qualified name, kind, resolution)`, with the
   relation id kept as evidence. This removes content-id churn from the
   structural view.
3. `structural.paths.convergence` enumerates bounded paths (depth <= 4, fixed
   expansion cap) from changed/added entities and reports targets reached by at
   least two paths. A path is a hypothesis, never an execution path. A path with
   any `candidate` relation sets `hypothesis=true`.
4. `structural.ownership` reports added/removed `owns`/`writes` relations when
   they exist. Otherwise coverage is `none` with an explicit limitation.
5. `structural.entry_points` is `basis=unavailable`; entry points require an
   architecture declaration and are not inferred from call degree.
6. `archmeld review-diff` accepts `--candidate-snapshot <id>` and returns the
   snapshot-to-snapshot diff (including `structural`). `--workspace` remains the
   default mode and is required when no candidate snapshot is given. Workspace
   review detects content-identical moves; it does not infer semantic refactors.
7. The export contract documents the additional edge vocabulary for external
   or future producers. CodeGraph 0.21.2 does not emit it; absence of writes
   evidence is reported as unknown, not as no writes.

## Consequences

- A move is no longer counted as a new implementation.
- Two paths that converge are visible, but the result is explicitly a static
  hypothesis; a candidate edge never becomes a resolved call.
- The structural diff answers "which call/import paths converge" and "which
  declared writes exist"; it does not answer "which state is actually written"
  unless a producer supplies that evidence.
- Path enumeration is bounded; the output reports the cap instead of claiming
  completeness.

## Evidence

- `crates/archmeld-core/tests/diff_review.rs`:
  `snapshot_diff_detects_moves_and_convergent_paths`.
- `contracts/upstreams/archmeld-export-v1.md` edge vocabulary.
