# ADR-025 · Read-only MCP surface and bounded ContextPacks

Status: accepted for the R1 MCP/context slice (B023).

## Decision

- The MCP stdio bridge is a second entry point into the same Store, not a
  second state or permission model. It exposes four read tools
  (`archmeld_inspect`, `archmeld_query`, `archmeld_context`,
  `archmeld_source`) and advertises the 14-tool contract surface, but every
  write-looking tool is refused with `permission_denied` before any core
  mutation.
- There is no approval tool, no apply tool that can authorize itself, and no
  arbitrary filesystem path argument. Inspection kinds resolve by authorized
  IDs; source is read only through an entity anchor. Unknown properties are
  rejected by the core request structs.
- Protocol output is stdout-only. Audit records are appended to a separate
  JSONL file with the tool, an argument digest, outcome and error code; no
  source, token or secret is logged.
- A ContextPack is built from one frozen snapshot and carries its snapshot,
  config and pack digests, the product contract version, permissions summary,
  selected entities, source snippets with path/blob/range, graph slices with
  basis/resolution/evidence refs, claims (counterexamples first), evidence
  summaries, unresolved questions and explicit omissions.
- Budgeting uses a deterministic bytes/4 estimate, not a model tokenizer.
  Objective, constraints, claims and counterexamples are considered before
  large source bodies. When space runs out the builder clips prose or lists an
  omission with a resume cursor; it never labels a truncated pack complete.
- The pack artifact is stored in the CAS and the returned digest is computed
  over the canonical pack without the digest/artifact fields.

## Residual limits

- Write actions through MCP are deliberately unavailable in this version. The
  trusted local API/UI remains the only path that can prepare/apply with an
  approval.
- `runtime_run` and `report` inspection return `unsupported` until those
  surfaces are implemented.
- The token estimate is coarse; clients must treat `estimated_tokens` as a
  bound estimate, not an exact model count.
- MCP tool annotations are client hints only and are never used as
  authorization.
