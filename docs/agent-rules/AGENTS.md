<!-- ARCHMELD:BEGIN -->
## ARCHMELD: local, read-only code evidence

ARCHMELD is available as MCP tools. Use it before multi-file grep when the task
needs call relationships, snapshot-bound changes, or evidence instead of guesses.

| User intent | First call | Then |
|---|---|---|
| "What changed / is this safe?" | `archmeld_brief` with `recipe=change` | `archmeld_review`, `archmeld_source` |
| "Summarize this project" | `archmeld_brief` with `recipe=project` | `archmeld_context`, `archmeld_query` |
| "Who calls this / what breaks?" | `archmeld_query` with `kind=impact` | `archmeld_source`, `archmeld_context` |
| "Where is this symbol?" | `archmeld_query` with `kind=symbols` | `archmeld_source` |
| "What does this code actually do?" | `archmeld_source` | `archmeld_query` for neighbors |

Rules:
- Call `archmeld_status` first when the snapshot or working-tree state is unknown.
- `verification=not_run` means the review is static; ARCHMELD never claims tests passed.
- `candidate` relations are syntax evidence, not resolved calls. Never present them as confirmed.
- `coverage=partial` or `unknown` means unknown, not absent.
- Read exact bytes with `archmeld_source`; a context snippet is not the full body.
- ARCHMELD is read-only: it does not edit, apply, run tests, or approve changes.
<!-- ARCHMELD:END -->
