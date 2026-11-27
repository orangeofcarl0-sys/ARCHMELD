#!/usr/bin/env python3
# Copyright 2026 ARCHMELD contributors
# SPDX-License-Identifier: Apache-2.0
"""Build offline CLI bundles, a VSIX and SBOM/checksums from release binaries.

Binaries are produced outside this script:
  cargo build --release -p archmeld-api                       (Windows)
  docker ... CARGO_TARGET_DIR=/work/target-linux cargo build --release -p archmeld-api
"""

from __future__ import annotations

import hashlib
import json
import os
import re
import shutil
import subprocess
import sys
import zipfile
from pathlib import Path

try:
    import tomllib
except ModuleNotFoundError:  # Python 3.10 fallback
    tomllib = None

ROOT = Path(__file__).resolve().parents[1]
DIST = ROOT / "dist"
VERSION = os.environ.get("ARCHMELD_VERSION", "0.1.0")


def upstream_commit() -> str:
    """Read the CodeGraph pin from the registry, falling back to the tool lock."""
    registry = ROOT / "upstreams.lock.json"
    if registry.is_file():
        data = json.loads(registry.read_text(encoding="utf-8"))
        for entry in data.get("upstreams", []):
            if entry.get("id") == "codegraph":
                commit = entry.get("pin", {}).get("commit")
                if commit:
                    return commit
    lock = ROOT / "toolchains.lock.json"
    if lock.is_file():
        data = json.loads(lock.read_text(encoding="utf-8"))
        commit = data.get("pinned", {}).get("codegraph_commit")
        if commit:
            return commit
    raise SystemExit("cannot determine the CodeGraph pin from upstreams.lock.json or toolchains.lock.json")


UPSTREAM = upstream_commit()
TOOLCHAIN = "rust 1.94.1; node 24.14.1"


def product_commit() -> str:
    result = subprocess.run(
        ["git", "-C", str(ROOT), "rev-parse", "HEAD"],
        capture_output=True,
        text=True,
    )
    return result.stdout.strip() if result.returncode == 0 else "unknown"


def packaging_dirty() -> bool:
    result = subprocess.run(
        ["git", "-C", str(ROOT), "status", "--porcelain"],
        capture_output=True,
        text=True,
    )
    return result.returncode == 0 and bool(result.stdout.strip())


def parse_version_line(text: str) -> tuple[str | None, str | None, bool | None]:
    """Parse `archmeld 0.1.0+dd124ca1` or a helper version line."""
    match = re.search(
        r"(?P<version>\d+\.\d+\.\d+)\+(?P<commit>[0-9a-f]{7,40})(?P<dirty>-dirty)?",
        text,
    )
    if not match:
        match = re.search(
            r"commit\s+(?P<commit>[0-9a-f]{7,40})(?P<dirty>-dirty)?", text
        )
        if not match:
            return None, None, None
        return None, match.group("commit"), bool(match.group("dirty"))
    return (
        match.group("version"),
        match.group("commit"),
        bool(match.group("dirty")),
    )


def product_build_metadata(binary: Path, platform: str) -> dict:
    if platform == "windows-x64":
        result = subprocess.run(
            [str(binary), "--version"], capture_output=True, text=True, timeout=60
        )
        text = (result.stdout or result.stderr or "").strip()
        version, commit, dirty = parse_version_line(text)
        return {
            "version": version,
            "build_git_commit": commit or "unknown",
            "build_git_dirty": dirty,
            "build_commit_source": "binary --version" if commit else "unverified",
            "version_line": text,
        }
    commit = os.environ.get("ARCHMELD_LINUX_PRODUCT_COMMIT", "unknown")
    dirty = os.environ.get("ARCHMELD_LINUX_PRODUCT_DIRTY", "0") == "1"
    return {
        "version": None,
        "build_git_commit": commit,
        "build_git_dirty": dirty if commit != "unknown" else None,
        "build_commit_source": (
            "env:ARCHMELD_LINUX_PRODUCT_COMMIT"
            if commit != "unknown"
            else "unverified"
        ),
        "version_line": "",
    }


