# ADR-029 · Count-based snapshot retention without evidence loss

Status: accepted for the R1 retention slice (B025 residual).

## Decision

- Retention is opt-in per project (`retention_config`). It is not a general
  purge and never runs unless a project configures `keep_snapshots`, with
  optional capture-time `auto_prune`.
- A snapshot outside the newest N is deleted only when no plan, ChangeSet,
  verification, claim, architecture, investigation, drift report, evidence row,
  runtime run, scenario run, job result, project-current pointer or explicit
  retention pin references it. Referenced snapshots remain queryable and the
  retention result lists the exact reasons.
- An in-progress analysis (`generations.status='staging'`, non-final
  `run_batches`, or staging rows) is retained as `active_import`.
- Deletion is one SQLite transaction. Child rows (relation evidence, relations,
  backend mappings, diagnostics, evidence, staging rows, run batches,
  generations, files) are removed before the snapshot row. CAS objects are not
  deleted here; a separate mark-and-sweep GC decides whether they are
  unreferenced. This keeps manifest blobs readable for diagnostics until GC.
- Dry-run is the default for manual pruning. A non-dry-run manual or automatic
  run records `retention_runs` with keep count, candidate/kept/deleted counts
  and the full decision list.
- Event rows are notifications, not an independent fact store. They do not pin
  a snapshot and retention does not rewrite the event log; old events are
  pruned through the separate event-retention watermark so a reconnecting
  client receives `resync_required` rather than silently missing history.
- Capture-time auto-pruning runs after a new snapshot is published. A retention
  error is reported in `CapturedSnapshot.retention` and does not roll back the
  already-published snapshot.

## Residual limits

- Backup encryption and remote retention are not implemented; backups are
  local, consistent, checksum-manifested directories.
- A background filesystem watcher is not part of this decision; drift remains
  an explicit API call, and a watcher must not make retention or drift
  decisions on its own.
