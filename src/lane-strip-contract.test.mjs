import { strict as assert } from 'node:assert';
import { readFileSync } from 'node:fs';
import { test } from 'node:test';

const source = readFileSync(new URL('./components/LaneStrip.jsx', import.meta.url), 'utf8');

test('lane strip uses one shared dispatch pool instead of model-based local queues', () => {
  assert.match(source, /共享待调度池/);
  assert.match(source, /getSharedWaitingTasks/);
  assert.doesNotMatch(source, /getLaneLocalTasks/);
  assert.doesNotMatch(source, />本地队列</);
});

test('shared pool modal describes dynamic lane assignment', () => {
  assert.match(source, /任一启用车道空闲后动态分配/);
  assert.match(source, /仅展示排队中和等待重试的任务/);
});
