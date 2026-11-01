#!/usr/bin/env python3
"""Run CodeBoarding's full analysis against the authorized model provider.

The provider is reached through `tooling/b017_model_proxy.py`, which injects the
required session header and the key from the ZCode provider config. The key is
never placed in argv, logs or the provenance file. The target repository is a
local copy; CodeBoarding writes `<target>/.codeboarding/analysis.json`.

This tool is evidence plumbing, not a product dependency.
"""

from __future__ import annotations

import argparse
import hashlib
import json
import os
import re
import shutil
import subprocess
import sys
import time
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]


def default_uv() -> str:
    candidate = Path(os.path.expandvars(r"%USERPROFILE%\.local\bin\uv.exe"))
    return str(candidate) if candidate.is_file() else "uv"


def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("--codeboarding", required=True, type=Path)
    parser.add_argument("--target", required=True, type=Path)
    parser.add_argument("--depth-cap", type=int, default=2)
    parser.add_argument(
        "--provider-config",
        default=os.path.expandvars(r"%USERPROFILE%\.zcode\v2\provider_config.json"),
    )
    parser.add_argument("--provider-name", default="opencode go orange")
    parser.add_argument("--model", default="deepseek-v4.1-flash")
    parser.add_argument("--uv", default=default_uv())
    parser.add_argument("--run-log", required=True, type=Path)
    parser.add_argument("--proxy-log", required=True, type=Path)
    parser.add_argument("--analysis-out", required=True, type=Path)
    parser.add_argument("--provenance-out", required=True, type=Path)
    parser.add_argument("--timeout", type=int, default=3600)
    args = parser.parse_args()

    codeboarding = args.codeboarding.resolve()
    target = args.target.resolve()
    if not (codeboarding / "codeboarding_cli").is_dir():
        raise SystemExit(f"not a CodeBoarding checkout: {codeboarding}")
    if not target.is_dir():
        raise SystemExit(f"target repository is missing: {target}")
    args.run_log.parent.mkdir(parents=True, exist_ok=True)
    args.proxy_log.parent.mkdir(parents=True, exist_ok=True)
    args.analysis_out.parent.mkdir(parents=True, exist_ok=True)
    args.provenance_out.parent.mkdir(parents=True, exist_ok=True)

    proxy_log = args.proxy_log.open("w", encoding="utf-8")
    proxy = subprocess.Popen(
        [
            sys.executable,
            str(ROOT / "tooling" / "b017_model_proxy.py"),
            "--provider-config",
            args.provider_config,
            "--provider-name",
            args.provider_name,
        ],
        stdout=subprocess.PIPE,
        stderr=proxy_log,
        text=True,
    )
    try:
        announcement = json.loads(proxy.stdout.readline())
        endpoint = announcement["endpoint"]
        env = dict(os.environ)
        env.update(
            {
                "OPENAI_BASE_URL": endpoint,
                "OPENAI_API_KEY": "proxy-local",
                "AGENT_MODEL": args.model,
                "CODEBOARDING_TELEMETRY": "false",
                "DO_NOT_TRACK": "1",
            }
        )
        command = [
            args.uv,
            "run",
            "python",
            "-m",
            "codeboarding_cli.main",
            "full",
            "--local",
            str(target),
            "--depth-cap",
            str(args.depth_cap),
        ]
        started = time.time()
        with args.run_log.open("w", encoding="utf-8", errors="replace") as log:
            result = subprocess.run(
                command,
                cwd=str(codeboarding),
                env=env,
                stdout=log,
                stderr=subprocess.STDOUT,
                timeout=args.timeout,
            )
        duration = round(time.time() - started, 1)
        if result.returncode != 0:
            print(json.dumps({"status": "failed", "exit": result.returncode}), file=sys.stderr)
            return result.returncode
    finally:
        proxy.terminate()
        try:
            proxy.wait(timeout=10)
        except subprocess.TimeoutExpired:
            proxy.kill()
        proxy_log.close()

    analysis_path = target / ".codeboarding" / "analysis.json"
    if not analysis_path.is_file():
        print(json.dumps({"status": "failed", "reason": "analysis.json missing"}), file=sys.stderr)
        return 1
    shutil.copyfile(analysis_path, args.analysis_out)
    analysis_bytes = args.analysis_out.read_bytes()
    analysis = json.loads(analysis_bytes)
    log_text = args.run_log.read_text(encoding="utf-8", errors="replace")
    input_tokens = sum(
        int(match.group(1))
        for match in re.finditer(r'Token Usage:.*usage=\{"input_tokens": (\d+)', log_text)
    )
    output_tokens = sum(
        int(match.group(1))
        for match in re.finditer(r'Token Usage:.*"output_tokens": (\d+)', log_text)
    )
    calls = sum(
        1
        for line in args.proxy_log.read_text(encoding="utf-8").splitlines()
        if line.strip()
    )
    commit = subprocess.run(
        ["git", "-C", str(codeboarding), "rev-parse", "HEAD"],
        capture_output=True,
        text=True,
    ).stdout.strip()
    version = "unknown"
    pyproject = codeboarding / "pyproject.toml"
    if pyproject.is_file():
        for line in pyproject.read_text(encoding="utf-8").splitlines():
            match = re.match(r'version\s*=\s*"([^"]+)"', line.strip())
            if match:
                version = match.group(1)
                break
    provenance = {
        "schema_version": "1.0",
        "generated_by": "tooling/b017_model_analysis_run.py",
        "provider": announcement.get("provider", args.provider_name),
        "model": args.model,
        "base_url_host": announcement.get("base_url_host", "unknown"),
        "calls": calls,
        "input_tokens": input_tokens,
        "output_tokens": output_tokens,
        "provider_version": version,
        "codeboarding_commit": commit,
        "target_repo": str(target),
        "analysis_path": str(args.analysis_out),
        "analysis_sha256": "sha256:" + hashlib.sha256(analysis_bytes).hexdigest(),
        "analysis_size_bytes": len(analysis_bytes),
        "run_log": str(args.run_log),
        "proxy_log": str(args.proxy_log),
        "duration_s": duration,
        "key_source": "ZCode provider configuration (provider_config.json); the key never enters argv, logs or this file",
        "limitations": [
            "model output varies by run; component counts and descriptions are observations of this run",
            "the source copy is prepared separately and should be pinned by commit before the run",
        ],
    }
    args.provenance_out.write_text(json.dumps(provenance, indent=2) + "\n", encoding="utf-8")
    print(
        json.dumps(
            {
                "status": "passed",
                "components": len(analysis.get("components", [])),
                "relations": len(analysis.get("components_relations", [])),
                "calls": calls,
                "input_tokens": input_tokens,
                "output_tokens": output_tokens,
                "duration_s": duration,
                "analysis_sha256": provenance["analysis_sha256"],
            }
        )
    )
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
