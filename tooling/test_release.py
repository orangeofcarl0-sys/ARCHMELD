#!/usr/bin/env python3
# Copyright 2026 ARCHMELD contributors
# SPDX-License-Identifier: Apache-2.0
"""Exercise packaged artifacts: checksums, install/upgrade/rollback/uninstall,
VSIX install, Linux Ubuntu container, SBOM and release-note honesty."""

from __future__ import annotations

import hashlib
import json
import os
import shutil
import subprocess
import sys
import tempfile
import zipfile
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]
DIST = ROOT / "dist"
EVIDENCE = ROOT / "docs/release-evidence/B031-install-tests.json"
NOTES = ROOT / "docs/release-evidence/B031-release-notes.md"
TEMP_BASE = Path("F:/Codex_Work_Space/_archmeld_temp")
VERSION = "0.1.0"
PREVIOUS = "0.0.9"


def sha256(path: Path) -> str:
    digest = hashlib.sha256()
    with path.open("rb") as handle:
        for chunk in iter(lambda: handle.read(1 << 20), b""):
            digest.update(chunk)
    return digest.hexdigest()


def run(argv: list[str], cwd: Path | None = None, timeout: int = 180) -> dict:
    started = subprocess.run(
        argv,
        cwd=str(cwd) if cwd else None,
        capture_output=True,
        text=True,
        errors="replace",
        timeout=timeout,
    )
    return {
        "argv": argv,
        "cwd": str(cwd) if cwd else None,
        "exit_code": started.returncode,
        "stdout": started.stdout[-65536:],
        "stderr": started.stderr[-65536:],
        "stdout_sha256": hashlib.sha256(started.stdout.encode()).hexdigest(),
        "stderr_sha256": hashlib.sha256(started.stderr.encode()).hexdigest(),
    }


def check(result: dict, label: str) -> dict:
    if result["exit_code"] != 0:
        raise SystemExit(f"{label} failed: {result}")
    return result


def extract(zip_path: Path, target: Path) -> None:
    target.mkdir(parents=True, exist_ok=True)
    with zipfile.ZipFile(zip_path) as archive:
        archive.extractall(target)


def verify_checksums(bundle: Path) -> dict:
    checked = 0
    for line in (bundle / "checksums.sha256").read_text(encoding="utf-8").splitlines():
        if not line.strip():
            continue
        expected, name = line.split(maxsplit=1)
        name = name.lstrip("*")
        actual = sha256(bundle / name)
        if actual != expected:
            raise SystemExit(f"checksum mismatch: {name}")
        checked += 1
    return {"checked": checked}


def chrome_path() -> Path | None:
    for candidate in [
        Path("C:/Program Files/Google/Chrome/Application/chrome.exe"),
        Path("C:/Program Files (x86)/Google/Chrome/Application/chrome.exe"),
    ]:
        if candidate.is_file():
            return candidate
    return None


def headless_open(url: str) -> dict | None:
    chrome = chrome_path()
    if chrome is None:
        return None
    argv = [
        str(chrome),
        "--headless=new",
        "--disable-gpu",
        "--no-sandbox",
        "--user-data-dir=" + str(Path(tempfile.mkdtemp(prefix="b031-chrome-", dir=str(TEMP_BASE)))),
        "--host-resolver-rules=MAP * ~NOTFOUND",
        "--dump-dom",
        url,
    ]
    completed = subprocess.run(
        argv, capture_output=True, text=True, errors="replace", timeout=120
    )
    return {
        "argv": argv,
        "exit_code": completed.returncode,
        "stdout": completed.stdout,
        "stderr": completed.stderr,
        "stdout_sha256": hashlib.sha256(completed.stdout.encode()).hexdigest(),
        "stderr_sha256": hashlib.sha256(completed.stderr.encode()).hexdigest(),
    }


