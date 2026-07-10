import { strict as assert } from 'node:assert';
import { test } from 'node:test';
import {
  deriveFastLaneStatus,
  deriveStandardLaneStatus,
  deriveNextAction,
  deriveTimelineEvents,
  selectKeyTimelineRecords,
  getLaneRemoteOccupancyDisplay,
  getLaneLocalTasks,
  getLaneStatuses,
} from './lane-utils.js';

function task(overrides = {}) {
  return {
    id: 'task-1',
    title: '测试任务',
    status: 'queued',
    submit_id: '',
    auto_query_stopped: false,
    params: { model_version: 'seedance2.0' },
    execution_records: [],
    queue_info: null,
    ...overrides,
  };
}

function record(overrides = {}) {
  return {
    id: 'rec-1',
    submit_id: 'sub-1',
    status: 'querying',
    finished_at: '',
    input_snapshot: { params: { model_version: 'seedance2.0fast' } },
    query_records: [],
    ...overrides,
  };
}

test('getLaneStatuses uses backend laneStatus when present', () => {
  const backend = [
    { queueKind: 'standard', modelVersion: 'seedance2.0' },
    { queueKind: 'fast', modelVersion: 'seedance2.0fast' },
  ];
  assert.equal(getLaneStatuses(backend, []), backend);
});

test('getLaneLocalTasks matches the lane card local task count', () => {
  const tasks = [
    task({ id: 'std-queued', status: 'queued' }),
    task({ id: 'std-active', status: 'querying' }),
    task({ id: 'std-scheduled', status: 'scheduled' }),
    task({ id: 'std-draft', status: 'draft' }),
    task({ id: 'std-done', status: 'succeeded' }),
    task({ id: 'fast-retry', status: 'retry_wait', params: { model_version: 'seedance2.0fast' } }),
  ];

  assert.deepEqual(getLaneLocalTasks(tasks, 'standard').map((item) => item.id), ['std-queued']);
  assert.deepEqual(getLaneLocalTasks(tasks, 'fast').map((item) => item.id), ['fast-retry']);
});

test('deriveFastLaneStatus detects active fast execution record fallback', () => {
  const lane = deriveFastLaneStatus([
    task({
      id: 'std-with-fast-record',
      title: '标准任务 Fast 兜底',
      status: 'submitted',
      submit_id: 'standard-sub',
      params: { model_version: 'seedance2.0' },
      execution_records: [record({
        submit_id: 'fast-sub',
        query_records: [{
          started_at: '2026-07-04T00:00:10Z',
          finished_at: '2026-07-04T00:00:20Z',
          stdout: '{"queue_info":{"queue_idx":7,"queue_length":99,"queue_status":"queueing"}}',
        }],
      })],
    }),
  ], new Date('2026-07-04T00:00:30Z').getTime());

  assert.equal(lane.isActive, true);
  assert.equal(lane.currentTaskId, 'std-with-fast-record');
  assert.equal(lane.currentTaskTitle, '标准任务 Fast 兜底');
  assert.equal(lane.submitId, 'fast-sub');
  assert.equal(lane.queuePosition, 7);
  assert.equal(lane.queueLength, 99);
  assert.equal(lane.nextCheckAt, '2026-07-04T00:01:20.000Z');
});

test('active cross-lane fast submit belongs only to the matching fast execution record lane', () => {
  const activeTask = task({
    id: 'active-cross-lane-fast',
    status: 'querying',
    submit_id: 'fast-current-submit',
    params: { model_version: 'seedance2.0' },
    execution_records: [record({
      submit_id: 'fast-current-submit',
      status: 'querying',
      started_at: '2026-07-04T00:00:00Z',
      finished_at: '2026-07-04T00:00:01Z',
      input_snapshot: { params: { model_version: 'seedance2.0fast' } },
    })],
  });

  const standard = deriveStandardLaneStatus([activeTask]);
  const fast = deriveFastLaneStatus([activeTask]);

  assert.equal(standard.isActive, false);
  assert.equal(fast.isActive, true);
  assert.equal(fast.currentTaskId, 'active-cross-lane-fast');
});

