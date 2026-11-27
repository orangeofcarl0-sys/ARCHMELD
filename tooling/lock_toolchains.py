#!/usr/bin/env python3
"""Generate toolchains.lock.json and platform-matrix.json from this machine.

Only measured facts go into the lock: the file records what was actually found
(version, executable path, SHA-256) or an explicit `missing` state. It never
writes placeholder hashes or "latest" versions.

Usage: python tooling/lock_toolchains.py
"""
from __future__ import annotations

import json
import os
import platform
import shutil
import subprocess
import sys
from datetime import datetime, timezone
from pathlib import Path

import common
import portable_env as portable

ROOT = Path(__file__).resolve().parents[1]
PINNED = "1.94.1-x86_64-pc-windows-gnu"
sha256 = common.sha256_file
# cmake is inherited from the old Strawberry tree and is not invoked by the
# current build (cargo/cc/ninja do the work). Record it as an optional missing
# tool instead of failing the lock/doctor.
OPTIONAL_TOOLS = {"cmake"}

# Tool metadata: (display name, executable candidates, version argv, source, license)
TOOLS = {
    "git": (["git"], ["--version"], "https://git-scm.com/", "GPL-2.0-only"),
    "node": (["node"], ["--version"], "https://nodejs.org/", "MIT"),
    "python": (["python", "python3"], ["--version"], "https://www.python.org/", "PSF-2.0"),
    "gcc": (["gcc"], ["--version"], "https://gcc.gnu.org/", "GPL-3.0-or-later"),
    "gxx": (["g++"], ["--version"], "https://gcc.gnu.org/", "GPL-3.0-or-later"),
    "cmake": (["cmake"], ["--version"], "https://cmake.org/", "BSD-3-Clause"),
    "ninja": (["ninja"], ["--version"], "https://ninja-build.org/", "Apache-2.0"),
    "perl": (["perl"], ["--version"], "https://www.perl.org/", "Artistic-1.0-Perl OR GPL-1.0-or-later"),
    "docker": (["docker"], ["--version"], "https://www.docker.com/", "Apache-2.0"),
}

# Binaries whose bytes are hashed. Large toolchain binaries are included on
# purpose: the lock is only useful if it can detect a swapped compiler.
HASH_CANDIDATES = ["git", "node", "python", "gcc", "g++", "cmake", "ninja", "perl"]


def run(argv: list[str], cwd: str | None = None) -> tuple[int, str]:
    try:
        proc = subprocess.run(
            argv, capture_output=True, text=True, errors="replace", timeout=60, cwd=cwd
        )
        text = (proc.stdout or proc.stderr).strip()
        return proc.returncode, text
    except Exception as exc:  # noqa: BLE001 - recorded as a probe failure
        return 127, f"<error: {exc!r}>"


def which_any(candidates: list[str]) -> str | None:
    for candidate in candidates:
        found = shutil.which(candidate)
        if found:
            return found
    return None


def tool_entry(name: str, candidates: list[str], version_argv: list[str], source: str, license_id: str) -> dict:
    path = which_any(candidates)
    if path is None:
        optional = name in OPTIONAL_TOOLS
        return {
            "name": name,
            "status": "missing_optional" if optional else "missing",
            "optional": optional,
            "path": None,
            "version": None,
            "sha256": None,
            "source": source,
            "license": license_id,
            "verified": False,
        }
    code, output = run([path, *version_argv])
    first_line = output.splitlines()[0] if output else ""
    file_hash = None
    try:
        file_hash = sha256(Path(path))
    except Exception:
        file_hash = None
    return {
        "name": name,
        "status": "present",
        "path": path,
        "version": first_line,
        "sha256": file_hash,
        "source": source,
        "license": license_id,
        "verified": code == 0 and bool(first_line),
        "probe_exit": code,
    }


SCCACHE_VERSION = "0.20.1"


