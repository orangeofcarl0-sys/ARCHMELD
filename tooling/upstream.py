#!/usr/bin/env python3
# Copyright 2026 ARCHMELD contributors
# SPDX-License-Identifier: Apache-2.0
"""Read-only upstream registry checks for ARCHMELD.

The registry (`upstreams.lock.json`) is the single source of truth for how each
external project is reused. This tool never merges, fetches, pushes or updates a
pin unless a future subcommand explicitly says so; `status`, `verify` and
`check` are read-only.

Run from the repository root:

    python tooling/upstream.py status --json
    python tooling/upstream.py verify --json --out docs/release-evidence/UPSTREAM-STATUS-YYYY-MM-DD.json
    python tooling/upstream.py check codegraph --json

U1 subcommands (`patches`, `diff`, `drill`, `record`) are added separately.
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
import tempfile
import time
from datetime import datetime, timezone
from pathlib import Path
from typing import Any

ROOT = Path(__file__).resolve().parents[1]
REGISTRY_PATH = ROOT / "upstreams.lock.json"
TOOLCHAIN_LOCK = ROOT / "toolchains.lock.json"
TEMP_BASE = Path("<temp>")

REUSE_MODES = {
    "thin_fork",
    "artifact_import",
    "process_adapter",
    "text_interop",
    "vendored_asset",
    "environment_pin",
}
PIN_KINDS = {"commit", "lockfile", "version"}
UPDATE_MODES = {
    "manual_merge_with_drill",
    "regenerate_artifact_and_run_contract_tests",
    "regenerate_fixture_and_run_decoder_tests",
    "version_probe_and_capability_profile",
    "regenerate_fixture_and_record_loss",
    "tolerant_text_fixture",
    "manual_vendor_with_license_hash",
    "doctor_and_relock",
}
COMMIT_RE = re.compile(r"^[0-9a-f]{40}(?:[0-9a-f]{24})?$")


def now_utc() -> str:
    return datetime.now(timezone.utc).isoformat()


def today() -> str:
    return datetime.now(timezone.utc).date().isoformat()


def sha256_bytes(data: bytes) -> str:
    return hashlib.sha256(data).hexdigest()


def sha256_text(text: str) -> str:
    return sha256_bytes(text.encode("utf-8"))


def sha256_file(path: Path) -> str:
    digest = hashlib.sha256()
    with path.open("rb") as handle:
        for chunk in iter(lambda: handle.read(1024 * 1024), b""):
            digest.update(chunk)
    return digest.hexdigest()


def read_json(path: Path) -> Any:
    return json.loads(path.read_text(encoding="utf-8"))


def write_json(path: Path, value: Any) -> None:
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_text(json.dumps(value, ensure_ascii=False, indent=2) + "\n", encoding="utf-8")


def subprocess_env() -> dict[str, str]:
    env = dict(os.environ)
    env.setdefault("PYTHONIOENCODING", "utf-8")
    if TEMP_BASE.is_dir():
        env.setdefault("TMP", str(TEMP_BASE))
        env.setdefault("TEMP", str(TEMP_BASE))
        env.setdefault("TMPDIR", str(TEMP_BASE))
    path_parts = env.get("PATH", "").split(os.pathsep)
    candidates = []
    cargo_bin = Path.home() / ".cargo" / "bin"
    if cargo_bin.is_dir():
        candidates.append(str(cargo_bin))
    strawberry = Path("D:/Strawberry/c/bin")
    if os.name == "nt" and strawberry.is_dir():
        candidates.append(str(strawberry))
    for candidate in reversed(candidates):
        if candidate not in path_parts:
            path_parts.insert(0, candidate)
    env["PATH"] = os.pathsep.join(path_parts)
    if os.name == "nt" and "RUSTUP_TOOLCHAIN" not in env:
        env["RUSTUP_TOOLCHAIN"] = "1.94.1-x86_64-pc-windows-gnu"
    return env


def run(
    argv: list[str],
    cwd: Path = ROOT,
    timeout: int = 120,
    text_limit: int | None = 65536,
) -> dict[str, Any]:
    """Run a command and record the evidence fields AGENTS.md requires.

    The environment is inherited but never dumped: only an allowlisted view is
    returned, so tokens or repository secrets cannot leak into logs.
    """
    env = subprocess_env()
    started_at = now_utc()
    started = time.monotonic()
    timed_out = False
    resolved = shutil.which(argv[0], path=env.get("PATH", ""))
    spawn_argv = [resolved, *argv[1:]] if resolved else argv
    try:
        completed = subprocess.run(
            spawn_argv,
            cwd=str(cwd),
            env=env,
            capture_output=True,
            timeout=timeout,
        )
        exit_code = completed.returncode
        stdout = completed.stdout
        stderr = completed.stderr
    except subprocess.TimeoutExpired as error:
        timed_out = True
        exit_code = -1
        stdout = error.stdout or b""
        stderr = error.stderr or b""
    except FileNotFoundError as error:
        exit_code = 127
        stdout = b""
        stderr = str(error).encode("utf-8", errors="replace")
    except OSError as error:
        exit_code = 126
        stdout = b""
        stderr = str(error).encode("utf-8", errors="replace")
    duration = time.monotonic() - started
    stdout_text = stdout.decode("utf-8", errors="replace")
    stderr_text = stderr.decode("utf-8", errors="replace")
    if text_limit is not None:
        stdout_text = stdout_text[-text_limit:]
        stderr_text = stderr_text[-text_limit:]
    return {
        "argv": argv,
        "cwd": str(cwd),
        "exit_code": exit_code,
        "timed_out": timed_out,
        "started_at": started_at,
        "finished_at": now_utc(),
        "duration_s": round(duration, 3),
        "stdout": stdout_text,
        "stderr": stderr_text,
        "stdout_sha256": sha256_bytes(stdout),
        "stderr_sha256": sha256_bytes(stderr),
    }


def tool_versions() -> dict[str, Any]:
    probes = {
        "git": ["git", "--version"],
        "cargo": ["cargo", "--version"],
        "rustc": ["rustc", "-vV"],
        "python": [sys.executable, "--version"],
    }
    result: dict[str, Any] = {}
    for name, argv in probes.items():
        record = run(argv, timeout=30)
        first_line = (record["stdout"] or record["stderr"]).strip().splitlines()
        result[name] = {
            "argv": argv,
            "exit_code": record["exit_code"],
            "first_line": first_line[0] if first_line else "",
        }
    return result


def resolve_pointer(value: Any, pointer: str) -> Any:
    if pointer in ("", "/"):
        return value
    current = value
    for raw_part in pointer.lstrip("/").split("/"):
        part = raw_part.replace("~1", "/").replace("~0", "~")
        if isinstance(current, list):
            try:
                current = current[int(part)]
            except (IndexError, ValueError) as error:
                raise KeyError(f"cannot resolve {pointer!r} at {part!r}") from error
        elif isinstance(current, dict):
            try:
                current = current[part]
            except KeyError:
                raise KeyError(f"cannot resolve {pointer!r} at {part!r}") from None
        else:
            raise KeyError(f"cannot resolve {pointer!r} at {part!r}")
    return current


def load_registry(path: Path = REGISTRY_PATH) -> dict[str, Any]:
    if not path.is_file():
        raise SystemExit(f"upstream registry not found: {path}")
    data = read_json(path)
    if not isinstance(data, dict):
        raise SystemExit(f"upstream registry is not a JSON object: {path}")
    return data


def upstreams_by_id(registry: dict[str, Any]) -> dict[str, dict[str, Any]]:
    return {entry["id"]: entry for entry in registry.get("upstreams", [])}


def upstream_scope_paths() -> list[str]:
    """Upstream-owned trees plus the root build/doc files."""
    paths = ["Cargo.toml", "Cargo.lock", "README.md"]
    for path in sorted((ROOT / "crates").glob("codegraph*")):
        if path.is_dir():
            paths.append(str(path.relative_to(ROOT)))
    return paths


def changed_upstream_paths(base: str) -> list[str]:
    result = run(["git", "diff", "--name-status", f"{base}..HEAD", "--", *upstream_scope_paths()], timeout=120)
    if result["exit_code"] != 0:
        raise RuntimeError(f"git diff failed: {result['stderr'].strip()}")
    changed: set[str] = set()
    for line in result["stdout"].splitlines():
        parts = line.split("\t")
        if len(parts) >= 2:
            changed.update(parts[1:])
    return sorted(changed)


def patch_check(base: str, manifest: dict[str, Any]) -> dict[str, Any]:
    changed = set(changed_upstream_paths(base))
    accounted = {path for entry in manifest.get("entries", []) for path in entry.get("paths", [])}
    unaccounted = sorted(changed - accounted)
    missing = sorted(accounted - changed)
    return {
        "base": base,
        "changed": sorted(changed),
        "accounted": sorted(accounted),
        "unaccounted_paths": unaccounted,
        "missing_paths": missing,
        "manifest_complete": not unaccounted and not missing,
    }


def patch_write(base: str, manifest: dict[str, Any]) -> dict[str, Any]:
    result = patch_check(base, manifest)
    manifest["base_commit"] = base
    manifest["unaccounted_paths"] = result["unaccounted_paths"]
    manifest["manifest_complete"] = result["manifest_complete"]
    for entry in manifest.get("entries", []):
        paths = entry.get("paths", [])
        if not paths:
            continue
        diff = run(["git", "diff", f"{base}..HEAD", "--", *paths], timeout=120)
        if diff["exit_code"] != 0:
            raise RuntimeError(f"git diff for {entry.get('id')} failed: {diff['stderr'].strip()}")
        entry["diff_sha256"] = sha256_text(diff["stdout"])
    return result


def validate_registry(registry: dict[str, Any]) -> list[dict[str, str]]:
    checks: list[dict[str, str]] = []

    def add(check_id: str, status: str, detail: str) -> None:
        checks.append({"id": check_id, "status": status, "detail": detail})

    if registry.get("schema_version") != "1.0":
        add("registry.schema", "fail", f"schema_version is {registry.get('schema_version')!r}, expected '1.0'")
    else:
        add("registry.schema", "pass", "schema_version 1.0")

    entries = registry.get("upstreams")
    if not isinstance(entries, list) or not entries:
        add("registry.upstreams", "fail", "upstreams must be a non-empty list")
        return checks

    seen: set[str] = set()
    for index, entry in enumerate(entries):
        prefix = f"upstreams[{index}]"
        if not isinstance(entry, dict):
            add(f"{prefix}.shape", "fail", "entry is not an object")
            continue
        upstream_id = entry.get("id")
        if not isinstance(upstream_id, str) or not re.fullmatch(r"[a-z0-9][a-z0-9-]*", upstream_id):
            add(f"{prefix}.id", "fail", f"invalid id {upstream_id!r}")
            continue
        if upstream_id in seen:
            add(f"{prefix}.id", "fail", f"duplicate id {upstream_id!r}")
        seen.add(upstream_id)

        reuse_mode = entry.get("reuse_mode")
        if reuse_mode not in REUSE_MODES:
            add(f"{upstream_id}.reuse_mode", "fail", f"unknown reuse_mode {reuse_mode!r}")

        source = entry.get("source")
        if not isinstance(source, dict) or not source.get("url"):
            add(f"{upstream_id}.source", "fail", "source.url is required")

        license_info = entry.get("license")
        if isinstance(license_info, dict) and license_info.get("attribution_file"):
            attribution = ROOT / str(license_info["attribution_file"])
            if not attribution.is_file():
                add(f"{upstream_id}.license.attribution_file", "fail", f"missing {license_info['attribution_file']}")
            else:
                add(f"{upstream_id}.license.attribution_file", "pass", str(license_info["attribution_file"]))

        pin = entry.get("pin")
        if not isinstance(pin, dict):
            add(f"{upstream_id}.pin", "fail", "pin object is required")
        else:
            kind = pin.get("kind")
            if kind not in PIN_KINDS:
                add(f"{upstream_id}.pin.kind", "fail", f"unknown pin kind {kind!r}")
            elif kind == "commit":
                commit = pin.get("commit")
                if not isinstance(commit, str) or not COMMIT_RE.fullmatch(commit):
                    add(f"{upstream_id}.pin.commit", "fail", f"invalid commit {commit!r}")
                else:
                    add(f"{upstream_id}.pin.commit", "pass", commit)
            elif kind == "lockfile":
                path = pin.get("path")
                if not isinstance(path, str) or not (ROOT / path).is_file():
                    add(f"{upstream_id}.pin.lockfile", "fail", f"lockfile not found: {path!r}")
                else:
                    add(f"{upstream_id}.pin.lockfile", "pass", path)
            else:
                if not pin.get("version"):
                    add(f"{upstream_id}.pin.version", "fail", "version is required")
                else:
                    add(f"{upstream_id}.pin.version", "pass", str(pin["version"]))

        contract = entry.get("contract")
        if not isinstance(contract, dict) or not contract.get("id") or not contract.get("version"):
            add(f"{upstream_id}.contract", "fail", "contract.id and contract.version are required")

        policy = entry.get("update_policy")
        if not isinstance(policy, dict):
            add(f"{upstream_id}.update_policy", "fail", "update_policy object is required")
        else:
            if policy.get("mode") not in UPDATE_MODES:
                add(f"{upstream_id}.update_policy.mode", "fail", f"unknown mode {policy.get('mode')!r}")
            for flag in ("auto_follow", "auto_merge", "auto_pin_update"):
                if policy.get(flag) is not False:
                    add(f"{upstream_id}.update_policy.{flag}", "fail", f"{flag} must be false")

        records = entry.get("pin_records", [])
        if not isinstance(records, list):
            add(f"{upstream_id}.pin_records", "fail", "pin_records must be a list")

    add("registry.ids", "pass", f"{len(seen)} unique upstream ids")
    return checks


def check_pin_records(upstream: dict[str, Any]) -> list[dict[str, str]]:
    checks: list[dict[str, str]] = []
    upstream_id = upstream["id"]
    pin = upstream.get("pin", {})
    if pin.get("kind") != "commit":
        return checks
    expected = pin["commit"]
    for index, record in enumerate(upstream.get("pin_records", [])):
        check_id = f"{upstream_id}.pin_records[{index}]"
        if record.get("ignore"):
            checks.append({"id": check_id, "status": "info", "detail": f"ignored historical record: {record.get('path')}"})
            continue
        path = ROOT / str(record.get("path", ""))
        if not path.is_file():
            status = "warn" if record.get("optional") else "fail"
            checks.append({"id": check_id, "status": status, "detail": f"missing {record.get('path')}"})
            continue
        try:
            text = path.read_text(encoding="utf-8")
        except OSError as error:
            checks.append({"id": check_id, "status": "fail", "detail": f"cannot read {record.get('path')}: {error}"})
            continue
        kind = record.get("kind")
        observed: Any = None
        try:
            if kind == "json":
                observed = resolve_pointer(json.loads(text), str(record.get("pointer", "")))
            elif kind == "text":
                needle = str(record.get("contains", ""))
                observed = needle if needle and needle in text else None
            elif kind == "python_const":
                match = re.search(str(record.get("regex", "")), text)
                observed = match.group(1) if match else None
            else:
                checks.append({"id": check_id, "status": "fail", "detail": f"unknown record kind {kind!r}"})
                continue
        except Exception as error:  # noqa: BLE001 - report the file, not a traceback
            checks.append({"id": check_id, "status": "fail", "detail": f"cannot resolve {record.get('path')}: {error}"})
            continue
        if observed != expected:
            status = "warn" if record.get("warn_only") else "fail"
            checks.append({
                "id": check_id,
                "status": status,
                "detail": f"{record.get('path')} has {observed!r}, expected {expected!r}",
            })
        else:
            checks.append({"id": check_id, "status": "pass", "detail": f"{record.get('path')} matches"})
    return checks


def check_dependency_isolation(metadata: dict[str, Any], package_name: str = "archmeld-core") -> list[dict[str, str]]:
    """Pure check: the private core must not depend on any codegraph crate."""
    checks: list[dict[str, str]] = []
    packages = metadata.get("packages", [])
    package = next((item for item in packages if item.get("name") == package_name), None)
    if package is None:
        return [{"id": "deps.core", "status": "fail", "detail": f"package {package_name!r} not found in cargo metadata"}]
    package_id = package.get("id")
    resolve = metadata.get("resolve") or {}
    nodes = resolve.get("nodes", [])
    by_id = {node.get("id"): node for node in nodes}
    names = {item.get("id"): item.get("name", "") for item in packages}
    if package_id not in by_id:
        return [{"id": "deps.core", "status": "fail", "detail": f"resolve node for {package_name!r} not found"}]

    queue: list[tuple[str, list[str]]] = [(package_id, [package_name])]
    seen: set[str] = set()
    offenders: list[str] = []
    while queue:
        current, chain = queue.pop(0)
        if current in seen:
            continue
        seen.add(current)
        node = by_id.get(current)
        if node is None:
            continue
        for dep in node.get("deps", []):
            dep_id = dep.get("pkg")
            if not dep_id or dep_id in seen:
                continue
            dep_name = names.get(dep_id, "")
            next_chain = chain + [dep_name]
            if dep_name.startswith("codegraph"):
                offenders.append(" -> ".join(next_chain))
                continue
            queue.append((dep_id, next_chain))
    if offenders:
        checks.append({"id": "deps.core", "status": "fail", "detail": "codegraph dependency: " + "; ".join(offenders)})
    else:
        checks.append({"id": "deps.core", "status": "pass", "detail": f"{package_name} closure has no codegraph crate"})
    return checks


def check_generated_artifacts(registry: dict[str, Any]) -> list[dict[str, str]]:
    checks: list[dict[str, str]] = []
    codegraph = upstreams_by_id(registry).get("codegraph")
    if not codegraph:
        return checks
    pin = codegraph.get("pin", {}).get("commit")
    generated = ROOT / "dist/release-manifest.json"
    if generated.is_file():
        try:
            value = read_json(generated).get("upstream_commit")
        except Exception as error:  # noqa: BLE001
            checks.append({"id": "artifacts.release_manifest", "status": "warn", "detail": f"cannot parse: {error}"})
        else:
            if value != pin:
                checks.append({
                    "id": "artifacts.release_manifest",
                    "status": "warn",
                    "detail": f"generated release manifest has {value!r}, registry has {pin!r}; regenerate, do not hand-edit",
                })
            else:
                checks.append({"id": "artifacts.release_manifest", "status": "pass", "detail": "upstream_commit matches"})
    historical = ROOT / "docs/archmeld/research/upstream-selection.json"
    if historical.is_file():
        try:
            value = read_json(historical).get("primary", {}).get("commit")
        except Exception as error:  # noqa: BLE001
            checks.append({"id": "artifacts.sealed_research", "status": "warn", "detail": f"cannot parse: {error}"})
        else:
            if value != pin:
                checks.append({
                    "id": "artifacts.sealed_research",
                    "status": "info",
                    "detail": f"sealed research record has {value!r}; superseded by registry, do not edit without re-sealing",
                })
            else:
                checks.append({"id": "artifacts.sealed_research", "status": "pass", "detail": "sealed research record matches"})
    return checks


def check_sealed_pack() -> list[dict[str, str]]:
    checks: list[dict[str, str]] = []
    if not (ROOT / ".git").is_dir():
        checks.append({"id": "sealed_pack.clean", "status": "info", "detail": "no .git; skipped"})
    else:
        record = run(["git", "status", "--porcelain", "--", "docs/archmeld"], timeout=30)
        if record["exit_code"] != 0:
            checks.append({"id": "sealed_pack.clean", "status": "warn", "detail": "git status failed"})
        elif record["stdout"].strip():
            checks.append({"id": "sealed_pack.clean", "status": "warn", "detail": "docs/archmeld has local changes; re-seal before claiming the pack is untouched"})
        else:
            checks.append({"id": "sealed_pack.clean", "status": "pass", "detail": "docs/archmeld is clean"})

    base = ROOT / "docs/archmeld"
    sums = base / "SHA256SUMS"
    if sums.is_file():
        mismatches: list[str] = []
        for line in sums.read_text(encoding="utf-8").splitlines():
            parts = line.split(None, 1)
            if len(parts) != 2:
                continue
            expected, name = parts[0], parts[1].lstrip("*").strip()
            path = base / name
            if not path.is_file():
                mismatches.append(f"{name}: missing")
            elif sha256_file(path) != expected:
                mismatches.append(name)
        if mismatches:
            checks.append({
                "id": "sealed_pack.digests",
                "status": "warn",
                "detail": "pre-existing SHA256SUMS mismatches: " + ", ".join(sorted(mismatches)),
            })
        else:
            checks.append({"id": "sealed_pack.digests", "status": "pass", "detail": "SHA256SUMS matches"})
    return checks


def cargo_metadata_record() -> tuple[dict[str, Any] | None, dict[str, Any]]:
    record = run(
        ["cargo", "metadata", "--format-version", "1", "--offline", "--locked"],
        timeout=180,
        text_limit=None,
    )
    stdout = record["stdout"]
    record["stdout_bytes"] = len(stdout.encode("utf-8"))
    # Keep evidence small; the digest above is enough to identify the output.
    record["stdout"] = ""
    if record["exit_code"] != 0:
        return None, record
    try:
        return json.loads(stdout), record
    except json.JSONDecodeError as error:
        record["parse_error"] = str(error)
        return None, record


def verify(registry: dict[str, Any], skip_cargo: bool = False) -> dict[str, Any]:
    checks = validate_registry(registry)
    for entry in registry.get("upstreams", []):
        checks.extend(check_pin_records(entry))
    checks.extend(check_generated_artifacts(registry))
    checks.extend(check_sealed_pack())
    commands: list[dict[str, Any]] = []

    manifest = ROOT / "patches/codegraph/manifest.json"
    if manifest.is_file():
        checks.append({"id": "patches.manifest", "status": "pass", "detail": "patches/codegraph/manifest.json exists"})
    else:
        checks.append({"id": "patches.manifest", "status": "warn", "detail": "patches/codegraph/manifest.json is missing (U1)"})

    contract_doc = ROOT / "contracts/upstreams/archmeld-export-v1.md"
    if contract_doc.is_file():
        checks.append({"id": "contract.export_doc", "status": "pass", "detail": "contract document exists"})
    else:
        checks.append({"id": "contract.export_doc", "status": "warn", "detail": "contracts/upstreams/archmeld-export-v1.md is missing"})

    if skip_cargo:
        checks.append({"id": "deps.core", "status": "info", "detail": "skipped by --skip-cargo"})
    else:
        metadata, record = cargo_metadata_record()
        commands.append(record)
        if metadata is None:
            checks.append({
                "id": "deps.core",
                "status": "fail",
                "detail": f"cargo metadata failed with exit {record['exit_code']}: {record['stderr'].strip()[:400]}",
            })
        else:
            checks.extend(check_dependency_isolation(metadata))

    failures = [check for check in checks if check["status"] == "fail"]
    warnings = [check for check in checks if check["status"] == "warn"]
    status = "failed" if failures else ("passed_with_warnings" if warnings else "passed")
    return {
        "schema_version": "1.0",
        "recorded_at": today(),
        "scope": "upstream registry, pin consistency, dependency isolation and sealed-pack cleanliness",
        "generated_by": "tooling/upstream.py verify",
        "auto_follow_latest": False,
        "registry_sha256": sha256_file(REGISTRY_PATH) if REGISTRY_PATH.is_file() else None,
        "toolchain_lock_sha256": sha256_file(TOOLCHAIN_LOCK) if TOOLCHAIN_LOCK.is_file() else None,
        "tools": tool_versions(),
        "checks": checks,
        "commands": commands,
        "status": status,
        "limitations": [
            "file and cargo-metadata checks only; Rust contract tests are separate",
            "no fetch, merge, push or pin update",
            "historical sealed-pack records are reported as information, not drift",
        ],
    }


def ls_remote(remote: str, ref: str) -> dict[str, Any]:
    record = run(["git", "ls-remote", remote, ref], timeout=60)
    if record["exit_code"] != 0:
        raise RuntimeError(f"git ls-remote {remote} {ref} failed: {record['stderr'].strip()}")
    commits: list[str] = []
    for line in record["stdout"].splitlines():
        parts = line.split("\t")
        if len(parts) == 2 and COMMIT_RE.fullmatch(parts[0]):
            commits.append(parts[0])
    if not commits:
        raise RuntimeError(f"git ls-remote {remote} {ref} returned no commit")
    return {"remote": remote, "ref": ref, "commit": commits[0], "command": record}


def status(registry: dict[str, Any], remote: bool = False) -> dict[str, Any]:
    rows: list[dict[str, Any]] = []
    warnings: list[str] = []
    for entry in registry.get("upstreams", []):
        pin = entry.get("pin", {})
        row = {
            "id": entry.get("id"),
            "reuse_mode": entry.get("reuse_mode"),
            "pin_kind": pin.get("kind"),
            "pin": pin.get("commit") or pin.get("version") or pin.get("path"),
            "version": pin.get("version"),
            "contract": f"{entry.get('contract', {}).get('id')} {entry.get('contract', {}).get('version')}",
            "last_drill": (entry.get("last_drill") or {}).get("result"),
            "update_available": None,
            "latest": None,
        }
        if remote and pin.get("kind") == "commit":
            source = entry.get("source", {})
            remote_name = source.get("remote")
            ref = source.get("ref")
            if remote_name and ref:
                try:
                    latest = ls_remote(remote_name, ref)
                except RuntimeError as error:
                    warnings.append(f"{entry.get('id')}: {error}")
                else:
                    row["latest"] = latest["commit"]
                    row["update_available"] = latest["commit"] != pin.get("commit")
        rows.append(row)
    return {"schema_version": "1.0", "recorded_at": today(), "upstreams": rows, "warnings": warnings}


def check(registry: dict[str, Any], upstream_id: str) -> dict[str, Any]:
    entries = upstreams_by_id(registry)
    entry = entries.get(upstream_id)
    if entry is None:
        raise SystemExit(f"unknown upstream id: {upstream_id}")
    pin = entry.get("pin", {})
    if pin.get("kind") != "commit":
        raise SystemExit(f"{upstream_id} has pin kind {pin.get('kind')!r}; remote commit check does not apply")
    source = entry.get("source", {})
    remote_name = source.get("remote")
    ref = source.get("ref")
    if not remote_name or not ref:
        raise SystemExit(f"{upstream_id} has no configured remote/ref")
    latest = ls_remote(remote_name, ref)
    update_available = latest["commit"] != pin.get("commit")
    return {
        "schema_version": "1.0",
        "recorded_at": today(),
        "generated_by": "tooling/upstream.py check",
        "upstream": upstream_id,
        "pin": pin.get("commit"),
        "latest": latest["commit"],
        "ref": ref,
        "update_available": update_available,
        "auto_follow_latest": False,
        "command": latest["command"],
    }


def upstream_diff(
    registry: dict[str, Any],
    upstream_id: str,
    to_ref: str,
    from_ref: str | None = None,
    fetch: bool = False,
) -> dict[str, Any]:
    entry = upstreams_by_id(registry).get(upstream_id)
    if entry is None:
        raise SystemExit(f"unknown upstream id: {upstream_id}")
    pin = entry.get("pin", {}).get("commit")
    if not pin:
        raise SystemExit(f"{upstream_id} has no commit pin")
    from_ref = from_ref or pin
    commands: list[dict[str, Any]] = []
    if fetch:
        source = entry.get("source", {})
        remote = source.get("remote")
        ref = source.get("ref")
        if not remote or not ref:
            raise SystemExit(f"{upstream_id} has no configured remote/ref for --fetch")
        fetched = run(["git", "fetch", remote, ref], timeout=180)
        commands.append(fetched)
        if fetched["exit_code"] != 0:
            raise SystemExit(f"git fetch failed: {fetched['stderr'].strip()}")

    resolved = run(["git", "rev-parse", "--verify", f"{to_ref}^{{commit}}"], timeout=30)
    commands.append(resolved)
    if resolved["exit_code"] != 0:
        raise SystemExit(f"cannot resolve {to_ref!r}; fetch it explicitly with --fetch or pass a local ref")
    to_commit = resolved["stdout"].strip()

    log = run(["git", "log", "--oneline", "--no-merges", f"{from_ref}..{to_ref}"], timeout=60)
    stat = run(["git", "diff", "--stat", f"{from_ref}..{to_ref}"], timeout=60)
    names = run(["git", "diff", "--name-status", f"{from_ref}..{to_ref}"], timeout=60)
    merge = run(["git", "merge-tree", "--write-tree", "--name-only", "HEAD", to_ref], timeout=120)
    commands.extend([log, stat, names, merge])

    changed: set[str] = set()
    for line in names["stdout"].splitlines():
        parts = line.split("\t")
        if len(parts) >= 2:
            changed.update(parts[1:])
    conflicts = [line.strip() for line in merge["stdout"].splitlines()[1:] if line.strip()]
    manifest_path = ROOT / str(entry.get("patch_manifest", ""))
    hotspots: set[str] = set()
    if manifest_path.is_file():
        manifest = read_json(manifest_path)
        hotspots = {
            path
            for item in manifest.get("entries", [])
            if item.get("conflict_hotspot")
            for path in item.get("paths", [])
        }
    return {
        "schema_version": "1.0",
        "recorded_at": today(),
        "generated_by": "tooling/upstream.py diff",
        "upstream": upstream_id,
        "from": from_ref,
        "from_pin": pin,
        "to": to_ref,
        "to_commit": to_commit,
        "update_available": to_commit != pin,
        "commits": log["stdout"].splitlines(),
        "diffstat": stat["stdout"].splitlines(),
        "changed_paths": sorted(changed),
        "conflicts": conflicts,
        "conflict_hotspots": sorted(changed & hotspots),
        "merge_exit_code": merge["exit_code"],
        "commands": commands,
        "auto_follow_latest": False,
    }


def drill_matrix(worktree: Path, env: dict[str, str], matrix: str) -> list[dict[str, Any]]:
    cargo = shutil.which("cargo", path=env.get("PATH", "")) or "cargo"
    commands = [
        [cargo, "test", "--offline", "-p", "archmeld-core", "-p", "archmeld-api", "--no-fail-fast", "-j", "2"],
        [cargo, "test", "--offline", "-p", "archmeld-core", "--test", "bridge_import", "-j", "2"],
        [cargo, "test", "--offline", "-p", "codegraph", "--test", "archmeld_probes", "--no-default-features", "-j", "2"],
    ]
    if matrix == "full":
        commands.extend([
            [cargo, "test", "--offline", "-p", "codegraph-php", "-j", "2"],
            [cargo, "test", "--offline", "-p", "codegraph-server", "--test", "archmeld_state_root", "--test", "parser_graph_properties_test", "-j", "2"],
            [cargo, "build", "--offline", "-p", "codegraph-server", "--example", "archmeld_export", "-j", "2"],
        ])
    results = []
    for argv in commands:
        results.append(run(argv, cwd=worktree, timeout=3600))
    return results


def drill(
    registry: dict[str, Any],
    upstream_id: str,
    ref: str,
    run_matrix: bool = False,
    matrix: str = "quick",
    keep_worktree: bool = False,
) -> dict[str, Any]:
    entry = upstreams_by_id(registry).get(upstream_id)
    if entry is None:
        raise SystemExit(f"unknown upstream id: {upstream_id}")
    clean = run(["git", "status", "--porcelain"], timeout=30)
    if clean["stdout"].strip():
        raise SystemExit("drill requires a clean main worktree; commit or stash local changes first")
    TEMP_BASE.mkdir(parents=True, exist_ok=True)
    temp = Path(tempfile.mkdtemp(prefix="upstream-drill-", dir=str(TEMP_BASE)))
    worktree = temp / "worktree"
    commands: list[dict[str, Any]] = [clean]
    added = run(["git", "worktree", "add", "--detach", str(worktree), "HEAD"], timeout=120)
    commands.append(added)
    if added["exit_code"] != 0:
        shutil.rmtree(temp, ignore_errors=True)
        raise SystemExit(f"git worktree add failed: {added['stderr'].strip()}")

    resolved = run(["git", "rev-parse", "--verify", f"{ref}^{{commit}}"], timeout=30)
    commands.append(resolved)
    to_commit = resolved["stdout"].strip() if resolved["exit_code"] == 0 else None
    merge = run(["git", "merge", "--no-commit", "--no-ff", ref], cwd=worktree, timeout=600)
    commands.append(merge)
    conflicts: list[str] = []
    if merge["exit_code"] != 0:
        conflict_result = run(["git", "diff", "--name-only", "--diff-filter=U"], cwd=worktree, timeout=30)
        commands.append(conflict_result)
        conflicts = [line.strip() for line in conflict_result["stdout"].splitlines() if line.strip()]

    tests: list[dict[str, Any]] = []
    status = "conflicted" if conflicts else "passed"
    if run_matrix and not conflicts:
        env = subprocess_env()
        env["CARGO_TARGET_DIR"] = str(temp / "target")
        env["LIBCLANG_PATH"] = "<workspace>/_archmeld_tools/pylibclang/clang/native"
        env["BINDGEN_EXTRA_CLANG_ARGS"] = (
            "-ID:/Strawberry/c/lib/gcc/x86_64-w64-mingw32/13.2.0/include "
            "-ID:/Strawberry/c/x86_64-w64-mingw32/include"
        )
        tests = drill_matrix(worktree, env, matrix)
        if any(test["exit_code"] != 0 for test in tests):
            status = "failed"

    if not keep_worktree:
        if conflicts:
            run(["git", "merge", "--abort"], cwd=worktree, timeout=30)
        run(["git", "worktree", "remove", "--force", str(worktree)], timeout=120)
        shutil.rmtree(temp, ignore_errors=True)

    return {
        "schema_version": "1.0",
        "recorded_at": today(),
        "generated_by": "tooling/upstream.py drill",
        "upstream": upstream_id,
        "ref": ref,
        "to_commit": to_commit,
        "from": run(["git", "rev-parse", "HEAD"], timeout=30)["stdout"].strip(),
        "base_pin": entry.get("pin", {}).get("commit"),
        "merge_exit_code": merge["exit_code"],
        "conflicts": conflicts,
        "matrix": matrix if run_matrix else "not_run",
        "tests": tests,
        "status": status,
        "auto_follow_latest": False,
        "commands": commands,
        "worktree_kept": keep_worktree,
    }


def plan_record_updates(root: Path, evidence: dict[str, Any], evidence_path: str) -> dict[str, Any]:
    registry_path = root / "upstreams.lock.json"
    registry = read_json(registry_path)
    codegraph = next((item for item in registry.get("upstreams", []) if item.get("id") == "codegraph"), None)
    if codegraph is None:
        raise SystemExit("registry has no codegraph entry")
    old_pin = codegraph.get("pin", {}).get("commit")
    new_pin = evidence.get("to_commit") or evidence.get("pin")
    if not isinstance(new_pin, str) or not COMMIT_RE.fullmatch(new_pin):
        raise SystemExit(f"evidence has no valid to_commit: {new_pin!r}")
    if new_pin == old_pin:
        return {"changed": False, "old_pin": old_pin, "new_pin": new_pin, "updates": []}
    if evidence.get("status") != "passed":
        raise SystemExit(f"refusing to record evidence with status {evidence.get('status')!r}")

    updates: list[dict[str, Any]] = []

    def add_text(relative: str, new_text: str) -> None:
        path = root / relative
        old_text = path.read_text(encoding="utf-8")
        updates.append({
            "path": relative,
            "old_sha256": sha256_text(old_text),
            "new_sha256": sha256_text(new_text),
            "content": new_text,
        })

    codegraph["pin"]["commit"] = new_pin
    if evidence.get("to_version"):
        codegraph["pin"]["version"] = evidence["to_version"]
    codegraph["pin"]["checked_at"] = today()
    codegraph["last_checked"] = today()
    codegraph["last_drill"] = {
        "path": evidence_path,
        "date": today(),
        "result": "passed",
        "acceptance": ["AT077"],
    }
    add_text("upstreams.lock.json", json.dumps(registry, ensure_ascii=False, indent=2) + "\n")

    toolchain_path = root / "toolchains.lock.json"
    toolchain = read_json(toolchain_path)
    toolchain.setdefault("pinned", {})["codegraph_commit"] = new_pin
    add_text("toolchains.lock.json", json.dumps(toolchain, ensure_ascii=False, indent=2) + "\n")

    for relative in ("AGENTS.md", "UPSTREAM_PATCHES.md", "third_party/NOTICE.md"):
        path = root / relative
        old_text = path.read_text(encoding="utf-8")
        if old_pin not in old_text:
            raise SystemExit(f"{relative} does not contain the current pin {old_pin}")
        add_text(relative, old_text.replace(old_pin, new_pin, 1))

    manifest_path = root / "patches/codegraph/manifest.json"
    if manifest_path.is_file():
        manifest = read_json(manifest_path)
        manifest["base_commit"] = new_pin
        add_text("patches/codegraph/manifest.json", json.dumps(manifest, ensure_ascii=False, indent=2) + "\n")

    return {"changed": True, "old_pin": old_pin, "new_pin": new_pin, "updates": updates}


def print_status(result: dict[str, Any]) -> None:
    header = f"{'id':<14} {'mode':<18} {'pin':<14} {'version':<12} {'contract':<28} {'drill':<8} {'update'}"
    print(header)
    for row in result["upstreams"]:
        pin = str(row.get("pin") or "")
        short = pin[:12] if COMMIT_RE.fullmatch(pin) else pin
        update = "" if row.get("update_available") is None else ("yes" if row["update_available"] else "no")
        print(
            f"{row['id']:<14} {row['reuse_mode']:<18} {short:<14} "
            f"{str(row.get('version') or ''):<12} {row['contract']:<28} "
            f"{str(row.get('last_drill') or ''):<8} {update}"
        )
    for warning in result.get("warnings", []):
        print(f"warning: {warning}", file=sys.stderr)


def print_checks(result: dict[str, Any]) -> None:
    for check_item in result["checks"]:
        print(f"[{check_item['status']:<5}] {check_item['id']}: {check_item['detail']}")
    print(f"status: {result['status']}")


def main(argv: list[str] | None = None) -> int:
    parser = argparse.ArgumentParser(description="Read-only ARCHMELD upstream registry checks")
    subparsers = parser.add_subparsers(dest="command", required=True)

    status_parser = subparsers.add_parser("status", help="show registry state")
    status_parser.add_argument("--json", action="store_true")
    status_parser.add_argument("--remote", action="store_true", help="query git ls-remote; read-only")

    verify_parser = subparsers.add_parser("verify", help="check registry, pins and dependency isolation")
    verify_parser.add_argument("--json", action="store_true")
    verify_parser.add_argument("--out", help="write the evidence JSON to this path")
    verify_parser.add_argument("--skip-cargo", action="store_true", help="skip cargo metadata (tests/offline)")

    check_parser = subparsers.add_parser("check", help="compare a commit pin with git ls-remote")
    check_parser.add_argument("id")
    check_parser.add_argument("--json", action="store_true")
    check_parser.add_argument("--fail-on-update", action="store_true", help="exit 3 when a newer commit exists")

    patches_parser = subparsers.add_parser("patches", help="check or refresh the path-level patch manifest")
    patches_parser.add_argument("--check", action="store_true", help="check only (default)")
    patches_parser.add_argument("--write", action="store_true", help="refresh diff hashes and unaccounted paths")
    patches_parser.add_argument("--json", action="store_true")

    diff_parser = subparsers.add_parser("diff", help="preflight a candidate upstream ref")
    diff_parser.add_argument("id")
    diff_parser.add_argument("--to", required=True)
    diff_parser.add_argument("--from", dest="from_ref")
    diff_parser.add_argument("--fetch", action="store_true", help="git fetch the configured remote/ref first")
    diff_parser.add_argument("--json", action="store_true")

    drill_parser = subparsers.add_parser("drill", help="merge a candidate ref in a temporary worktree")
    drill_parser.add_argument("id")
    drill_parser.add_argument("--ref", required=True)
    drill_parser.add_argument("--run", action="store_true", help="run the test matrix after a clean merge")
    drill_parser.add_argument("--matrix", choices=["quick", "full"], default="quick")
    drill_parser.add_argument("--keep-worktree", action="store_true")
    drill_parser.add_argument("--out")
    drill_parser.add_argument("--json", action="store_true")

    record_parser = subparsers.add_parser("record", help="guarded pin/doc update after a passed drill")
    record_parser.add_argument("id")
    record_parser.add_argument("--evidence", required=True)
    record_parser.add_argument("--yes", action="store_true")
    record_parser.add_argument("--dry-run", action="store_true")
    record_parser.add_argument("--json", action="store_true")

    args = parser.parse_args(argv)
    registry = load_registry()

    if args.command == "status":
        result = status(registry, remote=args.remote)
        if args.json:
            print(json.dumps(result, ensure_ascii=False, indent=2))
        else:
            print_status(result)
        return 0

    if args.command == "verify":
        result = verify(registry, skip_cargo=args.skip_cargo)
        if args.out:
            write_json(Path(args.out), result)
        if args.json:
            print(json.dumps(result, ensure_ascii=False, indent=2))
        else:
            print_checks(result)
        return 0 if result["status"] != "failed" else 1

    if args.command == "check":
        result = check(registry, args.id)
        if args.json:
            print(json.dumps(result, ensure_ascii=False, indent=2))
        else:
            print(f"{args.id}: pin {result['pin']} latest {result['latest']} update_available={result['update_available']}")
        if args.fail_on_update and result["update_available"]:
            return 3
        return 0

    if args.command == "patches":
        manifest_path = ROOT / "patches/codegraph/manifest.json"
        if not manifest_path.is_file():
            raise SystemExit(f"patch manifest not found: {manifest_path}")
        manifest = read_json(manifest_path)
        base = manifest.get("base_commit")
        if not base:
            raise SystemExit("patch manifest has no base_commit")
        if args.write:
            result = patch_write(base, manifest)
            write_json(manifest_path, manifest)
        else:
            result = patch_check(base, manifest)
        if args.json:
            print(json.dumps(result, ensure_ascii=False, indent=2))
        else:
            print(f"manifest_complete={result['manifest_complete']}")
            for path in result["unaccounted_paths"]:
                print(f"unaccounted: {path}")
            for path in result["missing_paths"]:
                print(f"missing: {path}")
        return 0 if result["manifest_complete"] else 1

    if args.command == "diff":
        result = upstream_diff(registry, args.id, args.to, from_ref=args.from_ref, fetch=args.fetch)
        if args.json:
            print(json.dumps(result, ensure_ascii=False, indent=2))
        else:
            print(f"{args.id}: {result['from']}..{result['to']} ({result['to_commit']})")
            print(f"commits: {len(result['commits'])}")
            print(f"conflicts: {len(result['conflicts'])}")
            for path in result["conflict_hotspots"]:
                print(f"hotspot: {path}")
        return 0

    if args.command == "drill":
        result = drill(
            registry,
            args.id,
            args.ref,
            run_matrix=args.run,
            matrix=args.matrix,
            keep_worktree=args.keep_worktree,
        )
        if args.out:
            write_json(Path(args.out), result)
        if args.json:
            print(json.dumps(result, ensure_ascii=False, indent=2))
        else:
            print(f"{args.id}: merge_exit={result['merge_exit_code']} conflicts={len(result['conflicts'])} status={result['status']}")
        return 0 if result["status"] in {"passed", "conflicted"} else 1

    if args.command == "record":
        evidence_path = Path(args.evidence)
        if not evidence_path.is_file():
            raise SystemExit(f"evidence not found: {evidence_path}")
        evidence = read_json(evidence_path)
        clean = run(["git", "status", "--porcelain"], timeout=30)
        if clean["stdout"].strip() and not args.dry_run:
            raise SystemExit("record requires a clean worktree")
        plan = plan_record_updates(ROOT, evidence, args.evidence)
        if args.json:
            print(json.dumps(plan, ensure_ascii=False, indent=2))
        else:
            if not plan["changed"]:
                print(f"no change: pin is already {plan['old_pin']}")
            else:
                print(f"pin {plan['old_pin']} -> {plan['new_pin']}")
                for update in plan["updates"]:
                    print(f"update: {update['path']}")
        if plan["changed"] and not args.dry_run:
            if not args.yes:
                raise SystemExit("record requires --yes (use --dry-run to inspect)")
            for update in plan["updates"]:
                (ROOT / update["path"]).write_text(update["content"], encoding="utf-8")
            print("recorded; commit the result manually")
        return 0

    parser.error(f"unknown command {args.command}")
    return 2


if __name__ == "__main__":
    raise SystemExit(main())
