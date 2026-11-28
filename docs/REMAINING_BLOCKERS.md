# ARCHMELD remaining blockers

Audit date: 2026-11-28. Current phase: **project testing**. Full release-level
acceptance is not required in this phase; release-time items are recorded but do
not block current test work. Audit base: the current HEAD (use `git rev-parse HEAD`; this file was last refreshed after the 2026-11-28 Palimpsest closure; upstream pin is `ae4b2e54fc8ce1c074523943fc750b20f68bf8aa`, 0.21.2).
This file separates **closed** work from **still-blocking** work. A blocker is
only closed when the evidence path below exists and a real command produced it.
It is not a roadmap wish list: every open item names a reproduction and a
closure condition.

## 1. Cleared in this round

| Item | Result | Evidence |
|---|---|---|
| B030 AT067 runtime linkage | Seven groups captured with VizTracer, V8, clangd probe, JFR, EventPipe, Go pprof and a Rust probe; each trace imported into a bound snapshot and mapped to declared entities | `docs/release-evidence/B030-runtime-linkage.json`, command `#b030-language_runtime_linkage-perf-20261128-012425` |
| B030 AT067 medium real projects | Lua 5.4.7, CPython asyncio, npm lib, OpenJDK `java.util.concurrent`, Newtonsoft.Json 13.0.3, Go `net/http`, CodeGraph crate; archive/source digests and builds verified | `docs/release-evidence/B030-medium-projects.json`, `adapters/languages/fixtures.lock.json`, command `#b030-language_medium_projects-perf-20261128-012425` |
| B030 AT067 controlled rename | Seven real LSP renames applied to staging only, approved by exact prepared digest, journaled apply, rebuilt/rerun with identical output; same-named symbols untouched | `docs/release-evidence/B030-rename-apply.json`, command `#b030-language_rename_apply-perf-20261128-012425` |
| B030 AT066 scenarios | normal, slow consumer, bounded-queue backpressure, cancellation, simulated disconnect | `docs/release-evidence/B030-case-b-scenarios.json`, command `#b030-case-b-scenarios` |
| B014 Rust runtime trace | Explicit Rust probe, 4001 spans, 4001 mapped, 2 observed relations | `docs/release-evidence/B030-runtime-linkage.json` |
| B031 AT068 clean-machine indexing | Installed `archmeld` + `archmeld-index` index a workspace on Windows 11 and Ubuntu 24.04 with no Cargo/model/network | `docs/release-evidence/B031-install-tests.json` |
| B031 AT068 offline report | Installed CLI exports/verifies a 4-file bundle; Windows headless Chrome renders it with DNS disabled; Ubuntu static HTML checked | `docs/release-evidence/B031-install-tests.json` |
| M1 agent onboarding | `archmeld init` / `mcp install` / `brief`; index registers the workspace root; 8 read-only MCP tools; README, agent rules and ADR-056 | `docs/release-evidence/M1-AGENT-ONBOARDING-2026-11-11.json` |
| M2.5-A/C adherence | Session-start rules, one-screen markdown brief, read-only AOCI map block; core 179/0/0, API 38/0/0 | `docs/release-evidence/M2.5-ADHERENCE-2026-11-24.json` |

`AT067` is now `passed` in the language matrix with `r1_naming_allowed: true`
for the seven-language gate. AT068 clean-machine indexing/report is closed, and
the real 0.21.2 upstream upgrade (AT077) passed on 2026-11-24. R1 is defined by
S15/AT068 as the Windows 11 x64 and Ubuntu 24.04 x64 CLI/container path;
macOS and the Ubuntu GUI/VS Code host are explicitly unsupported/not tested and
are not R1 acceptance gates. The C++ WSL reference index is verified.

## 1.1 2026-11-28 full-closure slice

Closed at HEAD `fd5c8758` with command records in `baseline/commands.ndjson`:

