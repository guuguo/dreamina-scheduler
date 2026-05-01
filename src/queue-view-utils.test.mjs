import { strict as assert } from 'node:assert';
import { test } from 'node:test';
import {
  getStatusTabGroup,
  filterTasks,
  sortTasks,
  paginateTasks,
  formatPaginationLabel,
  deriveTaskFlowSteps,
  deriveTaskProgress,
  getModelOptions,
  deriveQueueStats,
  canDeleteTask,
  getTaskResultItems,
  getTaskHitResources,
  getCommandPreviewPresentation,
} from './queue-view-utils.js';

function makeTask(overrides = {}) {
  return {
    id: 'task-1',
    title: 'test task',
    status: 'queued',
    prompt: 'test prompt',
    submit_id: '',
    params: { model_version: 'seedance2.0', ratio: '9:16' },
    updated_at: '2026-01-01T00:00:00Z',
    scheduled_at: '',
    attempt_count: 0,
    ...overrides,
  };
}

// ── getStatusTabGroup ────────────────────────────────────────────────────────

test('getStatusTabGroup: waiting statuses', () => {
  assert.equal(getStatusTabGroup('queued'), 'waiting');
  assert.equal(getStatusTabGroup('draft'), 'waiting');
  assert.equal(getStatusTabGroup('scheduled'), 'waiting');
});

test('getStatusTabGroup: running statuses', () => {
  assert.equal(getStatusTabGroup('submitting'), 'running');
  assert.equal(getStatusTabGroup('querying'), 'running');
  assert.equal(getStatusTabGroup('submitted'), 'running');
});

test('getStatusTabGroup: retry / done / failed / paused', () => {
  assert.equal(getStatusTabGroup('retry_wait'), 'retry');
  assert.equal(getStatusTabGroup('succeeded'), 'done');
  assert.equal(getStatusTabGroup('failed'), 'failed');
  assert.equal(getStatusTabGroup('failed'), 'failed');
  assert.equal(getStatusTabGroup('paused'), 'paused');
});

// ── filterTasks ──────────────────────────────────────────────────────────────

test('filterTasks: search on title', () => {
  const tasks = [makeTask({ id: '1', title: 'hello world' }), makeTask({ id: '2', title: 'goodbye' })];
  assert.equal(filterTasks(tasks, { searchQuery: 'hello' }).length, 1);
  assert.equal(filterTasks(tasks, { searchQuery: 'hello' })[0].id, '1');
});

test('filterTasks: search on prompt', () => {
  const tasks = [makeTask({ id: '1', prompt: 'beach scene' }), makeTask({ id: '2', prompt: 'city' })];
  assert.equal(filterTasks(tasks, { searchQuery: 'beach' }).length, 1);
});

test('filterTasks: search on submit_id', () => {
  const tasks = [makeTask({ id: '1', submit_id: 'abc123' }), makeTask({ id: '2', submit_id: 'xyz789' })];
  assert.equal(filterTasks(tasks, { searchQuery: 'abc' }).length, 1);
});

test('filterTasks: search is case-insensitive', () => {
  const tasks = [makeTask({ title: 'Hello World' })];
  assert.equal(filterTasks(tasks, { searchQuery: 'HELLO' }).length, 1);
});

test('filterTasks: statusTab filters correctly', () => {
  const tasks = [
    makeTask({ id: '1', status: 'queued' }),
    makeTask({ id: '2', status: 'succeeded' }),
    makeTask({ id: '3', status: 'failed' }),
    makeTask({ id: '4', status: 'submitting' }),
  ];
  assert.equal(filterTasks(tasks, { statusTab: 'waiting' }).length, 1);
  assert.equal(filterTasks(tasks, { statusTab: 'done' }).length, 1);
  assert.equal(filterTasks(tasks, { statusTab: 'failed' }).length, 1);
  assert.equal(filterTasks(tasks, { statusTab: 'running' }).length, 1);
  assert.equal(filterTasks(tasks, { statusTab: 'all' }).length, 4);
});

test('filterTasks: modelFilter', () => {
  const tasks = [
    makeTask({ id: '1', params: { model_version: 'seedance2.0' } }),
    makeTask({ id: '2', params: { model_version: 'seedance2.0fast' } }),
  ];
  const result = filterTasks(tasks, { modelFilter: 'seedance2.0' });
  assert.equal(result.length, 1);
  assert.equal(result[0].id, '1');
});

