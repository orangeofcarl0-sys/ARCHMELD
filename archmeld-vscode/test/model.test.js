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

test('module view aggregates by path prefix and keeps underlying relations and evidence', () => {
  const withPath = (id, path) => ({
    ...entity(id),
    anchors: [
      {
        path,
        blob_digest: 'sha256:' + 'b'.repeat(64),
        precision: 'line_only',
        line_start: 1,
        line_end: 2,
      },
    ],
  });
  const graph = new GraphModel();
  graph.mergeResult({
    entities: [
      withPath('a', 'src/a.py'),
      withPath('b', 'src/b.py'),
      withPath('c', 'test/a.py'),
    ],
    relations: [
      relation('r1', 'a', 'b'),
      relation('r2', 'a', 'c', {
        basis: 'semantic',
        resolution: 'resolved',
        evidence_ids: ['evidence.semantic'],
      }),
    ],
    evidence: [{ id: 'evidence.syntax' }, { id: 'evidence.semantic' }],
  });
  graph.setView('module');
  const nodes = graph.visibleNodes();
  assert.match(nodes.warning, /path-prefix projection/);
  assert.deepEqual(
    nodes.nodes.map((node) => node.entity.qualified_name).sort(),
    ['src', 'test'],
  );
  const positions = new Map(nodes.nodes.map((node) => [node.entity.qualified_name, node.position]));
  assert.ok(positions.get('src').x < positions.get('test').x, 'dependency direction sets columns');

  const edges = graph.visibleEdges(nodes.nodes.map((node) => node.entity.id));
  assert.equal(edges.length, 1, 'intra-module edges are not drawn');
  assert.deepEqual(edges[0].relation_ids, ['r2']);
  assert.deepEqual(edges[0].evidence_ids, ['evidence.semantic']);
  assert.equal(edges[0].basis, 'projection');
  assert.equal(edges[0].style.certainty, false);
  assert.equal(edges[0].style.label, 'module projection');

  graph.mergeResult({ entities: [withPath('d', 'src/d.py')], relations: [] });
  graph.setView('module');
  const after = new Map(
    graph.visibleNodes().nodes.map((node) => [node.entity.qualified_name, node.position]),
  );
  assert.deepEqual(after.get('src'), positions.get('src'), 'existing module position moved');
  assert.deepEqual(after.get('test'), positions.get('test'), 'existing module position moved');
});

test('manual positions override layout, persist across projections, and module members drill down', () => {
  const withPath = (id, path, kind = 'symbol') => ({
    ...entity(id, kind),
    anchors: [
      {
        path,
        blob_digest: 'sha256:' + 'b'.repeat(64),
        precision: 'line_only',
        line_start: 1,
        line_end: 2,
      },
    ],
  });
  const graph = new GraphModel();
  graph.mergeResult({
    entities: [withPath('a', 'src/a.py'), withPath('b', 'src/b.py'), withPath('c', 'test/a.py')],
    relations: [relation('r1', 'a', 'b'), relation('r2', 'a', 'c')],
  });
  graph.setView('file');
  assert.equal(graph.setPosition('a', { x: 11.4, y: 22.6 }), true);
  assert.deepEqual(
    graph.visibleNodes().nodes.find((node) => node.entity.id === 'a').position,
    { x: 11, y: 23 },
  );
  graph.setPosition('module:src', { x: 5, y: 6 });
  graph.setView('module');
  const module = graph.visibleNodes().nodes.find((node) => node.entity.id === 'module:src');
  assert.deepEqual(module.position, { x: 5, y: 6 });
  assert.deepEqual(
    graph.membersOfModule('module:src').map((member) => member.id),
    ['a', 'b'],
  );
  const aggregate = graph.displayEdge('module-edge:module:src->module:test');
  assert.equal(aggregate.aggregate, true);
  assert.deepEqual(aggregate.edge.relation_ids, ['r2']);
  assert.equal(graph.displayEdge('r1').aggregate, false);
  assert.deepEqual(graph.exportPositionOverrides(), { a: { x: 11, y: 23 }, 'module:src': { x: 5, y: 6 } });
});

