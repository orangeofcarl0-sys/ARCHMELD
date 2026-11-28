<!-- ARCHMELD:BEGIN -->
## ARCHMELD: local, read-only code evidence

ARCHMELD is the default evidence source for this repository. Do not skip it
because a task looks local: the snapshot records the working tree and the
change's affected callers.

### Session start (before planning or editing)

1. Call `archmeld_status` once. If a snapshot exists, call `archmeld_brief`
   with `recipe=auto`.
2. Put the brief's `counts`, `findings` (changed entities only), `context`
   (unchanged callers/anchors), `gaps`, `budget.omitted_*`, `unknowns` and
   `next_actions` into your plan. Do not treat context or gaps as changed
   entities. Cite the snapshot id and coverage when they change the answer.
3. Before editing a symbol, call `archmeld_query` with `kind=impact` for that
   symbol.

### Before you finish

4. Call `archmeld_review` and `archmeld_brief` with `recipe=change`.
5. Report the changed entities, affected callers/relations, and any
   `candidate`, `unknown` or `stale` facts. Never claim verification you did
   not run.

### Intent mapping

| User intent | First call | Then |
|---|---|---|
| "What changed / is this safe?" | `archmeld_brief` `recipe=change` | `archmeld_review`, `archmeld_source` |
| "Summarize this project" | `archmeld_brief` `recipe=project` | `archmeld_context`, `archmeld_query` |
| "Who calls this / what breaks?" | `archmeld_query` `kind=impact` | `archmeld_source`, `archmeld_context` |
| "Where is this symbol?" | `archmeld_query` `kind=symbols` | `archmeld_source` |
| "What does this code do?" | `archmeld_source` | `archmeld_query` for neighbors |

### Rules

- `candidate` relations are syntax evidence, not resolved calls. Never present
  them as confirmed.
- `coverage=partial` or `unknown` means unknown, not absent.
- `verification=not_run` means static review; ARCHMELD never claims tests
  passed.
- Read exact bytes with `archmeld_source`; a context snippet is not the body.
- ARCHMELD is read-only: it does not edit, apply, run tests, or approve.
- If the tools are unavailable, say so explicitly; do not imply ARCHMELD was
  consulted.
<!-- ARCHMELD:END -->
