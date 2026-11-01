// Copyright 2026 ARCHMELD contributors
// SPDX-License-Identifier: Apache-2.0

'use strict';

const crypto = require('node:crypto');

function esc(value) {
  return String(value ?? '')
    .replaceAll('&', '&amp;')
    .replaceAll('<', '&lt;')
    .replaceAll('>', '&gt;')
    .replaceAll('"', '&quot;')
    .replaceAll("'", '&#39;');
}

function safeJson(value) {
  return JSON.stringify(value)
    .replaceAll('<', '\\u003c')
    .replaceAll('>', '\\u003e')
    .replaceAll('&', '\\u0026')
    .replaceAll(' ', '\\u2028')
    .replaceAll(' ', '\\u2029');
}

function targetTemplate(base) {
  const target = JSON.parse(JSON.stringify(base));
  target.id = `${base.id}.to_be`;
  target.layer = 'to_be';
  target.status = 'candidate';
  target.revision = 1;
  target.dependencies = (target.dependencies ?? []).filter(
    (dependency) => dependency.status !== 'observed',
  );
  return target;
}

/**
 * To-Be graph editor. The textarea edits a candidate model; it never writes the
 * ledger or source tree. "Preview diff" sends the candidate to the read-only
 * diff route, which validates snapshot/layer/status and returns typed intents.
 */
function renderTargetEditorHtml({ base, cspSource = '', nonce }) {
  const scriptNonce = nonce || crypto.randomBytes(16).toString('hex');
  const template = targetTemplate(base);
  return `<!doctype html>
<html lang="en">
<head>
<meta charset="utf-8">
<meta http-equiv="Content-Security-Policy" content="default-src 'none'; style-src ${esc(cspSource || "'unsafe-inline'")} 'unsafe-inline'; script-src 'nonce-${esc(scriptNonce)}';">
<title>ARCHMELD To-Be editor</title>
<style>
  body { font-family: var(--vscode-font-family); color: var(--vscode-foreground); padding: 16px; line-height: 1.45; }
  h1 { font-size: 18px; margin: 0 0 4px; }
  h2 { font-size: 14px; margin: 16px 0 8px; text-transform: uppercase; letter-spacing: .04em; }
  .muted { color: var(--vscode-descriptionForeground); font-size: 12px; }
  textarea { width: 100%; min-height: 320px; font-family: var(--vscode-editor-font-family); background: var(--vscode-input-background); color: var(--vscode-input-foreground); border: 1px solid var(--vscode-input-border, transparent); padding: 8px; }
  .actions { margin: 12px 0; display: flex; gap: 8px; }
  button { color: var(--vscode-button-foreground); background: var(--vscode-button-background); border: 1px solid var(--vscode-button-border, transparent); padding: 6px 12px; cursor: pointer; }
  pre { font-family: var(--vscode-editor-font-family); background: var(--vscode-textCodeBlock-background); padding: 8px; overflow-x: auto; white-space: pre-wrap; }
  .warn { color: var(--vscode-editorWarning-foreground); }
</style>
</head>
<body>
<h1>To-Be editor · ${esc(base.id)}</h1>
<p class="muted">Base snapshot <code>${esc(base.snapshot_id)}</code>. The target must keep this snapshot, use <code>layer=to_be</code> and <code>status=candidate</code>. Editing never writes source or the ledger; the diff route only validates and returns candidate intents.</p>
<h2>Candidate model</h2>
<textarea id="target" spellcheck="false">${esc(JSON.stringify(template, null, 2))}</textarea>
<div class="actions">
  <button id="preview">Preview diff and intents</button>
  <button id="reset">Reset from base</button>
</div>
<p id="status" class="muted">Not previewed.</p>
<h2>Diff result</h2>
<pre id="result">No preview yet.</pre>
<script nonce="${esc(scriptNonce)}">
  (function () {
    const vscode = acquireVsCodeApi();
    const textarea = document.getElementById('target');
    const status = document.getElementById('status');
    const result = document.getElementById('result');
    const baseTemplate = ${safeJson(template)};
    document.getElementById('preview').addEventListener('click', () => {
      let target;
      try {
        target = JSON.parse(textarea.value);
      } catch (error) {
        status.textContent = 'JSON error: ' + error.message;
        return;
      }
      status.textContent = 'Previewing…';
      result.textContent = '';
      vscode.postMessage({ type: 'target.preview', target });
    });
    document.getElementById('reset').addEventListener('click', () => {
      textarea.value = JSON.stringify(baseTemplate, null, 2);
      status.textContent = 'Reset from base.';
      result.textContent = 'No preview yet.';
    });
    window.addEventListener('message', (event) => {
      const message = event.data;
      if (!message || message.type !== 'target.result') return;
      if (message.error) {
        status.textContent = 'Preview failed: ' + message.error;
        result.textContent = '';
        return;
      }
      status.textContent = 'Preview only; no source or ledger write occurred.';
      result.textContent = JSON.stringify(message.diff, null, 2);
    });
  }());
</script>
</body>
</html>`;
}

module.exports = { renderTargetEditorHtml, targetTemplate };
