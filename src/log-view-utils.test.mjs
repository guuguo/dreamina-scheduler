import { describe, it } from 'node:test';
import assert from 'node:assert/strict';
import {
  normalizeLogEntry, deriveLogStats, deriveCategoryCounts,
  filterLogs, paginateLogs, deriveLogAssociations,
  buildLogExportPayload, findDefaultSelectedLogId,
  getQuickLocationFilters, deriveTaskOptions,
  LOG_LEVELS, LOG_SOURCES, LEVEL_VARIANT_MAP,
} from './log-view-utils.js';

// ── 测试数据 ──────────────────────────────────────────

const now = new Date();
const todayISO = now.toISOString();
const legacyString = '导入素材：test.png';
const structuredLog = {
  id: 'log-001',
  timestamp: todayISO,
  level: 'info',
  source: 'Asset',
  category: 'asset',
  eventType: 'import',
  message: '导入素材：test.png',
  detail: '',
  taskId: null,
  taskTitle: null,
  submitId: null,
  executionRecordId: null,
  errorDetail: null,
  rawOutput: null,
  stdout: null,
  stderr: null,
  module: null,
};

const errorLog = {
  id: 'log-002',
  timestamp: todayISO,
  level: 'error',
  source: 'Worker',
  category: 'task',
  eventType: 'submit',
  message: '提交任务失败',
  detail: 'api error: timeout',
  taskId: 'task-1',
  taskTitle: '测试任务',
  submitId: 'sub-001',
  executionRecordId: null,
  errorDetail: 'timeout',
  rawOutput: null,
  stdout: null,
  stderr: null,
  module: null,
};

const cliLog = {
  id: 'log-003',
  timestamp: todayISO,
  level: 'success',
  source: 'CLI',
  category: 'cli',
  eventType: 'install',
  message: 'CLI 安装成功',
  detail: '',
  taskId: null,
  taskTitle: null,
  submitId: null,
  executionRecordId: null,
  errorDetail: null,
  rawOutput: null,
  stdout: 'installed',
  stderr: '',
  module: null,
};

const retryLog = {
  id: 'log-004',
  timestamp: todayISO,
  level: 'warn',
  source: 'RetryPolicy',
  category: 'retry',
  eventType: 'retry',
  message: '并发限制重试',
  detail: '',
  taskId: 'task-1',
  taskTitle: '测试任务',
  submitId: null,
  executionRecordId: null,
  errorDetail: null,
  rawOutput: null,
  stdout: null,
  stderr: null,
  module: 'concurrency_guard',
};

function makeLogs() {
  return [structuredLog, errorLog, cliLog, retryLog].map(normalizeLogEntry);
}

// ── normalizeLogEntry ─────────────────────────────────

describe('normalizeLogEntry', () => {
  it('normalizes legacy string log', () => {
    const result = normalizeLogEntry(legacyString);
    assert.equal(result.level, 'info');
    assert.equal(result.source, 'System');
    assert.equal(result.category, 'system');
    assert.equal(result.eventType, 'legacy.string_log');
    assert.equal(result.message, '导入素材：test.png');
    assert.equal(result.detail, '导入素材：test.png');
    assert.equal(result.legacyString, true);
    assert.ok(result.id);
  });

  it('truncates long legacy string message to 120 chars', () => {
    const long = 'x'.repeat(200);
    const result = normalizeLogEntry(long);
    assert.equal(result.message.length, 121); // 120 + …
    assert.ok(result.message.endsWith('…'));
    assert.equal(result.detail.length, 200);
  });

  it('normalizes structured log with all fields', () => {
    const result = normalizeLogEntry(structuredLog);
    assert.equal(result.id, 'log-001');
    assert.equal(result.level, 'info');
    assert.equal(result.source, 'Asset');
    assert.equal(result.category, 'asset');
    assert.equal(result.message, '导入素材：test.png');
    assert.equal(result.legacyString, false);
  });

  it('handles snake_case field names from backend', () => {
    const snakeLog = { id: 'log-s', level: 'error', task_id: 't1', submit_id: 's1', event_type: 'test' };
    const result = normalizeLogEntry(snakeLog);
    assert.equal(result.taskId, 't1');
    assert.equal(result.submitId, 's1');
    assert.equal(result.eventType, 'test');
  });

  it('provides defaults for missing fields', () => {
    const result = normalizeLogEntry({});
    assert.equal(result.level, 'info');
    assert.equal(result.source, 'System');
    assert.equal(result.category, 'system');
    assert.equal(result.legacyString, false);
  });
});

// ── deriveLogStats ───────────────────────────────────

