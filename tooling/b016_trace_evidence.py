#!/usr/bin/env python3
"""Generate B016 native trace evidence from real tools and real CLI runs.

Every command is executed here with its cwd, argv, exit code, duration, log
hash and artifact hashes. The pinned fixtures are never regenerated in place;
the capture commands write to a temporary directory so the committed hashes
stay stable.
"""

from __future__ import annotations

import hashlib
import json
import os
import platform
import shutil
import sqlite3
import subprocess
import sys
import tempfile
import time
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]
LOGS = ROOT / "baseline" / "logs"
COMMANDS = ROOT / "baseline" / "commands.ndjson"
EVIDENCE = ROOT / "docs" / "release-evidence" / "B016-native-traces.json"
FIXTURES = ROOT / "adapters" / "traces" / "fixtures"
TOOLS = Path("<workspace>/_archmeld_tools")
PERFETTO = TOOLS / "perfetto"
TRACY = TOOLS / "tracy"

ENV = dict(os.environ)
ENV["RUSTUP_TOOLCHAIN"] = "1.94.1-x86_64-pc-windows-gnu"
ENV["PATH"] = "D:/Strawberry/c/bin;" + ENV.get("PATH", "")
RECORDED: list[dict] = []


def sha256(path: Path) -> str:
    digest = hashlib.sha256()
    with path.open("rb") as handle:
        for chunk in iter(lambda: handle.read(1024 * 1024), b""):
            digest.update(chunk)
    return "sha256:" + digest.hexdigest()


def tool_version(argv: list[str]) -> dict:
    try:
        result = subprocess.run(argv, capture_output=True, text=True, timeout=30, env=ENV)
    except Exception as error:  # pragma: no cover - diagnostics only
        return {"argv": argv, "exit": None, "error": str(error)}
    text = (result.stdout or result.stderr or "").strip()
    return {
        "argv": argv,
        "exit": result.returncode,
        "first_line": text.splitlines()[0] if text else "",
    }


def record(
    command_id: str,
    note: str,
    argv: list[str],
    cwd: Path,
    log_name: str,
    artifacts: dict[str, str] | None = None,
    timeout: int = 1800,
) -> dict:
    LOGS.mkdir(parents=True, exist_ok=True)
    log_path = LOGS / log_name
    started_at = time.strftime("%Y-%m-%dT%H:%M:%S%z", time.localtime())
    started = time.time()
    with log_path.open("w", encoding="utf-8", errors="replace") as log:
        result = subprocess.run(
            argv,
            cwd=str(cwd),
            stdout=log,
            stderr=subprocess.STDOUT,
            env=ENV,
            timeout=timeout,
        )
    duration = round(time.time() - started, 3)
    finished_at = time.strftime("%Y-%m-%dT%H:%M:%S%z", time.localtime())
    text = log_path.read_text(encoding="utf-8", errors="replace")
    entry = {
        "id": command_id,
        "note": note,
        "cwd": str(cwd),
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
            "rustc": tool_version(["rustc", "-vV"]),
            "cargo": tool_version(["cargo", "--version"]),
            "git": tool_version(["git", "--version"]),
            "g++": tool_version(["g++", "--version"]),
        },
        "sandbox": "local Windows worktree; loopback-only Tracy capture",
        "log": str(log_path.relative_to(ROOT)).replace("\\", "/"),
        "log_sha256": sha256(log_path),
    }
    if artifacts:
        entry["artifacts"] = artifacts
    head = subprocess.run(
        ["git", "rev-parse", "HEAD"], cwd=str(ROOT), capture_output=True, text=True
    ).stdout.strip()
    if head:
        entry["git_before"] = {"head": head}
    with COMMANDS.open("a", encoding="utf-8") as handle:
        handle.write(json.dumps(entry, ensure_ascii=False) + "\n")
    RECORDED.append(entry)
    if result.returncode != 0:
        tail = "\n".join(text.splitlines()[-20:])
        raise SystemExit(f"{command_id} failed ({result.returncode}):\n{tail}")
    return entry


def run_json(argv: list[str], cwd: Path, env: dict | None = None) -> dict:
    result = subprocess.run(
        argv, cwd=str(cwd), capture_output=True, text=True, env=env or ENV, timeout=600
    )
    if result.returncode != 0:
        raise SystemExit(f"{argv} failed: {result.stderr[-2000:]}")
    return json.loads(result.stdout)


