# ADR-056: Zero-concept agent onboarding and one-screen briefs

Status: accepted for the M1 onboarding slice (2026-11-11).

## Context

ARCHMELD's evidence model was usable through the CLI, HTTP and MCP, but a real
first run had three avoidable failures:

1. `README.md` led with the pinned upstream CodeGraph quick start
   (`codegraph-server --mcp`), not the ARCHMELD MCP entry.
2. `archmeld index` called `ensure_project` but never wrote `project_roots`.
   MCP `archmeld_status` therefore reported `freshness=unknown`, and
   `archmeld_review` failed with "no active workspace root", even though the
   README documented `index` followed by `archmeld-mcp`.
3. The MCP surface required nine fields for a query, had no defaults, returned
   raw JSON without a summary, and had no next-action hints. There was no
   ARCHMELD agent-rule file; README linked upstream `codegraph_*` rules.

The same-agent experiment on a small fixture produced 17/17 localization fields
in both arms, so onboarding must sell a workflow, not an accuracy claim.

## Decision

1. `archmeld index` and `archmeld init` register the workspace root for the
   existing project through a root-only helper. They do not create a session or
   change the project id. `archmeld status --workspace` and MCP `status`/`review`
   then work on a real first run.
2. `archmeld init` is the primary entry point. It preflights the workspace,
   indexes it, registers the root, previews `AGENTS.md`/`CLAUDE.md` rule blocks,
   renders the Claude Code and Codex MCP commands, and runs a real
   `initialize → tools/list → status` MCP handshake. Source files are not
   written unless `--write` is explicit.
3. `archmeld mcp install` renders a canonical server descriptor and the exact
   client command. It prints by default; `--write` executes the client command;
   `--check` performs the handshake. The server does not rewrite global client
   configuration itself.
4. A new read-only MCP tool, `archmeld_brief`, composes existing `status` and
   `review` facts into a one-screen brief: `level`, `summary`, `scope`,
   `counts` (confirmed/candidate/unknown/stale), `findings`, `unknowns`,
   `next_actions` and `artifacts`. Recipes are `auto`, `project` and `change`;
   `auto` uses only deterministic signals (missing snapshot, multiple projects,
   dirty workspace).
5. MCP defaults live in the bridge, not the core: project and latest snapshot
   come from the bridge; `query` defaults to symbols with a bounded budget;
   `context` defaults to 4,000 tokens; `source` defaults to anchor 0 and 200 KB.
   Core request structs stay strict.
6. Every implemented MCP result gains an `agent_summary` and `next_actions`
   when the tool did not already provide them. Errors carry a `next_action`
   string. The implemented surface is eight read-only tools, within S11's
   14-tool cap.
7. ARCHMELD ships its own agent rules under `docs/agent-rules/`; the README
   labels the upstream CodeGraph rules as inherited and not applicable to
   `archmeld_*` tools.

## Consequences

- A first run is: install → `archmeld init` → ask the agent. The user does not
  need to choose a ledger path, remember a project id, or write MCP JSON.
- `archmeld_brief` remains a composition of existing facts. It adds no evidence,
  no inferred calls, no architecture synthesis and no test execution. Candidate
  relations stay candidate and `verification` stays `not_run`.
- The brief's severity mapping is deterministic: incoming static relation =
  high; added entity = medium; deleted = low; otherwise info. It is not a
  readiness score.
- `--write` is the only path that changes `AGENTS.md`/`CLAUDE.md` or executes a
  client `mcp add` command; the default output is a plan.
- M1 does not implement commit supervision or scoped multi-agent authority.
  Those remain M3/M4 and require their own ADRs.

## Evidence

- Unit/integration: `crates/archmeld-core/tests/brief.rs`,
  `crates/archmeld-api/tests/onboard.rs`, `crates/archmeld-api/tests/mcp_stdio.rs`.
- Release handshake: `tooling/test_release.py` runs `archmeld init` against the
  installed bundle and asserts `health.state=ok`, eight tools and clean
  freshness.
- Manual first run: `docs/release-evidence/` record for M1 (command, cwd,
  argv, exit code, artifact hashes).