describe('deriveLogStats', () => {
  it('counts today, errors, warnings, infos', () => {
    const logs = makeLogs();
    const stats = deriveLogStats(logs, 500);
    assert.equal(stats.today, 4);
    assert.equal(stats.errors, 1);
    assert.equal(stats.warnings, 1);
    assert.equal(stats.infos, 2); // info + success
    assert.equal(stats.retention, '保留 500 条');
  });

  it('shows 无限制 when retentionCount is 0', () => {
    const stats = deriveLogStats([], 0);
    assert.equal(stats.retention, '无限制');
  });

  it('counts 0 for empty logs', () => {
    const stats = deriveLogStats([], 500);
    assert.equal(stats.today, 0);
    assert.equal(stats.errors, 0);
  });
});

// ── deriveCategoryCounts ─────────────────────────────

describe('deriveCategoryCounts', () => {
  it('counts logs per sidebar category', () => {
    const logs = makeLogs();
    const counts = deriveCategoryCounts(logs);
    assert.equal(counts.all, 4);
    assert.equal(counts.task, 2); // errorLog + retryLog
    assert.equal(counts.cli, 1);
    assert.equal(counts.error, 1);
    assert.equal(counts.system, 0);
    assert.equal(counts.retry, 1);
  });
});

// ── filterLogs ───────────────────────────────────────

describe('filterLogs', () => {
  it('returns all logs with no filters', () => {
    const logs = makeLogs();
    assert.equal(filterLogs(logs).length, 4);
  });

  it('filters by level', () => {
    const logs = makeLogs();
    assert.equal(filterLogs(logs, { level: 'error' }).length, 1);
    assert.equal(filterLogs(logs, { level: 'warn' }).length, 1);
  });

  it('filters by source', () => {
    const logs = makeLogs();
    assert.equal(filterLogs(logs, { source: 'CLI' }).length, 1);
    assert.equal(filterLogs(logs, { source: 'Worker' }).length, 1);
  });

  it('filters by taskId', () => {
    const logs = makeLogs();
    assert.equal(filterLogs(logs, { taskId: 'task-1' }).length, 2);
  });

  it('filters by search keyword', () => {
    const logs = makeLogs();
    assert.equal(filterLogs(logs, { search: 'CLI' }).length, 1);
    assert.equal(filterLogs(logs, { search: 'sub-001' }).length, 1);
    assert.equal(filterLogs(logs, { search: '测试任务' }).length, 2);
  });

  it('filters by category sidebar key', () => {
    const logs = makeLogs();
    assert.equal(filterLogs(logs, { category: 'error' }).length, 1);
    assert.equal(filterLogs(logs, { category: 'cli' }).length, 1);
    assert.equal(filterLogs(logs, { category: 'task' }).length, 2);
    assert.equal(filterLogs(logs, { category: 'retry' }).length, 1);
  });

  it('combines multiple filters', () => {
    const logs = makeLogs();
    const result = filterLogs(logs, { category: 'task', level: 'error' });
    assert.equal(result.length, 1);
    assert.equal(result[0].id, 'log-002');
  });

  it('filters by timeRange 1h', () => {
    const now = new Date();
    const recent = { ...structuredLog, timestamp: now.toISOString(), id: 'recent' };
    const old = { ...structuredLog, timestamp: new Date(now - 7200000).toISOString(), id: 'old' };
    const logs = [recent, old].map(normalizeLogEntry);
    assert.equal(filterLogs(logs, { timeRange: '1h' }).length, 1);
  });

  it('hides debug logs by default', () => {
    const debugLog = normalizeLogEntry({ id: 'log-dbg', level: 'debug', source: 'Scheduler', category: 'queue', message: '队列暂无到期任务' });
    const logs = [...makeLogs(), debugLog];
    assert.equal(filterLogs(logs).length, 4);
    assert.equal(filterLogs(logs, { level: 'debug' }).length, 1);
  });
});

// ── paginateLogs ─────────────────────────────────────

describe('paginateLogs', () => {
  it('paginates correctly', () => {
    const logs = Array.from({ length: 50 }, (_, i) => normalizeLogEntry({ id: `log-${i}`, level: 'info', message: `log ${i}` }));
    const p = paginateLogs(logs, 2, 20);
    assert.equal(p.page, 2);
    assert.equal(p.total, 50);
    assert.equal(p.totalPages, 3);
    assert.equal(p.items.length, 20);
    assert.equal(p.items[0].id, 'log-20');
  });

  it('clamps page to valid range', () => {
    const logs = Array.from({ length: 10 }, (_, i) => normalizeLogEntry({ id: `log-${i}`, level: 'info' }));
    const p = paginateLogs(logs, 99, 20);
    assert.equal(p.page, 1);
  });

  it('handles empty logs', () => {
    const p = paginateLogs([], 1, 20);
    assert.equal(p.total, 0);
    assert.equal(p.totalPages, 1);
    assert.equal(p.items.length, 0);
  });
});

