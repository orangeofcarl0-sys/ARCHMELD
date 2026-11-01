// Copyright 2026 ARCHMELD contributors
// SPDX-License-Identifier: Apache-2.0

'use strict';

const test = require('node:test');
const assert = require('node:assert/strict');
const { renderTargetEditorHtml, targetTemplate } = require('../src/target');

const base = {
  id: 'arch.base',
  snapshot_id: 'sha256:' + 'a'.repeat(64),
  layer: 'as_is',
  status: 'accepted',
  revision: 7,
  components: [{ id: 'cb.1', name: '</script><script>alert(1)</script>', members: [] }],
  dependencies: [
    { source_id: 'cb.1', target_id: 'cb.2', reason: 'observed', status: 'observed' },
    { source_id: 'cb.1', target_id: 'cb.3', reason: 'candidate', status: 'candidate' },
  ],
};

test('target template switches to a candidate To-Be model and drops observed edges', () => {
  const target = targetTemplate(base);
  assert.equal(target.id, 'arch.base.to_be');
  assert.equal(target.layer, 'to_be');
  assert.equal(target.status, 'candidate');
  assert.equal(target.revision, 1);
  assert.equal(target.dependencies.length, 1);
  assert.equal(target.dependencies[0].status, 'candidate');
  assert.equal(target.snapshot_id, base.snapshot_id);
});

test('target editor is script-nonced, offline and escapes hostile model text', () => {
  const html = renderTargetEditorHtml({ base, cspSource: 'vscode-webview:' });
  assert.match(html, /To-Be editor/);
  assert.match(html, /Preview diff and intents/);
  assert.match(html, /script-src 'nonce-/);
  assert.match(html, /target.preview/);
  assert.match(html, /target.result/);
  assert.doesNotMatch(html, /<script>alert/);
  assert.match(html, /&lt;script&gt;alert/);
  assert.doesNotMatch(html, /https?:\/\//);
  assert.doesNotMatch(html, /script-src[^;]*unsafe-inline/);
});
