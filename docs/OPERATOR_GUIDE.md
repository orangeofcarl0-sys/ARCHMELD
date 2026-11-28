# ARCHMELD operator guide

This guide is for the local operator of the current ARCHMELD work tree. AT067
passed for the seven-language gate. R1 is defined by S15/AT068 as the Windows
11 x64 and Ubuntu 24.04 x64 CLI/container path; macOS and the Ubuntu GUI/VS Code
host are explicitly unsupported/not tested and are not R1 gates. AT068
CLI/container delivery is partial only because VSIX install is blocked on a
host without VS Code; AT077 passed. See `docs/REMAINING_BLOCKERS.md`.

## 1. Build and test (reference machine setup)

Tool and temp locations are resolved by `tooling/portable_env.py`; the commands
below are placeholders, not product defaults.

```sh
cd <repo-root>
export ARCHMELD_TOOLS_ROOT=<tools-root>
export ARCHMELD_TEMP=<temp-root>
export ARCHMELD_MINGW_BIN=<path>   # ARCHMELD_STRAWBERRY_BIN remains an alias
export RUSTUP_TOOLCHAIN=1.94.1-x86_64-pc-windows-gnu
python tooling/cargo_test.py -- -p archmeld-core -p archmeld-api
python tooling/portable_env.py --json
```

`cargo_test.py` is the supported test entry point: it sets the GNU toolchain,
prepends `ARCHMELD_MINGW_BIN` (so UCRT64's libstdc++ wins over Git's), resolves
libclang/bindgen includes, sets `CXXFLAGS` and `CARGO_INCREMENTAL=0`. A plain
`cargo test` resolves the MSVC host from `rust-toolchain.toml` and can load the
wrong `libstdc++-6.dll` at test runtime.

Release binaries:

```sh
export ARCHMELD_LIBCLANG_PATH=<tools-root>/pylibclang/clang/native
cargo build --release -p archmeld-api --bins
```

AT067/B030 language matrix (strict; fail instead of silently skipping):

```sh
export ARCHMELD_TOOLS_ROOT=<path>
export ARCHMELD_REQUIRE_TOOLS=1
export ARCHMELD_WRITE_EVIDENCE=1
export ARCHMELD_GCC=<path>
export ARCHMELD_GXX=<path>
export ARCHMELD_PYTHON=<absolute-python.exe>
python tooling/cargo_test.py -- -p archmeld-core --test language_runtime_linkage
python tooling/cargo_test.py -- -p archmeld-core --test language_medium_projects
python tooling/cargo_test.py -- -p archmeld-core --test language_rename_apply
python tooling/cargo_test.py -- -p archmeld-core --test language_matrix
python tooling/verify_b030_evidence.py --since <run-start-unix> --expect-head <short-head>
```

Helper builds (canonical; also writes the schema and provenance sidecars used by packaging):

```sh
export ARCHMELD_MINGW_BIN=<path>
export ARCHMELD_TEMP=<path>
python tooling/build_helper.py --profile dev --sidecar target/debug/helper-schema.txt
python tooling/build_helper.py --profile release --sidecar target/release/helper-schema.txt
```

The script reuses `cargo_env()`: it sets `CXXFLAGS=-include cstdint`, discovers
MinGW/GCC includes for bindgen, keeps the dev-profile `librocksdb-sys`
debuginfo disabled, and writes `helper-provenance.json` next to the helper
(schema/version/commit/dirty/sha256). A prose `BINDGEN_EXTRA_CLANG_ARGS` is
replaced with the resolved `-I` list. Keep `-j 2` for a source-built RocksDB on
the 16 GB host and never interrupt the C++ phase. The canonical environment now
sets both `CXXFLAGS` and `CXXFLAGS_x86_64_pc_windows_gnu` to the same value for
dev and release, so switching profiles does not create a second fingerprint.

### Fast and evidence test lanes

