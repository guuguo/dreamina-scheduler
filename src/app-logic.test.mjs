import assert from 'node:assert/strict';
import { test } from 'node:test';
import {
  getRoleMedia,
  buildQueueStats,
  resolveDropTarget,
  removeTempImageFromForm,
  computeRoleAssetIdsOnSave,
  resolveRemoveMediaTarget,
  buildTaskFormFromTaskForDuplicate,
  buildTaskFormFromTaskForEdit,
  normalizeImageModelSettingsForm,
  patchImageModelConfig,
  mergeSettingsFormOnStateRefresh,
  shouldRefreshStateAfterSchedulerTick,
} from './app-logic.js';

// ─── getRoleMedia ───

test('getRoleMedia derives images and audios from role asset_ids', () => {
  const role = { id: 'r1', asset_ids: ['img-1', 'aud-1', 'img-2'] };
  const assetById = new Map([
    ['img-1', { id: 'img-1', kind: 'image', stored_path: '/tmp/a.png', source_path: '/tmp/a.png' }],
    ['aud-1', { id: 'aud-1', kind: 'audio', stored_path: '/tmp/b.mp3', source_path: '/tmp/b.mp3' }],
    ['img-2', { id: 'img-2', kind: 'image', stored_path: '/tmp/c.png', source_path: '/tmp/c.png' }],
  ]);
  const media = getRoleMedia(role, assetById);
  assert.equal(media.images.length, 2);
  assert.equal(media.audios.length, 1);
  assert.equal(media.all.length, 3);
});

test('getRoleMedia deduplicates same-source assets', () => {
  const role = { id: 'r1', asset_ids: ['img-1', 'img-2'] };
  const assetById = new Map([
    ['img-1', { id: 'img-1', kind: 'image', stored_path: '/tmp/a.png', source_path: '/tmp/shared.png' }],
    ['img-2', { id: 'img-2', kind: 'image', stored_path: '/tmp/b.png', source_path: '/tmp/shared.png' }],
  ]);
  const media = getRoleMedia(role, assetById);
  assert.equal(media.images.length, 1);
});

test('getRoleMedia skips missing asset ids gracefully', () => {
  const role = { id: 'r1', asset_ids: ['img-1', 'missing'] };
  const assetById = new Map([
    ['img-1', { id: 'img-1', kind: 'image', stored_path: '/tmp/a.png', source_path: '/tmp/a.png' }],
  ]);
  const media = getRoleMedia(role, assetById);
  assert.equal(media.images.length, 1);
});

test('getRoleMedia returns empty for null role', () => {
  const media = getRoleMedia(null, new Map());
  assert.equal(media.images.length, 0);
  assert.equal(media.audios.length, 0);
});

test('patchImageModelConfig keeps newly added image model when clearing fields', () => {
  const legacyForm = {
    image_model_config: {
      base_url: 'https://api.example/v1',
      api_key: 'legacy-key',
      model: 'gpt-image-1',
    },
  };
  const added = {
    id: 'image-openai-new',
    name: '图片模型 2',
    base_url: 'https://api.openai.com/v1',
    api_key: '',
    model: 'gpt-image-1',
  };
  const normalized = normalizeImageModelSettingsForm({
    ...legacyForm,
    image_model_configs: [
      ...normalizeImageModelSettingsForm(legacyForm).image_model_configs,
      added,
    ],
    active_image_model_id: added.id,
    image_model_config: added,
  });

  const afterClearingName = patchImageModelConfig(normalized, 1, { name: '' });
  const afterClearingModel = patchImageModelConfig(afterClearingName, 1, { model: '' });

  assert.equal(afterClearingModel.image_model_configs.length, 2);
  assert.equal(afterClearingModel.active_image_model_id, added.id);
  assert.equal(afterClearingModel.image_model_configs[1].id, added.id);
  assert.equal(afterClearingModel.image_model_configs[1].name, '');
  assert.equal(afterClearingModel.image_model_configs[1].model, '');
  assert.equal(afterClearingModel.image_model_config.id, added.id);
});

