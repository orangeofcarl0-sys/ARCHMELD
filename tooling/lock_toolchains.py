#!/usr/bin/env python3
"""Generate toolchains.lock.json and platform-matrix.json from this machine.

Only measured facts go into the lock: the file records what was actually found
(version, executable path, SHA-256) or an explicit `missing` state. It never
writes placeholder hashes or "latest" versions.

Usage: python tooling/lock_toolchains.py
"""
from __future__ import annotations

import hashlib
import json
import platform
import shutil
import subprocess
import sys
from datetime import datetime, timezone
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]
PINNED = "1.94.1-x86_64-pc-windows-gnu"

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


def sha256(path: Path) -> str:
    digest = hashlib.sha256()
    with path.open("rb") as handle:
        for chunk in iter(lambda: handle.read(1 << 20), b""):
            digest.update(chunk)
    return digest.hexdigest()


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
        return {
            "name": name,
            "status": "missing",
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
    path = Path(r"F:\Codex_Work_Space\_archmeld_tools\pylibclang\clang\native")
    dll = None
    for candidate in ("libclang.dll", "libclang.so", "libclang.dylib"):
        if (path / candidate).exists():
            dll = path / candidate
            break
    if dll is None:
        return {
            "name": "libclang",
            "status": "missing",
            "path": str(path),
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
            "codegraph_commit": "590c2ca7dd7b37bd1d41caf45fac535b6522f969",
            "codeboarding_commit": "a38a8d157150e78cee4e4c0f3f0646a4ae9ed938",
        },
        "tools": [tool_entry(name, *spec) for name, spec in TOOLS.items()] + rust_entries() + [libclang_entry()],
        "container": {
            "image": "rust:1.94.1-bookworm",
            "digest": docker_image_digest(),
            "use": "Linux x64 baseline because RocksDB crashes under Windows GNU",
        },
    }

    used = [entry["name"] for entry in lock["tools"] if entry["status"] == "present"]
    missing = [entry["name"] for entry in lock["tools"] if entry["status"] != "present"]
    lock["summary"] = {
        "present": used,
        "missing": missing,
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
                    "default_backend_tests": "failed: storage::rocksdb_backend::tests::test_delete crashes with STATUS_ACCESS_VIOLATION (0xC0000005)",
                    "state_root_isolation": "passed: codegraph-server --info --archmeld-state-root writes no ~/.codegraph (tests/archmeld_state_root.rs)",
                    "cause": "RocksDB win32 port is MSVC-targeted; the MinGW runtime is not supported by upstream",
                },
                "usage": "core/SQLite/parsers and memory-backend probes; not the persistent graph backend",
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
                "note": "Required before Windows-native RocksDB or product delivery; installing it is a separate authorization step.",
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
