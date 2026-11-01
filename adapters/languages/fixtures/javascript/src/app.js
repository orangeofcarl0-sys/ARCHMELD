import { helper as h } from './util.js';

export function dynamic(obj, key) {
  return obj[key]();
}

export const value = h(2);
