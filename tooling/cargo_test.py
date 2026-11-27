#!/usr/bin/env python3
"""Run Cargo tests with the canonical Windows GNU environment.

This wrapper exists because a plain `cargo test` on this host resolves the
MSVC toolchain from `rust-toolchain.toml` and can load Git's mingw64
`libstdc++-6.dll` at test runtime. Both produce failures that look like code
bugs. It reuses `build_helper.cargo_env()` so the helper build, test runs and
recorded evidence see the same toolchain, include list and DLL search order.

Examples:
  py -3.11 tooling/cargo_test.py -- -p codegraph-server --lib
  py -3.11 tooling/cargo_test.py -- -p codegraph --lib
  py -3.11 tooling/cargo_test.py -- --release -p codegraph --lib test_performance_large_index
"""
from __future__ import annotations

import argparse
import json
import subprocess
import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]
sys.path.insert(0, str(ROOT / "tooling"))
import build_helper  # noqa: E402


def version(cmd: list[str], env: dict[str, str]) -> dict[str, object]:
    cmd = build_helper.resolve_executable(cmd, env)
    result = subprocess.run(
        cmd, cwd=ROOT, env=env, capture_output=True, text=True, timeout=30
    )
    text = (result.stdout or result.stderr).strip().splitlines()
    return {
        "argv": cmd,
        "exit": result.returncode,
        "first_line": text[0] if text else "",
    }


def has_jobs(argv: list[str]) -> bool:
    for item in argv:
        if item in {"-j", "--jobs"}:
            return True
        if item.startswith("-j") and item[2:].isdigit():
            return True
        if item.startswith("--jobs="):
            return True
    return False


def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("--jobs", type=int, default=None)
    parser.add_argument("--profile", choices=["dev", "release"], default="dev")
    parser.add_argument("--lane", choices=["canonical", "fast"], default="canonical")
    parser.add_argument("--target-dir", default=None)
    parser.add_argument("argv", nargs=argparse.REMAINDER)
    args = parser.parse_args()

    argv = list(args.argv)
    if argv and argv[0] == "--":
        argv = argv[1:]
    env = build_helper.cargo_env(args.profile, mode=args.lane)
    jobs = args.jobs if args.jobs is not None else (4 if args.lane == "fast" else 2)

    command = ["cargo", "test"]
    if args.profile == "release" and "--release" not in argv:
        command.append("--release")
    if "--offline" not in argv:
        command.append("--offline")
    if "--locked" not in argv:
        command.append("--locked")
    if not has_jobs(argv):
        command.extend(["-j", str(jobs)])
    if args.target_dir and "--target-dir" not in argv:
        command.extend(["--target-dir", args.target_dir])
    command.extend(argv)

    fingerprint = {
        "lane": args.lane,
        "profile": args.profile,
        "target_dir": args.target_dir,
        "rust_toolchain": env.get("RUSTUP_TOOLCHAIN"),
        "archmeld_rust_toolchain": env.get("ARCHMELD_RUST_TOOLCHAIN"),
        "mingw_bin": env.get("ARCHMELD_MINGW_BIN") or env.get("ARCHMELD_STRAWBERRY_BIN"),
        "libclang": env.get("LIBCLANG_PATH"),
        "bindgen_args": env.get("BINDGEN_EXTRA_CLANG_ARGS"),
        "cxxflags": env.get("CXXFLAGS"),
        "dev_cxxflags": env.get("CXXFLAGS_x86_64_pc_windows_gnu"),
        "cargo_incremental": env.get("CARGO_INCREMENTAL"),
        "temp": env.get("TMP"),
        "rocksdb_source": "source" if not env.get("ROCKSDB_LIB_DIR") else "prebuilt",
        "rocksdb_lib_dir": env.get("ROCKSDB_LIB_DIR"),
        "linker": "lld" if "fuse-ld=lld" in env.get("RUSTFLAGS", "") else "bfd",
        "rustc_wrapper": env.get("RUSTC_WRAPPER"),
        "sccache_dir": env.get("SCCACHE_DIR"),
        "rustc": version(["rustc", "-vV"], env),
        "cargo": version(["cargo", "--version"], env),
        "argv": command,
    }
    print(json.dumps({"cargo_test": fingerprint}, ensure_ascii=False), flush=True)
    return subprocess.run(
        build_helper.resolve_executable(command, env), cwd=ROOT, env=env
    ).returncode


if __name__ == "__main__":
    raise SystemExit(main())
