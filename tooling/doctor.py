#!/usr/bin/env python3
"""Verify toolchains.lock.json against the current machine.

This is the data source for the future `archmeld doctor` command. It never
installs anything: a mismatch or missing tool is reported as a failed check.

Usage: python tooling/doctor.py [--json]
Exit code 0 only when every recorded tool is present and hash-matching.
"""
from __future__ import annotations

import hashlib
import json
import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]
LOCK = ROOT / "toolchains.lock.json"


def sha256_file(path: Path) -> str:
    digest = hashlib.sha256()
    with path.open("rb") as handle:
        for chunk in iter(lambda: handle.read(1 << 20), b""):
            digest.update(chunk)
    return digest.hexdigest()


def main() -> int:
    as_json = "--json" in sys.argv
    if not LOCK.exists():
        print("FAIL toolchains.lock.json is missing; run tooling/lock_toolchains.py")
        return 1
    lock = json.loads(LOCK.read_text(encoding="utf-8"))
    checks = []
    for tool in lock.get("tools", []):
        name = tool.get("name")
        if tool.get("status") != "present":
            checks.append({"tool": name, "ok": False, "reason": "recorded as missing"})
            continue
        if tool.get("informational") and tool.get("sha256") is None:
            checks.append({"tool": name, "ok": True, "reason": "informational, verified at lock time"})
            continue
        path = Path(tool["path"])
        if not path.exists():
            checks.append({"tool": name, "ok": False, "reason": f"not found: {path}"})
            continue
        recorded = tool.get("sha256")
        if recorded is None:
            checks.append({"tool": name, "ok": False, "reason": "no recorded hash"})
            continue
        actual = sha256_file(path)
        checks.append(
            {
                "tool": name,
                "ok": actual == recorded,
                "reason": "hash match" if actual == recorded else "hash mismatch",
                "path": str(path),
            }
        )
    failed = [check for check in checks if not check["ok"]]
    if as_json:
        print(json.dumps({"checks": checks, "failed": len(failed)}, indent=2))
    else:
        for check in checks:
            print(("PASS " if check["ok"] else "FAIL ") + check["tool"] + ": " + check["reason"])
    return 0 if not failed else 1


if __name__ == "__main__":
    raise SystemExit(main())
