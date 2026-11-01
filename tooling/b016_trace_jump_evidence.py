#!/usr/bin/env python3
"""Regenerate the Tracy source-jump fixture and run the real extension host.

Step 1 writes a ledger containing one snapshot of the fixture repository and one
runtime run imported from the pinned Tracy capture. Step 2 runs the VS Code
extension host, calls `archmeld.openTraceFrame` for the first zone, and verifies
that the historical snapshot document opens at the retained source line.

Every command is recorded with cwd, argv, exit code, environment, log hash and
artifact hashes before the evidence is accepted.
"""

from __future__ import annotations

import hashlib
import json
import os
import platform
import shutil
import subprocess
import time
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]
LOGS = ROOT / "baseline" / "logs"
COMMANDS = ROOT / "baseline" / "commands.ndjson"
EVIDENCE = ROOT / "docs" / "release-evidence" / "B016-trace-source-jump.json"
STATE = ROOT / "docs" / "evidence" / "ui-trace-state"
REPO = ROOT / "docs" / "evidence" / "ui-trace-repo"
SERVE = ROOT / "target" / "debug" / "archmeld-serve.exe"
CODE = Path("D:/Program Files/Microsoft VS Code/Code.exe")
RESULT = ROOT / "docs" / "evidence" / "ui-result-b016-trace-jump.json"
SCREENSHOT = ROOT / "docs" / "evidence" / "b016-trace-source-jump.png"
PORT = 9386
PROFILE = ROOT / "target" / "ui-profile-tracejump"
ENV_BASE = {"RUSTUP_TOOLCHAIN": "1.94.1-x86_64-pc-windows-gnu"}


def sha256(path: Path) -> str:
    digest = hashlib.sha256()
    with path.open("rb") as handle:
        for chunk in iter(lambda: handle.read(1024 * 1024), b""):
            digest.update(chunk)
    return "sha256:" + digest.hexdigest()


def existing_ids() -> set[str]:
    ids: set[str] = set()
    if COMMANDS.is_file():
        for line in COMMANDS.read_text(encoding="utf-8").splitlines():
            if line.strip():
                try:
                    ids.add(json.loads(line)["id"])
                except Exception:
                    continue
    return ids


def record(
    entry_id: str,
    note: str,
    argv: list[str],
    env: dict[str, str],
    log_name: str,
    timeout: int,
    artifacts: dict[str, str] | None = None,
) -> tuple[int, dict]:
    log_path = LOGS / log_name
    LOGS.mkdir(parents=True, exist_ok=True)
    started_at = time.strftime("%Y-%m-%dT%H:%M:%S%z", time.localtime())
    started = time.time()
    with log_path.open("w", encoding="utf-8", errors="replace") as log:
        result = subprocess.run(
            argv, cwd=str(ROOT), stdout=log, stderr=subprocess.STDOUT, env=env, timeout=timeout
        )
    duration = round(time.time() - started, 3)
    head = subprocess.run(
        ["git", "rev-parse", "HEAD"], cwd=str(ROOT), capture_output=True, text=True
    ).stdout.strip()
    entry = {
        "id": entry_id,
        "note": note,
        "cwd": str(ROOT),
        "argv": argv,
        "exit_code": result.returncode,
        "started_at": started_at,
        "finished_at": time.strftime("%Y-%m-%dT%H:%M:%S%z", time.localtime()),
        "duration_s": duration,
        "platform": {
            "system": platform.system(),
            "release": platform.release(),
            "machine": platform.machine(),
            "python": platform.python_version(),
        },
        "environment": {
            key: value
            for key, value in env.items()
            if key in {"RUSTUP_TOOLCHAIN", "ARCHMELD_TRACE_UI_STATE", "ARCHMELD_TRACE_UI_REPO",
                       "ARCHMELD_UI_PROJECT", "ARCHMELD_UI_STATE", "ARCHMELD_UI_SNAPSHOT",
                       "ARCHMELD_UI_TRACE_RUN", "ARCHMELD_UI_CDP_PORT"}
        },
        "sandbox": "local Windows worktree; loopback HTTP and CDP; fixture ledger under docs/evidence",
        "log": f"baseline/logs/{log_name}",
        "log_sha256": sha256(log_path),
    }
    if artifacts:
        entry["artifacts"] = dict(artifacts)
    if head:
        entry["git_before"] = {"head": head}
    return result.returncode, entry


def append_entry(entry: dict) -> None:
    with COMMANDS.open("a", encoding="utf-8") as handle:
        handle.write(json.dumps(entry, ensure_ascii=False) + "\n")