test('mergeSettingsFormOnStateRefresh preserves dirty settings draft while settings page is open', () => {
  const current = {
    image_model_configs: [
      { id: 'saved', name: '已保存', base_url: 'https://api.example/v1', api_key: '', model: 'gpt-image-1' },
      { id: 'new-local', name: '', base_url: 'https://api.openai.com/v1', api_key: '', model: '' },
    ],
    active_image_model_id: 'new-local',
  };
  const incoming = {
    image_model_configs: [
      { id: 'saved', name: '已保存', base_url: 'https://api.example/v1', api_key: '', model: 'gpt-image-1' },
    ],
    active_image_model_id: 'saved',
  };

  const merged = mergeSettingsFormOnStateRefresh({
    activeView: 'settings',
    settingsDirty: true,
    currentSettingsForm: current,
    incomingSettings: incoming,
    emptySettings: incoming,
  });

  assert.equal(merged.image_model_configs.length, 2);
  assert.equal(merged.active_image_model_id, 'new-local');
});

test('shouldRefreshStateAfterSchedulerTick refreshes only live status pages', () => {
  assert.equal(shouldRefreshStateAfterSchedulerTick({ activeView: 'dashboard' }), true);
  assert.equal(shouldRefreshStateAfterSchedulerTick({ activeView: 'queue' }), true);
  assert.equal(shouldRefreshStateAfterSchedulerTick({ activeView: 'logs' }), true);
  assert.equal(shouldRefreshStateAfterSchedulerTick({ activeView: 'imagegen' }), true);
  assert.equal(shouldRefreshStateAfterSchedulerTick({ activeView: 'roles', roleEditor: null }), true);
  assert.equal(shouldRefreshStateAfterSchedulerTick({ activeView: 'roles', roleEditor: { mode: 'edit' } }), false);
  assert.equal(shouldRefreshStateAfterSchedulerTick({ activeView: 'create' }), false);
  assert.equal(shouldRefreshStateAfterSchedulerTick({ activeView: 'settings' }), false);
});

// ─── resolveDropTarget ─── 历史高频 bug：角色串素材

test('drop in create mode targets create form, not any existing role', () => {
  const target = resolveDropTarget({
    activeView: 'roles',
    roleEditor: { mode: 'create', roleId: null },
    selectedRoleId: 'role-A',
  });
  assert.equal(target.type, 'create');
  assert.equal(target.roleId, undefined);
});

test('drop in edit mode targets roleEditor.roleId, not selectedRoleId', () => {
  const target = resolveDropTarget({
    activeView: 'roles',
    roleEditor: { mode: 'edit', roleId: 'role-B' },
    selectedRoleId: 'role-A',
  });
  assert.equal(target.type, 'edit');
  assert.equal(target.roleId, 'role-B');
});

test('drop in detail mode (no editor) targets selectedRoleId', () => {
  const target = resolveDropTarget({
    activeView: 'roles',
    roleEditor: null,
    selectedRoleId: 'role-A',
  });
  assert.equal(target.type, 'detail');
  assert.equal(target.roleId, 'role-A');
});

test('drop outside roles goes to task form', () => {
  const target = resolveDropTarget({
    activeView: 'create',
    roleEditor: null,
    selectedRoleId: 'role-A',
  });
  assert.equal(target.type, 'task-form');
});

// ─── removeTempImageFromForm ─── 历史高频 bug：临时图删不掉

test('removing temp image also removes from image_asset_ids', () => {
  const form = {
    temp_image_paths: ['/tmp/a.png', '/tmp/b.png'],
    temp_image_asset_ids: ['t1', 't2'],
    image_asset_ids: ['t1', 't2', 'img-perm'],
  };
  const result = removeTempImageFromForm(form, 0);
  assert.deepEqual(result.temp_image_paths, ['/tmp/b.png']);
  assert.deepEqual(result.temp_image_asset_ids, ['t2']);
  assert.deepEqual(result.image_asset_ids, ['t2', 'img-perm']);
});

test('removing last temp image clears all temp fields', () => {
  const form = {
    temp_image_paths: ['/tmp/a.png'],
    temp_image_asset_ids: ['t1'],
    image_asset_ids: ['t1'],
  };
  const result = removeTempImageFromForm(form, 0);
  assert.deepEqual(result.temp_image_paths, []);
  assert.deepEqual(result.temp_image_asset_ids, []);
  assert.deepEqual(result.image_asset_ids, []);
});

