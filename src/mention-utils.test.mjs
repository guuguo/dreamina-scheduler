import assert from 'node:assert/strict';
import { test } from 'node:test';
import {
  applyMentionSelection,
  buildMentionItems,
  collectPromptMentions,
  highlightPromptMentions,
} from './mention-utils.js';

const role = {
  id: 'role-hero',
  name: '女主',
  aliases: ['小雅'],
  asset_ids: ['img-1', 'aud-1'],
};

const assets = new Map([
  ['img-1', { id: 'img-1', kind: 'image', name: '日常服', stored_path: '/tmp/hero.png', source_path: '/tmp/hero.png' }],
  ['aud-1', { id: 'aud-1', kind: 'audio', name: '温柔音色', stored_path: '/tmp/voice.mp3', source_path: '/tmp/voice.mp3' }],
]);

test('buildMentionItems exposes role images, temp images, and audio assets', () => {
  const items = buildMentionItems({
    roles: [role],
    assetById: assets,
    tempImagePaths: ['/tmp/story.png'],
    tempImageAssetIds: ['tmp-img-1'],
  });

  assert.ok(items.some((item) => item.key === 'img:img-1' && item.label === '女主日常服'));
  assert.ok(items.some((item) => item.key === 'aud:aud-1' && item.label === '女主温柔音色'));
  assert.ok(items.some((item) => item.key === 'temp:tmp-img-1' && item.label === '图片1'));
});

test('buildMentionItems hides disabled roles and their media resources', () => {
  const items = buildMentionItems({
    roles: [{ ...role, disabled: true }],
    assetById: assets,
  });

  assert.equal(items.some((item) => item.key === 'role:role-hero'), false);
  assert.equal(items.some((item) => item.key === 'img:img-1'), false);
  assert.equal(items.some((item) => item.key === 'aud:aud-1'), false);
});

test('buildMentionItems keeps imported asset names mention-safe when source file had spaces', () => {
  const spacedRole = {
    id: 'role-car',
    name: '威威',
    aliases: [],
    asset_ids: ['img-space'],
  };
  const spacedAssets = new Map([
    ['img-space', { id: 'img-space', kind: 'image', name: 'main_shot', stored_path: '/tmp/main shot.png', source_path: '/tmp/main shot.png' }],
  ]);

  const items = buildMentionItems({ roles: [spacedRole], assetById: spacedAssets });

  assert.ok(items.some((item) => item.key === 'img:img-space' && item.label === '威威main_shot'));
  assert.deepEqual(collectPromptMentions('@威威main_shot', items), [
    { text: '@威威main_shot', name: '威威main_shot', matched: true, type: 'image' },
  ]);
});

test('applyMentionSelection binds selected image and audio assets into the task form', () => {
  const baseForm = {
    prompt: '@女',
    image_asset_ids: [],
    audio_asset_ids: [],
    role_ids: [],
    manual_mention_ids: [],
  };

  const withImage = applyMentionSelection({
    form: baseForm,
    item: { label: '女主日常服', type: 'image', assetId: 'img-1' },
    atQuery: { start: 0, query: '女' },
  });
  assert.equal(withImage.prompt, '@女主日常服 ');
  assert.deepEqual(withImage.image_asset_ids, ['img-1']);

  const withAudio = applyMentionSelection({
    form: withImage,
    item: { label: '女主温柔音色', type: 'audio', assetId: 'aud-1' },
    atQuery: { start: withImage.prompt.length, query: '' },
  });
  assert.deepEqual(withAudio.audio_asset_ids, ['aud-1']);
});

test('matched temp image mentions are exposed for inline highlighting', () => {
  const items = buildMentionItems({
    tempImagePaths: ['/tmp/story.png'],
    tempImageAssetIds: ['tmp-img-1'],
  });
  const mentions = collectPromptMentions('@图片1 慢慢推近', items);
  const highlights = highlightPromptMentions('@图片1 慢慢推近', items);

  assert.deepEqual(mentions, [{ text: '@图片1', name: '图片1', matched: true, type: 'temp_image' }]);
  assert.ok(highlights.some((part) => part.type === 'mention' && part.text === '@图片1' && part.matched));
});

test('@role mention writes to manual_mention_ids and role_ids', () => {
  const form = { prompt: '@女', image_asset_ids: [], audio_asset_ids: [], role_ids: [], manual_mention_ids: [] };
  const result = applyMentionSelection({
    form,
    item: { label: '女主', type: 'role', roleId: 'role-hero' },
    atQuery: { start: 0, query: '女' },
  });
  assert.deepEqual(result.role_ids, ['role-hero']);
  assert.deepEqual(result.manual_mention_ids, ['role-hero']);
});

