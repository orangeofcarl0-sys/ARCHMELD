// Copyright 2026 ARCHMELD contributors
// SPDX-License-Identifier: Apache-2.0

'use strict';

const test = require('node:test');
const assert = require('node:assert/strict');
const { spawn } = require('node:child_process');
const { once } = require('node:events');
const fs = require('node:fs');
const os = require('node:os');
const path = require('node:path');
const readline = require('node:readline');
const { ArchmeldClient } = require('../src/api');

const binary = process.env.ARCHMELD_SERVE
  || path.join(__dirname, '..', '..', 'target', 'debug', 'archmeld-serve.exe');

test('client refuses non-loopback endpoints', () => {
  assert.throws(() => new ArchmeldClient({ endpoint: 'http://0.0.0.0:1234' }), /loopback/);
  assert.throws(() => new ArchmeldClient({ endpoint: 'https://127.0.0.1:1234' }), /loopback/);
});

test('report request carries the explicit source and trace opt-ins', async () => {
  let captured = null;
  const client = new ArchmeldClient({
    endpoint: 'http://127.0.0.1:9',
    token: 'token',
    fetchImpl: async (url, options) => {
      captured = { url, options };
      return { ok: true, text: async () => '{"report_id":"report.test","risk_level":"high"}' };
    },
  });
  const result = await client.report(
    'project.test',
    'sha256:' + 'a'.repeat(64),
    'sensitive-report',
    true,
    true,
  );
  assert.equal(captured.url, 'http://127.0.0.1:9/v1/reports');
  assert.deepEqual(JSON.parse(captured.options.body), {
    project_id: 'project.test',
    snapshot_id: 'sha256:' + 'a'.repeat(64),
    output_name: 'sensitive-report',
    include_source: true,
    include_raw_traces: true,
  });
  assert.equal(result.risk_level, 'high');
});

test('status and workspace review use the authorized workspace handle', async () => {
  const calls = [];
  const client = new ArchmeldClient({
    endpoint: 'http://127.0.0.1:9',
    token: 'token',
    fetchImpl: async (url, options) => {
      calls.push({ url, options });
      return { ok: true, text: async () => '{"state":"changed"}' };
    },
  });
  await client.status('project.test', 'ws.abc');
  assert.equal(
    calls[0].url,
    'http://127.0.0.1:9/v1/status?project_id=project.test&workspace_handle=ws.abc',
  );
  await client.reviewWorkspaceDiff('project.test', 'ws.abc');
  assert.equal(calls[1].url, 'http://127.0.0.1:9/v1/reviews/workspace-diff');
  assert.deepEqual(JSON.parse(calls[1].options.body), {
    project_id: 'project.test',
    workspace_handle: 'ws.abc',
    base_snapshot_id: null,
  });
});

test('real server enforces tokens and returns structured errors', async (t) => {
  if (!fs.existsSync(binary)) {
    t.skip(`archmeld-serve binary not found at ${binary}`);
    return;
  }
  const dir = fs.mkdtempSync(path.join(os.tmpdir(), 'archmeld-api-'));
  const child = spawn(binary, ['--state', dir, '--port', '0', '--token', 'test-token'], {
    stdio: ['ignore', 'pipe', 'pipe'],
  });
  const lines = readline.createInterface({ input: child.stdout });
  const [line] = await once(lines, 'line');
  const announcement = JSON.parse(line);
  const endpoint = announcement.endpoint;
  t.after(() => {
    lines.close();
    child.kill();
  });
  assert.match(endpoint, /^http:\/\/127\.0\.0\.1:\d+$/);

  const response = await fetch(`${endpoint}/v1/health`);
  assert.equal(response.status, 200);
  const health = await response.json();
  assert.equal(health.service, 'archmeld');

  const unauthorised = await fetch(`${endpoint}/v1/query`, {
    method: 'POST',
    headers: { 'content-type': 'application/json' },
    body: JSON.stringify({}),
  });
  assert.equal(unauthorised.status, 403);
  const denied = await unauthorised.json();
  assert.equal(denied.code, 'permission_denied');

  const client = new ArchmeldClient({ endpoint, token: 'test-token' });
  await assert.rejects(
    client.query({
      project_id: 'project.missing',
      snapshot_id: 'sha256:' + 'a'.repeat(64),
      kind: 'symbols',
      query: '',
      entity_ids: [],
      relation_kinds: [],
      max_nodes: 10,
      max_depth: 0,
      cursor: null,
    }),
    (error) => error.status === 404 && error.code === 'not_found',
  );
  await assert.rejects(
    client.source({ project_id: 'project.missing' }),
    (error) => error.status === 400 && error.code === 'invalid_request',
  );
});

test('runtime and source-file requests use encoded loopback routes', async () => {
  const calls = [];
  const client = new ArchmeldClient({
    endpoint: 'http://127.0.0.1:1234',
    fetchImpl: async (url, options) => {
      calls.push({ url, options });
      return { ok: true, status: 200, text: async () => '{}' };
    },
  });
  await client.runtime('project a', 'run/x', 7);
  assert.equal(
    calls[0].url,
    'http://127.0.0.1:1234/v1/runtime/run%2Fx?project_id=project%20a&limit=7',
  );
  await client.sourceFile({ project_id: 'p', snapshot_id: 's', path: 'src/a.py', line: 3 });
  assert.equal(calls[1].url, 'http://127.0.0.1:1234/v1/source/file');
  assert.equal(calls[1].options.method, 'POST');
  assert.equal(JSON.parse(calls[1].options.body).line, 3);
});
