# ADR-050: OCI container sandbox for contract checks

Status: accepted for the project testing phase.

## Context

The B022 `workspace_copy` sandbox materializes a snapshot copy and confines the
cwd, but it is not OS isolation: the child process keeps the host user's
filesystem and network access. B028 asks for an OS/container isolation mode.
Contract checks are the path that runs third-party commands during acceptance,
so they are the first place that needs real confinement.

## Decision

1. A contract check may declare
   `"sandbox": {"mode": "container", "include": [...], "exclude": [...],
   "image": "name@sha256:...", "network": "none"}`.
2. The image must be pinned by digest (`name@sha256:...` or a local
   `sha256:<id>`). The verifier inspects the local image, records its ID,
   repository digests and platform, requires `linux/*`, and never pulls
   (`--pull=never`). A missing or mismatched image fails the check closed.
3. The snapshot copy is materialized exactly as in `workspace_copy` and mounted
   at `/workspace`. The command runs with:
   - `--network none` (the contract may omit or set only `none`),
   - `--read-only` root filesystem,
   - `--tmpfs /tmp:rw,size=64m`,
   - `--security-opt no-new-privileges`,
   - `--rm` and a deterministic container name.
4. The command cwd is mapped to `/workspace/<relative cwd>`. Predicates still
   read artifacts from the host copy, so reports produced inside the container
   are verified without touching the workspace.
5. After the command (including a timeout), the verifier checks whether the
   container still exists and runs `docker rm -f` if needed; the result records
   `container.removed`. A passing check removes the host copy; a failed check
   retains it with the manifest digest.
6. The container runs as the image's default user and can write the bind-mounted
   copy. No custom seccomp profile, user mapping or Windows-container mode is
   claimed.

## Evidence

- Pinned `debian@sha256:99c0b25c…` (`linux/amd64`) with a real check:
  the command read the included `src/a.py`, could not see excluded
  `config/settings.toml` or `src/b.py`, wrote `/workspace/container-report.txt`
  and `/tmp/t`, failed to write `/root/x` (read-only root), found no `eth0`, and
  printed `container-ok`.
- The check passed, the container was removed, the host copy was removed, the
  authorized workspace stayed byte-identical, and no report appeared in the
  workspace.
- `docs/release-evidence/B022-container-sandbox.json`, test
  `crates/archmeld-core/tests/verification.rs` (gated on
  `ARCHMELD_CONTAINER_IMAGE`), command `#b028-container-sandbox-tests`.

## Consequences

- Verification checks can run untrusted commands with filesystem and network
  isolation instead of the workspace-copy allowlist alone.
- A container image must be provisioned locally; the verifier never pulls.
- Scenario execution gained the same snapshot-copy and container modes in
  ADR-051; this decision only covers contract checks.
- The default user inside the container is usually root; the copy is writable by
  design, and the host workspace is not mounted.
