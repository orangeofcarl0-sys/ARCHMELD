# ARCHMELD remaining blockers

Audit date: 2026-11-02. Current phase: **project testing**. Full release-level
acceptance is not required in this phase; release-time items are recorded but do
not block current test work. Audit base: the current HEAD (use `git rev-parse HEAD`; this file was last refreshed after the F01–F10 cross-review closure; upstream HEAD was rechecked and is still `590c2ca7dd7b37bd1d41caf45fac535b6522f969`).
This file separates **closed** work from **still-blocking** work. A blocker is
only closed when the evidence path below exists and a real command produced it.
It is not a roadmap wish list: every open item names a reproduction and a
closure condition.

## 1. Cleared in this round

| Item | Result | Evidence |
|---|---|---|
| B030 AT067 runtime linkage | Seven groups captured with VizTracer, V8, clangd probe, JFR, EventPipe, Go pprof and a Rust probe; each trace imported into a bound snapshot and mapped to declared entities | `docs/release-evidence/B030-runtime-linkage.json`, command `#b030-runtime-linkage` |
| B030 AT067 medium real projects | Lua 5.4.7, CPython asyncio, npm lib, OpenJDK `java.util.concurrent`, Newtonsoft.Json 13.0.3, Go `net/http`, CodeGraph crate; archive/source digests and builds verified | `docs/release-evidence/B030-medium-projects.json`, `adapters/languages/fixtures.lock.json`, command `#b030-medium-projects` |
| B030 AT067 controlled rename | Seven real LSP renames applied to staging only, approved by exact prepared digest, journaled apply, rebuilt/rerun with identical output; same-named symbols untouched | `docs/release-evidence/B030-rename-apply.json`, command `#b030-rename-apply` |
| B030 AT066 scenarios | normal, slow consumer, bounded-queue backpressure, cancellation, simulated disconnect | `docs/release-evidence/B030-case-b-scenarios.json`, command `#b030-case-b-scenarios` |
| B014 Rust runtime trace | Explicit Rust probe, 4001 spans, 4001 mapped, 2 observed relations | `docs/release-evidence/B030-runtime-linkage.json` |
| B031 AT068 clean-machine indexing | Installed `archmeld` + `archmeld-index` index a workspace on Windows 11 and Ubuntu 24.04 with no Cargo/model/network | `docs/release-evidence/B031-install-tests.json` |
| B031 AT068 offline report | Installed CLI exports/verifies a 4-file bundle; Windows headless Chrome renders it with DNS disabled; Ubuntu static HTML checked | `docs/release-evidence/B031-install-tests.json` |

`AT067` is now `passed` in the language matrix with `r1_naming_allowed: true`
for the seven-language gate. AT068 clean-machine indexing/report is now closed.
R1 is still not declared complete: AT077's real upstream upgrade is externally
blocked; the C++ WSL reference index is verified, while Ubuntu GUI/macOS host
runs remain external.

## 2. Release-time items (deferred in the test phase)

### 2.1 B031 / AT068 · clean-machine indexing

- Status: **closed**.
- The installed bundle now carries `archmeld` and `archmeld-index`. The helper
  parses the workspace with the pinned CodeGraph parsers into an in-memory graph
  and writes `archmeld-export-v1` NDJSON; `archmeld index` captures the
  snapshot and imports the export through the evidence bridge.
- Windows 11 and Ubuntu 24.04 container clean-prefix runs both produced a
  ledger (11 entities / 7 relations on the fixture), with no Cargo, model or
  network in the installed flow.
- Evidence: `docs/release-evidence/B031-install-tests.json`
  (`windows_cli.clean_machine`, `linux_cli.clean_machine_index_report`).
- Product note: the helper is a full structural parse per run; it does not use
  the crashing `codegraph-server` wrapper or RocksDB.

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

- Status: **deferred release item / blocked external** (not a current test blocker).
- What exists: pinned `590c2ca7dd7b37bd1d41caf45fac535b6522f969`, NOTICE
  attribution, synthetic interface-rename drill (baseline 0, mutated 101,
  restored 0).
- Live check:
  `git ls-remote upstream HEAD` and `refs/heads/main` both return the pinned
  `590c2ca…`. There is no newer upstream commit to upgrade to.
- Closure condition: upstream advances; then run the adapter contract tests and
  a real delta drill against that checkout, update the lock deliberately, and
  keep the synthetic drill only as an additional early warning.
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
review is static and executes no tests, and no same-agent comparison experiment
was run.

| Area | Missing | Closure condition |
|---|---|---|
| B000 | Windows RocksDB/MSVC; full Ubuntu 24.04 suite | Install MSVC or use a supported Linux worker; run the full suite on the target Ubuntu image |
| F09 | Closed for testing phase; residual span-parent containment is not confirmed call semantics and component view needs an explicit architecture | Only promote span relations if trace evidence supports call semantics; keep projections labeled candidate |
| F10 | Closed for testing phase; `review-diff` is static and no same-agent comparison was run | Execute tests separately before claiming correctness; a future comparison needs a controlled agent experiment |

B018 is closed for the declared scope: four executable rule checkers and three `custom_contract` assertions cover the current requirements, and arbitrary user scripts are never executed. Adding another assertion is a normal code change with tests, not an open defect; there is no active assertion requirement.

B029 is closed for the comparison scope: authorized real data, ULP and statistical modes, sample/population variance, ignore_nan, matlab_eps, and contract-declared multiplicative units are implemented and tested. MATLAB expression-language semantics (colon/linspace, implicit expansion, column-major reshape) remain explicitly out of scope for a comparator and are not claimed (ADR-053).

B027 is closed for this testing phase: opt-in source/trace export with explicit risk labels and budgets, an inline-SVG viewer with no scripts or CDN, and a whole-ledger migration protocol with integrity checks and a receipt (ADR-052).

B028 is closed for this testing phase: contract checks and scenario runs have snapshot-copy/OCI container modes (ADR-043, ADR-050, ADR-051), the Unix socket is 0600 with a bearer token (ADR-047), and the Windows named pipe uses a protected custom DACL read back from the live handle (ADR-048).

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
C++ WSL scip-clang v0.4.1 reference index was generated and imported; AT077
real-upstream upgrade and Ubuntu GUI/macOS host runs remain deferred release
items.” Do not claim R1 complete or “real upstream upgrade verified”. The
deferred items are not current test blockers.

## 5. Reproduce the open state

```text
# packaged index entry point (both platforms)
unzip -l dist/archmeld-cli-0.1.0-windows-x64.zip | grep -E "archmeld(-index)?\.exe"
# expected: bin/archmeld.exe and bin/archmeld-index.exe

# clean-prefix index/report/verify (Windows example)
archmeld index --workspace <repo> --state <state> --project project.clean
archmeld report --state <state> --project project.clean --snapshot <snapshot_id> --out <state>/report
archmeld verify-report --dir <state>/report

# no newer upstream
git ls-remote upstream HEAD
# expected: 590c2ca7dd7b37bd1d41caf45fac535b6522f969
```
