#!/usr/bin/env python3
"""Probe whether lld can link this Windows GNU workspace.

The fast lane may use lld only after a real C link, a real Rust link and (when
a prebuilt RocksDB cache is available) a workspace link probe all succeed. A
failed probe returns BFD; it never silently claims lld support.

Examples:
  py -3.11 tooling/link_probe.py --probe
  py -3.11 tooling/link_probe.py --probe --workspace --json
"""
from __future__ import annotations

import argparse
import json
import os
import subprocess
import sys
import time
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]
sys.path.insert(0, str(ROOT / "tooling"))
import build_helper  # noqa: E402
import portable_env  # noqa: E402


class LldError(RuntimeError):
    pass


def sysroot(env: dict[str, str]) -> Path:
    command = build_helper.resolve_executable(["rustc", "--print", "sysroot"], env)
    result = subprocess.run(command, capture_output=True, text=True, timeout=30, env=env)
    text = (result.stdout or "").strip()
    if result.returncode != 0 or not text:
        raise LldError(f"rustc --print sysroot failed: {result.returncode}")
    return Path(text)


def gcc_ld_dir(env: dict[str, str]) -> Path | None:
    candidate = (
        sysroot(env)
        / "lib"
        / "rustlib"
        / "x86_64-pc-windows-gnu"
        / "bin"
        / "gcc-ld"
    )
    return candidate if candidate.is_dir() else None


def llvm_lld_dir(env: dict[str, str]) -> Path | None:
    root = portable_env.tools_root(env=env)
    if root is None:
        return None
    candidate = root / "llvm18" / "bin"
    if (candidate / "ld.lld.exe").is_file() or (candidate / "ld.lld").is_file():
        return candidate
    return None


def linker_dir(env: dict[str, str]) -> Path:
    for candidate in (gcc_ld_dir(env), llvm_lld_dir(env)):
        if candidate is not None:
            return candidate
    raise LldError("no lld shim found (rustup gcc-ld and LLVM18 bin both missing)")


def link_args(env: dict[str, str]) -> list[str]:
    directory = linker_dir(env)
    return ["-fuse-ld=lld", f"-B{directory.as_posix()}/"]


def lld_version(env: dict[str, str]) -> str:
    directory = linker_dir(env)
    for name in ("ld.lld.exe", "ld.lld", "rust-lld.exe", "lld.exe"):
        path = directory / name
        if path.is_file():
            result = subprocess.run(
                [str(path), "--version"], capture_output=True, text=True, timeout=30, env=env
            )
            text = (result.stdout or result.stderr).strip().splitlines()
            if text:
                return text[0]
    return "unknown"


def run_checked(
    argv: list[str], env: dict[str, str], label: str, timeout: int = 120
) -> dict[str, object]:
    started = time.monotonic()
    result = subprocess.run(
        argv, capture_output=True, text=True, env=env, timeout=timeout
    )
    return {
        "label": label,
        "argv": argv,
        "exit_code": result.returncode,
        "duration_s": round(time.monotonic() - started, 3),
        "stdout": (result.stdout or "").strip()[-2000:],
        "stderr": (result.stderr or "").strip()[-2000:],
    }


def probe_c(env: dict[str, str]) -> dict[str, object]:
    temp = portable_env.temp_dir("archmeld-lld-c-", env=env)
    source = temp / "main.c"
    binary = temp / "main.exe"
    source.write_text("int main(void) { return 0; }\n", encoding="utf-8")
    gcc = portable_env.which("gcc", env=env)
    if gcc is None:
        raise LldError("gcc is not on the resolved PATH")
    args = [str(gcc), *link_args(env), str(source), "-o", str(binary)]
    result = run_checked(args, env, "c-link")
    if result["exit_code"] != 0:
        return result
    result["run"] = run_checked([str(binary)], env, "c-run")
    return result


