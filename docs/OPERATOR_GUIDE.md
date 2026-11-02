# ARCHMELD operator guide

This guide is for the local operator of the current ARCHMELD work tree. AT067
passed for the seven-language gate; R1 is still not named complete because
AT068 clean-machine delivery and AT077 real-upstream upgrade remain partial. See
`docs/REMAINING_BLOCKERS.md`.

## 1. Build and test (Windows reference machine)

```sh
cd <repo>
RUSTUP_TOOLCHAIN=1.94.1-x86_64-pc-windows-gnu \
PATH=/d/Strawberry/c/bin:/c/Users/<user>/.cargo/bin:$PATH \
cargo test -p archmeld-core -p archmeld-api
```

Release binaries:

```sh
RUSTUP_TOOLCHAIN=1.94.1-x86_64-pc-windows-gnu \
PATH=/d/Strawberry/c/bin:/c/Users/<user>/.cargo/bin:$PATH \
cargo build --release -p archmeld-api --bins
RUSTUP_TOOLCHAIN=1.94.1-x86_64-pc-windows-gnu PATH=/d/Strawberry/c/bin:/c/Users/<user>/.cargo/bin:$PATH LIBCLANG_PATH=<workspace>/_archmeld_tools/pylibclang/clang/native cargo build --release -p codegraph-server --example archmeld_export
```

Offline clean-machine flow (installed bundle; no Cargo, model or network):

```sh
archmeld index --workspace F:/path/to/repo --state F:/state --project project.local
archmeld status --state F:/state --project project.local --workspace F:/path/to/repo
archmeld review-diff --state F:/state --project project.local --workspace F:/path/to/repo
archmeld refresh --state F:/state --project project.local --workspace F:/path/to/repo
archmeld report --state F:/state --project project.local   --snapshot <snapshot_id> --out F:/state/report
archmeld verify-report --dir F:/state/report
```

`status --workspace` and `review-diff` are read-only. The review reports
per-file line overlap, base/current digests, static relation impact, confidence
and `verification.state=not_run`; it never executes a repository command.
`refresh` creates a new snapshot and reports the exact file/entity/relation
delta against the previous snapshot; the old snapshot remains addressable.
See ADR-054.

The default bundle has no source or trace bytes. Opting in copies them from
the ledger's CAS and marks the whole bundle sensitive:

```sh
archmeld report --state F:/state --project project.local \
  --snapshot <snapshot_id> --out F:/state/report-with-source \
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
archmeld ledger-migrate --from F:/old-state --to F:/new-state \
  --reason "move the evidence ledger to the archive disk"
```

The target must not exist; merging into an existing ledger is refused. The
source is opened read-only. SQLite integrity/foreign keys/schema/table counts
and every CAS object (on read and after copy) are verified, and
`migration.json` records the receipt. Exports and verification/scenario
sandboxes are not copied.

Evaluate a numeric predicate directly (no ChangeSet is created):

```sh
archmeld numeric-check --state F:/state --root F:/artifacts \
  --predicate F:/predicates/ulp.json
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
archmeld backup --state F:/state --project project.local --out F:/backups/enc
archmeld verify-backup --dir F:/backups/enc
archmeld restore-backup --dir F:/backups/enc --target F:/restored-state
```

The backup uses PBKDF2-HMAC-SHA256 (120k), AES-256-GCM per file and an
HMAC-authenticated manifest. Restore refuses a non-empty target and verifies
before creating it. The manifest records a non-secret key fingerprint and
generation.

Rotate the passphrase and keep an off-host copy on a mounted path:

```sh
export ARCHMELD_BACKUP_NEW_PASSPHRASE='...'
archmeld rotate-backup --dir F:/backups/enc --out F:/backups/enc-v2
archmeld verify-backup --dir F:/backups/enc-v2        # with the new passphrase
archmeld retain-backup --dir F:/backups/enc-v2 \
  --target F:/mounted-remote/archmeld --retention-id retention.2026-10-31
archmeld verify-backup --dir F:/mounted-remote/archmeld/retention.2026-10-31
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
archmeld scenario --state F:/state --project project.local   --snapshot <snapshot_id> --workspace F:/path/to/repo   --grant-id grant.local.1 --reason "run the project test command" --   python -m pytest -q
```

stdout/stderr artifacts, exit code, timeout and process-tree audit are stored in
the ledger; a cancelled or timed-out run is never reported as complete.

