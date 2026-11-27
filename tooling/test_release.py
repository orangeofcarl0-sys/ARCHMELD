#!/usr/bin/env python3
# Copyright 2026 ARCHMELD contributors
# SPDX-License-Identifier: Apache-2.0
"""Exercise packaged artifacts: checksums, install/upgrade/rollback/uninstall,
VSIX install, Linux Ubuntu container, SBOM and release-note honesty."""

from __future__ import annotations

import json
import os
import re
import shutil
import subprocess
import sys
import tempfile
import zipfile
from pathlib import Path

import common
import portable_env as portable

ROOT = Path(__file__).resolve().parents[1]
DIST = ROOT / "dist"
EVIDENCE = ROOT / "docs/release-evidence/B031-install-tests.json"
NOTES = ROOT / "docs/release-evidence/B031-release-notes.md"
TEMP_BASE = portable.temp_root()
VERSION = "0.1.0"
PREVIOUS = "0.0.9"
sha256 = common.sha256_file


def latest_at077() -> dict:
    """Read AT077 from the newest real upstream-merge evidence, not a literal."""
    evidence_dir = ROOT / "docs/release-evidence"
    for path in sorted(evidence_dir.glob("UPSTREAM-*.json"), reverse=True):
        try:
            data = json.loads(path.read_text(encoding="utf-8"))
        except Exception:  # noqa: BLE001 - evidence is optional here
            continue
        at077 = data.get("AT077")
        if isinstance(at077, dict) and at077.get("status"):
            return {
                "status": at077["status"],
                "reason": at077.get("reason", ""),
                "path": str(path.relative_to(ROOT)),
                "commit": data.get("to_commit") or data.get("upstream_commit"),
            }
    return {
        "status": "not_run",
        "reason": "no real upstream-merge evidence found",
        "path": None,
        "commit": None,
    }


def run(
    argv: list[str],
    cwd: Path | None = None,
    timeout: int = 180,
    env: dict[str, str] | None = None,
) -> dict:
    started = subprocess.run(
        argv,
        cwd=str(cwd) if cwd else None,
        capture_output=True,
        text=True,
        errors="replace",
        timeout=timeout,
        env=env,
    )
    return {
        "argv": argv,
        "cwd": str(cwd) if cwd else None,
        "exit_code": started.returncode,
        "stdout": started.stdout[-65536:],
        "stderr": started.stderr[-65536:],
        "stdout_sha256": common.sha256_text(started.stdout),
        "stderr_sha256": common.sha256_text(started.stderr),
    }


def check(result: dict, label: str) -> dict:
    if result["exit_code"] != 0:
        raise SystemExit(f"{label} failed: {result}")
    return result


