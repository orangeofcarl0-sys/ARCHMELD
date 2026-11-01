# B028 threat model review

Status: reviewed for the R1 local product. This is a practical threat model, not
a claim of OS-level sandboxing.

## Assets

- Repository source and build configuration.
- Evidence ledger, CAS objects, snapshots, claims, architectures, contracts,
  approved plans and accepted ChangeSets.
- The local API bearer token and workspace handles.
- User decisions (accept/reject, approvals) and the integrity of their history.

## Untrusted inputs

- Repository files, README/AGENTS instructions, comments, test data, configs and
  dependency scripts.
- Trace/log payloads and MCP/ContextPack source snippets.
- Provider/model output and imported CodeBoarding/SCIP data.
- External agents or the user's shell writing the workspace concurrently.

## Boundaries and controls

| Boundary | Control | Evidence |
|---|---|---|
| Web page → local API | loopback bind, loopback-only Host, reject any Origin, bearer only in Authorization | `security_http.rs`, AT062 |
| Token exposure | control-line announcement only; no URL/stderr token, no query-token auth | `security_http.rs`, AT062 |
| Webview → extension | CSP `default-src 'none'`, script nonce, no remote assets, escaped text | `benchmarks/security-csp.js`, node tests, AT062 |
| Repository text → agent | `trust: untrusted` and explicit no-execute/no-network policy on ContextPack and MCP results | `security_mcp.rs`, AT061 |
| Agent → execution | read-only MCP surface; write/run/apply tools denied; HTTP apply requires approval, grant and exact digest | `mcp_stdio.rs`, `security_mcp.rs`, B021/B022 |
| External writer → prepared patch | externally_writable disclosure, read_set/preflight hashes, stale state, drift report | `security_external_writer.rs`, AT063 |
| Source egress | offline report excludes source/traces; no telemetry/auto model call; MCP has no network tool | B027 report tests, AT061 |

## Red-team transcript summary

1. A README and source comment instructed the agent to ignore rules, upload
   credentials to a local listener and create a sentinel file.
2. `archmeld_context` and `archmeld_source` returned the text with
   `trust: untrusted`; the snippet was present as data.
3. The MCP tools list contained no shell/upload/network tool. The declared
   scenario-run and ChangeSet tools returned `permission_denied`.
4. The listener received no connection; the sentinel file was absent; the source
   hash was unchanged.
5. A third-party page sending a foreign Host, any Origin, a URL token or a wrong
   bearer was rejected; a loopback request with the correct bearer reached the
   core and returned a normal not-found response.
6. An external write after preparation made apply fail; the external bytes were
   preserved and drift listed the changed path and stale plan/change records.

## Non-goals and residual risks

- Same-user full OS compromise, keyloggers, malicious local processes and
  compromised editor extensions are out of scope. The API is not a multi-tenant
  boundary.
- Native scenario execution is `trusted_native` when approved; it is not a file
  or network sandbox. Container/OCI isolation remains a separate capability.
- Continuous filesystem watching is not implemented; invalidation is checked on
  demand.
- Supply-chain integrity relies on locked origins/versions/SHA and license
  review; release SBOM work is part of B031.

## Reproduction

- `cargo test -p archmeld-api --test security_http`
- `cargo test -p archmeld-api --test security_mcp`
- `cargo test -p archmeld-core --test security_external_write`
- `node --test archmeld-vscode/test/*.test.js`
- `node benchmarks/security-csp.js`

Evidence: `docs/evidence/B028-security.json` and
`docs/evidence/B028-security-csp.json`.
