import { test } from 'node:test';
import assert from 'node:assert/strict';
import {
  deriveCurrentExecutionRecord,
  deriveCurrentQueryRecords,
  deriveTaskHistory,
  historyHasResults,
  historyItemLabel,
  isInterruptNotice,
} from './task-history-utils.js';

// ── deriveTaskHistory ──────────────────────────────────────────────────────

test('null task returns empty array', () => {
  assert.deepEqual(deriveTaskHistory(null), []);
});

test('task with no history returns empty array', () => {
  const task = { id: 't1', status: 'queued', attempts: [], result_paths: [], result_urls: [], submit_id: '' };
  assert.deepEqual(deriveTaskHistory(task), []);
});

test('task with execution_records returns record items', () => {
  const task = {
    id: 't1',
    execution_records: [
      {
        id: 'rec-1',
        submit_id: 'sub-abc',
        status: 'succeeded',
        started_at: '2026-04-30T10:00:00Z',
        finished_at: '2026-04-30T10:05:00Z',
        result_paths: ['/tmp/a.mp4'],
        result_urls: ['https://cdn/a.mp4'],
        attempts: [],
        error_kind: '',
        error_detail: '',
        command_preview: [],
        input_snapshot: null,
      },
    ],
  };
  const items = deriveTaskHistory(task);
  assert.equal(items.length, 1);
  assert.equal(items[0].source, 'record');
  assert.equal(items[0].submit_id, 'sub-abc');
  assert.deepEqual(items[0].result_paths, ['/tmp/a.mp4']);
});

test('execution_records are sorted newest first', () => {
  const task = {
    id: 't2',
    execution_records: [
      { id: 'rec-1', started_at: '2026-04-30T09:00:00Z', submit_id: '', status: '', finished_at: '', result_paths: [], result_urls: [], attempts: [], error_kind: '', error_detail: '', command_preview: [] },
      { id: 'rec-2', started_at: '2026-04-30T11:00:00Z', submit_id: '', status: '', finished_at: '', result_paths: [], result_urls: [], attempts: [], error_kind: '', error_detail: '', command_preview: [] },
    ],
  };
  const items = deriveTaskHistory(task);
  assert.equal(items[0].id, 'rec-2', '最新记录应排在最前');
  assert.equal(items[1].id, 'rec-1');
});

test('retry_wait execution_records with same retry error are collapsed for display', () => {
  const task = {
    id: 't2b',
    execution_records: [
      {
        id: 'rec-retry-1',
        started_at: '2026-04-30T09:00:00Z',
        finished_at: '2026-04-30T09:00:01Z',
        submit_id: '',
        status: 'retry_wait',
        result_paths: [],
        result_urls: [],
        query_records: [],
        error_kind: 'ConcurrencyLimit',
        error_detail: 'api error: ret=1310, message=ExceedConcurrencyLimit submit_preflight={...}',
        command_preview: [],
      },
      {
        id: 'rec-retry-2',
        started_at: '2026-04-30T09:05:00Z',
        finished_at: '2026-04-30T09:05:01Z',
        submit_id: '',
        status: 'retry_wait',
        result_paths: [],
        result_urls: [],
        query_records: [],
        error_kind: 'ConcurrencyLimit',
        error_detail: 'api error: ret=1310, message=ExceedConcurrencyLimit submit_preflight={...}',
        command_preview: [],
      },
    ],
  };

  const items = deriveTaskHistory(task);

  assert.equal(items.length, 1);
  assert.equal(items[0].id, 'rec-retry-2');
  assert.equal(items[0].retry_count, 2);
  assert.equal(items[0].error_detail, '并发任务仍在生成中，已自动排队等待下次重试。');
});

test('retry_wait execution_records with different models stay separate', () => {
  const task = {
    id: 't2-models',
    execution_records: [
      {
        id: 'rec-standard',
        started_at: '2026-04-30T09:00:00Z',
        finished_at: '2026-04-30T09:00:01Z',
        submit_id: '',
        status: 'retry_wait',
        result_paths: [],
        result_urls: [],
        query_records: [],
        error_kind: 'ConcurrencyLimit',
        error_detail: 'api error: ret=1310, message=ExceedConcurrencyLimit',
        command_preview: ['multimodal2video', '--model_version=seedance2.0'],
        input_snapshot: { params: { model_version: 'seedance2.0' } },
      },
      {
        id: 'rec-fast',
        started_at: '2026-04-30T09:01:00Z',
        finished_at: '2026-04-30T09:01:01Z',
        submit_id: '',
        status: 'retry_wait',
        result_paths: [],
        result_urls: [],
        query_records: [],
        error_kind: 'ConcurrencyLimit',
        error_detail: 'api error: ret=1310, message=ExceedConcurrencyLimit',
        command_preview: ['multimodal2video', '--model_version=seedance2.0fast'],
        input_snapshot: { params: { model_version: 'seedance2.0fast' } },
      },
    ],
  };

  const items = deriveTaskHistory(task);

  assert.equal(items.length, 2);
  assert.equal(historyItemLabel(items[0], 2), '第 2 次 · Fast');
  assert.equal(historyItemLabel(items[1], 1), '第 1 次 · 标准');
});

