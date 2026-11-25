# ARCHMELD brief

1 file(s) changed; 1 affected entities; 1 candidate relation(s); verification not run

- level: change
- project: project.m1
- snapshot: sha256:9e43304f09e25de7bde38172ad7ca9a141d584b064acfa3b10d0830d7bddd4fc
- coverage: complete
- confirmed: 2 · candidate: 1 · unknown: 0 · stale: 1 · findings: 1

## Findings

- **HIGH** src/b.py::b changed — this entity changed and has incoming static relation(s) in the base snapshot
  - at `src/b.py:1`
  - entity `syn.94fadd34b94ad5a89bd288829d0f0274469547eb95abc6d83473de63e394395f`
  - next: `archmeld_source`

## Unknowns

- Does this change pass tests?
  - why: review is static and executed no repository command
  - resolve: run the repository's own tests or a verification contract

## Next

- `archmeld_source` — read the exact changed source range
- `archmeld_query` — reverse-traverse incoming relations for this entity
- `archmeld_review` — get the full per-file review and coverage

