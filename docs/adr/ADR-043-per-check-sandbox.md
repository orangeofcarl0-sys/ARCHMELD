# ADR-043: Per-check filesystem sandbox

Status: accepted for the project testing phase.

## Context

B022 runs frozen contract checks as child processes. A check with a command
historically ran with its cwd inside the authorized workspace, so a test command
could write into the repository. The verifier detects post-run drift, but
detection is not isolation: a check that writes a cache or report invalidates its
own evidence and can mutate files outside its declared scope before drift is
observed. B022 asks for each check to run in a declared sandbox/allowlist.

## Decision

1. A contract check may declare
   `"sandbox": {"mode": "workspace_copy", "include": [...], "exclude": [...]}`.
   `include`/`exclude` entries are workspace-relative POSIX prefixes matching a
   file or a directory subtree; `exclude` wins.
2. `workspace_copy` materializes exactly the selected files from the verified
   **resulting snapshot** (CAS blobs, not the mutable workspace) into
   `<ledger>/verify-sandboxes/<verification_id>/<check_id>/`. The command cwd
   must resolve inside that copy; absolute paths, `..` segments, symlinks and
   paths outside the copy are rejected before the command starts.
3. Predicates for that check read artifacts relative to the sandbox root, so a
   report cannot be sourced from the workspace.
4. A passing check removes its sandbox. Failed, errored, timed-out or cancelled
   checks retain it for inspection and record `retained: true`. The check detail
   records mode, include/exclude, source snapshot, file count, bytes, manifest
   digest and limitations.
5. `"mode": "workspace"` explicitly declares trusted-native execution without
   isolation and is always recorded as `isolated: false`. Every newly proposed
   command check must declare one of the two modes; an undeclared command check
   is rejected. Contracts frozen before this field existed remain verifiable
   and are recorded as `mode: workspace`, `declared: false`; the optional field
   is skipped when absent, so their digests stay stable.
6. Validation rejects an undeclared command check, a sandbox on a checker-only
   check, `workspace` mode with an allowlist, and `workspace_copy` with an empty
   include list. The published contract pack schema is not edited; the
   extension is tracked as `pending_pack_revision` in
   `contracts/archmeld/openapi-b022-extensions.patch.json`.

## Enforcement boundary

The copy is a materialized allowlist, not an OS-level jail. A child process can
still open absolute paths outside the sandbox with its own credentials; the
sandbox prevents accidental reads of excluded repository files and all writes to
the workspace, but it does not revoke filesystem permissions. This limitation is
recorded in every check detail and is the difference between this decision and
future container/OS isolation (B028).

## Evidence

- `docs/release-evidence/B022-sandbox.json` from a real verification run:
  a check read applied `src/a.py`, wrote `scratch/report.txt`, and the workspace
  stayed byte-identical; the sandbox had five files, 58 bytes, manifest digest
  `sha256:8289159c…`, and was removed after passing.
- Excluded files (`config/settings.toml`, `src/b.py`) were absent from the
  sandbox and the check asserted their absence.
- A cwd of `..` was rejected before execution with state `error` and
  `materialized: false`.
- Contract validation rejects an empty include list, a sandbox on a
  checker-only check, and `workspace` mode with an allowlist.
- Tests: `crates/archmeld-core/tests/verification.rs`, four sandbox cases.

## Consequences

- Checks can write caches/reports without invalidating the workspace snapshot.
- Verification is reproducible from the snapshot even if the workspace is
  concurrently edited after the pre-run drift check.
- Large repositories pay a copy cost proportional to the declared include set;
  contracts should keep includes narrow.
- OS-level confinement remains a separate B028 item.
