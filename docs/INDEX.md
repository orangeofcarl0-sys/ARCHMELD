# ARCHMELD documentation index

This is the single navigation entry for the repository. When two documents
disagree, the authority order below decides. If a disagreement is material, fix
the lower-authority document instead of adding a third statement.

## Orientation

| Item | Value |
|---|---|
| Product | Local, read-only code-evidence workbench for coding agents |
| Upstream base | CodeGraph `ae4b2e54fc8ce1c074523943fc750b20f68bf8aa` (0.21.2), thin fork |
| Current phase | Project testing; release-level items are tracked but do not block test work |
| Current status | [`HANDOFF_CHECKLIST.md`](HANDOFF_CHECKLIST.md) (operational truth), [`archmeld-progress.md`](archmeld-progress.md) (chronological record), [`REMAINING_BLOCKERS.md`](REMAINING_BLOCKERS.md) (open/closed blockers) |
| Decisions | [`docs/adr/README.md`](adr/README.md) (ADR-017…067) plus ADR-001…016 in the frozen pack |
| Build and test | [`OPERATOR_GUIDE.md`](OPERATOR_GUIDE.md) and [`tooling/README.md`](../tooling/README.md) |
| Evidence | [`docs/evidence/README.md`](evidence/README.md) and [`docs/release-evidence/README.md`](release-evidence/README.md) |

## Authority order

1. User instruction and [`AGENTS.md`](../AGENTS.md).
2. Accepted ADRs ([`docs/adr`](adr/) and ADR-001…016 in
   [`docs/archmeld/docs/03_ARCHITECTURE_DECISIONS.md`](archmeld/docs/03_ARCHITECTURE_DECISIONS.md)).
3. Versioned contracts: base schemas in
   [`docs/archmeld/contracts`](archmeld/contracts/) and patch overlays in
   [`contracts/archmeld`](../contracts/archmeld/).
4. Detailed specifications in [`docs/archmeld/specs`](archmeld/specs/).
5. Requirement, work-item and acceptance records in
   [`docs/archmeld/acceptance`](archmeld/acceptance/) and
   [`docs/archmeld/execution`](archmeld/execution/).
6. Execution evidence in [`docs/evidence`](evidence/) and
   [`docs/release-evidence`](release-evidence/).

## Four documentation layers

The repository deliberately keeps four layers. They are not alternate versions
of the same document; each answers a different question.

| Layer | Location | Question it answers | Mutability |
|---|---|---|---|
| Frozen design pack | `docs/archmeld/` | What was specified, contracted and planned at package v1.1? | Only by a deliberate contract revision plus `tooling/reseal_pack.py --write` |
| Live decisions | `docs/adr/` | Why does the implemented system differ from or extend the pack? | Append a new ADR; do not rewrite accepted ones |
| Operations | `docs/OPERATOR_GUIDE.md`, `docs/HANDOFF_CHECKLIST.md`, `docs/REMAINING_BLOCKERS.md`, `docs/archmeld-progress.md` | How do I build, run, hand off and judge current state? | Updated in place; status statements should link, not duplicate |
| Evidence | `docs/evidence/`, `docs/release-evidence/`, `baseline/commands.ndjson` | What actually ran, when, at which HEAD, with which result? | Append-only; never rewrite a past result |

`docs/archmeld/README.md` describes the original specification package and
still says "not an implemented product". That sentence is historical: it
describes the package at delivery time, not the current repository. Use
[`HANDOFF_CHECKLIST.md`](HANDOFF_CHECKLIST.md) for current status.

## Where to look

