#!/usr/bin/env python3
# Copyright 2026 ARCHMELD contributors
# SPDX-License-Identifier: Apache-2.0
"""Synthetic adapter-interface compatibility drill.

A temporary standalone copy of archmeld-core is compiled once, then a field of
the CodeGraph bridge contract is renamed and compiled again. The adapter
contract tests must detect the difference. This is explicitly synthetic: it is
not a newer upstream checkout, and it does not decide AT077. AT077 is tracked
by the real merge evidence in docs/release-evidence/UPSTREAM-*.json.

Run from the repository root:

    python tooling/upstream_delta_drill.py --out docs/release-evidence/ADAPTER-DRILL-YYYY-MM-DD.json
"""

from __future__ import annotations

import argparse
import hashlib
import json
import os
import shutil
import subprocess
import tempfile
from datetime import date
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]
DEFAULT_OUT = ROOT / "docs" / "release-evidence" / f"ADAPTER-DRILL-{date.today().isoformat()}.json"
TEMP_BASE = Path("<temp>")
TOOLCHAIN_LOCK = ROOT / "toolchains.lock.json"
REGISTRY = ROOT / "upstreams.lock.json"


def sha256(path: Path) -> str:
    return hashlib.sha256(path.read_bytes()).hexdigest()


def run(argv: list[str], cwd: Path, env: dict[str, str]) -> dict:
    result = subprocess.run(
        argv,
        cwd=str(cwd),
        env=env,
        capture_output=True,
        timeout=1200,
    )
    return {
        "argv": argv,
        "cwd": str(cwd),
        "exit_code": result.returncode,
        "stdout": result.stdout.decode("utf-8", errors="replace")[-4096:],
        "stderr": result.stderr.decode("utf-8", errors="replace")[-4096:],
        "stdout_sha256": hashlib.sha256(result.stdout).hexdigest(),
        "stderr_sha256": hashlib.sha256(result.stderr).hexdigest(),
    }


def current_pin() -> str:
    registry = json.loads(REGISTRY.read_text(encoding="utf-8"))
    for entry in registry.get("upstreams", []):
        if entry.get("id") == "codegraph":
            return entry["pin"]["commit"]
    return json.loads(TOOLCHAIN_LOCK.read_text(encoding="utf-8"))["pinned"]["codegraph_commit"]


def base_commit_in_patch_manifest() -> bool:
    text = (ROOT / "UPSTREAM_PATCHES.md").read_text(encoding="utf-8")
    pin = current_pin()
    for line in text.splitlines():
        if line.startswith("Base:"):
            return pin in line
    return False


def build_temp_workspace(temp: Path) -> Path:
    """Create a one-member workspace that resolves `.workspace = true`."""
    root_manifest = ROOT / "Cargo.toml"
    text = root_manifest.read_text(encoding="utf-8")
    # Replace the entire members array with a single member. The array is
    # multi-line; this is a dev-only drill, so a bounded regex is acceptable.
    import re

    text = re.sub(r"members\s*=\s*\[.*?\]", 'members = ["archmeld-core"]', text, count=1, flags=re.S)
    (temp / "Cargo.toml").write_text(text, encoding="utf-8")
    shutil.copy2(ROOT / "Cargo.lock", temp / "Cargo.lock")
    shutil.copytree(ROOT / "crates" / "archmeld-core", temp / "archmeld-core")
    # The bridge contract test is the drill target. Keep only that integration
    # test so unrelated test targets cannot make the baseline unbuildable.
    tests_dir = temp / "archmeld-core" / "tests"
    for child in tests_dir.iterdir():
        if child.name == "bridge_import.rs":
            continue
        if child.is_dir():
            shutil.rmtree(child)
        else:
            child.unlink()
    return temp


