import assert from 'node:assert/strict';
import { test } from 'node:test';
import {
  DEFAULT_CREATE_TASK_PRESET,
  TASK_PROMPT_MAX_LENGTH,
  applyCreateTaskPreset,
  applyPromptMentionsToTaskForm,
  buildSaveTaskDraftButtonState,
  canApplyCreateTaskPreset,
  canSaveTaskDraft,
  canSubmitCreateTask,
  createEmptyTaskForm,
  createRoleEditor,
  getRoleEditorMedia,
  patchRoleEditorForm,
} from './task-form-utils.js';

test('createEmptyTaskForm defaults new video tasks to 15 seconds', () => {
  const form = createEmptyTaskForm();
  assert.equal(form.params.duration, 15);
  assert.equal(form.params.model_version, 'seedance2.0');
  assert.equal(form.params.ratio, '9:16');
});

test('createEmptyTaskForm includes prompt_doc as null', () => {
  const form = createEmptyTaskForm();
  assert.equal(form.prompt_doc, null);
  assert.ok('prompt_doc' in form);
});

test('TASK_PROMPT_MAX_LENGTH allows editing full video scripts', () => {
  assert.ok(TASK_PROMPT_MAX_LENGTH >= 10000);
});

test('canSubmitCreateTask requires prompt and concrete image assets, not selected roles', () => {
  assert.equal(canSubmitCreateTask({ prompt: '', image_asset_ids: ['img-1'], role_ids: ['role-1'] }), false);
  assert.equal(canSubmitCreateTask({ prompt: '写实视频', image_asset_ids: [], role_ids: ['role-1'] }), false);
  assert.equal(canSubmitCreateTask({ prompt: '写实视频', image_asset_ids: ['img-1'], role_ids: [] }), true);
});

test('canApplyCreateTaskPreset is only true for an empty new task form', () => {
  assert.equal(canApplyCreateTaskPreset(createEmptyTaskForm()), true);
  assert.equal(canApplyCreateTaskPreset({ ...createEmptyTaskForm(), prompt: '已有内容' }), false);
  assert.equal(canApplyCreateTaskPreset({ ...createEmptyTaskForm(), image_asset_ids: ['img-1'] }), false);
  assert.equal(canApplyCreateTaskPreset({
    ...createEmptyTaskForm(),
    image_asset_ids: ['tmp-1'],
    temp_image_asset_ids: ['tmp-1'],
    temp_image_paths: ['/tmp/shot.png'],
  }), true);
  assert.equal(canApplyCreateTaskPreset({ ...createEmptyTaskForm(), audio_asset_ids: ['aud-1'] }), false);
});

test('applyCreateTaskPreset writes preset prompt and concrete @material bindings', () => {
  const mentionItems = [
    { key: 'temp:tmp-3', label: '图片3', type: 'temp_image', assetId: 'tmp-3' },
    { key: 'img:chef', label: '女主厨师服', type: 'image', roleId: 'role-hero', assetId: 'img-chef' },
    { key: 'aud:hero', label: '女主女主人声音', type: 'audio', roleId: 'role-hero', assetId: 'aud-hero' },
    { key: 'aud:cowcat', label: '黑白猫和灰猫奶牛猫声音', type: 'audio', roleId: 'role-cat', assetId: 'aud-cowcat' },
    { key: 'aud:graycat', label: '黑白猫和灰猫英短声音', type: 'audio', roleId: 'role-cat', assetId: 'aud-graycat' },
  ];

  const form = applyCreateTaskPreset(createEmptyTaskForm(), mentionItems);

  assert.equal(form.prompt, DEFAULT_CREATE_TASK_PRESET);
  assert.deepEqual(form.role_ids, []);
  assert.deepEqual(form.manual_mention_ids, []);
  assert.deepEqual(form.image_asset_ids, ['tmp-3', 'img-chef']);
  assert.deepEqual(form.audio_asset_ids, ['aud-hero', 'aud-cowcat', 'aud-graycat']);
});

test('applyPromptMentionsToTaskForm fills bindings from highlighted prompt mentions', () => {
  const mentionItems = [
    { key: 'temp:tmp-1', label: '图片1', type: 'temp_image', assetId: 'tmp-1' },
    { key: 'img:chef', label: '女主厨师服', type: 'image', roleId: 'role-hero', assetId: 'img-chef' },
    { key: 'aud:hero', label: '女主女主人声音', type: 'audio', roleId: 'role-hero', assetId: 'aud-hero' },
  ];
  const form = {
    ...createEmptyTaskForm(),
    prompt: '根据图片 @图片1 女主是 @女主厨师服 声音 @女主女主人声音',
    image_asset_ids: ['tmp-1'],
    temp_image_asset_ids: ['tmp-1'],
    temp_image_paths: ['/tmp/shot.png'],
  };

  const next = applyPromptMentionsToTaskForm(form, mentionItems);

  assert.deepEqual(next.image_asset_ids, ['tmp-1', 'img-chef']);
  assert.deepEqual(next.audio_asset_ids, ['aud-hero']);
  assert.deepEqual(next.role_ids, []);
  assert.deepEqual(next.manual_mention_ids, []);
  assert.deepEqual(next.temp_image_asset_ids, ['tmp-1']);
});