`tooling/test_lane.py` is the day-to-day entry point. The `fast` lane drops
`benchmark`, `t3_stress`, the four `language_*` AT067 tests and the seven
LSP/nested-cargo pack tests (`rust_pack`, `java_pack`, `go_pack`, `csharp_pack`,
`cpp_pack`, `language_packs`, `lsp_roundtrip`) from the routine loop, then runs
the remaining core tests, `archmeld-api --lib`, the parser suite and the stable
server tests in one invocation per group:

```sh
python tooling/test_lane.py --lane fast --dry-run   # show argv + env fingerprint
python tooling/test_lane.py --lane fast
python tooling/test_lane.py --lane core             # or server / parser
python tooling/test_lane.py --lane full             # everything, including packs
python tooling/test_lane.py --lane fast --include-timing
python tooling/test_lane.py --lane fast --include-external  # add the LSP packs
```

The excluded packs are not weakened: `--lane full` runs all of them, and
`--include-external` adds them to the fast selection when their coverage is
needed.

The fast lane defaults to the verified RocksDB cache, lld (after the probes
pass) and sccache (when installed). Every switch has an explicit fallback:
`--rocksdb source`, `--linker bfd`, `--sccache off`. The evidence lane always
uses source-built RocksDB, BFD and `-j 2`:

```sh
export ARCHMELD_TOOLS_ROOT=<path>
export ARCHMELD_REQUIRE_TOOLS=1
export ARCHMELD_WRITE_EVIDENCE=1
export ARCHMELD_GCC=<path>
export ARCHMELD_GXX=<path>
python tooling/test_lane.py --lane evidence
```

Reference run on the 24-core/16 GB host (2026-11-27): the warm fast lane took
**2m15s** (core 124s, API 1.1s, parser 4.8s, server 2.8s); a fresh target with
the prebuilt RocksDB archive built and ran `test_delete` in **1m43s**, versus
**16m14s** for the same server library with a source-built RocksDB. The
evidence lane keeps the source-built/BFD path and is expected to be slower.

### Prebuilt RocksDB cache (fast lane only)

The cache skips the 322 RocksDB C++ translation units. It is keyed by crate
version, features, target, rustc, GCC and `CXXFLAGS`, and a mismatch is a hard
error rather than a silent fallback:

```sh
python tooling/rocksdb_cache.py create --source-dir target/debug/build/librocksdb-sys-<hash>/out
python tooling/rocksdb_cache.py status
python tooling/rocksdb_cache.py verify --full   # link + test_delete runtime smoke
```

The archive and its `provenance.json` live under
`_archmeld_tools/rocksdb/<key>/`; the sha256 is recorded in fast-lane output.
The evidence lane never uses the cache.

### lld and sccache

```sh
python tooling/link_probe.py --probe --workspace --json
python tooling/install_sccache.py install --version 0.20.1
python tooling/install_sccache.py smoke
```

lld is used only after a real C link, a real Rust link and (when a cache is
available) a workspace link probe succeed; otherwise the fast lane stays on
BFD. sccache is pinned and installed from crates.io with `--locked`; its
version/sha256 are recorded in `provenance.json`, and it is never resolved from
"latest" at runtime. Both are fast-lane optimizations, not evidence inputs.

### Test debuginfo and temp root

`[profile.test] debug = 1` keeps file/line backtraces while shrinking the 100+
test executables; `dev` builds keep full debuginfo. For a failure that needs
variables, rerun the single test with `CARGO_PROFILE_TEST_DEBUG=2`.

Long GCC/linker command lines use response files. Keep `ARCHMELD_TEMP` on the
worktree drive (`_archmeld_temp` is the default when it exists); BFD `ld` on
Windows rejects `@<path> response-file paths.


Offline clean-machine flow (installed bundle; no Cargo, model or network):

```sh
# One-command onboarding: index, register the workspace root, render MCP
# configuration and run a real MCP handshake. Source files are not edited
# unless --write is passed for the AGENTS.md/CLAUDE.md rule block.
archmeld init --workspace <path> --state <path> --client claude,codex

# One-screen project/change brief (uses the registered workspace root).
archmeld brief --state <path> --project project.local --markdown

