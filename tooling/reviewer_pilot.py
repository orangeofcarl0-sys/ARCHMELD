#!/usr/bin/env python3
"""Reviewer pilot using durable `pi` sessions as independent agent reviewers.

This is an *agent* study. It does not claim human review. Each reviewer is a
separate durable pi session with its own state and no shared context; the two
reviewers use different model families. Arm order is randomized per fixture
and reviewer, and scoring is against a key written before any run.

Usage:
  py -3.11 tooling/reviewer_pilot.py --prepare
  py -3.11 tooling/reviewer_pilot.py --run-all
  py -3.11 tooling/reviewer_pilot.py --score
"""

from __future__ import annotations

import argparse
import hashlib
import json
import os
import random
import re
import shutil
import stat
import subprocess
import sys
import time
import uuid
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]
sys.path.insert(0, str(ROOT / "tooling"))
import e4a_launcher as e4a  # noqa: E402

DEFAULT_OUT = Path(r"<path>")
WINDOWS_CODE = Path(r"<path> Files/Microsoft VS Code/bin/code.cmd")
ARCHMELD = ROOT / "target" / "debug" / "archmeld.exe"
MCP = ROOT / "target" / "debug" / "archmeld-mcp.exe"
HELPER = ROOT / "target" / "debug" / "examples" / "archmeld_export.exe"

REVIEWERS = {
    "reviewer-1": "deepseek/deepseek-v4-pro",
    "reviewer-2": "omni-responses/deepseek-v4.1-flash",
}
THINKING = "high"
TIMEOUT = 300

FIXTURES: dict[str, dict] = {
    "move": {
        "files": {
            "src/core/alpha.py": "def alpha():\n    return 1\n",
            "src/caller.py": "from core.alpha import alpha\n\ndef caller():\n    return alpha()\n",
        },
        "moves": [("src/core/alpha.py", "src/core/alpha_moved.py")],
        "edits": {},
        "task": (
            "A review is in progress for the last commit. Determine which "
            "entity moved, its old and new paths, and whether any caller "
            "source changed. The caller file may now be stale."
        ),
        "answer_shape": {
            "moved_entity": "qualified or short name",
            "moved_from": "workspace-relative POSIX path",
            "moved_to": "workspace-relative POSIX path",
            "caller_changed": "true|false",
        },
        "key": {
            "entities": ["alpha"],
            "paths": ["src/core/alpha.py", "src/core/alpha_moved.py"],
            "caller_changed": False,
            "writes": "not_applicable",
            "relation": "not_applicable",
            "coverage": "any",
        },
    },
    "context": {
        "files": {
            "src/target.py": "def target():\n    return 1\n",
            "src/caller.py": "from target import target\n\ndef caller():\n    return target()\n",
        },
        "edits": {"src/target.py": "def target():\n    return 2\n"},
        "task": (
            "A review is in progress for the last commit. Identify the changed "
            "owner, any unchanged caller that must remain context (not a second "
            "change), and whether the incoming relation is a resolved call or a "
            "candidate hypothesis."
        ),
        "answer_shape": {
            "changed_owner": "entity name",
            "changed_path": "workspace-relative POSIX path",
            "unchanged_context_entity": "entity name",
            "incoming_resolution": "resolved|candidate|unknown",
        },
        "key": {
            "entities": ["target"],
            "context_entities": ["caller"],
            "paths": ["src/target.py", "src/caller.py"],
            "writes": "not_applicable",
            "relation": "candidate",
            "coverage": "any",
        },
    },
    "gap": {
        "files": {
            "src/settle.py": "def settle():\n    return 1\n",
            "src/ops.py": "from settle import settle\n\ndef run():\n    return settle()\n",
            "docs/notes.md": "# Notes\n\nsettlement behavior\n",
        },
        "edits": {
            "docs/notes.md": "# Notes\n\nsettlement behavior is documented here\n",
            "src/ops.py": "from settle import settle\n\ndef run():\n    return settle() + 1\n",
        },
        "task": (
            "A review is in progress for the last commit. Identify every "
            "changed file without an entity mapping (a named gap), and state "
            "whether there is evidence that run writes to settle. Do not infer "
            "writes from source shape alone."
        ),
        "answer_shape": {
            "unmapped_paths": ["workspace-relative POSIX path"],
            "changed_entities": ["entity name"],
            "writes_evidence": "present|unavailable",
            "coverage": "complete|partial|unknown",
        },
        "key": {
            "entities": ["run"],
            "paths": ["docs/notes.md", "src/ops.py"],
            "unmapped_paths": ["docs/notes.md"],
            "writes": "unavailable",
            "relation": "not_applicable",
            "coverage": "partial",
        },
    },
}


