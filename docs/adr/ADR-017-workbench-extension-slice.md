# ADR-017 · Workbench extension slice (B007)

Status: Accepted for the B007 slice; does not replace ADR-008 for R1.

## Update (2026-10-31)

Cytoscape.js 3.30.2 is now vendored at `archmeld-vscode/media/cytoscape.min.js`
(MIT, license included) and used with a preset layout; the SVG renderer remains
as a no-asset fallback. Cursor→entity now requires an anchor containing the
cursor line, and an editor-level test runs inside a real VS Code extension host.
Screenshot evidence is captured through the Chrome DevTools Protocol
(`Page.captureScreenshot` against the extension-host workbench target), which is
independent of desktop window occlusion. The first-run Welcome modal is dismissed
through the same CDP session before capture.

## Context

ADR-008 says the workbench reuses the existing upstream VS Code extension with
minimal edits, plus Cytoscape.js. The pinned upstream extension cannot be built
as-is in this workspace: its `package.json` dev-dependencies point at
`file:../vsforge/packages/*`, which is not part of the pinned checkout, and its
activation path is coupled to engine download, telemetry and a large tool
surface that ARCHMELD must remove or replace. B007 needs a real, testable
code↔graph↔evidence loop before those removals are designed.

## Decision

For this slice, add `archmeld-vscode/` as a small extension that consumes the
same local HTTP API as every other client:

- commands: open graph for the symbol at the cursor, open historical source,
  back;
- one webview panel with file / responsibility / runtime projections, an
  always-visible legend, deterministic identity-anchored positions, evidence
  inspection per edge, and unknown/candidate styling that is never promoted to
  a resolved edge;
- historical source is opened through a read-only `archmeld-snapshot:` virtual
  document, so an old snapshot never silently resolves to the current file.

The webview renders SVG directly instead of Cytoscape.js for this slice. The
interaction semantics (stable layout, view projection, edge certainty, back
stack, selection loop guard) are covered by unit tests today; a renderer swap
must not change those semantics.

## Consequences

- No dependency on the missing `vsforge` packages and no telemetry/download
  code is inherited.
- AT017/AT018 are only partially evidenced: no editor-level UI automation or
  screenshots yet, and Cytoscape.js is not used.
- The upstream extension remains in the tree untouched; B007 completion still
  requires either porting these behaviors into it or an explicit follow-up ADR.

## Revisit conditions

Revisit before B007 is marked complete, and before R1 packaging: run
editor-level automation (e.g. `@vscode/test-electron`), capture screenshots,
and decide Cytoscape vs the current SVG renderer against S08 §3.
