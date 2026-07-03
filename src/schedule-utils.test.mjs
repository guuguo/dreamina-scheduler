import { strict as assert } from 'node:assert';
import { test } from 'node:test';
import {
  buildBatchQueuePlan,
  buildBatchSchedulePlan,
  canUseAlternatingFastQueue,
  canScheduleTask,
  formatSchedulePlanSummary,
  resolvePrepareGenerateOperation,
  resolveScheduleAt,
} from './schedule-utils.js';

const baseNow = new Date('2026-05-01T10:00:00+08:00');

test('resolveScheduleAt: immediate returns null scheduled time', () => {
  assert.equal(resolveScheduleAt({ mode: 'immediate' }, baseNow), null);
});

test('resolveScheduleAt: relative hours schedules after the requested hours', () => {
  const result = new Date(resolveScheduleAt({ mode: 'relative', hours: 3 }, baseNow));
  assert.equal(result.getTime() - baseNow.getTime(), 3 * 60 * 60 * 1000);
});

test('resolveScheduleAt: tomorrow day time combines tomorrow with HH:mm', () => {
  const result = new Date(resolveScheduleAt({ mode: 'dayTime', day: 'tomorrow', time: '02:30' }, baseNow));
  const expected = new Date(baseNow);
  expected.setDate(expected.getDate() + 1);
  expected.setHours(2, 30, 0, 0);
  assert.equal(result.getTime(), expected.getTime());
});

test('resolveScheduleAt: today day time uses today when the time is still ahead', () => {
  const now = new Date('2026-05-01T00:26:00+08:00');
  const result = new Date(resolveScheduleAt({ mode: 'dayTime', day: 'today', time: '02:00' }, now));
  const expected = new Date(now);
  expected.setHours(2, 0, 0, 0);
  assert.equal(result.getTime(), expected.getTime());
});

test('resolveScheduleAt: auto day time rolls to tomorrow only after the time passed', () => {
  const now = new Date('2026-05-01T09:26:00+08:00');
  const result = new Date(resolveScheduleAt({ mode: 'dayTime', day: 'auto', time: '02:00' }, now));
  const expected = new Date(now);
  expected.setDate(expected.getDate() + 1);
  expected.setHours(2, 0, 0, 0);
  assert.equal(result.getTime(), expected.getTime());
});

test('resolveScheduleAt: custom datetime-local value converts to ISO', () => {
  const result = resolveScheduleAt({ mode: 'custom', customValue: '2026-05-02T11:45' }, baseNow);
  assert.equal(result, new Date('2026-05-02T11:45').toISOString());
});

test('buildBatchSchedulePlan: assigns incremental scheduled times by selected task order', () => {
  const startAt = new Date('2026-05-01T20:00:00+08:00').toISOString();
  assert.deepEqual(buildBatchSchedulePlan(['task-1', 'task-2', 'task-3'], { startAt, intervalMinutes: 30 }), [
    { taskId: 'task-1', scheduledAt: startAt },
    { taskId: 'task-2', scheduledAt: new Date(new Date(startAt).getTime() + 30 * 60 * 1000).toISOString() },
    { taskId: 'task-3', scheduledAt: new Date(new Date(startAt).getTime() + 60 * 60 * 1000).toISOString() },
  ]);
});

test('buildBatchSchedulePlan: defaults to continuous queue with the same start time', () => {
  const startAt = new Date('2026-05-01T20:00:00+08:00').toISOString();
  assert.deepEqual(buildBatchSchedulePlan(['task-1', 'task-2'], { startAt }), [
    { taskId: 'task-1', scheduledAt: startAt },
    { taskId: 'task-2', scheduledAt: startAt },
  ]);
});

test('buildBatchQueuePlan: immediate continuous queue uses null schedule for all tasks', () => {
  assert.deepEqual(buildBatchQueuePlan(['task-1', 'task-2'], { startAt: null, intervalMinutes: 0 }), [
    { taskId: 'task-1', scheduledAt: null },
    { taskId: 'task-2', scheduledAt: null },
  ]);
});