def gcc_version() -> str:
    candidates = []
    mingw = os.environ.get("ARCHMELD_MINGW_BIN") or os.environ.get(
        "ARCHMELD_STRAWBERRY_BIN"
    )
    if mingw:
        candidates.append(Path(mingw) / ("gcc.exe" if os.name == "nt" else "gcc"))
    found = shutil.which("gcc")
    if found:
        candidates.append(Path(found))
    for candidate in candidates:
        if not candidate.is_file():
            continue
        result = subprocess.run(
            [str(candidate), "--version"], capture_output=True, text=True
        )
        if result.returncode == 0 and result.stdout:
            return result.stdout.splitlines()[0]
    return "unknown"


PRODUCT_COMMIT = product_commit()
PACKAGING_DIRTY = packaging_dirty()
TOOLCHAIN = f"rust 1.94.1; node 24.14.1; gcc {gcc_version()}"


def helper_metadata(binary: Path, platform: str) -> dict:
    """Prove the packaged helper is the schema-1.2 producer, not just a hash."""
    info = {"path": binary.name, "sha256": sha256(binary)}
    if platform == "windows-x64":
        schema = subprocess.run(
            [str(binary), "--schema"], capture_output=True, text=True, timeout=60
        )
        version = subprocess.run(
            [str(binary), "--version"], capture_output=True, text=True, timeout=60
        )
        info["schema"] = (schema.stdout or "").strip()
        info["version"] = (version.stdout or version.stderr or "").strip()
        if schema.returncode != 0 or info["schema"] != "1.2":
            raise SystemExit(
                f"{platform} helper did not report schema 1.2: "
                f"exit={schema.returncode} stdout={info['schema']!r}"
            )
        _, commit, dirty = parse_version_line(info["version"])
        info["build_git_commit"] = commit or "unknown"
        info["build_git_dirty"] = dirty
        info["build_commit_source"] = "helper --version" if commit else "unverified"
    else:
        sidecar = binary.parent / "helper-schema.txt"
        if not sidecar.is_file():
            raise SystemExit(
                f"{platform} helper schema is unverified: {sidecar} is missing; "
                "run tooling/build_helper.py for this platform first"
            )
        info["schema"] = sidecar.read_text(encoding="utf-8").strip()
        if info["schema"] != "1.2":
            raise SystemExit(
                f"{platform} helper schema is {info['schema']!r}, expected 1.2"
            )
        provenance_path = binary.parent / "helper-provenance.json"
        if provenance_path.is_file():
            provenance = json.loads(provenance_path.read_text(encoding="utf-8"))
            info["version"] = provenance.get("version", "")
            info["build_git_commit"] = provenance.get("build_git_commit", "unknown")
            info["build_git_dirty"] = provenance.get("build_git_dirty")
            info["build_commit_source"] = "helper-provenance.json"
        else:
            commit = os.environ.get("ARCHMELD_LINUX_HELPER_COMMIT", "unknown")
            dirty = os.environ.get("ARCHMELD_LINUX_HELPER_DIRTY", "0") == "1"
            info["version"] = "not-executed-on-this-host"
            info["build_git_commit"] = commit
            info["build_git_dirty"] = dirty if commit != "unknown" else None
            info["build_commit_source"] = (
                "env:ARCHMELD_LINUX_HELPER_COMMIT"
                if commit != "unknown"
                else "unverified"
            )
    return info
