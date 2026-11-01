# ADR-026 · Optional asynchronous Instigator

Status: accepted for the R1 enhancement (B024); disabled by default.

## Decision

- The Instigator is an optional policy adapter over existing job/task rows. It
  is disabled unless a project explicitly configures it.
- Its only outputs are `NUDGE` and `SILENT`. It cannot create a task or goal,
  change a task revision/state, approve, apply, recover, or block delivery.
- A nudge names an existing task id and its exact revision. The scope reference
  must be that task; new scopes are refused. Stale revisions, expired
  messages, finished/missing tasks, cooldown windows and per-task budgets all
  produce `SILENT` without creating work.
- Proposal and delivery are separate non-blocking calls. Delivery rechecks the
  task revision, terminal state and expiry. Only a still-valid message becomes
  `delivered` and emits `instigator.nudge`; everything else is marked
  `discarded` with a reason.
- Disabling the Instigator discards pending messages on the next delivery pass
  and never affects task transitions. A task can finish while the Instigator
  is disabled, delayed or never invoked.

## Residual limits

- The task model is the existing job state machine; there is no separate global
  task graph yet.
- Message history is retained for audit; retention/GC follows the general event
  retention work (B025).
- The policy is local and deterministic; no model call is made to decide a
  nudge.