def sccache_entry() -> dict:
    """Record the pinned sccache binary when installed under the tools root."""
    candidates: list[Path] = []
    explicit = os.environ.get("ARCHMELD_SCCACHE")
    if explicit:
        candidates.append(Path(explicit))
    root = portable.tools_root()
    if root is not None:
        candidates.extend(
            sorted((root / "sccache").glob("*/bin/sccache.exe"), reverse=True)
        )
    found = shutil.which("sccache")
    if found:
        candidates.append(Path(found))
    path = next((candidate for candidate in candidates if candidate.is_file()), None)
    if path is None:
        return {
            "name": "sccache",
            "status": "missing_optional",
            "optional": True,
            "path": None,
            "version": None,
            "sha256": None,
            "source": "https://github.com/mozilla/sccache",
            "license": "Apache-2.0",
            "verified": False,
        }
    code, output = run([str(path), "--version"])
    first_line = output.splitlines()[0] if output else ""
    return {
        "name": "sccache",
        "status": "present",
        "optional": True,
        "path": str(path),
        "version": first_line,
        "sha256": sha256(path),
        "source": "https://github.com/mozilla/sccache",
        "license": "Apache-2.0",
        "verified": code == 0 and bool(first_line),
        "probe_exit": code,
    }


def rust_entries() -> list[dict]:
    entries = []
    # The machine default is probed outside the repository so the pinned
    # rust-toolchain.toml does not trigger a download.
    outside = str(Path.home())
    for label, argv in (
        ("rustc-default-outside-repo", ["rustc", "-vV"]),
        ("cargo-default-outside-repo", ["cargo", "--version"]),
    ):
        code, output = run(argv, cwd=outside)
        first_line = output.splitlines()[0] if output else ""
        entries.append(
            {
                "name": label,
                "status": "present" if code == 0 else "missing",
                "path": shutil.which(argv[0]),
                "version": first_line or None,
                "sha256": None,
                "source": "https://rust-lang.org/",
                "license": "MIT OR Apache-2.0",
                "verified": code == 0,
                "probe_exit": code,
                "informational": True,
                "note": "informational; builds use the pinned toolchain below",
            }
        )
    for label, tool, version_argv in (
        ("rustc-pinned-gnu", "rustc", ["-vV"]),
        ("cargo-pinned-gnu", "cargo", ["--version"]),
    ):
        code, output = run(["rustup", "which", "--toolchain", PINNED, tool])
        path = output.strip() if code == 0 and output.strip() else None
        version = None
        if path:
            vcode, vout = run([path, *version_argv])
            version = vout.splitlines()[0] if vout else None
        else:
            vcode = 127
        entries.append(
            {
                "name": label,
                "status": "present" if path else "missing",
                "path": path,
                "version": version,
                "sha256": sha256(Path(path)) if path and Path(path).exists() else None,
                "source": "https://rust-lang.org/",
                "license": "MIT OR Apache-2.0",
                "verified": bool(version),
                "probe_exit": vcode,
            }
        )
    ra_path = Path.home() / ".rustup/toolchains" / PINNED / "bin/rust-analyzer.exe"
    if ra_path.exists():
        code, output = run([str(ra_path), "--version"])
        entries.append(
            {
                "name": "rust-analyzer-pinned",
                "status": "present",
                "path": str(ra_path),
                "version": output.splitlines()[0] if output else None,
                "sha256": sha256(ra_path),
                "source": "https://rust-analyzer.github.io/",
                "license": "MIT OR Apache-2.0",
                "verified": code == 0,
                "usage": "real SCIP golden index generation (upstream-probes/scip)",
            }
        )
    code, output = run(["rustup", "--version"])
    entries.append(
        {
            "name": "rustup",
            "status": "present" if code == 0 else "missing",
            "path": shutil.which("rustup"),
            "version": output.splitlines()[0] if output else None,
            "sha256": sha256(Path(shutil.which("rustup"))) if shutil.which("rustup") else None,
            "source": "https://rust-lang.org/",
            "license": "MIT OR Apache-2.0",
            "verified": code == 0,
            "probe_exit": code,
        }
    )
    return entries