def sha256_file(path: Path) -> str:
    return "sha256:" + hashlib.sha256(path.read_bytes()).hexdigest()


def run_cmd(argv: list[str], cwd: Path | None, timeout: int = 120) -> dict:
    proc = subprocess.run(
        argv,
        cwd=str(cwd) if cwd else None,
        capture_output=True,
        text=True,
        encoding="utf-8",
        errors="replace",
        timeout=timeout,
    )
    return {
        "argv": argv,
        "exit_code": proc.returncode,
        "stdout": proc.stdout,
        "stderr": proc.stderr,
    }


def git(repo: Path, *args: str) -> None:
    result = run_cmd(["git", "-C", str(repo), *args], None)
    if result["exit_code"] != 0:
        raise SystemExit(f"git {args} failed: {result['stderr']}")


def force_rmtree(path: Path) -> None:
    if not path.exists():
        return
    for root, _dirs, files in os.walk(path):
        for name in files:
            try:
                os.chmod(Path(root) / name, stat.S_IWRITE)
            except OSError:
                pass
    shutil.rmtree(path)


def prepare_fixture_root(out: Path, name: str) -> Path:
    root = out / "fixtures" / name
    if root.exists():
        force_rmtree(root)
    repo = root / "repo"
    repo.mkdir(parents=True)
    spec = FIXTURES[name]
    for relative, text in spec["files"].items():
        target = repo / relative
        target.parent.mkdir(parents=True, exist_ok=True)
        target.write_text(text, encoding="utf-8")
    # The arm-B MCP config lives in .pi/; keep it out of the reviewed change.
    (repo / ".gitignore").write_text(".pi/\n.archmeld/\n", encoding="utf-8")
    git(repo, "init", "-q")
    git(repo, "add", ".")
    git(
        repo,
        "-c",
        "user.name=reviewer-pilot",
        "-c",
        "user.email=reviewer@example.invalid",
        "commit",
        "-q",
        "-m",
        "base",
    )
    init = run_cmd(
        [
            str(ARCHMELD),
            "init",
            "--workspace",
            str(repo),
            "--state",
            str(root / "base-state"),
            "--project",
            f"project.reviewer.{name}",
            "--client",
            "generic",
            "--index-helper",
            str(HELPER),
        ],
        ROOT,
        300,
    )
    if init["exit_code"] != 0:
        raise SystemExit(f"archmeld init failed for {name}: {init['stderr']}")
    init_json = json.loads(init["stdout"])
    (root / "init.json").write_text(json.dumps(init_json, indent=2), encoding="utf-8")
    # Candidate commit.
    for relative, text in spec.get("edits", {}).items():
        target = repo / relative
        target.parent.mkdir(parents=True, exist_ok=True)
        target.write_text(text, encoding="utf-8")
    for source, target in spec.get("moves", []):
        git(repo, "mv", source, target)
    git(repo, "add", "-A")
    git(
        repo,
        "-c",
        "user.name=reviewer-pilot",
        "-c",
        "user.email=reviewer@example.invalid",
        "commit",
        "-q",
        "-m",
        "candidate",
    )
    return repo


def write_key(out: Path) -> dict:
    key = {
        "schema_version": "1.0",
        "written_before_runs": True,
        "reviewers": REVIEWERS,
        "review_kind": "agent (pi durable sessions); not a human study",
        "fixtures": {
            name: {
                "snapshot_id": json.loads(
                    (out / "fixtures" / name / "init.json").read_text(encoding="utf-8")
                )["snapshot_id"],
                "key": spec["key"],
                "task": spec["task"],
                "answer_shape": spec["answer_shape"],
            }
            for name, spec in FIXTURES.items()
        },
    }
    (out / "key.json").write_text(json.dumps(key, indent=2), encoding="utf-8")
    return key