PLATFORMS = {
    "windows-x64": {
        "binaries": {
            "archmeld": ROOT / "target/release/archmeld.exe",
            "archmeld-index": ROOT / "target/release/examples/archmeld_export.exe",
            "archmeld-serve": ROOT / "target/release/archmeld-serve.exe",
            "archmeld-mcp": ROOT / "target/release/archmeld-mcp.exe",
        },
        "installer": "install.ps1",
    },
    "linux-x64": {
        "binaries": {
            "archmeld": ROOT / "target-linux/release/archmeld",
            "archmeld-index": ROOT / "target-linux/release/examples/archmeld_export",
            "archmeld-serve": ROOT / "target-linux/release/archmeld-serve",
            "archmeld-mcp": ROOT / "target-linux/release/archmeld-mcp",
        },
        "installer": "install.sh",
    },
}


def sha256(path: Path) -> str:
    digest = hashlib.sha256()
    with path.open("rb") as handle:
        for chunk in iter(lambda: handle.read(1 << 20), b""):
            digest.update(chunk)
    return digest.hexdigest()


def cargo_components() -> list[dict]:
    lock = ROOT / "Cargo.lock"
    if tomllib is None or not lock.is_file():
        return []
    data = tomllib.loads(lock.read_text(encoding="utf-8"))
    components = []
    for package in data.get("package", []):
        name = package.get("name")
        version = package.get("version")
        if not name or not version:
            continue
        components.append({
            "type": "library",
            "name": name,
            "version": version,
            "purl": f"pkg:cargo/{name}@{version}",
            "scope": "required" if name.startswith(("archmeld", "codegraph")) else "optional",
        })
    components.sort(key=lambda item: (item["name"], item["version"]))
    return components


def sbom(extra_components: list[dict] | None = None, artifact_files: list[Path] | None = None) -> dict:
    components = cargo_components()
    for component in extra_components or []:
        components.append(component)
    for path in artifact_files or []:
        components.append({
            "type": "file",
            "name": path.name,
            "version": VERSION,
            "hashes": [{"alg": "SHA-256", "content": sha256(path)}],
        })
    return {
        "bomFormat": "CycloneDX",
        "specVersion": "1.5",
        "version": 1,
        "metadata": {
            "tool": {"vendor": "ARCHMELD", "name": "package_release.py", "version": "1"},
            "component": {
                "type": "application",
                "name": "archmeld",
                "version": VERSION,
                "purl": f"pkg:generic/archmeld@{VERSION}",
            },
            "properties": [
                {"name": "archmeld.upstream_commit", "value": UPSTREAM},
                {"name": "archmeld.toolchain", "value": TOOLCHAIN},
                {"name": "archmeld.packaging_git_commit", "value": PRODUCT_COMMIT},
                {"name": "archmeld.release_status", "value": "provisional"},
            ],
        },
        "components": components,
    }


def write_json(path: Path, value: dict) -> None:
    path.write_text(json.dumps(value, ensure_ascii=False, indent=2) + "\n", encoding="utf-8")


def zip_deterministic(target: Path, files: list[tuple[Path, str]]) -> None:
    with zipfile.ZipFile(target, "w", zipfile.ZIP_DEFLATED) as archive:
        for source, arcname in sorted(files, key=lambda item: item[1]):
            info = zipfile.ZipInfo(arcname, date_time=(2026, 1, 1, 0, 0, 0))
            info.compress_type = zipfile.ZIP_DEFLATED
            info.external_attr = 0o755 << 16 if os.access(source, os.X_OK) else 0o644 << 16
            archive.writestr(info, source.read_bytes())