test('@nonexistent role is unmatched and does not guess', () => {
  const items = buildMentionItems({ roles: [role], assetById: assets });
  const mentions = collectPromptMentions('@不存在角色 测试', items);
  assert.equal(mentions[0].matched, false);
  assert.equal(mentions[0].type, 'unknown');
});

test('@image mention binds image asset id', () => {
  const form = { prompt: '@', image_asset_ids: [], audio_asset_ids: [], role_ids: [], manual_mention_ids: [] };
  const result = applyMentionSelection({
    form,
    item: { label: '女主日常服', type: 'image', assetId: 'img-1' },
    atQuery: { start: 0, query: '' },
  });
  assert.deepEqual(result.image_asset_ids, ['img-1']);
});

test('@audio mention binds audio asset id', () => {
  const form = { prompt: '@', image_asset_ids: [], audio_asset_ids: [], role_ids: [], manual_mention_ids: [] };
  const result = applyMentionSelection({
    form,
    item: { label: '女主温柔音色', type: 'audio', assetId: 'aud-1' },
    atQuery: { start: 0, query: '' },
  });
  assert.deepEqual(result.audio_asset_ids, ['aud-1']);
});

test('duplicate @mentions do not produce duplicate asset ids', () => {
  const form = { prompt: '@', image_asset_ids: ['img-1'], audio_asset_ids: [], role_ids: [], manual_mention_ids: [] };
  const result = applyMentionSelection({
    form,
    item: { label: '女主日常服', type: 'image', assetId: 'img-1' },
    atQuery: { start: 0, query: '' },
  });
  assert.deepEqual(result.image_asset_ids, ['img-1']);
});

test('mixed @role, @image, @audio mentions write to respective fields', () => {
  const form = { prompt: '@', image_asset_ids: [], audio_asset_ids: [], role_ids: [], manual_mention_ids: [] };
  const withRole = applyMentionSelection({
    form,
    item: { label: '女主', type: 'role', roleId: 'role-hero' },
    atQuery: { start: 0, query: '' },
  });
  const withImage = applyMentionSelection({
    form: withRole,
    item: { label: '女主日常服', type: 'image', assetId: 'img-1' },
    atQuery: { start: withRole.prompt.length, query: '' },
  });
  const withAudio = applyMentionSelection({
    form: withImage,
    item: { label: '女主温柔音色', type: 'audio', assetId: 'aud-1' },
    atQuery: { start: withImage.prompt.length, query: '' },
  });
  assert.deepEqual(withAudio.role_ids, ['role-hero']);
  assert.deepEqual(withAudio.image_asset_ids, ['img-1']);
  assert.deepEqual(withAudio.audio_asset_ids, ['aud-1']);
});

test('@image N only matches existing temp images', () => {
  const items = buildMentionItems({ tempImagePaths: ['/tmp/a.png', '/tmp/b.png'], tempImageAssetIds: ['tmp-1', 'tmp-2'] });
  const m1 = collectPromptMentions('@图片1', items);
  assert.equal(m1[0].matched, true);
  const m3 = collectPromptMentions('@图片3', items);
  assert.equal(m3[0].matched, false);
});

test('matched @ text is marked matched, unmatched is marked unmatched', () => {
  const items = buildMentionItems({ roles: [role], assetById: assets });
  const mentions = collectPromptMentions('@女主 @不存在 测试', items);
  assert.equal(mentions[0].matched, true);
  assert.equal(mentions[1].matched, false);
  const highlights = highlightPromptMentions('@女主 @不存在 测试', items);
  const matched = highlights.find((p) => p.type === 'mention' && p.matched === true);
  const unmatched = highlights.find((p) => p.type === 'mention' && p.matched === false);
  assert.ok(matched);
  assert.ok(unmatched);
});

test('@mention dropdown filters by query', () => {
  const items = buildMentionItems({ roles: [role], assetById: assets, tempImagePaths: ['/tmp/s.png'], tempImageAssetIds: ['tmp-s'] });
  const filtered = items.filter((item) => item.label.includes('女主'));
  assert.ok(filtered.length >= 1);
  assert.ok(filtered.every((item) => item.label.includes('女主')));
  const storyboard = items.filter((item) => item.type === 'temp_image');
  assert.equal(storyboard.length, 1);
  assert.equal(storyboard[0].label, '图片1');
});