test('filterTasks: combined search + statusTab', () => {
  const tasks = [
    makeTask({ id: '1', title: 'beach', status: 'queued' }),
    makeTask({ id: '2', title: 'beach', status: 'succeeded' }),
    makeTask({ id: '3', title: 'city', status: 'queued' }),
  ];
  const result = filterTasks(tasks, { searchQuery: 'beach', statusTab: 'waiting' });
  assert.equal(result.length, 1);
  assert.equal(result[0].id, '1');
});

// ── sortTasks ────────────────────────────────────────────────────────────────

test('sortTasks: created_desc puts newest first', () => {
  const tasks = [
    makeTask({ id: '1', created_at: '2026-01-01T00:00:00Z' }),
    makeTask({ id: '2', created_at: '2026-02-01T00:00:00Z' }),
  ];
  assert.equal(sortTasks(tasks, 'created_desc')[0].id, '2');
});

test('sortTasks: created_asc puts oldest first', () => {
  const tasks = [
    makeTask({ id: '1', created_at: '2026-02-01T00:00:00Z' }),
    makeTask({ id: '2', created_at: '2026-01-01T00:00:00Z' }),
  ];
  assert.equal(sortTasks(tasks, 'created_asc')[0].id, '2');
});

test('sortTasks: default is created_desc', () => {
  const tasks = [
    makeTask({ id: '1', created_at: '2026-01-01T00:00:00Z' }),
    makeTask({ id: '2', created_at: '2026-03-01T00:00:00Z' }),
    makeTask({ id: '3', created_at: '2026-02-01T00:00:00Z' }),
  ];
  assert.equal(sortTasks(tasks)[0].id, '2');
});

test('sortTasks: does not mutate original array', () => {
  const tasks = [makeTask({ id: '1' }), makeTask({ id: '2' })];
  const copy = [...tasks];
  sortTasks(tasks, 'created_desc');
  assert.deepEqual(tasks, copy);
});

// ── paginateTasks ────────────────────────────────────────────────────────────

test('paginateTasks: first page', () => {
  const tasks = Array.from({ length: 25 }, (_, i) => makeTask({ id: String(i) }));
  const { items, total, totalPages, startIndex, endIndex, page } = paginateTasks(tasks, 1, 8);
  assert.equal(items.length, 8);
  assert.equal(total, 25);
  assert.equal(totalPages, 4);
  assert.equal(startIndex, 0);
  assert.equal(endIndex, 8);
  assert.equal(page, 1);
});

test('paginateTasks: last partial page', () => {
  const tasks = Array.from({ length: 25 }, (_, i) => makeTask({ id: String(i) }));
  const { items, startIndex, endIndex } = paginateTasks(tasks, 4, 8);
  assert.equal(items.length, 1);
  assert.equal(startIndex, 24);
  assert.equal(endIndex, 25);
});

test('paginateTasks: clamps out-of-range page', () => {
  const tasks = Array.from({ length: 5 }, (_, i) => makeTask({ id: String(i) }));
  const { page } = paginateTasks(tasks, 99, 8);
  assert.equal(page, 1);
});

test('paginateTasks: empty list returns 0 total and page 1', () => {
  const { total, totalPages, items } = paginateTasks([], 1, 8);
  assert.equal(total, 0);
  assert.equal(totalPages, 1);
  assert.equal(items.length, 0);
});

// ── formatPaginationLabel ─────────────────────────────────────────────────────

test('formatPaginationLabel: full range', () => {
  assert.equal(formatPaginationLabel(0, 8, 25), '1–8 / 25');
});

test('formatPaginationLabel: last partial page', () => {
  assert.equal(formatPaginationLabel(24, 25, 25), '25–25 / 25');
});

test('formatPaginationLabel: empty', () => {
  assert.equal(formatPaginationLabel(0, 0, 0), '0 条');
});

// ── deriveTaskFlowSteps ───────────────────────────────────────────────────────

test('deriveTaskFlowSteps: queued marks first step active', () => {
  const steps = deriveTaskFlowSteps(makeTask({ status: 'queued' }));
  assert.equal(steps[0].state, 'active');
  assert.equal(steps[0].spinning, false);
  assert.ok(steps.slice(1).every((s) => s.state === 'pending'));
});

test('deriveTaskFlowSteps: submitting marks step 1 active', () => {
  const steps = deriveTaskFlowSteps(makeTask({ status: 'submitting' }));
  assert.equal(steps[0].state, 'done');
  assert.equal(steps[1].state, 'active');
  assert.equal(steps[1].spinning, true);
});