INSTALL_PS1 = r"""param(
  [string]$Prefix = "$env:LOCALAPPDATA\ARCHMELD",
  [switch]$Rollback,
  [switch]$Uninstall
)
$ErrorActionPreference = "Stop"
$Here = Split-Path -Parent $MyInvocation.MyCommand.Path
function Hash([string]$Path) { (Get-FileHash -Algorithm SHA256 -Path $Path).Hash.ToLower() }
$Manifest = Get-Content (Join-Path $Here "bundle.json") -Raw | ConvertFrom-Json
$Installed = Join-Path $Prefix "install.json"
if ($Uninstall) {
  if (Test-Path $Installed) {
    $record = Get-Content $Installed -Raw | ConvertFrom-Json
    foreach ($name in $record.files) {
      $target = Join-Path $Prefix $name
      if (Test-Path $target) { Remove-Item $target -Force }
    }
    Remove-Item $Installed -Force
  }
  Write-Output "uninstalled; project data was not touched"
  exit 0
}
if ($Rollback) {
  $backup = Join-Path $Prefix "backup"
  if (!(Test-Path $backup)) { throw "no backup to roll back" }
  Copy-Item (Join-Path $backup "*") (Join-Path $Prefix "bin") -Force -Recurse
  Write-Output "rolled back from $backup"
  exit 0
}
$checks = @{}
foreach ($line in Get-Content (Join-Path $Here "checksums.sha256")) {
  if (!$line.Trim()) { continue }
  $parts = $line -split "\s+", 2
  $checks[$parts[1].TrimStart("*")] = $parts[0]
}
foreach ($entry in $checks.GetEnumerator()) {
  $path = Join-Path $Here $entry.Key
  if (!(Test-Path $path)) { throw "missing $($entry.Key)" }
  if ((Hash $path) -ne $entry.Value) { throw "checksum mismatch: $($entry.Key)" }
}
New-Item -ItemType Directory -Force -Path (Join-Path $Prefix "bin") | Out-Null
$backup = Join-Path $Prefix "backup"
New-Item -ItemType Directory -Force -Path $backup | Out-Null
foreach ($name in $Manifest.files) {
  $source = Join-Path $Here $name
  if (!(Test-Path $source)) { continue }
  $target = Join-Path $Prefix $name
  if (Test-Path $target) { Copy-Item $target (Join-Path $backup (Split-Path $target -Leaf)) -Force }
  Copy-Item $source $target -Force
}
$record = @{ version = $Manifest.version; files = @($Manifest.files | Where-Object { $_ -like "bin/*" }); source = $Here }
$record | ConvertTo-Json | Set-Content $Installed
Write-Output "installed $($Manifest.version) to $Prefix"
Write-Output "next: add '$Prefix\bin' to PATH, then run 'archmeld init' in the repository you want to index"
Write-Output "or: & '$Prefix\bin\archmeld.exe' init --workspace ."
"""

INSTALL_SH = r"""#!/bin/sh
set -eu
here=$(CDPATH= cd -- "$(dirname -- "$0")" && pwd)
prefix=${ARCHMELD_PREFIX:-$HOME/.local/archmeld}
action=${1:-install}
case "$action" in
  uninstall)
    for name in bin/archmeld bin/archmeld-index bin/archmeld-serve bin/archmeld-mcp; do
      [ -f "$prefix/$name" ] && rm -f "$prefix/$name"
    done
    [ -f "$prefix/install.json" ] && rm -f "$prefix/install.json"
    echo "uninstalled; project data was not touched"
    ;;
  rollback)
    [ -d "$prefix/backup" ] || { echo "no backup to roll back" >&2; exit 1; }
    cp -f "$prefix"/backup/* "$prefix"/bin/
    echo "rolled back from $prefix/backup"
    ;;
  install)
    (cd "$here" && sha256sum -c checksums.sha256)
    mkdir -p "$prefix/bin" "$prefix/backup"
    for name in bin/archmeld bin/archmeld-index bin/archmeld-serve bin/archmeld-mcp; do
      [ -f "$here/$name" ] || continue
      [ -f "$prefix/$name" ] && cp -f "$prefix/$name" "$prefix/backup/$(basename "$name")"
      cp -f "$here/$name" "$prefix/$name"
      chmod 755 "$prefix/$name"
    done
    printf '{"version":"VERSION","files":["bin/archmeld","bin/archmeld-index","bin/archmeld-serve","bin/archmeld-mcp"],"source":"%s"}
' "$here" > "$prefix/install.json"
    echo "installed VERSION to $prefix"
    echo "next: add '$prefix/bin' to PATH, then run 'archmeld init' in the repository you want to index"
    echo "or: '$prefix/bin/archmeld' init --workspace ."
    ;;
  *) echo "usage: $0 [install|uninstall|rollback]" >&2; exit 2 ;;
esac
"""

