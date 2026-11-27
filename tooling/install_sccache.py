#!/usr/bin/env python3
"""Resolve, install and smoke-test sccache for the fast lane.

sccache is pinned; it is never resolved from "latest" at runtime. The install
path uses crates.io with `--locked` (checksums verified by Cargo). A local
smoke test compiles C, C++ and Rust twice and checks that the second compile
hit the cache before the fast lane enables the wrapper.

Examples:
  py -3.11 tooling/install_sccache.py status
  py -3.11 tooling/install_sccache.py install --version 0.20.1
  py -3.11 tooling/install_sccache.py smoke
  py -3.11 tooling/install_sccache.py stop
"""
from __future__ import annotations

import argparse
import hashlib
import json
import os
import subprocess
import sys
import time
from datetime import datetime, timezone
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]
sys.path.insert(0, str(ROOT / "tooling"))
import build_helper  # noqa: E402
import portable_env  # noqa: E402

PINNED_VERSION = "0.20.1"
SCHEMA = "archmeld-sccache/1"


class SccacheError(RuntimeError):
    pass


def sha256_file(path: Path) -> str:
    digest = hashlib.sha256()
    with path.open("rb") as handle:
        for chunk in iter(lambda: handle.read(1 << 20), b""):
            digest.update(chunk)
    return digest.hexdigest()


def tools_root(env: dict[str, str]) -> Path:
    root = env.get("ARCHMELD_TOOLS_ROOT") or portable_env.tools_root(env=env)
    if root is None:
        raise SccacheError(
            "ARCHMELD_TOOLS_ROOT is not set and _archmeld_tools does not exist"
        )
    return Path(root)


def candidates(env: dict[str, str], version: str) -> list[Path]:
    result: list[Path] = []
    explicit = env.get("ARCHMELD_SCCACHE")
    if explicit:
        result.append(Path(explicit))
    root = tools_root(env)
    for name in ("sccache.exe", "sccache"):
        result.append(root / "sccache" / version / "bin" / name)
    found = portable_env.which("sccache", env=env)
    if found is not None:
        result.append(found)
    return result


def find_binary(env: dict[str, str], version: str = PINNED_VERSION) -> Path | None:
    for path in candidates(env, version):
        if path.is_file():
            return path
    return None


def version_text(binary: Path, env: dict[str, str]) -> str:
    result = subprocess.run(
        [str(binary), "--version"], capture_output=True, text=True, timeout=30, env=env
    )
    text = (result.stdout or result.stderr).strip().splitlines()
    return text[0] if text else "unknown"


def wrapper(value: str) -> str:
    """cc-rs splits on whitespace; all resolved tool paths here are space-free."""
    return value


def overrides(binary: Path, env: dict[str, str]) -> dict[str, str]:
    gcc = portable_env.which("gcc", env=env)
    gxx = portable_env.which("g++", env=env)
    if gcc is None or gxx is None:
        raise SccacheError("gcc/g++ are not on the resolved PATH")
    root = tools_root(env)
    cache_dir = root / "sccache" / "cache"
    cache_dir.mkdir(parents=True, exist_ok=True)
    cc = wrapper(f"{binary} {gcc}")
    cxx = wrapper(f"{binary} {gxx}")
    return {
        "RUSTC_WRAPPER": str(binary),
        "CC": cc,
        "CXX": cxx,
        "CC_x86_64_pc_windows_gnu": cc,
        "CXX_x86_64_pc_windows_gnu": cxx,
        "SCCACHE_DIR": str(cache_dir),
        "SCCACHE_CACHE_SIZE": env.get("ARCHMELD_SCCACHE_CACHE_SIZE", "10G"),
    }


def resolve(mode: str = "auto", env: dict[str, str] | None = None) -> dict:
    env = dict(env or os.environ)
    binary = find_binary(env)
    if binary is None:
        if mode == "on":
            raise SccacheError(
                "sccache is not installed; run "
                "`py -3.11 tooling/install_sccache.py install --version 0.20.1` "
                "or use --sccache off"
            )
        return {
            "enabled": False,
            "env": {},
            "version": None,
            "reason": "sccache not installed",
        }
    return {
        "enabled": True,
        "env": overrides(binary, env),
        "binary": str(binary),
        "version": version_text(binary, env),
        "cache_dir": str(tools_root(env) / "sccache" / "cache"),
    }


def stats(binary: Path, env: dict[str, str]) -> dict:
    result = subprocess.run(
        [str(binary), "--show-stats", "--stats-format=json"],
        capture_output=True,
        text=True,
        timeout=60,
        env=env,
    )
    text = (result.stdout or "").strip()
    try:
        value = json.loads(text)
    except json.JSONDecodeError:
        return {"raw": text, "exit_code": result.returncode}
    return value if isinstance(value, dict) else {"raw": value}


def counter(value: dict, name: str) -> int:
    """Sum a `stats.<name>.counts` map from `--stats-format=json`."""
    stats = value.get("stats", value)
    if not isinstance(stats, dict):
        return 0
    block = stats.get(name)
    if not isinstance(block, dict):
        return 0
    counts = block.get("counts", {})
    if not isinstance(counts, dict):
        return 0
    return sum(int(item) for item in counts.values() if isinstance(item, (int, float)))


def compile_twice(argv: list[str], env: dict[str, str], label: str) -> dict:
    started = time.monotonic()
    first = subprocess.run(argv, capture_output=True, text=True, env=env, timeout=120)
    second = subprocess.run(argv, capture_output=True, text=True, env=env, timeout=120)
    return {
        "label": label,
        "argv": argv,
        "first_exit": first.returncode,
        "second_exit": second.returncode,
        "duration_s": round(time.monotonic() - started, 3),
        "first_stderr": (first.stderr or "").strip()[-1000:],
        "second_stderr": (second.stderr or "").strip()[-1000:],
    }


