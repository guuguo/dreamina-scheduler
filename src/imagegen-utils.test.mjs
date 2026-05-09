import { strict as assert } from 'node:assert';
import { test } from 'node:test';
import { shouldSuppressImageContextMenu } from './imagegen-utils.js';

test('shouldSuppressImageContextMenu returns true for image targets', () => {
  assert.equal(shouldSuppressImageContextMenu({ tagName: 'IMG' }), true);
  assert.equal(shouldSuppressImageContextMenu({ nodeName: 'img' }), true);
});

test('shouldSuppressImageContextMenu returns false for non-image targets', () => {
  assert.equal(shouldSuppressImageContextMenu({ tagName: 'BUTTON' }), false);
  assert.equal(shouldSuppressImageContextMenu({ nodeName: 'div' }), false);
  assert.equal(shouldSuppressImageContextMenu(null), false);
});
