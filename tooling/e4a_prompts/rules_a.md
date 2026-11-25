ARCHMELD is available as read-only MCP tools. Use it when you need call
relationships, snapshot-bound changes, or evidence instead of guesses.

- `candidate` relations are syntax evidence, not resolved calls. Never present
  them as confirmed.
- `coverage=partial` or `unknown` means unknown, not absent.
- `verification=not_run` means static review; ARCHMELD never claims tests
  passed.
- Read exact bytes with `archmeld_source`; a context snippet is not the body.
- ARCHMELD is read-only: it does not edit, apply, run tests, or approve.
