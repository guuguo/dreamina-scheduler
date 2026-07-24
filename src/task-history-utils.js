/**
 * task-history-utils.js
 * 把 task.execution_records + legacy task.attempts/result_paths/result_urls
 * 统一派生成 UI 可展示的历史列表。
 */

const RETRY_ERROR_DETAILS = {
  ConcurrencyLimit: '并发任务仍在生成中，已自动排队等待下次重试。',
  Transient: '提交时遇到临时网络或平台错误，已自动排队等待下次重试。',
};

const FAILED_RETRY_ERROR_DETAILS = {
  ConcurrencyLimit: '并发任务仍在生成中，已自动排队等待下次重试。',
  Transient: '提交时遇到临时网络或平台错误，自动重试已达上限，已标记失败。',
};

function isConcurrencyError(errorKind, errorDetail) {
  if (errorKind === 'ConcurrencyLimit') return true;
  if (!errorDetail) return false;
  return (
    errorDetail.includes('ExceedConcurrencyLimit') ||
    errorDetail.includes('ret=1310') ||
    errorDetail.includes('并发')
  );
}

function retryErrorKind(errorKind, errorDetail) {
  if (isConcurrencyError(errorKind, errorDetail)) return 'ConcurrencyLimit';
  if (errorKind === 'Transient') return 'Transient';
  return '';
}

export function executionModelVersion(item) {
  const snapshotVersion = item?.input_snapshot?.params?.model_version;
  if (snapshotVersion) return snapshotVersion;
  const modelArg = (item?.command_preview || []).find((arg) => String(arg).startsWith('--model_version='));
  return modelArg ? String(modelArg).slice('--model_version='.length) : '';
}

export function executionModelLabel(item) {
  const version = executionModelVersion(item);
  if (version === 'seedance2.0fast') return 'Fast';
  if (version === 'seedance2.0') return '标准';
  return version;
}

function compactRetryErrorDetail(errorKind, errorDetail) {
  const kind = retryErrorKind(errorKind, errorDetail);
  if (kind && RETRY_ERROR_DETAILS[kind]) return RETRY_ERROR_DETAILS[kind];
  return errorDetail || '';
}

function compactSubmitErrorDetail(status, errorKind, errorDetail) {
  const kind = retryErrorKind(errorKind, errorDetail);
  if (!kind) return errorDetail || '';
  if (status === 'failed') return FAILED_RETRY_ERROR_DETAILS[kind];
  return RETRY_ERROR_DETAILS[kind];
}

function collapseRetryWaitRecords(items) {
  const collapsed = [];
  const retryIndexes = new Map();

  for (const item of items) {
    const canCollapse = ['retry_wait', 'failed'].includes(item.status);
    const kind = canCollapse ? retryErrorKind(item.error_kind, item.error_detail) : '';
    if (!kind) {
      collapsed.push(item);
      continue;
    }

    const key = `retry_wait:${kind}:${executionModelVersion(item)}`;
    const existingIndex = retryIndexes.get(key);
    const compactItem = {
      ...item,
      error_kind: kind,
      error_detail: compactSubmitErrorDetail(item.status, kind, item.error_detail),
      retry_count: 1,
    };

    if (existingIndex === undefined) {
      retryIndexes.set(key, collapsed.length);
      collapsed.push(compactItem);
      continue;
    }

    const existing = collapsed[existingIndex];
    const useIncoming = (item.started_at || '') >= (existing.started_at || '');
    const nextStatus = useIncoming ? item.status : existing.status;
    collapsed[existingIndex] = {
      ...(useIncoming ? compactItem : existing),
      retry_count: Number(existing.retry_count || 1) + 1,
      error_kind: kind,
      error_detail: compactSubmitErrorDetail(nextStatus, kind, ''),
    };
  }

  return collapsed;
}

/**
 * 从单条 task 派生出执行历史列表，最新在前。
 * @param {object} task - ScheduledTask
 * @returns {Array<object>} historyItems
 */
