#!/usr/bin/env python3
"""Verify toolchains.lock.json against the current machine.

Resolution order for a tool: portable `path_env` + `relative_path`, the
`ARCHMELD_TOOLS_ROOT` fallback, `strategy=which`/`rustup_which`, then the
recorded `path` (observed, not a contract). It never installs anything.

Usage: python tooling/doctor.py [--json]
Exit code 0 only when every recorded tool resolves and hash-matches.
"""
from __future__ import annotations

import json
import os
import shutil
import subprocess
import sys
from pathlib import Path

import common
import portable_env

ROOT = Path(__file__).resolve().parents[1]
LOCK = ROOT / "toolchains.lock.json"
sha256_file = common.sha256_file


def resolve_tool(tool: dict) -> tuple[Path | None, str]:
    relative = tool.get("relative_path")
    path_env = tool.get("path_env")
    if relative:
        if path_env and os.environ.get(path_env):
            candidate = Path(os.environ[path_env]) / relative
            if candidate.exists():
                return candidate, f"env:{path_env}"
        root = portable_env.tools_root()
        if root is not None:
            candidate = root / relative
            if candidate.exists():
                return candidate, "tools_root"
    strategy = tool.get("strategy")
    if strategy == "which":
        name = tool.get("which_name") or (Path(relative).name if relative else tool.get("name"))
        found = shutil.which(str(name))
        if found:
            return Path(found), "which"
    if strategy == "rustup_which":
        rustup = shutil.which("rustup")
        args = tool.get("strategy_args") or {}
        toolchain = args.get("toolchain")
        name = args.get("tool") or tool.get("name")
        if rustup and toolchain and name:
            result = subprocess.run(
                [rustup, "which", "--toolchain", str(toolchain), str(name)],
                capture_output=True,
                text=True,
                timeout=30,
            )
            if result.returncode == 0:
                candidate = Path(result.stdout.strip())
                if candidate.exists():
                    return candidate, "rustup_which"
    recorded = tool.get("path")
    if recorded and Path(recorded).exists():
        return Path(recorded), "recorded"
    return None, "missing"


def helper_check(path: Path) -> dict:
    """Launch the packaged helper, not just hash it.

    `--schema` is executed with the current PATH, so a missing MinGW runtime
    DLL fails here before a user reaches `archmeld index`.
    """
    if not path.is_file():
        return {"helper": str(path), "ok": False, "reason": "helper is missing"}
    try:
        schema = subprocess.run(
            [str(path), "--schema"],
            capture_output=True,
            text=True,
            timeout=30,
        )
        version = subprocess.run(
            [str(path), "--version"],
            capture_output=True,
            text=True,
            timeout=30,
        )
    except Exception as exc:  # noqa: BLE001 - report the launch failure
        return {"helper": str(path), "ok": False, "reason": f"launch failed: {exc!r}"}
    schema_text = (schema.stdout or "").strip()
    version_text = (version.stdout or version.stderr or "").strip()
    diagnostics = []
    if schema.returncode != 0:
        diagnostics.append(
            f"--schema exited {schema.returncode} (0x{schema.returncode & 0xFFFFFFFF:08X})"
            if schema.returncode < 0
            else f"--schema exited {schema.returncode}"
        )
    if schema_text != "1.2":
        diagnostics.append(f"schema is {schema_text or '<empty>'}, expected 1.2")
    if version.returncode != 0:
        diagnostics.append(f"--version exited {version.returncode}")
    return {
        "helper": str(path),
        "sha256": sha256_file(path),
        "schema": schema_text,
        "version": version_text,
        "ok": not diagnostics,
        "reason": "schema 1.2 and version launch succeeded" if not diagnostics else "; ".join(diagnostics),
    }


def helper_paths(argv: list[str]) -> list[Path]:
    paths: list[Path] = []
    for index, arg in enumerate(argv):
        if arg == "--helper" and index + 1 < len(argv):
            paths.append(Path(argv[index + 1]))
        elif arg.startswith("--helper="):
            paths.append(Path(arg.split("=", 1)[1]))
    return paths


def main() -> int:
    as_json = "--json" in sys.argv
    helpers = helper_paths(sys.argv[1:])
    if not LOCK.exists():
        print("FAIL toolchains.lock.json is missing; run tooling/lock_toolchains.py")
        return 1
    lock = common.read_json(LOCK)
    checks = []
    for tool in lock.get("tools", []):
        name = tool.get("name")
        if tool.get("optional") and tool.get("status") == "missing_optional":
            checks.append({"tool": name, "ok": True, "reason": "optional tool missing"})
            continue
        if tool.get("status") != "present":
            checks.append({"tool": name, "ok": False, "reason": "recorded as missing"})
            continue
        if tool.get("informational") and tool.get("sha256") is None:
            checks.append({"tool": name, "ok": True, "reason": "informational, verified at lock time"})
            continue
        path, source = resolve_tool(tool)
        if path is None:
            checks.append({
                "tool": name,
                "ok": False,
                "reason": "not found by path_env/relative_path, tools_root, strategy or recorded path",
                "observed_path": tool.get("path"),
            })
            continue
        if path.is_dir():
            checks.append({
                "tool": name,
                "ok": True,
                "reason": "directory resolved",
                "source": source,
                "resolved_path": str(path),
                "observed_path": tool.get("path"),
            })
            continue
        recorded = tool.get("sha256")
        if recorded is None:
            checks.append({"tool": name, "ok": False, "reason": "no recorded hash", "source": source})
            continue
        actual = sha256_file(path)
        checks.append({
            "tool": name,
            "ok": actual == recorded,
            "reason": "hash match" if actual == recorded else "hash mismatch",
            "source": source,
            "resolved_path": str(path),
            "observed_path": tool.get("path"),
        })
    helpers_checked = [helper_check(path) for path in helpers]
    failed = [check for check in checks if not check["ok"]]
    failed.extend(check for check in helpers_checked if not check["ok"])
    if as_json:
        print(json.dumps({
            "checks": checks,
            "helpers": helpers_checked,
            "failed": len(failed),
        }, indent=2))
    else:
        for check in checks:
            print(("PASS " if check["ok"] else "FAIL ") + check["tool"] + ": " + check["reason"])
        for check in helpers_checked:
            print(
                ("PASS " if check["ok"] else "FAIL ")
                + "helper "
                + check["helper"]
                + ": "
                + check["reason"]
            )
    return 0 if not failed else 1


if __name__ == "__main__":
    raise SystemExit(main())
