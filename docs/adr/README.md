# Architecture decision records

The project has two ADR ranges:

- **ADR-001 … ADR-016** are part of the frozen design pack and live in
  [`../archmeld/docs/03_ARCHITECTURE_DECISIONS.md`](../archmeld/docs/03_ARCHITECTURE_DECISIONS.md).
  They record the foundation choices made when the package was written.
- **ADR-017 … ADR-061** live in this directory. They record implementation-time
  decisions, contract revisions and the accepted boundaries of each slice.

Both ranges have the same authority. Read the frozen range first for the
foundation, then the applicable file here for how the implementation resolved
the details.

## Accepted decisions

| ADR | Decision | Status / scope |
|---|---|---|
| [`ADR-017`](ADR-017-workbench-extension-slice.md) | Workbench extension slice (B007) | Accepted for B007; does not replace ADR-008 for R1 |
| [`ADR-018`](ADR-018-getentity-scope.md) | `getEntity` carries project and snapshot scope | Accepted; contract revision pending |
| [`ADR-019`](ADR-019-runtime-causal-mapping.md) | Runtime causal mapping, clock domains and scenario cancellation | Accepted; pack trace schema revision pending |
| [`ADR-020`](ADR-020-claims-and-candidate-architecture.md) | Claims, investigations and candidate-only architecture imports | Accepted; contract revision pending |
| [`ADR-021`](ADR-021-target-diff-and-rules.md) | Target-graph diff and executable rule checks | Accepted; contract revision pending |
| [`ADR-022`](ADR-022-contract-digest-and-plan-approval.md) | Frozen contract digest preimage and plan approval | Accepted; contract revision pending |
| [`ADR-023`](ADR-023-staging-and-journaled-apply.md) | Controlled staging, prepared digests and journaled apply | Accepted for the R1 change path (B020/B021) |
| [`ADR-024`](ADR-024-independent-verification.md) | Independent verification and the accepted gate | Accepted for the R1 verification slice (B022) |
| [`ADR-025`](ADR-025-read-only-mcp-and-context.md) | Read-only MCP surface and bounded ContextPacks | Accepted for the R1 MCP/context slice (B023) |
| [`ADR-026`](ADR-026-async-instigator.md) | Optional asynchronous Instigator | Accepted for the R1 enhancement (B024); disabled by default |
| [`ADR-027`](ADR-027-drift-backup-gc.md) | Drift reports, consistent backups and pin-aware GC | Accepted for the R1 operational slice (B025, AT056) |
| [`ADR-028`](ADR-028-numeric-invariant-performance-predicates.md) | Deterministic numeric, invariant and performance predicates | Accepted for B022 residual; used by B026/B029 |
| [`ADR-029`](ADR-029-snapshot-retention.md) | Count-based snapshot retention without evidence loss | Accepted for the R1 retention slice (B025 residual) |
| [`ADR-030`](ADR-030-architecture-review-and-acyclic.md) | Architecture review actions and the acyclic checker | Accepted for the R1 architecture slice (B017/B018 residuals) |
| [`ADR-031`](ADR-031-benchmarks-budgets-cancellation.md) | T0/T1/T2 benchmarks, budgets and cancellation | Accepted for the R1 scale slice (B026, AT058) |
| [`ADR-032`](ADR-032-offline-report-and-promotion.md) | Offline review bundles and validated mode promotion | Accepted for B027 (AT059–AT060) |
| [`ADR-033`](ADR-033-security-boundaries.md) | Security boundaries for B028 | Accepted for the R1 security slice (AT061–AT063) |
| [`ADR-034`](ADR-034-scientific-refactor-boundary.md) | Scientific refactor boundary | Accepted for the R1 scientific slice (B029, AT064–AT065) |
| [`ADR-035`](ADR-035-packaging-and-platform-claims.md) | Offline packaging and platform claims | Accepted for B031; the R1 naming condition (AT067) has since passed |
| [`ADR-036`](ADR-036-offline-index-and-report-cli.md) | Offline index and report CLI | Accepted 2026-10-31 |
| [`ADR-037`](ADR-037-declarative-architecture-rule-parameters.md) | Declarative architecture rule parameters and checkers | Accepted 2026-10-31 |
| [`ADR-038`](ADR-038-encrypted-backups.md) | Passphrase-encrypted local backups | Accepted 2026-10-31 |
| [`ADR-039`](ADR-039-runtime-counters-states.md) | Runtime counters and state intervals | Accepted for the project testing phase |
| [`ADR-040`](ADR-040-backup-rotation-retention.md) | Encrypted backup key rotation and path retention | Accepted for the project testing phase |
| [`ADR-041`](ADR-041-memory-ceiling.md) | OS-enforced memory ceiling for trace imports | Accepted for the project testing phase |
| [`ADR-042`](ADR-042-webview-paint-measurement.md) | Dedicated webview paint and frame measurement | Accepted for the project testing phase |
| [`ADR-043`](ADR-043-per-check-sandbox.md) | Per-check filesystem sandbox | Accepted for the project testing phase |
| [`ADR-044`](ADR-044-scenario-trace-association.md) | Explicit scenario-run association for trace imports | Accepted for the project testing phase |
| [`ADR-045`](ADR-045-trace-source-jump.md) | Snapshot source jump from a retained Tracy zone | Accepted for the project testing phase |
| [`ADR-046`](ADR-046-trace-processor-backend.md) | Optional Perfetto trace-processor query backend | Accepted for the project testing phase |
| [`ADR-047`](ADR-047-unix-socket-listener.md) | Unix-domain socket listener with owner-only permissions | Accepted for the project testing phase |
| [`ADR-048`](ADR-048-windows-named-pipe-listener.md) | Windows named-pipe listener with the bearer token | Accepted for the project testing phase |
| [`ADR-049`](ADR-049-authorized-model-interpretation.md) | Authorized model interpretation for CodeBoarding analysis | Accepted for the project testing phase |
| [`ADR-050`](ADR-050-container-sandbox.md) | OCI container sandbox for contract checks | Accepted for the project testing phase |
| [`ADR-051`](ADR-051-scenario-sandbox.md) | Scenario snapshot-copy and container execution | Accepted for the project testing phase |
| [`ADR-052`](ADR-052-report-opt-in-viewer-migration.md) | Opt-in source/trace export, static viewer and ledger migration | Accepted for the project testing phase |
| [`ADR-053`](ADR-053-numeric-modes-and-real-data.md) | ULP/statistical comparison, dimensional checks and real data | Accepted for the project testing phase |
| [`ADR-054`](ADR-054-session-status-refresh-diff-review.md) | Session status, explicit refresh and task-level diff review | Accepted for the 2026-11-02 cross-review closure |
| [`ADR-055`](ADR-055-module-runtime-views-and-manual-layout.md) | Module/runtime projections and stable manual layout | Accepted for the 2026-11-02 cross-review closure |
| [`ADR-056`](ADR-056-agent-onboarding-and-brief.md) | Zero-concept agent onboarding and one-screen briefs | Accepted for M1 onboarding (2026-11-11) |
| [`ADR-057`](ADR-057-session-start-adherence-and-aoci-interop.md) | Session-start adherence and optional AOCI interop | Accepted for M2.5-A–D (2026-11-24); E later covered by ADR-058 |
| [`ADR-058`](ADR-058-evidence-bound-cognition-map.md) | Evidence-bound repository cognition map (E) | E1/E2 implemented; E4a no-go; E3/E4b not promoted |
| [`ADR-059`](ADR-059-evidence-model-1.1.md) | Evidence model 1.1 and pack re-seal | Accepted for implementation (2026-11-26) |
| [`ADR-060`](ADR-060-release-only-upstream-intake.md) | Release-only CodeGraph intake | Accepted 2026-11-27 |
| [`ADR-061`](ADR-061-module-groups-and-re-exports.md) | Core module groups, root re-exports and in-place file splits | Accepted for implementation (2026-11-27) |

## How to add a decision

1. Use the next free number (`ADR-062` after ADR-061). Never renumber.
2. Record `Status`, `Context`, `Decision`, `Consequences`, and the acceptance or
   evidence that will prove the decision was implemented.
3. If it changes a sealed contract, update the pack and run
   `py -3.11 tooling/reseal_pack.py --write` in the same change.
4. Do not rewrite an accepted ADR to match later behaviour. Add a superseding
   ADR and link both directions.