// ── deriveLogAssociations ────────────────────────────

describe('deriveLogAssociations', () => {
  it('finds associated logs by taskId', () => {
    const logs = makeLogs();
    const target = logs.find((l) => l.id === 'log-002');
    const assoc = deriveLogAssociations(logs, target);
    assert.equal(assoc.length, 1);
    assert.equal(assoc[0].id, 'log-004');
  });

  it('finds associated logs by submitId', () => {
    const extraLog = normalizeLogEntry({ ...structuredLog, id: 'log-005', submitId: 'sub-001', category: 'task' });
    const logs = [...makeLogs(), extraLog];
    const target = logs.find((l) => l.id === 'log-002');
    const assoc = deriveLogAssociations(logs, target, 3);
    assert.ok(assoc.length >= 1);
  });

  it('returns empty for logs without taskId or submitId', () => {
    const logs = makeLogs();
    const target = logs.find((l) => l.id === 'log-001');
    const assoc = deriveLogAssociations(logs, target);
    assert.equal(assoc.length, 0);
  });

  it('excludes the target log itself', () => {
    const logs = makeLogs();
    const target = logs.find((l) => l.id === 'log-002');
    const assoc = deriveLogAssociations(logs, target);
    assert.ok(!assoc.some((l) => l.id === 'log-002'));
  });

  it('respects maxCount', () => {
    const logs = Array.from({ length: 10 }, (_, i) =>
      normalizeLogEntry({ id: `log-assoc-${i}`, level: 'info', taskId: 'task-1', category: 'task' })
    );
    const target = logs[0];
    const assoc = deriveLogAssociations(logs, target, 3);
    assert.equal(assoc.length, 3);
  });
});

// ── buildLogExportPayload ────────────────────────────

describe('buildLogExportPayload', () => {
  it('exports as JSON', () => {
    const logs = makeLogs().slice(0, 2);
    const json = buildLogExportPayload(logs, 'json');
    const parsed = JSON.parse(json);
    assert.equal(parsed.length, 2);
    assert.equal(parsed[0].id, 'log-001');
  });

  it('exports as text', () => {
    const logs = makeLogs().slice(0, 1);
    const text = buildLogExportPayload(logs, 'text');
    assert.ok(text.includes('[info]'));
    assert.ok(text.includes('导入素材'));
  });
});

// ── findDefaultSelectedLogId ─────────────────────────

describe('findDefaultSelectedLogId', () => {
  it('selects latest error log', () => {
    const logs = makeLogs();
    const id = findDefaultSelectedLogId(logs);
    assert.equal(id, 'log-002');
  });

  it('falls back to latest log when no error', () => {
    const logs = makeLogs().filter((l) => l.level !== 'error');
    const id = findDefaultSelectedLogId(logs);
    assert.equal(id, 'log-004'); // last in array
  });

  it('returns null for empty logs', () => {
    assert.equal(findDefaultSelectedLogId([]), null);
  });
});

// ── getQuickLocationFilters ──────────────────────────

describe('getQuickLocationFilters', () => {
  it('returns error filter for today_errors', () => {
    const f = getQuickLocationFilters('today_errors');
    assert.equal(f.level, 'error');
    assert.equal(f.timeRange, '24h');
  });

  it('returns 1h time range for last_1h', () => {
    const f = getQuickLocationFilters('last_1h');
    assert.equal(f.timeRange, '1h');
  });

  it('returns empty for unknown key', () => {
    const f = getQuickLocationFilters('unknown');
    assert.deepEqual(f, {});
  });
});

// ── deriveTaskOptions ────────────────────────────────

describe('deriveTaskOptions', () => {
  it('extracts unique task options from logs', () => {
    const logs = makeLogs();
    const options = deriveTaskOptions(logs);
    assert.equal(options.length, 1);
    assert.equal(options[0].id, 'task-1');
    assert.equal(options[0].title, '测试任务');
  });

  it('returns empty for logs without tasks', () => {
    const logs = [normalizeLogEntry(structuredLog)];
    assert.equal(deriveTaskOptions(logs).length, 0);
  });
});

// ── constants ────────────────────────────────────────

describe('constants', () => {
  it('LOG_LEVELS contains expected levels', () => {
    assert.deepEqual(LOG_LEVELS, ['error', 'warn', 'info', 'success', 'debug']);
  });

  it('LEVEL_VARIANT_MAP maps all levels', () => {
    for (const level of LOG_LEVELS) {
      assert.ok(LEVEL_VARIANT_MAP[level], `missing variant for ${level}`);
    }
  });
});