| I need to… | Read |
|---|---|
| Build or test on Windows GNU | [`OPERATOR_GUIDE.md`](OPERATOR_GUIDE.md), then [`tooling/README.md`](../tooling/README.md) |
| Know what is done and what is next | [`HANDOFF_CHECKLIST.md`](HANDOFF_CHECKLIST.md) |
| Reconstruct the history of a task | [`archmeld-progress.md`](archmeld-progress.md) |
| Find an open blocker and its reproduction | [`REMAINING_BLOCKERS.md`](REMAINING_BLOCKERS.md) |
| Understand a decision | [`docs/adr/README.md`](adr/README.md) |
| Check a schema or API contract | [`docs/archmeld/contracts/README.md`](archmeld/contracts/README.md), then [`contracts/`](../contracts/) patches |
| Reproduce a release gate | [`docs/release-evidence/README.md`](release-evidence/README.md) |
| Reproduce an earlier task run | [`docs/evidence/README.md`](evidence/README.md) |
| Update the pinned upstream | [`upstreams.lock.json`](../upstreams.lock.json), [`docs/upstream/UPGRADE_PLAYBOOK.md`](upstream/UPGRADE_PLAYBOOK.md) |
| Understand repository structure and complexity | [`assessments/SELF-ANALYSIS-COMPLEXITY-2026-11-27.md`](assessments/SELF-ANALYSIS-COMPLEXITY-2026-11-27.md) |
| Evaluate the Palimpsest review feedback | [`assessments/PALIMPSEST-FEEDBACK-2026-11-28.md`](assessments/PALIMPSEST-FEEDBACK-2026-11-28.md) |
| Plan a reviewer study (not yet run) | [`assessments/REVIEWER-STUDY-PROTOCOL-2026-11-28.md`](assessments/REVIEWER-STUDY-PROTOCOL-2026-11-28.md) |
| Publish a public snapshot | [`tooling/public_snapshot.py`](../tooling/public_snapshot.py), [`docs/OPERATOR_GUIDE.md`](OPERATOR_GUIDE.md) |

## Repository layout

| Path | Ownership | Purpose |
|---|---|---|
| `crates/archmeld-core`, `crates/archmeld-api` | ARCHMELD | Evidence ledger, snapshots, change path, CLI/HTTP/MCP |
| `crates/codegraph-*` | Upstream CodeGraph | Parsers, analysis server, harness; preserve license and NOTICE |
| `tooling/` | ARCHMELD | Build, test, upstream, release, publication and evidence utilities |
| `baseline/` | ARCHMELD | Canonical command recorder (`run_cmd.py`) and command ledger |
| `contracts/` | ARCHMELD | Contract patch overlays on top of the frozen base schemas |
| `adapters/`, `fixtures/`, `upstream-probes/` | ARCHMELD | Language/tool fixtures and captured upstream probes |
| `vscode/` | Upstream CodeGraph | Upstream `codegraph` extension; do not present it as ARCHMELD |
| `archmeld-vscode/` | ARCHMELD | ARCHMELD Workbench extension |
| `jetbrains/`, `mcp-package/` | Upstream CodeGraph | Preserved upstream surfaces; not the ARCHMELD product entry point |
| `scripts/` | Upstream plus packaging | Upstream helper scripts and release packaging shell entry points |
| `docs/archmeld/` | Frozen pack | Sealed design, contracts, specs, acceptance and work items |
| `docs/evidence/`, `docs/release-evidence/` | ARCHMELD | Execution and gate evidence |

### Editor and IDE surfaces

Only `archmeld-vscode/` is an ARCHMELD product surface. The other directories
are preserved upstream code or packaging scripts; they are not built or shipped
by this repository's release path.

| Surface | Owner | Built/shipped here | Gate | Product entry point |
|---|---|---|---|---|
| `vscode/` | Upstream `codegraph` | No (requires upstream `vsforge`) | No | No |
| `archmeld-vscode/` | ARCHMELD | Yes (`dist/archmeld-workbench-*.vsix`) | `package_release.py`, `test_release.py` | Yes |
| `jetbrains/` | Upstream | No | No (only `self_analysis_acceptance.py` expects parser errors under it) | No |
| `vs-extension/` | Retired 2026-11-27 | No | No | No |
| `mcp-package/` | Upstream | No | No | No |
| `scripts/` | Upstream plus packaging | No | No | No |
| `adapters/` | ARCHMELD | Fixture input | `test_release.py`, `upstream.py` | No |

## Rules that keep the structure from drifting

- **One status source per claim.** `HANDOFF_CHECKLIST.md` states current truth;
  other documents link to it rather than restating numbers.
- **The frozen pack is sealed.** Any edit under `docs/archmeld/` requires a
  deliberate revision and `py -3.11 tooling/reseal_pack.py --write` in the same
  change; `--check` must pass afterwards.
- **Evidence is append-only.** A re-run with a different result creates a new
  dated record or a new gate file; it never edits the old one.
- **One canonical tool per concern.** Use the entry points listed in
  [`tooling/README.md`](../tooling/README.md); task-specific `b0xx_*` and
  `e4a_*` scripts are kept for reproducibility, not as general-purpose tools.
- **Upstream files stay upstream.** Do not delete upstream extensions,
  parsers or licenses to make the tree look smaller.
- **Public publication goes through the snapshot exporter and the push guard.**
  See [`docs/OPERATOR_GUIDE.md`](OPERATOR_GUIDE.md) and
  [`tooling/publication_guard.py`](../tooling/publication_guard.py).