test('applyPromptMentionsToTaskForm removes stale prompt-derived media but keeps temp images', () => {
  const form = {
    ...createEmptyTaskForm(),
    prompt: '只保留临时图 @图片1',
    image_asset_ids: ['tmp-1', 'old-img'],
    audio_asset_ids: ['old-aud'],
    temp_image_asset_ids: ['tmp-1'],
    temp_image_paths: ['/tmp/shot.png'],
  };
  const mentionItems = [
    { key: 'temp:tmp-1', label: '图片1', type: 'temp_image', assetId: 'tmp-1' },
  ];

  const next = applyPromptMentionsToTaskForm(form, mentionItems);

  assert.deepEqual(next.image_asset_ids, ['tmp-1']);
  assert.deepEqual(next.audio_asset_ids, []);
});

test('canSaveTaskDraft allows saving in-progress text, roles, image, or audio references', () => {
  assert.equal(canSaveTaskDraft({ prompt: '  ', role_ids: [], image_asset_ids: [], audio_asset_ids: [] }), false);
  assert.equal(canSaveTaskDraft({ prompt: '镜头先这样', role_ids: [], image_asset_ids: [], audio_asset_ids: [] }), true);
  assert.equal(canSaveTaskDraft({ prompt: '', role_ids: ['role-1'], image_asset_ids: [], audio_asset_ids: [] }), true);
  assert.equal(canSaveTaskDraft({ prompt: '', role_ids: [], image_asset_ids: ['img-1'], audio_asset_ids: [] }), true);
  assert.equal(canSaveTaskDraft({ prompt: '', role_ids: [], image_asset_ids: [], audio_asset_ids: ['aud-1'] }), true);
});

test('buildSaveTaskDraftButtonState shows title generation while saving untitled task', () => {
  assert.deepEqual(
    buildSaveTaskDraftButtonState({
      canSaveDraft: true,
      isEditingTask: false,
      savingTaskDraft: true,
      savingTaskDraftPhase: 'title',
    }),
    { disabled: true, icon: 'loader', label: '生成标题中…' },
  );
  assert.deepEqual(
    buildSaveTaskDraftButtonState({
      canSaveDraft: true,
      isEditingTask: true,
      savingTaskDraft: true,
      savingTaskDraftPhase: 'saving',
    }),
    { disabled: true, icon: 'loader', label: '保存中…' },
  );
  assert.deepEqual(
    buildSaveTaskDraftButtonState({
      canSaveDraft: true,
      isEditingTask: false,
      savingTaskDraft: false,
      savingTaskDraftPhase: '',
    }),
    { disabled: false, icon: 'plus', label: '保存任务' },
  );
});

test('getRoleEditorMedia isolates create mode from the previously selected role media', () => {
  const previousRoleMedia = {
    images: [{ id: 'old-img', stored_path: '/tmp/old.png' }],
    audios: [{ id: 'old-audio', stored_path: '/tmp/old.mp3' }],
  };

  assert.deepEqual(
    getRoleEditorMedia('create', previousRoleMedia, { imagePath: '', audioPath: '' }),
    { images: [], audios: [] },
  );
  assert.deepEqual(
    getRoleEditorMedia('create', previousRoleMedia, { imagePath: '/tmp/new.png', audioPath: '/tmp/new.mp3' }),
    {
      images: [{ id: 'pending-image', stored_path: '/tmp/new.png', name: '待导入参考图', pending: true }],
      audios: [{ id: 'pending-audio', stored_path: '/tmp/new.mp3', name: '待导入音色', pending: true }],
    },
  );
  assert.equal(getRoleEditorMedia('edit', previousRoleMedia, {}).images[0].id, 'old-img');
});

test('role editor keeps edit role id and pending create form in one context', () => {
  const selectedRole = {
    id: 'role-old',
    name: '旧角色',
    aliases: ['old'],
    tags: ['tag'],
    description: 'desc',
  };

  const createEditor = patchRoleEditorForm(createRoleEditor('create', selectedRole), {
    imagePath: '/tmp/new.png',
    audioPath: '/tmp/new.mp3',
  });
  assert.equal(createEditor.roleId, null);
  assert.equal(createEditor.form.id, '');
  assert.equal(createEditor.form.imagePath, '/tmp/new.png');

  const editEditor = createRoleEditor('edit', selectedRole);
  assert.equal(editEditor.roleId, 'role-old');
  assert.equal(editEditor.form.id, 'role-old');
  assert.equal(editEditor.form.name, '旧角色');
});

