#!/usr/bin/env python3
"""Canonical ARCHMELD index-helper build.

The helper is a `codegraph-server` example, but its build environment is easy
to get wrong: the pinned RocksDB needs a MinGW runtime, libclang for bindgen,
a writable temp directory, and (in the dev profile) the ARCHMELD profile
override. This script builds one profile, then proves the result reports export
schema 1.2 and records a schema sidecar for the Linux release bundle.

Usage:
  py -3.11 tooling/build_helper.py --profile dev --sidecar target/debug/helper-schema.txt
  py -3.11 tooling/build_helper.py --profile release --sidecar target/release/helper-schema.txt
"""
from __future__ import annotations

import argparse
import json
import os
import re
import shutil
import subprocess
import sys
from pathlib import Path

import common

ROOT = Path(__file__).resolve().parents[1]
DEFAULT_RUST_TOOLCHAIN = "1.94.1-x86_64-pc-windows-gnu"
sha256 = common.sha256_file


def _effective_toolchain(env: dict[str, str]) -> str:
    """Pin the GNU host triple.

    `rust-toolchain.toml` only pins the channel, so an inherited
    `RUSTUP_TOOLCHAIN=1.94.1` resolves the MSVC host on this machine. Accept an
    explicit full triple, otherwise force the documented GNU toolchain.
    """
    explicit = env.get("ARCHMELD_RUST_TOOLCHAIN")
    if explicit:
        return explicit
    inherited = env.get("RUSTUP_TOOLCHAIN", "")
    if "x86_64-pc-windows-gnu" in inherited:
        return inherited
    return DEFAULT_RUST_TOOLCHAIN


def _prepend_path(env: dict[str, str], directory: str | Path) -> None:
    """Put `directory` first exactly once, preserving the rest of PATH."""
    target = Path(directory)
    parts = [part for part in env.get("PATH", "").split(os.pathsep) if part]
    if parts and Path(parts[0]) == target:
        return
    parts = [part for part in parts if Path(part) != target]
    env["PATH"] = os.pathsep.join([str(target), *parts])


def resolve_executable(command: list[str], env: dict[str, str]) -> list[str]:
    """Resolve argv[0] against the *child* PATH.

    Windows CreateProcess searches the caller's PATH, not the child env, so a
    cargo/rustc that only exists on the resolved PATH needs an absolute
    argv[0].
    """
    if not command:
        return command
    first = command[0]
    if any(separator in first for separator in ("/", "\\")):
        return list(command)
    found = shutil.which(first, path=env.get("PATH", ""))
    return [found or first, *command[1:]]


def cargo_env(
    profile: str = "dev",
    env: dict[str, str] | None = None,
    mode: str = "canonical",
    overrides: dict[str, str | None] | None = None,
) -> dict[str, str]:
    """Canonical Cargo environment for this Windows GNU worktree.

    Shared by the helper build, `cargo_test.py`, test lanes and the command
    recorder so every path sees the same toolchain, MinGW DLL search order,
    bindgen include list and temp root. The returned dict is a copy; the
    caller's environment is not mutated.

    `mode="fast"` is reserved for the explicit fast lane. Callers pass cache,
    linker and sccache knobs through `overrides`; canonical mode removes those
    knobs so an earlier experiment cannot change the build fingerprint.
    """
    sys.path.insert(0, str(ROOT / "tooling"))
    import portable_env  # noqa: E402

    base = dict(env or os.environ)
    if os.name == "nt" and not (
        base.get("ARCHMELD_MINGW_BIN") or base.get("ARCHMELD_STRAWBERRY_BIN")
    ):
        discovered = portable_env.which("gcc", env=base)
        if discovered is not None:
            base["ARCHMELD_MINGW_BIN"] = str(discovered.parent)
    result = portable_env.subprocess_env(env=base)

    toolchain = _effective_toolchain(base)
    result["RUSTUP_TOOLCHAIN"] = toolchain
    result["ARCHMELD_RUST_TOOLCHAIN"] = toolchain

    mingw = result.get("ARCHMELD_MINGW_BIN") or result.get("ARCHMELD_STRAWBERRY_BIN")
    if mingw and os.name == "nt":
        _prepend_path(result, mingw)

    temp = result.get("ARCHMELD_TEMP") or result.get("TMP")
    if temp:
        for key in ("TMP", "TEMP", "TMPDIR"):
            result[key] = temp

    if not result.get("LIBCLANG_PATH"):
        libclang = portable_env.libclang_path(env=result)
        if libclang is not None:
            result["LIBCLANG_PATH"] = str(libclang)

    includes = portable_env.gcc_include_dirs(env=result)
    gcc = portable_env.which("gcc", env=result)
    if gcc:
        system_include = Path(gcc).resolve().parent.parent / "include"
        if system_include.is_dir():
            includes = [system_include] + [
                path for path in includes if path != system_include
            ]

    cxxflags = base.get("ARCHMELD_CXXFLAGS") or "-include cstdint"
    result["CXXFLAGS"] = cxxflags
    # Keep the target-prefixed spelling in lockstep for dev and release. cc-rs
    # reads it first; a profile-dependent presence is a fingerprint branch.
    result["CXXFLAGS_x86_64_pc_windows_gnu"] = cxxflags

    explicit_bindgen = base.get("ARCHMELD_BINDGEN_ARGS")
    if explicit_bindgen:
        result["BINDGEN_EXTRA_CLANG_ARGS"] = explicit_bindgen
    elif includes:
        result["BINDGEN_EXTRA_CLANG_ARGS"] = " ".join(
            f"-I{path.as_posix()}" for path in includes
        )
    else:
        result.pop("BINDGEN_EXTRA_CLANG_ARGS", None)

    result["CARGO_INCREMENTAL"] = "0"
    for key in (
        "ROCKSDB_LIB_DIR",
        "ROCKSDB_STATIC",
        "SNAPPY_LIB_DIR",
        "RUSTFLAGS",
        "RUSTC_WRAPPER",
        "SCCACHE_DIR",
    ):
        result.pop(key, None)
    explicit_rustflags = base.get("ARCHMELD_RUSTFLAGS")
    if explicit_rustflags:
        result["RUSTFLAGS"] = explicit_rustflags

    for key, value in (overrides or {}).items():
        if value is None:
            result.pop(key, None)
        else:
            result[key] = str(value)
    return result