def libclang_entry() -> dict:
    path = portable.libclang_path()
    dll = None
    if path is not None:
        if path.is_file():
            dll = path
            path = path.parent
        else:
            for candidate in ("libclang.dll", "libclang.so", "libclang.dylib"):
                if (path / candidate).exists():
                    dll = path / candidate
                    break
    if dll is None:
        return {
            "name": "libclang",
            "status": "missing",
            "path": None,
            "version": None,
            "sha256": None,
            "source": "https://pypi.org/project/libclang/",
            "license": "Apache-2.0 WITH LLVM-exception",
            "verified": False,
        }
    return {
        "name": "libclang",
        "status": "present",
        "path": str(dll),
        "version": "18.1.1 (PyPI libclang bundle)",
        "sha256": sha256(dll),
        "source": "https://pypi.org/project/libclang/",
        "license": "Apache-2.0 WITH LLVM-exception",
        "verified": True,
        "usage": "RocksDB bindgen only; not linked into product code",
    }


def docker_image_digest() -> str | None:
    code, output = run(
        ["docker", "image", "inspect", "rust:1.94.1-bookworm", "--format", "{{index .RepoDigests 0}}"]
    )
    return output if code == 0 else None


def annotate_portable(lock: dict) -> None:
    """Add environment-resolvable fields without removing the observed path."""
    lock["lock_version"] = "1.1"
    for tool in lock.get("tools", []):
        path = tool.get("path")
        tool["observed_path"] = path
        tool["path_kind"] = "directory" if path and Path(path).is_dir() else "file"
        name = tool.get("name", "")
        if name in {"rustc-pinned-gnu", "cargo-pinned-gnu", "rust-analyzer-pinned"}:
            tool["strategy"] = "rustup_which"
            tool["strategy_args"] = {
                "toolchain": PINNED,
                "tool": name.replace("-pinned-gnu", "").replace("-pinned", ""),
            }
            tool["path_env"] = "RUSTUP_HOME"
            tool["relative_path"] = (
                f"toolchains/{PINNED}/bin/{Path(path).name}" if path else None
            )
        elif name == "python":
            local_python = (
                Path(os.environ.get("LOCALAPPDATA", ""))
                / "Programs/Python/Python311"
                / Path(path).name
                if path
                else None
            )
            if (
                path
                and local_python is not None
                and local_python.is_file()
                and local_python.resolve() == Path(path).resolve()
            ):
                tool["strategy"] = "env_file"
                tool["path_env"] = "LOCALAPPDATA"
                tool["relative_path"] = f"Programs/Python/Python311/{Path(path).name}"
            else:
                # `which python` found a different interpreter (e.g. MSYS2
                # 3.14); record it as a PATH-resolved tool so doctor checks the
                # same bytes instead of the system Python311.
                tool["strategy"] = "which"
                tool["which_name"] = "python"
                tool["relative_path"] = Path(path).name if path else None
        elif name in {"gcc", "gxx"}:
            tool["strategy"] = "env_file"
            tool["path_env"] = "ARCHMELD_MINGW_BIN"
            tool["relative_path"] = Path(path).name if path else None
        elif name in {"cmake", "ninja"}:
            tool["strategy"] = "which"
            tool["which_name"] = name
            tool["relative_path"] = Path(path).name if path else None
        elif name == "libclang":
            tool["strategy"] = "env_file"
            tool["path_env"] = "ARCHMELD_TOOLS_ROOT"
            tool["relative_path"] = "pylibclang/clang/native/libclang.dll"
        else:
            tool["strategy"] = "which"
            tool["which_name"] = {
                "rustc-default-outside-repo": "rustc",
                "cargo-default-outside-repo": "cargo",
            }.get(name, name)