archmeld index --workspace <path> --state <path> --project project.local
archmeld status --state <path> --project project.local --workspace <path>
archmeld review-diff --state <path> --project project.local --workspace <path>
archmeld refresh --state <path> --project project.local --workspace <path>
archmeld report --state <path> --project project.local   --snapshot <snapshot_id> --out <path>
archmeld verify-report --dir <path>
```

`archmeld index` also registers the workspace root, so a later `archmeld-mcp`
can report `status` freshness and run `review` without an HTTP `openProject`
step.

`status --workspace` and `review-diff` are read-only. The review reports
per-file line overlap, base/current digests, static relation impact, confidence
and `verification.state=not_run`; it never executes a repository command.
`review-diff` also returns `provenance` (base/candidate Git and content-tree
identity), `timings`, and a `structural` block for snapshot-to-snapshot mode:
`review-diff --candidate-snapshot <id>` compares two indexed snapshots, detects
moves, stable relation changes and bounded convergent-path hypotheses. Use
`--persist` to store the result in `review_reports` and pin cited snapshots;
`--freeze --persist` additionally copies changed/added candidate bytes into the
CAS and pins their objects. `review-diff --report <id>` reads a persisted
report. See ADR-063, ADR-065, ADR-066 and ADR-067.
`refresh` creates a new snapshot and reports the exact file/entity/relation
delta against the previous snapshot; the old snapshot remains addressable.
See ADR-054.

The default bundle has no source or trace bytes. Opting in copies them from
the ledger's CAS and marks the whole bundle sensitive:

```sh
archmeld report --state <path> --project project.local \
  --snapshot <snapshot_id> --out <path> \
  --include-source --include-raw-traces
```

The manifest then records `source_state`/`raw_trace_state`
(`excluded`, `none`, `included` or `partial`), per-file `kind`,
`source_path`/`owners`, byte budgets (64 MiB source, 256 MiB traces) and
`risk_level=high`. `secrets_included` is
`unknown: no secret scanner ran`; the exporter does not redact. The HTML shows
a banner and lists files by path/digest, never inline bytes. The inline SVG
viewer uses anchor links and CSS `:target`; it has no scripts or remote URLs.
In VS Code, `ARCHMELD: Export Offline Report` asks for a safe output name,
offers the two opt-ins (both off by default) and requires a modal confirmation
before exporting a sensitive bundle.

Copy a complete ledger to a new location with integrity checks:

```sh
archmeld ledger-migrate --from <path> --to <path> \
  --reason "move the evidence ledger to the archive disk"
```

The target must not exist; merging into an existing ledger is refused. The
source is opened read-only. SQLite integrity/foreign keys/schema/table counts
and every CAS object (on read and after copy) are verified, and
`migration.json` records the receipt. Exports and verification/scenario
sandboxes are not copied.

Evaluate a numeric predicate directly (no ChangeSet is created):

```sh
archmeld numeric-check --state <path> --root <path> \
  --predicate <path>
```

`mode` may be `absolute_relative` (default), `ulp` (`max_ulps`) or
`statistical` (`statistics.mean_abs_error_max`, `rms_error_max`,
`max_abs_error_max`, `p95_abs_error_max`, `variance_max`, `stddev_max`;
`normalization` is `sample` by default or `population`).
`tolerance_model: "matlab_eps"` uses the IEEE-754 spacing at the compared
magnitude (`eps_multiplier`). `nonfinite_policy: "ignore_nan"` skips NaN pairs
and records `ignored_nan`. `unit_mode: "dimension"` compares unit dimension and
scale from the fixed SI table; `unit_definitions` adds up to 64
contract-declared multiplicative units. Unknown, affine or logarithmic units
are `inconclusive`. `semantics` accepts only `ieee754_elementwise`; MATLAB
expression-language semantics remain unsupported.

Encrypted local backup (passphrase from the environment, never argv):

```sh
export ARCHMELD_BACKUP_PASSPHRASE='...'
archmeld backup --state <path> --project project.local --out <path>
archmeld verify-backup --dir <path>
archmeld restore-backup --dir <path> --target <path>
```

The backup uses PBKDF2-HMAC-SHA256 (120k), AES-256-GCM per file and an
HMAC-authenticated manifest. Restore refuses a non-empty target and verifies
before creating it. The manifest records a non-secret key fingerprint and
generation.

Rotate the passphrase and keep an off-host copy on a mounted path:

```sh
export ARCHMELD_BACKUP_NEW_PASSPHRASE='...'
archmeld rotate-backup --dir <path> --out <path>
archmeld verify-backup --dir <path>        # with the new passphrase
archmeld retain-backup --dir <path> \
  --target <path> --retention-id retention.2026-10-31
