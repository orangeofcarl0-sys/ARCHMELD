# B030 release evidence · language matrix and Case B

Status: **AT066 passed; AT067 passed for the seven-language gate. R1 is not yet
named complete because AT077's real newer-upstream upgrade is externally
blocked (upstream HEAD still equals the pinned commit).**

## AT066 · C++ capture → Python estimator → UI

Passed. The reference case is `examples/case-b/`:

- The C++ program is a **simulated capture device** (explicitly not hardware):
  it writes a deterministic 32-sample voltage CSV.
- The Python estimator consumes that real CSV and writes units/shape metadata.
  The static UI renders the real estimate with no scripts or remote assets.
- A real trace (`case-b-pipeline`) maps 3 spans to 3 source entities with 0
  unmapped spans and is bound to the base snapshot/build.
- A staged ChangeSet extracts one pure estimator update helper. Capture and UI
  bytes are unchanged; the estimator file changes.
- The frozen contract checks the pipeline test report, the full 32-sample
  numeric trace against the pre-change result from the C++ capture
  (`max_abs_error=0`), units/shape/finite range, and the UI document digest.
- Verification is independent, accepted separately, and an offline report is
  exported and checksum-verified.
- Evidence: `docs/evidence/B030-e2e-case-b.json`.
- Limits: the device is simulated; the scenario matrix (normal, slow consumer,
  bounded-queue backpressure, cancellation, disconnect) is in
  `docs/release-evidence/B030-case-b-scenarios.json`; the trace provides source
  mapping, not fabricated observed call edges; UI is static HTML, not the VS
  Code panel.

## AT067 · seven-language release matrix

Status: **passed** for the seven-language gate. Every row below has a real
result; missing behaviours remain listed as explicit limitations instead of
being averaged into a green score.

| Group | Structure/def-ref | Build | Runtime execution | Runtime linkage | Medium real project | Controlled change |
|---|---|---|---|---|---|---|
| C/C++ | verified (clangd) | verified fixture (g++) | verified `12.57 6` | explicit probe → 8001 mapped spans, 0 unmapped | Lua 5.4.7, 40 C files, build+`2+2` | clangd rename applied via ChangeSet `12.57 6` → `12.57 6` |
| Python | verified (Pyright) | verified (`compileall`) | verified `6` | VizTracer → 4002 mapped spans | CPython 3.11.9 asyncio, 33 files | Pyright rename applied via ChangeSet `6` → `6` |
| JS/TS | verified (tsc server) | verified (`tsc`, `node --check`) | verified `4` | V8 CPU profiles → mapped JS/TS samples (sampling is lossy; counts vary per run) | npm 24.14.1 lib, 117 files | JavaScript and TypeScript renames applied via ChangeSet `4` → `4` |
| Java | verified (jdtls) | verified (`javac`) | verified `24` | JFR → 12 mapped spans | OpenJDK 25 `java.util.concurrent`, 68 files | jdtls rename applied via ChangeSet, javac clean |
| C# | verified (csharp-ls) | verified (`dotnet build`) | verified `12` | EventPipe/speedscope → 61 mapped spans | Newtonsoft.Json 13.0.3, 235 files | csharp-ls rename applied via ChangeSet `12` → `12` |
| Go | verified (gopls) | verified (`go build/test`) | verified `2 4 6` | `runtime/pprof` summary → 4 mapped spans | Go 1.27.3 `net/http`, 158 files | gopls rename applied via ChangeSet `2 4 6` → `2 4 6` |
| Rust | verified (rust-analyzer) | verified (`cargo check/test`) | verified `2` | explicit probe → 4001 mapped spans | CodeGraph `crates/codegraph`, 44 files | rust-analyzer rename applied via ChangeSet `2` → `2` |

Evidence:

- `docs/release-evidence/B030-language-matrix.json` (matrix and acceptance state)
- `docs/release-evidence/B030-runtime-linkage.json` (native capture, import,
  bound mapping, sampling losses)
- `docs/release-evidence/B030-medium-projects.json` (pinned samples, archive
  hashes, source digests, builds)
- `docs/release-evidence/B030-rename-apply.json` (staging-only edits, exact
  prepared-digest approval, journaled apply, rebuild)
- `adapters/languages/fixtures.lock.json` (sample URLs, versions, licenses and
  hashes)
- `docs/evidence/B030-*.json` mirrors.

Commands: `#b030-runtime-linkage`, `#b030-medium-projects`,
`#b030-rename-apply`, `#b030-language-matrix`.

### Limitations kept explicit

1. C/C++ full SCIP indexing still needs a Linux/WSL worker. Windows clangd
   definition/reference and rename are verified; `scip-clang` has no official
   Windows binary.
2. Sampling tools report lossy captures: V8, JFR, EventPipe and Go pprof spans
   are sample intervals, not exact call durations. Each trace sets `lossy=true`
   and lists the loss notes; the importer keeps coverage `partial` for them.
4. The medium samples are real public projects/toolchain sources, not user
   repositories. Java uses the JDK source zip with `--patch-module`; the C#
   project restores NuGet packages on a new machine.

Runtime linkage only claims what was captured: explicit probes and sampled
stacks mapped to declared entity locations. It never promotes static candidates
and never presents sample intervals as exact call timing.