| Item | Result | Evidence |
|---|---|---|
| MCP status paging | changed/added/deleted page independently; stale workspace/snapshot cursors rejected | `mcp.rs` unit tests; `#closure-targeted-tests-2026-11-28` |
| Brief/map counts and budgets | one count vocabulary, finding `confidence`, map findings/gaps, top-level coverage/verification | `tests/brief.rs`; ADR-068 |
| Review-report lifecycle | list/delete, 8 MiB cap, transactional pins, identity/digest verification, GC roots | store unit tests, `tests/diff_review.rs`; ADR-068 |
| Provenance unknowns | non-git/subdirectory/snapshot-only states explicit; ambiguous duplicate moves | `tests/diff_review.rs`; ADR-068 |
| Rule exception keys | assertion/limit in custom keys, duplicate rule ids rejected, result cap reported | `tests/blueprint_rules.rs`; ADR-064 |
| Human reads/writes | `evidence import-human`, human/declared only, CAS attestation, structural ownership | `tests/human_evidence.rs`; ADR-069 |
| I1 incremental parsing | stable work dir, persistent graph, deletion sweep, full export | `#i1-incremental-2026-11-28-b`; ADR-070 |
| AT067 at HEAD | seven-language groups + verifier passed | `b030-*-perf-20261128-012425` |
| B031 rebuild | Windows/Linux at `fd5c8758`, commit_consistency=true; CLI/container install, index, report, Chrome and VSIX install/uninstall | `#test-release-2026-11-28-e`, `B031-install-tests.json` |
| Doctor | locked Python 3.11; failed=0 | `#doctor-2026-11-28-e` |

Still open after this slice:

- **Reviewer pilot** — an agent-only pilot ran (12 durable `pi` sessions;
  `docs/evidence/reviewer-pilot-2026-11-28/`): arm B 6/6 matched, arm A 3/6,
  2 unsupported "resolved" claims in arm A. This is not a human study and
  claims no significance; a human study still requires two people.
- **Full test lane** — attempted with `--jobs 1`; see `baseline/commands.ndjson`
  for the recorded outcome. It is not claimed as passed unless that record is 0.
- **B000 Ubuntu upstream suite** — not rerun in this slice; the upstream pin is
  unchanged, so the 2026-11-27 evidence remains the record.
- **I2 delta ledger and runtime reads/writes** — still out of scope. I1 parses
  incrementally but publishes a complete snapshot/export.

## 1.2 2026-11-28 structural-debt slice

Closed under ADR-071 (behaviour-preserving; no contract/schema change):

| Item | Result |
|---|---|
| SCIP length overflow | crafted varint now returns `invalid input`; unit + import tests |
| Shared error/hash primitives | `CoreError` at `src/error.rs`; one `short_hash` wrapper; one content-tree digest |
| Store containment | idempotency inserts outside `Store` 18→0; typed snapshot and latest-revision queries; one-line Store SQL 87→71, `tx.*` 70→61 |
| Entry points | `cli.rs` 3,054→64; `api/lib.rs` 1,862→350; command tree pinned by a clap test |
| Large functions | `check_rule` 626→32; `diff_snapshots` 427→106; `create_context_pack` 650→148; `change_brief` 531→228 |
| Ratchet | `tooling/check_architecture_debt.py` runs in the fast lane; baseline only decreases |

Deferred structural debt (not blockers for current R1 evidence):

- `change_brief` remains 228 lines (160 is a warning line, not a hard gate). The residue is output
  assembly plus the budget/unknown tail; splitting it needs a wide transport
  struct and does not remove coupling.
- Typed DTOs for review/status/brief beyond the existing typed keys.
- One shared file-diff model behind drift, workspace review and snapshot diff.
- Six remaining large functions: `import_codegraph_export`,
  `publish_generation`, `import_trace`, `import_scip`, `check_map`,
  `apply_changeset_with_hook`.
- `Store.conn` privatisation once the remaining direct queries are typed.
- An import-based self-architecture rule; candidate call edges are not
  architecture evidence.

