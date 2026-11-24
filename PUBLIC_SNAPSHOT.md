# Public snapshot

This repository is a sanitized public snapshot of ARCHMELD, exported from a
private development repository on 2026-11-24. It is a single snapshot commit,
not the private development history.

This snapshot includes the M1 agent-onboarding slice:

- `archmeld init` / `archmeld mcp install` / `archmeld brief`;
- workspace-root registration so MCP `status` and `review` work on a first run;
- the read-only `archmeld_brief` MCP tool (8 implemented, read-only tools);
- ARCHMELD agent rules and ADR-056;
- the 2026-11-03 same-agent comparison evidence and the M1 first-run evidence.

Redactions applied to this snapshot:

- authorized remote workstation host and user names;
- local user paths, workspace paths and the local machine hostname;
- the identity and aggregate values of the private recorded dataset used for
  B029 evidence. The dataset itself and its derived tables are not included.

Local paths in the retained evidence are replaced with `<repo>`, `<workspace>`
and `<user>` placeholders.

Other intentional differences:

- transient C# fixture build outputs under
  `adapters/languages/fixtures/csharp/{bin,obj}` were omitted; run the fixture
  build to regenerate them.
- artifact hashes in `baseline/commands.ndjson` refer to the private originals
  before redaction. The public evidence files differ from those originals only
  by the redactions listed above.

No credentials, bearer tokens or private keys are included. The secret-like
strings under `crates/codegraph-harness/fixtures/` are deliberately fake
examples used by the secret-detection tests.

The upstream CodeGraph license, NOTICE and provenance files are retained.
