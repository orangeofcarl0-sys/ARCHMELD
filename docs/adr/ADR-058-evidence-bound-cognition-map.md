# ADR-058: Evidence-bound repository cognition map (E)

Status: E1 and E2 accepted and implemented (2026-11-25): read-only map
format, `map check`, `map status`, `map maintain`, `brief(recipe=map)`,
status summary and `inspect(kind=map_entry)`. E4a ran on 2026-11-25 and is
**no-go**: A (no map) was correct in 2/3 runs, B (map) in 1/1, and the gate
required A <=1/3. E stays at the read-only E2 surface; E3 (`map apply`),
E4b and rule v3 are not promoted. This ADR records the design boundary; it
does not authorize a full implementation.

## Context

M2.5-D showed that the session-start contract changes agent behavior: the full
ritual was B 3/3 vs A 0/3, tokens were 0.942x, and all six runs kept candidate
edges as candidate. E is now eligible: a persistent, model-authored cognition
map whose relation claims are checked against an ARCHMELD snapshot.

AOCI-CODE remains the reference for the workflow, not for the implementation.
Its map is model-authored text, but its `R` fields are path-level prose that the
program does not validate. ARCHMELD can do better: resolve file/entity/relation
references, refuse to promote a candidate edge to confirmed, and report
orphan/ambiguous/stale explicitly.

The capability being added is not another code analyzer. ARCHMELD already has
structural analysis (graph, impact, source, runtime). What it lacks is a
persistent, governed, model-readable layer that records responsibility, intent
and non-obvious constraints across sessions. AOCI demonstrates that layer; E
adds it with ARCHMELD evidence binding. The full comparison is
`docs/assessments/AOCI-CORE-AND-ARCHMELD-COGNITION-GAP-2026-11-25.md`.

Two constraints shape the design:

1. Syntax entity IDs contain the file blob and line range, so they change when
   lines move. A map must use logical refs and resolve them against a snapshot.
2. The M2.5-D fixture exposed a real Python misbinding: `from pricing import
   apply_discount` was attributed to `legacy.apply_discount`. It was fixed in
   `codegraph-server` on 2026-11-25 (per-file import bindings; duplicate free
   functions without a binding stay unresolved). E1 still uses only
   file/entity refs; `relation:` syntax and resolution remain unimplemented
   and are a separate E3/E5 task.

The detailed plan is `docs/assessments/E-EVIDENCE-COGNITION-PLAN-2026-11-24.md`.

## Decision

1. The map is a single committed text file, default `archmeld.map.txt` at the
   workspace root. It has a header with the snapshot id, scope and budget, and
   one file-level Entry per managed file:
   `name[tags]: F:... | R:... | A:... | S:...`.
2. `F`/`A`/`S` are model-authored semantics. The program never generates,
   prefills or rewrites them. `R` is a comma-separated list of `file:` or
   `entity:` logical references; `relation:` refs are deferred until the Python
   same-name binding is fixed.
3. Every ref resolves against the map's snapshot. `orphan`, `ambiguous`,
   `unknown` and `candidate` are explicit; `confirmed` is rejected when only a
   candidate relation supports the ref. The map never upgrades a candidate call.
4. `map check` and `map status` are read-only and land first (E1). `map
   maintain` and `brief(recipe=map)` follow (E2). A constraint-seeded A/B
   outcome experiment (E4a) precedes `map apply`: if reading the map does not
   change decisions, E stays read-only and E3 is not built. `map apply`
   (candidate file, content hash, expected hash, snapshot binding, atomic
   write, receipt) is E3 and only starts after E4a passes.
5. Writes are CLI-only. The MCP surface stays at eight read-only tools;
   `inspect(kind=map_entry)` may be added as an inspect kind, not a new tool.
   Every read surface declares `semantic_claims=unverified` and
   `verification=not_run`; summaries that do not resolve refs declare
   `validation=not_run`; `brief(recipe=map)` exposes `map_state` separately
   from the brief's own `state`.
6. The default budget is 40k tokens. Scope declarations (`observe`/`exclude`)
   and per-entry limits keep the map bounded; over-budget is a warning, never a
   silent truncation.
7. AOCI and ARCHMELD maps coexist. `brief` labels the provider; it never merges
   AOCI prose into an ARCHMELD evidence relation.

## Consequences

- E is a projection over the ledger, not a second graph. `semantic_claims` is
  always `unverified`; the map is not a test result, a runtime trace or a
  readiness score.
- The map's value is falsifiable. Its differentiator is change-scoped
  selection, graph-free constraints and refs that fail loudly — not
  persistence alone (README/ADRs are persistent too). E4a ran and showed no
  sufficient outcome difference: A was correct in 2/3 runs without the map,
  B in 1/1 with it, so E stops at read-only. The result is recorded in
  `docs/release-evidence/E4A-NOGO-2026-11-25.json`; it is a no-go for this
  design, not a proof that maps never help.
- The map is committed and reviewed like code. It is excluded from the
  `.archmeld/` session-card directory so card writes do not dirty it.
- `map apply` never commits or pushes; the user owns the Git boundary.
- The Python same-name binding gap is a prerequisite for relation refs, not for
  E1. E1 can ship while the gap is recorded and tested separately.
- E4 is a go/no-go pilot: if the map exceeds 40k tokens, is not maintained, or
  agents ignore it, E stays at E2 and is not promoted into the rules.

## Evidence

- `docs/assessments/E-EVIDENCE-COGNITION-PLAN-2026-11-24.md` (detailed plan).
- `docs/assessments/AOCI-CORE-AND-ARCHMELD-COGNITION-GAP-2026-11-25.md`
  (AOCI core ideas and the capability gap).
- `docs/evidence/m2.5d-adherence-2026-11-24/` (D result and the Python
  misbinding finding).
- `docs/REMAINING_BLOCKERS.md` (the Python gap row).
- E1 implementation: `crates/archmeld-core/src/cognition_map.rs`,
  `crates/archmeld-api/src/cli.rs` (`map check|status`).
- E1 tests: `crates/archmeld-core/tests/cognition_map.rs` (8/8) and
  `crates/archmeld-api/src/cli.rs` `map_tests` (1/1). The M1 fixture check is
  exit 0 with confirmed refs and exit 1 for one orphan entity ref. Final
  core/API: 235 passed / 0 failed (78 sections), exit 0.
- `docs/release-evidence/E1-MAP-CHECK-2026-11-25.json` and
  `docs/evidence/e1-map-check-2026-11-25/`.
- E2 implementation: `map maintain` (snapshot-to-snapshot stale/orphan/
  missing), `brief(recipe=map)`, project/change `cognition_map` blocks,
  `status.projects[].map`, and `inspect(kind=map_entry)`.
- E2 tests: cognition_map 10/10, brief 5/5, MCP 1/1 (map_entry through the real
  process), CLI map 1/1. Full core/API: 238 passed / 0 failed (78 sections),
  exit 0.
- `docs/release-evidence/E2-MAP-MAINTAIN-BRIEF-2026-11-25.json` and
  `docs/evidence/e2-map-maintain-brief-2026-11-25/`.
- Not yet: E3 (`map apply`) or E4 pilot.