## 2. Release-time items (deferred in the test phase)

### 2.1 B031 / AT068 · clean-machine indexing

- Status: **closed**.
- The installed bundle now carries `archmeld` and `archmeld-index`. The helper
  parses the workspace with the pinned CodeGraph parsers into an in-memory graph
  and writes `archmeld-export-v1` NDJSON; `archmeld index` captures the
  snapshot and imports the export through the evidence bridge.
- Windows 11 and Ubuntu 24.04 container clean-prefix runs both produced a
  ledger (11 entities / 8 relations on the fixture). The installed flow invokes
  no Cargo or model; network is not instrumented, and PATH is inherited from the
  developer host unless the separate clean-PATH check is used.
- Evidence: `docs/release-evidence/B031-install-tests.json`
  (`windows_cli.clean_machine`, `linux_cli.clean_machine_index_report`).
- Product note: the helper is a full structural parse per run; it does not use
  the persistent `codegraph-server` wrapper and does not open RocksDB. The
  historical Windows `0xC0000005` was a Strawberry/winlibs GCC 13.2.0 runtime
  mismatch, not a current limitation.

### 2.2 B031 / AT068 · offline report opening from the packaged bundle

- Status: **closed**.
- `archmeld report` exports a checksummed bundle from the installed prefix;
  `archmeld verify-report` rechecks all four files and the bundle digest.
- Windows headless Chrome rendered the installed `report.html` with every host
  mapped to NOTFOUND and saw the ARCHMELD title. Ubuntu verified the same static
  HTML and confirmed there are no scripts or remote URLs; a rendered Ubuntu GUI
  browser was not available in the container and remains a declared limitation.
- Evidence: `docs/release-evidence/B031-install-tests.json`, release notes.

### 2.3 B031 / AT077 · real newer-upstream upgrade

- Status: **closed for the testing phase** (2026-11-24).
- What exists: a real merge from the previous pin
  `590c2ca7dd7b37bd1d41caf45fac535b6522f969` to upstream 0.21.2
  `ae4b2e54fc8ce1c074523943fc750b20f68bf8aa` (4 commits, 40 files,
  +2,991/−457), NOTICE attribution, and the earlier synthetic
  interface-rename drill (baseline 0, mutated 101, restored 0).
- Interface change found: upstream moved `Args::parse()` into `main()` so the
  thread budget is known before any pool exists, and added `max_files` to
  `DaemonConfig`. The only content conflict was
  `crates/codegraph-server/src/main.rs`; the local state root is now installed
  immediately after `Args::parse()` and before the startup crash marker.
- Contract evidence: `archmeld_state_root` 1/1, parser graph properties 12/12,
  PHP 89/89, core/API 211/0/0 (75 sections), and an end-to-end export with
  3 files / 9 nodes / 9 edges and PHP `qualified_name=Thing::render`.
- Pin records updated deliberately: `AGENTS.md`, `UPSTREAM_PATCHES.md`,
  `toolchains.lock.json`, `tooling/lock_toolchains.py`,
  `third_party/NOTICE.md`.
- Evidence: `docs/release-evidence/UPSTREAM-0.21.2-2026-11-24.json`,
  `docs/evidence/upstream-0.21.2-2026-11-24/logs/`,
  `docs/evidence/upstream-0.21.2-2026-11-24/`.
- Do not silently move the pin or claim a real upgrade happened.

### 2.4 B030 · C/C++ full SCIP

- Status: **closed for the testing phase**.
- What exists: clangd definition/reference, macro/virtual/function-pointer
  checks and a gated rename; plus a pinned WSL run of `scip-clang v0.4.1`
  (user-local binary, no system package) over the C++ fixture. The generated
  `index.scip` imported 3 documents, 21 entities and 21 reference relations
  with complete coverage, 0 omitted documents and 0 unresolved items;
  references were not promoted to resolved calls.
