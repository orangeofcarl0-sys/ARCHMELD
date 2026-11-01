// Copyright 2026 ARCHMELD contributors
// SPDX-License-Identifier: Apache-2.0

'use strict';

function esc(value) {
  return String(value ?? '')
    .replaceAll('&', '&amp;')
    .replaceAll('<', '&lt;')
    .replaceAll('>', '&gt;')
    .replaceAll('"', '&quot;')
    .replaceAll("'", '&#39;');
}

function renderList(items, empty) {
  if (!items || items.length === 0) {
    return `<p class="empty">${esc(empty)}</p>`;
  }
  return `<ul>${items.map((item) => `<li>${item}</li>`).join('')}</ul>`;
}

/**
 * Static candidate-review page. It is intentionally read-only: it shows what is
 * candidate, what is unverified and what was ignored, and it never offers to
 * turn a candidate into an evidence-ledger fact.
 */
function renderReviewHtml({ architecture, review, checks = null, cspSource }) {
  const status = architecture?.status ?? 'unknown';
  const state = review?.state ?? 'unknown';
  const components = (architecture?.components ?? []).map(
    (component) =>
      `<li><strong>${esc(component.name)}</strong> <code>${esc(component.id)}</code>` +
      `<div class="muted">${esc(component.purpose)}</div>` +
      `<div class="muted">members: ${(component.member_entity_ids ?? []).length}</div></li>`,
  );
  const unverified = (review?.source_references?.details ?? [])
    .filter((item) => item.status === 'unverified')
    .map(
      (item) =>
        `<li><code>${esc(item.path)}</code>:${esc(item.line_start)}-${esc(item.line_end)} ` +
        `<span class="muted">${esc(item.qualified_name)}</span> — ${esc(item.reason ?? 'unverified source range')}</li>`,
    );
  const missingMembers = (review?.member_entities?.missing ?? []).map(
    (item) => `<li><code>${esc(item.entity_id)}</code> in ${esc(item.component_id)}</li>`,
  );
  const missingEvidence = (review?.missing_evidence ?? []).map(
    (item) => `<li><code>${esc(item.evidence_id)}</code> (${esc(item.component_id ?? item.source_id)})</li>`,
  );
  const unresolved = (review?.unresolved_dependency_endpoints ?? []).map(
    (item) =>
      `<li>${esc(item.source_id)} → ${esc(item.target_id)} <span class="muted">${esc(item.reason)}</span></li>`,
  );
  const ignored = review?.commands_ignored ?? 0;
  const ruleResults = checks?.results ?? [];
  const ruleRows = ruleResults.map((result) => {
    const violations = result.violations ?? [];
    return (
      `<li><code>${esc(result.rule_id)}</code> · ${esc(result.kind)} · ` +
      `<span class="${result.verdict === 'passed' ? '' : 'warn'}">${esc(result.verdict)}</span> · ` +
      `coverage: ${esc(result.coverage?.state ?? 'none')} · ` +
      `violations: ${violations.length}` +
      (violations.length ? `<pre>${esc(JSON.stringify(violations).slice(0, 2000))}</pre>` : '') +
      `</li>`
    );
  });
  const ruleState = checks?.state ?? 'unverified';
  return `<!doctype html>
<html lang="en">
<head>
<meta charset="utf-8">
<meta http-equiv="Content-Security-Policy" content="default-src 'none'; style-src ${esc(cspSource || "'unsafe-inline'")} 'unsafe-inline';">
<title>ARCHMELD candidate review</title>
<style>
  body { font-family: var(--vscode-font-family); color: var(--vscode-foreground); padding: 16px; line-height: 1.45; }
  h1 { font-size: 18px; margin: 0 0 4px; }
  h2 { font-size: 14px; margin: 20px 0 8px; text-transform: uppercase; letter-spacing: .04em; }
  .badges span { display: inline-block; border: 1px solid var(--vscode-panel-border); border-radius: 999px; padding: 1px 8px; margin-right: 6px; font-size: 12px; }
  .warn { border-color: var(--vscode-editorWarning-foreground); }
  .muted { color: var(--vscode-descriptionForeground); font-size: 12px; }
  .empty { color: var(--vscode-descriptionForeground); font-style: italic; }
  ul { padding-left: 18px; }
  li { margin: 4px 0; }
  code { font-family: var(--vscode-editor-font-family); }
  pre { font-family: var(--vscode-editor-font-family); background: var(--vscode-textCodeBlock-background); padding: 6px; overflow-x: auto; white-space: pre-wrap; font-size: 11px; }
</style>
</head>
<body>
<h1>${esc(architecture?.id ?? 'architecture candidate')}</h1>
<div class="badges">
  <span>status: ${esc(status)}</span>
  <span>layer: ${esc(architecture?.layer ?? 'unknown')}</span>
  <span class="${state === 'complete' ? '' : 'warn'}">review: ${esc(state)}</span>
  <span>candidate only</span>
  <span class="warn">certain calls added: ${esc(review?.certain_calls_added ?? 0)}</span>
  <span>provider commands ignored: ${esc(ignored)}</span>
</div>
<h2>Components (${(architecture?.components ?? []).length})</h2>
${renderList(components, 'No components.')}
<h2>Dependencies (${(architecture?.dependencies ?? []).length})</h2>
${renderList(
  (architecture?.dependencies ?? []).map(
    (dependency) =>
      `<li>${esc(dependency.source_id)} → ${esc(dependency.target_id)} <span class="muted">${esc(dependency.reason)} (${esc(dependency.status)})</span></li>`,
  ),
  'No dependencies.',
)}
<h2>Architecture rules (${ruleResults.length})</h2>
<div class="badges">
  <span class="${ruleState === 'passed' ? '' : 'warn'}">rule state: ${esc(ruleState)}</span>
  <span class="${(checks?.blocking_rules ?? []).length ? 'warn' : ''}">blocking: ${(checks?.blocking_rules ?? []).length}</span>
</div>
${renderList(ruleRows, 'No rules are declared on this architecture.')}
<h2>Unverified source references (${unverified.length})</h2>
${renderList(unverified, 'All source references resolved in this snapshot.')}
<h2>Missing member entities (${missingMembers.length})</h2>
${renderList(missingMembers, 'No missing member entities.')}
<h2>Missing evidence (${missingEvidence.length})</h2>
${renderList(missingEvidence, 'No missing evidence references.')}
<h2>Unresolved dependency endpoints (${unresolved.length})</h2>
${renderList(unresolved, 'Every dependency endpoint names a declared component.')}
</body>
</html>`;
}

module.exports = { renderReviewHtml };