def main() -> int:
    parser = argparse.ArgumentParser(description="Synthetic adapter-interface drill")
    parser.add_argument("--out", type=Path, default=DEFAULT_OUT)
    parser.add_argument("--keep-temp", action="store_true")
    parser.add_argument("--dry-run", action="store_true", help="print the plan without compiling")
    args = parser.parse_args()

    TEMP_BASE.mkdir(parents=True, exist_ok=True)
    temp = Path(tempfile.mkdtemp(prefix="adapter-drill-", dir=str(TEMP_BASE)))
    target = temp / "target"
    workspace = build_temp_workspace(temp)
    pin = current_pin()

    env = dict(os.environ)
    env["CARGO_TARGET_DIR"] = str(target)
    env["RUSTUP_TOOLCHAIN"] = "1.94.1-x86_64-pc-windows-gnu"
    env["PATH"] = (
        "D:/Strawberry/c/bin;"
        + str(Path.home() / ".cargo" / "bin")
        + ";"
        + env.get("PATH", "")
    )
    cargo = shutil.which("cargo", path=env["PATH"]) or "cargo"
    env["LIBCLANG_PATH"] = "<workspace>/_archmeld_tools/pylibclang/clang/native"
    env["BINDGEN_EXTRA_CLANG_ARGS"] = (
        "-ID:/Strawberry/c/lib/gcc/x86_64-w64-mingw32/13.2.0/include "
        "-ID:/Strawberry/c/x86_64-w64-mingw32/include"
    )
    env["TMP"] = str(TEMP_BASE)
    env["TEMP"] = str(TEMP_BASE)

    bridge = workspace / "archmeld-core" / "src" / "bridge.rs"
    original = bridge.read_text(encoding="utf-8")
    if args.dry_run:
        print(json.dumps({"temp": str(temp), "bridge": str(bridge), "pin": pin}, indent=2))
        shutil.rmtree(temp, ignore_errors=True)
        return 0

    baseline = run([cargo, "check", "--tests", "--offline"], workspace, env)
    mutated_text = original.replace("max_entities_per_batch", "max_batch_entities")
    detected = False
    mutated = {"exit_code": None, "note": "baseline failed; mutation not run"}
    restored = {"exit_code": None, "note": "baseline failed; restore not run"}
    if baseline["exit_code"] == 0:
        if mutated_text == original:
            raise SystemExit("drill target field was not found")
        bridge.write_text(mutated_text, encoding="utf-8")
        mutated = run([cargo, "check", "--tests", "--offline"], workspace, env)
        bridge.write_text(original, encoding="utf-8")
        restored = run([cargo, "check", "--tests", "--offline"], workspace, env)
        detected = mutated["exit_code"] != 0 and restored["exit_code"] == 0

    notice = ROOT / "NOTICE"
    toolchains = ROOT / "toolchains.lock.json"
    evidence = {
        "schema_version": "1.0",
        "recorded_at": date.today().isoformat(),
        "scope": "synthetic adapter-interface drill for archmeld-core",
        "generated_by": "tooling/upstream_delta_drill.py",
        "upstream_commit": pin,
        "pinned_in_upstream_patches": base_commit_in_patch_manifest(),
        "notice_sha256": sha256(notice),
        "toolchain_lock_sha256": sha256(toolchains),
        "auto_follow_latest": False,
        "drill_kind": "synthetic_interface_rename",
        "acceptance": "not_applicable",
        "note": "AT077 is tracked by the real upstream merge evidence, not by this synthetic drill.",
        "baseline_check": baseline,
        "mutated_check": mutated,
        "restored_check": restored,
        "detected": baseline["exit_code"] == 0 and detected,
        "limitations": [
            "synthetic rename only; not a newer upstream checkout",
            "the drill does not prove a real merge or pin update",
        ],
    }
    args.out.parent.mkdir(parents=True, exist_ok=True)
    args.out.write_text(json.dumps(evidence, ensure_ascii=False, indent=2) + "\n", encoding="utf-8")
    print(json.dumps({
        "baseline": baseline["exit_code"],
        "mutated": mutated["exit_code"],
        "restored": restored["exit_code"],
        "detected": evidence["detected"],
        "out": str(args.out),
    }, indent=2))
    if not args.keep_temp:
        shutil.rmtree(temp, ignore_errors=True)
    return 0 if evidence["detected"] else 1


if __name__ == "__main__":
    raise SystemExit(main())
