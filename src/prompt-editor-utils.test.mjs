import assert from 'node:assert/strict';
import { test } from 'node:test';
import {
  tiptapDocToPromptText,
  extractMentionRefsFromTiptapDoc,
  applyMentionRefsToTaskForm,
  promptTextToTiptapDoc,
  getPromptTextFromTiptapEditor,
  shouldBlockPromptTextInput,
  getAllowedPromptPasteText,
  shouldSyncExternalPromptValue,
} from './prompt-editor-utils.js';

// ── fixtures ──

const mentionItems = [
  { key: 'role:hero', label: '女主', type: 'role', roleId: 'role-hero', assetId: '' },
  { key: 'img:img-1', label: '女主日常服', type: 'image', roleId: 'role-hero', assetId: 'img-1' },
  { key: 'aud:aud-1', label: '女主温柔音色', type: 'audio', roleId: 'role-hero', assetId: 'aud-1' },
  { key: 'temp:tmp-1', label: '分镜图1', type: 'temp_image', roleId: '', assetId: 'tmp-1' },
];

function makeDocWithMentions(mentions) {
  const content = mentions.map((m) => ({
    type: 'mention',
    attrs: { id: m.key, label: m.label, type: m.type, roleId: m.roleId || '', assetId: m.assetId || '' },
  }));
  return { type: 'doc', content: [{ type: 'paragraph', content }] };
}

// ── tiptapDocToPromptText ──

test('tiptapDocToPromptText converts mention nodes to @label text', () => {
  const doc = makeDocWithMentions([
    { key: 'role:hero', label: '女主', type: 'role' },
  ]);
  assert.equal(tiptapDocToPromptText(doc), '@女主');
});

test('tiptapDocToPromptText mixes text and mention nodes', () => {
  const doc = {
    type: 'doc',
    content: [{
      type: 'paragraph',
      content: [
        { type: 'text', text: '慢慢推近 ' },
        { type: 'mention', attrs: { id: 'role:hero', label: '女主', type: 'role', roleId: 'role-hero', assetId: '' } },
        { type: 'text', text: ' 在海边漫步' },
      ],
    }],
  };
  assert.equal(tiptapDocToPromptText(doc), '慢慢推近 @女主 在海边漫步');
});

test('tiptapDocToPromptText handles multiple paragraphs as newlines', () => {
  const doc = {
    type: 'doc',
    content: [
      { type: 'paragraph', content: [{ type: 'text', text: '第一行' }] },
      { type: 'paragraph', content: [{ type: 'text', text: '第二行' }] },
    ],
  };
  assert.equal(tiptapDocToPromptText(doc), '第一行\n第二行');
});

test('tiptapDocToPromptText returns empty string for null doc', () => {
  assert.equal(tiptapDocToPromptText(null), '');
  assert.equal(tiptapDocToPromptText({}), '');
});

// ── extractMentionRefsFromTiptapDoc ──

test('extractMentionRefsFromTiptapDoc groups role, image, audio, temp_image refs', () => {
  const doc = makeDocWithMentions([
    { key: 'role:hero', label: '女主', type: 'role', roleId: 'role-hero' },
    { key: 'img:img-1', label: '女主日常服', type: 'image', assetId: 'img-1' },
    { key: 'aud:aud-1', label: '女主温柔音色', type: 'audio', assetId: 'aud-1' },
    { key: 'temp:tmp-1', label: '分镜图1', type: 'temp_image', assetId: 'tmp-1' },
  ]);
  const refs = extractMentionRefsFromTiptapDoc(doc);
  assert.deepEqual(refs.roleIds, ['role-hero']);
  assert.deepEqual(refs.imageAssetIds, ['img-1', 'tmp-1']);
  assert.deepEqual(refs.audioAssetIds, ['aud-1']);
  assert.deepEqual(refs.tempImageAssetIds, ['tmp-1']);
});

test('extractMentionRefsFromTiptapDoc deduplicates ids', () => {
  const doc = makeDocWithMentions([
    { key: 'role:hero', label: '女主', type: 'role', roleId: 'role-hero' },
    { key: 'role:hero2', label: '女主', type: 'role', roleId: 'role-hero' },
  ]);
  const refs = extractMentionRefsFromTiptapDoc(doc);
  assert.deepEqual(refs.roleIds, ['role-hero']);
});

test('extractMentionRefsFromTiptapDoc returns empty arrays for null doc', () => {
  const refs = extractMentionRefsFromTiptapDoc(null);
  assert.deepEqual(refs.roleIds, []);
  assert.deepEqual(refs.imageAssetIds, []);
  assert.deepEqual(refs.audioAssetIds, []);
});

// ── applyMentionRefsToTaskForm ──