test('final failed concurrency retry record is collapsed into one short display item', () => {
  const task = {
    id: 't2c',
    execution_records: [
      {
        id: 'rec-retry-1',
        started_at: '2026-04-30T09:00:00Z',
        finished_at: '2026-04-30T09:00:01Z',
        submit_id: '',
        status: 'retry_wait',
        result_paths: [],
        result_urls: [],
        query_records: [],
        error_kind: 'ConcurrencyLimit',
        error_detail: 'api error: ret=1310, message=ExceedConcurrencyLimit submit_preflight={...}',
        command_preview: [],
      },
      {
        id: 'rec-failed',
        started_at: '2026-04-30T09:05:00Z',
        finished_at: '2026-04-30T09:05:01Z',
        submit_id: '',
        status: 'failed',
        result_paths: [],
        result_urls: [],
        query_records: [],
        error_kind: 'ConcurrencyLimit',
        error_detail: 'api error: ret=1310, message=ExceedConcurrencyLimit submit_preflight={...}',
        command_preview: [],
      },
    ],
  };

  const items = deriveTaskHistory(task);

  assert.equal(items.length, 1);
  assert.equal(items[0].id, 'rec-failed');
  assert.equal(items[0].status, 'failed');
  assert.equal(items[0].retry_count, 2);
  assert.equal(items[0].error_detail, '并发任务仍在生成中，已自动排队等待下次重试。');
});

test('single legacy failed concurrency record uses short error detail', () => {
  const task = {
    id: 't2d',
    execution_records: [
      {
        id: 'rec-failed',
        started_at: '2026-04-30T09:00:00Z',
        finished_at: '2026-04-30T09:00:01Z',
        submit_id: '',
        status: 'failed',
        result_paths: [],
        result_urls: [],
        query_records: [],
        error_kind: 'ConcurrencyLimit',
        error_detail: 'api error: ret=1310, message=ExceedConcurrencyLimit submit_preflight={...}',
        command_preview: [],
      },
    ],
  };

  const items = deriveTaskHistory(task);

  assert.equal(items.length, 1);
  assert.equal(items[0].retry_count, 1);
  assert.equal(items[0].error_detail, '并发任务仍在生成中，已自动排队等待下次重试。');
});

test('legacy task with only top-level attempts/results becomes legacy record', () => {
  const task = {
    id: 't3',
    status: 'succeeded',
    submit_id: 'sub-legacy',
    result_paths: ['/tmp/legacy.mp4'],
    result_urls: ['https://cdn/legacy.mp4'],
    attempts: [{ id: 'a1', status: 'done' }],
    last_error: '',
    created_at: '2026-04-01T00:00:00Z',
    finished_at: '2026-04-01T00:05:00Z',
    command_preview: [],
    execution_records: [],
  };
  const items = deriveTaskHistory(task);
  assert.equal(items.length, 1);
  assert.equal(items[0].source, 'legacy');
  assert.equal(items[0].submit_id, 'sub-legacy');
  assert.deepEqual(items[0].result_paths, ['/tmp/legacy.mp4']);
  assert.equal(items[0].attempts.length, 1);
});

test('legacy task with only submit_id produces legacy record', () => {
  const task = { id: 't4', submit_id: 'sub-x', attempts: [], result_paths: [], result_urls: [], execution_records: [] };
  const items = deriveTaskHistory(task);
  assert.equal(items.length, 1);
  assert.equal(items[0].source, 'legacy');
});

// ── current execution record ────────────────────────────────────────────────

test('deriveCurrentExecutionRecord defaults to execution record matching current task submit_id', () => {
  const task = {
    id: 't5',
    status: 'querying',
    submit_id: 'sub-2',
    execution_records: [
      {
        id: 'rec-1',
        submit_id: 'sub-1',
        status: 'succeeded',
        started_at: '2026-04-30T10:00:00Z',
        query_records: [{ id: 'qr-1', status: 'succeeded' }],
      },
      {
        id: 'rec-2',
        submit_id: 'sub-2',
        status: 'querying',
        started_at: '2026-04-30T11:00:00Z',
        query_records: [{ id: 'qr-2', status: 'querying' }],
      },
    ],
  };

  const current = deriveCurrentExecutionRecord(task);

  assert.equal(current.id, 'rec-2');
  assert.equal(current.status, 'querying');
  assert.equal(current.submit_id, 'sub-2');
});