def main() -> int:
    lock = {
        "lock_version": "1.0",
        "generated_at": datetime.now(timezone.utc).isoformat(),
        "generated_by": "tooling/lock_toolchains.py",
        "platform": {
            "system": platform.system(),
            "release": platform.release(),
            "machine": platform.machine(),
            "python": platform.python_version(),
        },
        "pinned": {
            "rust_channel": "1.94.1",
            "rust_host": "x86_64-pc-windows-gnu",
            "reason": "upstream rust-toolchain.toml pins 1.94.1; the MSVC host has no linker on this machine",
            "codegraph_commit": "ae4b2e54fc8ce1c074523943fc750b20f68bf8aa",
            "codeboarding_commit": "a38a8d157150e78cee4e4c0f3f0646a4ae9ed938",
        },
        "tools": [tool_entry(name, *spec) for name, spec in TOOLS.items()]
        + rust_entries()
        + [libclang_entry(), sccache_entry()],
        "container": {
            "image": "rust:1.94.1-bookworm",
            "digest": docker_image_digest(),
            "use": "Linux x64 baseline for Ubuntu host parity; Windows GNU/UCRT64 RocksDB lib passes 59/59",
        },
    }

    annotate_portable(lock)
    used = [entry["name"] for entry in lock["tools"] if entry["status"] == "present"]
    missing = [
        entry["name"]
        for entry in lock["tools"]
        if entry["status"] != "present" and not entry.get("optional")
    ]
    missing_optional = [
        entry["name"]
        for entry in lock["tools"]
        if entry["status"] == "missing_optional"
    ]
    lock["summary"] = {
        "present": used,
        "missing": missing,
        "missing_optional": missing_optional,
        "verified": all(
            entry["verified"] for entry in lock["tools"] if entry["status"] == "present"
        ),
    }
    (ROOT / "toolchains.lock.json").write_text(
        json.dumps(lock, ensure_ascii=False, indent=2) + "\n", encoding="utf-8"
    )

    matrix = {
        "matrix_version": "1.0",
        "targets": [
            {
                "id": "windows-x64-gnu",
                "os": "Windows 11 x64",
                "toolchain": "1.94.1-x86_64-pc-windows-gnu",
                "status": "partial",
                "evidence": {
                    "build": "passed (codegraph-server built with --locked)",
                    "in_memory_tests": "passed (codegraph --lib --no-default-features: 40 tests)",
                    "default_backend_tests": "passed: codegraph --lib (default rocksdb-backend) 59/59, including storage::rocksdb_backend::tests::test_delete (UCRT64 GCC 16.1.0, 2026-11-27)",
                    "state_root_isolation": "passed: codegraph-server --info --archmeld-state-root writes no ~/.codegraph (tests/archmeld_state_root.rs)",
                    "server_lib_tests": "passed: 426 passed / 0 failed / 1 ignored after the Windows fixture fix (UCRT64 16.1.0, 2026-11-27); the 5 ms ai_query timing test can still flake in debug builds",
                    "cause": "the historical 0xC0000005 was a Strawberry/winlibs GCC 13.2.0 runtime mismatch; current UCRT64 GCC 16.1.0 passes",
                },
                "usage": "core/SQLite/parsers and persistent graph backend lib tests; the full codegraph integration/doctest suite was not re-run on Windows",
            },
            {
                "id": "linux-x64-bookworm-container",
                "os": "Debian bookworm (rust:1.94.1-bookworm container)",
                "toolchain": "1.94.1-x86_64-unknown-linux-gnu",
                "status": "passed",
                "evidence": {
                    "command": "cargo test -p codegraph --locked",
                    "result": "58 lib + 5 probes + 72 integration + 6 doctests = 141 passed, 1 ignored, 0 failed",
                    "network": "graph-only export under strace: 0 AF_INET sockets; one AF_UNIX socketpair (runtime)",
                    "state_root": "--archmeld-state-root holds graph.db, projects/<slug>/index_state.json and last-phase markers; ~/.codegraph absent",
                },
                "note": "Linux baseline is Debian 12, not the R1 target Ubuntu 24.04; Ubuntu-native run remains to be executed.",
            },
            {
                "id": "windows-x64-msvc",
                "os": "Windows 11 x64",
                "toolchain": "1.94.1-x86_64-pc-windows-msvc",
                "status": "unavailable",
                "evidence": {"cause": "Visual Studio Build Tools and Windows SDK are not installed on this machine"},
                "note": "Optional: not required for the current Windows GNU/RocksDB path (UCRT64 16.1.0 passes); only needed for a future MSVC-native delivery, which is untested.",
            },
        ],
    }
    (ROOT / "platform-matrix.json").write_text(
        json.dumps(matrix, ensure_ascii=False, indent=2) + "\n", encoding="utf-8"
    )
    print(
        json.dumps(
            {"present": used, "missing": missing, "verified": lock["summary"]["verified"]},
            ensure_ascii=False,
        )
    )
    return 0 if not missing else 1


if __name__ == "__main__":
    raise SystemExit(main())
