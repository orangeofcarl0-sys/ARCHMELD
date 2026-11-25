# ADR-057: Session-start adherence and optional AOCI interop

Status: accepted for M2.5-A through M2.5-D (2026-11-24). The session-start
rules, markdown card, read-only AOCI block, optional session card and the
3-pair adherence experiment are implemented. D met the success line, so the
evidence-bound cognition map (E) is eligible for design but is not started.

## Context

The M1 slice made ARCHMELD usable on a first run, but it did not answer why a
coding agent would choose it over `grep` when the task looks local. The
2026-11-03 same-agent experiment cannot answer that question: both arms
received a complete HTTP call sheet, and both scored 17/17 fields. It measured
evidence correctness, not natural adherence. The ARCHMELD arm cost 14 calls /
229,400 tokens / 66.7 s against 9 calls / 140,449 tokens / 39.0 s, so the
current interface is also the more expensive one when used as instructed.

AOCI-CODE (`aoci-spec/aoci-code`, v0.2.0-rc4) approaches the same problem from
the other end: it makes a persistent, Git-versioned, model-authored cognition
map the first thing every agent run reads, and makes maintenance part of the
definition of done. Its mechanisms are instructive; its implementation is not
a fit. It is Go, about 200k LOC, FSL-1.1-MIT, has three write tools, and its
whole-index overview costs about 140k tokens per session in a reported ERP
case. It also explicitly forbids generating semantics from an AST, so it cannot
be driven from ARCHMELD's graph.

The full assessment is `docs/assessments/AOCI-CODE-2026-11-24.md`.

## Decision

1. Do not vendor, fork or embed AOCI-CODE. Do not replace the CodeGraph core
   with it. Its value is the workflow pattern, not the code.
2. Adopt the adherence mechanics that fit ARCHMELD's evidence model:
   - the agent rules become a session-start and post-edit contract, not a
     conditional "use before multi-file grep" hint;
   - `archmeld_brief` returns a one-screen markdown content item in MCP in
     addition to the existing JSON, so the model reads a conclusion instead of
     parsing fields;
   - `archmeld brief --write` can persist `.archmeld/SESSION.md`, bound to the
     snapshot, commit and working-tree state, and `archmeld_status` reports
     whether it matches the current snapshot. The file is opt-in, never
     auto-committed, and never authoritative.
3. Accept optional, read-only AOCI interop: when a repository has
   `aoci.txt`/`aoci.code.txt`, `archmeld_brief(recipe=change)` lists the AOCI
   entries whose object path is in the changed set, and `recipe=project`
   reports volume and entry counts. ARCHMELD does not write AOCI files, does
   not execute the AOCI binary, and treats parse failures as warnings.
4. Defer the evidence-bound cognition map (model-authored FRAS-like entries
   whose relations are validated against the snapshot) until the adherence
   experiment in step 5 shows that the cheaper mechanics are actually used.
5. Measure adherence before claiming an improvement: two conditions (current
   rules vs session contract + markdown brief), same agent/model/task, no call
   sheet, three runs per arm. Record calls before the first edit, tokens,
   correctness, and whether candidate edges are described as candidates.
   Success line: the session arm calls status/brief before the first edit in at
   least two of three runs, costs no more than 1.3x the baseline arm, and does
   not lose correctness or blur candidate/resolved. If it fails, do not build
   the cognition map.

## Consequences

- The MCP surface stays read-only and at eight tools. No AOCI write tool,
  no `maintain`/`update_entry`/`remove_entry` equivalent.
- Brief output remains a composition of existing facts. The markdown item is
  rendered from the same JSON; it adds no new evidence. `verification` stays
  `not_run`; candidate relations stay candidate.
- A persisted `SESSION.md` is a cache, not a second source of truth. A stale
  card must be reported as stale, and the agent must fall back to
  `archmeld_status`/`archmeld_brief` rather than trusting it.
- The AOCI block is explicitly attributed (`provider: "aoci"`) and is not a
  resolved call graph. If the AOCI format changes, the block degrades to a
  warning; it must not make `brief` fail.
- AOCI's FSL license is compatible with reading a user's own text files; it is
  not a license to copy its implementation. No AOCI source is copied.

## Evidence

- Assessment: `docs/assessments/AOCI-CODE-2026-11-24.md`.
- Current adherence baseline: `docs/evidence/agent-comparison-2026-11-03/`
  and `docs/release-evidence/AGENT-COMPARISON-2026-11-03.json`.
- M1 first-run baseline: `docs/release-evidence/M1-AGENT-ONBOARDING-2026-11-11.json`.
- M2.5-A: `crates/archmeld-api/tests/onboard.rs` asserts the installed rule
  text contains the session-start and before-finish contracts;
  `crates/archmeld-api/tests/mcp_stdio.rs` spawns the real `archmeld-mcp`
  process and asserts `archmeld_brief` returns both the JSON item and a
  markdown card. Focused run on 2026-11-24: `onboard` 6/6, `mcp_stdio` 1/1,
  full `archmeld-api` suite 38 passed / 0 failed (17 sections). Log:
  `docs/evidence/m2.5a-adherence-2026-11-24/archmeld-api.log` (sha256
  `925835098ab95d0a272d85c9fcfa9cc03b6b19ad02cf090d7116929930d1c869`).
- M2.5-C: `crates/archmeld-core/src/cognition.rs` reads `aoci.txt` and its
  code Volume, maps Entry paths to repository-relative changed files, and
  returns `present=false` or warnings instead of failing. Tests:
  `crates/archmeld-core/tests/cognition.rs` (5/5: neutral root, missing map,
  CRLF + malformed line, absolute foreign root by suffix, markdown card).
  Full core suite on 2026-11-24: 179 passed / 0 failed (59 sections); log
  `docs/evidence/m2.5a-adherence-2026-11-24/archmeld-core.log` (sha256
  `91633a38c05988f60155b387bd95c8e4f5e7305c28211d3aed6714fcb81d45f3`).
- M2.5-B: `crates/archmeld-core/tests/session_card.rs` (5/5) covers write,
  snapshot match/mismatch, dirty, missing, a non-ARCHMELD file without
  `--force`, and the fact that the card does not make the workspace stale
  (`.archmeld/` is skipped by the snapshot and drift walkers). `archmeld
  status` reports `session_card`, and `brief --write` produces
  `.archmeld/SESSION.md` on a clean fixture.
- M2.5-D: three pairs on a 17-file fixture, no call sheet. Full ritual
  (status + brief + impact before edit; review + change brief after) was
  B 3/3 vs A 0/3; brief before edit B 3/3 vs A 1/3; impact before edit
  B 3/3 vs A 2/3. Mean tokens B 680,358 vs A 722,590 (0.942x), with B doing
  less post-edit work (6.3 vs 26.3 calls). Correctness 6/6 and no candidate
  promoted to confirmed. Evidence:
  `docs/evidence/m2.5d-adherence-2026-11-24/`.
- The experiment found a real Python same-name binding defect
  (`from pricing import apply_discount` binding to `legacy.apply_discount`),
  recorded in `docs/REMAINING_BLOCKERS.md`; it is not counted as an agent
  error.
- E is eligible now that D met the success line, but no implementation or
  acceptance exists yet.
- Final core/API regression after B/D: 222 passed / 0 failed / 0 ignored
  (77 sections), exit 0; log
  `docs/evidence/m2.5b-session-card-2026-11-24/final-core-api.log`
  (sha256 `c6f4fe1d1d078d51ed1844bf0fd231eff3db3c4f6b7bedfe7086de616c4f724d`).
