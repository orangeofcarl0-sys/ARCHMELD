#!/usr/bin/env python3
"""Pin a prebuilt librocksdb archive for the fast test lane.

The canonical lane always builds RocksDB from the pinned source. The fast lane
may link a verified archive instead, which removes the 322 C++ translation
units from every cold build. The archive is keyed by crate version, feature
set, target, rustc version, GCC version and CXXFLAGS; a mismatch is an error,
never a silent fallback.

Examples:
  py -3.11 tooling/rocksdb_cache.py status
  py -3.11 tooling/rocksdb_cache.py create --source-dir target/debug/build/librocksdb-sys-bbca3a7ec03e7cf9/out
  py -3.11 tooling/rocksdb_cache.py create            # build from source (slow)
  py -3.11 tooling/rocksdb_cache.py verify --full
"""
from __future__ import annotations

import argparse
import json
import os
import re
import shutil
import subprocess
import sys
from datetime import datetime, timezone
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]
sys.path.insert(0, str(ROOT / "tooling"))
import build_helper  # noqa: E402
import common  # noqa: E402
import portable_env  # noqa: E402

FEATURES = ["bzip2", "lz4", "snappy", "static", "zlib", "zstd"]
TARGET = "x86_64-pc-windows-gnu"
LINK_ARGS = ["-lrpcrt4", "-lshlwapi"]
SCHEMA = "archmeld-rocksdb-cache/1"
sha256_file = common.sha256_file


class CacheError(RuntimeError):
    pass


def lock_version(name: str = "librocksdb-sys") -> str:
    text = (ROOT / "Cargo.lock").read_text(encoding="utf-8")
    pattern = re.compile(
        r'\[\[package\]\]\s*name = "' + re.escape(name) + r'"\s*version = "([^"]+)"'
    )
    match = pattern.search(text)
    if not match:
        raise CacheError(f"{name} not found in Cargo.lock")
    return match.group(1)


def tool_version(argv: list[str], env: dict[str, str]) -> str:
    try:
        result = subprocess.run(
            build_helper.resolve_executable(argv, env),
            capture_output=True,
            text=True,
            timeout=30,
            env=env,
        )
    except (OSError, subprocess.SubprocessError) as exc:
        raise CacheError(f"cannot run {argv}: {exc}") from exc
    text = (result.stdout or result.stderr).strip().splitlines()
    return text[0] if text else ""


def versions(env: dict[str, str]) -> dict[str, str]:
    rustc = tool_version(["rustc", "-vV"], env)
    gcc_path = portable_env.which("gcc", env=env)
    if gcc_path is None:
        raise CacheError("gcc is not on the resolved PATH")
    gcc = tool_version([str(gcc_path), "--version"], env)
    return {"rustc": rustc, "gcc": gcc, "target": TARGET}


def cache_root(env: dict[str, str]) -> Path:
    explicit = env.get("ARCHMELD_ROCKSDB_CACHE")
    if explicit:
        return Path(explicit)
    root = portable_env.tools_root(env=env)
    if root is None:
        raise CacheError(
            "ARCHMELD_TOOLS_ROOT is not set and the repository-sibling "
            "_archmeld_tools directory does not exist"
        )
    return root / "rocksdb"


def key_material(env: dict[str, str]) -> dict[str, str]:
    material = {
        "crate": f"librocksdb-sys {lock_version()}",
        "features": ",".join(FEATURES),
        "target": TARGET,
        "cxxflags": env.get("CXXFLAGS", ""),
        "bindgen": env.get("BINDGEN_EXTRA_CLANG_ARGS", ""),
    }
    material.update(versions(env))
    return material


def cache_key(env: dict[str, str]) -> str:
    material = key_material(env)
    payload = json.dumps(material, sort_keys=True, ensure_ascii=False)
    digest = common.sha256_text(payload)[:12]
    crate = lock_version().replace("+", "_")
    match = re.search(r"(\d+\.\d+\.\d+)", material.get("gcc", ""))
    gcc = match.group(1) if match else "gcc"
    return f"librocksdb-{crate}-{TARGET}-gcc{gcc}-{digest}"


