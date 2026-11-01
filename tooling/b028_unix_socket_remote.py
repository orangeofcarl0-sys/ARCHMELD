#!/usr/bin/env python3
"""Remote proof for the Unix-domain socket listener (stdlib only).

Checks the announcement, the 0600 socket mode, bearer-token enforcement over the
socket, and refusal to replace an existing path. Prints one JSON line.
"""

import json
import os
import socket
import stat
import subprocess
import sys
import time

serve, state, sock = sys.argv[1:4]
os.makedirs(state, exist_ok=True)
try:
    os.unlink(sock)
except FileNotFoundError:
    pass
proc = subprocess.Popen(
    [serve, "--state", state, "--unix-socket", sock, "--token", "test-token"],
    stdout=subprocess.PIPE,
    stderr=subprocess.PIPE,
    text=True,
)
line = proc.stdout.readline()
if not line:
    print(json.dumps({"ok": False, "error": "no announcement", "stderr": proc.stderr.read()}))
    sys.exit(1)
announce = json.loads(line)
mode = stat.S_IMODE(os.stat(sock).st_mode)


def request(auth=None, body=b"{}"):
    client = socket.socket(socket.AF_UNIX, socket.SOCK_STREAM)
    client.connect(sock)
    headers = ["POST /v1/query HTTP/1.1", "Host: localhost", "Connection: close"]
    if auth:
        headers.append(f"Authorization: Bearer {auth}")
    if body is not None:
        headers.append("Content-Type: application/json")
        headers.append(f"Content-Length: {len(body)}")
    client.sendall(("\r\n".join(headers) + "\r\n\r\n").encode() + body)
    data = b""
    while True:
        chunk = client.recv(65536)
        if not chunk:
            break
        data += chunk
    client.close()
    return data.decode(errors="replace")


no_auth = request(None)
wrong = request("wrong-token")
allowed = request("test-token")
proc.kill()
proc.wait()
try:
    os.unlink(sock)
except FileNotFoundError:
    pass
with open(sock, "w", encoding="utf-8") as handle:
    handle.write("stale")
stale = subprocess.run(
    [serve, "--state", state, "--unix-socket", sock, "--no-auth"],
    capture_output=True,
    text=True,
    timeout=30,
)
try:
    os.unlink(sock)
except FileNotFoundError:
    pass
result = {
    "ok": (
        announce.get("endpoint") is None
        and announce.get("socket_mode") == "0600"
        and announce.get("token") == "test-token"
        and mode == 0o600
        and no_auth.startswith("HTTP/1.1 403")
        and wrong.startswith("HTTP/1.1 403")
        and not allowed.startswith("HTTP/1.1 403")
        and stale.returncode == 1
        and "refusing to replace existing socket" in stale.stderr
    ),
    "announcement": announce,
    "socket_mode": oct(mode),
    "no_auth_status": no_auth.split("\r\n", 1)[0],
    "wrong_token_status": wrong.split("\r\n", 1)[0],
    "allowed_status": allowed.split("\r\n", 1)[0],
    "stale_socket_exit": stale.returncode,
    "stale_socket_stderr": stale.stderr.strip(),
}
print(json.dumps(result))
sys.exit(0 if result["ok"] else 1)