export function deriveTaskHistory(task) {
  if (!task) return [];

  const items = [];

  // 优先使用持久化的 execution_records
  if (task.execution_records && task.execution_records.length > 0) {
    for (const rec of task.execution_records) {
      items.push({
        id: rec.id,
        submit_id: rec.submit_id || '',
        status: rec.status || 'unknown',
        started_at: rec.started_at || '',
        finished_at: rec.finished_at || '',
        result_paths: rec.result_paths || [],
        result_urls: rec.result_urls || [],
        query_records: rec.query_records || rec.attempts || [],
        // 兼容旧字段名
        attempts: rec.query_records || rec.attempts || [],
        error_kind: rec.error_kind || '',
        error_detail: rec.error_detail || '',
        command_preview: rec.command_preview || [],
        input_snapshot: rec.input_snapshot || null,
        source: 'record',
      });
    }
    // 最新在前
    items.splice(0, items.length, ...collapseRetryWaitRecords(items));
    items.sort((a, b) => (b.started_at > a.started_at ? 1 : -1));
    return items;
  }

  // legacy 兼容：只有顶层 attempts/results 时，合成一条 legacy 记录
  const hasLegacyData =
    (task.attempts && task.attempts.length > 0) ||
    (task.result_paths && task.result_paths.length > 0) ||
    (task.result_urls && task.result_urls.length > 0) ||
    task.submit_id;

  if (hasLegacyData) {
    items.push({
      id: `legacy-${task.id}`,
      submit_id: task.submit_id || '',
      status: task.status || 'unknown',
      started_at: task.created_at || '',
      finished_at: task.finished_at || '',
      result_paths: task.result_paths || [],
      result_urls: task.result_urls || [],
      query_records: task.attempts || [],
      attempts: task.attempts || [],
      error_kind: '',
      error_detail: task.last_error || '',
      command_preview: task.command_preview || [],
      input_snapshot: null,
      source: 'legacy',
    });
  }

  return items;
}

function resultCount(item) {
  const pathCount = new Set(item?.result_paths || []).size;
  const urlCount = new Set(item?.result_urls || []).size;
  return Math.max(pathCount, urlCount);
}

function localDayStart(value) {
  const date = new Date(value);
  if (Number.isNaN(date.getTime())) return null;
  date.setHours(0, 0, 0, 0);
  return date;
}

function localDayKey(value) {
  return [
    value.getFullYear(),
    String(value.getMonth() + 1).padStart(2, '0'),
    String(value.getDate()).padStart(2, '0'),
  ].join('-');
}

/**
 * 按执行完成时间统计生成的视频结果数量。
 * 同一次执行的本地路径与远程 URL 通常指向同一批结果，因此取两者较大值避免重复计数。
 */
export function buildGenerationStats(tasks = [], now = new Date()) {
  const todayStart = localDayStart(now);
  const emptyRange = () => ({ total: 0, standard: 0, fast: 0, records: [] });
  if (!todayStart) {
    return {
      today: 0,
      yesterday: 0,
      last7Days: 0,
      total: 0,
      standard: 0,
      fast: 0,
      days: [],
      ranges: {
        today: emptyRange(),
        yesterday: emptyRange(),
        last7Days: emptyRange(),
      },
    };
  }
  const yesterdayStart = new Date(todayStart);
  yesterdayStart.setDate(yesterdayStart.getDate() - 1);
  const last7DaysStart = new Date(todayStart);
  last7DaysStart.setDate(last7DaysStart.getDate() - 6);
  const tomorrowStart = new Date(todayStart);
  tomorrowStart.setDate(tomorrowStart.getDate() + 1);
  const days = Array.from({ length: 7 }, (_, index) => {
    const date = new Date(todayStart);
    date.setDate(date.getDate() - index);
    return {
      key: localDayKey(date),
      label: index === 0 ? '今天' : index === 1 ? '昨天' : `${date.getMonth() + 1}月${date.getDate()}日`,
      count: 0,
      standard: 0,
      fast: 0,
      records: [],
    };
  });
  const dayByKey = new Map(days.map((day) => [day.key, day]));

  const stats = {
    today: 0,
    yesterday: 0,
    last7Days: 0,
    total: 0,
    standard: 0,
    fast: 0,
    days,
    ranges: {
      today: emptyRange(),
      yesterday: emptyRange(),
      last7Days: emptyRange(),
    },
  };

  const addToRange = (range, record) => {
    range.total += record.count;
    range[record.modelKind] += record.count;
    range.records.push(record);
  };

  for (const task of tasks || []) {
    for (const item of deriveTaskHistory(task)) {
      if (item.status !== 'succeeded') continue;
      const count = resultCount(item);
      if (!count) continue;
      const modelVersion = executionModelVersion(item) || task?.params?.model_version || '';
      const modelKind = modelVersion === 'seedance2.0fast' ? 'fast' : 'standard';
      const record = {
        id: item.id,
        taskId: task.id,
        taskTitle: task.title || '未命名任务',
        finishedAt: item.finished_at || '',
        count,
        modelKind,
        modelVersion: modelVersion || (modelKind === 'fast' ? 'seedance2.0fast' : 'seedance2.0'),
      };
      stats.total += count;
      stats[modelKind] += count;
      const finishedAt = new Date(item.finished_at || '');
      if (Number.isNaN(finishedAt.getTime())) continue;
      if (finishedAt >= todayStart && finishedAt < tomorrowStart) {
        stats.today += count;
        addToRange(stats.ranges.today, record);
      }
      if (finishedAt >= yesterdayStart && finishedAt < todayStart) {
        stats.yesterday += count;
        addToRange(stats.ranges.yesterday, record);
      }
      if (finishedAt >= last7DaysStart && finishedAt < tomorrowStart) {
        stats.last7Days += count;
        addToRange(stats.ranges.last7Days, record);
        const day = dayByKey.get(localDayKey(finishedAt));
        if (day) {
          day.count += count;
          day[record.modelKind] += count;
          day.records.push(record);
        }
      }
    }
  }
  Object.values(stats.ranges).forEach((range) => {
    range.records.sort((a, b) => (b.finishedAt || '').localeCompare(a.finishedAt || ''));
  });
  stats.days.forEach((day) => {
    day.records.sort((a, b) => (b.finishedAt || '').localeCompare(a.finishedAt || ''));
  });
  return stats;
}

