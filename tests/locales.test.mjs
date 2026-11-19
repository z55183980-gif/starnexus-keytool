import { test } from 'node:test';
import assert from 'node:assert/strict';
import { DICT } from '../src/locales.ts';
import { RU } from '../src/locales.ru.ts';

const holes = text => [...text.matchAll(/\{(\w+)\}/g)].map(m => m[1]).sort().join(',');

test('every translated string has a Russian version', () => {
  const missing = Object.keys(DICT).filter(key => !(key in RU));
  assert.deepEqual(missing, []);
  const stale = Object.keys(RU).filter(key => !(key in DICT));
  assert.deepEqual(stale, []);
});

test('Russian strings keep the same placeholders as the source', () => {
  const broken = Object.entries(RU).filter(([key, value]) => holes(key) !== holes(value));
  assert.deepEqual(broken, []);
});