def probe_rust(env: dict[str, str]) -> dict[str, object]:
    temp = portable_env.temp_dir("archmeld-lld-rust-", env=env)
    source = temp / "main.rs"
    binary = temp / "main.exe"
    source.write_text("fn main() {}\n", encoding="utf-8")
    rustc = build_helper.resolve_executable(["rustc"], env)[0]
    args = [rustc, str(source), "-o", str(binary)]
    for item in link_args(env):
        args.extend(["-C", f"link-arg={item}"])
    result = run_checked(args, env, "rust-link")
    if result["exit_code"] != 0:
        return result
    result["run"] = run_checked([str(binary)], env, "rust-run")
    return result


def probe_workspace(env: dict[str, str], target_dir: str, jobs: int) -> dict[str, object]:
    try:
        import rocksdb_cache  # noqa: PLC0415

        cache = rocksdb_cache.resolve(mode="auto", env=env)
    except Exception as exc:  # noqa: BLE001 - a skipped probe is recorded
        return {
            "label": "workspace-link",
            "skipped": True,
            "reason": f"prebuilt RocksDB cache unavailable: {exc}",
        }
    overrides = dict(cache["env"])
    args = list(link_args(env))
    overrides["RUSTFLAGS"] = " ".join(f"-Clink-arg={item}" for item in args)
    probe_env = build_helper.cargo_env(mode="fast", env=env, overrides=overrides)
    command = [
        "cargo",
        "test",
        "-p",
        "codegraph-server",
        "--lib",
        "--no-run",
        "--offline",
        "--locked",
        "-j",
        str(jobs),
        "--target-dir",
        target_dir,
    ]
    return run_checked(
        build_helper.resolve_executable(command, probe_env),
        probe_env,
        "workspace-link",
        timeout=1800,
    )


def probe(json_output: bool = False, workspace: bool = False, target_dir: str = "target-lld") -> dict:
    env = build_helper.cargo_env(mode="canonical")
    payload: dict[str, object] = {
        "linker": "bfd",
        "link_args": [],
        "version": None,
        "probes": [],
    }
    try:
        payload["version"] = lld_version(env)
        for probe_fn in (probe_c, probe_rust):
            result = probe_fn(env)
            payload["probes"].append(result)
            if result.get("exit_code") != 0 or (
                isinstance(result.get("run"), dict) and result["run"].get("exit_code") != 0
            ):
                raise LldError(f"{result['label']} probe failed")
        if workspace:
            payload["probes"].append(probe_workspace(env, target_dir, 4))
        payload["linker"] = "lld"
        payload["link_args"] = link_args(env)
    except (LldError, OSError, subprocess.SubprocessError) as exc:
        payload["reason"] = str(exc)
    if json_output:
        print(json.dumps(payload, ensure_ascii=False, indent=2))
    else:
        print(f"linker: {payload['linker']} ({payload.get('version')})")
        for result in payload["probes"]:
            print(
                f"{result.get('label')}: exit={result.get('exit_code')} "
                f"duration={result.get('duration_s')}s"
            )
        if payload.get("reason"):
            print(f"fallback: {payload['reason']}")
    return payload


def resolve(mode: str = "auto", env: dict[str, str] | None = None) -> dict:
    if mode == "bfd":
        return {"linker": "bfd", "link_args": [], "version": None}
    payload = probe()
    if payload["linker"] == "lld":
        return payload
    if mode == "lld":
        raise LldError(f"lld probe failed: {payload.get('reason')}")
    return {"linker": "bfd", "link_args": [], "version": None, "reason": payload.get("reason")}


def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("--probe", action="store_true")
    parser.add_argument("--workspace", action="store_true")
    parser.add_argument("--target-dir", default="target-lld")
    parser.add_argument("--json", action="store_true")
    args = parser.parse_args()
    payload = probe(json_output=args.json, workspace=args.workspace, target_dir=args.target_dir)
    return 0 if payload["linker"] == "lld" else 1


if __name__ == "__main__":
    raise SystemExit(main())
