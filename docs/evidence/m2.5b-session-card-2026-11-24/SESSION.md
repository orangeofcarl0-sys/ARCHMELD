<!-- ARCHMELD SESSION CARD -->
# ARCHMELD session card

- snapshot: sha256:9e43304f09e25de7bde38172ad7ca9a141d584b064acfa3b10d0830d7bddd4fc
- commit: unknown
- coverage: partial
- workspace: dirty-or-stale
- counts: confirmed=2 candidate=1 unknown=1 stale=1 findings=2
- generated_at_unix: 1795530355
- note: generated from the evidence snapshot; regenerate after changes. This card is a projection, not a source of truth and not a call graph.

# ARCHMELD brief

2 file(s) changed; 1 affected entities; 1 candidate relation(s); verification not run

- level: change
- project: project.m1
- snapshot: sha256:9e43304f09e25de7bde38172ad7ca9a141d584b064acfa3b10d0830d7bddd4fc
- coverage: partial
- confirmed: 2 · candidate: 1 · unknown: 1 · stale: 1 · findings: 2

## Findings

- **HIGH** src/b.py::b changed — this entity changed and has incoming static relation(s) in the base snapshot
  - at `src/b.py:1`
  - entity `syn.94fadd34b94ad5a89bd288829d0f0274469547eb95abc6d83473de63e394395f`
  - next: `archmeld_source`
- **MEDIUM** changed files without a base entity mapping — 1 file(s) have no base anchors
  - next: `archmeld_review`

## Unknowns

- Which files or symbols are missing from this view?
  - why: coverage is partial
  - resolve: inspect coverage.limitations or re-index with a larger budget
- Does this change pass tests?
  - why: review is static and executed no repository command
  - resolve: run the repository's own tests or a verification contract

## Next

- `archmeld_source` — read the exact changed source range
- `archmeld_query` — reverse-traverse incoming relations for this entity
- `archmeld_review` — get the full per-file review and coverage
