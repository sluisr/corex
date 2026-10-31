import test from 'node:test';
import assert from 'node:assert/strict';
import { slugify, chunk } from './slug.js';

test('slugify replaces every space', () => {
  assert.equal(slugify('Hola Mundo Cruel'), 'hola-mundo-cruel');
});

test('chunk has no empty tail', () => {
  assert.deepEqual(chunk([1, 2, 3, 4], 2), [[1, 2], [3, 4]]);
});
