#!/usr/bin/env python3
# Copyright 2026 ARCHMELD contributors
# SPDX-License-Identifier: Apache-2.0
"""Small, dependency-free primitives shared by maintained tooling scripts.

This module deliberately owns no environment, Cargo or evidence-format logic:
`portable_env` resolves tools, `build_helper.cargo_env` builds the canonical
environment, and `baseline/run_cmd.py` records evidence. Keeping this module
stdlib-only means a pre-push hook can import it without side effects.
"""
from __future__ import annotations

import hashlib
import json
import subprocess
import time
from pathlib import Path
from typing import Any, Sequence

ROOT = Path(__file__).resolve().parents[1]


def sha256_bytes(data: bytes, *, prefix: str = "") -> str:
    return prefix + hashlib.sha256(data).hexdigest()


def sha256_text(text: str, *, prefix: str = "") -> str:
    return sha256_bytes(text.encode("utf-8"), prefix=prefix)


def sha256_file(
    path: Path, *, prefix: str = "", missing_ok: bool = False
) -> str | None:
    """Hash a file with a single streaming pass.

    `missing_ok=True` is for evidence recorders that represent a missing log as
    `None`; every other caller should fail loudly.
    """
    if not path.exists():
        if missing_ok:
            return None
        raise FileNotFoundError(path)
    with path.open("rb") as handle:
        digest = hashlib.file_digest(handle, "sha256").hexdigest()
    return prefix + digest


def read_json(path: Path) -> Any:
    return json.loads(path.read_text(encoding="utf-8"))


def write_json(path: Path, value: Any, *, mkdir: bool = True) -> None:
    if mkdir:
        path.parent.mkdir(parents=True, exist_ok=True)
    path.write_text(
        json.dumps(value, ensure_ascii=False, indent=2) + "\n", encoding="utf-8"
    )


def run_capture(
    argv: Sequence[str],
    *,
    cwd: Path | None = None,
    env: dict[str, str] | None = None,
    timeout: float | None = None,
    check: bool = False,
    text: bool = True,
    strip: bool = False,
    tail: int | None = None,
    stdin_text: str | None = None,
) -> dict[str, Any]:
    """Run a command and return a record instead of raising on non-zero exit.

    Every behavioral switch is explicit so callers can preserve their previous
    semantics: text vs bytes, stripping, tail truncation, timeout and check.
    """
    command = [str(item) for item in argv]
    started = time.monotonic()
    timed_out = False
    try:
        completed = subprocess.run(
            command,
            cwd=str(cwd) if cwd is not None else None,
            env=env,
            input=stdin_text,
            capture_output=True,
            text=text,
            timeout=timeout,
            check=False,
        )
        exit_code: int | None = completed.returncode
        stdout = completed.stdout
        stderr = completed.stderr
    except subprocess.TimeoutExpired as error:
        timed_out = True
        exit_code = None
        stdout = error.stdout or ("" if text else b"")
        stderr = error.stderr or ("" if text else b"")
    except FileNotFoundError as error:
        exit_code = 127
        stdout = "" if text else b""
        stderr = str(error)
    except OSError as error:
        exit_code = 126
        stdout = "" if text else b""
        stderr = str(error)
    duration_s = round(time.monotonic() - started, 3)

    if tail is not None:
        stdout = stdout[-tail:]
        stderr = stderr[-tail:]
    if strip:
        stdout = stdout.strip()
        stderr = stderr.strip()

    if check and exit_code != 0:
        raise subprocess.CalledProcessError(
            exit_code if exit_code is not None else -1,
            command,
            output=stdout,
            stderr=stderr,
        )

    return {
        "argv": command,
        "exit": exit_code,
        "stdout": stdout,
        "stderr": stderr,
        "timed_out": timed_out,
        "duration_s": duration_s,
    }


def run_checked(*argv: str, cwd: Path = ROOT) -> str:
    result = run_capture(list(argv), cwd=cwd, check=True, text=True, strip=True)
    return result["stdout"]
