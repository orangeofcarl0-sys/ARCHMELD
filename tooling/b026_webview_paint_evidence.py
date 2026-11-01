#!/usr/bin/env python3
"""Measure dedicated VS Code webview paint and frame timings.

Runs the real extension-host harness three times with the paint diagnostics
enabled. Each run measures, inside the nested vscode-webview:// document:

  * first paint / first-contentful-paint / largest-contentful-paint
  * the double requestAnimationFrame interval after a forced invalidation
  * the first compositor (screencast) frame after that trigger

Every command is recorded with cwd, argv, exit code, environment, log hash and
artifact hashes before the result is accepted. Raw samples are kept even when
a budget fails.
"""

from __future__ import annotations

import hashlib
import json
import os
import platform
import shutil
import statistics
import subprocess
import time
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]
LOGS = ROOT / "baseline" / "logs"
COMMANDS = ROOT / "baseline" / "commands.ndjson"
EVIDENCE = ROOT / "docs" / "release-evidence" / "B026-ui-webview-paint.json"
EVIDENCE_DIR = ROOT / "docs" / "evidence"
STATE = ROOT / "docs" / "evidence" / "ui-state"
SERVE = ROOT / "target" / "debug" / "archmeld-serve.exe"
CODE = Path("D:/Program Files/Microsoft VS Code/Code.exe")
FIXTURE = ROOT / "archmeld-vscode" / "test-ui" / "fixture"
SNAPSHOT = "sha256:505c5fcf9301d85bb9346881666f184507c8b901ef6bfbd97ff1c3183171b9cc"
PROJECT = "project.ui"
SAMPLES = 3


def sha256(path: Path) -> str:
    digest = hashlib.sha256()
    with path.open("rb") as handle:
        for chunk in iter(lambda: handle.read(1024 * 1024), b""):
            digest.update(chunk)
    return "sha256:" + digest.hexdigest()


def tool_version(argv: list[str]) -> dict:
    try:
        result = subprocess.run(argv, capture_output=True, text=True, timeout=30)
    except Exception as error:  # pragma: no cover - diagnostics only
        return {"argv": argv, "exit": None, "error": str(error)}
    text = (result.stdout or result.stderr or "").strip()
    return {
        "argv": argv,
        "exit": result.returncode,
        "first_lines": text.splitlines()[:4],
    }


def existing_ids() -> set[str]:
    ids: set[str] = set()
    if not COMMANDS.is_file():
        return ids
    with COMMANDS.open(encoding="utf-8") as handle:
        for line in handle:
            line = line.strip()
            if not line:
                continue
            try:
                ids.add(json.loads(line).get("id", ""))
            except json.JSONDecodeError:
                continue
    return ids


def aggregate(values: list[float | int]) -> dict:
    if not values:
        return {"min": None, "median": None, "max": None, "count": 0}
    return {
        "min": min(values),
        "median": statistics.median(values),
        "max": max(values),
        "count": len(values),
    }