test('removing temp image with missing asset id still removes from paths', () => {
  const form = {
    temp_image_paths: ['/tmp/a.png', '/tmp/b.png'],
    temp_image_asset_ids: ['t1'],
    image_asset_ids: ['t1'],
  };
  const result = removeTempImageFromForm(form, 1);
  assert.deepEqual(result.temp_image_paths, ['/tmp/a.png']);
  assert.deepEqual(result.image_asset_ids, ['t1']);
});

test('editing a draft restores persisted temp image assets into temp preview fields', () => {
  const task = {
    prompt: '@图片1 继续编辑',
    image_asset_ids: ['temp-1'],
    audio_asset_ids: [],
    role_ids: [],
    manual_mention_ids: [],
    auto_match_roles: true,
    temp_image_asset_ids: ['temp-1'],
    temp_image_paths: ['/old/path/should-not-win.png'],
    params: { model_version: 'seedance2.0', ratio: '9:16', duration: 5, video_resolution: '720p' },
  };
  const assetById = new Map([
    ['temp-1', { id: 'temp-1', kind: 'image', stored_path: '/cache/temp-1.png' }],
  ]);

  const form = buildTaskFormFromTaskForEdit(task, assetById);

  assert.deepEqual(form.temp_image_asset_ids, ['temp-1']);
  assert.deepEqual(form.temp_image_paths, ['/cache/temp-1.png']);
  assert.deepEqual(form.image_asset_ids, ['temp-1']);
});

test('editing a draft infers temp image assets from @image mention labels', () => {
  const task = {
    prompt: '@图片1 继续编辑',
    image_asset_ids: ['temp-1'],
    audio_asset_ids: [],
    role_ids: [],
    manual_mention_ids: [],
    auto_match_roles: true,
    temp_image_asset_ids: [],
    temp_image_paths: [],
    params: { model_version: 'seedance2.0', ratio: '9:16', duration: 5, video_resolution: '720p' },
  };
  const assetById = new Map([
    ['temp-1', { id: 'temp-1', kind: 'image', stored_path: '/cache/temp-1.png' }],
  ]);

  const form = buildTaskFormFromTaskForEdit(task, assetById);

  assert.deepEqual(form.temp_image_asset_ids, ['temp-1']);
  assert.deepEqual(form.temp_image_paths, ['/cache/temp-1.png']);
});

test('editing an older task without duration falls back to 15 seconds', () => {
  const form = buildTaskFormFromTaskForEdit({
    prompt: '旧草稿',
    params: { model_version: 'seedance2.0', ratio: '9:16', video_resolution: '720p' },
  });

  assert.equal(form.params.duration, 15);
});

test('duplicating a task creates a new unscheduled draft form with copied content', () => {
  const task = {
    prompt: '@图片1 生成视频',
    image_asset_ids: ['temp-1', 'img-1'],
    audio_asset_ids: ['aud-1'],
    role_ids: ['role-1'],
    manual_mention_ids: ['role-1'],
    auto_match_roles: false,
    scheduled_at: '2026-05-01T02:00:00+08:00',
    status: 'succeeded',
    submit_id: 'sub_123',
    result_paths: ['/tmp/result.mp4'],
    execution_records: [{ id: 'rec-1' }],
    temp_image_asset_ids: ['temp-1'],
    temp_image_paths: ['/old/path.png'],
    params: { model_version: 'seedance2.0', ratio: '9:16', duration: 15, video_resolution: '720p' },
  };
  const assetById = new Map([
    ['temp-1', { id: 'temp-1', kind: 'image', stored_path: '/cache/temp-1.png' }],
  ]);

  const form = buildTaskFormFromTaskForDuplicate(task, assetById);

  assert.equal(form.prompt, task.prompt);
  assert.deepEqual(form.image_asset_ids, ['temp-1', 'img-1']);
  assert.deepEqual(form.audio_asset_ids, ['aud-1']);
  assert.deepEqual(form.role_ids, ['role-1']);
  assert.deepEqual(form.manual_mention_ids, ['role-1']);
  assert.equal(form.auto_match_roles, false);
  assert.equal(form.scheduled_at, '');
  assert.deepEqual(form.temp_image_asset_ids, ['temp-1']);
  assert.deepEqual(form.temp_image_paths, ['/cache/temp-1.png']);
  assert.deepEqual(form.params, task.params);
  assert.equal(form.submit_id, undefined);
  assert.equal(form.result_paths, undefined);
  assert.equal(form.execution_records, undefined);
});