- Evidence: `docs/release-evidence/B030-cpp-scip.json` and
  `crates/archmeld-core/tests/cpp_scip_real.rs`; index
  `upstream-probes/scip/cpp.scip` sha256
  `059b0bb763ec00751a8e705a0aaa34104d30ebcd7e9b8c508ffb376703f050a8`.
- Limitation: the WSL host lacks `g++`, so scip-clang exited 0 with a
  resource-directory warning; SCIP covers references, while call semantics
  remain the clangd pack's assertions. The fixture is small.

### 2.5 B030 · JavaScript separate rename apply

- Status: **closed**.
- JavaScript now runs its own typescript-language-server rename through the
  same staging, exact prepared-digest approval and journaled apply path; the
  rebuilt node smoke output is unchanged and `src/other.js` keeps its
  same-named export.
- Evidence: `docs/release-evidence/B030-rename-apply.json` (`javascript` entry)
  and the JS/TS row in `docs/release-evidence/B030-language-matrix.json`.

## 3. Non-release-gate product gaps

These are real but do not block the AT067 language gate. They are still tracked
so they are not lost.

B016 native trace closure (2026-10-31): counters and state intervals are imported
from real tools, and a real `.tracy` capture is retained in the CAS. Perfetto
protobuf decoding, Chrome counter tracks and explicit probe JSONL are covered by
`perfetto_native` (5 tests) and `tracy_native` (3 tests); the CLI path is
`archmeld trace-import`. The bounded limitations — Tracy's native protocol is not
re-decoded, Tracy plots carry no units, its CSV exports no application state
intervals, and Perfetto SDK counters cannot carry debug annotations — are
recorded in `adapters/traces/fixtures.lock.json` and
`docs/release-evidence/B016-native-traces.json`.

B025 closure (2026-10-31): encrypted backups now record key provenance and can
be re-encrypted under a new passphrase with generation/`rotated_from` history,
and a verified bundle can be copied to a path-based retention target and
verified again on the destination. The operations are path-based: no cloud API,
credential or network transport is implemented, and the retention sidecar is not
authenticated. Evidence: `docs/release-evidence/B025-key-rotation-retention.json`,
`docs/release-evidence/B025-os-watch.json`, ADR-040.

B022 sandbox closure (2026-10-31): a contract check can declare a
`workspace_copy` sandbox. Verification materializes the selected files
from the verified resulting snapshot into a ledger-owned copy, confines
cwd and predicate artifacts to it, removes the copy on pass and retains
it with a manifest digest on failure. New contract proposals with a
command check must declare `workspace_copy` or `workspace`; an undeclared
command check is rejected. Legacy contracts record
`mode=workspace, declared=false, isolated=false` explicitly; the optional
field does not change their digests. This is a materialized allowlist,
not OS-level confinement: a child process can still open absolute paths
outside the copy. Evidence: `docs/release-evidence/B022-sandbox.json`,
ADR-043.

Palimpsest follow-ups (2026-11-28): brief findings/context/gaps, review
provenance, structural snapshot diff, rule exceptions, review cost and
report retention, and candidate-byte freeze are implemented and covered by
ADR-062…067. Three items remain explicitly bounded: incremental indexing is
not implemented (all index/refresh paths are full parse); real `reads`/`writes`
evidence is accepted only from an external/future export producer, because
CodeGraph 0.21.2 does not emit it; the reviewer study is a protocol only and
has not been run. The review benchmark `perf-review-2026-11-28-b` measures
capture/review/diff on a 200-file fixture and deliberately does not measure
graph indexing.

B026 webview paint closure (2026-10-31): the graph webview reports its own
paint timeline and render completion, and the extension-host harness measures
the nested `vscode-webview://` document through CDP: FCP 400-512 ms,
double-rAF 5.5-5.6 ms, probe round trip 8.7-13.4 ms, and first compositor
frame after a forced invalidation 18.3-22.0 ms over three real runs, all
within the declared budgets. The measurement does not cover GPU execution
time, shader cost or scan-out, and screencast intervals include capture
throttling, so they are recorded without a gate. Evidence:
`docs/release-evidence/B026-ui-webview-paint.json`, ADR-042.