A scenario can run against the captured snapshot instead of the live workspace,
and optionally inside a pinned container:

```sh
archmeld scenario --state F:/state --project project.local \
  --snapshot <snapshot_id> --workspace F:/path/to/repo \
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
archmeld trace-import --state F:/state --project project.local \
  --run-id run.pftrace.1 --snapshot <snapshot_id> \
  --format perfetto --input F:/traces/capture.pftrace \
  --tool perfetto-sdk --tool-version 59.1

archmeld trace-import --state F:/state --project project.local \
  --run-id run.tracy.1 --snapshot <snapshot_id> \
  --format tracy --tracy-capture F:/traces/capture.tracy \
  --tracy-csv F:/traces/capture.csv --locator F:/traces/locator.json \
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
archmeld trace-import --state F:/state --project project.local \
  --run-id run.t3.1 --snapshot <snapshot_id> \
  --format normalized --input F:/traces/t3.json \
  --memory-limit-mb 4096 --tool t3 --tool-version 1
```

On Windows the process is placed in a job object with process/job memory limits;
on Linux and macOS `RLIMIT_AS` is lowered. Exceeding the ceiling aborts the
process, and an atomic import leaves no partial runtime run. The chosen value is
recorded as `memory_limit_mb` in the output.

Poll a workspace for drift against a base snapshot (the watcher never writes the
workspace):

```sh
archmeld watch --state F:/state --project project.local --snapshot <snapshot_id> \
  --workspace F:/path/to/repo --interval-ms 1000 --iterations 10
```

Omit `--iterations` to run until the process is terminated. The default mode is
polling. Use `--mode os` to wait for real OS file notifications instead:

```sh
archmeld watch --state F:/state --project project.local --snapshot <snapshot_id> \
  --workspace F:/path/to/repo --mode os --debounce-ms 200 --iterations 10
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
  -v <repo>:/work -w /work \
  -e CARGO_TARGET_DIR=/work/target-linux \
  rust:1.94.1-bookworm cargo build --release -p archmeld-api
```

Packaging and release drills:

```sh
python tooling/package_release.py
ARCHMELD_VERSION=0.0.9 python tooling/package_release.py   # upgrade drill only
ARCHMELD_VERSION=0.1.0 python tooling/package_release.py
python tooling/test_release.py
python tooling/upstream_delta_drill.py
```

`dist/` is ignored by Git; release-evidence files record exact artifact hashes.
Artifacts are local and must not be uploaded without an explicit release
decision.

## 2. Run the local services

The server binds loopback only. Every request must use a loopback `Host` and must
not send `Origin`; bearer tokens are accepted only in `Authorization`.

```sh
target/release/archmeld-serve.exe --state <workspace>/_archmeld_state \
  --allow-workspace F:/path/to/repo --port 0
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
archmeld-serve --state F:/state --named-pipe "\.\piperchmeld" --token <token>
```

The pipe uses the default owner/SYSTEM/administrators DACL plus the bearer
token; a custom security descriptor is not set. See ADR-047 and ADR-048.

MCP read-only server:

```sh
target/release/archmeld-mcp.exe --state <workspace>/_archmeld_state \
  --project <project_id> --audit <workspace>/_archmeld_temp/mcp-audit.jsonl
```

The MCP surface advertises the contract tools but only inspect/query/context/
source are callable; run/write/apply tools return `permission_denied`. Content
returned from a repository is `untrusted` data and is never executed.

## 3. Data locations and safety

- State root: explicit `--state` directory; contains `state.sqlite3` and
  `objects/sha256/`.
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
- Tool missing or wrong version: `tooling/doctor.py` and
  `toolchains.lock.json` are the source of truth. Do not auto-install from a
  repository script.
- Clang/libclang build errors: use the locked toolchain paths in `AGENTS.md`.
- C: drive pressure: set `TMP/TEMP` and benchmark/temp roots to
  `<workspace>/_archmeld_temp`; `target-linux/` and `dist/` are ignored.

## 6. Current release status

- B030: AT066 passed; **AT067 failed**. No per-language runtime linkage/medium
  real projects; R1 naming is blocked.
- B031: AT068 partial, AT069 passed, AT077 partial. Packaging/install evidence
  exists; clean-machine indexing, packaged offline report opening, Ubuntu VS Code
  host and a real newer-upstream upgrade remain open.
- B017/B018/B016/B014 residuals and B022/B025 residuals are listed in
  `docs/archmeld-progress.md`.