test('responsibility view uses architecture member ids and retains dependency evidence', () => {
  const graph = new GraphModel();
  graph.mergeResult({
    entities: [entity('a'), entity('b'), entity('c')],
    relations: [relation('r1', 'a', 'b'), relation('r2', 'b', 'c')],
    evidence: [{ id: 'evidence.syntax' }, { id: 'evidence.arch' }],
  });
  graph.setArchitecture({
    id: 'arch.responsibility',
    snapshot_id: 'sha256:' + 'a'.repeat(64),
    layer: 'to_be',
    status: 'candidate',
    components: [
      {
        id: 'component.a',
        name: 'Input',
        member_entity_ids: ['a', 'b'],
        evidence_ids: [],
      },
      {
        id: 'component.b',
        name: 'Output',
        member_entity_ids: ['c'],
        evidence_ids: [],
      },
    ],
    dependencies: [
      {
        source_id: 'component.a',
        target_id: 'component.b',
        reason: 'input feeds output',
        evidence_ids: ['evidence.arch'],
        status: 'candidate',
      },
    ],
  });
  graph.setView('responsibility');
  const visible = graph.visibleNodes();
  assert.match(visible.warning, /candidate architecture/);
  assert.deepEqual(
    visible.nodes.map((node) => node.entity.qualified_name).sort(),
    ['Input', 'Output'],
  );
  assert.deepEqual(
    graph.membersOfComponent('component:component.a').map((member) => member.id),
    ['a', 'b'],
  );
  const edge = graph.visibleEdges(visible.nodes.map((node) => node.entity.id))[0];
  assert.equal(edge.id, 'component-edge:component:component.a->component:component.b');
  assert.deepEqual(edge.relation_ids, ['r2']);
  assert.deepEqual(edge.evidence_ids.sort(), ['evidence.arch', 'evidence.syntax']);
  assert.equal(edge.architecture_dependencies[0].reason, 'input feeds output');
  assert.equal(edge.style.certainty, false);
  assert.equal(edge.style.label, 'component projection');
  assert.equal(graph.displayEdge(edge.id).aggregate, true);
});

test('runtime filter keeps only mapped entities and labels edges as static adjacency', () => {
  const graph = new GraphModel();
  graph.mergeResult({
    entities: [entity('a'), entity('b'), entity('c')],
    relations: [relation('r1', 'a', 'b'), relation('r2', 'b', 'c')],
  });
  const filter = graph.setRuntimeRun(
    {
      spans: [
        {
          span_id: 's1',
          parent_span_id: null,
          attributes: { 'archmeld.entity_id': 'a' },
        },
        {
          span_id: 's2',
          parent_span_id: 's1',
          attributes: { 'archmeld.entity_id': 'b' },
        },
        { span_id: 's3', parent_span_id: 'missing', attributes: {} },
      ],
    },
    'run.runtime.1',
  );
  assert.deepEqual(filter.entityIds, ['a', 'b']);
  assert.equal(filter.spans, 3);
  assert.equal(filter.spanEdges, 1);
  graph.setView('runtime');
  const visible = graph.visibleNodes();
  assert.deepEqual(visible.nodes.map((node) => node.entity.id).sort(), ['a', 'b']);
  assert.match(visible.warning, /observed span-parent/);
  const visibleEdges = graph.visibleEdges(['a', 'b']);
  const spanEdge = visibleEdges.find((edge) => edge.id === 'runtime-edge:s1->s2');
  assert.ok(spanEdge, 'observed span-parent edge missing');
  assert.equal(spanEdge.source_id, 'a');
  assert.equal(spanEdge.target_id, 'b');
  assert.equal(spanEdge.style.label, 'span parent');
  assert.equal(spanEdge.style.certainty, false);
  assert.deepEqual(spanEdge.span_ids, ['s1', 's2']);
  assert.ok(visibleEdges.some((edge) => edge.id === 'r1'), 'static adjacency retained');
  graph.clearRuntimeFilter();
  assert.match(graph.visibleNodes().warning, /no matching entity kinds/);
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
