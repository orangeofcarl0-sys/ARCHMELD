#!/usr/bin/env python3
"""Thin MCP shim for manual E4a runs and smoke tests.

Usage:
  python e4a_mcp_shim.py --config <run>/tool.json list
  python e4a_mcp_shim.py --config <run>/tool.json call archmeld_inspect '{"kind":"map_entry","id":"src/pricing.py"}'

The config is {"mcp": "...", "state": "...", "project": "...", "audit": "...", "calls": "..."}.
Each invocation starts a fresh `archmeld-mcp` process. The server audit log is
the authoritative tool-call record; calls.jsonl records this shim's view.
"""
from __future__ import annotations

import argparse
import json
import subprocess
import time
from pathlib import Path


def read_config(path: Path) -> dict:
    return json.loads(path.read_text(encoding="utf-8"))


def rpc_call(config: dict, method: str, params: dict, request_id: int) -> dict:
    mcp = config["mcp"]
    argv = [mcp, "--state", config["state"], "--project", config["project"]]
    if config.get("audit"):
        argv.extend(["--audit", config["audit"]])
    messages = [
        {
            "jsonrpc": "2.0",
            "id": 1,
            "method": "initialize",
            "params": {
                "protocolVersion": "2025-06-18",
                "capabilities": {},
                "clientInfo": {"name": "e4a-shim", "version": "1.0"},
            },
        },
        {"jsonrpc": "2.0", "method": "notifications/initialized", "params": {}},
        {"jsonrpc": "2.0", "id": request_id, "method": method, "params": params},
    ]
    payload = "\n".join(json.dumps(message) for message in messages) + "\n"
    started = time.time()
    proc = subprocess.run(
        argv,
        input=payload,
        text=True,
        encoding="utf-8",
        errors="replace",
        capture_output=True,
        timeout=120,
    )
    responses = []
    for line in proc.stdout.splitlines():
        line = line.strip()
        if not line:
            continue
        try:
            responses.append(json.loads(line))
        except json.JSONDecodeError:
            continue
    response = next(
        (item for item in responses if item.get("id") == request_id),
        {"error": {"message": f"no response for id {request_id}; stderr={proc.stderr[-500:]}"}},
    )
    return {
        "response": response,
        "duration_ms": int((time.time() - started) * 1000),
        "exit_code": proc.returncode,
    }


def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("--config", required=True)
    parser.add_argument("command", choices=["list", "call"])
    parser.add_argument("tool", nargs="?")
    parser.add_argument("args", nargs="?", default="{}")
    args = parser.parse_args()
    config = read_config(Path(args.config))
    if args.command == "list":
        result = rpc_call(config, "tools/list", {}, 2)
        response = result["response"]
        tools = response.get("result", {}).get("tools", [])
        print(json.dumps({"tools": [tool.get("name") for tool in tools]}, indent=2))
        return 0
    if not args.tool:
        parser.error("call requires a tool name")
    try:
        tool_args = json.loads(args.args)
    except json.JSONDecodeError as error:
        parser.error(f"args are not JSON: {error}")
    result = rpc_call(
        config,
        "tools/call",
        {"name": args.tool, "arguments": tool_args},
        2,
    )
    response = result["response"]
    content = response.get("result", {}).get("content", [])
    texts = [item.get("text", "") for item in content if item.get("type") == "text"]
    is_error = bool(response.get("result", {}).get("isError")) or "error" in response
    record = {
        "ts_ms": int(time.time() * 1000),
        "tool": args.tool,
        "args": tool_args,
        "ok": not is_error,
        "is_error": is_error,
        "content_items": len(content),
        "content_lengths": [len(text) for text in texts],
        "duration_ms": result["duration_ms"],
        "exit_code": result["exit_code"],
    }
    if config.get("calls"):
        with Path(config["calls"]).open("a", encoding="utf-8") as stream:
            stream.write(json.dumps(record, ensure_ascii=False) + "\n")
    if texts:
        print("\n".join(texts))
    else:
        print(json.dumps(response, ensure_ascii=False, indent=2))
    return 0 if not is_error else 1


if __name__ == "__main__":
    raise SystemExit(main())
