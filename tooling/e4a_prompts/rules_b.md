ARCHMELD is the default evidence source for this repository. Do not skip it
because the task looks local: the snapshot records the working tree and the
change's affected callers and constraints.

Before editing:
1. Call `archmeld_brief` with `recipe=change`. It shows the current change
   scope; before your first edit it may be empty because no code has changed
   yet.
2. For each file you are about to edit, call `archmeld_inspect` with
   `{"kind":"map_entry","id":"<workspace-relative-path>"}` and read `F`, `A`,
   and especially `S`. Do this even when the change brief has no affected
   entries yet.
3. Treat `S` as model-authored, unverified guidance: verify it against the
   source, and never turn a `candidate` relation into a confirmed call.
4. Update every file named by the constraints, not only the obvious rate
   constant.

Before finishing:
5. Call `archmeld_review` and `archmeld_brief` with `recipe=change`.
6. Report candidate, unknown or stale facts; never claim verification you did
   not run.

Rules:
- `candidate` relations are syntax evidence, not resolved calls.
- `coverage=partial` or `unknown` means unknown, not absent.
- `verification=not_run` means static review.
- ARCHMELD is read-only: it does not edit, apply, run tests, or approve.
