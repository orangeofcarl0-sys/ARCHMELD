# ADR-047: Unix-domain socket listener with owner-only permissions

Status: accepted for the project testing phase.

## Context

B028 asks for a socket/pipe transport with a token ACL. The server only bound
loopback TCP, so the token crossed a TCP socket that any local process could
attempt to reach. On Unix, a domain socket with mode 0600 and the existing
bearer middleware gives a filesystem ACL without changing the HTTP contract.

## Decision

1. `archmeld-serve --unix-socket <path>` serves the same axum router over a
   `tokio::net::UnixListener`. It is mutually exclusive with the TCP default in
   the sense that it takes precedence; `--port` is ignored in this mode.
2. The socket is chmod 0600 immediately after bind and the mode is read back;
   any mismatch exits non-zero. An existing filesystem entry at the path is
   refused instead of replaced.
3. The announcement JSON reports `endpoint: null`, the socket path,
   `socket_mode: "0600"`, the token (as before) and workspaces.
4. The bearer-token middleware is unchanged: a request without a token or with
   a wrong token is 403, a valid token reaches the handler.
5. Windows has no equivalent path here; `--unix-socket` exits 2 with an explicit
   "supported on Unix only" message. Windows named pipes remain a separate
   implementation and are not silently emulated by TCP.

## Evidence

- Remote WSL2 (kernel 6.6.87.2, Python 3.12.3) with the Linux release binary
  SHA-256 `sha256:6bff5f10…`: socket mode `0600`, no-token and wrong-token
  requests returned 403, a valid token reached the handler (400 for the empty
  query body), and an existing path was refused with exit 1.
- Windows test: `--unix-socket` exits 2 with the explicit unsupported message.
- `docs/release-evidence/B028-unix-socket.json`, commands
  `#b028-unix-socket-linux-build`, `#b028-unix-socket-windows-test`,
  `#b028-unix-socket-remote`.
- Tests: `crates/archmeld-api/tests/unix_socket.rs`.

## Consequences

- Local clients on Unix can avoid TCP entirely; access is limited by filesystem
  permissions plus the bearer token.
- The token is still printed on stdout for the operator, as in TCP mode; the
  socket file must live in a directory the operator controls.
- Container/OS-level isolation and Windows named pipes remain open B028 items.
