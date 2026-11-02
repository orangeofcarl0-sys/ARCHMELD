#!/usr/bin/env python3
# Copyright 2026 ARCHMELD contributors
# SPDX-License-Identifier: Apache-2.0
"""Synthetic upstream-interface compatibility drill.

A temporary standalone copy of archmeld-core is compiled once, then the
CodeGraph bridge interface is renamed and compiled again. The adapter contract
test target must detect the difference. This is explicitly a synthetic drill,
not a newer upstream checkout; the pinned upstream commit and NOTICE are
recorded unchanged.
"""

from __future__ import annotations

import hashlib
import json
import os
import shutil
import subprocess
import tempfile
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]
OUT = ROOT / "docs/release-evidence/B031-upstream-delta.json"
UPSTREAM = "590c2ca7dd7b37bd1d41caf45fac535b6522f969"
TEMP_BASE = Path("<workspace>/_archmeld_temp")


def sha256(path: Path) -> str:
    return hashlib.sha256(path.read_bytes()).hexdigest()


def run(argv: list[str], cwd: Path, env: dict[str, str]) -> dict:
    result = subprocess.run(
        argv,
        cwd=str(cwd),
        env=env,
        capture_output=True,
        text=True,
        errors="replace",
        timeout=600,
    )
    return {
        "argv": argv,
        "cwd": str(cwd),
        "exit_code": result.returncode,
        "stdout": result.stdout[-4096:],
        "stderr": result.stderr[-4096:],
    }


def main() -> int:
    temp = Path(tempfile.mkdtemp(prefix="b031-upstream-", dir=str(TEMP_BASE)))
    target = temp / "target"
    source = ROOT / "crates/archmeld-core"
    copy = temp / "archmeld-core"
    shutil.copytree(source, copy)
    manifest = copy / "Cargo.toml"
    text = manifest.read_text(encoding="utf-8")
    for key, value in {
        "edition.workspace = true": 'edition = "2021"',
        "license.workspace = true": 'license = "Apache-2.0"',
        "repository.workspace = true": 'repository = "https://github.com/codegraph-ai/codegraph"',
        "authors.workspace = true": 'authors = ["ARCHMELD contributors"]',
        "serde.workspace = true": 'serde = { version = "1.0", features = ["derive"] }',
        "serde_json.workspace = true": 'serde_json = "1.0"',
        "thiserror.workspace = true": 'thiserror = "1.0"',
    }.items():
        text = text.replace(key, value)
    manifest.write_text(text, encoding="utf-8")
    env = dict(os.environ)
    env["CARGO_TARGET_DIR"] = str(target)
    env.setdefault("RUSTUP_TOOLCHAIN", "1.94.1-x86_64-pc-windows-gnu")
    env["PATH"] = "D:/Strawberry/c/bin;" + env.get("PATH", "")
    baseline = run(["cargo", "check", "--tests", "--offline"], copy, env)
    bridge = copy / "src/bridge.rs"
    original = bridge.read_text(encoding="utf-8")
    mutated = original.replace("max_entities_per_batch", "max_batch_entities")
    if mutated == original:
        raise SystemExit("drill target field was not found")
    bridge.write_text(mutated, encoding="utf-8")
    detected = run(["cargo", "check", "--tests", "--offline"], copy, env)
    bridge.write_text(original, encoding="utf-8")
    restored = run(["cargo", "check", "--tests", "--offline"], copy, env)
    notice = (ROOT / "NOTICE").read_text(encoding="utf-8")
    upstream_patches = (ROOT / "UPSTREAM_PATCHES.md").read_text(encoding="utf-8")
    toolchains = json.loads((ROOT / "toolchains.lock.json").read_text(encoding="utf-8"))
    evidence = {
        "generated_by": "tooling/upstream_delta_drill.py",
        "upstream_commit": UPSTREAM,
        "pinned_in_upstream_patches": UPSTREAM in upstream_patches,
        "notice_sha256": sha256(ROOT / "NOTICE"),
        "notice_preserves_codegraph_attribution": "CodeGraph" in notice,
        "toolchain_lock_sha256": sha256(ROOT / "toolchains.lock.json"),
        "auto_follow_latest": False,
        "drill_kind": "synthetic interface rename in the CodeGraph bridge contract",
        "baseline_check": baseline,
        "mutated_check": detected,
        "restored_check": restored,
        "detected": baseline["exit_code"] == 0 and detected["exit_code"] != 0 and restored["exit_code"] == 0,
        "AT077": {
            "status": "partial",
            "reason": "adapter contract tests detect a synthetic interface rename and NOTICE/pin are preserved; no newer upstream checkout was available for a real upgrade",
            "detected_difference": detected["exit_code"] != 0,
            "silent_version_switch": False,
        },
    }
    OUT.write_text(json.dumps(evidence, ensure_ascii=False, indent=2) + "\n", encoding="utf-8")
    print(json.dumps({
        "baseline": baseline["exit_code"],
        "mutated": detected["exit_code"],
        "restored": restored["exit_code"],
        "detected": evidence["detected"],
        "AT077": evidence["AT077"]["status"],
    }, indent=2))
    shutil.rmtree(temp, ignore_errors=True)
    return 0 if evidence["detected"] else 1


if __name__ == "__main__":
    raise SystemExit(main())