archmeld verify-backup --dir <path>
```

Rotation re-encrypts every file with a fresh salt and nonce and records
`generation` and `rotated_from` in `key_provenance`; the source bundle is left in
place. Retention verifies the source, copies only manifest-listed files to the
target and verifies the copy before writing `retention.json`. The target is a
path on any mounted filesystem; no cloud API or credential is used, and the
retention sidecar is not authenticated.

Run one scenario against an existing snapshot (the cwd must resolve inside the
authorized workspace; the grant is explicit):

```sh
archmeld scenario --state <path> --project project.local   --snapshot <snapshot_id> --workspace <path>   --grant-id grant.local.1 --reason "run the project test command" --   python -m pytest -q
```

stdout/stderr artifacts, exit code, timeout and process-tree audit are stored in
the ledger; a cancelled or timed-out run is never reported as complete.

A scenario can run against the captured snapshot instead of the live workspace,
and optionally inside a pinned container:

```sh
archmeld scenario --state <path> --project project.local \
  --snapshot <snapshot_id> --workspace <path> \
  --grant-id grant.local.1 --reason "run the project test command" \
  --sandbox-mode container --sandbox-include src --sandbox-include tests \
  --sandbox-image debian@sha256:<digest> --sandbox-network none \
  --sandbox-cwd . -- python -m pytest -q
```

`workspace_copy` runs on the host with cwd inside the ledger-owned copy;
`container` additionally uses `--network none`, a read-only root, a 64 MiB
`/tmp` tmpfs and `no-new-privileges`. The image must already exist locally and
be referenced by digest; tags are refused and nothing is pulled. The manifest
records `sandbox.mode`, the image identity, the materialized file count and
digest, the container workdir and whether the container was removed. A
succeeded run removes the copy; every other completion retains it with
`retained=true`. Environment values are passed to the target but redacted in the
recorded `exec_argv`; only names appear in `env_names`.

Frozen contract checks may declare a per-check filesystem sandbox:

```json
{
  "id": "check.tests",
  "command": {"argv": ["python", "-m", "pytest", "-q"], "cwd": ".", "timeout_ms": 120000},
  "sandbox": {"mode": "workspace_copy", "include": ["src", "tests", "pyproject.toml"]}
}
```

`workspace_copy` materializes the selected files from the verified resulting
snapshot into `<ledger>/verify-sandboxes/<verification_id>/<check_id>/`, runs the
command with cwd inside that copy, and resolves predicate artifacts there.
For OS isolation use `"mode": "container"` with a digest-pinned local image
(`"image": "debian@sha256:..."`); the copy is mounted at `/workspace`, the
container runs with `--network none`, a read-only root, a 64 MiB `/tmp` tmpfs and
`no-new-privileges`, and the image is never pulled. Docker/Linux containers are
required. See ADR-050. A
passing check removes the copy; a failed one retains it and records its manifest
digest. Every newly proposed command check must declare a sandbox: `workspace_copy`
for an isolated snapshot copy, or `"mode": "workspace"` for explicit
trusted-native execution. Contracts frozen before the field existed are
recorded as `declared=false, isolated=false` and remain verifiable. This is a
materialized allowlist, not an OS jail: the child process can still open
absolute paths outside the copy. See ADR-043.

Import a native runtime trace into an existing snapshot. The locator file is
optional and maps only exact locations or explicit probe symbols; counters and
states never create causal edges.

```sh
archmeld trace-import --state <path> --project project.local \
  --run-id run.pftrace.1 --snapshot <snapshot_id> \
  --format perfetto --input <path> \
  --tool perfetto-sdk --tool-version 59.1

