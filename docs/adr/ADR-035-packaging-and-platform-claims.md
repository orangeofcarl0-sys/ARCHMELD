# ADR-035 · Offline packaging and platform claims

Status: accepted for the B031 packaging slice; R1 naming remains blocked by
AT067.

## Decision

- Release binaries are built from the pinned workspace and Rust 1.94.1. Windows
  uses the GNU toolchain; Linux is built in the pinned
  `rust:1.94.1-bookworm` image. Binaries are not built on a moving `latest`
  toolchain.
- Each CLI bundle contains only platform binaries, LICENSE, NOTICE,
  THIRD_PARTY_NOTICES, VERSION, README, bundle.json, SBOM and checksums, plus an
  installer. Installers verify every checksum before copying, back up an
  existing binary set, and write an install record. Uninstall removes only
  installed binaries/records; project and evidence data are never touched.
  Rollback restores the previous binary set.
- The VSIX is assembled offline from the extension source and local Cytoscape
  asset. It is installable via the VS Code CLI; no publisher identity from the
  upstream project is used.
- `dist/sbom.cdx.json` is CycloneDX 1.5 with Cargo.lock components and artifact
  hashes. `dist/SHA256SUMS` and `dist/release-manifest.json` bind exact
  versions, upstream commit, toolchain and artifact hashes.
- Platform claims are explicit: Windows 11 x64 and Ubuntu 24.04 x64 are the only
  tested targets. macOS is not built, not tested and not supported in R1.
  Linux VS Code host/UI was not tested. Clean-machine indexing and opening an
  offline report directly from the packaged bundle were not run.
- The upgrade drill uses a locally built previous version (0.0.9) because no
  published historical release exists. This is recorded as a drill, not a
  shipped upgrade path.
- Upstream compatibility uses a synthetic interface-rename drill in a temporary
  copy: the adapter contract test target detects the change. The pinned commit,
  NOTICE and toolchain lock are preserved; no runtime auto-follow of upstream
  main is introduced. A real newer-upstream checkout was not available, so
  AT077 remains partial.

## Residual limits

- R1 is not declared complete: AT067 (seven-language runtime linkage and medium
  real projects) remains failed.
- Packaging evidence is local; artifacts are attached to a release only after
  the remaining release gates pass.
- Linux GUI/editor integration, clean-machine indexing and packaged offline
  report opening remain untested.