test('applyMentionRefsToTaskForm writes role_ids and manual_mention_ids from role refs', () => {
  const form = { prompt: '', role_ids: [], manual_mention_ids: [], image_asset_ids: [], audio_asset_ids: [] };
  const refs = { roleIds: ['role-hero'], imageAssetIds: [], audioAssetIds: [], tempImageAssetIds: [] };
  const result = applyMentionRefsToTaskForm(form, refs);
  assert.deepEqual(result.role_ids, ['role-hero']);
  assert.deepEqual(result.manual_mention_ids, ['role-hero']);
});

test('applyMentionRefsToTaskForm preserves roles added via role picker (non-mention)', () => {
  const form = {
    prompt: '@女主 hello',
    role_ids: ['role-hero', 'role-picker-only'],
    manual_mention_ids: ['role-hero'],
    image_asset_ids: [],
    audio_asset_ids: [],
  };
  const refs = { roleIds: ['role-hero'], imageAssetIds: [], audioAssetIds: [], tempImageAssetIds: [] };
  const result = applyMentionRefsToTaskForm(form, refs);
  assert.ok(result.role_ids.includes('role-picker-only'), 'role picker selection must be preserved');
  assert.ok(result.role_ids.includes('role-hero'), 'mention role must be kept');
  assert.deepEqual(result.manual_mention_ids, ['role-hero']);
});

test('applyMentionRefsToTaskForm writes image_asset_ids including temp_image_asset_ids', () => {
  const form = {
    prompt: '',
    role_ids: [],
    manual_mention_ids: [],
    image_asset_ids: [],
    audio_asset_ids: [],
    temp_image_asset_ids: ['tmp-1'],
  };
  const refs = { roleIds: [], imageAssetIds: ['img-1'], audioAssetIds: [], tempImageAssetIds: ['tmp-1'] };
  const result = applyMentionRefsToTaskForm(form, refs);
  assert.deepEqual(result.image_asset_ids, ['img-1', 'tmp-1']);
});

test('applyMentionRefsToTaskForm removes stale mention-derived role when its mention is deleted', () => {
  const form = {
    prompt: '',
    role_ids: ['role-hero', 'role-old', 'role-picker-only'],
    manual_mention_ids: ['role-hero', 'role-old'],
    image_asset_ids: ['img-1', 'img-deleted'],
    audio_asset_ids: ['aud-1', 'aud-deleted'],
  };
  const refs = { roleIds: ['role-hero'], imageAssetIds: [], audioAssetIds: ['aud-1'], tempImageAssetIds: [] };
  const result = applyMentionRefsToTaskForm(form, refs);
  assert.ok(!result.role_ids.includes('role-old'), 'deleted mention role must be removed');
  assert.ok(result.role_ids.includes('role-hero'), 'remaining mention role must stay');
  assert.ok(result.role_ids.includes('role-picker-only'), 'role picker selection must survive');
  assert.deepEqual(result.manual_mention_ids, ['role-hero']);
  assert.deepEqual(result.audio_asset_ids, ['aud-1']);
});

test('applyMentionRefsToTaskForm preserves other form fields', () => {
  const form = {
    prompt: 'hello',
    role_ids: [],
    manual_mention_ids: [],
    image_asset_ids: [],
    audio_asset_ids: [],
    params: { model_version: 'seedance2.0' },
    temp_image_paths: ['/tmp/a.png'],
  };
  const refs = { roleIds: [], imageAssetIds: [], audioAssetIds: [], tempImageAssetIds: [] };
  const result = applyMentionRefsToTaskForm(form, refs);
  assert.equal(result.prompt, 'hello');
  assert.deepEqual(result.params, { model_version: 'seedance2.0' });
  assert.deepEqual(result.temp_image_paths, ['/tmp/a.png']);
});

// ── promptTextToTiptapDoc ──

test('promptTextToTiptapDoc converts @label mentions to mention nodes', () => {
  const doc = promptTextToTiptapDoc('@女主 在海边', mentionItems);
  const para = doc.content[0];
  assert.equal(para.type, 'paragraph');
  assert.equal(para.content[0].type, 'mention');
  assert.equal(para.content[0].attrs.label, '女主');
  assert.equal(para.content[1].type, 'text');
  assert.equal(para.content[1].text, ' 在海边');
});

test('promptTextToTiptapDoc converts matching preset @materials to highlightable mention nodes', () => {
  const presetItems = [
    { key: 'temp:tmp-3', label: '分镜图3', type: 'temp_image', assetId: 'tmp-3' },
    { key: 'img:chef', label: '女主厨师服', type: 'image', assetId: 'img-chef' },
    { key: 'aud:hero', label: '女主女主人声音', type: 'audio', assetId: 'aud-hero' },
  ];
  const doc = promptTextToTiptapDoc('根据分镜图 @分镜图3 女主是 @女主厨师服 声音： @女主女主人声音', presetItems);
  const mentions = doc.content[0].content.filter((node) => node.type === 'mention');

  assert.deepEqual(mentions.map((node) => node.attrs.label), ['分镜图3', '女主厨师服', '女主女主人声音']);
  assert.deepEqual(mentions.map((node) => node.attrs.type), ['temp_image', 'image', 'audio']);
});

