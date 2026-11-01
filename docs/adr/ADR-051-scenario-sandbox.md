# ADR-051: Scenario snapshot-copy and container execution

Status: accepted for the project testing phase.

## Context

Verification checks can already run inside a materialized snapshot copy or a
pinned OCI container (ADR-043, ADR-050). Scenario runs used by the
scenario/trace link and the VS Code runtime view still executed the caller's
command directly on the host workspace. B028 asks for the same isolation for
scenario execution, and a scenario manifest must say which isolation actually
applied.

A scenario is not a verification check: it has no predicate and its
`succeeded` state only means the process exited as requested. Isolation must
therefore not change the result vocabulary, and trace capture or stdio that the
target writes must stay observable through the normal stdout/stderr digests.

## Decision

1. `ScenarioSpec` gains an optional `sandbox` declaration with modes
   `workspace`, `workspace_copy` and `container`. Omitting it is the existing
   trusted-native mode and records `isolated=false`.
2. Snapshot materialization is shared with verification in
   `crates/archmeld-core/src/sandbox.rs`. Files are read only from the ledger
   CAS for the declared snapshot, selected by relative POSIX `include`/
   `exclude` prefixes, and written under
   `<state>/scenario-sandboxes/<project>/<run_id>/`. The manifest digest is
   computed over the selected path/digest/size rows.
3. `workspace_copy` runs the target on the host but with cwd inside the copy.
   The copy is removed after `succeeded`; any other completion retains it for
   inspection and records `retained=true`.
4. `container` starts a locally present, digest-pinned image
   (`name@sha256:...` or `sha256:<id>`) with
   `docker run --rm --network none --read-only --tmpfs /tmp:rw,size=64m
   --security-opt no-new-privileges --pull never` and the copy bind-mounted at
   `/workspace`. The image id, repo digests and platform are read from
   `docker image inspect`; a tag-only reference or a missing image is refused,
   never pulled.
5. The docker client runs with the same no-shell, environment allowlist,
   bounded-output, timeout and process-tree audit as every other command. The
   target environment is passed with `-e NAME=value`; the recorded
   `sandbox.exec_argv` replaces those values with `<redacted>`.
6. After every completion, including timeout and cancellation, the runner
   executes `docker rm -f <name>` when the container still exists and records
   `sandbox.container.removed`. A timeout or cancellation is never reported as
   `succeeded`.
7. Declarations that cannot be enforced fail before spawn: unknown mode, empty
   `include` for a copy, a non-digest image, `network` other than `none`, an
   absolute or `..` sandbox cwd, and `image`/`network` on a non-container mode.
8. The CLI exposes `--sandbox-mode`, `--sandbox-include`, `--sandbox-exclude`,
   `--sandbox-image`, `--sandbox-network`, `--sandbox-cwd`; the HTTP
   `POST /v1/scenarios` body accepts the same `sandbox` object and rejects
   unknown fields.

## Evidence

- `docs/release-evidence/B028-scenario-container.json`:
  - `debian@sha256:99c0b25c...` (local image, linux/amd64), `--network none`,
    `--read-only`, 64 MiB `/tmp`.
  - The target read `src/app.py` with the captured bytes while the live
    workspace held different bytes; it did not see `live_only.txt` or the
    excluded `config/secret.txt`; it could write `/tmp` and `/workspace`, could
    not write `/root`, and found no `eth0`.
  - The container was removed, the copy was removed, the authorized workspace
    stayed byte-identical, and the environment value did not appear in the
    manifest.
  - A `sleep 30` target under a 700 ms timeout ended `timed_out`, the container
    was removed, and the copy was retained.
- Tests: `crates/archmeld-core/tests/scenario_sandbox.rs` (container cases gated
  on `ARCHMELD_CONTAINER_IMAGE`), `crates/archmeld-api/tests/scenario_http.rs`,
  `crates/archmeld-api/tests/trace_import_cli.rs`.
- Commands are recorded in `baseline/commands.ndjson` under
  `#b028-scenario-container-tests` and `#b028-scenario-container-regression`.

## Consequences

- Scenario runs can now read exactly the captured snapshot and, in container
  mode, run without host network access.
- Docker/Linux only. The image must be provisioned locally; the runner never
  pulls. The container runs as the image default user and the bind mount is
  writable by design, so the target can modify the ledger-owned copy but not
  the authorized workspace.
- Container processes are not visible to the host process audit; the docker
  client is audited instead. CPU/memory limits are not applied.
- Trace capture still depends on the target writing into the copy or exporting
  through stdout; the scenario manifest records digests and completion, never a
  claim that the business contract passed.
