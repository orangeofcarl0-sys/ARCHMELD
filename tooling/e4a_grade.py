#!/usr/bin/env python3
"""Grade one E4a run by behavior and by map consultation, not by claims."""
from __future__ import annotations

import argparse
import hashlib
import json
import subprocess
import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]

BEHAVIOR = {
    "nonlexical": """
import sys
sys.path.insert(0, 'src')
import pricing, invoice, analytics
assert abs(pricing.apply_discount(100) - 85.0) < 1e-9, pricing.apply_discount(100)
assert abs(invoice.invoice_total(100) - 85.0) < 1e-9, invoice.invoice_total(100)
assert analytics.check_total() is True
""",
    "lexical": """
import sys
sys.path.insert(0, 'src')
import pricing, invoice, analytics
assert abs(pricing.apply_discount(100) - 85.0) < 1e-9, pricing.apply_discount(100)
assert abs(invoice.invoice_total(100) - 85.0) < 1e-9, invoice.invoice_total(100)
assert analytics.check_price() is True
""",
}


def load_jsonl(path: Path) -> list[dict]:
    if not path.is_file():
        return []
    return [json.loads(line) for line in path.read_text(encoding="utf-8").splitlines() if line.strip()]


def final_answer(result: dict | None) -> dict | None:
    if not result:
        return None
    candidate = (
        result.get("structured_output")
        or result.get("result")
        or result.get("text")
    )
    if isinstance(candidate, dict):
        return candidate
    if isinstance(candidate, str):
        text = candidate.strip()
        if text.startswith("```"):
            text = text.strip("`")
            if text.startswith("json"):
                text = text[4:]
        try:
            return json.loads(text)
        except json.JSONDecodeError:
            return None
    return None


def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("--run", required=True)
    parser.add_argument("--fixture", choices=sorted(BEHAVIOR), required=True)
    args = parser.parse_args()
    run_dir = Path(args.run)
    run = json.loads((run_dir / "run.json").read_text(encoding="utf-8"))
    repo = run_dir / "repo"
    behavior = subprocess.run(
        [sys.executable, "-I", "-c", BEHAVIOR[args.fixture]],
        cwd=str(repo),
        text=True,
        encoding="utf-8",
        errors="replace",
        capture_output=True,
        timeout=60,
    )
    calls = load_jsonl(run_dir / "calls.jsonl")
    edits = load_jsonl(run_dir / "edits.jsonl")
    audit = load_jsonl(run_dir / "audit.jsonl")
    inspect = [
        call
        for call in calls
        if call.get("tool") == "archmeld_inspect"
        and call.get("args", {}).get("kind") == "map_entry"
    ]
    briefs = [
        call
        for call in calls
        if call.get("tool") == "archmeld_brief"
        and call.get("args", {}).get("recipe") in ("change", None)
    ]
    first_edit = edits[0]["seq"] if edits else None
    map_before_edit = None
    if first_edit is not None and inspect:
        map_before_edit = inspect[0]["seq"] < first_edit
    answer = final_answer(run.get("result"))
    declared = []
    if answer and isinstance(answer.get("constraints_applied"), list):
        declared = [
            item.get("file", "")
            for item in answer["constraints_applied"]
            if isinstance(item, dict)
        ]
    audit_errors = [
        item for item in audit if item.get("outcome") not in (None, "ok", "success")
    ]
    changed = run.get("changed_files", [])
    result = {
        "schema_version": "1.0",
        "run_id": run.get("run_id"),
        "fixture": args.fixture,
        "arm": run.get("arm"),
        "behavior_ok": behavior.returncode == 0,
        "behavior_stderr": behavior.stderr[-1000:],
        "files_changed": changed,
        "pricing_changed": "src/pricing.py" in changed,
        "invoice_changed": "src/invoice.py" in changed,
        "analytics_changed": "src/analytics.py" in changed,
        "map_present": run.get("map_checked"),
        "map_consulted": bool(inspect or briefs),
        "map_entry_inspect": len(inspect),
        "brief_change": len(briefs),
        "map_before_first_edit": map_before_edit,
        "constraints_declared": declared,
        "answer_present": answer is not None,
        "answer": answer,
        "tool_errors": len(audit_errors),
        "outcome": (
            "pass"
            if behavior.returncode == 0
            and "src/invoice.py" in changed
            and "src/analytics.py" in changed
            else "fail"
        ),
    }
    (run_dir / "grade-result.json").write_text(
        json.dumps(result, ensure_ascii=False, indent=2), encoding="utf-8"
    )
    print(json.dumps(result, ensure_ascii=False, indent=2))
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
