// Copyright 2026 ARCHMELD contributors
// SPDX-License-Identifier: Apache-2.0

'use strict';

const test = require('node:test');
const assert = require('node:assert/strict');
const { renderReviewHtml } = require('../src/review');

test('candidate review shows unverified evidence, ignored commands and no promotion', () => {
  const architecture = {
    id: 'arch.codeboarding.demo',
    status: 'candidate',
    layer: 'as_is',
    components: [
      {
        id: 'cb.1',
        name: '<script>alert(1)</script>',
        purpose: 'Candidate & unverified',
        member_entity_ids: ['syn.aaaa'],
      },
    ],
    dependencies: [
      { source_id: 'cb.1', target_id: 'cb.999', reason: 'invented <edge>', status: 'candidate' },
    ],
  };
  const review = {
    state: 'partial',
    candidate_only: true,
    certain_calls_added: 0,
    commands_ignored: 2,
    source_references: {
      details: [
        {
          path: 'src/old.py',
          line_start: 10,
          line_end: 99,
          qualified_name: 'pkg.old',
          status: 'unverified',
          reason: 'line range is outside the snapshot file',
        },
      ],
    },
    member_entities: {
      missing: [{ component_id: 'cb.1', entity_id: 'syn.bbbb' }],
    },
    missing_evidence: [{ component_id: 'cb.1', evidence_id: 'evidence.missing' }],
    unresolved_dependency_endpoints: [
      { source_id: 'cb.1', target_id: 'cb.999', reason: 'invented <edge>' },
    ],
  };
  const html = renderReviewHtml({ architecture, review, cspSource: 'vscode-webview:' });
  assert.match(html, /candidate only/);
  assert.match(html, /certain calls added: 0/);
  assert.match(html, /provider commands ignored: 2/);
  assert.match(html, /src\/old\.py/);
  assert.match(html, /line range is outside the snapshot file/);
  assert.match(html, /syn\.bbbb/);
  assert.match(html, /cb\.999/);
  assert.doesNotMatch(html, /<script>alert/);
  assert.match(html, /&lt;script&gt;alert/);
  assert.match(html, /invented &lt;edge&gt;/);
  assert.doesNotMatch(html, /http:\/\//);
});

test('complete review does not invent warnings', () => {
  const html = renderReviewHtml({
    architecture: { id: 'arch.ok', status: 'candidate', layer: 'as_is', components: [], dependencies: [] },
    review: {
      state: 'complete',
      certain_calls_added: 0,
      commands_ignored: 0,
      source_references: { details: [] },
      member_entities: { missing: [] },
      missing_evidence: [],
      unresolved_dependency_endpoints: [],
    },
    cspSource: 'vscode-webview:',
  });
  assert.match(html, /review: complete/);
  assert.match(html, /All source references resolved/);
  assert.match(html, /No missing member entities/);
  assert.match(html, /Every dependency endpoint names a declared component/);
});

test('architecture rule results and coverage are shown in the review panel', () => {
  const html = renderReviewHtml({
    architecture: { id: 'arch.rules', status: 'candidate', layer: 'to_be', components: [], dependencies: [] },
    review: {
      state: 'complete',
      certain_calls_added: 0,
      commands_ignored: 0,
      source_references: { details: [] },
      member_entities: { missing: [] },
      missing_evidence: [],
      unresolved_dependency_endpoints: [],
    },
    checks: {
      state: 'incomplete_coverage',
      blocking_rules: ['rule.forbid'],
      results: [
        {
          rule_id: 'rule.forbid',
          kind: 'forbid_dependency',
          verdict: 'no_violation_found',
          checkability: 'executable',
          coverage: { state: 'partial' },
          violations: [{ path: ['<script>alert(1)</script>'] }],
        },
      ],
    },
    cspSource: 'vscode-webview:',
  });
  assert.match(html, /rule state: incomplete_coverage/);
  assert.match(html, /blocking: 1/);
  assert.match(html, /rule\.forbid/);
  assert.match(html, /coverage: partial/);
  assert.match(html, /violations: 1/);
  assert.doesNotMatch(html, /<script>alert/);
});
