#!/usr/bin/env python3
"""Run one self-analysis batch against a clean worktree.

The wrapper is invoked through baseline/run_cmd.py, which appends its command
record only after this process exits. Index, status and acceptance therefore
observe the clean tree; the record append happens afterwards, which is stated in
the evidence.
"""

from __future__ import annotations

import argparse
import hashlib
import json
import shutil
import subprocess
import sys
import time
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]


def sha256(path: Path) -> str:
    return "sha256:" + hashlib.sha256(path.read_bytes()).hexdigest()


def run(argv: list[str], cwd: Path, timeout: int = 1800) -> dict:
    started = time.time()
    proc = subprocess.run(
        argv,
        cwd=str(cwd),
        capture_output=True,
        text=True,
        encoding="utf-8",
        errors="replace",
        timeout=timeout,
    )
    return {
        "argv": argv,
        "exit_code": proc.returncode,
        "duration_s": round(time.time() - started, 3),
        "stdout": proc.stdout,
        "stderr": proc.stderr,
    }


def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("--state", required=True)
    parser.add_argument("--project", default="archmeld-self")
    parser.add_argument("--out", required=True)
    args = parser.parse_args()
    state = Path(args.state)
    out = Path(args.out)
    if state.exists():
        shutil.rmtree(state)
    out.mkdir(parents=True, exist_ok=True)
    head = subprocess.run(
        ["git", "rev-parse", "HEAD"], cwd=str(ROOT), capture_output=True, text=True
    ).stdout.strip()
    status_before = subprocess.run(
        ["git", "status", "--porcelain"], cwd=str(ROOT), capture_output=True, text=True
    ).stdout.splitlines()

    index = run(
        [
            str(ROOT / "target/release/archmeld.exe"),
            "index",
            "--workspace",
            str(ROOT),
            "--state",
            str(state),
            "--project",
            args.project,
            "--max-files",
            "5000",
            "--include-dir",
            "examples",
            "--exclude-dir",
            out.name,
            "--index-helper",
            str(ROOT / "target/release/examples/archmeld_export.exe"),
        ],
        ROOT,
    )
    if index["exit_code"] != 0:
        (out / "index.stdout.json").write_text(index["stdout"], encoding="utf-8")
        (out / "index.stderr.log").write_text(index["stderr"], encoding="utf-8")
        print(json.dumps({"stage": "index", "exit": index["exit_code"], "stderr": index["stderr"][-2000:]}))
        return 1
    index_json = json.loads(index["stdout"])

    status = run(
        [
            str(ROOT / "target/release/archmeld.exe"),
            "status",
            "--state",
            str(state),
            "--project",
            args.project,
            "--workspace",
            str(ROOT),
        ],
        ROOT,
    )
    # The evidence directory is inside the workspace. Delay writing these
    # artifacts until after `status` has observed the tree, otherwise a fresh
    # run reports its own output as added files.
    (out / "index.stdout.json").write_text(index["stdout"], encoding="utf-8")
    (out / "index.stderr.log").write_text(index["stderr"], encoding="utf-8")
    (out / "status.json").write_text(status["stdout"], encoding="utf-8")
    (out / "status.stderr.log").write_text(status["stderr"], encoding="utf-8")

    acceptance = run(
        [
            sys.executable,
            str(ROOT / "tooling/self_analysis_acceptance.py"),
            "--state",
            str(state),
            "--project",
            args.project,
            "--mcp",
            str(ROOT / "target/release/archmeld-mcp.exe"),
            "--helper",
            str(ROOT / "target/release/examples/archmeld_export.exe"),
            "--out",
            str(out / "acceptance.json"),
        ],
        ROOT,
    )
    (out / "acceptance.stderr.log").write_text(acceptance["stderr"], encoding="utf-8")
    report = json.loads((out / "acceptance.json").read_text(encoding="utf-8"))
    status_after = subprocess.run(
        ["git", "status", "--porcelain"], cwd=str(ROOT), capture_output=True, text=True
    ).stdout.splitlines()
    meta = {
        "schema_version": "1.0",
        "head": head,
        "project": args.project,
        "state": str(state),
        "snapshot_id": index_json.get("snapshot_id"),
        "run_id": index_json.get("run_id"),
        "index_mode": index_json.get("index_mode"),
        "files": index_json.get("snapshot_files"),
        "entities": (index_json.get("bridge") or {}).get("entities"),
        "relations": (index_json.get("bridge") or {}).get("relations"),
        "index_duration_s": index["duration_s"],
        "status_duration_s": status["duration_s"],
        "acceptance_duration_s": acceptance["duration_s"],
        "acceptance_passed": report.get("passed"),
        "acceptance_failed": report.get("failed"),
        "status_before": status_before,
        "status_after": status_after,
        "record_note": "run_cmd appends its own record after this process, so status_after is dirty by that record; index/status/acceptance ran on the clean tree",
        "archmeld_sha256": sha256(ROOT / "target/release/archmeld.exe"),
        "mcp_sha256": sha256(ROOT / "target/release/archmeld-mcp.exe"),
        "helper_sha256": sha256(ROOT / "target/release/examples/archmeld_export.exe"),
        "acceptance": report,
    }
    (out / "RUN.json").write_text(json.dumps(meta, indent=2, ensure_ascii=False), encoding="utf-8")
    print(json.dumps({k: meta[k] for k in ("head", "snapshot_id", "files", "entities", "relations", "acceptance_passed", "acceptance_failed", "index_mode")}, indent=2))
    return 0 if report.get("failed") == 0 else 1


if __name__ == "__main__":
    raise SystemExit(main())
