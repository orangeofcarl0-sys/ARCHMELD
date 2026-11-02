// Copyright 2026 ARCHMELD contributors
// SPDX-License-Identifier: Apache-2.0

'use strict';

const test = require('node:test');
const assert = require('node:assert/strict');
const { GraphModel } = require('../src/model');
const { renderHtml, escapeHtml } = require('../src/webview');

function graphWithInjection() {
  const graph = new GraphModel();
  graph.mergeResult({
    entities: [
      { id: 'e1', kind: 'symbol', qualified_name: '<script>alert(1)</script>' },
      { id: 'e2', kind: 'symbol', qualified_name: 'b' },
    ],
    relations: [
      {
        id: 'r1',
        source_id: 'e1',
        target_id: 'e2',
        kind: 'static_call_candidate',
        basis: 'syntax',
        resolution: 'candidate',
        evidence_ids: ['missing.evidence'],
      },
    ],
  });
  return graph;
}

test('webview escapes labels and never loads remote resources', () => {
  const html = renderHtml({ graph: graphWithInjection() });
  assert.ok(!html.includes('<script>alert(1)</script>'));
  assert.ok(html.includes('&lt;script&gt;alert(1)&lt;/script&gt;'));
  assert.ok(!/https?:\/\//.test(html), 'no remote URL may appear in the webview');
  assert.ok(html.includes("default-src 'none'"));
  assert.match(html, /script-src 'nonce-[0-9a-f]+'/);
  assert.ok(!/script-src[^;]*'unsafe-inline'/.test(html), 'fallback script uses a nonce, not unsafe-inline');
  assert.match(html, /<script nonce="[0-9a-f]+">/);
});

test('webview labels candidate edges and shows the always-on legend', () => {
  const html = renderHtml({ graph: graphWithInjection() });
  assert.ok(html.includes('dashed'), 'candidate edge uses a dashed stroke');
  assert.ok(html.includes('>unknown<') || html.includes('unknown'), 'missing evidence renders as unknown');
  assert.ok(html.includes('certain'));
  assert.ok(html.includes('candidate'));
  assert.ok(html.includes('Responsibilities'));
  assert.ok(html.includes('Runtime'));
});

test('empty and error states are distinguishable', () => {
  const empty = renderHtml({ graph: new GraphModel() });
  assert.ok(empty.includes('No entities'));
  assert.ok(empty.includes('not an indexing failure'));
  const error = renderHtml({ graph: new GraphModel(), error: 'API unreachable' });
  assert.ok(error.includes('Graph unavailable'));
  assert.ok(error.includes('API unreachable'));
  assert.ok(!error.includes('No entities'));
});

test('stale/historical snapshots are announced', () => {
  const html = renderHtml({ graph: new GraphModel(), stale: true });
  assert.ok(html.includes('historical snapshot'));
});

test('graph webview reports its own paint and render timings under the CSP nonce', () => {
  const html = renderHtml({ graph: graphWithInjection(), nonce: 'abc123', paintProbe: true });
  assert.ok(html.includes("type: 'paintReport'"));
  assert.ok(html.includes("type: 'renderReport'"));
  assert.ok(html.includes("data.type !== 'archmeld.paintProbe'"));
  assert.ok(html.includes('requestAnimationFrame'));
  assert.ok((html.match(/<script nonce="abc123">/g) || []).length >= 2);
  assert.ok(html.includes("script-src 'nonce-abc123'"));
});

test('paint probe is opt-in and absent from the default render', () => {
  const html = renderHtml({ graph: graphWithInjection(), nonce: 'abc123' });
  assert.ok(!html.includes('archmeld.paintProbe'));
  assert.ok(!html.includes("type: 'paintReport'"));
});

test('escapeHtml covers quotes used in attributes', () => {
  assert.equal(escapeHtml('a"b\'c<d>'), 'a&quot;b&#39;c&lt;d&gt;');
});

test('cytoscape renderer embeds preset elements and a local-only asset', () => {
  const html = renderHtml({
    graph: graphWithInjection(),
    cytoscapeUri: 'vscode-resource://extension/media/cytoscape.min.js',
    cspSource: 'vscode-resource://extension',
    nonce: 'abc123',
  });
  assert.ok(html.includes('id="cy"'));
  assert.ok(html.includes('src="vscode-resource://extension/media/cytoscape.min.js"'));
  assert.ok(html.includes('type="application/json" id="archmeld-graph"'));
  assert.ok(html.includes('"name":"preset"') || html.includes("name: 'preset'"));
  assert.ok(html.includes('edge.dashed'));
  assert.ok(html.includes('edge.dotted'));
  assert.ok(html.includes('"position"'));
  assert.ok(html.includes('data-view="module"'));
  assert.ok(html.includes("cy.on('dragfree', 'node'"));
  assert.ok(html.includes("type: 'moveNode'"));
  assert.ok(html.includes("script-src 'nonce-abc123'"));
  assert.ok(html.includes('vscode-resource://extension'));
  assert.ok(!html.includes('<script>alert(1)</script>'));
  assert.ok(html.includes(String.raw`\u003cscript\u003e`));
  assert.ok(!/https?:\/\//.test(html), 'no remote URL may appear in the webview');
});