README = """# ARCHMELD CLI bundle

Local, offline binaries for one platform. No model provider is required for
read-only navigation, verification, reports or the MCP read surface.

- `archmeld` — offline CLI: `init`, `brief`, `status`, `review-diff`, `refresh`,
  `index`, `report`, `verify-report`, backups, scenarios and trace import.
- `archmeld-index` — one-shot in-memory CodeGraph bridge used by `archmeld index`.
- `archmeld-serve` — loopback HTTP API; prints one JSON control line.
- `archmeld-mcp` — read-only MCP stdio server.

Install with `./install.sh` (Linux) or `install.ps1` (Windows). The installer
verifies `checksums.sha256`, copies only binaries, backs up a previous install,
and never touches project/evidence data. `uninstall` removes installed binaries
only. `rollback` restores the previous binaries.

Next step after install (from the repository you want to index):

```bash
archmeld init
```

`init` indexes the workspace, registers its root, prints the exact
`claude mcp add` / `codex mcp add` command, previews the agent-rule block and
runs a real MCP handshake. Add `--write` to install the rule files and execute
the client command. The MCP server is read-only and never uploads source.

See `bundle.json`, `sbom.cdx.json`, `LICENSE`, `NOTICE` and
`THIRD_PARTY_NOTICES.md`. macOS is not a tested release platform.
"""


def build_cli(platform: str) -> tuple[Path, dict]:
    info = PLATFORMS[platform]
    for name, binary in info["binaries"].items():
        if not binary.is_file():
            raise SystemExit(f"missing {platform} binary: {binary}")
    staging = DIST / "build" / f"archmeld-cli-{VERSION}-{platform}"
    if staging.exists():
        shutil.rmtree(staging)
    (staging / "bin").mkdir(parents=True)
    for name, binary in info["binaries"].items():
        shutil.copy2(binary, staging / "bin" / f"{name}{binary.suffix}")
    for source, target in [
        (ROOT / "LICENSE", "LICENSE"),
        (ROOT / "NOTICE", "NOTICE"),
        (ROOT / "third_party" / "NOTICE.md", "THIRD_PARTY_NOTICES.md"),
    ]:
        if source.is_file():
            shutil.copy2(source, staging / target)
    (staging / "README.md").write_text(README, encoding="utf-8")
    if platform == "windows-x64":
        (staging / "install.ps1").write_text(INSTALL_PS1, encoding="utf-8")
    else:
        (staging / "install.sh").write_bytes(INSTALL_SH.replace("VERSION", VERSION).encode("utf-8"))
        os.chmod(staging / "install.sh", 0o755)
    helper = helper_metadata(info["binaries"]["archmeld-index"], platform)
    product = product_build_metadata(info["binaries"]["archmeld"], platform)
    if product.get("version") and product["version"] != VERSION:
        raise SystemExit(
            f"{platform} binary reports version {product['version']!r}, "
            f"but the bundle version is {VERSION!r}"
        )
    (staging / "VERSION").write_text(
        f"version={VERSION}\n"
        f"upstream={UPSTREAM}\n"
        f"toolchain={TOOLCHAIN}\n"
        f"platform={platform}\n"
        f"product_build_commit={product['build_git_commit']}\n"
        f"product_build_dirty={product['build_git_dirty']}\n"
        f"packaging_git_commit={PRODUCT_COMMIT}\n"
        f"release_status=provisional\n",
        encoding="utf-8",
    )
    files = sorted(
        str(path.relative_to(staging)).replace("\\", "/")
        for path in staging.rglob("*")
        if path.is_file() and path.name not in {"checksums.sha256", "bundle.json", "sbom.cdx.json"}
    )
    commit_consistent = (
        product["build_git_commit"] == helper["build_git_commit"]
        and product["build_git_commit"] == PRODUCT_COMMIT
    )
    if commit_consistent:
        consistency_reason = "all build commits equal the packaging commit"
    else:
        consistency_reason = (
            f"product={product['build_git_commit']} helper={helper['build_git_commit']} "
            f"packaging={PRODUCT_COMMIT}"
        )
    bundle = {
        "schema_version": "1.1",
        "name": "archmeld-cli",
        "version": VERSION,
        "platform": platform,
        "upstream_commit": UPSTREAM,
        "product_git_commit": product["build_git_commit"],
        "product_build_commit": product["build_git_commit"],
        "product_build_dirty": product["build_git_dirty"],
        "product_build_commit_source": product["build_commit_source"],
        "packaging_git_commit": PRODUCT_COMMIT,
        "packaging_git_dirty": PACKAGING_DIRTY,
        "commit_consistency": commit_consistent,
        "commit_consistency_reason": consistency_reason,
        "release_status": "provisional",
        "toolchain": TOOLCHAIN,
        "helper": helper,
        "model_required": False,
        "offline_assets": True,
        "installer": info["installer"],
        "files": files,
        "binaries": [
            f"bin/{name}{binary.suffix}" for name, binary in info["binaries"].items()
        ],
    }
    write_json(staging / "bundle.json", bundle)
    sbom_components = [
        {"type": "file", "name": path.name, "version": VERSION,
         "hashes": [{"alg": "SHA-256", "content": sha256(path)}]}
        for path in sorted((staging / "bin").iterdir())
    ]
    write_json(staging / "sbom.cdx.json", sbom(sbom_components))
    checksum_files = sorted(
        path for path in staging.rglob("*")
        if path.is_file() and path.name != "checksums.sha256"
    )
    (staging / "checksums.sha256").write_text(
        "".join(f"{sha256(path)}  {path.relative_to(staging).as_posix()}\n" for path in checksum_files),
        encoding="utf-8",
    )
    target = DIST / f"archmeld-cli-{VERSION}-{platform}.zip"
    zip_deterministic(
        target,
        [(path, path.relative_to(staging).as_posix()) for path in staging.rglob("*") if path.is_file()],
    )
    return target, bundle