archmeld trace-import --state <path> --project project.local \
  --run-id run.tracy.1 --snapshot <snapshot_id> \
  --format tracy --tracy-capture <path> \
  --tracy-csv <path> --locator <path> \
  --tool tracy --tool-version 0.13.1
```

`--format probe` reads explicit JSONL span/counter/state lines, `--format
chrome` reads Chrome Trace JSON (counter tracks `ph:"C"` included), and
`--format normalized` imports an explicit `archmeld-trace-v1` envelope. A trace
without a snapshot requires `--allow-unbound`; a `--config-digest` that
disagrees with the snapshot build is rejected. Pass
`--scenario-run-id <run_id>` to link the trace to an existing scenario run: the
run must belong to the project and match the trace snapshot/build config, and
the timeline and evidence then report `scenario_run_id`. Without the flag, a
`scenario_id` embedded in the trace is recorded as producer-declared and
unverified, not as a link. For Tracy, produce the CSV with
`tracy-csvexport -u -p capture.tracy`; the raw `.tracy` file is retained in the
CAS. See `adapters/traces/fixtures.lock.json` for the pinned fixture provenance.
For Perfetto, add `--trace-processor <trace_processor_shell>` to query slices
and counters through the pinned SQL backend instead of the in-process decoder;
the tool version and binary hash are reported. See ADR-046.
In VS Code, `ARCHMELD: Open Trace Frame` (`archmeld.openTraceFrame`) lists the
run's zones with retained Tracy source locations and opens the exact line from
the snapshot CAS; `POST /v1/source/file` and `GET /v1/runtime/{run_id}` back it.
See ADR-045.

Add a hard memory ceiling to an import (applied before the trace is read):

```sh
archmeld trace-import --state <path> --project project.local \
  --run-id run.t3.1 --snapshot <snapshot_id> \
  --format normalized --input <path> \
  --memory-limit-mb 4096 --tool t3 --tool-version 1
```

On Windows the process is placed in a job object with process/job memory limits;
on Linux and macOS `RLIMIT_AS` is lowered. Exceeding the ceiling aborts the
process, and an atomic import leaves no partial runtime run. The chosen value is
recorded as `memory_limit_mb` in the output.

Poll a workspace for drift against a base snapshot (the watcher never writes the
workspace):

```sh
archmeld watch --state <path> --project project.local --snapshot <snapshot_id> \
  --workspace <path> --interval-ms 1000 --iterations 10
```

Omit `--iterations` to run until the process is terminated. The default mode is
polling. Use `--mode os` to wait for real OS file notifications instead:

```sh
archmeld watch --state <path> --project project.local --snapshot <snapshot_id> \
  --workspace <path> --mode os --debounce-ms 200 --iterations 10
```

In OS mode the initial report is always written, each event only wakes the
watcher, and the workspace is re-hashed after the debounce quiet period. A
dropped or coalesced notification cannot produce a false clean report, but it can
delay detection until the next event. Neither mode writes the workspace.

`archmeld` calls the sibling `archmeld-index` helper. The helper parses the
workspace with the pinned CodeGraph parsers into an in-memory graph and writes
`archmeld-export-v1` NDJSON under `<state>/index-runs/`; it does not open
RocksDB, load an embedding model, or start a watcher. The report is static and
script-free; open `report.html` with networking disabled and verify the
manifest before trusting a copied bundle.

Linux release build (pinned image):

```sh
MSYS_NO_PATHCONV=1 docker run --rm \
  -v <repo-root>:/work -w /work \
  -e CARGO_TARGET_DIR=/work/target-linux \
  rust:1.94.1-bookworm cargo build --release -p archmeld-api
