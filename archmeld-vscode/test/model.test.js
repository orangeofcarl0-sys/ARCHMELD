// Copyright 2026 ARCHMELD contributors
// SPDX-License-Identifier: Apache-2.0

'use strict';

const test = require('node:test');
const assert = require('node:assert/strict');
const { GraphModel, relationStyle, positionFor } = require('../src/model');

const entity = (id, kind = 'symbol') => ({
  id,
  kind,
  qualified_name: `module::${id}`,
  snapshot_id: 'sha256:' + 'a'.repeat(64),
  project_id: 'project.test',
});

const relation = (id, source, target, extra = {}) => ({
  id,
  source_id: source,
  target_id: target,
  kind: 'static_call_candidate',
  basis: 'syntax',
  resolution: 'candidate',
  evidence_ids: ['evidence.syntax'],
  conditions: [],
  ...extra,
});

test('relations keep basis, resolution and unknown state distinct', () => {
  const candidate = relationStyle(relation('r1', 'a', 'b'), ['evidence.syntax']);
  assert.equal(candidate.lineStyle, 'dashed');
  assert.equal(candidate.certainty, false);
  assert.equal(candidate.label, 'syntax/candidate');

  const resolved = relationStyle(
    relation('r2', 'a', 'b', {
      kind: 'resolved_call',
      basis: 'semantic',
      resolution: 'resolved',
      evidence_ids: ['evidence.semantic'],
    }),
    ['evidence.semantic'],
  );
  assert.equal(resolved.lineStyle, 'solid');
  assert.equal(resolved.certainty, true);
  assert.equal(resolved.label, 'resolved');

  const unknown = relationStyle(relation('r3', 'a', 'b', { evidence_ids: [] }), []);
  assert.equal(unknown.lineStyle, 'dotted');
  assert.equal(unknown.label, 'unknown');
  assert.equal(unknown.certainty, false);
});

test('layout positions depend only on entity identity and never shift on merge', () => {
  const graph = new GraphModel();
  graph.mergeResult({ entities: [entity('a'), entity('b')], relations: [] });
  const before = new Map([...graph.nodes].map(([id, node]) => [id, node.position]));
  assert.deepEqual(before.get('a'), positionFor('a'));

  graph.mergeResult({ entities: [entity('a'), entity('c'), entity('d')], relations: [] });
  for (const [id, position] of before) {
    assert.deepEqual(graph.nodes.get(id).position, position, `position of ${id} moved`);
  }
});

test('views project the same entities without inventing a parent tree', () => {
  const graph = new GraphModel();
  graph.mergeResult({
    entities: [entity('file.a', 'file'), entity('sym.a', 'symbol'), entity('mod.a', 'module')],
    relations: [],
  });
  graph.setView('file');
  assert.deepEqual(graph.visibleNodes().nodes.map((n) => n.entity.id).sort(), ['file.a', 'sym.a']);
  graph.setView('responsibility');
  assert.deepEqual(graph.visibleNodes().nodes.map((n) => n.entity.id), ['mod.a']);
  graph.setView('runtime');
  const runtime = graph.visibleNodes();
  assert.equal(runtime.nodes.length, 3, 'falls back to the local graph instead of pretending empty');
  assert.match(runtime.warning, /no matching entity kinds/);
});

test('selection updates are loop-safe and back navigation restores history', () => {
  const graph = new GraphModel();
  assert.equal(graph.pushSelection({ entityId: 'a', originView: 'file' }), true);
  assert.equal(graph.pushSelection({ entityId: 'a', originView: 'file' }), false);
  assert.equal(graph.pushSelection({ entityId: 'b', originView: 'file' }), true);
  assert.deepEqual(graph.back(), { entityId: 'a', originView: 'file' });
  assert.equal(graph.back(), null);
});

test('evidence lookup never fabricates evidence', () => {
  const graph = new GraphModel();
  graph.mergeResult({
    entities: [entity('a'), entity('b')],
    relations: [relation('r1', 'a', 'b')],
    evidence: [{ id: 'evidence.syntax', kind: 'syntax', locator: 'src/app.py' }],
  });
  assert.equal(graph.evidenceFor('r1').length, 1);
  assert.deepEqual(graph.evidenceFor('missing'), []);
});

test('entityAt requires an exact anchor containing the cursor line', () => {
  const graph = new GraphModel();
  const anchor = (lineStart, lineEnd, precision = 'line_only', byteStart = null, byteEnd = null) => ({
    path: 'src/a.py',
    blob_digest: 'sha256:' + 'b'.repeat(64),
    precision,
    byte_start: byteStart,
    byte_end: byteEnd,
    line_start: lineStart,
    line_end: lineEnd,
    encoding: 'utf-8',
  });
  graph.mergeResult({
    entities: [
      { id: 'e1', kind: 'symbol', qualified_name: 'a::one', anchors: [anchor(1, 2, 'bytes', 0, 10)] },
      { id: 'e2', kind: 'symbol', qualified_name: 'a::two', anchors: [anchor(2, 2, 'bytes', 10, 20)] },
      { id: 'e3', kind: 'symbol', qualified_name: 'a::three', anchors: [anchor(1, 5)] },
      { id: 'e4', kind: 'symbol', qualified_name: 'b::one', anchors: [{ ...anchor(1, 2), path: 'src/b.py' }] },
    ],
    relations: [],
  });
  // Cursor line is 0-based: line 1 is upstream line 2.
  assert.equal(graph.entityAt([...graph.nodes.values()].map((n) => n.entity), { path: 'src/a.py', line: 1, character: 0 }).id, 'e2');
  // Byte precision beats a wider line-only anchor on line 0.
  assert.equal(graph.entityAt([...graph.nodes.values()].map((n) => n.entity), { path: 'src/a.py', line: 0, character: 3 }).id, 'e1');
  assert.equal(graph.entityAt([...graph.nodes.values()].map((n) => n.entity), { path: 'src/a.py', line: 9, character: 0 }), null);
  assert.equal(graph.entityAt([...graph.nodes.values()].map((n) => n.entity), { path: 'src/missing.py', line: 0, character: 0 }), null);
});
