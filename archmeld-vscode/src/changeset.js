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

function preview(entry) {
  if (!entry) {
    return '<span class="muted">absent</span>';
  }
  const text = entry.text ?? '';
  const marker = entry.truncated ? ' <span class="muted">(truncated)</span>' : '';
  return `<div class="digest"><code>${esc(entry.digest)}</code> · ${esc(entry.size)} B${marker}</div>` +
    `<pre>${esc(text)}</pre>`;
}

/**
 * ChangeSet review page. It shows the byte-level diff, the exact prepared
 * digest and the approval binding. Native actions are only offered for the
 * states where the core can legally accept them; the extension asks for
 * explicit confirmation before apply/recover.
 */
function renderChangesetHtml({ changeset, diff, verification = null, cspSource = '', nonce }) {
  const scriptNonce = nonce || crypto.randomBytes(16).toString('hex');
  const state = changeset?.effective_state ?? changeset?.state ?? 'unknown';
  const preparedDigest = changeset?.prepared_digest ?? null;
  const approval = changeset?.approval ?? null;
  const operations = (diff?.operations ?? []).map(
    (operation) =>
      `<section class="op">` +
      `<h3>${esc(operation.op)} <code>${esc(operation.path)}</code>` +
      (operation.destination ? ` → <code>${esc(operation.destination)}</code>` : '') +
      (operation.case_only ? ' <span class="badge warn">case-only</span>' : '') +
      `</h3>` +
      `<div class="before"><h4>before</h4>${preview(operation.before)}</div>` +
      `<div class="after"><h4>after</h4>${preview(operation.after)}</div>` +
      `</section>`,
  );
  const verdict = verification?.verdict ?? null;
  const actions = [];
  if (changeset?.state === 'prepared' && changeset?.effective_state === 'prepared' && !approval) {
    actions.push(
      '<button data-action="approve" class="primary">Approve this exact prepared change</button>',
    );
  }
  if (changeset?.state === 'prepared' && changeset?.effective_state === 'prepared' && approval) {
    actions.push('<button data-action="apply" class="danger">Apply this exact change</button>');
  }
  if (['applying', 'failed', 'conflicted', 'applied'].includes(changeset?.state)) {
    actions.push('<button data-action="recover" class="danger">Recover owned writes</button>');
  }
  if (changeset?.state === 'applied' && !verification) {
    actions.push('<button data-action="verify" class="primary">Verify the applied snapshot</button>');
  }
  if (
    changeset?.state === 'verifying' &&
    verification &&
    verification.state === 'accepted' &&
    verdict?.acceptance === true
  ) {
    actions.push('<button data-action="accept" class="primary">Accept this verified change</button>');
  }
  const stale = changeset?.effective_state === 'stale';
  const checks = (verification?.checks ?? [])
    .map(
      (check) =>
        `<li><code>${esc(check.check_id)}</code> · ${esc(check.state)}` +
        (check.required ? ' · <span class="badge">required</span>' : '') +
        `</li>`,
    )
    .join('');
  const verificationHtml = verification
    ? `<h2>Independent verification</h2>
<div class="badges">
  <span class="${verdict?.acceptance ? '' : 'warn'}">verification: ${esc(verification.state ?? '?')}</span>
  <span class="${verdict?.acceptance ? '' : 'warn'}">acceptance: ${esc(verdict?.acceptance ?? false)}</span>
  <span>snapshot: <code>${esc(verification.snapshot_id ?? '?')}</code></span>
</div>
${(verdict?.reasons ?? []).length ? `<ul>${verdict.reasons.map((reason) => `<li>${esc(reason)}</li>`).join('')}</ul>` : ''}
${checks ? `<ul class="checks">${checks}</ul>` : '<p class="muted">No checks recorded.</p>'}
<p class="note">Verification ran against the resulting snapshot. Acceptance is a separate action and rechecks for drift.</p>`
    : '<h2>Independent verification</h2><p class="muted">No verification has been run for this ChangeSet.</p>';
  return `<!doctype html>
<html lang="en">
<head>
<meta charset="utf-8">
<meta http-equiv="Content-Security-Policy" content="default-src 'none'; style-src ${esc(cspSource || "'unsafe-inline'")} 'unsafe-inline'; script-src 'nonce-${esc(scriptNonce)}';">
<title>ARCHMELD ChangeSet review</title>
<style>
  body { font-family: var(--vscode-font-family); color: var(--vscode-foreground); padding: 16px; line-height: 1.45; }
  h1 { font-size: 18px; margin: 0 0 4px; }
  h2 { font-size: 14px; margin: 20px 0 8px; text-transform: uppercase; letter-spacing: .04em; }
  h3 { font-size: 13px; margin: 12px 0 4px; }
  h4 { font-size: 11px; margin: 4px 0; color: var(--vscode-descriptionForeground); text-transform: uppercase; }
  .badges span, .badge { display: inline-block; border: 1px solid var(--vscode-panel-border); border-radius: 999px; padding: 1px 8px; margin-right: 6px; font-size: 12px; }
  .warn { border-color: var(--vscode-editorWarning-foreground); }
  .danger { border-color: var(--vscode-errorForeground); }
  .muted, .digest { color: var(--vscode-descriptionForeground); font-size: 12px; }
  .op { border-top: 1px solid var(--vscode-panel-border); padding-top: 8px; }
  .before, .after { margin: 6px 0; }
  pre { font-family: var(--vscode-editor-font-family); background: var(--vscode-textCodeBlock-background); padding: 8px; overflow-x: auto; white-space: pre-wrap; }
  code { font-family: var(--vscode-editor-font-family); }
  .actions { margin: 16px 0; display: flex; gap: 8px; flex-wrap: wrap; }
  button { color: var(--vscode-button-foreground); background: var(--vscode-button-background); border: 1px solid var(--vscode-button-border, transparent); padding: 6px 12px; cursor: pointer; }
  button.danger { background: var(--vscode-inputValidation-errorBackground, #7f1d1d); }
  .note { font-size: 12px; color: var(--vscode-descriptionForeground); }
</style>
</head>
<body>
<h1>${esc(changeset?.id ?? 'ChangeSet')}</h1>
<div class="badges">
  <span class="${stale ? 'warn' : ''}">state: ${esc(state)}</span>
  <span>revision: ${esc(changeset?.revision ?? '?')}</span>
  <span>plan: ${esc(changeset?.plan_id ?? '?')}</span>
  <span>step: ${esc(changeset?.step_id ?? '?')}</span>
  <span class="${approval ? '' : 'warn'}">approval: ${esc(approval ? approval.approval_id : 'none')}</span>
</div>
<p class="note">The source repository has not been modified by preparation. The prepared digest binds exactly this diff; changing staging invalidates the approval.</p>
<h2>Prepared digest</h2>
<p><code>${esc(preparedDigest ?? 'not prepared')}</code></p>
<h2>Allowed paths</h2>
<p>${(changeset?.allowed_paths ?? []).map((path) => `<code>${esc(path)}</code>`).join(' · ') || '<span class="muted">none</span>'}</p>
<h2>Read set</h2>
<p>${(changeset?.read_set ?? []).map((path) => `<code>${esc(path)}</code>`).join(' · ') || '<span class="muted">none</span>'}</p>
<h2>Operations (${operations.length})</h2>
${operations.join('') || '<p class="muted">No file changes in this prepared revision.</p>'}
${verificationHtml}
<h2>Actions</h2>
<div class="actions">${actions.join('') || '<span class="muted">No action is legal in this state.</span>'}</div>
<p class="note">Apply and recover require explicit confirmation. Approvals are server-recorded; this panel is not a signature.</p>
<script nonce="${esc(scriptNonce)}">
  (function () {
    const vscode = acquireVsCodeApi();
    document.addEventListener('click', (event) => {
      const action = event.target.closest('[data-action]');
      if (!action) return;
      vscode.postMessage({
        type: 'changeset.' + action.getAttribute('data-action'),
        changesetId: ${JSON.stringify(changeset?.id ?? '')},
        revision: ${JSON.stringify(changeset?.revision ?? null)},
        preparedDigest: ${JSON.stringify(preparedDigest)},
        approvalId: ${JSON.stringify(approval?.approval_id ?? null)},
        verificationId: ${JSON.stringify(verification?.verification_id ?? null)},
      });
    });
  }());
</script>
</body>
</html>`;
}

module.exports = { renderChangesetHtml };
