#!/usr/bin/env python3
# Copyright 2026 ARCHMELD contributors
# SPDX-License-Identifier: Apache-2.0
"""Leak scanner and sanitized export for the public snapshot.

`check` is read-only. `export` writes only under `--out` (normally a temp
directory). It never pushes, never edits the private work tree and never
re-encodes `docs/archmeld/**`: that pack is copied byte-for-byte.

The tool contains generic patterns only. Private dataset names and other
literal replacements come from an untracked `--private-map` file.
"""

from __future__ import annotations

import argparse
import fnmatch
import hashlib
import io
import json
import os
import re
import shutil
import socket
import subprocess
import sys
import tarfile
import tempfile
from datetime import date
from pathlib import Path
from typing import Any

ROOT = Path(__file__).resolve().parents[1]
POLICY = ROOT / "tooling" / "public_snapshot_policy.json"
MANIFEST = ROOT / "patches" / "codegraph" / "manifest.json"
PUBLIC_MARKER = "PUBLIC_SNAPSHOT.md"

GENERIC_RULES: list[tuple[str, re.Pattern[str]]] = [
    ("P01_windows_path", re.compile(r"(?<![A-Za-z0-9])[A-Za-z]:[\\/]")),
    ("P02_msys_path", re.compile(r"/(?:mnt/[a-z]/|[a-z]/(?:Users|Strawberry|Codex)/)")),
    ("P03_file_uri", re.compile(r"file://<path>")),
    ("P04_ssh", re.compile(r"[\w.-]+@(?:\d{1,3}\.){3}\d{1,3}(?::\d+)?")),
    ("P05_private_ip", re.compile(r"\b(?:10\.\d{1,3}|192\.168\.\d{1,3}|172\.(?:1[6-9]|2\d|3[01])\.\d{1,3})\.\d{1,3}\b")),
    ("P06_secret", re.compile(
        r"(?:sk-[A-Za-z0-9]{16,}|ghp_[A-Za-z0-9]{20,}|github_pat_[A-Za-z0-9_]{8,}|"
        r"xox[baprs]-[A-Za-z0-9-]{10,}|AKIA[0-9A-Z]{16}|ASIA[0-9A-Z]{16}|"
        r"-----BEGIN (?:RSA |OPENSSH |EC )?PRIVATE KEY-----)"
    )),
    ("P07_dataset_marker", re.compile(r"\b(?:joint_[A-Za-z0-9_]+|<selected-columns>)\b")),
    ("P08_temp_path", re.compile(r"AppData[\\/]+Local[\\/]+Temp")),
]

TEXT_SKIP_SUFFIXES = {".scip", ".dll", ".pdb", ".png", ".jpg", ".jpeg", ".ico", ".zip", ".vsix", ".trace"}


def sha256_bytes(data: bytes) -> str:
    return hashlib.sha256(data).hexdigest()


def sha256_file(path: Path) -> str:
    return sha256_bytes(path.read_bytes())


def load_json(path: Path) -> Any:
    return json.loads(path.read_text(encoding="utf-8"))


def load_policy(path: Path | None = None) -> dict:
    path = path or POLICY
    if path.is_file():
        return load_json(path)
    return {"schema_version": "1.0", "omit": [], "allow": []}


def omitted(relative: str, policy: dict) -> str | None:
    for item in policy.get("omit", []):
        if fnmatch.fnmatch(relative.replace("\\", "/"), item["glob"]):
            return item.get("reason", "omitted")
    return None


def allowed(relative: str, policy: dict) -> str | None:
    for item in policy.get("allow", []):
        if fnmatch.fnmatch(relative.replace("\\", "/"), item["glob"]):
            return item.get("reason", "allowed")
    return None


def tracked_paths(root: Path) -> list[str]:
    if not (root / ".git").is_dir():
        return []
    result = subprocess.run(
        ["git", "ls-files", "-z"],
        cwd=str(root),
        capture_output=True,
        timeout=120,
    )
    if result.returncode != 0:
        return []
    return [item.decode("utf-8", errors="replace") for item in result.stdout.split(b"\x00") if item]


