// Copyright 2026 ARCHMELD contributors
// SPDX-License-Identifier: Apache-2.0

'use strict';

const test = require('node:test');
const assert = require('node:assert/strict');
const { renderChangesetHtml } = require('../src/changeset');

const baseChangeset = {
  id: 'changeset.demo',
  revision: 4,
  state: 'prepared',
  effective_state: 'prepared',
  plan_id: 'plan.demo',
  step_id: 'step.1',
  prepared_digest: 'sha256:' + 'a'.repeat(64),
  allowed_paths: ['src/a.py'],
  read_set: ['config/settings.toml'],
  approval: null,
};

const baseDiff = {
  source_mutation: 'none',
  operations: [
    {
      op: 'update',
      path: 'src/a.py',
      destination: null,
      case_only: false,
      before: { digest: 'sha256:' + 'b'.repeat(64), size: 10, text: 'VALUE = 1\n', truncated: false },
      after: { digest: 'sha256:' + 'c'.repeat(64), size: 10, text: 'VALUE = 2\n', truncated: false },
    },
  ],
};

test('prepared change offers approval, not apply', () => {
  const html = renderChangesetHtml({ changeset: baseChangeset, diff: baseDiff, cspSource: 'vscode-webview:' });
  assert.match(html, /state: prepared/);
  assert.match(html, /src\/a\.py/);
  assert.match(html, /VALUE = 1/);
  assert.match(html, /VALUE = 2/);
  assert.match(html, /Approve this exact prepared change/);
  assert.doesNotMatch(html, /Apply this exact change/);
  assert.match(html, /approval: none/);
  assert.match(html, /source repository has not been modified/);
  assert.match(html, /script-src 'nonce-/);
});

test('approval enables the exact apply action and recovery appears after apply', () => {
  const approved = {
    ...baseChangeset,
    approval: { approval_id: 'approval.1234567890abcdef1234567890abcdef' },
  };
  const html = renderChangesetHtml({ changeset: approved, diff: baseDiff, cspSource: 'vscode-webview:' });
  assert.match(html, /Apply this exact change/);
  assert.doesNotMatch(html, /Approve this exact prepared change/);
  assert.match(html, /approval\.1234567890abcdef1234567890abcdef/);

  const applied = { ...approved, state: 'applied', revision: 5 };
  const appliedHtml = renderChangesetHtml({ changeset: applied, diff: baseDiff, cspSource: 'vscode-webview:' });
  assert.match(appliedHtml, /Recover owned writes/);
  assert.doesNotMatch(appliedHtml, /Apply this exact change/);
});

test('stale and hostile content is shown as escaped text only', () => {
  const changeset = {
    ...baseChangeset,
    effective_state: 'stale',
    allowed_paths: ['<img src=x onerror=alert(1)>'],
    approval: null,
  };
  const diff = {
    operations: [
      {
        op: 'update',
        path: '../<script>alert(1)</script>.py',
        destination: null,
        case_only: false,
        before: { digest: 'sha256:' + 'd'.repeat(64), size: 3, text: '<script>x</script>', truncated: true },
        after: null,
      },
    ],
  };
  const html = renderChangesetHtml({ changeset, diff, cspSource: 'vscode-webview:' });
  assert.match(html, /state: stale/);
  assert.doesNotMatch(html, /<script>alert/);
  assert.match(html, /&lt;script&gt;alert/);
  assert.match(html, /&lt;img src=x onerror=alert\(1\)&gt;/);
  assert.match(html, /truncated/);
  assert.match(html, /No action is legal in this state/);
});

test('applied change offers verification and no acceptance', () => {
  const applied = {
    ...baseChangeset,
    state: 'applied',
    revision: 5,
    approval: { approval_id: 'approval.1234567890abcdef1234567890abcdef' },
  };
  const html = renderChangesetHtml({ changeset: applied, diff: baseDiff, cspSource: 'vscode-webview:' });
  assert.match(html, /Verify the applied snapshot/);
  assert.doesNotMatch(html, /Accept this verified change/);
  assert.match(html, /No verification has been run/);
});

test('accepted verification shows checks and enables the separate accept action', () => {
  const verifying = {
    ...baseChangeset,
    state: 'verifying',
    revision: 5,
    approval: { approval_id: 'approval.1234567890abcdef1234567890abcdef' },
  };
  const verification = {
    verification_id: 'verification.demo',
    state: 'accepted',
    snapshot_id: 'sha256:' + 'e'.repeat(64),
    verdict: { acceptance: true, reasons: [] },
    checks: [
      { check_id: 'check.tests', state: 'passed', required: true },
      { check_id: 'check.units', state: 'passed', required: true },
    ],
  };
  const html = renderChangesetHtml({
    changeset: verifying,
    diff: baseDiff,
    verification,
    cspSource: 'vscode-webview:',
  });
  assert.match(html, /Independent verification/);
  assert.match(html, /verification: accepted/);
  assert.match(html, /acceptance: true/);
  assert.match(html, /check\.tests/);
  assert.match(html, /Accept this verified change/);
  assert.match(html, /verificationId: "verification\.demo"/);
});

test('failed verification never offers acceptance', () => {
  const verifying = { ...baseChangeset, state: 'verifying', revision: 5 };
  const verification = {
    verification_id: 'verification.failed',
    state: 'failed',
    snapshot_id: 'sha256:' + 'f'.repeat(64),
    verdict: { acceptance: false, reasons: ['check.tests failed'] },
    checks: [{ check_id: 'check.tests', state: 'failed', required: true }],
  };
  const html = renderChangesetHtml({
    changeset: verifying,
    diff: baseDiff,
    verification,
    cspSource: 'vscode-webview:',
  });
  assert.match(html, /check\.tests failed/);
  assert.match(html, /verification: failed/);
  assert.doesNotMatch(html, /Accept this verified change/);
});