test('removing temp image recalculates temp image numbering', () => {
  const items3 = buildMentionItems({ tempImagePaths: ['/tmp/a.png', '/tmp/b.png', '/tmp/c.png'], tempImageAssetIds: ['t1', 't2', 't3'] });
  assert.equal(items3.filter((i) => i.type === 'temp_image').length, 3);
  assert.equal(items3.find((i) => i.assetId === 't1').label, '图片1');
  assert.equal(items3.find((i) => i.assetId === 't2').label, '图片2');
  const afterRemove = buildMentionItems({ tempImagePaths: ['/tmp/b.png', '/tmp/c.png'], tempImageAssetIds: ['t2', 't3'] });
  assert.equal(afterRemove.find((i) => i.assetId === 't2').label, '图片1');
  assert.equal(afterRemove.find((i) => i.assetId === 't3').label, '图片2');
});

test('pasting clipboard image inserts @image N', () => {
  const items = buildMentionItems({ tempImagePaths: ['/tmp/clip.png'], tempImageAssetIds: ['clip-1'] });
  const form = { prompt: '', image_asset_ids: [], audio_asset_ids: [], role_ids: [], manual_mention_ids: [] };
  const result = applyMentionSelection({
    form,
    item: { label: '图片1', type: 'temp_image', assetId: 'clip-1' },
    atQuery: { start: 0, query: '' },
  });
  assert.ok(result.prompt.includes('@图片1'));
  assert.deepEqual(result.image_asset_ids, ['clip-1']);
});

test('temp_image mentions can use unique random labels without depending on ordinal names', () => {
  const items = [
    ...buildMentionItems({ tempImagePaths: ['/tmp/existing.png'], tempImageAssetIds: ['old-2'] }),
    { key: 'temp:new-6d', label: '图片834271', type: 'temp_image', assetId: 'new-6d', storedPath: '/tmp/new.png' },
  ];

  const mentions = collectPromptMentions('@图片834271', items);
  assert.deepEqual(mentions, [{ text: '@图片834271', name: '图片834271', matched: true, type: 'temp_image' }]);

  const form = { prompt: '', image_asset_ids: [], audio_asset_ids: [], role_ids: [], manual_mention_ids: [] };
  const result = applyMentionSelection({
    form,
    item: { label: '图片834271', type: 'temp_image', assetId: 'new-6d' },
    atQuery: { start: 0, query: '' },
  });

  assert.ok(result.prompt.includes('@图片834271'));
  assert.deepEqual(result.image_asset_ids, ['new-6d']);
});

test('historical clipboard temp images get unique mention labels instead of repeated 粘贴图片', () => {
  const recentA = new Date(Date.now() - 2 * 60 * 60 * 1000).toISOString();
  const recentB = new Date(Date.now() - 1 * 60 * 60 * 1000).toISOString();
  const items = buildMentionItems({
    tempImagePaths: ['/tmp/current.png'],
    tempImageAssetIds: ['current-1'],
    assetById: new Map([
      ['hist-a', { id: 'hist-a', kind: 'image', name: '粘贴图片', stored_path: '/tmp/hist-a.png', tags: ['temp_image'], created_at: recentA }],
      ['hist-b', { id: 'hist-b', kind: 'image', name: '粘贴图片', stored_path: '/tmp/hist-b.png', tags: ['temp_image'], created_at: recentB }],
    ]),
  });

  const tempItems = items.filter((item) => item.type === 'temp_image');
  const labels = tempItems.map((item) => item.label);

  assert.equal(labels[0], '图片1');
  assert.equal(new Set(labels).size, labels.length);
  assert.ok(labels.includes('图片hista'));
  assert.ok(labels.includes('图片histb'));
  assert.ok(!labels.includes('粘贴图片'));
});

test('historical clipboard temp image mention matches only its unique generated label', () => {
  const recentA = new Date(Date.now() - 2 * 60 * 60 * 1000).toISOString();
  const recentB = new Date(Date.now() - 1 * 60 * 60 * 1000).toISOString();
  const items = buildMentionItems({
    assetById: new Map([
      ['hist-a', { id: 'hist-a', kind: 'image', name: '粘贴图片', stored_path: '/tmp/hist-a.png', tags: ['temp_image'], created_at: recentA }],
      ['hist-b', { id: 'hist-b', kind: 'image', name: '粘贴图片', stored_path: '/tmp/hist-b.png', tags: ['temp_image'], created_at: recentB }],
    ]),
  });

  assert.deepEqual(collectPromptMentions('@图片hista', items), [
    { text: '@图片hista', name: '图片hista', matched: true, type: 'temp_image' },
  ]);
  assert.deepEqual(collectPromptMentions('@粘贴图片', items), [
    { text: '@粘贴图片', name: '粘贴图片', matched: false, type: 'unknown' },
  ]);
});
