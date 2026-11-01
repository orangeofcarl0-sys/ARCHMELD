// Copyright 2026 ARCHMELD contributors
// SPDX-License-Identifier: Apache-2.0

'use strict';

const test = require('node:test');
const assert = require('node:assert/strict');
const { canonicalize, digest, strictParse } = require('../src/canonical');

test('AM-C14N-1 vectors match the pack', () => {
  assert.equal(canonicalize({ z: 1, a: '中文😀' }), '{"a":"中文😀","z":1}');
  assert.equal(
    digest({ z: 1, a: '中文😀' }),
    'sha256:8fbd973f9e57bb947d29ee5cf71a58798b1432a2cc175f609ac4776b58425e42',
  );
  const second = { b: true, a: [null, 'line\r\n', '\u0001', -3] };
  assert.equal(
    canonicalize(second),
    '{"a":[null,"line\\r\\n","\\u0001",-3],"b":true}',
  );
  assert.equal(
    digest(second),
    'sha256:2b45fd8625c35a11dfcc7227852b4a368cd77eea46963036dc9ede93cad48c12',
  );
  const third = { path: 'src/例子.py', time_ns: '9007199254740993' };
  assert.equal(
    digest(third),
    'sha256:f118b3a6d414f7863e8e43889fad6586b687426110e33a6e2a2360b5ae482c41',
  );
});

test('AM-C14N-1 rejects floats, unsafe integers, non-ASCII keys and lone surrogates', () => {
  assert.throws(() => canonicalize(1.5), /non-integer/);
  assert.throws(() => canonicalize(9007199254740992), /unsafe/);
  assert.throws(() => canonicalize({ '中': 1 }), /ASCII/);
  assert.throws(() => canonicalize('\ud800'), /surrogate/);
  assert.throws(() => canonicalize(undefined), /unsupported/);
});

test('strict JSON parsing rejects duplicate keys instead of collapsing them', () => {
  assert.throws(() => strictParse('{"a":1,"a":2}'), /duplicate object key/);
  assert.throws(() => strictParse('{"outer":{"x":1,"x":2}}'), /duplicate object key/);
  assert.deepEqual(strictParse('{"a":[1,true,null,"中文"]}'), {
    a: [1, true, null, '中文'],
  });
});