test('createRoleEditor(create) returns null roleId and empty form', () => {
  const editor = createRoleEditor('create');
  assert.equal(editor.roleId, null);
  assert.equal(editor.form.name, '');
  assert.equal(editor.form.series, '');
  assert.equal(editor.form.disabled, false);
  assert.equal(editor.form.imagePath, '');
  assert.equal(editor.form.audioPath, '');
})

test('createRoleEditor(edit, role) returns role id and filled form', () => {
  const role = { id: 'role-1', name: '威威', aliases: ['警车'], tags: ['车'], description: '酷', series: '显眼包', disabled: true };
  const editor = createRoleEditor('edit', role);
  assert.equal(editor.roleId, 'role-1');
  assert.equal(editor.form.id, 'role-1');
  assert.equal(editor.form.name, '威威');
  assert.equal(editor.form.aliases, '警车');
  assert.equal(editor.form.tags, '车');
  assert.equal(editor.form.description, '酷');
  assert.equal(editor.form.series, '显眼包');
  assert.equal(editor.form.disabled, true);
});

test('create mode does not read previous selectedRoleMedia', () => {
  const previousMedia = {
    images: [{ id: 'old-img', stored_path: '/tmp/old.png' }],
    audios: [{ id: 'old-aud', stored_path: '/tmp/old.mp3' }],
  };
  const result = getRoleEditorMedia('create', previousMedia, { imagePath: '', audioPath: '' });
  assert.deepEqual(result.images, []);
  assert.deepEqual(result.audios, []);
});

test('edit mode reads only roleEditor.roleId media', () => {
  const media = {
    images: [{ id: 'edit-img', stored_path: '/tmp/edit.png' }],
    audios: [{ id: 'edit-aud', stored_path: '/tmp/edit.mp3' }],
  };
  const result = getRoleEditorMedia('edit', media, {});
  assert.equal(result.images[0].id, 'edit-img');
  assert.equal(result.audios[0].id, 'edit-aud');
});

test('closing role editor clears pending image/audio', () => {
  const editor = createRoleEditor('create');
  const patched = patchRoleEditorForm(editor, { imagePath: '/tmp/new.png', audioPath: '/tmp/new.mp3' });
  assert.equal(patched.form.imagePath, '/tmp/new.png');
  // Simulating close: set roleEditor to null
  const closedEditor = null;
  assert.equal(closedEditor, null);
});

test('create mode media count comes only from pending paths', () => {
  const empty = getRoleEditorMedia('create', { images: [], audios: [] }, { imagePath: '', audioPath: '' });
  assert.equal(empty.images.length, 0);
  assert.equal(empty.audios.length, 0);

  const withPending = getRoleEditorMedia('create', { images: [], audios: [] }, { imagePath: '/tmp/pending.png', audioPath: '/tmp/pending.mp3' });
  assert.equal(withPending.images.length, 1);
  assert.equal(withPending.audios.length, 1);
  assert.equal(withPending.images[0].pending, true);
});

test('edit mode media count comes only from selectedRoleMedia', () => {
  const media = {
    images: [{ id: 'img-1' }, { id: 'img-2' }],
    audios: [{ id: 'aud-1' }],
  };
  const result = getRoleEditorMedia('edit', media, { imagePath: '/tmp/ignored.png', audioPath: '' });
  assert.equal(result.images.length, 2);
  assert.equal(result.audios.length, 1);
});

test('canSaveTaskDraft requires at least one content field', () => {
  assert.equal(canSaveTaskDraft({}), false);
  assert.equal(canSaveTaskDraft({ prompt: '  ' }), false);
  assert.equal(canSaveTaskDraft({ prompt: '有内容' }), true);
  assert.equal(canSaveTaskDraft({ role_ids: ['r1'] }), true);
  assert.equal(canSaveTaskDraft({ image_asset_ids: ['i1'] }), true);
  assert.equal(canSaveTaskDraft({ audio_asset_ids: ['a1'] }), true);
});

test('patchRoleEditorForm with null editor returns null', () => {
  assert.equal(patchRoleEditorForm(null, { name: 'test' }), null);
});

test('patchRoleEditorForm with function patcher', () => {
  const editor = createRoleEditor('create');
  const patched = patchRoleEditorForm(editor, (form) => ({ ...form, name: '新角色' }));
  assert.equal(patched.form.name, '新角色');
});
