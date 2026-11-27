# ARCHMELD brief

6623 file(s) changed; 0 affected entities; 0 candidate relation(s); verification not run

- level: change
- project: archmeld.self
- snapshot: sha256:66aaec6db1355ce1f7b4259a37ea417bdc84cafe1e38716c9f955a336d62a896
- coverage: partial
- confirmed: 0 · candidate: 0 · unknown: 6623 · stale: 1 · findings: 1

## Findings

- **MEDIUM** changed files without a base entity mapping — 6623 file(s) have no base anchors
  - next: `archmeld_review`

## Unknowns

- Which files or symbols are missing from this view?
  - why: coverage is partial
  - resolve: inspect coverage.limitations or re-index with a larger budget
- Does this change pass tests?
  - why: review is static and executed no repository command
  - resolve: run the repository's own tests or a verification contract

## Next

- `archmeld_review` — get the full per-file review and coverage