test('duplicating a task returns independent array copies', () => {
  const task = {
    prompt: '复制任务',
    image_asset_ids: ['img-1'],
    audio_asset_ids: ['aud-1'],
    role_ids: ['role-1'],
    manual_mention_ids: ['role-1'],
    temp_image_asset_ids: [],
    temp_image_paths: [],
    params: { model_version: 'seedance2.0', ratio: '9:16', duration: 15, video_resolution: '720p' },
  };

  const form = buildTaskFormFromTaskForDuplicate(task, new Map());

  assert.notEqual(form.image_asset_ids, task.image_asset_ids);
  assert.notEqual(form.audio_asset_ids, task.audio_asset_ids);
  assert.notEqual(form.role_ids, task.role_ids);
  assert.notEqual(form.manual_mention_ids, task.manual_mention_ids);
  assert.notEqual(form.params, task.params);
});

// ─── computeRoleAssetIdsOnSave ─── 历史高频 bug：编辑角色丢素材

test('create mode only includes new assets', () => {
  const result = computeRoleAssetIdsOnSave({
    mode: 'create',
    existingAssetIds: ['old-1'],
    newAssetIds: ['new-1', 'new-2'],
  });
  assert.deepEqual(result, ['new-1', 'new-2']);
});

test('edit mode preserves existing and appends new with dedup', () => {
  const result = computeRoleAssetIdsOnSave({
    mode: 'edit',
    existingAssetIds: ['old-1', 'old-2'],
    newAssetIds: ['new-1', 'old-2'],
  });
  assert.deepEqual(result, ['old-1', 'old-2', 'new-1']);
});

test('edit mode with no new assets preserves all existing', () => {
  const result = computeRoleAssetIdsOnSave({
    mode: 'edit',
    existingAssetIds: ['old-1', 'old-2'],
    newAssetIds: [],
  });
  assert.deepEqual(result, ['old-1', 'old-2']);
});

// ─── resolveRemoveMediaTarget ─── 历史高频 bug：从编辑页删除串到详情角色

test('remove media in edit mode uses roleEditor.roleId', () => {
  const target = resolveRemoveMediaTarget({
    roleEditor: { mode: 'edit', roleId: 'role-B' },
    selectedRoleId: 'role-A',
  });
  assert.equal(target, 'role-B');
});

test('remove media in detail mode uses selectedRoleId', () => {
  const target = resolveRemoveMediaTarget({
    roleEditor: null,
    selectedRoleId: 'role-A',
  });
  assert.equal(target, 'role-A');
});

test('remove media with no context returns null', () => {
  const target = resolveRemoveMediaTarget({
    roleEditor: null,
    selectedRoleId: null,
  });
  assert.equal(target, null);
});

test('remove media in create mode falls back to selectedRoleId', () => {
  const target = resolveRemoveMediaTarget({
    roleEditor: { mode: 'create', roleId: null },
    selectedRoleId: 'role-A',
  });
  assert.equal(target, 'role-A');
});

// ─── buildQueueStats ───

test('buildQueueStats counts each status category', () => {
  const tasks = [
    { status: 'queued' },
    { status: 'scheduled' },
    { status: 'retry_wait' },
    { status: 'submitting' },
    { status: 'querying' },
    { status: 'succeeded' },
    { status: 'paused' },
    { status: 'failed' },
  ];
  const stats = buildQueueStats(tasks);
  assert.equal(stats.waiting, 3);
  assert.equal(stats.running, 2);
  assert.equal(stats.done, 1);
  assert.equal(stats.paused, 1);
});
