#!/usr/bin/env python3
"""Block private-history branches from being pushed to the public fork.

The public repository only accepts `main` and `public-snapshot-*` branches.
This guard is used by the tracked `.githooks/pre-push` hook and by the
`archmeld-public-branch-guard` workflow. It fails closed for a public remote;
a private remote is left untouched.

Examples:
  py -3.11 tooling/publication_guard.py --install
  py -3.11 tooling/publication_guard.py --check refs/heads/archmeld/initial \
      --remote-url https://github.com/<identity>-sys/ARCHMELD.git
  py -3.11 tooling/publication_guard.py --ci
"""
from __future__ import annotations

import argparse
import fnmatch
import json
import os
import subprocess
import sys
from pathlib import Path
from typing import Iterable

ROOT = Path(__file__).resolve().parents[1]
DEFAULT_PUBLIC_PATTERNS = ("*<identity>-sys/archmeld*",)
ALLOWED_REFS = {"refs/heads/main"}
SNAPSHOT_PREFIX = "refs/heads/public-snapshot-"
ZERO_SHA = "0" * 40


def public_patterns() -> tuple[str, ...]:
    raw = os.environ.get("ARCHMELD_PUBLIC_REMOTE_PATTERNS", "")
    patterns = tuple(item.strip() for item in raw.split(",") if item.strip())
    return patterns or DEFAULT_PUBLIC_PATTERNS


def is_public_remote(remote_url: str | None) -> bool:
    if not remote_url:
        return False
    url = remote_url.strip().lower()
    return any(fnmatch.fnmatch(url, pattern.lower()) for pattern in public_patterns())


def allowed_ref(ref: str) -> bool:
    return ref in ALLOWED_REFS or ref.startswith(SNAPSHOT_PREFIX)


def is_deletion(local_sha: str, remote_sha: str) -> bool:
    return bool(local_sha) and set(local_sha) == {"0"} and bool(remote_sha) and set(remote_sha) != {"0"}


def resolve_remote_url(remote: str) -> str:
    if not remote:
        return ""
    try:
        result = subprocess.run(
            ["git", "remote", "get-url", remote],
            cwd=ROOT,
            capture_output=True,
            text=True,
            timeout=15,
        )
    except (OSError, subprocess.SubprocessError):
        return ""
    return result.stdout.strip() if result.returncode == 0 else ""


def override_enabled() -> bool:
    return os.environ.get("ARCHMELD_ALLOW_PUBLIC_BRANCH_PUSH") == "1"


def check_ref(
    ref: str,
    remote_url: str | None,
    remote_label: str = "remote",
    local_sha: str = "",
    remote_sha: str = "",
) -> tuple[bool, str]:
    """Return (allowed, reason) for one ref update."""
    if not is_public_remote(remote_url):
        return True, "not a public remote"
    if is_deletion(local_sha, remote_sha):
        if ref == "refs/heads/main":
            return False, "refusing to delete public main"
        return True, "branch deletion allowed"
    if allowed_ref(ref):
        return True, "allowed public ref"
    return (
        False,
        f"refusing {ref!r} on public remote {remote_label!r}; "
        "only refs/heads/main and refs/heads/public-snapshot-* are allowed",
    )


def rejection_message(ref: str, remote_label: str) -> str:
    return (
        f"\nRefusing to push {ref!r} to public remote {remote_label!r}.\n"
        "The public repository only accepts main and public-snapshot-* branches.\n"
        "Publish through a sanitized snapshot instead:\n"
        "  1. py -3.11 tooling/public_snapshot.py export --out <dir> --ref HEAD\n"
        "  2. py -3.11 tooling/public_snapshot.py verify-export <dir>\n"
        "  3. create a public-snapshot-* branch from public main and open a PR\n"
        "For cleanup of a mistakenly pushed branch only:\n"
        "  ARCHMELD_ALLOW_PUBLIC_BRANCH_PUSH=1 git push <remote> --delete <branch>\n"
    )


def hook(remote_label: str, remote_url: str, lines: Iterable[str]) -> int:
    url = remote_url or resolve_remote_url(remote_label)
    if not is_public_remote(url):
        return 0
    blocked = False
    for line in lines:
        parts = line.split()
        if len(parts) != 4:
            continue
        local_ref, local_sha, remote_ref, remote_sha = parts
        ok, reason = check_ref(
            remote_ref,
            url,
            remote_label=remote_label,
            local_sha=local_sha,
            remote_sha=remote_sha,
        )
        if ok:
            continue
        if override_enabled():
            print(
                f"WARNING: public branch guard overridden for {remote_ref}: {reason}",
                file=sys.stderr,
            )
            continue
        print(rejection_message(remote_ref, remote_label), file=sys.stderr)
        blocked = True
    return 1 if blocked else 0


def ci_ref() -> str:
    ref = os.environ.get("GITHUB_REF", "").strip()
    if ref.startswith("refs/heads/"):
        return ref
    head = os.environ.get("GITHUB_HEAD_REF", "").strip()
    if head:
        return f"refs/heads/{head}"
    return ""


def ci_remote_url() -> str:
    repository = os.environ.get("GITHUB_REPOSITORY", "").strip()
    if not repository:
        return ""
    server = os.environ.get("GITHUB_SERVER_URL", "https://github.com").rstrip("/")
    return f"{server}/{repository}.git"


def install() -> int:
    result = subprocess.run(
        ["git", "config", "core.hooksPath", ".githooks"],
        cwd=ROOT,
        capture_output=True,
        text=True,
        timeout=30,
    )
    if result.returncode != 0:
        print(f"failed to set core.hooksPath: {result.stderr.strip()}", file=sys.stderr)
        return result.returncode
    print("core.hooksPath=.githooks (pre-push guard enabled for this clone)")
    return 0


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--hook", action="store_true", help="read pre-push stdin")
    parser.add_argument("--check", metavar="REF", help="check one ref")
    parser.add_argument("--remote", default="origin")
    parser.add_argument("--remote-url", default="")
    parser.add_argument("--ci", action="store_true")
    parser.add_argument("--install", action="store_true")
    parser.add_argument("--json", action="store_true")
    parser.add_argument("hook_args", nargs="*")
    args = parser.parse_args()

    if args.install:
        return install()

    if args.hook:
        remote_label = args.hook_args[0] if args.hook_args else args.remote
        remote_url = args.hook_args[1] if len(args.hook_args) > 1 else args.remote_url
        return hook(remote_label, remote_url, sys.stdin)

    if args.ci:
        ref = ci_ref()
        remote_url = ci_remote_url()
        if not ref:
            print("publication guard: no branch ref in GitHub event; skipping", file=sys.stderr)
            return 0
        ok, reason = check_ref(ref, remote_url, remote_label="public repository")
        if ok:
            print(f"publication guard: allowed {ref}")
            return 0
        print(rejection_message(ref, "public repository"), file=sys.stderr)
        print(f"publication guard: {reason}", file=sys.stderr)
        return 1

    if args.check:
        url = args.remote_url or resolve_remote_url(args.remote)
        ok, reason = check_ref(args.check, url, remote_label=args.remote)
        payload = {"ref": args.check, "remote": args.remote, "remote_url": url, "allowed": ok, "reason": reason}
        print(json.dumps(payload, ensure_ascii=False, indent=2) if args.json else f"{'allowed' if ok else 'blocked'}: {reason}")
        return 0 if ok else 1

    parser.error("one of --hook, --check, --ci or --install is required")
    return 2


if __name__ == "__main__":
    raise SystemExit(main())