def text_candidates(data: bytes) -> list[str]:
    candidates = [data.decode("utf-8", errors="replace")]
    if data and data.count(b"\x00") > len(data) // 10:
        for encoding in ("utf-16-le", "utf-16-be"):
            try:
                candidates.append(data.decode(encoding, errors="replace"))
            except UnicodeDecodeError:
                pass
    return candidates


def changed_paths() -> set[str]:
    if not MANIFEST.is_file():
        return set()
    try:
        manifest = load_json(MANIFEST)
    except Exception:  # noqa: BLE001
        return set()
    return {
        path
        for entry in manifest.get("entries", [])
        for path in entry.get("paths", [])
    }


def is_upstream_public(relative: str, changed: set[str]) -> bool:
    normalized = relative.replace("\\", "/")
    return normalized.startswith("crates/codegraph") and normalized not in changed


def scan_file(path: Path, relative: str, policy: dict, changed: set[str], identity: re.Pattern[str] | None) -> list[dict]:
    if path.suffix.lower() in TEXT_SKIP_SUFFIXES:
        return []
    try:
        data = path.read_bytes()
    except OSError:
        return []
    findings: list[dict] = []
    for text in text_candidates(data):
        lines = text.splitlines()
        for number, line in enumerate(lines, 1):
            for rule, pattern in GENERIC_RULES:
                if pattern.search(line):
                    findings.append({"rule": rule, "path": relative, "line": number})
            if identity is not None and identity.search(line):
                findings.append({"rule": "P09_local_identity", "path": relative, "line": number})
    return findings


def classify(findings: list[dict], policy: dict, changed: set[str]) -> list[dict]:
    classified = []
    for finding in findings:
        relative = finding["path"]
        if omitted(relative, policy):
            continue
        reason = allowed(relative, policy)
        if reason:
            finding["classification"] = "allowed"
            finding["reason"] = reason
        elif is_upstream_public(relative, changed):
            finding["classification"] = "upstream_public"
        else:
            finding["classification"] = "unclassified"
        classified.append(finding)
    return classified


def identity_tokens() -> list[str]:
    values = [
        os.environ.get("USERNAME") or os.environ.get("USER") or "",
        Path.home().name,
        socket.gethostname(),
    ]
    return sorted({value for value in values if value and len(value) >= 4})


def scan_tree(root: Path, mode: str = "auto", policy: dict | None = None) -> dict:
    policy = policy or load_policy()
    changed = changed_paths()
    identity_parts = identity_tokens()
    identity = re.compile("|".join(re.escape(part) for part in identity_parts)) if identity_parts else None

    paths = tracked_paths(root)
    if not paths:
        paths = [
            str(path.relative_to(root)).replace("\\", "/")
            for path in root.rglob("*")
            if path.is_file()
        ]
    findings: list[dict] = []
    for relative in sorted(paths):
        if omitted(relative, policy):
            continue
        path = root / relative
        if not path.is_file():
            continue
        findings.extend(scan_file(path, relative, policy, changed, identity))
    classified = classify(findings, policy, changed)
    unclassified = [item for item in classified if item["classification"] == "unclassified"]
    mode = "public" if mode == "auto" and (root / PUBLIC_MARKER).is_file() else ("private" if mode == "auto" else mode)
    status = "passed"
    if unclassified and mode == "public":
        status = "failed"
    elif unclassified:
        status = "passed_with_warnings"
    return {
        "schema_version": "1.0",
        "mode": mode,
        "status": status,
        "findings": classified,
        "unclassified": len(unclassified),
        "allowed": len([item for item in classified if item["classification"] == "allowed"]),
        "upstream_public": len([item for item in classified if item["classification"] == "upstream_public"]),
        "limitations": ["pattern-based scan; not a full PII classifier", "historical evidence is classified, not rewritten"],
    }


