# ADR-048: Windows named-pipe listener with the bearer token

Status: accepted for the project testing phase.

## Context

B028 asks for a named-pipe/Unix-socket transport with a token ACL. The Unix
socket is covered by ADR-047; Windows clients still had only loopback TCP, which
any local process can attempt to reach.

## Decision

1. `archmeld-serve --named-pipe \.\pipe\<name>` serves the same axum router over
   a `tokio::net::windows::named_pipe` server. Every instance, including the
   first, is created with `CreateNamedPipeW` and a protected DACL
   (`D:P(A;;GA;;;SY)(A;;GA;;;BA)(A;;GA;;;<current-user-SID>)`) that grants full
   access only to SYSTEM, the Administrators group and the current user. The
   first instance also sets `FILE_FLAG_FIRST_PIPE_INSTANCE`, and each accepted
   connection is followed by a new instance with the same descriptor.
2. The name must start with `\.\pipe\`; anything else exits 2 instead of
   creating a file-like path. The pipe sets `PIPE_REJECT_REMOTE_CLIENTS`.
3. The unchanged bearer-token middleware still guards every request. The
   announcement records `endpoint: null`, `named_pipe`, the token and the ACL
   object: type `custom_dacl`, `protected`, no `everyone`/`anonymous` ACE, the
   current user present, the ACE count and a digest of the SDDL read back from
   the live pipe handle with `GetSecurityInfo` and
   `ConvertSecurityDescriptorToStringSecurityDescriptorW`.
4. On non-Windows platforms `--named-pipe` exits 2 with an explicit message.

## Evidence

- Real Windows test: announcement has `endpoint: null`, the pipe name and an
  `acl` object with `type=custom_dacl`, `verified=true`, `protected=true`,
  `everyone=false`, `anonymous=false`, `current_user=true` and three ACEs; a
  request without a token and a request with a wrong token return 403; a valid
  token reaches the handler (400 for the empty query body).
- A malformed name exits 2 with "must start with".
- `docs/release-evidence/B028-named-pipe.json`, test
  `crates/archmeld-api/tests/named_pipe.rs`, command
  `#b028-named-pipe-tests`.

## Consequences

- Windows local clients can avoid TCP; access is limited by an explicit
  protected DACL (current user, SYSTEM, Administrators) plus the bearer token.
  Other local users and `Everyone` are not granted access by the descriptor.
- The DACL is read back from the created handle and reported; the test asserts
  the readback rather than trusting the requested SDDL string.
- Container/OS-level isolation is covered for verification checks (ADR-050) and
  scenario runs (ADR-051); process-level OS sandboxing beyond Docker remains
  outside this decision.
