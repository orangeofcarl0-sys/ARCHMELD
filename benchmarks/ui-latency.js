#!/usr/bin/env node
// Copyright 2026 ARCHMELD contributors
// SPDX-License-Identifier: Apache-2.0
//
// Deterministic webview-render benchmark. This exercises the same model and
// HTML renderer used by the VS Code panel, but it is not an editor paint
// measurement: no extension host or GPU is involved. The limitation is part of
// the B026 evidence.

'use strict';

const path = require('node:path');
const { GraphModel } = require(path.join(__dirname, '..', 'archmeld-vscode', 'src', 'model.js'));
const { renderHtml } = require(path.join(__dirname, '..', 'archmeld-vscode', 'src', 'webview.js'));

function parsePositive(index, fallback) {
  const value = Number(process.argv[2 + index] ?? fallback);
  if (!Number.isInteger(value) || value < 1) {
    throw new Error(`invalid benchmark parameter at index ${index}`);
  }
  return value;
}

const nodeCount = parsePositive(0, 500);
const edgeCount = parsePositive(1, 1000);
const repetitions = parsePositive(2, 30);

const entities = [];
for (let index = 0; index < nodeCount; index += 1) {
  const id = `entity.ui.${String(index).padStart(4, '0')}`;
  entities.push({
    id,
    kind: index % 5 === 0 ? 'file' : 'symbol',
    language: 'python',
    qualified_name: `module_${index}.function_${index}`,
    external: false,
    anchors: [{
      path: `src/module_${index}.py`,
      blob_digest: 'sha256:' + '0'.repeat(64),
      precision: 'line_only',
      byte_start: null,
      byte_end: null,
      line_start: 1,
      line_end: 1,
      encoding: 'utf-8',
    }],
  });
}
const relations = [];
for (let index = 0; index < edgeCount; index += 1) {
  relations.push({
    id: `relation.ui.${String(index).padStart(4, '0')}`,
    source_id: entities[index % nodeCount].id,
    target_id: entities[(index * 7 + 3) % nodeCount].id,
    kind: 'references',
    basis: 'syntax',
    resolution: 'resolved',
    evidence_ids: ['evidence.ui.bench'],
  });
}
const model = new GraphModel();
model.mergeResult({
  entities,
  relations,
  evidence: [{ id: 'evidence.ui.bench', kind: 'syntax' }],
  warnings: [],
  coverage: { state: 'complete' },
});

function percentile(samples, percentileValue) {
  const sorted = [...samples].sort((left, right) => left - right);
  const rank = Math.ceil((percentileValue / 100) * sorted.length);
  return sorted[Math.max(0, Math.min(sorted.length - 1, rank - 1))];
}

const samples = [];
let htmlBytes = 0;
for (let index = 0; index < repetitions; index += 1) {
  const started = process.hrtime.bigint();
  const html = renderHtml({
    graph: model,
    title: 'ARCHMELD benchmark',
    nonce: 'benchmark-nonce',
  });
  const elapsed = Number(process.hrtime.bigint() - started) / 1e6;
  samples.push(Number(elapsed.toFixed(3)));
  htmlBytes = html.length;
}
const sorted = [...samples].sort((left, right) => left - right);
console.log(JSON.stringify({
  kind: 'webview_html_generation',
  node_count: nodeCount,
  edge_count: edgeCount,
  repetitions,
  samples_ms: samples,
  min_ms: sorted[0],
  median_ms: percentile(samples, 50),
  p95_ms: percentile(samples, 95),
  max_ms: sorted[sorted.length - 1],
  html_bytes: htmlBytes,
  limitation: 'editor host and paint latency are not measured; this is deterministic renderer cost',
}));