def build_vsix() -> Path:
    staging = DIST / "build" / "vsix"
    if staging.exists():
        shutil.rmtree(staging)
    extension = staging / "extension"
    shutil.copytree(ROOT / "archmeld-vscode", extension, ignore=shutil.ignore_patterns("test", "test-ui", "*.vsix"))
    package_path = extension / "package.json"
    package = json.loads(package_path.read_text(encoding="utf-8"))
    package["version"] = VERSION
    package["publisher"] = "archmeld"
    write_json(package_path, package)
    for source, target in [
        (ROOT / "LICENSE", extension / "LICENSE"),
        (ROOT / "NOTICE", extension / "NOTICE"),
        (ROOT / "third_party" / "NOTICE.md", extension / "THIRD_PARTY_NOTICES.md"),
    ]:
        if source.is_file():
            shutil.copy2(source, target)
    (staging / "extension.vsixmanifest").write_text(
        f"""<?xml version="1.0" encoding="utf-8"?>
<PackageManifest Version="2.0.0" xmlns="http://schemas.microsoft.com/developer/vsx-schema/2011">
  <Metadata>
    <Identity Language="en-US" Id="archmeld-workbench" Version="{VERSION}" Publisher="archmeld" />
    <DisplayName>ARCHMELD Workbench</DisplayName>
    <Description xml:space="preserve">Evidence-driven code and graph navigation for ARCHMELD.</Description>
    <Categories>Other</Categories>
    <GalleryFlags>Public</GalleryFlags>
  </Metadata>
  <Installation>
    <InstallationTarget Id="Microsoft.VisualStudio.Code" />
  </Installation>
  <Dependencies />
  <Assets>
    <Asset Type="Microsoft.VisualStudio.Code.Manifest" Path="extension/package.json" Addressable="true" />
    <Asset Type="Microsoft.VisualStudio.Services.Content.Details" Path="extension/README.md" Addressable="true" />
  </Assets>
</PackageManifest>
""",
        encoding="utf-8",
    )
    (staging / "[Content_Types].xml").write_text(
        """<?xml version="1.0" encoding="utf-8"?>
<Types xmlns="http://schemas.openxmlformats.org/package/2006/content-types">
  <Default Extension="vsixmanifest" ContentType="text/xml" />
  <Default Extension="json" ContentType="application/json" />
  <Default Extension="js" ContentType="application/javascript" />
  <Default Extension="md" ContentType="text/markdown" />
  <Default Extension="txt" ContentType="text/plain" />
  <Default Extension="xml" ContentType="text/xml" />
</Types>
""",
        encoding="utf-8",
    )
    target = DIST / f"archmeld-workbench-{VERSION}.vsix"
    zip_deterministic(
        target,
        [(path, path.relative_to(staging).as_posix()) for path in staging.rglob("*") if path.is_file()],
    )
    return target


