#!/usr/bin/env python3
# Copyright 2026 ARCHMELD contributors
# SPDX-License-Identifier: Apache-2.0
"""Portable environment resolution for ARCHMELD development tools.

No absolute machine path is encoded here. Resolution order is:

    explicit argument -> ARCHMELD_* environment variable -> repository-sibling
    directory (only if it exists) -> standard environment variable -> system
    default

The repository-sibling `_archmeld_temp` is preferred over a system-wide TMP so
long GCC/linker response files stay on the worktree drive; BFD ld on Windows
rejects drive-absolute `@...` response-file paths.

Tests use the same variables through `crates/archmeld-core/tests/support`.
"""

from __future__ import annotations

import argparse
import json
import os
import shutil
import subprocess
import sys
import tempfile
from dataclasses import dataclass, asdict
from pathlib import Path
from typing import Mapping, Sequence

ROOT = Path(__file__).resolve().parents[1]

ENV_TEMP = "ARCHMELD_TEMP"
ENV_TOOLS = "ARCHMELD_TOOLS_ROOT"
ENV_STATE = "ARCHMELD_STATE_ROOT"
ENV_REQUIRE_TOOLS = "ARCHMELD_REQUIRE_TOOLS"
ENV_STRAWBERRY = "ARCHMELD_STRAWBERRY_BIN"
ENV_MINGW = "ARCHMELD_MINGW_BIN"
ENV_LIBCLANG = "ARCHMELD_LIBCLANG_PATH"
ENV_GCC_ROOT = "ARCHMELD_GCC_ROOT"
ENV_PYTHON = "ARCHMELD_PYTHON"
DEFAULT_RUST_TOOLCHAIN = "1.94.1-x86_64-pc-windows-gnu"


def _env(env: Mapping[str, str] | None) -> Mapping[str, str]:
    return os.environ if env is None else env


def _path_from(value: str | Path | None) -> Path | None:
    if value is None:
        return None
    text = str(value).strip()
    return Path(text) if text else None


def temp_root(explicit: str | Path | None = None, env: Mapping[str, str] | None = None) -> Path:
    env = _env(env)
    for candidate in (explicit, env.get(ENV_TEMP)):
        path = _path_from(candidate)
        if path is not None:
            return path
    sibling = ROOT.parent / "_archmeld_temp"
    if sibling.is_dir():
        return sibling
    for candidate in (env.get("TMP"), env.get("TEMP"), env.get("TMPDIR")):
        path = _path_from(candidate)
        if path is not None:
            return path
    return Path(tempfile.gettempdir())


def assert_safe_temp_root(path: Path) -> None:
    resolved = path.resolve()
    if resolved == ROOT.resolve():
        raise ValueError(f"refusing to use the repository root as a temp root: {resolved}")
    if resolved.parent == resolved:
        raise ValueError(f"refusing to use a filesystem root as a temp root: {resolved}")


def temp_dir(prefix: str, root: str | Path | None = None, env: Mapping[str, str] | None = None) -> Path:
    base = Path(root) if root is not None else temp_root(env=env)
    assert_safe_temp_root(base)
    base.mkdir(parents=True, exist_ok=True)
    return Path(tempfile.mkdtemp(prefix=prefix, dir=str(base)))


def tools_root(
    explicit: str | Path | None = None,
    env: Mapping[str, str] | None = None,
) -> Path | None:
    env = _env(env)
    for candidate in (explicit, env.get(ENV_TOOLS)):
        path = _path_from(candidate)
        if path is not None:
            return path
    sibling = ROOT.parent / "_archmeld_tools"
    if sibling.is_dir():
        return sibling
    return None


def state_root(
    explicit: str | Path | None = None,
    env: Mapping[str, str] | None = None,
) -> Path | None:
    env = _env(env)
    for candidate in (explicit, env.get(ENV_STATE)):
        path = _path_from(candidate)
        if path is not None:
            return path
    sibling = ROOT.parent / "_archmeld_state"
    if sibling.is_dir():
        return sibling
    return None


@dataclass(frozen=True)
class Resolution:
    requested: str
    path: Path | None
    source: str
    exists: bool

    def as_dict(self) -> dict:
        data = asdict(self)
        data["path"] = str(self.path) if self.path is not None else None
        return data


def which(name: str, env: Mapping[str, str] | None = None) -> Path | None:
    found = shutil.which(name, path=_env(env).get("PATH", ""))
    return Path(found) if found else None


