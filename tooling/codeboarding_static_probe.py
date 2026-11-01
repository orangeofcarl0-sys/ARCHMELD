#!/usr/bin/env python3
# Copyright 2026 ARCHMELD contributors
# SPDX-License-Identifier: Apache-2.0
"""Run the pinned CodeBoarding static analyzer natively and summarize results.

This does not call an LLM and does not produce CodeBoarding's `analysis.json`.
It exercises the real scanner + language-server + CFG pipeline and prints the
observed languages, CFG sizes, reference counts and diagnostics. Use it to
verify that the adapter consumes a real native analysis instead of a fixture.

The CodeBoarding checkout, its Python environment, tokei and the language
server binaries must already be installed. `--extra-bin` prepends a directory
containing `pyright-langserver` (or the relevant server) to PATH.
"""

from __future__ import annotations

import argparse
import json
import os
import subprocess
import sys
import tempfile
from pathlib import Path


def _git_commit(path: Path) -> str | None:
    try:
        return subprocess.run(
            ["git", "-C", str(path), "rev-parse", "HEAD"],
            capture_output=True,
            text=True,
            check=True,
        ).stdout.strip()
    except Exception:
        return None


def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("--codeboarding", required=True, type=Path)
    parser.add_argument("--repo", required=True, type=Path)
    parser.add_argument("--out", required=True, type=Path)
    parser.add_argument("--extra-bin", action="append", default=[], type=Path)
    parser.add_argument("--cache-dir", type=Path)
    args = parser.parse_args()

    repo = args.repo.resolve()
    if not repo.is_dir():
        parser.error(f"repository does not exist: {repo}")
    codeboarding = args.codeboarding.resolve()
    if not (codeboarding / "static_analyzer").is_dir():
        parser.error(f"not a CodeBoarding checkout: {codeboarding}")

    path_parts = [str(path) for path in args.extra_bin] + [os.environ.get("PATH", "")]
    os.environ["PATH"] = os.pathsep.join(part for part in path_parts if part)
    sys.path.insert(0, str(codeboarding))

    from static_analyzer import StaticAnalyzer  # noqa: E402

    cache = args.cache_dir
    if cache is None:
        cache = Path(
            tempfile.mkdtemp(
                prefix="archmeld-cb-static-",
                dir=os.environ.get("ARCHMELD_TEMP") or None,
            )
        )
    cache.mkdir(parents=True, exist_ok=True)

    with StaticAnalyzer(repo, resident_servers=False) as analyzer:
        results = analyzer.analyze(cache, skip_cache=True)

    summary = {
        "schema_version": "1.0",
        "tool": "codeboarding-static-analyzer",
        "repo": str(repo),
        "codeboarding": str(codeboarding),
        "codeboarding_commit": _git_commit(codeboarding),
        "python": sys.version.split()[0],
        "python_executable": sys.executable,
        "cache_dir": str(cache),
        "languages": [],
        "cfgs": {},
        "references": {},
        "diagnostics": {},
        "analysis_json_produced": False,
        "llm_called": False,
    }
    for language, cfg in results.available_cfgs().items():
        name = str(language)
        summary["languages"].append(name)
        summary["cfgs"][name] = {
            "nodes": len(getattr(cfg, "nodes", {})),
            "edges": len(getattr(cfg, "edges", [])),
        }
    for language in results.results:
        name = language.value if hasattr(language, "value") else str(language)
        try:
            summary["references"][name] = len(results.get_references(language))
        except Exception:
            summary["references"][name] = 0
    for language, diags in results.diagnostics.items():
        name = language.value if hasattr(language, "value") else str(language)
        summary["diagnostics"][name] = sum(len(items) for items in diags.values())
    summary["limitations"] = [
        "static analysis only; CodeBoarding's LLM component interpretation is not run",
        "no analysis.json is produced by this probe",
        "counts describe the pinned fixture/project; they are not a precision claim",
    ]

    args.out.parent.mkdir(parents=True, exist_ok=True)
    args.out.write_text(json.dumps(summary, indent=2) + "\n", encoding="utf-8")
    print(json.dumps(summary))
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