test('deriveStandardLaneStatus cooldown ignores non-concurrency and due retries', () => {
  const nowMs = new Date('2026-07-04T00:00:00Z').getTime();
  const lane = deriveStandardLaneStatus([
    task({
      id: 'transient',
      status: 'retry_wait',
      next_run_at: '2026-07-04T00:00:10Z',
      last_error: 'temporary network error',
    }),
    task({
      id: 'due-concurrency',
      status: 'retry_wait',
      next_run_at: '2026-07-03T23:59:55Z',
      last_error: 'ExceedConcurrencyLimit',
    }),
    task({
      id: 'current-concurrency',
      status: 'retry_wait',
      next_run_at: '2026-07-04T00:01:30Z',
      last_error: 'ExceedConcurrencyLimit',
    }),
  ], nowMs);

  assert.equal(lane.isCoolingDown, true);
  assert.equal(lane.nextCheckAt, '2026-07-04T00:01:30Z');
  assert.equal(lane.cooldownReason, '并发限制，1 个任务等待重试');
});

test('retry wait lane uses the newest execution record timestamp when records are out of order', () => {
  const nowMs = new Date('2026-07-04T00:02:00Z').getTime();
  const retryTask = task({
    status: 'retry_wait',
    next_run_at: '2026-07-04T00:05:00Z',
    last_error: 'temporary network error',
    execution_records: [
      record({
        id: 'new-fast-retry',
        status: 'retry_wait',
        started_at: '2026-07-04T00:01:00Z',
        finished_at: '2026-07-04T00:01:10Z',
        input_snapshot: { params: { model_version: 'seedance2.0fast' } },
      }),
      record({
        id: 'old-standard-retry-appended-later',
        status: 'retry_wait',
        started_at: '2026-07-04T00:00:00Z',
        finished_at: '2026-07-04T00:00:10Z',
        input_snapshot: { params: { model_version: 'seedance2.0' } },
      }),
    ],
  });

  const standard = deriveStandardLaneStatus([retryTask], nowMs);
  const fast = deriveFastLaneStatus([retryTask], nowMs);

  assert.equal(standard.waitingTaskCount, 0);
  assert.equal(fast.waitingTaskCount, 1);
});

test('remote occupancy display treats concurrency cooldown as remote occupied', () => {
  const display = getLaneRemoteOccupancyDisplay({
    queueKind: 'fast',
    isActive: false,
    isCoolingDown: true,
    cooldownReason: '并发限制，1 个任务等待重试',
    currentTaskTitle: '',
    queuePosition: null,
    queueLength: null,
  });

  assert.equal(display.value, '远端占位中');
  assert.equal(display.copy, '并发限制，1 个任务等待重试');
  assert.equal(display.tone, 'warn');
});

test('remote occupancy display shows idle only when there is no active or cooldown signal', () => {
  const display = getLaneRemoteOccupancyDisplay({
    queueKind: 'standard',
    isActive: false,
    isCoolingDown: false,
    currentTaskTitle: '',
    queuePosition: null,
    queueLength: null,
  });

  assert.equal(display.value, '空闲');
  assert.equal(display.tone, 'idle');
});

test('deriveNextAction shows fast queued scheduler ETA', () => {
  const nowMs = new Date('2026-07-04T00:00:00Z').getTime();
  const next = deriveNextAction(task({
    status: 'queued',
    params: { model_version: 'seedance2.0fast' },
  }), nowMs, { schedulerTickSeconds: 30 });
  const checkAt = new Date(nowMs + 30_000);
  const checkClock = `${String(checkAt.getHours()).padStart(2, '0')}:${String(checkAt.getMinutes()).padStart(2, '0')}`;

  assert.equal(next.action, '约 30 秒内 走 Fast');
  assert.equal(next.reason, `任务在队列中等待，预计 ${checkClock} 前后提交到Fast车道。`);
});