def resolve_tool(
    name: str,
    env_var: str | None = None,
    relative: str | None = None,
    explicit: str | Path | None = None,
    env: Mapping[str, str] | None = None,
    allow_which: bool = True,
) -> Resolution:
    env = _env(env)
    for candidate, source in ((explicit, "explicit"), (env.get(env_var) if env_var else None, "env")):
        path = _path_from(candidate)
        if path is not None:
            return Resolution(name, path, source, path.exists())
    root = tools_root(env=env)
    if root is not None and relative:
        path = root / relative
        return Resolution(name, path, "tools_root", path.exists())
    if allow_which:
        path = which(name, env=env)
        if path is not None:
            return Resolution(name, path, "which", path.exists())
    return Resolution(name, None, "missing", False)


def libclang_path(env: Mapping[str, str] | None = None) -> Path | None:
    env = _env(env)
    for candidate in (env.get(ENV_LIBCLANG), env.get("LIBCLANG_PATH")):
        path = _path_from(candidate)
        if path is not None:
            return path
    root = tools_root(env=env)
    if root is not None:
        candidate = root / "pylibclang" / "clang" / "native"
        if candidate.is_dir():
            return candidate
    return None


def gcc_include_dirs(env: Mapping[str, str] | None = None) -> list[Path]:
    env = _env(env)
    roots: list[Path] = []
    for candidate in (env.get(ENV_GCC_ROOT), env.get(ENV_MINGW), env.get(ENV_STRAWBERRY)):
        path = _path_from(candidate)
        if path is not None:
            # ARCHMELD_MINGW_BIN points at the `bin` directory; its parent is
            # the UCRT64 root. Using parent.parent escaped to the MSYS2 root
            # and pulled clang64/mingw64 headers into bindgen.
            roots.append(path.parent if path.name.lower() == "bin" else path)
    dirs: set[Path] = set()
    for root in roots:
        for pattern in ("include", "lib/gcc/*/*/include", "*/include", "*-w64-mingw32/include"):
            for path in root.glob(pattern):
                if path.is_dir():
                    dirs.add(path.resolve())
    gcc = which("gcc", env=env) or which("gcc.exe", env=env)
    if gcc is not None:
        try:
            result = subprocess.run(
                [str(gcc), "-print-file-name=include"],
                capture_output=True,
                text=True,
                timeout=15,
                env=dict(env),
            )
        except (OSError, subprocess.SubprocessError):
            result = None
        if result is not None and result.returncode == 0:
            candidate = Path(result.stdout.strip())
            if candidate.is_dir():
                dirs.add(candidate.resolve())
        try:
            machine = subprocess.run(
                [str(gcc), "-dumpmachine"],
                capture_output=True,
                text=True,
                timeout=15,
                env=dict(env),
            )
            sysroot = subprocess.run(
                [str(gcc), "-print-sysroot"],
                capture_output=True,
                text=True,
                timeout=15,
                env=dict(env),
            )
        except (OSError, subprocess.SubprocessError):
            machine = sysroot = None
        if machine is not None and machine.returncode == 0:
            triple = machine.stdout.strip()
            if triple:
                for candidate in (
                    Path(gcc).resolve().parent.parent / triple / "include",
                    (
                        Path(sysroot.stdout.strip()) / triple / "include"
                        if sysroot is not None
                        and sysroot.returncode == 0
                        and sysroot.stdout.strip()
                        else None
                    ),
                ):
                    if candidate is not None and candidate.is_dir():
                        dirs.add(candidate.resolve())
    return sorted(dirs, key=str)


def subprocess_env(
    extra_path: Sequence[str | Path] = (),
    env: Mapping[str, str] | None = None,
) -> dict[str, str]:
    result = dict(_env(env))
    result.setdefault("PYTHONIOENCODING", "utf-8")
    root = temp_root(env=result)
    result["TMP"] = str(root)
    result["TEMP"] = str(root)
    result.setdefault("TMPDIR", str(root))

    path_parts = result.get("PATH", "").split(os.pathsep)
    candidates: list[str] = [str(Path(item)) for item in extra_path]
    pinned_python = _path_from(result.get(ENV_PYTHON))
    if pinned_python is not None and pinned_python.is_file():
        candidates.append(str(pinned_python.parent))
    python_bin = Path(sys.executable).resolve().parent if sys.executable else None
    if python_bin is not None and python_bin.is_dir():
        candidates.append(str(python_bin))
    cargo_bin = Path.home() / ".cargo" / "bin"
    if cargo_bin.is_dir():
        candidates.append(str(cargo_bin))
    strawberry = _path_from(result.get(ENV_MINGW)) or _path_from(result.get(ENV_STRAWBERRY))
    if os.name == "nt" and strawberry is not None and strawberry.is_dir():
        candidates.append(str(strawberry))
    for candidate in reversed(candidates):
        if candidate and candidate not in path_parts:
            path_parts.insert(0, candidate)
    result["PATH"] = os.pathsep.join(path_parts)

    if os.name == "nt" and "RUSTUP_TOOLCHAIN" not in result:
        result["RUSTUP_TOOLCHAIN"] = DEFAULT_RUST_TOOLCHAIN

    libclang = libclang_path(env=result)
    if libclang is not None and libclang.exists():
        result.setdefault("LIBCLANG_PATH", str(libclang))
    includes = gcc_include_dirs(env=result)
    if includes and "BINDGEN_EXTRA_CLANG_ARGS" not in result:
        result["BINDGEN_EXTRA_CLANG_ARGS"] = " ".join(
            f"-I{Path(item).as_posix()}" for item in includes
        )
    return result