/**
 * 派生当前查看/操作的执行记录。
 * 优先级：显式选中的执行记录 > task.submit_id 对应记录 > 最新执行记录/legacy。
 * @param {object} task
 * @param {string|null} selectedExecutionId
 * @returns {object|null}
 */
export function deriveCurrentExecutionRecord(task, selectedExecutionId = null) {
  const history = deriveTaskHistory(task);
  if (!history.length) return null;

  if (selectedExecutionId) {
    const selected = history.find((item) => item.id === selectedExecutionId);
    if (selected) return selected;
  }

  const status = task?.status || 'draft';
  const hasActiveExecution = [
    'submitting',
    'submitted',
    'querying',
    'retry_wait',
    'succeeded',
    'failed',
    'blocked',
  ].includes(status);
  if (!hasActiveExecution) return null;

  if (task?.submit_id) {
    const currentSubmit = history.find((item) => item.submit_id === task.submit_id);
    if (currentSubmit) return currentSubmit;
  }

  return history[0];
}

/**
 * 返回当前执行记录自己的查询记录，避免多次执行的查询日志混在一起。
 * @param {object} task
 * @param {string|null} selectedExecutionId
 * @returns {Array<object>}
 */
export function deriveCurrentQueryRecords(task, selectedExecutionId = null) {
  const current = deriveCurrentExecutionRecord(task, selectedExecutionId);
  return current?.query_records || current?.attempts || [];
}

/**
 * 历史记录中是否存在成功结果
 * @param {Array} historyItems
 */
export function historyHasResults(historyItems) {
  return historyItems.some(
    (item) => item.result_paths.length > 0 || item.result_urls.length > 0,
  );
}

/**
 * 格式化历史记录的摘要标签（用于列表头部）
 * @param {object} item
 * @param {number} index - 1-based
 */
export function historyItemLabel(item, index) {
  const parts = [`第 ${index} 次`];
  const modelLabel = executionModelLabel(item);
  if (modelLabel) parts.push(modelLabel);
  if (Number(item?.retry_count || 0) > 1) parts.push(`自动重试 ${item.retry_count} 次`);
  else if (item.submit_id) parts.push(item.submit_id.slice(0, 8));
  return parts.join(' · ');
}

/**
 * 判断执行记录的 error_detail 是否属于本地轮询中断 notice（不代表远程任务失败）。
 * 中断 notice 不以红色异常展示，UI 应提示"可继续查询"。
 * @param {string} errorDetail
 * @returns {boolean}
 */
export function isInterruptNotice(errorDetail) {
  if (!errorDetail) return false;
  const INTERRUPT_PATTERNS = ['查询中断', '应用重启'];
  return INTERRUPT_PATTERNS.some((pattern) => errorDetail.includes(pattern));
}
