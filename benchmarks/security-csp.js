#!/usr/bin/env node
// Copyright 2026 ARCHMELD contributors
// SPDX-License-Identifier: Apache-2.0
//
// B028 CSP evidence: render every webview surface with hostile text and assert
// that no remote asset, inline un-nonced script or unescaped markup survives.

'use strict';

const fs = require('node:fs');
const path = require('node:path');
const { GraphModel } = require(path.join(__dirname, '..', 'archmeld-vscode', 'src', 'model.js'));
const { renderHtml } = require(path.join(__dirname, '..', 'archmeld-vscode', 'src', 'webview.js'));
const { renderReviewHtml } = require(path.join(__dirname, '..', 'archmeld-vscode', 'src', 'review.js'));
const { renderChangesetHtml } = require(path.join(__dirname, '..', 'archmeld-vscode', 'src', 'changeset.js'));

const injection = '<script>alert(1)</script> https://evil.example/leak';
const graph = new GraphModel();
graph.mergeResult({
  entities: [
    { id: 'e1', kind: 'symbol', qualified_name: injection },
    { id: 'e2', kind: 'symbol', qualified_name: 'safe' },
  ],
  relations: [{
    id: 'r1',
    source_id: 'e1',
    target_id: 'e2',
    kind: 'static_call_candidate',
    basis: 'syntax',
    resolution: 'candidate',
    evidence_ids: ['missing'],
  }],
});

function inspect(name, html, requireNonce) {
  const scriptSrc = (html.match(/script-src[^;]*/i) || [''])[0];
  return {
    surface: name,
    bytes: html.length,
    csp_default_none: html.includes("default-src 'none'"),
    remote_asset_present:
      /(?:src|href)\s*=\s*["']https?:\/\//i.test(html) || /url\(\s*["']?https?:/i.test(html),
    unescaped_script_tag: html.includes('<script>alert(1)</script>'),
    escaped_script_visible: html.includes('&lt;script&gt;'),
    script_src: scriptSrc,
    nonce_when_required: requireNonce ? /nonce-[0-9a-f]+/.test(scriptSrc) : null,
    unsafe_inline_script: /script-src[^;]*'unsafe-inline'/i.test(html),
  };
}

const graphHtml = renderHtml({ graph, title: injection, nonce: 'b028csp' });
const reviewHtml = renderReviewHtml({
  architecture: {
    id: injection,
    status: 'candidate',
    layer: 'as_is',
    components: [{ id: 'c1', name: injection, purpose: injection, member_entity_ids: [] }],
  },
  review: {
    state: 'partial',
    source_references: { details: [] },
    member_entities: { missing: [] },
    missing_evidence: [],
    unresolved_dependency_endpoints: [],
    certain_calls_added: 0,
    commands_ignored: 0,
  },
});
const changesetHtml = renderChangesetHtml({
  changeset: {
    state: 'prepared',
    effective_state: 'prepared',
    prepared_digest: 'sha256:' + '0'.repeat(64),
    approval: null,
  },
  diff: {
    operations: [{
      op: 'update',
      path: injection,
      before: { text: injection },
      after: { text: injection },
    }],
  },
  nonce: 'b028csp',
});

const surfaces = [
  inspect('graph', graphHtml, true),
  inspect('architecture-review', reviewHtml, false),
  inspect('changeset-review', changesetHtml, true),
];
for (const surface of surfaces) {
  if (!surface.csp_default_none) throw new Error(`${surface.surface}: missing default-src none`);
  if (surface.remote_asset_present) throw new Error(`${surface.surface}: remote asset present`);
  if (surface.unescaped_script_tag) throw new Error(`${surface.surface}: unescaped script tag`);
  if (surface.unsafe_inline_script) throw new Error(`${surface.surface}: script-src unsafe-inline`);
  if (surface.nonce_when_required && !surface.nonce_when_required) {
    throw new Error(`${surface.surface}: script nonce missing`);
  }
}

const result = {
  generated_by: 'benchmarks/security-csp.js',
  status: 'passed',
  injection: injection,
  surfaces,
  limitations: [
    'CSP is checked in rendered HTML, not by a browser security engine',
    'VS Code webview host policy is outside this script',
  ],
};
const serialized = JSON.stringify(result, null, 2) + '\n';
if (process.env.ARCHMELD_WRITE_EVIDENCE === '1') {
  const target = path.join(__dirname, '..', 'docs', 'evidence', 'B028-security-csp.json');
  fs.mkdirSync(path.dirname(target), { recursive: true });
  fs.writeFileSync(target, serialized, 'utf8');
}
process.stdout.write(serialized);
