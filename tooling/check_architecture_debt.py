#!/usr/bin/env python3
# Copyright 2026 ARCHMELD contributors
# SPDX-License-Identifier: Apache-2.0
"""Measure the structural-debt markers recorded by the 2026-11-28 audit.

The check is a ratchet: every monotonic metric must stay at or below the value
in the baseline file. The baseline can only be lowered deliberately with
``--update`` after a refactor slice lands.

The production-unwrap count skips ``#[cfg(test)]`` items by tracking braces from
the attribute to the end of the following item. This is a deliberately simple
source scan, not a Rust parser; it is stable enough for a monotonic ratchet and
never substitutes for compiler or test evidence.
"""

from __future__ import annotations

import argparse
import datetime
import json
import re
import subprocess
import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]
CORE_SRC = ROOT / "crates/archmeld-core/src"
API_SRC = ROOT / "crates/archmeld-api/src"
BASELINE = ROOT / "docs/assessments/DEBT-BASELINE-2026-11-28.json"

MONOTONIC = (
    "sql_store_conn_oneline",
    "sql_store_conn_multiline",
    "sql_tx_calls",
    "sql_idempotency_inserts",
    "production_unwrap_expect",
    "short_hash_defs",
    "content_tree_digest_defs",
    "files_over_800",
    "cli_lines",
    "api_lib_lines",
)

SQL_ONELINE = re.compile(
    r"\bstore\.conn\.(prepare|execute|query|query_row|query_map|query_one)\b"
)
SQL_MULTILINE = re.compile(r"^\s*\.conn\b")
SQL_TX = re.compile(r"\btx\.(execute|query_row|query_map|prepare)\b")
SQL_IDEMPOTENCY = re.compile(r"INSERT INTO idempotency")
UNWRAP = re.compile(r"\.(unwrap|expect)\s*\(")
SHORT_HASH_DEF = re.compile(r"\bfn\s+short_hash\b")
CONTENT_TREE_DEF = re.compile(r"\bfn\s+content_tree_digest\b|let\s+content_tree_digest\b")


def rust_files(root: Path) -> list[Path]:
    return sorted(path for path in root.rglob("*.rs") if path.is_file())


def production_lines(text: str) -> list[str]:
    """Return lines outside top-level ``#[cfg(test)]`` items."""
    lines = text.splitlines()
    kept: list[str] = []
    index = 0
    while index < len(lines):
        if lines[index].strip().startswith("#[cfg(test)]"):
            depth = 0
            opened = False
            index += 1
            while index < len(lines):
                depth += lines[index].count("{") - lines[index].count("}")
                if "{" in lines[index]:
                    opened = True
                if opened and depth <= 0:
                    index += 1
                    break
                index += 1
            continue
        kept.append(lines[index])
        index += 1
    return kept


def relative(path: Path) -> str:
    return path.relative_to(ROOT).as_posix()


def is_store(path: Path) -> bool:
    return "evidence/store/" in relative(path)


def measure() -> dict:
    metrics = {
        "sql_store_conn_oneline": 0,
        "sql_store_conn_multiline": 0,
        "sql_tx_calls": 0,
        "sql_idempotency_inserts": 0,
        "production_unwrap_expect": 0,
        "short_hash_defs": 0,
        "content_tree_digest_defs": 0,
        "files_over_800": 0,
        "cli_lines": 0,
        "api_lib_lines": 0,
        "core_api_files": 0,
        "core_api_lines": 0,
    }
    per_file: dict[str, int] = {}
    for path in rust_files(CORE_SRC) + rust_files(API_SRC):
        text = path.read_text(encoding="utf-8", errors="replace")
        lines = text.splitlines()
        rel = relative(path)
        metrics["core_api_files"] += 1
        metrics["core_api_lines"] += len(lines)
        per_file[rel] = len(lines)
        if len(lines) >= 800:
            metrics["files_over_800"] += 1
        if rel.endswith("crates/archmeld-api/src/cli.rs"):
            metrics["cli_lines"] = len(lines)
        if rel.endswith("crates/archmeld-api/src/lib.rs"):
            metrics["api_lib_lines"] = len(lines)
        if is_store(path):
            continue
        for line in lines:
            if SQL_ONELINE.search(line):
                metrics["sql_store_conn_oneline"] += 1
            if SQL_MULTILINE.search(line):
                metrics["sql_store_conn_multiline"] += 1
            if SQL_TX.search(line):
                metrics["sql_tx_calls"] += 1
            if SQL_IDEMPOTENCY.search(line):
                metrics["sql_idempotency_inserts"] += 1
        for line in production_lines(text):
            metrics["production_unwrap_expect"] += len(UNWRAP.findall(line))
    for path in rust_files(CORE_SRC):
        text = path.read_text(encoding="utf-8", errors="replace")
        metrics["short_hash_defs"] += len(SHORT_HASH_DEF.findall(text))
        metrics["content_tree_digest_defs"] += len(CONTENT_TREE_DEF.findall(text))
    metrics["largest_files"] = sorted(
        per_file.items(), key=lambda item: (-item[1], item[0])
    )[:12]
    return metrics


def head() -> str | None:
    try:
        return subprocess.run(
            ["git", "rev-parse", "HEAD"],
            cwd=str(ROOT),
            capture_output=True,
            text=True,
            check=True,
        ).stdout.strip()
    except (OSError, subprocess.CalledProcessError):
        return None


def write_baseline(metrics: dict) -> None:
    payload = {
        "schema_version": "1.0",
        "generated_at": datetime.datetime.now(datetime.timezone.utc)
        .replace(microsecond=0)
        .isoformat()
        .replace("+00:00", "Z"),
        "head": head(),
        "source": "tooling/check_architecture_debt.py --update",
        "note": "Monotonic metrics may only stay level or decrease. Update deliberately after a refactor slice lands.",
        "metrics": metrics,
    }
    BASELINE.write_text(json.dumps(payload, indent=2) + "\n", encoding="utf-8")


def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("--check", action="store_true")
    parser.add_argument("--update", action="store_true")
    parser.add_argument("--json", action="store_true")
    args = parser.parse_args()
    if not args.check and not args.update:
        parser.error("pass --check or --update")
    metrics = measure()
    if args.update:
        write_baseline(metrics)
    if args.check:
        if not BASELINE.is_file():
            print(f"missing baseline: {BASELINE}", file=sys.stderr)
            return 1
        baseline = json.loads(BASELINE.read_text(encoding="utf-8")).get("metrics", {})
        failures = []
        for name in MONOTONIC:
            current = int(metrics.get(name, 0))
            recorded = int(baseline.get(name, 0))
            if current > recorded:
                failures.append(f"{name}: {current} > baseline {recorded}")
        if failures:
            print("architecture debt increased:", file=sys.stderr)
            for failure in failures:
                print(f"  {failure}", file=sys.stderr)
            return 1
    if args.json:
        print(json.dumps({"metrics": metrics}, indent=2))
    else:
        summary = {name: metrics[name] for name in MONOTONIC}
        print(json.dumps(summary, indent=2))
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
