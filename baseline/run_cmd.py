#!/usr/bin/env python3
"""Run a command and append a machine-checkable record to baseline/commands.ndjson.

Every record captures cwd, argv, exit code, timing, tool versions, git HEAD,
and SHA-256 of the captured stdout/stderr logs. Records are append-only: a
re-run adds a new record instead of rewriting history.
"""
from __future__ import annotations

import argparse
import hashlib
import json
import os
import platform
import re
import subprocess
import sys
from datetime import datetime, timezone
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]
LOG_DIR = ROOT / "baseline" / "logs"
RECORD_FILE = ROOT / "baseline" / "commands.ndjson"


def sha256_file(path: Path) -> str | None:
    if not path.exists():
        return None
    h = hashlib.sha256()
    with path.open("rb") as f:
        for chunk in iter(lambda: f.read(1 << 20), b""):
            h.update(chunk)
    return h.hexdigest()


def tool_version(argv: list[str]) -> dict:
    try:
        out = subprocess.run(
            argv, capture_output=True, text=True, timeout=30, encoding="utf-8", errors="replace"
        )
        text = (out.stdout or out.stderr).strip().splitlines()
        return {"argv": argv, "exit": out.returncode, "first_line": text[0] if text else ""}
    except Exception as exc:  # noqa: BLE001 - record failures instead of hiding them
        return {"argv": argv, "error": repr(exc)}


def git_state(cwd: Path) -> dict:
    def run(*args: str) -> str:
        try:
            return subprocess.run(
                ["git", *args], cwd=cwd, capture_output=True, text=True, timeout=30
            ).stdout.strip()
        except Exception as exc:  # noqa: BLE001
            return f"<error: {exc!r}>"

    return {
        "head": run("rev-parse", "HEAD"),
        "branch": run("rev-parse", "--abbrev-ref", "HEAD"),
        "status_porcelain": run("status", "--porcelain").splitlines()[:50],
    }


def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("--id", required=True, help="stable record id, e.g. b000-test-codegraph")
    parser.add_argument("--cwd", default=".")
    parser.add_argument("--env", action="append", default=[], help="KEY=VALUE passed to child")
    parser.add_argument("--note", default="")
    parser.add_argument("argv", nargs=argparse.REMAINDER)
    args = parser.parse_args()

    argv = args.argv
    if argv and argv[0] == "--":
        argv = argv[1:]
    if not argv:
        parser.error("no command given")

    cwd = (ROOT / args.cwd).resolve()
    env = dict(os.environ)
    for item in args.env:
        key, _, value = item.partition("=")
        env[key] = value
    env.setdefault("PYTHONIOENCODING", "utf-8")
    # Keep compiler scratch files off the small system drive.
    default_temp = ROOT.parent / "_archmeld_temp"
    if default_temp.is_dir():
        for key in ("TMP", "TEMP", "TMPDIR"):
            env.setdefault(key, str(default_temp))

    LOG_DIR.mkdir(parents=True, exist_ok=True)
    slug = re.sub(r"[^A-Za-z0-9_.-]+", "-", args.id)
    stdout_path = LOG_DIR / f"{slug}.stdout.log"
    stderr_path = LOG_DIR / f"{slug}.stderr.log"

    before = git_state(cwd)
    started = datetime.now(timezone.utc)
    with stdout_path.open("w", encoding="utf-8", errors="replace") as out, stderr_path.open(
        "w", encoding="utf-8", errors="replace"
    ) as err:
        proc = subprocess.run(argv, cwd=cwd, env=env, stdout=out, stderr=err)
    finished = datetime.now(timezone.utc)
    after = git_state(cwd)

    record = {
        "id": args.id,
        "note": args.note,
        "cwd": str(cwd),
        "argv": argv,
        "exit_code": proc.returncode,
        "started_at": started.isoformat(),
        "finished_at": finished.isoformat(),
        "duration_s": round((finished - started).total_seconds(), 3),
        "platform": {
            "system": platform.system(),
            "release": platform.release(),
            "machine": platform.machine(),
            "python": sys.version.split()[0],
        },
        "tools": {
            "rustc": tool_version(["rustc", "-vV"]),
            "cargo": tool_version(["cargo", "--version"]),
            "git": tool_version(["git", "--version"]),
            "node": tool_version(["node", "--version"]),
            "npm": tool_version(["npm", "--version"]),
        },
        "git_before": before,
        "git_after": after,
        "logs": {
            "stdout": str(stdout_path.relative_to(ROOT)),
            "stdout_sha256": sha256_file(stdout_path),
            "stderr": str(stderr_path.relative_to(ROOT)),
            "stderr_sha256": sha256_file(stderr_path),
        },
    }
    with RECORD_FILE.open("a", encoding="utf-8") as f:
        f.write(json.dumps(record, ensure_ascii=False) + "\n")

    print(json.dumps({k: record[k] for k in ("id", "exit_code", "duration_s")}, ensure_ascii=False))
    return proc.returncode


if __name__ == "__main__":
    raise SystemExit(main())