def prompt_for(name: str, arm: str, snapshot: str) -> str:
    spec = FIXTURES[name]
    tools = (
        "You may use read-only ARCHMELD MCP tools: archmeld_brief(recipe=change), "
        "archmeld_review, archmeld_query, archmeld_source. Candidate relations are "
        "hypotheses, not resolved calls."
        if arm == "b"
        else "Use only the repository, git, and file reads. Do not look for or use "
        "ARCHMELD tools."
    )
    shape = json.dumps(spec["answer_shape"], indent=2)
    return f"""You are an independent code reviewer. This is a read-only review.

Repository: the current working directory.
Base snapshot: {snapshot}
Review the last commit: `git show HEAD` / `git diff HEAD^ HEAD`.

Task: {spec["task"]}

{tools}

Do not edit or write files. Do not run project tests. Cite a workspace-relative
path and line for each factual claim. If the evidence does not establish a
write, say "unavailable"; do not infer runtime behavior from source shape.

Return ONLY one JSON object as your final message, with this answer shape plus
"citations" ([{{"path": "...", "line": 1}}]), "confidence"
("high"|"medium"|"low"), and "notes":

{shape}
"""


def extract_json(text: str | None) -> dict | None:
    if not text:
        return None
    candidates = re.findall(r"```(?:json)?\s*(\{.*?\})\s*```", text, re.S)
    candidates.append(text)
    for candidate in candidates:
        start = candidate.find("{")
        while start != -1:
            depth = 0
            for index in range(start, len(candidate)):
                char = candidate[index]
                if char == "{":
                    depth += 1
                elif char == "}":
                    depth -= 1
                    if depth == 0:
                        try:
                            value = json.loads(candidate[start : index + 1])
                        except json.JSONDecodeError:
                            break
                        if isinstance(value, dict):
                            return value
                        break
            start = candidate.find("{", start + 1)
    return None


def prepare_run(out: Path, fixture: str, reviewer: str, arm: str) -> dict:
    run_id = f"{reviewer}-{fixture}-{arm}"
    run_dir = out / "runs" / run_id
    if run_dir.exists():
        force_rmtree(run_dir)
    repo = run_dir / "repo"
    shutil.copytree(out / "fixtures" / fixture / "repo", repo)
    # Copy the base state so MCP describes the base snapshot, not the candidate.
    state = run_dir / "state"
    shutil.copytree(out / "fixtures" / fixture / "base-state", state)
    snapshot = json.loads(
        (out / "fixtures" / fixture / "init.json").read_text(encoding="utf-8")
    )["snapshot_id"]
    if arm == "b":
        audit = run_dir / "audit.jsonl"
        mcp_config = {
            "mcpServers": {
                "archmeld": {
                    "command": str(MCP),
                    "args": [
                        "--state",
                        str(state),
                        "--project",
                        f"project.reviewer.{fixture}",
                        "--audit",
                        str(audit),
                    ],
                    "exposure": "direct",
                    "toolExposure": {"*": "direct"},
                    "description": (
                        "Read-only ARCHMELD evidence. Candidate relations are "
                        "not resolved calls."
                    ),
                }
            }
        }
        (repo / ".pi").mkdir(exist_ok=True)
        (repo / ".pi" / "mcp.json").write_text(
            json.dumps(mcp_config, indent=2), encoding="utf-8"
        )
    prompt = prompt_for(fixture, arm, snapshot)
    (run_dir / "prompt.txt").write_text(prompt, encoding="utf-8")
    return {
        "run_id": run_id,
        "run_dir": run_dir,
        "repo": repo,
        "state": state,
        "snapshot": snapshot,
        "arm": arm,
        "fixture": fixture,
        "reviewer": reviewer,
        "prompt": prompt,
    }