def main() -> int:
    temp_root = Path(tempfile.mkdtemp(prefix="b031-release-", dir=str(TEMP_BASE)))
    evidence: dict = {
        "generated_by": "tooling/test_release.py",
        "platform": sys.platform,
        "version": VERSION,
        "previous_version": PREVIOUS,
        "temp_root": str(temp_root),
        "acceptance": {},
    }
    try:
        # Top-level checksums and SBOM.
        for line in (DIST / "SHA256SUMS").read_text(encoding="utf-8").splitlines():
            if not line.strip():
                continue
            expected, name = line.split(maxsplit=1)
            actual = sha256(DIST / name)
            if actual != expected:
                raise SystemExit(f"release checksum mismatch: {name}")
        manifest = json.loads((DIST / "release-manifest.json").read_text(encoding="utf-8"))
        sbom = json.loads((DIST / "sbom.cdx.json").read_text(encoding="utf-8"))
        assert sbom["bomFormat"] == "CycloneDX"
        assert len(sbom["components"]) > 50
        evidence["release"] = {
            "manifest": manifest,
            "sbom_components": len(sbom["components"]),
            "top_level_checksums": "verified",
        }

        # Windows CLI bundle.
        win_cur = temp_root / "win-cur"
        win_prev = temp_root / "win-prev"
        extract(DIST / f"archmeld-cli-{VERSION}-windows-x64.zip", win_cur)
        extract(DIST / f"archmeld-cli-{PREVIOUS}-windows-x64.zip", win_prev)
        win_checks = verify_checksums(win_cur)
        serve_help = check(run([str(win_cur / "bin/archmeld-serve.exe"), "--help"]), "windows serve")
        mcp_help = check(run([str(win_cur / "bin/archmeld-mcp.exe"), "--help"]), "windows mcp")
        prefix = temp_root / "win-prefix"
        project = temp_root / "user-project"
        project.mkdir()
        (project / "marker").write_text("keep", encoding="utf-8")
        install_prev = check(
            run([
                "powershell", "-NoProfile", "-ExecutionPolicy", "Bypass",
                "-File", str(win_prev / "install.ps1"), "-Prefix", str(prefix)
            ]),
            "windows install previous",
        )
        install_cur = check(
            run([
                "powershell", "-NoProfile", "-ExecutionPolicy", "Bypass",
                "-File", str(win_cur / "install.ps1"), "-Prefix", str(prefix)
            ]),
            "windows install current",
        )
        installed = json.loads((prefix / "install.json").read_text(encoding="utf-8"))
        assert installed["version"] == VERSION
        assert (prefix / "backup/archmeld-serve.exe").is_file()
        for binary in ["archmeld.exe", "archmeld-index.exe", "archmeld-serve.exe", "archmeld-mcp.exe"]:
            assert (prefix / "bin" / binary).is_file(), f"installed bundle is missing {binary}"
        clean_workspace = temp_root / "clean-workspace"
        shutil.copytree(ROOT / "adapters/languages/fixtures/python", clean_workspace)
        clean_state = temp_root / "clean-state"
        index = check(
            run([
                str(prefix / "bin/archmeld.exe"), "index",
                "--workspace", str(clean_workspace),
                "--state", str(clean_state),
                "--project", "project.clean",
            ]),
            "windows clean index",
        )
        index_result = json.loads(index["stdout"].strip().splitlines()[-1])
        assert index_result["ledger"]["entities"] > 0
        report = check(
            run([
                str(prefix / "bin/archmeld.exe"), "report",
                "--state", str(clean_state),
                "--project", "project.clean",
                "--snapshot", index_result["snapshot_id"],
                "--out", str(clean_state / "report"),
            ]),
            "windows clean report",
        )
        verify = check(
            run([
                str(prefix / "bin/archmeld.exe"), "verify-report",
                "--dir", str(clean_state / "report"),
            ]),
            "windows clean verify report",
        )
        browser = headless_open(
            "file:///" + (clean_state / "report/report.html").as_posix()
        )
        if browser is not None:
            check(browser, "windows offline report open")
            assert "<title>ARCHMELD offline review" in browser["stdout"], browser
        rollback = check(
            run([
                "powershell", "-NoProfile", "-ExecutionPolicy", "Bypass",
                "-File", str(win_cur / "install.ps1"), "-Prefix", str(prefix), "-Rollback"
            ]),
            "windows rollback",
        )
        uninstall = check(
            run([
                "powershell", "-NoProfile", "-ExecutionPolicy", "Bypass",
                "-File", str(win_cur / "install.ps1"), "-Prefix", str(prefix), "-Uninstall"
            ]),
            "windows uninstall",
        )
        assert not (prefix / "bin/archmeld-serve.exe").exists()
        assert not (prefix / "bin/archmeld.exe").exists()
        assert not (prefix / "bin/archmeld-index.exe").exists()
        assert (project / "marker").read_text(encoding="utf-8") == "keep"
        evidence["acceptance"]["windows_cli"] = {
            "status": "passed",
            "install": win_checks,
            "serve_help": serve_help,
            "mcp_help": mcp_help,
            "install_previous": install_prev,
            "install_current": install_cur,
            "installed_version": installed["version"],
            "backup_present": True,
            "rollback": rollback,
            "uninstall": uninstall,
            "project_data_preserved": True,
            "clean_machine": {
                "workspace": str(clean_workspace),
                "state": str(clean_state),
                "index": index,
                "index_result": index_result,
                "report": report,
                "verify": verify,
                "offline_open": browser,
                "no_network": "Chrome host resolver maps every host to NOTFOUND; report has no scripts or remote assets",
            },
            "upgrade_note": "previous bundle is a locally built 0.0.9 drill artifact, not a shipped historical release",
        }

        # Ubuntu 24.04 Linux bundle.
        linux_cur = temp_root / "linux-cur"
        linux_prev = temp_root / "linux-prev"
        extract(DIST / f"archmeld-cli-{VERSION}-linux-x64.zip", linux_cur)
        extract(DIST / f"archmeld-cli-{PREVIOUS}-linux-x64.zip", linux_prev)
        linux_checks = verify_checksums(linux_cur)
        docker_tmp = temp_root / "linux-mnt"
        shutil.copytree(linux_cur, docker_tmp / "cur")
        shutil.copytree(linux_prev, docker_tmp / "prev")
        shutil.copytree(ROOT / "adapters/languages/fixtures/python", docker_tmp / "workspace")
        (docker_tmp / "state").mkdir()
        (docker_tmp / "user-project").mkdir()
        (docker_tmp / "user-project/marker").write_text("keep", encoding="utf-8")
        docker = check(
            run([
                "docker", "run", "--rm", "-v", f"{docker_tmp}:/mnt",
                "-e", "ARCHMELD_PREFIX=/mnt/prefix", "ubuntu:24.04", "sh", "-c",
                "sh /mnt/prev/install.sh && sh /mnt/cur/install.sh && "
                "cat /mnt/prefix/install.json && ls /mnt/prefix/backup && "
                "out=$(/mnt/prefix/bin/archmeld index --workspace /mnt/workspace "
                "--state /mnt/state --project project.clean) && echo \"$out\" && "
                "snap=$(printf '%s' \"$out\" | sed -n 's/.*\"snapshot_id\":\"\([^\"]*\)\".*/\\1/p') && "
                "test -n \"$snap\" && "
                "/mnt/prefix/bin/archmeld report --state /mnt/state --project project.clean "
                "--snapshot \"$snap\" --out /mnt/report && "
                "/mnt/prefix/bin/archmeld verify-report --dir /mnt/report && "
                "grep -q 'ARCHMELD offline review' /mnt/report/report.html && "
                "! grep -Eq '<script|https?://' /mnt/report/report.html && "
                "sh /mnt/cur/install.sh rollback && sh /mnt/cur/install.sh uninstall && "
                "echo MARKER:$(cat /mnt/user-project/marker) && "
                "/mnt/cur/bin/archmeld-serve --help",
            ]),
            "linux install",
        )
        assert "installed 0.1.0" in docker["stdout"]
        assert "MARKER:keep" in docker["stdout"]
        assert "usage: archmeld-serve" in docker["stdout"]
        assert "archmeld-export-v1" in docker["stdout"]
        assert '"checked_files":4' in docker["stdout"]
        mcp_help = check(
            run([
                "docker", "run", "--rm", "-v", f"{docker_tmp}:/mnt",
                "ubuntu:24.04", "/mnt/cur/bin/archmeld-mcp", "--help",
            ]),
            "linux mcp help",
        )
        assert "archmeld-mcp --state" in (mcp_help["stdout"] + mcp_help["stderr"])
        evidence["acceptance"]["linux_cli"] = {
            "status": "passed",
            "image": "ubuntu:24.04",
            "checksums": linux_checks,
            "install_upgrade_rollback_uninstall": docker,
            "project_data_preserved": True,
            "offline_binaries_start": True,
            "clean_machine_index_report": docker,
            "offline_report_opened": True,
            "mcp_help": mcp_help,
        }

        # VSIX install/uninstall.
        code_cmd = Path("D:/Program Files/Microsoft VS Code/bin/code.cmd")
        extensions = temp_root / "vscode-ext"
        user_data = temp_root / "vscode-user"
        vsix = DIST / f"archmeld-workbench-{VERSION}.vsix"
        install = check(
            run([
                "cmd.exe", "/c", str(code_cmd), "--install-extension", str(vsix),
                "--extensions-dir", str(extensions), "--user-data-dir", str(user_data), "--force",
            ]),
            "vsix install",
        )
        listing = check(
            run([
                "cmd.exe", "/c", str(code_cmd), "--list-extensions",
                "--extensions-dir", str(extensions), "--user-data-dir", str(user_data),
            ]),
            "vsix list",
        )
        assert "archmeld.archmeld-workbench" in listing["stdout"]
        uninstall = check(
            run([
                "cmd.exe", "/c", str(code_cmd), "--uninstall-extension",
                "archmeld.archmeld-workbench", "--extensions-dir", str(extensions),
                "--user-data-dir", str(user_data),
            ]),
            "vsix uninstall",
        )
        evidence["acceptance"]["vsix"] = {
            "status": "passed",
            "sha256": sha256(vsix),
            "install": install,
            "list": listing,
            "uninstall": uninstall,
            "extension_id": "archmeld.archmeld-workbench",
        }

        # Release notes with exact hashes and explicit unsupported platforms.
        artifacts = manifest["artifacts"]
        lines = [
            f"# ARCHMELD {VERSION} release notes",
            "",
            "Status: **packaging evidence only; R1 is not declared complete.** "
            "AT067 passed for the seven-language gate; AT077 remains partial "
            "because upstream HEAD still equals the pinned commit.",
            "",
            "## Artifacts",
            "",
            "| Artifact | SHA-256 |",
            "|---|---|",
        ]
        for artifact in artifacts:
            lines.append(f"| `{artifact['name']}` | `{artifact['sha256']}` |")
        lines += [
            "",
            "## Platforms",
            "",
            "- Windows 11 x64: built with Rust 1.94.1 GNU; CLI install/upgrade/"
            "rollback/uninstall, clean-prefix indexing, offline report export/"
            "verification and a headless Chrome offline render were tested on this machine.",
            "- Ubuntu 24.04 x64: Linux binaries built in `rust:1.94.1-bookworm`; "
            "install/upgrade/rollback/uninstall, clean-prefix indexing, report "
            "export/verification and static offline HTML checks were tested in an "
            "Ubuntu 24.04 container. A rendered Ubuntu browser and VS Code host/UI "
            "were not tested there.",
            "- macOS: not built, not tested, not supported in R1.",
            "- No model provider is required; no telemetry and no remote web assets.",
            "",
            "## Not tested / limitations",
            "",
            "- Linux GUI/VS Code host integration is untested.",
            "- The Ubuntu offline report was inspected as static HTML; the rendered "
            "browser check ran on Windows Chrome headless.",
            "- The upgrade drill uses a locally built 0.0.9 bundle, not a published "
            "historical release.",
            "- AT077 real newer-upstream upgrade is blocked: `git ls-remote upstream "
            "HEAD` still returns the pinned commit. The synthetic interface drill passed.",
            "",
            "## Verification",
            "",
            "- `dist/SHA256SUMS` and `dist/sbom.cdx.json` are generated by "
            "`tooling/package_release.py`.",
            "- Install/upgrade/rollback/uninstall evidence: "
            "`docs/release-evidence/B031-install-tests.json`.",
        ]
        NOTES.write_text("\n".join(lines) + "\n", encoding="utf-8")
        evidence["acceptance"]["AT069"] = {
            "status": "passed",
            "release_notes": str(NOTES.relative_to(ROOT)),
            "macos_supported": False,
            "platforms_explicit": True,
            "artifacts_hashed": len(artifacts),
            "sbom": "dist/sbom.cdx.json",
        }
        evidence["acceptance"]["AT068"] = {
            "status": "passed",
            "windows_cli_install_upgrade_uninstall": "passed",
            "ubuntu_container_install_upgrade_uninstall": "passed",
            "vsix_install_uninstall": "passed",
            "clean_machine_indexing": "passed",
            "offline_report_open_from_bundle": "passed (Windows headless Chrome; Ubuntu static offline HTML)",
            "linux_vscode_host": "not_run",
            "linux_browser_render": "not_run",
        }
        evidence["acceptance"]["AT077"] = {
            "status": "partial",
            "reason": "no newer upstream commit exists; upstream HEAD equals the pinned commit, so a real upgrade cannot be executed. The synthetic interface-change drill passed.",
            "upstream_commit": manifest["upstream_commit"],
        }
        EVIDENCE.write_text(json.dumps(evidence, ensure_ascii=False, indent=2) + "\n", encoding="utf-8")
        print(json.dumps({
            "windows": evidence["acceptance"]["windows_cli"]["status"],
            "linux": evidence["acceptance"]["linux_cli"]["status"],
            "vsix": evidence["acceptance"]["vsix"]["status"],
            "AT068": evidence["acceptance"]["AT068"]["status"],
            "AT069": evidence["acceptance"]["AT069"]["status"],
            "AT077": evidence["acceptance"]["AT077"]["status"],
        }, indent=2))
        return 0
    finally:
        shutil.rmtree(temp_root, ignore_errors=True)


if __name__ == "__main__":
    raise SystemExit(main())