Cross-review closure (2026-11-02): F01–F08 are closed for the testing phase
with real tests (MCP handler registry/capabilities, shared line anchors,
snapshot-first indexing, snapshot-bound source search, objective-driven
ContextPack with cursor, explicit edge truncation and cross-page boundary
edges, and scenario prepare/execute/finalize with cooperative cancellation).
Evidence: `docs/reviews/2026-11-02-cross-review-closure.md`,
`docs/release-evidence/CROSSREVIEW-2026-11-02.json`. F09 and F10 are closed for
the declared testing-phase scope: module and candidate-architecture component
projections retain relation/evidence ids, positions persist, runtime filtering
and drill-down work; status/review-diff/refresh exist in CLI, HTTP and MCP, and the
extension can discover project/snapshot. Residuals are explicit: runtime
span-parent edges are containment rather than confirmed calls, the
responsibility view requires an explicitly selected candidate architecture,
review is static and executes no tests, and the same-agent comparison
experiment has now been run with a ceiling-effect null result.

Same-agent comparison (2026-11-03): one paired run of the same general-purpose
agent and model answered the same four localization questions on a 7-file
fixture, with and without ARCHMELD. Both arms scored 17/17 localization fields,
4/4 fully correct tasks, 0 info requests and 0 unsupported conclusions. The
ARCHMELD arm additionally labeled the `run_report -> compute_metric` relation
as `static_call_candidate`/`candidate` rather than a resolved call, but used
more tool calls (14 vs 9), tokens (229,400 vs 140,449) and time (66.7 s vs
39.0 s). This closes the “no experiment was run” gap only; it is a single
small-fixture run and does not demonstrate an accuracy or productivity
advantage. Evidence:
`docs/release-evidence/AGENT-COMPARISON-2026-11-03.json`.

Agent onboarding closure (2026-11-11): `archmeld index` and `archmeld init`
now register the workspace root, so MCP `status` freshness and `review` work on
a real first run. `archmeld init` renders the Claude Code/Codex MCP command,
previews the `AGENTS.md`/`CLAUDE.md` rule block and runs a real
`initialize → tools/list → status` handshake. The new read-only
`archmeld_brief` tool composes status/review into a one-screen brief with
`counts` (confirmed/candidate/unknown/stale), `findings`, `unknowns` and
`next_actions`; query/context/source have bridge-side defaults. The implemented
MCP surface is eight read-only tools; no approve/apply tool is exposed. Final
core/API regression is 211 passed / 0 failed / 0 ignored (75 sections), and a
clean 4-file first run completed in 749 ms with a real MCP handshake. This
closes the usability/entry gap, not the commit-supervision or scoped
multi-agent work (M3/M4). Evidence:
`docs/release-evidence/M1-AGENT-ONBOARDING-2026-11-11.json` and ADR-056.

AOCI-CODE assessment (2026-11-24): the model-adherence gap is a workflow
problem, not a graph-accuracy problem. The 2026-11-03 experiment gave both arms
a full call sheet, so it measured evidence correctness, not whether an agent
chooses ARCHMELD on its own. AOCI-CODE makes a Git-versioned cognition map the
first thing every run reads and makes maintenance part of done; it is not a
call graph and costs about 140k tokens per session in a reported ERP case.
Decision: do not vendor or fork it; add a session-start rule contract and a
one-screen markdown brief, keep optional read-only AOCI interop, and gate the
evidence-bound cognition map on an adherence experiment. Implementation is
M2.5-A/B/C/D in `docs/assessments/AOCI-CODE-2026-11-24.md`; ADR-057 records the
boundary. This is a decision; M2.5-B/D and E1/E2 were implemented later, and
E4a was a no-go, so E stays at the read-only E2 surface.