test('promptTextToTiptapDoc leaves unmatched @text as plain text', () => {
  const doc = promptTextToTiptapDoc('@不存在 测试', mentionItems);
  const para = doc.content[0];
  assert.equal(para.content[0].type, 'text');
  assert.equal(para.content[0].text, '@不存在');
  assert.equal(para.content[1].type, 'text');
  assert.equal(para.content[1].text, ' 测试');
});

test('promptTextToTiptapDoc splits newlines into separate paragraphs', () => {
  const doc = promptTextToTiptapDoc('第一行\n第二行', mentionItems);
  assert.equal(doc.content.length, 2);
  assert.equal(doc.content[0].content[0].text, '第一行');
  assert.equal(doc.content[1].content[0].text, '第二行');
});

test('promptTextToTiptapDoc returns empty paragraph for empty prompt', () => {
  const doc = promptTextToTiptapDoc('', mentionItems);
  assert.equal(doc.content.length, 1);
  assert.equal(doc.content[0].type, 'paragraph');
  assert.equal(doc.content[0].content, undefined);
});

// ── round-trip ──

test('round-trip: promptText → doc → promptText preserves @label text', () => {
  const original = '@女主 在海边漫步 @分镜图1 慢慢推近';
  const doc = promptTextToTiptapDoc(original, mentionItems);
  const restored = tiptapDocToPromptText(doc);
  assert.equal(restored, original);
});

test('getPromptTextFromTiptapEditor uses JSON serialization to avoid expanding newlines', () => {
  const doc = promptTextToTiptapDoc('第一行\n第二行', mentionItems);
  const editor = {
    getJSON: () => doc,
    getText: () => '第一行\n\n第二行',
  };

  assert.equal(getPromptTextFromTiptapEditor(editor), '第一行\n第二行');
});

test('round-trip: deleting a mention from doc removes its ref', () => {
  const original = '@女主 @女主日常服 在海边';
  const doc = promptTextToTiptapDoc(original, mentionItems);
  const refsBefore = extractMentionRefsFromTiptapDoc(doc);
  assert.deepEqual(refsBefore.roleIds, ['role-hero']);
  assert.deepEqual(refsBefore.imageAssetIds, ['img-1']);

  // Simulate deleting the image mention from doc
  const para = doc.content[0];
  para.content = para.content.filter((n) => !(n.type === 'mention' && n.attrs.type === 'image'));

  const refsAfter = extractMentionRefsFromTiptapDoc(doc);
  assert.deepEqual(refsAfter.roleIds, ['role-hero']);
  assert.deepEqual(refsAfter.imageAssetIds, []);
});

// ── maxLength input guard ──

test('shouldBlockPromptTextInput uses current editor length after mention deletion', () => {
  assert.equal(shouldBlockPromptTextInput({
    maxLength: 10,
    currentLength: 8,
    from: 8,
    to: 8,
    text: '好',
  }), false);
});

test('shouldBlockPromptTextInput blocks pure insertion beyond maxLength', () => {
  assert.equal(shouldBlockPromptTextInput({
    maxLength: 10,
    currentLength: 10,
    from: 10,
    to: 10,
    text: '好',
  }), true);
});

test('shouldBlockPromptTextInput allows replacement even when old content is over maxLength', () => {
  assert.equal(shouldBlockPromptTextInput({
    maxLength: 10,
    currentLength: 15,
    from: 3,
    to: 8,
    text: '短',
  }), false);
});

test('getAllowedPromptPasteText truncates pasted text to remaining maxLength', () => {
  assert.equal(getAllowedPromptPasteText({
    maxLength: 10,
    currentLength: 8,
    selectedLength: 0,
    text: 'abcdef',
  }), 'ab');
});

test('getAllowedPromptPasteText allows replacement using selected text capacity', () => {
  assert.equal(getAllowedPromptPasteText({
    maxLength: 10,
    currentLength: 10,
    selectedLength: 4,
    text: 'abcdef',
  }), 'abcd');
});

test('shouldSyncExternalPromptValue detects preset text pushed from outside editor', () => {
  assert.equal(shouldSyncExternalPromptValue({
    editorText: '',
    externalValue: '根据分镜图 @分镜图1',
    lastExternalValue: '',
    isInternalUpdate: false,
  }), true);
});

test('shouldSyncExternalPromptValue ignores unchanged or internal updates', () => {
  assert.equal(shouldSyncExternalPromptValue({
    editorText: '根据分镜图 @分镜图1',
    externalValue: '根据分镜图 @分镜图1',
    lastExternalValue: '根据分镜图 @分镜图1',
    isInternalUpdate: false,
  }), false);
  assert.equal(shouldSyncExternalPromptValue({
    editorText: '',
    externalValue: '内部输入',
    lastExternalValue: '',
    isInternalUpdate: true,
  }), false);
});
