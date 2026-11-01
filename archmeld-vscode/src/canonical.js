// Copyright 2026 ARCHMELD contributors
// SPDX-License-Identifier: Apache-2.0

'use strict';

const crypto = require('node:crypto');

const MAX_SAFE = 9007199254740991;

/**
 * AM-C14N-1 canonical JSON for this client. It is intentionally narrow:
 * ASCII object keys, sorted keys, integers only, no floats and no lone
 * surrogates. Strings use JSON.stringify escapes, which match the pack vectors.
 */
function canonicalize(value) {
  if (value === null) {
    return 'null';
  }
  if (typeof value === 'boolean') {
    return value ? 'true' : 'false';
  }
  if (typeof value === 'number') {
    if (!Number.isFinite(value) || !Number.isInteger(value) || Math.abs(value) > MAX_SAFE) {
      throw new Error('AM-C14N-1 rejects non-integer, non-finite or unsafe numbers');
    }
    return String(value);
  }
  if (typeof value === 'string') {
    assertNoLoneSurrogate(value);
    return JSON.stringify(value);
  }
  if (Array.isArray(value)) {
    return `[${value.map((item) => canonicalize(item)).join(',')}]`;
  }
  if (typeof value === 'object') {
    const keys = Object.keys(value).sort();
    const parts = [];
    for (const key of keys) {
      if (!/^[\x00-\x7F]+$/.test(key)) {
        throw new Error('AM-C14N-1 object keys must be ASCII');
      }
      parts.push(`${JSON.stringify(key)}:${canonicalize(value[key])}`);
    }
    return `{${parts.join(',')}}`;
  }
  throw new Error(`unsupported canonical type ${typeof value}`);
}

function assertNoLoneSurrogate(text) {
  for (let index = 0; index < text.length; index += 1) {
    const code = text.charCodeAt(index);
    if (code >= 0xd800 && code <= 0xdbff) {
      const next = text.charCodeAt(index + 1);
      if (!(next >= 0xdc00 && next <= 0xdfff)) {
        throw new Error('AM-C14N-1 rejects lone surrogate code units');
      }
      index += 1;
    } else if (code >= 0xdc00 && code <= 0xdfff) {
      throw new Error('AM-C14N-1 rejects lone surrogate code units');
    }
  }
}

function digest(value) {
  const bytes = Buffer.from(canonicalize(value), 'utf8');
  return `sha256:${crypto.createHash('sha256').update(bytes).digest('hex')}`;
}

function digestBytes(bytes) {
  return `sha256:${crypto.createHash('sha256').update(bytes).digest('hex')}`;
}

/**
 * Parse JSON text while rejecting duplicate object keys. `JSON.parse` silently
 * collapses duplicates, so the duplicate check has to happen before it.
 */
function strictParse(text) {
  scanForDuplicates(text);
  return JSON.parse(text);
}

function scanForDuplicates(text) {
  let index = 0;
  const whitespace = () => {
    while (index < text.length && /\s/.test(text[index])) {
      index += 1;
    }
  };
  const fail = (message) => {
    throw new Error(`${message} at offset ${index}`);
  };
  const parseString = () => {
    if (text[index] !== '"') {
      fail('expected string');
    }
    const start = index;
    index += 1;
    while (index < text.length) {
      const char = text[index];
      if (char === '\\') {
        index += 2;
        continue;
      }
      if (char === '"') {
        index += 1;
        return JSON.parse(text.slice(start, index));
      }
      if (char < ' ') {
        fail('control character in string');
      }
      index += 1;
    }
    return fail('unterminated string');
  };
  const parseNumber = () => {
    const match = /^-?(0|[1-9][0-9]*)(\.[0-9]+)?([eE][+-]?[0-9]+)?/.exec(text.slice(index));
    if (!match) {
      fail('invalid number');
    }
    index += match[0].length;
  };
  const parseValue = () => {
    whitespace();
    const char = text[index];
    if (char === '{') {
      index += 1;
      whitespace();
      const keys = new Set();
      if (text[index] === '}') {
        index += 1;
        return;
      }
      for (;;) {
        whitespace();
        const key = parseString();
        if (keys.has(key)) {
          fail(`duplicate object key ${JSON.stringify(key)}`);
        }
        keys.add(key);
        whitespace();
        if (text[index] !== ':') {
          fail('expected colon');
        }
        index += 1;
        parseValue();
        whitespace();
        if (text[index] === ',') {
          index += 1;
          continue;
        }
        if (text[index] === '}') {
          index += 1;
          return;
        }
        fail('expected comma or closing brace');
      }
    }
    if (char === '[') {
      index += 1;
      whitespace();
      if (text[index] === ']') {
        index += 1;
        return;
      }
      for (;;) {
        parseValue();
        whitespace();
        if (text[index] === ',') {
          index += 1;
          continue;
        }
        if (text[index] === ']') {
          index += 1;
          return;
        }
        fail('expected comma or closing bracket');
      }
    }
    if (char === '"') {
      parseString();
      return;
    }
    for (const literal of ['true', 'false', 'null']) {
      if (text.startsWith(literal, index)) {
        index += literal.length;
        return;
      }
    }
    parseNumber();
  };
  parseValue();
  whitespace();
  if (index !== text.length) {
    fail('trailing content');
  }
}

module.exports = { canonicalize, digest, digestBytes, strictParse };