Python cross-module same-name binding (2026-11-24, found by M2.5-D; **closed
2026-11-25**): in the adherence fixture, `from pricing import apply_discount`
in `checkout.py`, `wrapper.py` and `analytics.py` was bound by the graph to
`legacy.py::apply_discount` (an unrelated function returning `value * 0.5`),
while `pricing.py::apply_discount` got only the same-file `list_price` edge.
`impact` on the real function therefore under-reported callers. The resolver
used one global bare-name map for both import-symbol edges and unresolved
calls, so duplicate free functions collapsed to one declaration. Fix:
`codegraph-server` now builds per-file Python import bindings
(`from module import name`, `import module [as alias]`) and resolves imports
before the global fallback; a duplicate free-function name with no local/import
binding stays unresolved instead of guessing a homonym. Focused test 1/1,
watcher module 29/29; the M2.5-D export now binds `checkout`,
`summarize_order`, and `check_price` to `pricing.apply_discount` (15), not
`legacy` (13). `relation:` refs remain unimplemented; this was a prerequisite,
not the implementation. Evidence:
`docs/release-evidence/PYTHON-IMPORT-BINDING-2026-11-25.json` and
`docs/evidence/python-binding-2026-11-25/`.

Per-symbol import alias (2026-11-25, closed): `from m import x as y` was
dropped by the extractor; a call to `y` could stay unresolved or bind a
unique global homonym named `y`. The parser now emits one relation per alias
(`symbols=[x]`, `alias=y`), the resolver keeps exported and local bindings
separate, and an unresolved/ambiguous alias is shadowed so it cannot fall
back to a global name. Extractor 1/1, parser edge 1/1, watcher 34/34,
`codegraph-python` 122/0/1; the 16 remaining server-lib failures are the known
unrelated set. Evidence:
`docs/release-evidence/PYTHON-ALIAS-BINDING-2026-11-25.json` and
`docs/evidence/python-alias-binding-2026-11-25/`.

| Area | Missing | Closure condition |
|---|---|---|
| B000 | Closed for the Linux worker path: full Ubuntu 24.04 suite passed. Windows GNU/UCRT64 `codegraph --lib` now passes 59/59 including RocksDB; the historical 0xC0000005 was a Strawberry/winlibs GCC 13.2.0 mismatch. MSVC is unexercised but optional. Summary: `docs/release-evidence/WINDOWS-VERIFICATION-2026-11-27.json` | Run the full Windows suite with MSVC only if an MSVC-native delivery is required |
| F09 | Closed for testing phase; residual span-parent containment is not confirmed call semantics and component view needs an explicit architecture | Only promote span relations if trace evidence supports call semantics; keep projections labeled candidate |
| F10 | Closed for testing phase; `review-diff` is static and the same-agent comparison recorded a ceiling-effect null result | Execute tests separately before claiming correctness; a larger or multi-run benchmark is needed to measure real accuracy differences |
| Python same-name cross-module calls | **Closed 2026-11-25**: per-file Python import bindings resolve `from pricing import apply_discount` to `pricing.apply_discount`; duplicate free functions without a binding stay unresolved. Focused 1/1, watcher 29/29, M2.5-D export binds checkout/wrapper/analytics to pricing (15), not legacy (13) | Evidence `docs/release-evidence/PYTHON-IMPORT-BINDING-2026-11-25.json`; `relation:` refs remain a separate unimplemented feature |
| M2.5 adherence | A-D are implemented and tested (session rules, markdown card, AOCI block, session card, 3-pair experiment). D met the success line (full ritual B 3/3 vs A 0/3; tokens 0.942x; 6/6 correct). E1/E2 are implemented (map check/status/maintain, brief(recipe=map), status map, inspect map_entry). E2.1 adds semantic labels and `map_state`; E2.2 adds `S/A/refs` to `affected()`, `map_digest`/`freshness`, and clean absent/needs_snapshot states; core/API 240/0/0. E4a ran on `pi` 1.0.4 + `omni-anthropic/deepseek-v4.1-flash` with a 26-file nonlexical fixture: A (no map) 2/3 correct, B (map) 1/1 correct; the gate required A <=1/3, so it is **no-go**. E stays at E2 read-only; `map apply` (E3), E4b and rule v3 are not promoted | Closed for the testing phase at E2. Reopen E only with a different task/fixture and a new ADR; do not build `map apply` from this result. The Python same-name binding and per-symbol alias are closed; `relation:` refs remain a separate unimplemented item; all writes CLI-only |
| M3 commit supervision | Not implemented; `watch` observes workspace bytes and snapshots record HEAD, but there is no commit trigger, commit-range review or commit gate | Add `commit-review` CLI and a non-blocking post-commit hook; test merge/empty/rename/binary/submodule/shallow/detached cases |
| Upstream update hygiene | **Closed 2026-11-25 by U0/U1**: registry pin `ae4b2e54`, `upstream.py status/verify/check/probes/patches/diff/drill/rollback-drill/record`, patch manifest 0 unaccounted, weekly read-only workflow; `upstream_delta_drill.py` reads the registry and no longer decides AT077 | Keep the sealed `docs/archmeld/research/upstream-selection.json` as historical; no auto-merge/pin/push |
| M4 multi-agent authority | One bearer token, caller-supplied `actor` and one project writer lease; no scoped read/propose/apply/verify tokens or proposer≠approver enforcement | Add scoped tokens and separation-of-duties tests; keep one writer lease and never self-approve through MCP |

