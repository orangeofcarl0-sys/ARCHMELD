# ADR-027 · Drift reports, consistent backups and pin-aware GC

Status: accepted for the R1 operational slice (B025); AT056.

## Decision

- Drift is computed by comparing the current workspace against an explicit base
  snapshot. The report lists changed/deleted/added paths, affected entities and
  relations, claims whose declared scope overlaps a changed path, affected
  architecture memberships, approved plans and prepared/applied change sets
  bound to the drifted base.
- A drift report is data, not a mutation: it never edits source, claim history,
  plan approvals or snapshots. Affected items are marked `needs_review`/`stale`
  only inside the report, so the prior decision and its evidence remain
  inspectable.
- Reopening the ledger after drift must still serve the old snapshot's entities
  and source and the accepted/rejected claim history. The test closes the Store
  and reopens the same state directory before asserting this.
- The watcher primitive is an on-demand poll (`analyze_drift`); a long-running
  filesystem watcher can call the same function. It is deliberately not a
  repo-wide reset or auto-apply trigger.
- Backups use SQLite `VACUUM INTO` for a consistent database snapshot and copy
  every CAS object into the backup directory with a manifest of verified
  digests. A backup is re-openable as a normal ledger and must pass
  `integrity_check`.
- GC is mark-and-sweep over the global CAS. References are collected from
  snapshots, files, evidence, runtime/scenario/check outputs, change/verification
  artifacts and idempotency results. Pinned object digests are protected. Only
  an explicit non-dry-run call deletes candidates; there is no owned-root-wide
  delete and no history rewrite.

## Residual limits

- Snapshot-count retention and automatic pruning of old snapshots are not
  enabled in this slice; the default policy keeps all analysis snapshots.
- Background filesystem notifications are not embedded; callers poll
  `analyze_drift` or schedule it.
- Backup retention and encryption are deployment concerns; this slice records
  the backup digest/manifest locally.
- GC is global to the ledger root, not per project; shared digests remain
  protected by references and pins.
