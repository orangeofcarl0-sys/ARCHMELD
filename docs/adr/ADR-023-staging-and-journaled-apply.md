# ADR-023 · Controlled staging, prepared digests and journaled apply

Status: accepted for the R1 change path (B020/B021).

## Context

S09 requires that agent or tool transforms never edit the analyzed repository
directly, that an approval binds the exact prepared content, and that a crash
during multi-file application is detectable and recoverable without a
repository-wide reset. The ledger and the filesystem cannot share one atomic
transaction, so the design must separate "what should change" from "what has
already been replaced".

## Decision

### Staging and preparation

- Preparation materializes base-snapshot bytes into
  `<state_root>/staging/<project>/<changeset>/`. The analyzed repository is
  read only through the CAS; staging is refused if it resolves inside an active
  project root.
- Operators or tools edit only that directory. `prepare` rescans it, rejects
  symlinks, non-UTF-8 files, reserved names, case-folding collisions, paths
  outside the plan step's `allowed_paths`, and edits to read-set context.
- Declared native moves must preserve content; a content-changing rename is
  split into move + update. Undeclared renames appear as delete + create and
  are never guessed back into a move.
- The diff records `before`/`after` digests and sizes. Every side is stored in
  the CAS; the prepared artifact is the canonical S19 preimage, and a separate
  canonical patch artifact names every operation exactly. The
  `prepared_digest` follows S19 §1 exactly and is recomputable by any client.
- Binary modifications are rejected in this version rather than silently
  treated as text. There are no directory operations; a create's parent
  directory must already exist at apply time.

### Approvals

- The plan-scope approval (B019) authorizes preparation only.
- `prepared_change` approvals are append-only rows that bind
  `(changeset_id, revision, prepared_digest)`. Re-staging creates a new
  revision and digest; the old approval remains history and no longer matches.
- Apply requires the exact approval row and the exact digest. A changed patch,
  expired approval, changed plan revision or changed frozen contract is
  rejected before any filesystem check.
- Two explicit reject transitions (`approved -> rejected`, `prepared ->
  rejected`) extend the sealed state machine; they are recorded in
  `contracts/archmeld/state-machine-b020-b021.patch.json` rather than silently
  editing the sealed pack.

### Apply

- The single-writer lease has an owner, an expiry and a monotonically
  increasing fencing token. A takeover after expiry increments the token; the
  old holder is refused at every subsequent per-file boundary.
- Preflight rechecks every target and read-set file against the prepared
  preimage and revalidates path roots, symlinks and Windows junctions. Any
  mismatch marks the ChangeSet stale and no replacement is performed.
- Before the first replacement, one SQLite transaction writes the `applying`
  revision, operation rows and journal intent rows. Each file then advances
  through `replace_started` -> `replaced`; a failure writes `failed` or
  `conflicted`. Cross-file atomicity is not claimed.
- Replacements use a same-directory temporary file, flush and rename; creates
  use an exclusive hard link so an appearing target is never overwritten.
  Deletes and moves are ordinary filesystem operations after a fresh hash
  check.
- After all replacements the workspace is captured as a new snapshot. The
  ChangeSet becomes `applied`, not `accepted`; required checks and architecture
  diff belong to B022.

### Recovery

- `applying` is an interrupted state, not a completed one. Recovery records the
  interruption as `failed`, then enters `recovering`.
- Journal rows are processed in reverse. An updated file is restored only when
  its current bytes equal the postimage; a created file is deleted only when it
  still matches the postimage; a deleted file is recreated from the preimage; a
  move is reversed from the destination.
- Any current content that differs is a conflict: the file is preserved, its
  bytes are stored as a user-version artifact, and the ChangeSet ends
  `conflicted` while other conflict-free files are still restored.
- Recovery never runs `git reset --hard`, `git clean`, or any whole-tree
  operation, and only touches paths present in this ChangeSet's journal.

## Residual limits

- Hash-check and rename are separate syscalls; a hostile process can still
  race the final window. Every existing ancestor and the target are rechecked,
  and the acceptance suite covers the prepare-to-apply link swap, but this is
  not a kernel-level transactional guarantee.
- A writer that crashes leaves its lease until expiry. Recovery must acquire
  the next fencing token after expiry; there is no process-liveness oracle.
- Apply revalidates the declared `read_set` and every operation target. Build
  or configuration files that influence the change must therefore be declared
  in `read_set`; unrelated files outside the patch are not treated as inputs.
- A ChangeSet that failed during preparation can be replanned in place once the
  staging content is corrected. A ChangeSet that failed during apply has a
  journal and must be recovered instead.
- Binary artifacts, directory creation/removal, and cross-file atomicity are
  out of scope for this version.

## Consequences

The system can now stage and apply real changes with byte-exact evidence,
refuse stale or partially approved changes, survive a crash after an arbitrary
replacement, and preserve later user edits instead of overwriting them.