B018 is closed for the declared scope: four executable rule checkers and three `custom_contract` assertions cover the current requirements, and arbitrary user scripts are never executed. Adding another assertion is a normal code change with tests, not an open defect; there is no active assertion requirement.

B029 is closed for the comparison scope: authorized real data, ULP and statistical modes, sample/population variance, ignore_nan, matlab_eps, and contract-declared multiplicative units are implemented and tested. MATLAB expression-language semantics (colon/linspace, implicit expansion, column-major reshape) remain explicitly out of scope for a comparator and are not claimed (ADR-053).

B027 is closed for this testing phase: opt-in source/trace export with explicit risk labels and budgets, an inline-SVG viewer with no scripts or CDN, and a whole-ledger migration protocol with integrity checks and a receipt (ADR-052).

B028 is closed for this testing phase: contract checks and scenario runs have snapshot-copy/OCI container modes (ADR-043, ADR-050, ADR-051), the Unix socket is 0600 with a bearer token (ADR-047), and the Windows named pipe uses a protected custom DACL read back from the live handle (ADR-048).

B000 is closed for the supported Linux worker path: the full upstream `codegraph` suite ran in Ubuntu 24.04 with RocksDB, Rust 1.94.1, GCC 13.3 and Clang 18 — 141 passed (58 lib + 5 probes + 72 integration + 6 doctests), 0 failed, 1 ignored, exit 0. Evidence: `docs/release-evidence/B000-ubuntu-24.04.json` and `baseline/logs/b000-ubuntu-24.04.log`. On Windows GNU/UCRT64, `codegraph --lib` now passes 59/59 including `storage::rocksdb_backend::tests::test_delete`; the old `0xC0000005` was a Strawberry/winlibs GCC 13.2.0 runtime mismatch, not a current platform limitation. Windows MSVC is unexercised but not required for the RocksDB path.

