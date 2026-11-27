# ARCHMELD brief

1828 files, 11043 entities, 46244 relations, 7 language(s)

- level: module
- project: archmeld.self
- snapshot: sha256:66aaec6db1355ce1f7b4259a37ea417bdc84cafe1e38716c9f955a336d62a896
- coverage: partial
- confirmed: 13232 · candidate: 33012 · unknown: 5 · stale: 1 · findings: 2

## Findings

- **MEDIUM** snapshot coverage is partial — some files or symbols were omitted or unresolved during indexing
  - next: `archmeld_status`
- **MEDIUM** workspace differs from the indexed snapshot — the working tree has uncommitted changes; the graph describes older bytes
  - next: `archmeld_review`

## Unknowns

- Which files or symbols are missing from this view?
  - why: coverage is partial
  - resolve: inspect coverage.limitations or re-index with a larger budget
- binary: 24 file(s)
  - why: declared coverage limitation
  - resolve: treat the result as bounded evidence, not as absence
- excluded_dir: 7 file(s)
  - why: declared coverage limitation
  - resolve: treat the result as bounded evidence, not as absence
- gitignored: 6552 file(s)
  - why: declared coverage limitation
  - resolve: treat the result as bounded evidence, not as absence
- too_large: 47 file(s)
  - why: declared coverage limitation
  - resolve: treat the result as bounded evidence, not as absence

## Next

- `archmeld_query` — list indexed symbols in the current snapshot
- `archmeld_context` — build a bounded, evidence-backed project context
- `archmeld_review` — compare the working tree with this snapshot