def read_provenance(directory: Path) -> dict:
    path = directory / "provenance.json"
    if not path.is_file():
        raise CacheError(f"{directory}: provenance.json missing")
    try:
        value = json.loads(path.read_text(encoding="utf-8"))
    except (OSError, json.JSONDecodeError) as exc:
        raise CacheError(f"{directory}: invalid provenance: {exc}") from exc
    if not isinstance(value, dict):
        raise CacheError(f"{directory}: provenance is not an object")
    return value


def verify_dir(directory: Path, env: dict[str, str]) -> dict:
    provenance = read_provenance(directory)
    archive = directory / "librocksdb.a"
    if not archive.is_file():
        raise CacheError(f"{directory}: librocksdb.a missing")
    if provenance.get("schema") != SCHEMA:
        raise CacheError(f"{directory}: unexpected schema {provenance.get('schema')!r}")
    expected = key_material(env)
    for key in ("crate", "features", "target", "cxxflags", "rustc", "gcc"):
        if provenance.get(key) != expected[key]:
            raise CacheError(
                f"{directory}: {key} mismatch ({provenance.get(key)!r} != {expected[key]!r})"
            )
    actual = sha256_file(archive)
    if provenance.get("archive_sha256") != actual:
        raise CacheError(f"{directory}: archive sha256 changed")
    if provenance.get("bytes") != archive.stat().st_size:
        raise CacheError(f"{directory}: archive size changed")
    snappy = directory / "libsnappy.a"
    link_args = list(LINK_ARGS)
    if snappy.is_file():
        link_args.append("-lstdc++")
    return {
        "dir": str(directory),
        "sha256": actual,
        "bytes": archive.stat().st_size,
        "provenance": provenance,
        "env": {
            "ROCKSDB_LIB_DIR": str(directory),
            "ROCKSDB_STATIC": "1",
            **({"SNAPPY_LIB_DIR": str(directory)} if snappy.is_file() else {}),
        },
        "link_args": link_args,
    }


def resolve(mode: str = "auto", env: dict[str, str] | None = None) -> dict:
    env = dict(env or os.environ)
    root = cache_root(env)
    if not root.is_dir():
        raise CacheError(
            f"RocksDB cache root {root} does not exist; run "
            "`py -3.11 tooling/rocksdb_cache.py create --source-dir <out>` "
            "or use --rocksdb source"
        )
    candidates = sorted(
        (path for path in root.iterdir() if path.is_dir()),
        key=lambda path: path.stat().st_mtime,
        reverse=True,
    )
    errors: list[str] = []
    for directory in candidates:
        try:
            return verify_dir(directory, env)
        except CacheError as exc:
            errors.append(str(exc))
    detail = "\n".join(errors[:5]) if errors else "no cache directories"
    raise CacheError(
        f"no verified RocksDB cache under {root}:\n{detail}\n"
        "use --rocksdb source to build from source"
    )


def copy_archive(source: Path, directory: Path) -> dict[str, str]:
    directory.mkdir(parents=True, exist_ok=True)
    archive = source / "librocksdb.a"
    if not archive.is_file():
        raise CacheError(f"{source}: librocksdb.a missing")
    shutil.copy2(archive, directory / "librocksdb.a")
    snappy = source / "libsnappy.a"
    if snappy.is_file():
        shutil.copy2(snappy, directory / "libsnappy.a")
    return {
        "archive_sha256": sha256_file(directory / "librocksdb.a"),
        "bytes": (directory / "librocksdb.a").stat().st_size,
    }


def build_from_source(env: dict[str, str], jobs: int) -> Path:
    print("building librocksdb-sys from source (this is the slow path)", file=sys.stderr)
    command = [
        "cargo",
        "build",
        "-p",
        "librocksdb-sys",
        "--offline",
        "--locked",
        "-j",
        str(jobs),
    ]
    result = subprocess.run(
        build_helper.resolve_executable(command, env), cwd=ROOT, env=env
    )
    if result.returncode != 0:
        raise CacheError(f"cargo build -p librocksdb-sys failed: {result.returncode}")
    candidates = sorted(
        (ROOT / "target" / "debug" / "build").glob("librocksdb-sys-*/out/librocksdb.a"),
        key=lambda path: path.stat().st_mtime,
        reverse=True,
    )
    if not candidates:
        raise CacheError("no librocksdb.a found under target/debug/build")
    return candidates[0].parent