B016 trace-processor closure (2026-10-31): `--format perfetto --trace-processor
`<path>` now queries the pinned `trace_processor_shell` with explicit SQL and
maps slices/counters to spans/counter series. The tool version and SHA-256
are recorded, and the result is compared with the in-process decoder on the
same fixture. States, flows and debug annotations stay unsupported and are
named as loss/limitations. Evidence:
`docs/release-evidence/B016-trace-processor.json`, ADR-046.

B017 model closure (2026-11-01): CodeBoarding was run with the authorized
`opencode go orange` / `deepseek-v4.1-flash` provider (key from the ZCode
provider config through a loopback header-injecting proxy) on a pinned
source copy. The real `analysis.json` (6 root components, 101 relations)
imported as 29 candidate components and 132 dependencies with all 136
source references resolved, zero ledger entities/relations added, and the
provider/model recorded in the architecture producer. The product
provider remains unconfigured by default. Evidence:
`docs/release-evidence/B017-model-interpretation.json`, ADR-049.

## 4. Release statement

Current test-phase statement: “AT067 seven-language gate passed; AT068
clean-machine indexing and offline report passed on Windows and Ubuntu; the
C++ WSL scip-clang v0.4.1 reference index was generated and imported; the
Ubuntu 24.04 upstream suite passed 141/0/1; the same-agent with/without
ARCHMELD comparison was executed and recorded a ceiling-effect null accuracy
result; M1 agent onboarding (`init`/`mcp install`/`brief`, eight read-only MCP
tools) was implemented and verified with a real MCP handshake; the real
upstream 0.21.2 upgrade drill (AT077) was executed and passed on 2026-11-24
(merge `1ca0ae8b`, core/API 211/0/0, PHP 89/0/0, contract tests 13/0/0,
end-to-end export 3 files/9 nodes/9 edges); commit supervision (M3) and scoped
multi-agent authority (M4) remain future work; the M2.5-A/B/C/D adherence
slices are implemented (session rules, markdown card, AOCI block, session card,
3-pair experiment), and E1/E2 are implemented; E4a ran and was no-go, so E3/E4b are not promoted. The P01–P25 self-analysis repairs are recorded in `SELF-ANALYSIS-FIX-2026-11-27.json`; B030 evidence was regenerated at HEAD `fd5c8758` on 2026-11-28. B031 was rebuilt from `fd5c8758` on 2026-11-28: both platform bundles report `commit_consistency=true`, Windows/Ubuntu CLI install, clean index, report and headless Chrome checks passed, and the package remains `release_status=provisional` because VSIX install was blocked by a missing VS Code host (`AT068=partial`, `blocked_external`), not because the binaries are stale. Ubuntu GUI/VS Code host and macOS are outside R1 (S15/AT068): unsupported/not tested, not deferred gates.” Do not claim R1 complete or a full release acceptance until the VSIX check has run on a host with VS Code.

## 5. Reproduce the open state

```text
# packaged index entry point (both platforms)
unzip -l dist/archmeld-cli-0.1.0-windows-x64.zip | grep -E "archmeld(-index)?\.exe"
# expected: bin/archmeld.exe and bin/archmeld-index.exe

# clean-prefix index/report/verify (Windows example)
archmeld index --workspace <repo> --state <state> --project project.clean
archmeld report --state <state> --project project.clean --snapshot <snapshot_id> --out <state>/report
archmeld verify-report --dir <state>/report

# current upstream head
git ls-remote upstream HEAD
# expected: ae4b2e54fc8ce1c074523943fc750b20f68bf8aa
```

## 6. Reproduce the same-agent comparison

The fixture, both exact prompts and the raw arm outputs are archived under
`docs/evidence/agent-comparison-2026-11-03/`. The origin fixture was at
`<path> git `df2a5d13…`, with an
uncommitted `src/change.py` line 2 change from `value * 0.9` to `value * 0.8`.

```text
target/debug/archmeld-serve.exe --state <state> --allow-workspace <fixture> --no-auth --port 0
# then send docs/evidence/agent-comparison-2026-11-03/prompts.md
# (common block + condition A or B) to two same-model subagents
py docs/evidence/agent-comparison-2026-11-03/grade.py
```

Recorded result: both arms 17/17 fields, 4/4 tasks, 0 info requests, 0
unsupported conclusions; ARCHMELD arm 14 tool calls / 229,400 tokens / 66.7 s
versus 9 / 140,449 / 39.0 s. This is a single-run ceiling effect, not an
advantage claim.