def run_pi(run: dict) -> dict:
    client = e4a.find_client("pi")
    cli = e4a.pi_cli_js()
    node = shutil.which("node")
    if not client or not cli or not node:
        raise SystemExit("pi/node not found")
    session_id = str(uuid.uuid4())
    session_dir = run["run_dir"] / "pi-sessions"
    session_dir.mkdir(exist_ok=True)
    tools = "read,bash,mcp__*" if run["arm"] == "b" else "read,bash"
    command = [
        node,
        str(cli),
        "-p",
        "--mode",
        "json",
        "--model",
        REVIEWERS[run["reviewer"]],
        "--thinking",
        THINKING,
        "--session-dir",
        str(session_dir),
        "--session-id",
        session_id,
        "--name",
        run["run_id"],
        "--approve",
        "--no-context-files",
        "--no-extensions",
    ]
    if run["arm"] == "b":
        command += ["-e", "builtin:mcp"]
    else:
        command += ["--no-mcp"]
    command += [
        "--no-skills",
        "--no-prompt-templates",
        "--no-themes",
        "--tools",
        tools,
    ]
    env = dict(os.environ)
    env["PI_TELEMETRY"] = "0"
    started = time.time()
    try:
        proc = subprocess.run(
            command,
            cwd=str(run["repo"]),
            input=run["prompt"],
            text=True,
            encoding="utf-8",
            errors="replace",
            capture_output=True,
            timeout=TIMEOUT,
            env=env,
        )
        stdout, stderr, exit_code, timed_out = (
            proc.stdout,
            proc.stderr,
            proc.returncode,
            False,
        )
    except subprocess.TimeoutExpired as error:
        stdout = error.stdout or ""
        stderr = error.stderr or ""
        exit_code = None
        timed_out = True
    (run["run_dir"] / "stdout.jsonl").write_text(e4a.redact(stdout), encoding="utf-8")
    (run["run_dir"] / "stderr.log").write_text(e4a.redact(stderr), encoding="utf-8")
    calls, shell, edits, result = e4a.parse_pi_stream(stdout, run["run_dir"])
    answer = extract_json(result.get("text"))
    (run["run_dir"] / "answer.json").write_text(
        json.dumps(answer, indent=2, ensure_ascii=False) if answer else "null",
        encoding="utf-8",
    )
    return {
        "reviewer": run["reviewer"],
        "model": REVIEWERS[run["reviewer"]],
        "fixture": run["fixture"],
        "arm": run["arm"],
        "snapshot_id": run["snapshot"],
        "started_at": time.strftime("%Y-%m-%dT%H:%M:%SZ", time.gmtime(started)),
        "duration_ms": int((time.time() - started) * 1000),
        "exit_code": exit_code,
        "timed_out": timed_out,
        "answer": answer,
        "calls": calls,
        "shell": shell,
        "edits": edits,
        "usage": result.get("usage"),
        "session_id": session_id,
        "session_dir": str(session_dir),
        "prompt_sha256": hashlib.sha256(run["prompt"].encode("utf-8")).hexdigest(),
    }


def normalize(text: str) -> str:
    return re.sub(r"[^a-z0-9_.:/-]+", "", (text or "").lower())


def score(answer: dict | None, key: dict, repo: Path) -> dict:
    result = {
        "matched": False,
        "owner_ok": False,
        "paths_ok": False,
        "labels_ok": False,
        "citations_ok": False,
        "unsupported": False,
        "reasons": [],
    }
    if not answer:
        result["reasons"].append("no parsable JSON answer")
        return result
    text = normalize(json.dumps(answer))
    result["owner_ok"] = all(normalize(entity) in text for entity in key["entities"])
    result["paths_ok"] = all(normalize(path) in text for path in key["paths"])
    labels_ok = True
    if key["writes"] != "not_applicable":
        declared = normalize(str(answer.get("writes_evidence", "")))
        labels_ok &= declared == key["writes"]
        if key["writes"] == "unavailable" and declared in {"present", "observed", "resolved"}:
            result["unsupported"] = True
    if key["relation"] != "not_applicable":
        declared = normalize(
            str(answer.get("incoming_resolution") or answer.get("relation") or "")
        )
        if key["relation"] == "candidate":
            labels_ok &= declared in {"candidate", "hypothesis", "unresolved"}
            if declared in {"resolved", "resolvedcall", "observed"}:
                result["unsupported"] = True
    if "caller_changed" in key:
        declared = answer.get("caller_changed")
        if isinstance(declared, str):
            declared = declared.strip().lower() in {"true", "yes", "1"}
        labels_ok &= bool(declared) is key["caller_changed"]
    if key.get("unmapped_paths"):
        unmapped = normalize(json.dumps(answer.get("unmapped_paths", [])))
        labels_ok &= all(normalize(path) in unmapped for path in key["unmapped_paths"])
    if key["coverage"] != "any":
        declared = normalize(str(answer.get("coverage", "")))
        labels_ok &= declared == key["coverage"]
    result["labels_ok"] = bool(labels_ok)
    citations = answer.get("citations") or []
    citation_ok = bool(citations)
    for citation in citations if isinstance(citations, list) else []:
        path = citation.get("path") if isinstance(citation, dict) else None
        if not path or not (repo / path).is_file():
            citation_ok = False
    result["citations_ok"] = citation_ok
    result["matched"] = (
        result["owner_ok"]
        and result["paths_ok"]
        and result["labels_ok"]
        and result["citations_ok"]
        and not result["unsupported"]
    )
    return result


