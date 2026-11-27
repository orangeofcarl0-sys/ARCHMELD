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
import shutil
import subprocess
import sys
from datetime import datetime, timezone
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]
LOG_DIR = ROOT / "baseline" / "logs"
RECORD_FILE = ROOT / "baseline" / "commands.ndjson"

sys.path.insert(0, str(ROOT / "tooling"))
import build_helper  # noqa: E402
import portable_env as portable  # noqa: E402


def sha256_file(path: Path) -> str | None:
    if not path.exists():
        return None
    h = hashlib.sha256()
    with path.open("rb") as f:
        for chunk in iter(lambda: f.read(1 << 20), b""):
            h.update(chunk)
    return h.hexdigest()


def tool_version(argv: list[str], env: dict[str, str] | None = None) -> dict:
    try:
        out = subprocess.run(
            argv,
            capture_output=True,
            text=True,
            timeout=30,
            encoding="utf-8",
            errors="replace",
            env=env,
        )
        text = (out.stdout or out.stderr).strip().splitlines()
        return {"argv": argv, "exit": out.returncode, "first_line": text[0] if text else ""}
    except Exception as exc:  # noqa: BLE001 - record failures instead of hiding them
        return {"argv": argv, "error": repr(exc)}


def allowlisted_environment(env: dict[str, str]) -> dict:
    """Record resolved, replayable inputs without dumping the whole environment."""
    keys = (
        "RUSTUP_TOOLCHAIN",
        "ARCHMELD_RUST_TOOLCHAIN",
        "ARCHMELD_TOOLS_ROOT",
        "ARCHMELD_TEMP",
        "ARCHMELD_MINGW_BIN",
        "ARCHMELD_STRAWBERRY_BIN",
        "ARCHMELD_GCC_ROOT",
        "ARCHMELD_LIBCLANG_PATH",
        "ARCHMELD_REQUIRE_TOOLS",
        "ARCHMELD_WRITE_EVIDENCE",
        "ARCHMELD_GCC",
        "ARCHMELD_GXX",
        "ARCHMELD_PYTHON",
        "ARCHMELD_NODE",
        "ARCHMELD_TSC",
        "ARCHMELD_JAVAC",
        "ARCHMELD_JAVA",
        "ARCHMELD_DOTNET",
        "ARCHMELD_DOTNET_TRACE",
        "ARCHMELD_CLANGXX",
        "ARCHMELD_CLANGD",
        "ARCHMELD_PYRIGHT",
        "ARCHMELD_TS_SERVER",
        "ARCHMELD_GO",
        "LIBCLANG_PATH",
        "CXXFLAGS",
        "CXXFLAGS_x86_64_pc_windows_gnu",
        "CARGO_INCREMENTAL",
        "TMP",
        "TEMP",
        "RUSTC_WRAPPER",
        "SCCACHE_DIR",
        "ROCKSDB_LIB_DIR",
        "ROCKSDB_STATIC",
        "RUSTFLAGS",
        "ARCHMELD_CXXFLAGS",
        "ARCHMELD_BINDGEN_ARGS",
        "ARCHMELD_RUSTFLAGS",
    )
    recorded = {key: env[key] for key in keys if env.get(key)}
    bindgen = env.get("BINDGEN_EXTRA_CLANG_ARGS")
    replayable = True
    if bindgen:
        if "-I" in bindgen:
            recorded["BINDGEN_EXTRA_CLANG_ARGS"] = bindgen
        else:
            replayable = False
    recorded["environment_mode"] = (
        "explicit"
        if any(
            key in env
            for key in ("ARCHMELD_TOOLS_ROOT", "ARCHMELD_MINGW_BIN", "ARCHMELD_TEMP")
        )
        else "resolved"
    )
    recorded["environment_replayable"] = replayable
    return recorded


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
    base_env = dict(os.environ)
    for item in args.env:
        key, _, value = item.partition("=")
        base_env[key] = value
    # Cargo/rustc runs must use the same canonical environment as
    # `cargo_test.py`; otherwise every recorded build changes the
    # librocksdb-sys fingerprint. Git/Python/other commands keep the portable
    # environment only.
    command_name = Path(argv[0]).name.lower()
    is_cargo = command_name in {"cargo", "cargo.exe", "rustc", "rustc.exe"}
    if is_cargo:
        env = build_helper.cargo_env(env=base_env)
    else:
        env = portable.subprocess_env(env=base_env)
    env.setdefault("PYTHONIOENCODING", "utf-8")

    # Windows CreateProcess searches the caller's PATH, not the child's env.
    # Resolve a bare cargo/rustc/python against the resolved child PATH so the
    # recorded command runs from a plain shell as well.
    exec_argv = list(argv)
    resolved_executable = None
    if not any(separator in exec_argv[0] for separator in ("/", "\\")):
        found = shutil.which(exec_argv[0], path=env.get("PATH", ""))
        if found:
            exec_argv[0] = found
            resolved_executable = found

    LOG_DIR.mkdir(parents=True, exist_ok=True)
    slug = re.sub(r"[^A-Za-z0-9_.-]+", "-", args.id)
    stdout_path = LOG_DIR / f"{slug}.stdout.log"
    stderr_path = LOG_DIR / f"{slug}.stderr.log"

    before = git_state(cwd)
    started = datetime.now(timezone.utc)
    with stdout_path.open("w", encoding="utf-8", errors="replace") as out, stderr_path.open(
        "w", encoding="utf-8", errors="replace"
    ) as err:
        proc = subprocess.run(exec_argv, cwd=cwd, env=env, stdout=out, stderr=err)
    finished = datetime.now(timezone.utc)
    after = git_state(cwd)

    record = {
        "id": args.id,
        "note": args.note,
        "cwd": str(cwd),
        "argv": argv,
        "resolved_executable": resolved_executable,
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
            "rustc": tool_version(["rustc", "-vV"], env),
            "cargo": tool_version(["cargo", "--version"], env),
            "git": tool_version(["git", "--version"], env),
            "node": tool_version(["node", "--version"], env),
            "npm": tool_version(["npm", "--version"], env),
        },
        "environment": allowlisted_environment(env),
        "cargo_environment": {
            "mode": "canonical" if is_cargo else "portable",
            "rocksdb_source": "source" if not env.get("ROCKSDB_LIB_DIR") else "prebuilt",
            "linker": "lld" if "fuse-ld=lld" in env.get("RUSTFLAGS", "") else "bfd",
            "rustc_wrapper": env.get("RUSTC_WRAPPER"),
            "sccache_dir": env.get("SCCACHE_DIR"),
            "target_dir": env.get("CARGO_TARGET_DIR"),
        },
        "environment_resolution": portable.describe(env=env),
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
