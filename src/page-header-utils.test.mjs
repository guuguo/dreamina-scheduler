import assert from 'node:assert/strict';
import { test } from 'node:test';
import { buildSecondaryPageHeaderConfig } from './page-header-utils.js';

test('buildSecondaryPageHeaderConfig returns create task title and task center back label', () => {
  assert.deepEqual(buildSecondaryPageHeaderConfig('task'), {
    title: '新建 multimodal2video 任务',
    backLabel: '返回任务中心',
  });
});

test('buildSecondaryPageHeaderConfig returns edit task title in edit mode (no name)', () => {
  assert.deepEqual(buildSecondaryPageHeaderConfig('task', { mode: 'edit' }), {
    title: '编辑：未命名任务',
    backLabel: '返回任务中心',
  });
});

test('buildSecondaryPageHeaderConfig returns task name in edit mode', () => {
  assert.deepEqual(buildSecondaryPageHeaderConfig('task', { mode: 'edit', name: '火龙女侠' }), {
    title: '编辑：火龙女侠',
    backLabel: '返回任务中心',
  });
});

test('buildSecondaryPageHeaderConfig returns role editor title and role list back label', () => {
  assert.deepEqual(buildSecondaryPageHeaderConfig('role', { mode: 'edit', name: '女主' }), {
    title: '女主',
    backLabel: '返回角色列表',
  });
});

test('buildSecondaryPageHeaderConfig returns create role title', () => {
  assert.equal(buildSecondaryPageHeaderConfig('role', { mode: 'create' }).title, '新建角色');
});