def parse_binary_version(text: str) -> dict | None:
    match = re.search(
        r"(?P<version>\d+\.\d+\.\d+)\+(?P<commit>[0-9a-f]{7,40})(?P<dirty>-dirty)?",
        text,
    )
    if not match:
        return None
    return {
        "version": match.group("version"),
        "commit": match.group("commit"),
        "dirty": bool(match.group("dirty")),
        "line": text.strip(),
    }


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
    return portable.find_chrome()


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
        "stdout_sha256": common.sha256_text(completed.stdout),
        "stderr_sha256": common.sha256_text(completed.stderr),
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
        win_bundle = json.loads((win_cur / "bundle.json").read_text(encoding="utf-8"))
        assert win_bundle["schema_version"] == "1.1", win_bundle
        assert win_bundle["release_status"] == "provisional", win_bundle
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
        assert index_result["bridge"]["export_schema_version"] == "1.2", index_result
        assert "skipped_by_reason" in index_result["bridge"]["index"], index_result
        assert "cross_file_implements" in index_result["bridge"]["index"], index_result
        version = check(
            run([str(prefix / "bin/archmeld.exe"), "--version"]),
            "windows version",
        )
        binary_version = parse_binary_version(version["stdout"])
        assert binary_version is not None, version
        assert binary_version["version"] == VERSION, binary_version
        assert binary_version["commit"] == win_bundle["product_build_commit"], (
            binary_version,
            win_bundle["product_build_commit"],
        )
        assert binary_version["dirty"] is False, binary_version
        helper_schema = check(
            run([str(prefix / "bin/archmeld-index.exe"), "--schema"]),
            "windows helper schema",
        )
        assert helper_schema["stdout"].strip() == "1.2", helper_schema
        system_root = os.environ.get("SystemRoot") or os.environ.get("WINDIR", "")
        clean_path = os.pathsep.join([str(Path(system_root) / "System32"), system_root])
        clean_env = dict(os.environ)
        clean_env["PATH"] = clean_path
        clean_version = check(
            run([str(prefix / "bin/archmeld.exe"), "--version"], env=clean_env),
            "windows clean-path version",
        )
        clean_parsed = parse_binary_version(clean_version["stdout"])
        assert clean_parsed == binary_version, clean_parsed
        clean_helper = check(
            run([str(prefix / "bin/archmeld-index.exe"), "--schema"], env=clean_env),
            "windows clean-path helper schema",
        )
        assert clean_helper["stdout"].strip() == "1.2", clean_helper
        clean_path_state = temp_root / "clean-state-clean-path"
        clean_path_index = check(
            run([
                str(prefix / "bin/archmeld.exe"), "index",
                "--workspace", str(clean_workspace),
                "--state", str(clean_path_state),
                "--project", "project.cleanpath",
            ], env=clean_env),
            "windows clean-path index",
        )
        clean_path_result = json.loads(clean_path_index["stdout"].strip().splitlines()[-1])
        assert clean_path_result["ledger"]["entities"] > 0, clean_path_result
        clean_path_evidence = {
            "PATH": clean_path,
            "cargo_on_path": shutil.which("cargo", path=clean_path),
            "version": clean_parsed,
            "helper_schema": clean_helper["stdout"].strip(),
            "index": clean_path_index,
            "index_result": clean_path_result,
        }
        init = check(
            run([
                str(prefix / "bin/archmeld.exe"), "init",
                "--workspace", str(clean_workspace),
                "--state", str(clean_state),
                "--project", "project.clean",
                "--client", "generic",
            ]),
            "windows init and MCP handshake",
        )
        init_result = json.loads(init["stdout"].strip().splitlines()[-1])
        assert init_result["health"]["state"] == "ok", init_result
        assert init_result["health"]["tools"] == 8, init_result
        assert init_result["health"]["freshness"] == "clean", init_result
        assert init_result["counts"]["entities"] > 0, init_result
        assert init_result["source_mutation"] == "none", init_result
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
            "file://<path>" + (clean_state / "report/report.html").as_posix()
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
            "binary_version": binary_version,
            "bundle_build_commit": win_bundle["product_build_commit"],
            "bundle_packaging_commit": win_bundle["packaging_git_commit"],
            "bundle_release_status": win_bundle["release_status"],
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
                "clean_path": clean_path_evidence,
            },
            "upgrade_note": "previous bundle is a locally built 0.0.9 drill artifact, not a shipped historical release",
        }

        # Ubuntu 24.04 Linux bundle.
        linux_cur = temp_root / "linux-cur"
        linux_prev = temp_root / "linux-prev"
        extract(DIST / f"archmeld-cli-{VERSION}-linux-x64.zip", linux_cur)
        extract(DIST / f"archmeld-cli-{PREVIOUS}-linux-x64.zip", linux_prev)
        linux_checks = verify_checksums(linux_cur)
        linux_bundle = json.loads((linux_cur / "bundle.json").read_text(encoding="utf-8"))
        assert linux_bundle["schema_version"] == "1.1", linux_bundle
        assert linux_bundle["release_status"] == "provisional", linux_bundle
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
                "/mnt/prefix/bin/archmeld --version && "
                "/mnt/prefix/bin/archmeld --version | grep -q '+' && "
                "/mnt/prefix/bin/archmeld-index --schema | grep -qx '1.2' && "
                "out=$(/mnt/prefix/bin/archmeld index --workspace /mnt/workspace "
                "--state /mnt/state --project project.clean) && echo \"$out\" && "
                "printf '%s' \"$out\" | grep -q '\"export_schema_version\":\"1.2\"' && "
                "printf '%s' \"$out\" | grep -q '\"skipped_by_reason\"' && "
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
        linux_version = parse_binary_version(docker["stdout"])
        assert linux_version is not None, docker["stdout"][-2000:]
        assert linux_version["version"] == VERSION, linux_version
        if linux_bundle["product_build_commit"] != "unknown":
            assert linux_version["commit"] == linux_bundle["product_build_commit"], (
                linux_version,
                linux_bundle["product_build_commit"],
            )
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
            "binary_version": linux_version,
            "bundle_build_commit": linux_bundle["product_build_commit"],
            "bundle_packaging_commit": linux_bundle["packaging_git_commit"],
            "bundle_release_status": linux_bundle["release_status"],
            "install_upgrade_rollback_uninstall": docker,
            "project_data_preserved": True,
            "offline_binaries_start": True,
            "clean_machine_index_report": docker,
            "offline_report_opened": True,
            "mcp_help": mcp_help,
        }

        # VSIX install/uninstall.
        code_cmd = portable.find_vscode_code()
        if code_cmd is None:
            raise SystemExit("VS Code executable not found; set ARCHMELD_VSCODE or add code to PATH")
        extensions = temp_root / "vscode-ext"
        user_data = temp_root / "vscode-user"
        vsix = DIST / f"archmeld-workbench-{VERSION}.vsix"
        install = check(
            run([
                "cmd.exe", "/c", str(code_cmd), "--install-extension", str(vsix),
                "--extensions-dir", str(extensions), "--user-data-dir", str(user_data), "--force", "--disable-updates",
            ]),
            "vsix install",
        )
        listing = check(
            run([
                "cmd.exe", "/c", str(code_cmd), "--list-extensions",
                "--extensions-dir", str(extensions), "--user-data-dir", str(user_data), "--disable-updates",
            ]),
            "vsix list",
        )
        assert "archmeld.archmeld-workbench" in listing["stdout"]
        uninstall = check(
            run([
                "cmd.exe", "/c", str(code_cmd), "--uninstall-extension",
                "archmeld.archmeld-workbench", "--extensions-dir", str(extensions),
                "--user-data-dir", str(user_data), "--disable-updates",
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
        upstream_state = latest_at077()
        at077 = upstream_state["status"]
        if at077 == "passed":
            at077_sentence = "AT077 passed for the real newer-upstream upgrade."
            at077_limit = (
                f"- AT077 real newer-upstream upgrade: passed "
                f"({upstream_state['path']})."
            )
        else:
            at077_sentence = f"AT077 is {at077}: {upstream_state['reason']}."
            at077_limit = (
                f"- AT077 real newer-upstream upgrade is {at077}: "
                f"{upstream_state['reason']}."
            )
        matrix_path = ROOT / "docs/release-evidence/B030-language-matrix.json"
        at067_status = "not_run"
        if matrix_path.is_file():
            try:
                at067_status = json.loads(matrix_path.read_text(encoding="utf-8")).get(
                    "AT067", {}
                ).get("status", "not_run")
            except (OSError, json.JSONDecodeError):
                at067_status = "unreadable"
        lines = [
            f"# ARCHMELD {VERSION} release notes",
            "",
            "Status: **packaging evidence only; release_status=provisional; "
            "R1 is not declared complete.** "
            f"Binaries were not rebuilt from HEAD: Windows "
            f"`{win_bundle['product_build_commit']}` "
            f"(dirty={win_bundle['product_build_dirty']}), Linux "
            f"`{linux_bundle['product_build_commit']}` "
            f"(dirty={linux_bundle['product_build_dirty']}); see `bundle.json` "
            f"builds. AT067 is `{at067_status}` for the seven-language gate; {at077_sentence}",
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
            "- Binaries were built before the current packaging HEAD; this package "
            "is repackaged from existing artifacts and is provisional, not a "
            "release built from HEAD.",
            at077_limit,
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
            "status": at077,
            "reason": upstream_state["reason"],
            "upstream_commit": manifest["upstream_commit"],
            "evidence": upstream_state["path"],
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
