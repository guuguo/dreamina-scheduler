import { test } from 'node:test';
import assert from 'node:assert/strict';
import {
  normalizeMentionItems,
  deriveRoleChips,
  filterItems,
  moveGridFocus,
  sortByRecent,
} from './material-mention-picker-utils.js';

const sampleItems = [
  { key: 'role:r1', label: '女主', type: 'role', roleId: 'r1', roleName: '女主' },
  { key: 'img:a1', label: '女主日常服', type: 'image', roleId: 'r1', roleName: '女主', assetId: 'a1', assetName: '日常服', storedPath: '/a/1.jpg' },
  { key: 'img:a2', label: '女主厨师服', type: 'image', roleId: 'r1', roleName: '女主', assetId: 'a2', assetName: '厨师服', storedPath: '/a/2.jpg' },
  { key: 'aud:b1', label: '女主温柔音色', type: 'audio', roleId: 'r1', roleName: '女主', assetId: 'b1', assetName: '温柔音色', storedPath: '/b/1.mp3', duration_seconds: 5 },
  { key: 'temp:c1', label: '图片1', type: 'temp_image', assetId: 'c1', storedPath: '/c/1.jpg' },
];

// ── normalizeMentionItems ──────────────────────────────────────────────────

test('filters out role type items', () => {
  const items = normalizeMentionItems(sampleItems);
  assert.ok(!items.find((i) => i.type === 'role'));
  assert.equal(items.length, 4);
});

test('image item has displayType role_image and correct insertText', () => {
  const items = normalizeMentionItems(sampleItems);
  const img = items.find((i) => i.key === 'img:a1');
  assert.equal(img.displayType, 'role_image');
  assert.equal(img.insertText, '@女主日常服');
  assert.equal(img.roleName, '女主');
});

test('audio item has displayType role_audio and durationSeconds', () => {
  const items = normalizeMentionItems(sampleItems);
  const aud = items.find((i) => i.key === 'aud:b1');
  assert.equal(aud.displayType, 'role_audio');
  assert.equal(aud.durationSeconds, 5);
});

test('temp_image item has displayType temp_image and empty roleId', () => {
  const items = normalizeMentionItems(sampleItems);
  const tmp = items.find((i) => i.key === 'temp:c1');
  assert.equal(tmp.displayType, 'temp_image');
  assert.equal(tmp.roleId, '');
});

test('searchText contains label, assetName, roleName and type word', () => {
  const items = normalizeMentionItems(sampleItems);
  const aud = items.find((i) => i.key === 'aud:b1');
  assert.ok(aud.searchText.includes('温柔'));
  assert.ok(aud.searchText.includes('音频'));
  assert.ok(aud.searchText.includes('女主'));
});

test('empty mentionItems returns empty array', () => {
  assert.deepEqual(normalizeMentionItems([]), []);
});

// ── deriveRoleChips ───────────────────────────────────────────────────────

test('deriveRoleChips starts with all and includes role chips', () => {
  const items = normalizeMentionItems(sampleItems);
  const chips = deriveRoleChips(items);
  assert.equal(chips[0].id, 'all');
  assert.ok(chips.find((c) => c.id === 'r1'));
});

test('deriveRoleChips with no-role items adds other chip', () => {
  const withNoRole = [
    ...normalizeMentionItems(sampleItems),
    { key: 'img:x', label: '无角色图', type: 'image', displayType: 'role_image', roleId: '', roleName: '', assetId: 'x', storedPath: '', durationSeconds: null, insertText: '@无角色图', assetName: '无角色图', searchText: '无角色图 图片', isRecent: false },
  ];
  const chips = deriveRoleChips(withNoRole);
  assert.ok(chips.find((c) => c.id === 'other'));
});

// ── filterItems ───────────────────────────────────────────────────────────

test('filterItems category role_image returns only images', () => {
  const items = normalizeMentionItems(sampleItems);
  const result = filterItems(items, { category: 'role_image' });
  assert.ok(result.every((i) => i.displayType === 'role_image'));
  assert.equal(result.length, 2);
});

