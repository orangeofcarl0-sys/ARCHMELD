#!/usr/bin/env python3
"""E4a launcher: run one A/B arm in a disposable fixture with a real client.

The launcher prepares an isolated repo + ARCHMELD state, registers the real
`archmeld-mcp` through the client's own MCP configuration, runs the client
non-interactively, and records the client stream plus the server audit log.

Backends: `pi` (durable session, project-local `.pi/mcp.json` with direct MCP
exposure), `claude` (per-run `--mcp-config` with `--strict-mcp-config`), and
`codex` (probed; not exercised here). Pi runs use `--no-extensions -e
builtin:mcp` so broken global extensions do not block the run and MCP stays
enabled. Model keys are never placed in argv or logs.
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
import uuid
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]
DEFAULT_ARCHMELD = ROOT / "target" / "debug" / "archmeld.exe"
DEFAULT_MCP = ROOT / "target" / "debug" / "archmeld-mcp.exe"
DEFAULT_HELPER = ROOT / "target" / "debug" / "examples" / "archmeld_export.exe"
DEFAULT_PI_MODEL = "omni-anthropic/deepseek-v4.1-flash"
PROMPTS = ROOT / "tooling" / "e4a_prompts"
FIXTURES = ROOT / "fixtures" / "e4a"
SECRET_PATTERNS = [
    re.compile(r"sk-[A-Za-z0-9_-]{8,}"),
    re.compile(r"ghp_[A-Za-z0-9]{8,}"),
    re.compile(r"github_pat_[A-Za-z0-9_]{8,}"),
    re.compile(r"xox[bp]-[A-Za-z0-9-]{8,}"),
    re.compile(r"Bearer\s+[A-Za-z0-9._-]{8,}"),
]


def redact(text: str) -> str:
    for pattern in SECRET_PATTERNS:
        text = pattern.sub("[REDACTED]", text)
    return text


def sha256_bytes(data: bytes) -> str:
    return "sha256:" + hashlib.sha256(data).hexdigest()


def sha256_file(path: Path) -> str:
    return sha256_bytes(path.read_bytes())


def tree_hashes(root: Path) -> dict[str, str]:
    result: dict[str, str] = {}
    for path in sorted(root.rglob("*")):
        if path.is_file() and ".git" not in path.parts:
            result[path.relative_to(root).as_posix()] = sha256_file(path)
    return result


def run_cmd(
    argv: list[str],
    cwd: Path,
    timeout: int,
    stdin_text: str | None = None,
) -> dict:
    started = time.time()
    try:
        proc = subprocess.run(
            argv,
            cwd=str(cwd),
            input=stdin_text,
            text=True,
            encoding="utf-8",
            errors="replace",
            capture_output=True,
            timeout=timeout,
        )
        return {
            "argv": argv,
            "exit_code": proc.returncode,
            "stdout": proc.stdout,
            "stderr": proc.stderr,
            "duration_ms": int((time.time() - started) * 1000),
            "timed_out": False,
        }
    except subprocess.TimeoutExpired as error:
        return {
            "argv": argv,
            "exit_code": None,
            "stdout": error.stdout or "",
            "stderr": error.stderr or "",
            "duration_ms": int((time.time() - started) * 1000),
            "timed_out": True,
        }


def find_client(name: str) -> str | None:
    if os.name == "nt":
        for candidate in (f"{name}.cmd", f"{name}.exe", name):
            found = shutil.which(candidate)
            if found:
                return found
        return None
    return shutil.which(name)


def pi_cli_js() -> Path | None:
    """Resolve pi's JS entry so we do not run it through cmd.exe.

    `cmd /c pi.cmd` leaves the node child holding the pipes when a timeout
    kills only cmd, so the launcher would hang instead of recording a timeout.
    """
    client = find_client("pi")
    if not client:
        return None
    candidate = (
        Path(client).parent
        / "node_modules"
        / "@earendil-works"
        / "pi-coding-agent"
        / "dist"
        / "bundle"
        / "cli.js"
    )
    return candidate if candidate.is_file() else None


def probe() -> dict:
    result: dict[str, object] = {}
    for name in ("pi", "claude", "codex"):
        path = find_client(name)
        item: dict[str, object] = {"found": bool(path), "path": path}
        if path:
            version = run_cmd([path, "--version"], ROOT, 30)
            item["version"] = (version["stdout"] or version["stderr"]).strip().splitlines()[:1]
            item["version_exit"] = version["exit_code"]
        result[name] = item
    return result


def copy_fixture(fixture: Path, repo: Path) -> None:
    if repo.exists():
        shutil.rmtree(repo)
    shutil.copytree(
        fixture,
        repo,
        ignore=shutil.ignore_patterns("map.template.txt", "__pycache__", "*.pyc"),
    )


def prepare(args: argparse.Namespace) -> dict:
    fixture = (
        Path(args.fixture).resolve()
        if Path(args.fixture).is_dir()
        else FIXTURES / args.fixture
    )
    if not fixture.is_dir():
        raise SystemExit(f"fixture not found: {fixture}")
    run_id = args.run_id or f"{args.fixture}-{args.arm}-{time.strftime('%Y%m%d-%H%M%S')}-{uuid.uuid4().hex[:6]}"
    run_dir = Path(args.out).resolve() / "runs" / run_id
    repo = run_dir / "repo"
    state = run_dir / "state"
    audit = run_dir / "audit.jsonl"
    run_dir.mkdir(parents=True, exist_ok=True)
    copy_fixture(fixture, repo)

    archmeld = Path(args.archmeld_bin).resolve()
    mcp = Path(args.mcp_bin).resolve()
    if not archmeld.is_file() or not mcp.is_file():
        raise SystemExit(f"missing binaries: {archmeld} or {mcp}; build first")

    init_argv = [
        str(archmeld),
        "init",
        "--workspace",
        str(repo),
        "--state",
        str(state),
        "--project",
        args.project,
        "--client",
        "generic",
    ]
    if args.index_helper:
        init_argv.extend(["--index-helper", str(Path(args.index_helper).resolve())])
    init = run_cmd(init_argv, ROOT, args.timeout)
    (run_dir / "init.json").write_text(init["stdout"], encoding="utf-8")
    (run_dir / "init.err").write_text(init["stderr"], encoding="utf-8")
    if init["exit_code"] != 0:
        raise SystemExit(f"archmeld init failed: {init['exit_code']}\n{init['stderr']}")
    try:
        init_json = json.loads(init["stdout"])
    except json.JSONDecodeError as error:
        raise SystemExit(f"init output is not JSON: {error}") from error
    snapshot = init_json.get("snapshot_id")
    if not snapshot:
        raise SystemExit(f"init output has no snapshot_id: {init_json}")

    map_check = None
    if args.arm == "b":
        template = (fixture / "map.template.txt").read_text(encoding="utf-8")
        (repo / "archmeld.map.txt").write_text(
            template.replace("{{SNAPSHOT}}", snapshot), encoding="utf-8"
        )
        check = run_cmd(
            [
                str(archmeld),
                "map",
                "check",
                "--state",
                str(state),
                "--project",
                args.project,
                "--workspace",
                str(repo),
            ],
            ROOT,
            args.timeout,
        )
        map_check = check
        (run_dir / "map-check.json").write_text(check["stdout"], encoding="utf-8")
        (run_dir / "map-check.err").write_text(check["stderr"], encoding="utf-8")
        if check["exit_code"] != 0:
            raise SystemExit(f"map check failed: {check['exit_code']}\n{check['stderr']}")

    mcp_config = {
        "mcpServers": {
            "archmeld": {
                "command": str(mcp),
                "args": [
                    "--state",
                    str(state),
                    "--project",
                    args.project,
                    "--audit",
                    str(audit),
                ],
            }
        }
    }
    (run_dir / "mcp.json").write_text(json.dumps(mcp_config, indent=2), encoding="utf-8")

    # Pi reads project MCP servers from .pi/mcp.json after --approve. Direct
    # tool exposure is required; with the default codemode exposure the model
    # sees only read/bash and falls back to hand-written JSON-RPC.
    pi_server = dict(mcp_config["mcpServers"]["archmeld"])
    pi_server["exposure"] = "direct"
    pi_server["toolExposure"] = {"*": "direct"}
    pi_server["description"] = (
        "Read-only ARCHMELD evidence: status, brief, review, query, context, "
        "source, inspect. Candidate relations are not resolved calls."
    )
    (repo / ".pi").mkdir(exist_ok=True)
    (repo / ".pi" / "mcp.json").write_text(
        json.dumps({"mcpServers": {"archmeld": pi_server}}, indent=2),
        encoding="utf-8",
    )

    if args.prompt_file:
        prompt = Path(args.prompt_file).read_text(encoding="utf-8")
    else:
        task = (PROMPTS / "task.md").read_text(encoding="utf-8")
        rules = (PROMPTS / f"rules_{args.arm}.md").read_text(encoding="utf-8")
        prompt = task.replace("<RULES>", rules)
    (run_dir / "prompt.txt").write_text(prompt, encoding="utf-8")

    initial = tree_hashes(repo)
    (run_dir / "initial-tree.json").write_text(json.dumps(initial, indent=2), encoding="utf-8")

    return {
        "run_id": run_id,
        "run_dir": run_dir,
        "repo": repo,
        "state": state,
        "audit": audit,
        "snapshot": snapshot,
        "prompt": prompt,
        "mcp_config": run_dir / "mcp.json",
        "init": init_json,
        "map_check": map_check,
        "initial_tree": initial,
        "archmeld": archmeld,
        "mcp": mcp,
    }


def parse_claude_stream(
    stdout: str, run_dir: Path
) -> tuple[list[dict], list[dict], list[dict], dict | None]:
    calls: list[dict] = []
    shell: list[dict] = []
    edits: list[dict] = []
    result: dict | None = None
    seq = 0
    for line in stdout.splitlines():
        line = line.strip()
        if not line:
            continue
        try:
            event = json.loads(line)
        except json.JSONDecodeError:
            continue
        if event.get("type") == "assistant":
            for block in event.get("message", {}).get("content", []):
                if block.get("type") != "tool_use":
                    continue
                seq += 1
                name = block.get("name", "")
                args = block.get("input", {})
                if name.startswith("mcp__archmeld__"):
                    calls.append(
                        {
                            "seq": seq,
                            "tool": name.split("__")[-1],
                            "args": args,
                        }
                    )
                elif name == "Bash":
                    shell.append({"seq": seq, "command": args.get("command")})
                elif name in ("Edit", "Write", "MultiEdit"):
                    edits.append(
                        {
                            "seq": seq,
                            "tool": name,
                            "file_path": args.get("file_path")
                            or args.get("path")
                            or args.get("target_file"),
                        }
                    )
        elif event.get("type") == "result":
            result = event
    for item in calls + shell + edits:
        item["seq"] = item["seq"]
    (run_dir / "calls.jsonl").write_text(
        "\n".join(json.dumps(item, ensure_ascii=False) for item in calls) + ("\n" if calls else ""),
        encoding="utf-8",
    )
    (run_dir / "shell.jsonl").write_text(
        "\n".join(json.dumps(item, ensure_ascii=False) for item in shell) + ("\n" if shell else ""),
        encoding="utf-8",
    )
    (run_dir / "edits.jsonl").write_text(
        "\n".join(json.dumps(item, ensure_ascii=False) for item in edits) + ("\n" if edits else ""),
        encoding="utf-8",
    )
    return calls, shell, edits, result


def run_claude(args: argparse.Namespace, prepared: dict) -> dict:
    client = find_client("claude")
    if not client:
        raise SystemExit("claude CLI not found; install it or use --backend codex")
    command = [
        client,
        "-p",
        "--mcp-config",
        str(prepared["mcp_config"]),
        "--strict-mcp-config",
        "--output-format",
        "stream-json",
        "--verbose",
        "--permission-mode",
        "bypassPermissions",
        "--no-session-persistence",
    ]
    if args.max_budget_usd is not None:
        command.extend(["--max-budget-usd", str(args.max_budget_usd)])
    if os.name == "nt":
        command = ["cmd", "/c", *command]
    env = dict(os.environ)
    env.update(
        {
            "CLAUDE_CODE_DISABLE_NONESSENTIAL_TRAFFIC": "1",
            "DISABLE_TELEMETRY": "1",
            "DISABLE_ERROR_REPORTING": "1",
            "DISABLE_AUTOUPDATER": "1",
        }
    )
    started = time.time()
    try:
        proc = subprocess.run(
            command,
            cwd=str(prepared["repo"]),
            input=prepared["prompt"],
            text=True,
            encoding="utf-8",
            errors="replace",
            capture_output=True,
            timeout=args.timeout,
            env=env,
        )
        stdout, stderr, exit_code, timed_out = proc.stdout, proc.stderr, proc.returncode, False
    except subprocess.TimeoutExpired as error:
        stdout = error.stdout or ""
        stderr = error.stderr or ""
        exit_code = None
        timed_out = True
    (prepared["run_dir"] / "stdout.jsonl").write_text(redact(stdout), encoding="utf-8")
    (prepared["run_dir"] / "stderr.log").write_text(redact(stderr), encoding="utf-8")
    calls, shell, edits, result = parse_claude_stream(stdout, prepared["run_dir"])
    return {
        "backend": "claude",
        "client": client,
        "argv": command,
        "exit_code": exit_code,
        "timed_out": timed_out,
        "duration_ms": int((time.time() - started) * 1000),
        "calls": calls,
        "shell": shell,
        "edits": edits,
        "result": result,
        "usage": (result or {}).get("usage"),
        "total_cost_usd": (result or {}).get("total_cost_usd"),
        "num_turns": (result or {}).get("num_turns"),
    }


def parse_pi_stream(
    stdout: str, run_dir: Path
) -> tuple[list[dict], list[dict], list[dict], dict]:
    calls: list[dict] = []
    shell: list[dict] = []
    edits: list[dict] = []
    usage = {"input": 0, "output": 0, "cacheRead": 0, "cacheWrite": 0, "totalTokens": 0}
    final_text: str | None = None
    model: str | None = None
    provider: str | None = None
    thinking: str | None = None
    seq = 0
    for line in stdout.splitlines():
        line = line.strip()
        if not line:
            continue
        try:
            event = json.loads(line)
        except json.JSONDecodeError:
            continue
        if event.get("type") != "message_end":
            continue
        message = event.get("message", {})
        if message.get("role") != "assistant":
            continue
        block_usage = message.get("usage") or {}
        for key in usage:
            usage[key] += int(block_usage.get(key) or 0)
        model = message.get("model") or model
        provider = message.get("provider") or provider
        thinking = message.get("thinkingLevel") or thinking
        for block in message.get("content", []):
            if block.get("type") == "toolCall":
                seq += 1
                name = block.get("name", "")
                arguments = block.get("arguments", {})
                if name.startswith("mcp__archmeld__"):
                    calls.append(
                        {"seq": seq, "tool": name.split("__")[-1], "args": arguments}
                    )
                elif name == "bash":
                    shell.append({"seq": seq, "command": arguments.get("command")})
                elif name in ("edit", "write"):
                    edits.append(
                        {
                            "seq": seq,
                            "tool": name,
                            "file_path": arguments.get("path")
                            or arguments.get("file_path"),
                        }
                    )
            elif block.get("type") == "text":
                final_text = block.get("text") or final_text
    (run_dir / "calls.jsonl").write_text(
        "\n".join(json.dumps(item, ensure_ascii=False) for item in calls)
        + ("\n" if calls else ""),
        encoding="utf-8",
    )
    (run_dir / "shell.jsonl").write_text(
        "\n".join(json.dumps(item, ensure_ascii=False) for item in shell)
        + ("\n" if shell else ""),
        encoding="utf-8",
    )
    (run_dir / "edits.jsonl").write_text(
        "\n".join(json.dumps(item, ensure_ascii=False) for item in edits)
        + ("\n" if edits else ""),
        encoding="utf-8",
    )
    result = {
        "text": final_text,
        "usage": usage,
        "model": model,
        "provider": provider,
        "thinking_level": thinking,
    }
    return calls, shell, edits, result


def run_pi(args: argparse.Namespace, prepared: dict) -> dict:
    client = find_client("pi")
    cli = pi_cli_js()
    node = shutil.which("node")
    if not client or not cli or not node:
        raise SystemExit("pi CLI/node not found; install pi or use --backend claude")
    session_id = str(uuid.uuid4())
    session_dir = prepared["run_dir"] / "pi-sessions"
    session_dir.mkdir(exist_ok=True)
    command = [
        node,
        str(cli),
        "-p",
        "--mode",
        "json",
        "--model",
        args.pi_model,
        "--session-dir",
        str(session_dir),
        "--session-id",
        session_id,
        "--name",
        prepared["run_id"],
        "--approve",
        "--no-context-files",
        "--no-extensions",
        "-e",
        "builtin:mcp",
        "--no-skills",
        "--no-prompt-templates",
        "--no-themes",
        "--tools",
        "read,bash,edit,write,mcp__*",
    ]
    env = dict(os.environ)
    env.update({"PI_TELEMETRY": "0"})
    started = time.time()
    try:
        proc = subprocess.run(
            command,
            cwd=str(prepared["repo"]),
            input=prepared["prompt"],
            text=True,
            encoding="utf-8",
            errors="replace",
            capture_output=True,
            timeout=args.timeout,
            env=env,
        )
        stdout, stderr, exit_code, timed_out = proc.stdout, proc.stderr, proc.returncode, False
    except subprocess.TimeoutExpired as error:
        stdout = error.stdout or ""
        stderr = error.stderr or ""
        exit_code = None
        timed_out = True
    (prepared["run_dir"] / "stdout.jsonl").write_text(redact(stdout), encoding="utf-8")
    (prepared["run_dir"] / "stderr.log").write_text(redact(stderr), encoding="utf-8")
    calls, shell, edits, result = parse_pi_stream(stdout, prepared["run_dir"])
    return {
        "backend": "pi",
        "client": client,
        "argv": command,
        "exit_code": exit_code,
        "timed_out": timed_out,
        "duration_ms": int((time.time() - started) * 1000),
        "calls": calls,
        "shell": shell,
        "edits": edits,
        "result": result,
        "usage": result.get("usage"),
        "total_cost_usd": None,
        "num_turns": None,
        "session_id": session_id,
        "session_dir": str(session_dir),
    }


def run_codex(args: argparse.Namespace, prepared: dict) -> dict:
    client = find_client("codex")
    if not client:
        raise SystemExit("codex CLI not found; install it or use --backend claude")
    raise SystemExit(
        "codex backend is not exercised in this environment; the adapter is "
        "intentionally blocked until `codex exec` and its MCP config contract "
        "are probed on an installed client"
    )


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--backend", choices=["auto", "pi", "claude", "codex"], default="auto")
    parser.add_argument("--probe", action="store_true")
    parser.add_argument("--dry-run", action="store_true")
    parser.add_argument("--prepare-only", action="store_true")
    parser.add_argument("--fixture", default="nonlexical")
    parser.add_argument("--arm", choices=["a", "b"], required=False)
    parser.add_argument("--out", default=str(ROOT / "docs" / "evidence" / "e4a-2026-11-25"))
    parser.add_argument("--run-id")
    parser.add_argument("--prompt-file")
    parser.add_argument("--project", default="project.e4a")
    parser.add_argument("--archmeld-bin", default=str(DEFAULT_ARCHMELD))
    parser.add_argument("--mcp-bin", default=str(DEFAULT_MCP))
    parser.add_argument(
        "--index-helper",
        default=str(DEFAULT_HELPER) if DEFAULT_HELPER.is_file() else None,
    )
    parser.add_argument("--timeout", type=int, default=900)
    parser.add_argument("--max-budget-usd", type=float)
    parser.add_argument("--pi-model", default=DEFAULT_PI_MODEL)
    args = parser.parse_args()

    if args.probe:
        print(json.dumps(probe(), indent=2))
        return 0
    if not args.arm:
        parser.error("--arm is required unless --probe is used")

    if args.dry_run:
        fixture = (
            Path(args.fixture).resolve()
            if Path(args.fixture).is_dir()
            else FIXTURES / args.fixture
        )
        print(
            json.dumps(
                {
                    "fixture": str(fixture),
                    "arm": args.arm,
                    "out": str(Path(args.out) / "runs" / (args.run_id or "<generated>")),
                    "backend": args.backend,
                    "pi": find_client("pi"),
                    "claude": find_client("claude"),
                    "codex": find_client("codex"),
                },
                indent=2,
            )
        )
        return 0

    prepared = prepare(args)
    if args.prepare_only:
        print(
            json.dumps(
                {
                    "run_dir": str(prepared["run_dir"]),
                    "snapshot_id": prepared["snapshot"],
                    "map_checked": prepared["map_check"] is not None,
                    "map_state": (
                        json.loads(prepared["map_check"]["stdout"]).get("state")
                        if prepared["map_check"]
                        and prepared["map_check"]["stdout"].strip()
                        else None
                    ),
                },
                indent=2,
            )
        )
        return 0
    backend = args.backend
    if backend == "auto":
        if find_client("pi"):
            backend = "pi"
        elif find_client("claude"):
            backend = "claude"
        else:
            backend = "codex"
    if backend == "pi":
        outcome = run_pi(args, prepared)
    elif backend == "claude":
        outcome = run_claude(args, prepared)
    else:
        outcome = run_codex(args, prepared)

    final = tree_hashes(prepared["repo"])
    (prepared["run_dir"] / "final-tree.json").write_text(
        json.dumps(final, indent=2), encoding="utf-8"
    )
    changed = sorted(
        path
        for path in set(prepared["initial_tree"]) | set(final)
        if prepared["initial_tree"].get(path) != final.get(path)
    )
    run = {
        "schema_version": "1.0",
        "run_id": prepared["run_id"],
        "fixture": args.fixture,
        "arm": args.arm,
        "backend": outcome["backend"],
        "client": outcome.get("client"),
        "exit_code": outcome["exit_code"],
        "timed_out": outcome["timed_out"],
        "duration_ms": outcome["duration_ms"],
        "snapshot_id": prepared["snapshot"],
        "map_checked": prepared["map_check"] is not None,
        "prompt_sha256": sha256_bytes(prepared["prompt"].encode("utf-8")),
        "map_sha256": (
            sha256_file(prepared["repo"] / "archmeld.map.txt")
            if (prepared["repo"] / "archmeld.map.txt").is_file()
            else None
        ),
        "initial_tree": prepared["initial_tree"],
        "final_tree": final,
        "changed_files": changed,
        "usage": outcome.get("usage"),
        "total_cost_usd": outcome.get("total_cost_usd"),
        "num_turns": outcome.get("num_turns"),
        "mcp_calls": len(outcome["calls"]),
        "shell_calls": len(outcome["shell"]),
        "edits": len(outcome["edits"]),
        "session_id": outcome.get("session_id"),
        "session_dir": outcome.get("session_dir"),
        "model": (outcome.get("result") or {}).get("model"),
        "provider": (outcome.get("result") or {}).get("provider"),
        "result": outcome.get("result"),
        "environment": {
            "os": os.name,
            "cwd": str(prepared["repo"]),
            "toolchain": os.environ.get("RUSTUP_TOOLCHAIN"),
            "client_version": probe().get(outcome["backend"]),
        },
    }
    (prepared["run_dir"] / "run.json").write_text(
        json.dumps(run, ensure_ascii=False, indent=2), encoding="utf-8"
    )
    print(json.dumps({"run_id": prepared["run_id"], "run_dir": str(prepared["run_dir"]), "changed": changed}, indent=2))
    return 0 if outcome["exit_code"] == 0 else 1


if __name__ == "__main__":
    raise SystemExit(main())
