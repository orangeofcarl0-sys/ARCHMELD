# ARCHMELD agent rules

Short, read-only instructions that teach a coding agent when to call the
`archmeld_*` MCP tools instead of falling back to multi-file grep.

- `AGENTS.md` — Codex / AGENTS.md-compatible agents.
- `claude.md` — Claude Code project memory.

`archmeld init` previews the block and `archmeld init --write` installs it into
the repository as `AGENTS.md` / `CLAUDE.md` using the
`<!-- ARCHMELD:BEGIN -->` markers. The detailed tool schemas live in
`docs/archmeld/contracts/mcp-tools.json`; these files stay short on purpose so
they do not crowd out the agent's working context.