test('deriveTaskFlowSteps: querying marks processing step as spinning', () => {
  const steps = deriveTaskFlowSteps(makeTask({ status: 'querying' }));
  assert.equal(steps[3].state, 'active');
  assert.equal(steps[3].spinning, true);
});

test('deriveTaskFlowSteps: succeeded marks all done', () => {
  const steps = deriveTaskFlowSteps(makeTask({ status: 'succeeded' }));
  assert.ok(steps.every((s) => s.state === 'done'));
});

test('deriveTaskFlowSteps: failed marks error at submitted step', () => {
  const steps = deriveTaskFlowSteps(makeTask({ status: 'failed' }));
  assert.ok(steps.some((s) => s.state === 'error'));
  assert.ok(!steps.every((s) => s.state === 'error'));
});

test('deriveTaskFlowSteps: returns 5 steps', () => {
  assert.equal(deriveTaskFlowSteps(makeTask()).length, 5);
});

test('canDeleteTask: draft and terminal statuses can be deleted', () => {
  assert.equal(canDeleteTask(makeTask({ status: 'draft' })), true);
  assert.equal(canDeleteTask(makeTask({ status: 'succeeded' })), true);
  assert.equal(canDeleteTask(makeTask({ status: 'failed' })), true);
  assert.equal(canDeleteTask(makeTask({ status: 'paused' })), true);
  assert.equal(canDeleteTask(makeTask({ status: 'paused' })), true);
});

test('canDeleteTask: active queue statuses are not deleted from detail actions', () => {
  assert.equal(canDeleteTask(makeTask({ status: 'queued' })), false);
  assert.equal(canDeleteTask(makeTask({ status: 'submitting' })), false);
  assert.equal(canDeleteTask(makeTask({ status: 'querying' })), false);
});

test('getTaskResultItems: exposes local result paths and remote urls', () => {
  const items = getTaskResultItems(makeTask({
    result_paths: ['/tmp/result.mp4'],
    result_urls: ['https://example.com/result.mp4'],
  }));

  // 本地文件已存在时，不重复展示远程 URL
  assert.deepEqual(items, [
    { kind: 'path', value: '/tmp/result.mp4', label: 'result.mp4' },
  ]);
});

test('getTaskResultItems: exposes remote urls when no local paths', () => {
  const items = getTaskResultItems(makeTask({
    result_paths: [],
    result_urls: ['https://example.com/result.mp4'],
  }));

  assert.deepEqual(items, [
    { kind: 'url', value: 'https://example.com/result.mp4', label: 'result.mp4' },
  ]);
});

test('getTaskHitResources: returns image resources before audio resources in task order', () => {
  const task = makeTask({
    image_asset_ids: ['img-2', 'img-1'],
    audio_asset_ids: ['aud-1', 'aud-2'],
  });
  const assets = new Map([
    ['img-1', { id: 'img-1', name: '图片 1' }],
    ['img-2', { id: 'img-2', name: '图片 2' }],
    ['aud-1', { id: 'aud-1', name: '音频 1' }],
    ['aud-2', { id: 'aud-2', name: '音频 2' }],
  ]);

  assert.deepEqual(getTaskHitResources(task, assets), [
    { type: 'image', displayType: 'role_image', label: '角色图片', asset: assets.get('img-2') },
    { type: 'image', displayType: 'role_image', label: '角色图片', asset: assets.get('img-1') },
    { type: 'audio', displayType: 'role_audio', label: '音频素材', asset: assets.get('aud-1') },
    { type: 'audio', displayType: 'role_audio', label: '音频素材', asset: assets.get('aud-2') },
  ]);
});

test('getTaskHitResources: filters missing assets and handles empty task', () => {
  const assets = new Map([
    ['img-1', { id: 'img-1', name: '图片 1' }],
  ]);

  assert.deepEqual(getTaskHitResources(makeTask({
    image_asset_ids: ['missing-img', 'img-1'],
    audio_asset_ids: ['missing-audio'],
  }), assets), [
    { type: 'image', displayType: 'role_image', label: '角色图片', asset: assets.get('img-1') },
  ]);
  assert.deepEqual(getTaskHitResources(null, assets), []);
});