```

Packaging and release drills:

```sh
python tooling/package_release.py
ARCHMELD_VERSION=0.0.9 python tooling/package_release.py   # upgrade drill only
ARCHMELD_VERSION=0.1.0 python tooling/package_release.py
python tooling/test_release.py
python tooling/upstream.py status --remote --json
python tooling/upstream.py verify --skip-cargo --json
python tooling/upstream.py drill codegraph --ref <release-tag-or-release-commit> --run --matrix quick --json
python tooling/upstream.py rollback-drill codegraph --to 903bfe0a --run --matrix full \
  --probe-revert 1ca0ae8b --json
```

Upgrade and rollback procedures are in
`docs/upstream/UPGRADE_PLAYBOOK.md` and `docs/upstream/ROLLBACK.md`. The old
`upstream_delta_drill.py` is superseded by `upstream.py`.

`dist/` is ignored by Git; release-evidence files record exact artifact hashes.
Artifacts are local and must not be uploaded without an explicit release
decision.

### Public remote push guard

The public fork only accepts `main` and `public-snapshot-*` branches. Enable
the tracked pre-push hook once per clone:

```sh
py -3.11 tooling/publication_guard.py --install
```

The hook reads Git's pre-push ref list and blocks private branches such as
`archmeld/initial` on the public fork. It allows branch deletion for cleanup
and is a no-op for a private remote. The same check runs on pushes through
`.github/workflows/archmeld-public-branch-guard.yml`. Override only for an
explicit cleanup:

```sh
ARCHMELD_ALLOW_PUBLIC_BRANCH_PUSH=1 git push origin --delete archmeld/initial
```

Publication still requires a sanitized snapshot:

```sh
python tooling/public_snapshot.py export --out <dir> --ref HEAD
python tooling/public_snapshot.py verify-export <dir>
```

## 2. Run the local services

The server binds loopback only. Every request must use a loopback `Host` and must
not send `Origin`; bearer tokens are accepted only in `Authorization`.

```sh
target/release/archmeld-serve.exe --state <state-root> \
  --allow-workspace <workspace> --port 0
```

The single stdout control line contains the endpoint and generated token. Do not
put the token in a URL, share it, or paste it into logs. For local tests only,
`--no-auth` is allowed; it still enforces Host/Origin.

On Unix, serve the same API over a domain socket with mode 0600; an existing
path is refused and the bearer token is still required:

```sh
archmeld-serve --state /path/to/state --unix-socket /run/user/1000/archmeld.sock --token <token>
```

Windows rejects `--unix-socket` explicitly instead of falling back to TCP. Use a
named pipe there:

```powershell
archmeld-serve --state <path> --named-pipe "\\.\pipe\archmeld" --token <token>
```

The pipe uses the default owner/SYSTEM/administrators DACL plus the bearer
token; a custom security descriptor is not set. See ADR-047 and ADR-048.

MCP read-only server:

```sh
target/release/archmeld-mcp.exe --state <state-root> \
  --project <project_id> --audit <temp-root>/mcp-audit.jsonl