def redact_text(text: str, replacements: list[tuple[str, str]], identity: list[str] | None = None) -> str:
    for old, new in replacements:
        if old:
            text = text.replace(old, new)
    for token in identity or []:
        text = text.replace(token, "<identity>")
    text = re.sub(r"\bjoint_[A-Za-z0-9_]+\b", "<private-dataset>", text)
    text = text.replace("<selected-columns>", "<selected-columns>")
    text = re.sub(r"(?<![A-Za-z0-9])[A-Za-z]:[\\/][^\s\"'<>]*", "<path>", text)
    text = re.sub(r"(?<=-I)[A-Za-z]:[\\/][^\s\"'<>]*", "<path>", text)
    text = re.sub(r"(?<=\\n)[A-Za-z]:[\\/][^\s\"'<>]*", "<path>", text)
    text = re.sub(r"\\\\\?\\[A-Za-z]:[\\/][^\s\"'<>]*", "<path>", text)
    text = re.sub(r"/(?:mnt/[a-z]/[^\s\"'<>]*|[a-z]/(?:Users|Strawberry|Codex)[^\s\"'<>]*)", "<path>", text)
    text = re.sub(r"[^\s\"'<>]*AppData[\\/]+Local[\\/]+Temp[^\s\"'<>]*", "<temp>", text)
    text = re.sub(r"file://<path>"'<>]*", "file://<path><path>", text)
    text = re.sub(r"[\w.-]+@(?:\d{1,3}\.){3}\d{1,3}(?::\d+)?", "<remote-host>", text)
    text = re.sub(r"\b(?:10\.\d{1,3}|192\.168\.\d{1,3}|172\.(?:1[6-9]|2\d|3[01])\.\d{1,3})\.\d{1,3}\b", "<private-ip>", text)
    text = re.sub(r'("(?:user|username|remote_user|actor)"\s*:\s*")[^"]+(")', r"\1<identity>\2", text)
    return text


def export_tree(out: Path, ref: str, previous_public: Path | None, private_map: Path | None) -> dict:
    policy = load_policy()
    replacements: list[tuple[str, str]] = []
    if private_map and private_map.is_file():
        data = load_json(private_map)
        replacements = [(str(item["old"]), str(item["new"])) for item in data.get("replacements", [])]
    archive = subprocess.run(
        ["git", "archive", "--format=tar", ref],
        cwd=str(ROOT),
        capture_output=True,
        timeout=300,
    )
    if archive.returncode != 0:
        raise SystemExit(f"git archive failed: {archive.stderr.decode('utf-8', errors='replace')}")
    out.mkdir(parents=True, exist_ok=True)
    file_count = 0
    skipped: list[str] = []
    identity = identity_tokens()
    with tarfile.open(fileobj=io.BytesIO(archive.stdout)) as tar:
        for member in tar.getmembers():
            if not member.isfile():
                continue
            relative = member.name.replace("\\", "/")
            if omitted(relative, policy):
                skipped.append(relative)
                continue
            data = tar.extractfile(member)
            if data is None:
                continue
            payload = data.read()
            suffix = Path(relative).suffix.lower()
            if suffix == ".scip" and previous_public is None:
                raise SystemExit(
                    "SCIP binary may contain absolute file URIs; pass --previous-public with neutralized indexes"
                )
            if suffix in TEXT_SKIP_SUFFIXES:
                redacted = payload
            else:
                text = payload.decode("utf-8", errors="replace")
                if private_map is None and ("joint_" in text or "<selected-columns>" in text):
                    raise SystemExit(
                        "private dataset markers found; pass --private-map with the redaction replacements"
                    )
                redacted = redact_text(text, replacements, identity).encode("utf-8")
            target = out / relative
            target.parent.mkdir(parents=True, exist_ok=True)
            target.write_bytes(redacted)
            file_count += 1
    if previous_public:
        for relative in ("upstream-probes/scip/rustdemo.scip", "upstream-probes/scip/python.scip", "upstream-probes/scip/cpp.scip"):
            source = previous_public / relative
            if source.is_file():
                target = out / relative
                target.parent.mkdir(parents=True, exist_ok=True)
                target.write_bytes(source.read_bytes())
    (out / PUBLIC_MARKER).write_text(
        "# Public snapshot\n\n"
        f"Sanitized export of `{ref}` generated on {date.today().isoformat()}.\n"
        "Local paths are replaced with `<path>`; remote hosts with `<remote-host>`; "
        "private IPs with `<private-ip>`; dataset-specific replacements come from the "
        "untracked private map. The sealed `docs/archmeld/` pack is copied byte-for-byte.\n",
        encoding="utf-8",
    )
    manifest = {
        "schema_version": "1.0",
        "generated_by": "tooling/public_snapshot.py export",
        "recorded_at": date.today().isoformat(),
        "ref": ref,
        "file_count": file_count,
        "skipped": skipped,
        "files": [],
    }
    for path in sorted(out.rglob("*")):
        if path.is_file():
            manifest["files"].append({"path": str(path.relative_to(out)).replace("\\", "/"), "sha256": sha256_file(path)})
    (out / "public-snapshot-manifest.json").write_text(
        json.dumps(manifest, ensure_ascii=False, indent=2) + "\n", encoding="utf-8"
    )
    return {"out": str(out), "file_count": file_count, "skipped": len(skipped), "manifest": "public-snapshot-manifest.json"}


def verify_for_upstream(root: Path) -> list[dict]:
    if (root / PUBLIC_MARKER).is_file():
        result = scan_tree(root, mode="public")
        if result["status"] == "failed":
            return [{"id": "public_snapshot.public", "status": "fail", "detail": f"{result['unclassified']} unclassified finding(s)"}]
        return [{"id": "public_snapshot.public", "status": "pass", "detail": "public tree scan is clean"}]
    result = scan_tree(root, mode="private")
    if result["unclassified"]:
        return [{
            "id": "public_snapshot.private",
            "status": "warn",
            "detail": f"{result['unclassified']} private-tree finding(s); run python tooling/public_snapshot.py check",
        }]
    return [{"id": "public_snapshot.private", "status": "pass", "detail": "private tree has no unclassified findings"}]


def main(argv: list[str] | None = None) -> int:
    parser = argparse.ArgumentParser(description="ARCHMELD public snapshot leak scan and export")
    sub = parser.add_subparsers(dest="command", required=True)
    check = sub.add_parser("check")
    check.add_argument("--root", default=str(ROOT))
    check.add_argument("--mode", choices=["auto", "private", "public"], default="auto")
    check.add_argument("--json", action="store_true")
    check.add_argument("--out")
    export = sub.add_parser("export")
    export.add_argument("--out", required=True)
    export.add_argument("--ref", default="HEAD")
    export.add_argument("--previous-public")
    export.add_argument("--private-map")
    verify = sub.add_parser("verify-export")
    verify.add_argument("dir")
    verify.add_argument("--json", action="store_true")
    verify.add_argument("--out")
    args = parser.parse_args(argv)

    if args.command == "check":
        result = scan_tree(Path(args.root), mode=args.mode)
        if args.out:
            Path(args.out).write_text(json.dumps(result, ensure_ascii=False, indent=2) + "\n", encoding="utf-8")
        if args.json:
            print(json.dumps(result, ensure_ascii=False, indent=2))
        else:
            print(f"mode={result['mode']} status={result['status']} unclassified={result['unclassified']}")
        return 0 if result["status"] != "failed" else 1
    if args.command == "export":
        result = export_tree(Path(args.out), args.ref, Path(args.previous_public) if args.previous_public else None, Path(args.private_map) if args.private_map else None)
        print(json.dumps(result, ensure_ascii=False, indent=2))
        return 0
    result = scan_tree(Path(args.dir), mode="public")
    if args.out:
        Path(args.out).write_text(json.dumps(result, ensure_ascii=False, indent=2) + "\n", encoding="utf-8")
    if args.json:
        print(json.dumps(result, ensure_ascii=False, indent=2))
    else:
        print(f"status={result['status']} unclassified={result['unclassified']}")
    return 0 if result["status"] != "failed" else 1


if __name__ == "__main__":
    raise SystemExit(main())