def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("--profile", choices=["dev", "release"], default="dev")
    parser.add_argument("--jobs", type=int, default=2)
    parser.add_argument("--target-dir", default=None)
    parser.add_argument("--sidecar", default=None)
    parser.add_argument("--no-build", action="store_true")
    args = parser.parse_args()

    env = cargo_env(args.profile)

    if not args.no_build:
        command = [
            "cargo",
            "build",
            "--offline",
            "--locked",
            "-p",
            "codegraph-server",
            "--example",
            "archmeld_export",
            "-j",
            str(args.jobs),
        ]
        if args.profile == "release":
            command.insert(2, "--release")
        if args.target_dir:
            command[2:2] = ["--target-dir", args.target_dir]
        result = subprocess.run(resolve_executable(command, env), cwd=ROOT, env=env)
        if result.returncode != 0:
            return result.returncode

    target = Path(args.target_dir) if args.target_dir else ROOT / "target"
    profile_dir = target / (args.profile if args.profile == "release" else "debug")
    helper = profile_dir / "examples" / (
        "archmeld_export.exe" if os.name == "nt" else "archmeld_export"
    )
    if not helper.is_file():
        print(f"helper not found: {helper}", file=sys.stderr)
        return 1
    schema = subprocess.run(
        [str(helper), "--schema"], capture_output=True, text=True, timeout=60, env=env
    )
    version = subprocess.run(
        [str(helper), "--version"], capture_output=True, text=True, timeout=60, env=env
    )
    schema_text = (schema.stdout or "").strip()
    version_text = (version.stdout or version.stderr or "").strip()
    if schema.returncode != 0 or schema_text != "1.2":
        print(
            f"helper schema check failed: exit={schema.returncode} schema={schema_text!r}",
            file=sys.stderr,
        )
        return 1
    if args.sidecar:
        sidecar = Path(args.sidecar)
        sidecar.parent.mkdir(parents=True, exist_ok=True)
        sidecar.write_text(schema_text + "\n", encoding="utf-8")
    match = re.search(r"commit\s+([0-9a-f]{7,40}(?:-dirty)?)", version_text)
    build_commit = match.group(1) if match else "unknown"
    provenance = {
        "schema": schema_text,
        "version": version_text,
        "build_git_commit": build_commit,
        "build_git_dirty": build_commit.endswith("-dirty"),
        "sha256": sha256(helper),
        "bytes": helper.stat().st_size,
        "profile": args.profile,
    }
    (helper.parent / "helper-provenance.json").write_text(
        json.dumps(provenance, ensure_ascii=False, indent=2) + "\n",
        encoding="utf-8",
    )
    print(
        json.dumps(
            {
                "profile": args.profile,
                "helper": str(helper),
                "bytes": helper.stat().st_size,
                "sha256": sha256(helper),
                "schema": schema_text,
                "version": version_text,
            }
        )
    )
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
