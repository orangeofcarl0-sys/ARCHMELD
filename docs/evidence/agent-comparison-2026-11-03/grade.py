#!/usr/bin/env python3
"""Mechanical grader for the 2026-11-03 same-agent comparison.

The task has four questions and 17 checked localization fields. This script
only compares the reported fields with the pre-recorded ground truth and
validates that every evidence path/line reference exists. Conclusion support
and info-request usefulness are graded manually in the release evidence file.
"""

import hashlib
import json
import os
import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parent
REPO = Path(r"<workspace>\agent-bench-vizIZe\repo")
OUT = ROOT / "grade-result.json"

EXPECTED = {
    "Q1": {
        "file": "src/late.py",
        "line": 242,
        "symbol": "late_target",
        "symbol_line": 241,
    },
    "Q2": {"expression": "value * 2", "line": 243},
    "Q3": {"caller": "run_report", "file": "src/report.py", "line": 4},
    "Q4": {
        "file": "src/change.py",
        "line": 2,
        "symbol": "apply_discount",
        "old": "value * 0.9",
        "new": "value * 0.8",
        "caller": "checkout",
        "caller_file": "src/checkout.py",
        "caller_line": 4,
    },
}


def norm_path(value):
    return str(value).replace("\\", "/").strip().lstrip("./")


def norm_expr(value):
    text = " ".join(str(value).strip().split())
    if text.startswith("return "):
        text = text[len("return "):].strip()
    return text


def norm(field, value):
    if field in {"file", "caller_file"}:
        return norm_path(value)
    if field in {"old", "new", "expression"}:
        return norm_expr(value)
    return value


def line_count(path):
    try:
        return len(path.read_text(encoding="utf-8").splitlines())
    except OSError:
        return None


def grade_arm(name):
    data = json.loads((ROOT / f"arm-{name.lower()}.json").read_text(encoding="utf-8"))
    by_id = {item.get("id"): item for item in data.get("localizations", [])}
    detail = {}
    correct = 0
    total = 0
    tasks_all_correct = 0
    for qid, fields in EXPECTED.items():
        item = by_id.get(qid, {})
        field_results = {}
        all_ok = True
        for field, expected in fields.items():
            total += 1
            actual = item.get(field)
            ok = actual is not None and norm(field, actual) == norm(field, expected)
            field_results[field] = {
                "expected": expected,
                "actual": actual,
                "correct": ok,
            }
            correct += int(ok)
            all_ok = all_ok and ok
        tasks_all_correct += int(all_ok)
        detail[qid] = field_results

    ref_issues = []
    refs_checked = 0
    for conclusion in data.get("conclusions", []):
        for ref in conclusion.get("evidence", []):
            path = REPO / norm_path(ref.get("path", ""))
            refs_checked += 1
            if not path.is_file():
                ref_issues.append(
                    {"conclusion": conclusion.get("id"), "issue": "missing_path", "ref": ref}
                )
                continue
            count = line_count(path)
            line = ref.get("line")
            if not isinstance(line, int) or count is None or not (1 <= line <= count):
                ref_issues.append(
                    {
                        "conclusion": conclusion.get("id"),
                        "issue": "line_out_of_range",
                        "ref": ref,
                        "line_count": count,
                    }
                )

    return {
        "fields_correct": correct,
        "fields_total": total,
        "field_accuracy": round(correct / total, 6) if total else None,
        "tasks_all_fields_correct": tasks_all_correct,
        "tasks_total": len(EXPECTED),
        "info_requests": len(data.get("info_requests", [])),
        "uncertainties": len(data.get("uncertainties", [])),
        "conclusions": len(data.get("conclusions", [])),
        "evidence_refs_checked": refs_checked,
        "evidence_ref_issues": ref_issues,
        "detail": detail,
    }


def main():
    arms = {name: grade_arm(name) for name in ("A", "B")}
    result = {
        "schema_version": "1.0",
        "experiment": "same-agent with/without ARCHMELD, 2026-11-03",
        "ground_truth": EXPECTED,
        "arms": arms,
        "delta_b_minus_a": {
            "fields_correct": arms["B"]["fields_correct"] - arms["A"]["fields_correct"],
            "field_accuracy": round(
                arms["B"]["field_accuracy"] - arms["A"]["field_accuracy"], 6
            ),
            "info_requests": arms["B"]["info_requests"] - arms["A"]["info_requests"],
            "uncertainties": arms["B"]["uncertainties"] - arms["A"]["uncertainties"],
        },
        "run": {
            "cwd": os.getcwd(),
            "argv": sys.argv,
            "python": sys.version.split()[0],
            "grader_sha256": "sha256:"
            + hashlib.sha256(Path(__file__).read_bytes()).hexdigest(),
            "exit_code": 0,
        },
    }
    OUT.write_text(json.dumps(result, ensure_ascii=False, indent=2) + "\n", encoding="utf-8")
    print(json.dumps(result, ensure_ascii=False, indent=2))


if __name__ == "__main__":
    main()