test('getTaskHitResources: distinguishes role images from temp images', () => {
  const task = makeTask({
    image_asset_ids: ['img-role', 'img-temp'],
    temp_image_asset_ids: ['img-temp'],
    audio_asset_ids: ['aud-1'],
  });
  const assets = new Map([
    ['img-role', { id: 'img-role', name: '女主厨师服' }],
    ['img-temp', { id: 'img-temp', name: '分镜图 1', tags: ['temp_image'] }],
    ['aud-1', { id: 'aud-1', name: '女主声音' }],
  ]);

  assert.deepEqual(getTaskHitResources(task, assets), [
    { type: 'image', displayType: 'role_image', label: '角色图片', asset: assets.get('img-role') },
    { type: 'image', displayType: 'temp_image', label: '临时参考图', asset: assets.get('img-temp') },
    { type: 'audio', displayType: 'role_audio', label: '音频素材', asset: assets.get('aud-1') },
  ]);
});

test('getTaskHitResources: does not mark role image as temp when temp ids contain stale non-temp asset', () => {
  const task = makeTask({
    image_asset_ids: ['img-role', 'img-temp'],
    temp_image_asset_ids: ['img-role', 'img-temp'],
  });
  const assets = new Map([
    ['img-role', { id: 'img-role', kind: 'image', name: '厨师服', tags: [] }],
    ['img-temp', { id: 'img-temp', kind: 'image', name: '分镜图 1', tags: ['temp_image'] }],
  ]);

  assert.deepEqual(getTaskHitResources(task, assets), [
    { type: 'image', displayType: 'role_image', label: '角色图片', asset: assets.get('img-role') },
    { type: 'image', displayType: 'temp_image', label: '临时参考图', asset: assets.get('img-temp') },
  ]);
});

test('getCommandPreviewPresentation: command preview is shown as modal entry', () => {
  assert.deepEqual(getCommandPreviewPresentation('multimodal2video'), {
    hasCommand: true,
    shouldRenderInlineBlock: false,
    actionLabel: '查看命令',
    hint: '已生成命令预览，点击查看完整命令',
  });
});

test('getCommandPreviewPresentation: empty command hides section', () => {
  assert.equal(getCommandPreviewPresentation('', false).hasCommand, false);
});

// ── deriveTaskProgress ────────────────────────────────────────────────────────

test('deriveTaskProgress: succeeded = 100%', () => {
  const { percent, stage } = deriveTaskProgress(makeTask({ status: 'succeeded' }));
  assert.equal(percent, 100);
  assert.equal(stage, '执行成功');
});

test('deriveTaskProgress: failed = 0%', () => {
  assert.equal(deriveTaskProgress(makeTask({ status: 'failed' })).percent, 0);
});

test('deriveTaskProgress: queued is low percent', () => {
  assert.ok(deriveTaskProgress(makeTask({ status: 'queued' })).percent < 20);
});

test('deriveTaskProgress: retry_wait stage includes attempt count', () => {
  const { stage } = deriveTaskProgress(makeTask({ status: 'retry_wait', attempt_count: 3 }));
  assert.ok(stage.includes('3'));
});

// ── deriveQueueStats ──────────────────────────────────────────────────────────

test('deriveQueueStats: counts each group', () => {
  const tasks = [
    makeTask({ status: 'queued' }),
    makeTask({ status: 'submitting' }),
    makeTask({ status: 'retry_wait' }),
    makeTask({ status: 'succeeded' }),
    makeTask({ status: 'failed' }),
    makeTask({ status: 'paused' }),
  ];
  const stats = deriveQueueStats(tasks);
  assert.equal(stats.waiting, 1);
  assert.equal(stats.running, 1);
  assert.equal(stats.retry, 1);
  assert.equal(stats.done, 1);
  assert.equal(stats.failed, 1);
  assert.equal(stats.paused, 1);
});

test('deriveQueueStats: empty task list', () => {
  const stats = deriveQueueStats([]);
  assert.equal(stats.waiting, 0);
  assert.equal(stats.done, 0);
});

// ── getModelOptions ───────────────────────────────────────────────────────────

test('getModelOptions: deduplicates and sorts', () => {
  const tasks = [
    makeTask({ params: { model_version: 'seedance2.0fast' } }),
    makeTask({ params: { model_version: 'seedance2.0' } }),
    makeTask({ params: { model_version: 'seedance2.0' } }),
  ];
  const opts = getModelOptions(tasks);
  assert.equal(opts.length, 2);
  assert.equal(opts[0], 'seedance2.0');
});
