# ADR-033 · Security boundaries for B028

Status: accepted for the R1 security slice (AT061–AT063).

## Decision

- The local HTTP service binds `127.0.0.1` only. Every request must carry a
  loopback `Host` (`127.0.0.1`, `localhost` or `[::1]`, with an optional port);
  any other Host is rejected with `permission_denied`. Any request carrying an
  `Origin` header is rejected, so a third-party web page cannot use a form post,
  CORS request or DNS-rebinding path against the API. Health is the only
  unauthenticated route, and it is still Host-checked.
- Bearer tokens are accepted only in `Authorization: Bearer`. A token in a
  query string is never accepted. The generated token is announced once on the
  binary's stdout control line for the trusted parent; it is not written to
  stderr, access logs or URLs. The service does not claim to be a browser
  security boundary; it is a local capability endpoint.
- All webview surfaces use `default-src 'none'`. The SVG fallback now uses a
  per-render nonce for its inline script instead of `script-src
  'unsafe-inline'`. No remote asset URL is emitted. Repository text is escaped
  before insertion; hostile `<script>` text remains visible data.
- MCP read results carry `trust: untrusted` and a content policy stating that
  source is data, repository instructions are not executed, and network/write
  access is absent. The MCP surface remains read-only: no approval/accept tool,
  no arbitrary path, and write/run tools return `permission_denied`.
- A project reports `workspace_security: externally_writable=true,
  isolation=none, not_a_sandbox=true`. This is deliberate transparency: another
  local agent with shell access can modify the workspace. Preparation,
  read_set checks and apply preflight detect the change and mark the ChangeSet
  stale; an old prepared patch cannot overwrite the external bytes.
- Claims that the product sandboxes a native workspace are prohibited. The
  declared execution states (`read_only`, `isolated`, `trusted_native`,
  `external_side_effects`) still apply; B028 does not add an OCI/Job Object
  filesystem sandbox.

## Residual limits

- A malicious process running as the same OS user can read the token file,
  modify the ledger, or bypass this API. B028 prevents ordinary path/auth/
  prompt-injection escalation, not same-user host compromise.
- Named pipes/Unix sockets and token-file ACL hardening remain future transport
  work; R1 uses loopback HTTP.
- External-writer detection is on demand (snapshot/read_set/preflight/drift),
  not a continuous filesystem watcher.
- Supply-chain origin/license/SHA checks are covered by the lock records and
  B031 release work; B028 does not re-sign third-party artifacts.