test('buildBatchQueuePlan: immediate with interval queues first task now and schedules the rest', () => {
  const now = new Date('2026-05-01T20:00:00+08:00');
  assert.deepEqual(buildBatchQueuePlan(['task-1', 'task-2', 'task-3'], { startAt: null, intervalMinutes: 15, now }), [
    { taskId: 'task-1', scheduledAt: null },
    { taskId: 'task-2', scheduledAt: new Date(now.getTime() + 15 * 60 * 1000).toISOString() },
    { taskId: 'task-3', scheduledAt: new Date(now.getTime() + 30 * 60 * 1000).toISOString() },
  ]);
});

test('buildBatchQueuePlan: delayed start applies interval from the selected start time', () => {
  const startAt = new Date('2026-05-01T20:00:00+08:00').toISOString();
  assert.deepEqual(buildBatchQueuePlan(['task-1', 'task-2'], { startAt, intervalMinutes: 30 }), [
    { taskId: 'task-1', scheduledAt: startAt },
    { taskId: 'task-2', scheduledAt: new Date(new Date(startAt).getTime() + 30 * 60 * 1000).toISOString() },
  ]);
});

test('formatSchedulePlanSummary: immediate queue plans do not show epoch time', () => {
  assert.equal(formatSchedulePlanSummary([
    { taskId: 'task-1', scheduledAt: null },
    { taskId: 'task-2', scheduledAt: null },
  ]), '2 个任务 · 立即连续排队');
});

test('formatSchedulePlanSummary: immediate interval plans summarize immediate start and scheduled tail', () => {
  const later = new Date('2026-05-01T20:15:00+08:00').toISOString();
  const result = formatSchedulePlanSummary([
    { taskId: 'task-1', scheduledAt: null },
    { taskId: 'task-2', scheduledAt: later },
  ]);
  assert.match(result, /^2 个任务 · 立即开始，排布至 /);
  assert.doesNotMatch(result, /1970/);
});

test('canUseAlternatingFastQueue: only standard seedance tasks can opt in', () => {
  assert.equal(canUseAlternatingFastQueue([
    { params: { model_version: 'seedance2.0' } },
    { params: { model_version: 'seedance2.0' } },
  ]), true);
  assert.equal(canUseAlternatingFastQueue([
    { params: { model_version: 'seedance2.0' } },
    { params: { model_version: 'seedance2.0fast' } },
  ]), false);
  assert.equal(canUseAlternatingFastQueue([]), false);
});

test('canScheduleTask: saved or finished tasks can be prepared for scheduled generation', () => {
  assert.equal(canScheduleTask({ status: 'draft' }), true);
  assert.equal(canScheduleTask({ status: 'queued' }), true);
  assert.equal(canScheduleTask({ status: 'scheduled' }), true);
  assert.equal(canScheduleTask({ status: 'paused' }), true);
  assert.equal(canScheduleTask({ status: 'retry_wait' }), true);
  assert.equal(canScheduleTask({ status: 'failed' }), true);
  assert.equal(canScheduleTask({ status: 'failed' }), true);
  assert.equal(canScheduleTask({ status: 'succeeded' }), true);
  assert.equal(canScheduleTask({ status: 'submitting' }), false);
  assert.equal(canScheduleTask({ status: 'querying' }), false);
  assert.equal(canScheduleTask({ status: 'submitted' }), false);
});

test('resolvePrepareGenerateOperation: immediate submit starts generation now', () => {
  assert.deepEqual(resolvePrepareGenerateOperation({ scheduledAt: null }), {
    type: 'submit',
    scheduledAt: null,
  });
});

test('resolvePrepareGenerateOperation: scheduled time prepares delayed generation', () => {
  const scheduledAt = new Date('2026-05-01T20:00:00+08:00').toISOString();
  assert.deepEqual(resolvePrepareGenerateOperation({ scheduledAt }), {
    type: 'schedule',
    scheduledAt,
  });
});