def smoke(binary: Path, env: dict[str, str] | None = None) -> dict:
    env = dict(env or os.environ)
    wrapper_env = dict(env)
    wrapper_env.update(overrides(binary, env))
    temp = portable_env.temp_dir("archmeld-sccache-", env=wrapper_env)
    gcc = portable_env.which("gcc", env=wrapper_env)
    gxx = portable_env.which("g++", env=wrapper_env)
    rustc = build_helper.resolve_executable(["rustc"], wrapper_env)[0]
    if gcc is None or gxx is None:
        raise SccacheError("gcc/g++ missing for sccache smoke test")

    c_source = temp / "main.c"
    c_source.write_text("int main(void) { return 0; }\n", encoding="utf-8")
    cpp_source = temp / "main.cpp"
    cpp_source.write_text("int main() { return 0; }\n", encoding="utf-8")
    rust_source = temp / "main.rs"
    rust_source.write_text("fn main() {}\n", encoding="utf-8")

    subprocess.run([str(binary), "--zero-stats"], capture_output=True, text=True, env=wrapper_env, timeout=60)
    probes = [
        compile_twice(
            [str(binary), str(gcc), "-c", str(c_source), "-o", str(temp / "main-c.o")],
            wrapper_env,
            "c",
        ),
        compile_twice(
            [str(binary), str(gxx), "-c", str(cpp_source), "-o", str(temp / "main-cpp.o")],
            wrapper_env,
            "cpp",
        ),
        compile_twice(
            [
                str(binary),
                str(rustc),
                "--emit=obj",
                "--crate-name",
                "smoke",
                str(rust_source),
                "-o",
                str(temp / "main-rust.o"),
            ],
            wrapper_env,
            "rust",
        ),
    ]
    after = stats(binary, wrapper_env)
    return {
        "probes": probes,
        "stats": after,
        "cache_hits": counter(after, "cache_hits"),
        "cache_misses": counter(after, "cache_misses"),
    }


def install(version: str, jobs: int) -> int:
    env = build_helper.cargo_env(mode="canonical")
    root = tools_root(env) / "sccache" / version
    root.mkdir(parents=True, exist_ok=True)
    binary = find_binary(env, version)
    if binary is None:
        command = [
            "cargo",
            "install",
            "sccache",
            "--locked",
            "--no-default-features",
            "--version",
            version,
            "--root",
            str(root),
            "-j",
            str(jobs),
        ]
        print(f"$ {' '.join(command)}", flush=True)
        result = subprocess.run(
            build_helper.resolve_executable(command, env), cwd=ROOT, env=env
        )
        if result.returncode != 0:
            print(f"sccache install failed: exit {result.returncode}", file=sys.stderr)
            return result.returncode
        binary = find_binary(env, version)
        if binary is None:
            print("sccache binary not found after install", file=sys.stderr)
            return 1
    smoke_result = smoke(binary, env)
    if smoke_result["cache_hits"] < 1:
        print(
            "sccache smoke test did not produce a cache hit; refusing to enable it. "
            f"stats={json.dumps(smoke_result['stats'], ensure_ascii=False)[:2000]}",
            file=sys.stderr,
        )
        return 1
    provenance = {
        "schema": SCHEMA,
        "installed_at": datetime.now(timezone.utc).isoformat(),
        "version": version_text(binary, env),
        "method": "cargo-install",
        "root": str(root),
        "binary": str(binary),
        "sha256": sha256_file(binary),
        "bytes": binary.stat().st_size,
        "cargo": build_helper.resolve_executable(["cargo", "--version"], env),
        "rustc": build_helper.resolve_executable(["rustc", "-vV"], env),
        "smoke": {
            "cache_hits": smoke_result["cache_hits"],
            "probes": [
                {
                    "label": probe["label"],
                    "first_exit": probe["first_exit"],
                    "second_exit": probe["second_exit"],
                }
                for probe in smoke_result["probes"]
            ],
        },
    }
    (root / "provenance.json").write_text(
        json.dumps(provenance, ensure_ascii=False, indent=2) + "\n", encoding="utf-8"
    )
    print(json.dumps({"version": provenance["version"], "sha256": provenance["sha256"], "cache_hits": smoke_result["cache_hits"]}, ensure_ascii=False))
    return 0


def main() -> int:
    parser = argparse.ArgumentParser()
    sub = parser.add_subparsers(dest="command", required=True)
    install_parser = sub.add_parser("install")
    install_parser.add_argument("--version", default=PINNED_VERSION)
    install_parser.add_argument("--jobs", type=int, default=4)
    sub.add_parser("status")
    sub.add_parser("smoke")
    sub.add_parser("stop")
    args = parser.parse_args()

    env = build_helper.cargo_env(mode="canonical")
    try:
        if args.command == "install":
            return install(args.version, args.jobs)
        if args.command == "status":
            print(json.dumps(resolve(mode="auto", env=env), ensure_ascii=False, indent=2))
            return 0
        binary = find_binary(env)
        if binary is None:
            print("sccache not installed", file=sys.stderr)
            return 1
        if args.command == "smoke":
            print(json.dumps(smoke(binary, env), ensure_ascii=False, indent=2))
            return 0
        result = subprocess.run([str(binary), "--stop-server"], capture_output=True, text=True, env=env, timeout=60)
        return result.returncode
    except SccacheError as exc:
        print(f"ERROR: {exc}", file=sys.stderr)
        return 1


if __name__ == "__main__":
    raise SystemExit(main())