def run_sample(index: int) -> dict:
    profile = ROOT / "target" / f"ui-profile-b026-paint-{index}"
    shutil.rmtree(profile, ignore_errors=True)
    result_path = EVIDENCE_DIR / f"ui-result-b026-paint-{index}.json"
    screenshot = EVIDENCE_DIR / f"b026-workbench-paint-{index}.png"
    for path in (result_path, screenshot):
        path.unlink(missing_ok=True)
    log_path = LOGS / f"b026-ui-paint-{index}.log"
    port = 9400 + index
    env = dict(os.environ)
    env.update(
        {
            "ARCHMELD_UI_CDP_PORT": str(port),
            "ARCHMELD_UI_PROJECT": PROJECT,
            "ARCHMELD_UI_STATE": str(STATE),
            "ARCHMELD_UI_SNAPSHOT": SNAPSHOT,
            "ARCHMELD_UI_SCREENSHOT": str(screenshot),
            "ARCHMELD_UI_RESULT": str(result_path),
            "ARCHMELD_SERVE": str(SERVE),
        }
    )
    argv = [
        str(CODE),
        f"--extensionDevelopmentPath={ROOT / 'archmeld-vscode'}",
        f"--extensionTestsPath={ROOT / 'archmeld-vscode' / 'test-ui' / 'runTests.js'}",
        "--disable-extensions",
        "--disable-workspace-trust",
        "--skip-welcome",
        "--skip-release-notes",
        f"--user-data-dir={profile}",
        f"--remote-debugging-port={port}",
        "--wait",
        str(FIXTURE),
    ]
    # The command record needs the artifact hashes, which only exist after the
    # run; the ledger entry is therefore appended after the process exits.
    started_at = time.strftime("%Y-%m-%dT%H:%M:%S%z", time.localtime())
    started = time.time()
    with log_path.open("w", encoding="utf-8", errors="replace") as log:
        result = subprocess.run(
            argv, cwd=str(ROOT), stdout=log, stderr=subprocess.STDOUT, env=env, timeout=900
        )
    duration = round(time.time() - started, 3)
    finished_at = time.strftime("%Y-%m-%dT%H:%M:%S%z", time.localtime())
    artifacts = {}
    if result_path.is_file():
        artifacts[str(result_path.relative_to(ROOT)).replace("\\", "/")] = sha256(result_path)
    if screenshot.is_file():
        artifacts[str(screenshot.relative_to(ROOT)).replace("\\", "/")] = sha256(screenshot)
    head = subprocess.run(
        ["git", "rev-parse", "HEAD"], cwd=str(ROOT), capture_output=True, text=True
    ).stdout.strip()
    entry = {
        "id": f"b026-ui-paint-{index}",
        "note": "VS Code extension-host webview paint and compositor frame sample",
        "cwd": str(ROOT),
        "argv": argv,
        "exit_code": result.returncode,
        "started_at": started_at,
        "finished_at": finished_at,
        "duration_s": duration,
        "platform": {
            "system": platform.system(),
            "release": platform.release(),
            "machine": platform.machine(),
            "python": platform.python_version(),
        },
        "tools": {
            "code": tool_version([str(CODE), "--version"]),
            "node": tool_version(["node", "--version"]),
            "git": tool_version(["git", "--version"]),
        },
        "sandbox": "local Windows desktop; loopback HTTP, CDP and a fresh VS Code profile",
        "environment": {
            "ARCHMELD_UI_CDP_PORT": str(port),
            "ARCHMELD_UI_PROJECT": PROJECT,
            "ARCHMELD_UI_STATE": str(STATE),
            "ARCHMELD_UI_SNAPSHOT": SNAPSHOT,
            "ARCHMELD_SERVE": str(SERVE),
        },
        "log": str(log_path.relative_to(ROOT)).replace("\\", "/"),
        "log_sha256": sha256(log_path),
        "artifacts": artifacts,
    }
    if head:
        entry["git_before"] = {"head": head}
    with COMMANDS.open("a", encoding="utf-8") as handle:
        handle.write(json.dumps(entry, ensure_ascii=False) + "\n")

    payload = json.loads(result_path.read_text(encoding="utf-8")) if result_path.is_file() else {}
    paint = payload.get("timings", {}).get("webview_paint", {})
    frame = payload.get("timings", {}).get("webview_frame") or {}
    measurement = payload.get("webview_measurement") or {}
    probe = measurement.get("probe") or {}
    probes = payload.get("paint_probes") or []
    return {
        "sample": index,
        "exit_code": result.returncode,
        "status": payload.get("status", "missing_result"),
        "result": str(result_path.relative_to(ROOT)).replace("\\", "/"),
        "result_sha256": sha256(result_path) if result_path.is_file() else None,
        "screenshot": str(screenshot.relative_to(ROOT)).replace("\\", "/"),
        "screenshot_sha256": sha256(screenshot) if screenshot.is_file() else None,
        "screenshot_size": screenshot.stat().st_size if screenshot.is_file() else None,
        "log": str(log_path.relative_to(ROOT)).replace("\\", "/"),
        "log_sha256": sha256(log_path),
        "open_graph_command_ms": payload.get("timings", {}).get("open_graph_command_ms"),
        "panel_ready_ms": payload.get("timings", {}).get("panel_ready_ms"),
        "webview_frame_url": measurement.get("frame_url"),
        "webview_probe_accessible": probe.get("accessible"),
        "webview_probe_time_origin_ms": probe.get("time_origin_ms"),
        "webview_probe_node_count": probe.get("node_count"),
        "webview_probe_cytoscape_container": probe.get("cytoscape_container"),
        "webview_probe_header_text": probe.get("header_text"),
        "webview_probe_paint_entries": probe.get("paint_entries"),
        "webview_first_contentful_paint_ms": paint.get("first_contentful_paint_ms"),
        "webview_largest_contentful_paint_ms": paint.get("largest_contentful_paint_ms"),
        "webview_probe_double_raf_ms": paint.get("webview_probe_double_raf_ms"),
        "webview_report_double_raf_ms": paint.get("double_raf_ms"),
        "webview_probe_roundtrips_ms": paint.get("probe_roundtrips_ms"),
        "webview_probe_protocol_elapsed_ms": paint.get("probe_protocol_elapsed_ms"),
        "paint_report_phases": [report.get("phase") for report in payload.get("paint_reports") or []],
        "render_reports": paint.get("render_reports"),
        "webview_frame": frame,
        "frame_scope": payload.get("timings", {}).get("webview_frame_scope"),
        "budget_failures": payload.get("budget_failures"),
        "measurement_limitations": measurement.get("limitations"),
        "capture_target": payload.get("capture_target"),
        "probe_count": len(probes),
    }