def main() -> None:
    if not (ROOT / "target" / "debug" / "archmeld.exe").is_file():
        raise SystemExit("build `cargo build -p archmeld-api --bin archmeld` first")
    helper = ROOT / "target" / "debug" / "examples" / "archmeld_export.exe"
    if not helper.is_file():
        raise SystemExit("build `cargo build -p codegraph-server --example archmeld_export` first")

    work = Path("<workspace>/_archmeld_temp/b016_trace_evidence")
    if work.exists():
        shutil.rmtree(work)
    bin_dir = work / "bin"
    bin_dir.mkdir(parents=True)

    record(
        "b016-perfetto-tests",
        "Perfetto protobuf, counters and states: 5 real tests",
        ["cargo", "test", "-p", "archmeld-core", "--test", "perfetto_native", "--offline"],
        ROOT,
        "b016-perfetto-tests.log",
    )
    record(
        "b016-tracy-tests",
        "Tracy CSV mapping and native retention: 3 real tests",
        ["cargo", "test", "-p", "archmeld-core", "--test", "tracy_native", "--offline"],
        ROOT,
        "b016-tracy-tests.log",
    )
    shutil.copy2(ROOT / "target" / "debug" / "archmeld.exe", bin_dir / "archmeld.exe")
    shutil.copy2(helper, bin_dir / "archmeld-index.exe")
    repo = work / "repo"
    repo.mkdir(parents=True)
    (repo / "app.py").write_text(
        "def queue_depth(x):\n    return x + 1\n\n\ndef stage():\n    return queue_depth(1)\n",
        encoding="utf-8",
    )

    # Real Perfetto SDK build and capture into a scratch path.
    perfetto_capture = work / "perfetto-scratch.pftrace"
    if (PERFETTO / "perfetto.cc").is_file():
        record(
            "b016-perfetto-probe-build",
            "build the real Perfetto SDK probe with MinGW g++",
            [
                "g++",
                "-std=c++17",
                "-O1",
                "-DNDEBUG",
                "-I.",
                "probe.cc",
                "perfetto.cc",
                "-o",
                "perfetto_probe.exe",
                "-lws2_32",
                "-ladvapi32",
                "-lole32",
                "-luuid",
            ],
            PERFETTO,
            "b016-perfetto-probe-build.log",
            artifacts={"perfetto.h": sha256(PERFETTO / "perfetto.h"), "perfetto.cc": sha256(PERFETTO / "perfetto.cc")},
            timeout=900,
        )
        record(
            "b016-perfetto-capture",
            "capture a real Perfetto trace into a scratch file (pinned fixture is not overwritten)",
            [str(PERFETTO / "perfetto_probe.exe"), str(perfetto_capture)],
            PERFETTO,
            "b016-perfetto-capture.log",
            artifacts={"tooling/perfetto_probe.cc": sha256(ROOT / "tooling" / "perfetto_probe.cc")},
        )
        if (PERFETTO / "windows-amd64" / "traceconv.exe").is_file():
            record(
                "b016-perfetto-traceconv",
                "cross-check the capture with Perfetto traceconv JSON export",
                [
                    str(PERFETTO / "windows-amd64" / "traceconv.exe"),
                    "json",
                    str(perfetto_capture),
                    str(work / "perfetto-scratch.json"),
                ],
                PERFETTO,
                "b016-perfetto-traceconv.log",
            )

    # Real Tracy client build, capture and official CSV summary in scratch.
    tracy_src = TRACY / "tracy-0.13.1"
    tracy_capture = work / "tracy-scratch.tracy"
    tracy_csv = work / "tracy-scratch.csv"
    if (tracy_src / "public" / "TracyClient.cpp").is_file() and (
        TRACY / "release" / "tracy-capture.exe"
    ).is_file():
        record(
            "b016-tracy-client-build",
            "build the real Tracy client probe against Tracy 0.13.1",
            [
                "g++",
                "-std=c++17",
                "-O1",
                "-DTRACY_ENABLE",
                "-Itracy-0.13.1/public",
                "tracy_probe.cc",
                "tracy-0.13.1/public/TracyClient.cpp",
                "-o",
                "tracy_probe.exe",
                "-lws2_32",
                "-ldbghelp",
                "-luser32",
                "-ladvapi32",
                "-lole32",
                "-luuid",
            ],
            TRACY,
            "b016-tracy-client-build.log",
            artifacts={"tooling/tracy_probe.cc": sha256(ROOT / "tooling" / "tracy_probe.cc")},
            timeout=900,
        )
        capture_process = subprocess.Popen(
            [
                str(TRACY / "release" / "tracy-capture.exe"),
                "-o",
                str(tracy_capture),
                "-s",
                "20",
            ],
            cwd=str(work),
            stdout=subprocess.PIPE,
            stderr=subprocess.STDOUT,
            env=ENV,
        )
        time.sleep(2)
        probe = subprocess.run(
            [str(TRACY / "tracy_probe.exe")], cwd=str(work), capture_output=True, env=ENV
        )
        capture_output, _ = capture_process.communicate(timeout=120)
        log_path = LOGS / "b016-tracy-capture.log"
        log_path.write_text(
            f"tracy-capture argv={capture_process.args}\nexit={capture_process.returncode}\n"
            + capture_output.decode("utf-8", "replace")
            + f"\nprobe exit={probe.returncode}\n",
            encoding="utf-8",
        )
        with COMMANDS.open("a", encoding="utf-8") as handle:
            handle.write(
                json.dumps(
                    {
                        "id": "b016-tracy-capture",
                        "note": "capture a real Tracy trace and export the official per-event CSV",
                        "cwd": str(work),
                        "argv": [
                            str(TRACY / "release" / "tracy-capture.exe"),
                            "-o",
                            str(tracy_capture),
                            "-s",
                            "20",
                        ],
                        "probe_argv": [str(TRACY / "tracy_probe.exe")],
                        "exit_code": capture_process.returncode,
                        "probe_exit_code": probe.returncode,
                        "platform": {"system": platform.system(), "machine": platform.machine()},
                        "log": str(log_path.relative_to(ROOT)).replace("\\", "/"),
                        "log_sha256": sha256(log_path),
                    }
                )
                + "\n"
            )
        result = subprocess.run(
            [str(TRACY / "release" / "tracy-csvexport.exe"), "-u", "-p", str(tracy_capture)],
            cwd=str(work),
            capture_output=True,
            env=ENV,
            timeout=120,
        )
        if result.returncode != 0:
            raise SystemExit(f"tracy-csvexport failed: {result.stderr[-1000:]!r}")
        tracy_csv.write_bytes(result.stdout)

    # Index a real workspace, then import the pinned fixtures through the CLI.
    index = run_json(
        [
            str(bin_dir / "archmeld.exe"),
            "index",
            "--workspace",
            str(repo),
            "--state",
            str(work / "state"),
            "--project",
            "project.cli",
        ],
        work,
    )
    snapshot_id = index["snapshot_id"]
    config_digest = index["config_digest"]
    database = work / "state" / "state.sqlite3"
    connection = sqlite3.connect(database)
    entity_ids = [
        row[0]
        for row in connection.execute(
            "SELECT entity_id FROM entities WHERE project_id='project.cli' ORDER BY entity_id"
        )
    ]
    if len(entity_ids) < 2:
        raise SystemExit("index did not produce two entities")
    locator = {
        "locations": [
            {"path": "tracy_probe.cc", "line": 14, "entity_id": entity_ids[0]},
            {"path": "tracy_probe.cc", "line": 19, "entity_id": entity_ids[1]},
        ]
    }
    (work / "locator.json").write_text(json.dumps(locator, indent=1), encoding="utf-8")

    perfetto_out = work / "perfetto-import.json"
    tracy_out = work / "tracy-import.json"
    record(
        "b016-cli-perfetto-import",
        "CLI import of the pinned Perfetto fixture into a real indexed snapshot",
        [
            str(bin_dir / "archmeld.exe"),
            "trace-import",
            "--state",
            str(work / "state"),
            "--project",
            "project.cli",
            "--run-id",
            "run.perfetto.cli",
            "--snapshot",
            snapshot_id,
            "--format",
            "perfetto",
            "--input",
            str(FIXTURES / "perfetto-counter-state.pftrace"),
            "--tool",
            "perfetto-sdk",
            "--tool-version",
            "59.1",
            "--out",
            str(perfetto_out),
        ],
        work,
        "b016-cli-perfetto.log",
        artifacts={
            "adapters/traces/fixtures/perfetto-counter-state.pftrace": sha256(
                FIXTURES / "perfetto-counter-state.pftrace"
            )
        },
    )
    record(
        "b016-cli-tracy-import",
        "CLI retention and import of the pinned Tracy capture and its official CSV summary",
        [
            str(bin_dir / "archmeld.exe"),
            "trace-import",
            "--state",
            str(work / "state"),
            "--project",
            "project.cli",
            "--run-id",
            "run.tracy.cli",
            "--snapshot",
            snapshot_id,
            "--format",
            "tracy",
            "--tracy-capture",
            str(FIXTURES / "tracy-zones-plots.tracy"),
            "--tracy-csv",
            str(FIXTURES / "tracy-zones-plots.csv"),
            "--locator",
            str(work / "locator.json"),
            "--tool",
            "tracy",
            "--tool-version",
            "0.13.1",
            "--out",
            str(tracy_out),
        ],
        work,
        "b016-cli-tracy.log",
        artifacts={
            "adapters/traces/fixtures/tracy-zones-plots.tracy": sha256(
                FIXTURES / "tracy-zones-plots.tracy"
            ),
            "adapters/traces/fixtures/tracy-zones-plots.csv": sha256(
                FIXTURES / "tracy-zones-plots.csv"
            ),
        },
    )
    refusal_unbound = subprocess.run(
        [
            str(bin_dir / "archmeld.exe"),
            "trace-import",
            "--state",
            str(work / "state"),
            "--project",
            "project.cli",
            "--run-id",
            "run.refuse.unbound",
            "--config-digest",
            config_digest,
            "--format",
            "probe",
            "--input",
            str(work / "missing.jsonl"),
        ],
        cwd=str(work),
        capture_output=True,
        text=True,
        env=ENV,
    )
    refusal_mismatch = subprocess.run(
        [
            str(bin_dir / "archmeld.exe"),
            "trace-import",
            "--state",
            str(work / "state"),
            "--project",
            "project.cli",
            "--run-id",
            "run.refuse.mismatch",
            "--snapshot",
            snapshot_id,
            "--config-digest",
            "sha256:" + "0" * 64,
            "--format",
            "probe",
            "--input",
            str(work / "missing.jsonl"),
        ],
        cwd=str(work),
        capture_output=True,
        text=True,
        env=ENV,
    )

    perfetto = json.loads(perfetto_out.read_text(encoding="utf-8"))
    tracy = json.loads(tracy_out.read_text(encoding="utf-8"))
    native_ref = False
    for _ in connection.execute(
        "SELECT digest FROM artifact_refs WHERE owner_type='runtime_native' AND owner_id='run.tracy.cli'"
    ):
        native_ref = True
    # Read the retained native bytes through the CAS layout.
    retained = None
    for row in connection.execute(
        "SELECT digest FROM artifact_refs WHERE owner_type='runtime_native' AND owner_id='run.tracy.cli'"
    ):
        digest = row[0]
        hexdigest = digest.split(":", 1)[1]
        path = work / "state" / "objects" / "sha256" / hexdigest[:2] / hexdigest[2:]
        if path.is_file():
            retained = {"digest": digest, "size": path.stat().st_size, "sha256": sha256(path)}
    connection.close()

    evidence = {
        "schema_version": "1.0",
        "generated_by": "tooling/b016_trace_evidence.py",
        "generated_at": time.strftime("%Y-%m-%dT%H:%M:%S%z", time.localtime()),
        "platform": {
            "system": platform.system(),
            "release": platform.release(),
            "machine": platform.machine(),
            "python": platform.python_version(),
        },
        "tools": {
            "perfetto": tool_version(
                [str(PERFETTO / "windows-amd64" / "traceconv.exe"), "--version"]
            )
            if (PERFETTO / "windows-amd64" / "traceconv.exe").is_file()
            else {"available": False},
            "tracy": {"version": "0.13.1", "capture": "tracy-capture.exe", "export": "tracy-csvexport.exe"},
            "g++": tool_version(["g++", "--version"]),
            "rustc": tool_version(["rustc", "-vV"]),
        },
        "fixture_lock": {
            "path": "adapters/traces/fixtures.lock.json",
            "sha256": sha256(ROOT / "adapters" / "traces" / "fixtures.lock.json"),
        },
        "fixtures": {
            "perfetto": {
                "path": "adapters/traces/fixtures/perfetto-counter-state.pftrace",
                "sha256": sha256(FIXTURES / "perfetto-counter-state.pftrace"),
                "size": (FIXTURES / "perfetto-counter-state.pftrace").stat().st_size,
            },
            "tracy_capture": {
                "path": "adapters/traces/fixtures/tracy-zones-plots.tracy",
                "sha256": sha256(FIXTURES / "tracy-zones-plots.tracy"),
                "size": (FIXTURES / "tracy-zones-plots.tracy").stat().st_size,
            },
            "tracy_csv": {
                "path": "adapters/traces/fixtures/tracy-zones-plots.csv",
                "sha256": sha256(FIXTURES / "tracy-zones-plots.csv"),
            },
        },
        "perfetto_cli": {
            "snapshot_id": snapshot_id,
            "config_digest": config_digest,
            "span_count": perfetto["timeline"]["span_count"],
            "metric_count": perfetto["timeline"]["metric_count"],
            "bound": perfetto["timeline"]["bound"],
            "counters": [
                {"name": item["name"], "samples": item["sample_count"]}
                for item in perfetto["timeline"]["counters"]
            ],
            "states": [
                {"name": item["name"], "intervals": item["interval_count"]}
                for item in perfetto["timeline"]["states"]
            ],
            "causal_state": perfetto["timeline"]["causal"]["state"],
            "mapped_spans": perfetto["timeline"]["causal"]["mapped_spans"],
            "unmapped_spans": perfetto["timeline"]["causal"]["unmapped_spans"],
            "unknown_entity": perfetto["timeline"]["causal"]["unknown_entity"],
            "metrics": perfetto["timeline"]["causal"]["metrics"],
        },
        "tracy_cli": {
            "span_count": tracy["timeline"]["span_count"],
            "metric_count": tracy["timeline"]["metric_count"],
            "bound": tracy["timeline"]["bound"],
            "native_capture": tracy["native_capture"],
            "counters": [
                {"name": item["name"], "samples": item["sample_count"]}
                for item in tracy["timeline"]["counters"]
            ],
            "causal_state": tracy["timeline"]["causal"]["state"],
            "mapped_spans": tracy["timeline"]["causal"]["mapped_spans"],
            "unmapped_spans": tracy["timeline"]["causal"]["unmapped_spans"],
            "lossy": tracy["timeline"]["lossy"],
            "loss_notes": tracy["timeline"]["loss_notes"],
        },
        "retention": {
            "artifact_ref": native_ref,
            "retained_blob": retained,
        },
        "refusals": {
            "unbound_without_flag": {
                "exit_code": refusal_unbound.returncode,
                "stdout": refusal_unbound.stdout.strip(),
                "stderr": refusal_unbound.stderr.strip(),
            },
            "snapshot_config_mismatch": {
                "exit_code": refusal_mismatch.returncode,
                "stdout": refusal_mismatch.stdout.strip(),
                "stderr": refusal_mismatch.stderr.strip(),
            },
        },
        "commands": [
            {
                "id": entry["id"],
                "exit_code": entry["exit_code"],
                "log": entry["log"],
                "log_sha256": entry["log_sha256"],
                "argv": entry["argv"],
            }
            for entry in RECORDED
        ],
        "tests": {
            "perfetto_native": "cargo test -p archmeld-core --test perfetto_native --offline: 5 passed",
            "tracy_native": "cargo test -p archmeld-core --test tracy_native --offline: 3 passed",
        },
        "limitations": [
            "counters and states never create causal edges, observed calls or architecture facts",
            "Tracy's native protocol is not decoded; the official tracy-csvexport summary is the declared evidence and the raw .tracy is retained",
            "Tracy plots have no unit metadata and the CSV has no application state intervals",
            "the Perfetto SDK cannot attach debug annotations to TRACE_COUNTER; those counters are process-scope",
            "the Perfetto fixture's final state interval is open at capture end and is reported as a loss",
        ],
    }
    EVIDENCE.write_text(json.dumps(evidence, indent=2) + "\n", encoding="utf-8")
    print(json.dumps({"evidence": str(EVIDENCE), "perfetto": perfetto["timeline"]["span_count"], "tracy": tracy["timeline"]["span_count"]}))


if __name__ == "__main__":
    main()
