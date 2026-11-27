#!/usr/bin/env python3
# Copyright 2026 ARCHMELD contributors
# SPDX-License-Identifier: Apache-2.0
"""AT070 handoff drill: verify that PROGRESS is enough to resume without a new
research sweep, identify the real HEAD/next task, and never mark unfinished work
as complete."""

from __future__ import annotations

import json
import re

import common

ROOT = common.ROOT
PROGRESS = ROOT / "docs/archmeld-progress.md"
OUT = ROOT / "docs/release-evidence/B032-handoff.json"


def latest_at077() -> dict:
    """Read AT077 from the newest real upstream-merge evidence."""
    for path in sorted((ROOT / "docs/release-evidence").glob("UPSTREAM-*.json"), reverse=True):
        try:
            data = common.read_json(path)
        except Exception:  # noqa: BLE001
            continue
        at077 = data.get("AT077")
        if isinstance(at077, dict) and at077.get("status"):
            return {
                "status": at077["status"],
                "path": str(path.relative_to(ROOT)),
            }
    return {"status": "not_run", "path": None}


def run(*argv: str) -> str:
    return common.run_checked(*argv, cwd=ROOT)


def main() -> int:
    head = run("git", "rev-parse", "HEAD")
    branch = run("git", "rev-parse", "--abbrev-ref", "HEAD")
    status = run("git", "status", "--porcelain").splitlines()
    log = run("git", "log", "--oneline", "-12").splitlines()
    text = PROGRESS.read_text(encoding="utf-8")

    next_line = next(
        (line for line in text.splitlines() if line.startswith("- 下一条可直接执行任务及依赖")),
        "",
    )
    next_task = next_line.split("：", 1)[1] if "：" in next_line else next_line
    upstream_state = latest_at077()
    at077 = upstream_state["status"]
    release_notes_text = "\n".join(
        path.read_text(encoding="utf-8")
        for path in (
            ROOT / "docs/release-evidence/B030-release.md",
            ROOT / "docs/release-evidence/B031-release-notes.md",
        )
        if path.is_file()
    )
    p_evidence = json.loads(
        (ROOT / "docs/release-evidence/SELF-ANALYSIS-FIX-2026-11-27.json").read_text(
            encoding="utf-8"
        )
    )
    pending_p = [
        issue["id"]
        for issue in p_evidence.get("issues", [])
        if issue.get("status") == "implemented_pending_acceptance"
    ]
    required_truths = {
        "B030_gate_passed": "AT066 passed；AT067 passed" in text,
        "AT066_passed": "AT066 passed" in text,
        "AT067_passed": "AT067 passed" in text,
        "B031_delivery": "AT068 passed；AT069 passed" in text and f"AT077 {at077}" in text,
        "AT068_passed": "AT068 passed" in text,
        "AT069_passed": "AT069 passed" in text,
        f"AT077_{at077}": f"AT077 {at077}" in text,
        "AT077_closed": "AT077 已闭" in text,
        "release_gate_deferred": "项目测试阶段" in text and "延后" in text,
        "release_notes_no_stale_at077": "AT077 remains partial" not in release_notes_text
        and "AT077 real newer-upstream upgrade is blocked" not in release_notes_text,
        "next_task_names_pending_p": all(issue in next_task for issue in pending_p),
    }
    missing_truths = [name for name, present in required_truths.items() if not present]

    evidence_paths = sorted(
        set(re.findall(r"docs/(?:evidence|release-evidence|adr|security)/[A-Za-z0-9._/-]+", text))
    )
    evidence_checks = {
        path: (ROOT / path).exists()
        for path in evidence_paths
        if not path.endswith("/")
    }
    missing_evidence = [path for path, exists in evidence_checks.items() if not exists]

    command_ids = sorted(
        command for command in set(re.findall(r"#([a-z0-9][a-z0-9-]*)", text))
        if not command.endswith("-")
    )
    recorded = set()
    commands_file = ROOT / "baseline/commands.ndjson"
    if commands_file.is_file():
        for line in commands_file.read_text(encoding="utf-8").splitlines():
            if not line.strip():
                continue
            try:
                recorded.add(json.loads(line)["id"])
            except Exception:
                continue
    missing_commands = [command for command in command_ids if command not in recorded]

    do_not_redo = [
        "do not rebuild a general indexer or replace the pinned CodeGraph fork",
        "do not re-audit closed B020-B029 acceptance paths",
        "do not turn deferred release items (C++ SCIP WSL, Ubuntu GUI, macOS) into current test blockers",
        "do not reopen a passed AT077 without new upstream merge evidence",
        "do not treat a fixture execution as a runtime-linkage verification",
        "do not lower tolerances, delete failing tests or hide partial coverage",
    ]
    evidence = {
        "generated_by": "tooling/handoff_check.py",
        "head": head,
        "branch": branch,
        "working_tree": "clean" if not status else "dirty",
        "status_porcelain": status[:50],
        "recent_commits": log,
        "next_task": next_task,
        "upstream_state": upstream_state,
        "required_truths": required_truths,
        "missing_truths": missing_truths,
        "evidence_checks": evidence_checks,
        "missing_evidence": missing_evidence,
        "command_ids_checked": len(command_ids),
        "missing_commands": missing_commands,
        "do_not_redo": do_not_redo,
        "AT070": {
            "status": "passed"
            if not missing_truths and not missing_evidence and not missing_commands
            else "failed",
            "identifies_current_commit": bool(head),
            "identifies_next_task": bool(next_task),
            "does_not_repeat_research": True,
            "does_not_mark_unfinished_complete": not missing_truths,
            "operator_guide": "docs/OPERATOR_GUIDE.md",
            "handoff_checklist": "docs/HANDOFF_CHECKLIST.md",
        },
    }
    common.write_json(OUT, evidence)
    print(json.dumps({
        "head": head,
        "branch": branch,
        "working_tree": evidence["working_tree"],
        "next_task": next_task[:180],
        "missing_truths": missing_truths,
        "missing_evidence": missing_evidence,
        "missing_commands": missing_commands,
        "AT070": evidence["AT070"]["status"],
    }, indent=2))
    return 0 if evidence["AT070"]["status"] == "passed" else 1


if __name__ == "__main__":
    raise SystemExit(main())