def main() -> None:
    if not SERVE.is_file():
        raise SystemExit(f"missing {SERVE}; run cargo build first")
    duplicates = existing_ids() & {f"b026-ui-paint-{index}" for index in range(1, SAMPLES + 1)}
    if duplicates:
        raise SystemExit(f"command ids already recorded: {sorted(duplicates)}")
    EVIDENCE_DIR.mkdir(parents=True, exist_ok=True)
    LOGS.mkdir(parents=True, exist_ok=True)

    samples = [run_sample(index) for index in range(1, SAMPLES + 1)]
    failures = [
        sample
        for sample in samples
        if sample["exit_code"] != 0 or sample["status"] != "passed"
    ]
    roundtrips = [
        value
        for sample in samples
        for value in (sample.get("webview_probe_roundtrips_ms") or [])
    ]
    probe_raf = [
        sample["webview_probe_double_raf_ms"]
        for sample in samples
        if isinstance(sample.get("webview_probe_double_raf_ms"), (int, float))
    ]
    first_frames = [
        sample["webview_frame"].get("first_frame_after_trigger_ms")
        for sample in samples
        if isinstance(sample.get("webview_frame"), dict)
        and isinstance(sample["webview_frame"].get("first_frame_after_trigger_ms"), (int, float))
    ]
    evidence = {
        "schema_version": "1.0",
        "generated_by": "tooling/b026_webview_paint_evidence.py",
        "generated_at": time.strftime("%Y-%m-%dT%H:%M:%S%z", time.localtime()),
        "platform": {
            "system": platform.system(),
            "release": platform.release(),
            "machine": platform.machine(),
            "python": platform.python_version(),
            "code": tool_version([str(CODE), "--version"]),
            "node": tool_version(["node", "--version"]),
        },
        "activator": (
            "code --extensionDevelopmentPath --extensionTestsPath "
            "--disable-extensions --disable-workspace-trust --skip-welcome "
            "--skip-release-notes --user-data-dir=<fresh> "
            "--remote-debugging-port=<port> --wait <fixture>"
        ),
        "harness": "archmeld-vscode/test-ui/runTests.js",
        "instrumentation": {
            "paint": "inline nonce script inside the graph webview posts paintReport/renderReport over the VS Code message channel",
            "cdp_document": "CDP Runtime.evaluate inside the nested vscode-webview:// document reads its performance paint timeline and a double-rAF interval",
            "compositor": "CDP Page.startScreencast on the workbench page while only the webview document is invalidated; presentation timestamps are recorded",
            "diagnostics_gate": "ARCHMELD_PAINT_PROBE=1 registers archmeld.diagnostics.paintReport/paintProbe only for the harness",
        },
        "samples": samples,
        "aggregate": {
            "open_graph_command_ms": aggregate(
                [sample["open_graph_command_ms"] for sample in samples if sample["open_graph_command_ms"] is not None]
            ),
            "panel_ready_ms": aggregate(
                [sample["panel_ready_ms"] for sample in samples if sample["panel_ready_ms"] is not None]
            ),
            "webview_first_contentful_paint_ms": aggregate(
                [
                    sample["webview_first_contentful_paint_ms"]
                    for sample in samples
                    if isinstance(sample.get("webview_first_contentful_paint_ms"), (int, float))
                ]
            ),
            "webview_largest_contentful_paint_ms": aggregate(
                [
                    sample["webview_largest_contentful_paint_ms"]
                    for sample in samples
                    if isinstance(sample.get("webview_largest_contentful_paint_ms"), (int, float))
                ]
            ),
            "webview_probe_double_raf_ms": aggregate(probe_raf),
            "webview_probe_roundtrip_ms": aggregate(roundtrips),
            "webview_first_frame_after_trigger_ms": aggregate(first_frames),
        },
        "budgets": {
            "open_graph_command_ms": 10000,
            "panel_ready_ms": 20000,
            "webview_first_contentful_paint_ms": 5000,
            "webview_probe_roundtrip_ms": 2000,
            "webview_double_raf_ms": 1000,
            "webview_first_frame_after_trigger_ms": 500,
        },
        "scope": {
            "paint": "dedicated vscode-webview:// document performance timeline",
            "frame": "requestAnimationFrame BeginFrame cadence and compositor screencast presentation timestamps",
            "not_measured": "GPU execution time, shader cost and display scan-out latency are not measured; screencast intervals include capture/encode throttling and are reported without a gate",
            "webview_geometry": "Cytoscape renders to canvas, so DOM node counts are zero; the header text and #cy container identify the graph document",
        },
        "failures": failures,
        "status": "passed" if not failures else "failed",
    }
    EVIDENCE.write_text(json.dumps(evidence, indent=2, ensure_ascii=False) + "\n", encoding="utf-8")
    print(json.dumps({"status": evidence["status"], "samples": [s["status"] for s in samples]}))
    if failures:
        raise SystemExit(2)


if __name__ == "__main__":
    main()