test('filterItems category role_audio returns only audios', () => {
  const items = normalizeMentionItems(sampleItems);
  const result = filterItems(items, { category: 'role_audio' });
  assert.equal(result.length, 1);
  assert.equal(result[0].key, 'aud:b1');
});

test('filterItems category temp_image returns only temp images', () => {
  const items = normalizeMentionItems(sampleItems);
  const result = filterItems(items, { category: 'temp_image' });
  assert.equal(result.length, 1);
  assert.equal(result[0].key, 'temp:c1');
});

test('filterItems by roleId filters correctly', () => {
  const items = normalizeMentionItems(sampleItems);
  const result = filterItems(items, { roleId: 'r1' });
  assert.ok(result.every((i) => i.roleId === 'r1'));
  assert.equal(result.length, 3);
});

test('filterItems by query matches label substring', () => {
  const items = normalizeMentionItems(sampleItems);
  const result = filterItems(items, { query: '厨师' });
  assert.equal(result.length, 1);
  assert.equal(result[0].key, 'img:a2');
});

test('filterItems empty query returns all non-role items', () => {
  const items = normalizeMentionItems(sampleItems);
  const result = filterItems(items, { query: '' });
  assert.equal(result.length, 4);
});

test('filterItems no match returns empty array', () => {
  const items = normalizeMentionItems(sampleItems);
  const result = filterItems(items, { query: '不存在的素材xyz999' });
  assert.equal(result.length, 0);
});

test('filterItems category + roleId combination works', () => {
  const items = normalizeMentionItems(sampleItems);
  const result = filterItems(items, { category: 'role_image', roleId: 'r1' });
  assert.ok(result.every((i) => i.displayType === 'role_image' && i.roleId === 'r1'));
  assert.equal(result.length, 2);
});

test('filterItems recent category shows all items (no isRecent filter)', () => {
  const items = normalizeMentionItems(sampleItems).map((i, idx) => ({ ...i, isRecent: idx === 0 }));
  const result = filterItems(items, { category: 'recent' });
  assert.equal(result.length, items.length);
});

// ── moveGridFocus ─────────────────────────────────────────────────────────

test('ArrowRight moves to next item', () => {
  assert.equal(moveGridFocus(0, 'ArrowRight', 4, 8), 1);
});

test('ArrowLeft at 0 stays at 0', () => {
  assert.equal(moveGridFocus(0, 'ArrowLeft', 4, 8), 0);
});

test('ArrowDown moves down one row', () => {
  assert.equal(moveGridFocus(1, 'ArrowDown', 4, 8), 5);
});

test('ArrowUp at row 0 stays at top', () => {
  assert.equal(moveGridFocus(1, 'ArrowUp', 4, 8), 0);
});

test('ArrowDown at last row stays at last item', () => {
  assert.equal(moveGridFocus(7, 'ArrowDown', 4, 8), 7);
});

test('ArrowRight at last item stays at last item', () => {
  assert.equal(moveGridFocus(7, 'ArrowRight', 4, 8), 7);
});

test('moveGridFocus returns currentIndex if total is 0', () => {
  assert.equal(moveGridFocus(0, 'ArrowRight', 4, 0), 0);
});

// ── sortByRecent ──────────────────────────────────────────────────────────

test('sortByRecent puts recently used items first', () => {
  const items = normalizeMentionItems(sampleItems);
  const sorted = sortByRecent(items, ['b1']);
  assert.equal(sorted[0].assetId, 'b1');
  assert.ok(sorted[0].isRecent);
});

test('sortByRecent with empty list returns items unchanged', () => {
  const items = normalizeMentionItems(sampleItems);
  const sorted = sortByRecent(items, []);
  assert.deepEqual(sorted, items);
});

test('sortByRecent marks only matched items as isRecent', () => {
  const items = normalizeMentionItems(sampleItems);
  const sorted = sortByRecent(items, ['a1']);
  const recentItems = sorted.filter((i) => i.isRecent);
  assert.equal(recentItems.length, 1);
  assert.equal(recentItems[0].assetId, 'a1');
});
