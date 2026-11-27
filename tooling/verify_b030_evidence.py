#!/usr/bin/env python3
"""Fail closed on a B030/AT067 evidence bundle.

The four language-matrix tests write separate JSON files. This verifier checks
that the matrix actually consumed fresh outputs, that every language group has
the required build/runtime/linkage/medium-project/controlled-change result, and
that no stale Strawberry/winlibs compiler path leaked into the evidence.

Usage:
  py -3.11 tooling/verify_b030_evidence.py --since <unix-seconds>
  py -3.11 tooling/verify_b030_evidence.py --expect-head 3116eb24
"""
from __future__ import annotations

import argparse
import json
import sys
from pathlib import Path
from typing import Any

ROOT = Path(__file__).resolve().parents[1]
RELEASE = ROOT / "docs" / "release-evidence"
FILES = {
    "runtime-linkage": RELEASE / "B030-runtime-linkage.json",
    "medium-projects": RELEASE / "B030-medium-projects.json",
    "rename-apply": RELEASE / "B030-rename-apply.json",
    "language-matrix": RELEASE / "B030-language-matrix.json",
}
STALE_MARKERS = ("Strawberry", "winlibs", "GCC 13.2.0")


def load(path: Path) -> tuple[dict[str, Any] | None, list[str]]:
    if not path.is_file():
        return None, [f"{path.relative_to(ROOT)} is missing"]
    try:
        value = json.loads(path.read_text(encoding="utf-8"))
    except (OSError, json.JSONDecodeError) as exc:
        return None, [f"{path.relative_to(ROOT)} is not valid JSON: {exc}"]
    if not isinstance(value, dict):
        return None, [f"{path.relative_to(ROOT)} is not a JSON object"]
    return value, []


def verified(value: Any) -> bool:
    return isinstance(value, dict) and value.get("state") == "verified"


def run_ok(value: Any) -> bool:
    return isinstance(value, dict) and value.get("exit_code") == 0


def check_group(name: str, group: Any, errors: list[str]) -> None:
    if not isinstance(group, dict):
        errors.append(f"group {name}: not an object")
        return
    if name == "javascript_typescript":
        for part in ("typescript", "javascript"):
            part_value = group.get(part)
            if not isinstance(part_value, dict):
                errors.append(f"group {name}.{part}: missing")
                continue
            if not run_ok(part_value.get("build")):
                errors.append(f"group {name}.{part}: build not ok")
            if not run_ok(part_value.get("runtime")):
                errors.append(f"group {name}.{part}: runtime not ok")
        linkage = group.get("runtime_linkage")
        change = group.get("controlled_change")
        for part in ("javascript", "typescript"):
            if not verified((linkage or {}).get(part) if isinstance(linkage, dict) else None):
                errors.append(f"group {name}.{part}: runtime linkage not verified")
            if not verified((change or {}).get(part) if isinstance(change, dict) else None):
                errors.append(f"group {name}.{part}: controlled change not verified")
    else:
        if not run_ok(group.get("build")):
            errors.append(f"group {name}: build not ok")
        if not run_ok(group.get("runtime")):
            errors.append(f"group {name}: runtime not ok")
        if not verified(group.get("runtime_linkage")):
            errors.append(f"group {name}: runtime linkage not verified")
        if not verified(group.get("controlled_change")):
            errors.append(f"group {name}: controlled change not verified")
    medium = group.get("medium_project")
    if not verified(medium):
        errors.append(f"group {name}: medium project not verified")
    elif medium.get("build_exit_code") != 0:
        errors.append(f"group {name}: medium project build failed")


def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("--since", type=int, default=None, help="run start, unix seconds")
    parser.add_argument("--expect-head", default=None, help="expected short product HEAD")
    args = parser.parse_args()

    errors: list[str] = []
    evidence: dict[str, dict[str, Any]] = {}
    for name, path in FILES.items():
        value, problems = load(path)
        errors.extend(problems)
        if value is not None:
            evidence[name] = value

    for name, value in evidence.items():
        text = json.dumps(value, ensure_ascii=False)
        for marker in STALE_MARKERS:
            if marker in text:
                errors.append(f"{name}: stale compiler marker {marker!r}")
        recorded = value.get("recorded_at_unix")
        if not isinstance(recorded, int):
            errors.append(f"{name}: recorded_at_unix missing")
        elif args.since is not None and recorded < args.since:
            errors.append(f"{name}: recorded_at_unix {recorded} < run start {args.since}")
        if args.expect_head is not None and value.get("product_head") != args.expect_head:
            errors.append(
                f"{name}: product_head {value.get('product_head')!r} != {args.expect_head!r}"
            )

    matrix = evidence.get("language-matrix")
    if matrix is not None:
        at067 = matrix.get("AT067")
        if not isinstance(at067, dict):
            errors.append("language-matrix: AT067 missing")
        else:
            if at067.get("status") != "passed":
                errors.append(f"language-matrix: AT067 status {at067.get('status')!r}")
            if at067.get("r1_naming_allowed") is not True:
                errors.append("language-matrix: r1_naming_allowed is not true")
        if matrix.get("blockers") != []:
            errors.append(f"language-matrix: blockers {matrix.get('blockers')!r}")
        for key in ("runtime_linkage_evidence", "medium_project_evidence", "rename_evidence"):
            block = matrix.get(key)
            if not isinstance(block, dict) or block.get("loaded") is not True:
                errors.append(f"language-matrix: {key} not loaded")
        groups = matrix.get("groups")
        if not isinstance(groups, dict) or not groups:
            errors.append("language-matrix: groups missing")
        else:
            for name, group in groups.items():
                check_group(name, group, errors)
        gpp = (matrix.get("tool_versions") or {}).get("g++")
        if not isinstance(gpp, dict):
            errors.append("language-matrix: tool_versions.g++ missing")
        else:
            first_line = str(gpp.get("first_line", ""))
            argv = " ".join(str(item) for item in gpp.get("argv", []))
            if "16.1.0" not in first_line:
                errors.append(f"language-matrix: g++ version is not 16.1.0: {first_line!r}")
            if "ucrt64" not in argv.lower():
                errors.append(f"language-matrix: g++ is not the UCRT64 compiler: {argv!r}")

    if errors:
        for error in errors:
            print(f"ERROR: {error}", file=sys.stderr)
        print(json.dumps({"status": "failed", "errors": errors}, ensure_ascii=False))
        return 1
    print(json.dumps({"status": "passed", "files": list(FILES)}, ensure_ascii=False))
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