def run_all(out: Path) -> int:
    if not (out / "key.json").is_file():
        raise SystemExit("run --prepare first")
    key = json.loads((out / "key.json").read_text(encoding="utf-8"))
    records = []
    rng = random.Random(20261128)
    for fixture in FIXTURES:
        for reviewer in REVIEWERS:
            arms = ["a", "b"]
            rng.shuffle(arms)
            for arm in arms:
                prepared = prepare_run(out, fixture, reviewer, arm)
                record = run_pi(prepared)
                record["score"] = score(
                    record["answer"], key["fixtures"][fixture]["key"], prepared["repo"]
                )
                (prepared["run_dir"] / "run.json").write_text(
                    json.dumps(record, indent=2, ensure_ascii=False), encoding="utf-8"
                )
                records.append(record)
                print(
                    json.dumps(
                        {
                            "run": prepared["run_id"],
                            "matched": record["score"]["matched"],
                            "duration_ms": record["duration_ms"],
                            "calls": len(record["calls"]),
                            "shell": len(record["shell"]),
                            "unsupported": record["score"]["unsupported"],
                        }
                    ),
                    flush=True,
                )
    (out / "runs.json").write_text(
        json.dumps(records, indent=2, ensure_ascii=False), encoding="utf-8"
    )
    return 0


def aggregate(out: Path) -> int:
    records = json.loads((out / "runs.json").read_text(encoding="utf-8"))
    lines = [
        "# Reviewer pilot (agent reviewers)",
        "",
        "Status: run with two durable `pi` sessions backed by different model",
        "families. This is **not** a human reviewer study and does not claim one.",
        "",
        "| reviewer | model | fixture | arm | matched | owner | paths | labels | citations | unsupported | duration_ms | calls |",
        "|---|---|---|---|---|---|---|---|---|---|---|---|",
    ]
    for record in records:
        score = record["score"]
        lines.append(
            "| {reviewer} | `{model}` | {fixture} | {arm} | {matched} | {owner_ok} | "
            "{paths_ok} | {labels_ok} | {citations_ok} | {unsupported} | {duration} | {calls} |".format(
                reviewer=record["reviewer"],
                model=record["model"],
                fixture=record["fixture"],
                arm=record["arm"],
                matched=score["matched"],
                owner_ok=score["owner_ok"],
                paths_ok=score["paths_ok"],
                labels_ok=score["labels_ok"],
                citations_ok=score["citations_ok"],
                unsupported=score["unsupported"],
                duration=record["duration_ms"],
                calls=len(record["calls"]),
            )
        )
    lines += [
        "",
        "## Limitations",
        "",
        "- Agent reviewers, not humans: no human time-to-owner or human judgment.",
        "- Small sample (3 fixtures x 2 arms x 2 reviewers); descriptive only, no",
        "  statistical significance.",
        "- Scoring is deterministic against the pre-registered key and is blind to",
        "  the arm; there is no second human scorer.",
        "- A negative or mixed result is reported as-is.",
        "",
    ]
    (out / "RESULT.md").write_text("\n".join(lines), encoding="utf-8")
    print(json.dumps({"runs": len(records), "result": str(out / "RESULT.md")}))
    return 0


def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("--out", default=str(DEFAULT_OUT))
    parser.add_argument("--prepare", action="store_true")
    parser.add_argument("--run-all", action="store_true")
    parser.add_argument("--score", action="store_true")
    args = parser.parse_args()
    out = Path(args.out)
    out.mkdir(parents=True, exist_ok=True)
    if args.prepare:
        for name in FIXTURES:
            prepare_fixture_root(out, name)
        write_key(out)
        print(json.dumps({"prepared": list(FIXTURES), "key": str(out / "key.json")}))
        return 0
    if args.run_all:
        return run_all(out)
    if args.score:
        return aggregate(out)
    parser.error("choose --prepare, --run-all or --score")
    return 2


if __name__ == "__main__":
    raise SystemExit(main())