def main() -> None:
    if not SERVE.is_file():
        raise SystemExit("build archmeld-serve first")
    ids = existing_ids()
    for entry_id in ("b016-trace-ui-fixture", "b016-trace-jump-host"):
        if entry_id in ids:
            raise SystemExit(f"{entry_id} is already recorded; use a new id or inspect the ledger")
    shutil.rmtree(STATE, ignore_errors=True)
    fixture_env = dict(os.environ)
    fixture_env.update(ENV_BASE)
    fixture_env["ARCHMELD_TRACE_UI_STATE"] = str(STATE)
    fixture_env["ARCHMELD_TRACE_UI_REPO"] = str(REPO)
    fixture_code, fixture_entry = record(
        "b016-trace-ui-fixture",
        "Write the Tracy source-jump fixture ledger from the real capture",
        ["cargo", "test", "-p", "archmeld-core", "--test", "trace_ui_fixture", "--",
         "--test-threads=1"],
        fixture_env,
        "b016-trace-ui-fixture.log",
        600,
    )
    fixture_entry["artifacts"] = {
        "docs/evidence/ui-trace-state/ui-trace-fixture.json": sha256(
            STATE / "ui-trace-fixture.json"
        ),
        "docs/evidence/ui-trace-state/state.sqlite3": sha256(STATE / "state.sqlite3"),
    }
    append_entry(fixture_entry)
    if fixture_code != 0:
        raise SystemExit(f"fixture generation failed with {fixture_code}")
    manifest = json.loads((STATE / "ui-trace-fixture.json").read_text(encoding="utf-8"))

    RESULT.unlink(missing_ok=True)
    SCREENSHOT.unlink(missing_ok=True)
    shutil.rmtree(PROFILE, ignore_errors=True)
    host_env = dict(os.environ)
    host_env.update(
        {
            "ARCHMELD_UI_CDP_PORT": str(PORT),
            "ARCHMELD_UI_PROJECT": manifest["project_id"],
            "ARCHMELD_UI_STATE": str(STATE),
            "ARCHMELD_UI_SNAPSHOT": manifest["snapshot_id"],
            "ARCHMELD_UI_TRACE_RUN": manifest["run_id"],
            "ARCHMELD_UI_SCREENSHOT": str(SCREENSHOT),
            "ARCHMELD_UI_RESULT": str(RESULT),
            "ARCHMELD_SERVE": str(SERVE),
        }
    )
    host_argv = [
        str(CODE),
        f"--extensionDevelopmentPath={ROOT / 'archmeld-vscode'}",
        f"--extensionTestsPath={ROOT / 'archmeld-vscode' / 'test-ui' / 'runTests.js'}",
        "--disable-extensions",
        "--disable-workspace-trust",
        "--skip-welcome",
        "--skip-release-notes",
        f"--user-data-dir={PROFILE}",
        f"--remote-debugging-port={PORT}",
        "--wait",
        str(ROOT / "archmeld-vscode" / "test-ui" / "fixture"),
    ]
    host_code, host_entry = record(
        "b016-trace-jump-host",
        "VS Code extension host opens the retained Tracy zone at its snapshot source line",
        host_argv,
        host_env,
        "b016-trace-jump-host.log",
        900,
    )
    artifacts = {}
    for path in (RESULT, SCREENSHOT):
        if path.is_file():
            artifacts[str(path.relative_to(ROOT)).replace("\\", "/")] = sha256(path)
    host_entry["artifacts"] = artifacts
    append_entry(host_entry)
    payload = json.loads(RESULT.read_text(encoding="utf-8"))
    evidence = {
        "schema_version": "1.0",
        "generated_by": "tooling/b016_trace_jump_evidence.py",
        "generated_at": time.strftime("%Y-%m-%dT%H:%M:%S%z", time.localtime()),
        "platform": {
            "system": platform.system(),
            "release": platform.release(),
            "machine": platform.machine(),
            "python": platform.python_version(),
        },
        "fixture": {
            "ledger": "docs/evidence/ui-trace-state",
            "repository": "docs/evidence/ui-trace-repo",
            "trace": "adapters/traces/fixtures/tracy-zones-plots.tracy",
            "csv": "adapters/traces/fixtures/tracy-zones-plots.csv",
            "project_id": manifest["project_id"],
            "snapshot_id": manifest["snapshot_id"],
            "run_id": manifest["run_id"],
            "spans": manifest["spans"],
        },
        "host": {
            "status": payload.get("status"),
            "command_exit_code": host_code,
            "span_id": payload.get("span_id"),
            "path": payload.get("path"),
            "line": payload.get("line"),
            "line_start": payload.get("line_start"),
            "line_end": payload.get("line_end"),
            "active_line": payload.get("active_line"),
            "expected_line": payload.get("expected_line"),
            "trace_command_ms": payload.get("trace_command_ms"),
            "budgets": payload.get("budgets"),
            "budget_failures": payload.get("budget_failures"),
            "result": "docs/evidence/ui-result-b016-trace-jump.json",
            "result_sha256": sha256(RESULT),
            "screenshot": "docs/evidence/b016-trace-source-jump.png",
            "screenshot_sha256": sha256(SCREENSHOT),
        },
        "scope": {
            "measured": "actual vscode-webview-independent source document opened from the snapshot CAS at the retained zone line",
            "not_measured": "trace processor queries and Tracy native protocol decoding remain out of scope",
        },
        "status": "passed"
        if host_code == 0 and payload.get("status") == "passed"
        else "failed",
    }
    EVIDENCE.write_text(json.dumps(evidence, indent=2, ensure_ascii=False) + "\n", encoding="utf-8")
    print(json.dumps({"status": evidence["status"], "line": evidence["host"]["line"]}))
    if evidence["status"] != "passed":
        raise SystemExit(2)


if __name__ == "__main__":
    main()