test('deriveCurrentExecutionRecord respects selected execution record id', () => {
  const task = {
    id: 't6',
    status: 'querying',
    submit_id: 'sub-2',
    execution_records: [
      { id: 'rec-1', submit_id: 'sub-1', status: 'succeeded', started_at: '2026-04-30T10:00:00Z', query_records: [] },
      { id: 'rec-2', submit_id: 'sub-2', status: 'querying', started_at: '2026-04-30T11:00:00Z', query_records: [] },
    ],
  };

  const current = deriveCurrentExecutionRecord(task, 'rec-1');

  assert.equal(current.id, 'rec-1');
  assert.equal(current.status, 'succeeded');
  assert.equal(current.submit_id, 'sub-1');
});

test('deriveCurrentExecutionRecord does not treat old submit_id as current while edited task is queued', () => {
  const task = {
    id: 't6b',
    status: 'queued',
    submit_id: 'sub-old',
    execution_records: [
      { id: 'rec-old', submit_id: 'sub-old', status: 'succeeded', started_at: '2026-04-30T10:00:00Z', query_records: [] },
    ],
  };

  assert.equal(deriveCurrentExecutionRecord(task), null);
  assert.equal(deriveCurrentExecutionRecord(task, 'rec-old')?.id, 'rec-old');
});


test('deriveCurrentQueryRecords returns only selected execution record query records', () => {
  const task = {
    id: 't7',
    status: 'querying',
    submit_id: 'sub-2',
    execution_records: [
      { id: 'rec-1', submit_id: 'sub-1', status: 'succeeded', started_at: '2026-04-30T10:00:00Z', query_records: [{ id: 'qr-old' }] },
      { id: 'rec-2', submit_id: 'sub-2', status: 'querying', started_at: '2026-04-30T11:00:00Z', query_records: [{ id: 'qr-current' }] },
    ],
  };

  assert.deepEqual(deriveCurrentQueryRecords(task, 'rec-1').map((item) => item.id), ['qr-old']);
  assert.deepEqual(deriveCurrentQueryRecords(task).map((item) => item.id), ['qr-current']);
});

// ── historyHasResults ──────────────────────────────────────────────────────

test('historyHasResults returns true when any item has result_paths', () => {
  const items = [{ result_paths: ['/a.mp4'], result_urls: [] }];
  assert.equal(historyHasResults(items), true);
});

test('historyHasResults returns true when any item has result_urls', () => {
  const items = [{ result_paths: [], result_urls: ['https://x.mp4'] }];
  assert.equal(historyHasResults(items), true);
});

test('historyHasResults returns false when no results', () => {
  const items = [{ result_paths: [], result_urls: [] }];
  assert.equal(historyHasResults(items), false);
});

// ── historyItemLabel ───────────────────────────────────────────────────────

test('historyItemLabel with submit_id shows truncated id', () => {
  const item = { submit_id: 'abcdefgh1234' };
  assert.equal(historyItemLabel(item, 1), '第 1 次 · abcdefgh');
});

test('historyItemLabel includes execution model label', () => {
  assert.equal(historyItemLabel({
    submit_id: '',
    input_snapshot: { params: { model_version: 'seedance2.0fast' } },
  }, 1), '第 1 次 · Fast');
  assert.equal(historyItemLabel({
    submit_id: 'abcdefgh1234',
    input_snapshot: { params: { model_version: 'seedance2.0' } },
  }, 2), '第 2 次 · 标准 · abcdefgh');
});

test('historyItemLabel without submit_id shows index only', () => {
  const item = { submit_id: '' };
  assert.equal(historyItemLabel(item, 2), '第 2 次');
});

test('historyItemLabel includes collapsed retry count', () => {
  const item = { submit_id: '', retry_count: 5 };
  assert.equal(historyItemLabel(item, 2), '第 2 次 · 自动重试 5 次');
});

// ── isInterruptNotice ──────────────────────────────────────────────────────

test('isInterruptNotice returns true for "应用重启，查询中断"', () => {
  assert.equal(isInterruptNotice('应用重启，查询中断'), true);
});

test('isInterruptNotice returns true for plain "查询中断"', () => {
  assert.equal(isInterruptNotice('查询中断'), true);
});

test('isInterruptNotice returns true for strings containing "应用重启"', () => {
  assert.equal(isInterruptNotice('应用重启，任务从 querying 恢复'), true);
});

test('isInterruptNotice returns false for real error messages', () => {
  assert.equal(isInterruptNotice('ExceedConcurrencyLimit'), false);
  assert.equal(isInterruptNotice('network timeout'), false);
  assert.equal(isInterruptNotice('生成失败'), false);
});

test('isInterruptNotice returns false for empty or null', () => {
  assert.equal(isInterruptNotice(''), false);
  assert.equal(isInterruptNotice(null), false);
  assert.equal(isInterruptNotice(undefined), false);
});
