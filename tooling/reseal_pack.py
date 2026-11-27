#!/usr/bin/env python3
"""Recompute the sealed specification pack manifest and SHA256SUMS.

`docs/archmeld` is a specification pack, not product code. A deliberate
contract revision may update its files, but `PACK_MANIFEST.json` and
`SHA256SUMS` must be regenerated in the same change. This tool is the only
writer for those two files.

Usage:
  py -3.11 tooling/reseal_pack.py --check
  py -3.11 tooling/reseal_pack.py --write
"""
from __future__ import annotations

import argparse
import sys
from pathlib import Path

import common

ROOT = common.ROOT
PACK = ROOT / "docs" / "archmeld"
MANIFEST = PACK / "PACK_MANIFEST.json"
SUMS = PACK / "SHA256SUMS"
EXCLUDES = {"PACK_MANIFEST.json", "SHA256SUMS"}


def pack_files() -> list[Path]:
    return sorted(
        path
        for path in PACK.rglob("*")
        if path.is_file() and path.relative_to(PACK).as_posix() not in EXCLUDES
    )


def load_manifest() -> dict:
    return common.read_json(MANIFEST)


def current_entries() -> list[dict]:
    return [
        {
            "path": path.relative_to(PACK).as_posix(),
            "size_bytes": path.stat().st_size,
            "digest": common.sha256_file(path, prefix="sha256:"),
        }
        for path in pack_files()
    ]


def compare(manifest: dict) -> list[str]:
    recorded = {item["path"]: item for item in manifest["files"]}
    actual = {item["path"]: item for item in current_entries()}
    problems: list[str] = []
    for path in sorted(recorded.keys() - actual.keys()):
        problems.append(f"missing from pack: {path}")
    for path in sorted(actual.keys() - recorded.keys()):
        problems.append(f"unrecorded file: {path}")
    for path in sorted(recorded.keys() & actual.keys()):
        if recorded[path]["digest"] != actual[path]["digest"]:
            problems.append(f"digest mismatch: {path}")
        if recorded[path]["size_bytes"] != actual[path]["size_bytes"]:
            problems.append(f"size mismatch: {path}")
    return problems


def write(manifest: dict) -> None:
    entries = current_entries()
    manifest["version"] = "1.1"
    manifest["specification_date"] = "2026-11-26"
    manifest["files"] = entries
    common.write_json(MANIFEST, manifest)
    lines = [
        f"{entry['digest'].removeprefix('sha256:')}  {entry['path']}"
        for entry in entries
    ]
    SUMS.write_text("\n".join(lines) + "\n", encoding="utf-8")


def main() -> int:
    parser = argparse.ArgumentParser()
    group = parser.add_mutually_exclusive_group(required=True)
    group.add_argument("--check", action="store_true")
    group.add_argument("--write", action="store_true")
    args = parser.parse_args()
    manifest = load_manifest()
    if args.check:
        problems = compare(manifest)
        for problem in problems:
            print(problem)
        print(f"{len(problems)} sealed-pack mismatch(es)")
        return 1 if problems else 0
    write(manifest)
    print(f"re-sealed {len(manifest['files'])} files; version 1.1")
    return 0


if __name__ == "__main__":
    sys.exit(main())
