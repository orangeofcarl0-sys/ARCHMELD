import { helper as h } from './util';

export function dynamic(obj: any, key: string) {
  return obj[key]();
}

export const value = h(2);
