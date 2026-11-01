# ARCHMELD Workbench extension (B007 slice)

A small VS Code extension that turns the local ARCHMELD API into a
code↔graph↔evidence workbench. See `docs/adr/ADR-017-workbench-extension-slice.md`
for why this is a separate, dependency-free extension rather than an edit of the
pinned upstream extension.

## Run

1. Start the core API (created by the Rust workspace):

   ```sh
   target/debug/archmeld-serve.exe --state <ledger-dir> --port 8765
   ```

   It prints one JSON line with `endpoint` and `token`.

2. Point VS Code at it:

   ```sh
   code --extensionDevelopmentPath=F:\Codex_Work_Space\ARCHMELD-work\archmeld-vscode
   ```

   Settings (or environment variables, which win):

   - `archmeld.endpoint` / `ARCHMELD_ENDPOINT`
   - `archmeld.token` / `ARCHMELD_TOKEN`
   - `archmeld.projectId` / `ARCHMELD_PROJECT`
   - `archmeld.snapshotId` / `ARCHMELD_SNAPSHOT`

3. Put the cursor on an indexed symbol and run `ARCHMELD: Open Graph for Symbol`.
   Clicking a node opens the snapshot's historical source in a read-only
   virtual document; clicking an edge shows its evidence, basis and resolution.

## What the UI guarantees

- Node positions are derived from entity identity, so expanding neighbors never
  reshuffles the existing graph.
- Resolved, candidate and unknown edges use solid/dashed/dotted strokes plus
  text labels (colour is never the only encoding).
- Missing evidence renders as `unknown`; a candidate is never labelled resolved.
- File / responsibility / runtime are projections of one model, not a generated
  parent/child tree. An empty projection falls back to the local graph with an
  explicit warning instead of pretending there is no data.
- Historical source is served from the snapshot blob, marked as historical.

## Trace source jump

`archmeld.openTraceFrame` takes `{runId, spanId?}` (or a bare run id). It reads
the runtime timeline, lists only zones that retain `tracy.src_file` and
`tracy.src_line`, reads the exact line window from the snapshot CAS through
`/v1/source/file`, and opens it in the read-only `archmeld-snapshot-file`
document with the cursor on the zone line. A run without located zones is an
explicit error, never a guessed symbol. The extension-host harness exercises
this path when `ARCHMELD_UI_TRACE_RUN` is set; see ADR-045 and
`docs/release-evidence/B016-trace-source-jump.json`.

## Tests

```sh
node --test test/*.test.js
```

Covers precise cursor→anchor selection, layout stability, view projection,
relation certainty, selection loop guard and back navigation, HTML
escaping/offline CSP, Cytoscape element/style generation, and a real spawned
`archmeld-serve` for health, token 403 and structured 404/400 errors.

Editor-level test (real VS Code extension host):

```sh
# prepare docs/evidence/ui-state with the fixture, then:
ARCHMELD_UI_STATE=... ARCHMELD_UI_PROJECT=... ARCHMELD_UI_SNAPSHOT=... ARCHMELD_UI_RESULT=docs/evidence/ui-result.json ARCHMELD_UI_SCREENSHOT=docs/evidence/workbench.png ARCHMELD_UI_CDP_PORT=9333 ARCHMELD_SERVE=target/debug/archmeld-serve.exe code --extensionDevelopmentPath=. --extensionTestsPath=./test-ui/runTests.js --disable-extensions --skip-welcome --skip-release-notes --remote-debugging-port=9333 --user-data-dir=<temp> --wait test-ui/fixture
```

The test asserts the `ARCHMELD Graph` tab opens, dismisses the first-run
Welcome/onboarding modal through CDP, and captures the rendered workbench with
`Page.captureScreenshot` (see ADR-017). The result also records host timings
(`timings.open_graph_command_ms`, `timings.panel_ready_ms`), the declared
budgets and any budget failures; raw measurements are written before a budget
failure is raised. Pass `--skip-welcome --skip-release-notes` and keep the
harness's onboarding dismissal so screenshots show the graph panel.

The harness sets `ARCHMELD_PAINT_PROBE=1` before activating the extension. That
gate registers `archmeld.diagnostics.paintReport` and
`archmeld.diagnostics.paintProbe`, which expose the graph webview's own paint
timeline, LCP, render completion and forced double-rAF interval. The harness
also attaches CDP to the `vscode-webview://` iframe target, reads the nested
graph document's paint timeline directly, and records a workbench screencast
while only that document is invalidated. Budgets (FCP, probe round trip,
double-rAF, first compositor frame after the trigger) are applied after the raw
samples are written. GPU execution time, shader cost and scan-out are not
measured; screencast intervals include capture throttling and are not gated.
See ADR-042 and `docs/release-evidence/B026-ui-webview-paint.json`.