def _first_existing(candidates: Sequence[Path]) -> Path | None:
    for candidate in candidates:
        if candidate.is_file():
            return candidate
    return None


def find_vscode_code(env: Mapping[str, str] | None = None) -> Path | None:
    env = _env(env)
    explicit = _path_from(env.get("ARCHMELD_VSCODE") or env.get("VSCODE_EXE") or env.get("CODE_EXE"))
    if explicit is not None and explicit.is_file():
        return explicit
    local = _path_from(env.get("LOCALAPPDATA"))
    program_files = _path_from(env.get("PROGRAMFILES"))
    program_files_x86 = _path_from(env.get("PROGRAMFILES(X86)"))
    candidates: list[Path] = []
    for program in (program_files, program_files_x86):
        if program is not None:
            candidates.append(program / "Microsoft VS Code" / "bin" / "code.cmd")
            candidates.append(program / "Microsoft VS Code" / "Code.exe")
    if local is not None:
        candidates.append(local / "Programs" / "Microsoft VS Code" / "bin" / "code.cmd")
        candidates.append(local / "Programs" / "Microsoft VS Code" / "Code.exe")
    found = _first_existing(candidates)
    if found is not None:
        return found
    for name in ("code", "code.cmd", "code.exe"):
        found = which(name, env=env)
        if found is not None:
            return found
    return None


def find_chrome(env: Mapping[str, str] | None = None) -> Path | None:
    env = _env(env)
    explicit = _path_from(env.get("ARCHMELD_CHROME"))
    if explicit is not None and explicit.is_file():
        return explicit
    roots = [
        _path_from(env.get("PROGRAMFILES")),
        _path_from(env.get("PROGRAMFILES(X86)")),
        _path_from(env.get("LOCALAPPDATA")),
    ]
    candidates = [
        root / "Google" / "Chrome" / "Application" / "chrome.exe"
        for root in roots
        if root is not None
    ]
    found = _first_existing(candidates)
    if found is not None:
        return found
    for name in ("chrome", "chrome.exe", "google-chrome", "chromium"):
        found = which(name, env=env)
        if found is not None:
            return found
    return None


def describe(env: Mapping[str, str] | None = None) -> dict:
    env = _env(env)
    tools = tools_root(env=env)
    libclang = libclang_path(env=env)
    return {
        "repo_root": str(ROOT),
        "temp_root": str(temp_root(env=env)),
        "tools_root": str(tools) if tools is not None else None,
        "state_root": str(state_root(env=env)) if state_root(env=env) is not None else None,
        "libclang_path": str(libclang) if libclang is not None else None,
        "gcc_include_dirs": [str(path) for path in gcc_include_dirs(env=env)],
        "python": Path(sys.executable).name,
        "overrides": {
            key: env[key]
            for key in (ENV_TEMP, ENV_TOOLS, ENV_STATE, ENV_REQUIRE_TOOLS, ENV_MINGW, ENV_STRAWBERRY, ENV_LIBCLANG, ENV_GCC_ROOT)
            if key in env
        },
    }


def main(argv: list[str] | None = None) -> int:
    parser = argparse.ArgumentParser(description="Show portable ARCHMELD environment resolution")
    parser.add_argument("--json", action="store_true")
    parser.add_argument("--check", action="store_true", help="verify the temp root is usable")
    args = parser.parse_args(argv)
    data = describe()
    if args.check:
        try:
            temp_dir("portable-env-check-")
        except Exception as error:  # noqa: BLE001 - report the actionable error
            print(f"FAIL temp root: {error}", file=sys.stderr)
            return 1
        data["temp_root_writable"] = True
    if args.json:
        print(json.dumps(data, ensure_ascii=False, indent=2))
    else:
        for key, value in data.items():
            print(f"{key}: {value}")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
