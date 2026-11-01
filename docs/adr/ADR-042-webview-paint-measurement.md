# ADR-042: Dedicated webview paint and frame measurement

Status: accepted for the project testing phase.

## Context

B026 previously measured the VS Code extension host command latency, panel
readiness and a page-scoped CDP `TaskDuration`. Those numbers describe the
workbench page and the extension host, not the graph webview document. A VS Code
webview is an out-of-process `vscode-webview://` target whose extension HTML is
nested one frame deeper; its paint timeline is therefore invisible to page-level
metrics. B026 asks for a webview-specific paint/frame measurement with budgets.

## Decision

1. The graph webview HTML carries a CSP-nonce-scoped script that reports its own
   `paint` entries, LCP, navigation timings and render completion
   (`cytoscape`/`svg`) through the normal VS Code message channel. A probe
   message forces style/layout invalidation and reports the double
   `requestAnimationFrame` interval from inside the same document.
2. Diagnostics are gated: `ARCHMELD_PAINT_PROBE=1` registers
   `archmeld.diagnostics.paintReport` and `archmeld.diagnostics.paintProbe`.
   Without the variable the commands do not exist and the product surface does
   not read paint observations.
3. The extension-host harness attaches CDP to the `vscode-webview://` iframe
   target, walks nested same-origin frames to the graph document, and reads the
   same performance timeline independently. It also records a workbench page
   screencast while only the webview document is invalidated; every emitted
   frame is a compositor presentation observation for that update.
4. Budgets are applied after the raw samples are written: webview
   first-contentful-paint <= 5000 ms, probe round trip <= 2000 ms, double-rAF
   frame <= 1000 ms, and first compositor frame after the trigger <= 500 ms.
   Missing measurements are failures, not silent passes.
5. The evidence is explicit about scope: GPU execution time, shader cost and
   display scan-out are not measured; screencast intervals include
   capture/encode throttling and are recorded without a gate; the fixture graph
   is small (two nodes, one edge), so this is a latency measurement, not a
   scale benchmark.

## Evidence

- Three real extension-host runs on Windows 10 with VS Code
  `--remote-debugging-port` and a fresh profile.
- Webview document FCP: 400 / 512 / 512 ms (min/median/max); LCP identical.
- Double-rAF after a forced invalidation: 5.5 / 5.6 / 5.6 ms.
- Probe round trip: 8.7-13.4 ms across nine probes.
- First compositor frame after the trigger: 18.3 / 21.7 / 22.0 ms; three to
  seven frames observed per run.
- `archmeld.openGraph` returned in 132-154 ms and the panel was ready in
  337-362 ms.
- Evidence: `docs/release-evidence/B026-ui-webview-paint.json`, raw results
  `docs/evidence/ui-result-b026-paint-1.json`,
  `docs/evidence/ui-result-b026-paint-2.json`,
  `docs/evidence/ui-result-b026-paint-3.json`, screenshots
  `docs/evidence/b026-workbench-paint-1.png`,
  `docs/evidence/b026-workbench-paint-2.png`,
  `docs/evidence/b026-workbench-paint-3.png`, commands
  `#b026-ui-paint-1`, `#b026-ui-paint-2`, `#b026-ui-paint-3`.

## Consequences

- Paint and frame latency are attributed to the webview document instead of the
  workbench page.
- The webview now carries a small measurement script. It is inert without the
  environment gate at the extension side, but it still runs in the document;
  its cost is limited to two animation frames per load plus one per probe.
- Cytoscape renders to canvas, so DOM entity counts are zero in the CDP probe;
  the render report (two nodes, one edge) and the `#cy` container identify the
  document unambiguously.
- A true GPU execution profile remains out of scope for this test phase.