test('deriveNextAction distinguishes remote processing without queue position', () => {
  const next = deriveNextAction(task({
    status: 'querying',
    queue_info: {
      queue_idx: null,
      queue_length: null,
    },
  }));

  assert.equal(next.action, '远端处理中');
  assert.equal(next.reason, '任务已提交到标准车道远端，但暂未返回排队名次。');
});

test('deriveNextAction keeps explicit remote queue progress', () => {
  const next = deriveNextAction(task({
    status: 'querying',
    queue_info: {
      queue_idx: 6932,
      queue_length: 298595,
    },
  }));

  assert.equal(next.action, '远端排队 #6932 / 298595');
});

test('deriveNextAction switches a cooling fast retry to idle standard immediately', () => {
  const nowMs = new Date('2026-07-04T00:00:00Z').getTime();
  const next = deriveNextAction(task({
    status: 'retry_wait',
    next_run_at: '2026-07-04T00:03:00Z',
    last_error: 'ExceedConcurrencyLimit',
    params: { model_version: 'seedance2.0fast' },
  }), nowMs, {
    schedulerTickSeconds: 30,
    laneStatuses: [
      { queueKind: 'standard', enabled: true, isActive: false, isCoolingDown: false },
      { queueKind: 'fast', enabled: true, isActive: false, isCoolingDown: true },
    ],
  });

  assert.equal(next.action, '约 30 秒内 走 标准');
  assert.equal(next.reason, '标准车道空闲，将在下次调度检查时切换提交。');
});

test('deriveNextAction shows generation review retry at queue tail', () => {
  const next = deriveNextAction(task({
    status: 'retry_wait',
    next_run_at: '2026-07-03T23:59:00Z',
    last_error: 'generation failed: pre-TNS check did not pass',
    execution_records: [{
      status: 'retry_wait',
      error_kind: 'GenerationPrecheck',
    }],
  }), new Date('2026-07-04T00:00:00Z').getTime());

  assert.equal(next.action, '应立即重试 · 队尾');
  assert.equal(next.reason, '生成审核未通过，已移到队伍末尾；前面的任务完成后再重试。');
});

test('selectKeyTimelineRecords keeps five useful records and collapses adjacent probes', () => {
  const events = [
    { at: '2026-07-04T00:05:00Z', time: '08:05', title: 'Fast 探测', detail: '并发限制' },
    { at: '2026-07-04T00:04:00Z', time: '08:04', title: '标准 探测', detail: '并发限制' },
    { at: '2026-07-04T00:03:00Z', time: '08:03', title: '标准 提交成功', detail: '已提交' },
    { at: '2026-07-04T00:02:00Z', time: '08:02', title: '标准 开始', detail: '开始' },
    { at: '2026-07-04T00:01:00Z', time: '08:01', title: '进入队列', detail: '排队' },
  ];
  const queries = [
    { id: 'q1', status: 'querying', started_at: '2026-07-04T00:04:30Z' },
    { id: 'q2', status: 'querying', started_at: '2026-07-04T00:03:30Z' },
  ];

  const records = selectKeyTimelineRecords(events, queries, 5);
  assert.equal(records.length, 4);
  assert.equal(records.filter((record) => record.kind === 'query').length, 0);
  assert.equal(records[0].event.title, 'Fast 探测');
  assert.equal(records[1].event.title, '标准 提交成功');
});

test('deriveTimelineEvents includes execution start and does not truncate full history', () => {
  const executionRecords = Array.from({ length: 9 }, (_, index) => record({
    id: `rec-${index}`,
    status: 'succeeded',
    started_at: `2026-07-04T00:${String(index).padStart(2, '0')}:00Z`,
    finished_at: `2026-07-04T00:${String(index).padStart(2, '0')}:30Z`,
    input_snapshot: { params: { model_version: 'seedance2.0fast' } },
  }));

  const events = deriveTimelineEvents(task({
    queued_at: '2026-07-03T23:59:00Z',
    execution_records: executionRecords,
  }));

  assert.ok(events.length > 8);
  assert.ok(events.some((event) => event.title === 'Fast 开始'));
  assert.ok(events.some((event) => event.title === 'Fast 完成'));
  assert.ok(events.some((event) => event.title === '进入队列'));
});