```

The MCP surface advertises only handlers this build can execute:
`archmeld_status`, `archmeld_brief`, `archmeld_review`, `archmeld_query`,
`archmeld_context`, `archmeld_source`, `archmeld_inspect` and
`archmeld_capabilities`. Run/write/apply tools are not exposed and return
`permission_denied`; the contract's broader grant-only declarations are reported
by `archmeld_capabilities`. Content returned from a repository is `untrusted`
data and is never executed.

`archmeld_brief` returns the JSON result plus a one-screen markdown card. A
change brief separates changed `findings` from unchanged `context` and per-file
`gaps`; `max_findings`, `max_context`, `max_gaps` and `max_relation_items`
bound the output and `budget.omitted_*` reports truncation. AOCI entries are
read-only context and are never parsed relations; a missing or malformed map is
a warning, not an error.

`archmeld brief --write` writes the same card to
`<workspace>/.archmeld/SESSION.md` (or `--out <path>`), with a header carrying
the snapshot, commit, coverage, workspace state and counts. `archmeld status`
reports `session_card.present`, `marker`, `snapshot_match` and `stale` for the
active workspace; a missing card is not stale. The card is a projection, not a
source of truth; an existing non-ARCHMELD file is only overwritten with
`--force`.

E1/E2 map commands are read-only:

```sh
archmeld map check --state <state> --project <id> --workspace <repo>
archmeld map status --state <state> --project <id> --workspace <repo> --markdown
archmeld map maintain --state <state> --project <id> --workspace <repo>
archmeld brief --state <state> --project <id> --workspace <repo> --recipe map
```

`map check`/`status`/`maintain` exit 1 when the map state is `invalid`;
`absent`/`stale` exit 0. `archmeld status` adds a `map` summary and
`archmeld_inspect(kind=map_entry, id=<path>)` reads one Entry. `map apply`
(E3) is not implemented; nothing here writes the map.

## 3. Data locations and safety

- State root: explicit `--state` directory; contains `state.sqlite3` and
  `objects/sha256/`.
- Workspace card: `<workspace>/.archmeld/SESSION.md` when `brief --write` is
  used; `.archmeld/` is skipped by the snapshot and drift walkers, so the card
  never counts as source drift. It is generated metadata, not a source of
  truth.
- Staging: `<state>/staging/<project>/<changeset>/`; the active workspace is not
  written during preparation.
- Reports: core/API exports; the HTTP route places them under
  `<state>/exports/`.
- Backups: `VACUUM INTO` copy plus CAS objects and a checksum manifest.
- Retention: opt-in `keep_snapshots`; referenced snapshots and pins are never
  deleted. Use dry-run first.
- Uninstalling the CLI/VSIX removes binaries/extension files only. It does not
  delete project data or the state root.
- The workspace is `externally_writable`; another shell-capable agent can change
  it. Stale ChangeSets are rejected by read_set/preflight hashes.

## 4. Change and verification flow

1. Open a project and capture a snapshot.
2. Propose claims/architecture candidates; they remain candidates.
3. Freeze a BehaviorContract and approve a plan.
4. Prepare a ChangeSet; inspect the staged diff and prepared digest.
5. Approve the exact prepared digest.
6. Apply; the writer lease, preflight and journal protect the operation.
7. Verify the applied resulting snapshot; required checks must pass.
8. Accept separately; accept rechecks snapshot drift.
9. Export an offline report if needed; source/traces are excluded by default.

Never bypass staging with direct writes, never edit tests/tolerances to pass,
and never call a partial/unsupported check accepted.

## 5. Troubleshooting

- `schema N is newer than this binary`: use the matching binary; do not
  downgrade the database.
- `stale_snapshot`/`evidence_invalid`: the workspace changed after preparation,
  verification or acceptance. Re-snapshot and review; do not force apply.
- Missing CAS object: restore from a consistent backup; promotion and reports
  fail closed.
- Tool missing or wrong version: `tooling/doctor.py`, `toolchains.lock.json`
  and `tooling/portable_env.py --json` are the source of truth. Do not
  auto-install from a repository script.
- Clang/libclang build errors: set `ARCHMELD_LIBCLANG_PATH` (or
  `ARCHMELD_TOOLS_ROOT`); do not hardcode a local path in product code.
- C: drive pressure: set `ARCHMELD_TEMP` (and `TMP`/`TEMP` if a child tool needs
  them); `target-linux/` and `dist/` are ignored.

## 6. Current release status

- B030: AT066 and **AT067 passed** for the seven-language gate; C/C++ full SCIP
  is exercised on the WSL worker and Ubuntu GUI/macOS host runs remain
  release-level items.
- B031: AT068 passed, AT069 passed, **AT077 passed (2026-11-24)**. Packaging,
  clean-machine indexing, offline report opening and the real 0.21.2 upstream
  upgrade have evidence; the Ubuntu VS Code host run remains open.
- B017/B018/B016/B014 residuals and B022/B025 residuals are listed in
  `docs/archmeld-progress.md`.