def create(source_dir: str | None, jobs: int, env: dict[str, str]) -> dict:
    directory = cache_root(env) / cache_key(env)
    if (directory / "librocksdb.a").is_file():
        try:
            return verify_dir(directory, env)
        except CacheError:
            pass
    source = Path(source_dir) if source_dir else build_from_source(env, jobs)
    copied = copy_archive(source, directory)
    material = key_material(env)
    provenance = {
        "schema": SCHEMA,
        "created_at": datetime.now(timezone.utc).isoformat(),
        "key": directory.name,
        "source": str(source),
        "cxxflags": env.get("CXXFLAGS", ""),
        "bindgen": env.get("BINDGEN_EXTRA_CLANG_ARGS", ""),
        "features": ",".join(FEATURES),
        "link_args": LINK_ARGS,
        **material,
        **copied,
    }
    (directory / "provenance.json").write_text(
        json.dumps(provenance, ensure_ascii=False, indent=2) + "\n", encoding="utf-8"
    )
    return verify_dir(directory, env)


def full_verify(info: dict, env: dict[str, str], target_dir: str) -> int:
    overrides = dict(info["env"])
    if info["link_args"]:
        overrides["RUSTFLAGS"] = " ".join(
            f"-Clink-arg={item}" for item in info["link_args"]
        )
    overrides["CARGO_TARGET_DIR"] = target_dir
    check_env = build_helper.cargo_env(mode="fast", env=env, overrides=overrides)
    command = [
        "cargo",
        "test",
        "-p",
        "codegraph",
        "--lib",
        "--no-run",
        "--offline",
        "--locked",
        "-j",
        "4",
        "--target-dir",
        target_dir,
    ]
    print(f"$ {' '.join(command)}", flush=True)
    result = subprocess.run(
        build_helper.resolve_executable(command, check_env), cwd=ROOT, env=check_env
    )
    if result.returncode != 0:
        return result.returncode
    runtime = [
        "cargo",
        "test",
        "-p",
        "codegraph",
        "--lib",
        "test_delete",
        "--offline",
        "--locked",
        "-j",
        "4",
        "--target-dir",
        target_dir,
    ]
    print(f"$ {' '.join(runtime)}", flush=True)
    result = subprocess.run(
        build_helper.resolve_executable(runtime, check_env), cwd=ROOT, env=check_env
    )
    if result.returncode != 0:
        return result.returncode
    print("prebuilt RocksDB link and runtime smoke passed")
    return 0


def status(env: dict[str, str]) -> int:
    root = cache_root(env)
    payload: list[dict] = []
    if root.is_dir():
        for directory in sorted(root.iterdir()):
            if not directory.is_dir():
                continue
            try:
                info = verify_dir(directory, env)
                payload.append(
                    {
                        "dir": info["dir"],
                        "sha256": info["sha256"],
                        "bytes": info["bytes"],
                        "verify": "ok",
                    }
                )
            except CacheError as exc:
                payload.append({"dir": str(directory), "verify": "failed", "error": str(exc)})
    print(json.dumps({"root": str(root), "entries": payload}, ensure_ascii=False, indent=2))
    return 0


def main() -> int:
    parser = argparse.ArgumentParser()
    sub = parser.add_subparsers(dest="command", required=True)
    create_parser = sub.add_parser("create")
    create_parser.add_argument("--source-dir", default=None)
    create_parser.add_argument("--jobs", type=int, default=2)
    sub.add_parser("status")
    verify_parser = sub.add_parser("verify")
    verify_parser.add_argument("--full", action="store_true")
    verify_parser.add_argument("--target-dir", default="target-rocksdb-cache-check")
    args = parser.parse_args()

    env = build_helper.cargo_env(mode="canonical")
    try:
        if args.command == "create":
            info = create(args.source_dir, args.jobs, env)
            print(json.dumps({key: info[key] for key in ("dir", "sha256", "bytes")}, ensure_ascii=False, indent=2))
            return 0
        if args.command == "status":
            return status(env)
        info = resolve(env=env)
        print(json.dumps({key: info[key] for key in ("dir", "sha256", "bytes", "env", "link_args")}, ensure_ascii=False, indent=2))
        if args.full:
            return full_verify(info, env, args.target_dir)
        return 0
    except CacheError as exc:
        print(f"ERROR: {exc}", file=sys.stderr)
        return 1


if __name__ == "__main__":
    raise SystemExit(main())