def main() -> int:
    DIST.mkdir(parents=True, exist_ok=True)
    windows, windows_bundle = build_cli("windows-x64")
    linux, linux_bundle = build_cli("linux-x64")
    vsix = build_vsix()
    artifacts = [windows, linux, vsix]
    hashes = {path.name: sha256(path) for path in artifacts}
    (DIST / "SHA256SUMS").write_text(
        "".join(f"{digest}  {name}\n" for name, digest in sorted(hashes.items())),
        encoding="utf-8",
    )
    write_json(
        DIST / "release-manifest.json",
        {
            "schema_version": "1.1",
            "product": "ARCHMELD",
            "version": VERSION,
            "upstream_commit": UPSTREAM,
            "product_git_commit": PRODUCT_COMMIT,
            "product_git_commit_meaning": "packaging commit; use builds[*].product_build_commit for binary identity",
            "packaging_git_commit": PRODUCT_COMMIT,
            "packaging_git_dirty": PACKAGING_DIRTY,
            "release_status": "provisional",
            "toolchain": TOOLCHAIN,
            "builds": {
                "windows-x64": {
                    "product_build_commit": windows_bundle["product_build_commit"],
                    "product_build_dirty": windows_bundle["product_build_dirty"],
                    "helper_build_commit": windows_bundle["helper"]["build_git_commit"],
                    "helper_build_dirty": windows_bundle["helper"]["build_git_dirty"],
                    "commit_consistency": windows_bundle["commit_consistency"],
                    "commit_consistency_reason": windows_bundle["commit_consistency_reason"],
                },
                "linux-x64": {
                    "product_build_commit": linux_bundle["product_build_commit"],
                    "product_build_dirty": linux_bundle["product_build_dirty"],
                    "helper_build_commit": linux_bundle["helper"]["build_git_commit"],
                    "helper_build_dirty": linux_bundle["helper"]["build_git_dirty"],
                    "commit_consistency": linux_bundle["commit_consistency"],
                    "commit_consistency_reason": linux_bundle["commit_consistency_reason"],
                },
            },
            "helpers": {
                "windows-x64": windows_bundle["helper"],
                "linux-x64": linux_bundle["helper"],
            },
            "artifacts": [
                {
                    "name": path.name,
                    "sha256": hashes[path.name],
                    "size": path.stat().st_size,
                }
                for path in artifacts
            ],
            "platforms": {
                "windows-x64": "built and smoke-tested",
                "linux-x64": "built in rust:1.94.1-bookworm; runtime smoke in Ubuntu 24.04",
                "macos": "not built and not supported in R1",
            },
            "model_required": False,
            "offline_assets": True,
        },
    )
    write_json(DIST / "sbom.cdx.json", sbom(artifact_files=artifacts))
    print(json.dumps({"artifacts": hashes, "version": VERSION}, indent=2))
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
