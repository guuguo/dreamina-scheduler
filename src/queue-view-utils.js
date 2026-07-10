/**
 * Pure utility functions for the queue center view.
 * All functions are side-effect free and testable without a DOM.
 */

const WAITING_STATUSES = ['draft', 'queued', 'scheduled'];
const RUNNING_STATUSES = ['submitting', 'submitted', 'querying'];
const RETRY_STATUSES = ['retry_wait'];
const DONE_STATUSES = ['succeeded'];
const FAILED_STATUSES = ['failed', 'schema_error'];
const PAUSED_STATUSES = ['paused'];

/** Returns the tab group key for a given task status. */
export function getStatusTabGroup(status) {
  if (WAITING_STATUSES.includes(status)) return 'waiting';
  if (RUNNING_STATUSES.includes(status)) return 'running';
  if (RETRY_STATUSES.includes(status)) return 'retry';
  if (DONE_STATUSES.includes(status)) return 'done';
  if (FAILED_STATUSES.includes(status)) return 'failed';
  if (PAUSED_STATUSES.includes(status)) return 'paused';
  return 'other';
}

/**
 * Filter tasks by search query, status tab, and model.
 * @param {object[]} tasks
 * @param {{ searchQuery?: string, statusTab?: string, modelFilter?: string }} opts
 */
export function filterTasks(tasks, { searchQuery = '', statusTab = 'all', modelFilter = 'all' } = {}) {
  let result = tasks;

  if (statusTab !== 'all') {
    result = result.filter((t) => getStatusTabGroup(t.status) === statusTab);
  }

  if (modelFilter !== 'all') {
    result = result.filter((t) => (t.params?.model_version || '') === modelFilter);
  }

  if (searchQuery.trim()) {
    const q = searchQuery.trim().toLowerCase();
    result = result.filter(
      (t) =>
        (t.title || '').toLowerCase().includes(q) ||
        (t.prompt || '').toLowerCase().includes(q) ||
        (t.submit_id || '').toLowerCase().includes(q)
    );
  }

  return result;
}

/**
 * Sort tasks by creation time.
 * @param {object[]} tasks
 * @param {'created_desc'|'created_asc'} sortBy
 */
export function sortTasks(tasks, sortBy = 'created_desc') {
  const arr = [...tasks];
  switch (sortBy) {
    case 'created_desc':
      return arr.sort((a, b) => ((b.created_at || '') > (a.created_at || '') ? 1 : -1));
    case 'created_asc':
      return arr.sort((a, b) => ((a.created_at || '') > (b.created_at || '') ? 1 : -1));
    default:
      return arr;
  }
}

/**
 * Paginate a task array.
 * @returns {{ items, total, totalPages, page, startIndex, endIndex }}
 */
export function paginateTasks(tasks, page, pageSize) {
  const total = tasks.length;
  const totalPages = Math.max(1, Math.ceil(total / pageSize));
  const safePage = Math.max(1, Math.min(page, totalPages));
  const startIndex = (safePage - 1) * pageSize;
  const endIndex = Math.min(startIndex + pageSize, total);
  return { items: tasks.slice(startIndex, endIndex), total, totalPages, page: safePage, startIndex, endIndex };
}

/** Format "1–8 / 25" pagination label. */
export function formatPaginationLabel(startIndex, endIndex, total) {
  if (total === 0) return '0 条';
  return `${startIndex + 1}–${endIndex} / ${total}`;
}

const FLOW_STEP_DEFS = [
  { key: 'pending', label: '待提交' },
  { key: 'submitting', label: '提交中' },
  { key: 'submitted', label: '排队中' },
  { key: 'processing', label: '处理中' },
  { key: 'completed', label: '完成' },
];

/**
 * Derive flow step states from task status.
 * Each step: { key, label, state: 'done'|'active'|'error'|'pending' }
 */
export function deriveTaskFlowSteps(task) {
  const status = task?.status || 'draft';
  const isFailed = FAILED_STATUSES.includes(status);
  const isRunning = RUNNING_STATUSES.includes(status);

  let activeIndex;
  if (['draft', 'queued', 'scheduled', 'paused'].includes(status)) activeIndex = 0;
  else if (status === 'submitting') activeIndex = 1;
  else if (['submitted', 'retry_wait'].includes(status)) activeIndex = 2;
  else if (status === 'querying') activeIndex = 3;
  else if (status === 'succeeded') activeIndex = FLOW_STEP_DEFS.length; // all done
  else if (isFailed) activeIndex = 2;
  else activeIndex = 0;

  return FLOW_STEP_DEFS.map((step, i) => {
    if (i < activeIndex) return { ...step, state: 'done', spinning: false };
    if (i === activeIndex) return { ...step, state: isFailed ? 'error' : 'active', spinning: isRunning };
    return { ...step, state: 'pending', spinning: false };
  });
}

export function canDeleteTask(task) {
  return !['submitting', 'submitted', 'querying'].includes(task?.status);
}

export function getTaskResultItems(task) {
  const paths = (task?.result_paths || []).map((value) => ({
    kind: 'path',
    value,
    label: basename(value),
  }));
  // 本地文件已存在时不重复展示远程 URL
  if (paths.length > 0) return paths;
  const urls = (task?.result_urls || []).map((value) => ({
    kind: 'url',
    value,
    label: basename(value),
  }));
  return urls;
}

export function getTaskHitResources(task, assetById) {
  if (!task || !assetById) return [];
  const resources = [];
  let index = 0;
  for (const id of task.image_asset_ids || []) {
    const asset = assetById.get(id);
    if (asset) {
      const isTempImage = isTemporaryImageAsset(asset);
      resources.push({
        type: 'image',
        displayType: isTempImage ? 'temp_image' : 'role_image',
        label: isTempImage ? '临时参考图' : '角色图片',
        asset,
        displayName: asset.name || '',
        subName: asset.name || '',
        mentionIndex: ++index,
      });
    }
  }
  for (const id of task.audio_asset_ids || []) {
    const asset = assetById.get(id);
    if (asset) {
      resources.push({
        type: 'audio',
        displayType: 'role_audio',
        label: '音频素材',
        asset,
        displayName: asset.name || '',
        subName: asset.name || '',
        mentionIndex: ++index,
      });
    }
  }
  return resources;
}

function isTemporaryImageAsset(asset) {
  const tags = new Set(asset?.tags || []);
  return tags.has('temp_image') || tags.has('temporary') || tags.has('clipboard');
}

/**
 * Derive a user-facing display name for a resource.
 * Prefers mention label (from prompt_doc mention attrs) over asset name.
 */
export function displayName(asset, mentionAttrs) {
  return mentionAttrs?.label || asset?.name || '';
}

export function getCommandPreviewPresentation(commandText) {
  const hasCommand = Boolean(String(commandText || '').trim());
  return {
    hasCommand,
    shouldRenderInlineBlock: false,
    actionLabel: '查看命令',
    hint: hasCommand ? '已生成命令预览，点击查看完整命令' : '',
  };
}

export function deriveTaskDispatchInfo(task, {
  nowMs = Date.now(),
  schedulerTickSeconds = 30,
  concurrencyCooldownUntil = '',
} = {}) {
  const status = task?.status || 'draft';
  const attemptCount = Number(task?.attempt_count || 0);
  const plannedSubmitCount = Math.max(1, Number(task?.planned_submit_count || 1));
  const concurrencyRetryCount = Number(task?.concurrency_retry_count || 0);
  const concurrencyRetryText = concurrencyRetryCount > 0
    ? `已等待 ${concurrencyRetryCount} 轮，持续重试`
    : '尚未触发并发等待';
  const nextRunAt = task?.next_run_at || (status === 'scheduled' ? task?.scheduled_at : '');
  const nextTime = nextRunAt ? new Date(nextRunAt).getTime() : NaN;
  const isFutureNextRun = Number.isFinite(nextTime) && nextTime > nowMs;
  const isDueNextRun = Number.isFinite(nextTime) && nextTime <= nowMs;
  const cooldownTime = concurrencyCooldownUntil ? new Date(concurrencyCooldownUntil).getTime() : NaN;
  const hasFutureConcurrencyCooldown = Number.isFinite(cooldownTime) && cooldownTime > nowMs;
  const safeSchedulerTickSeconds = Math.max(1, Number(schedulerTickSeconds || 30));
  const fallbackCheckAtMs = nowMs + safeSchedulerTickSeconds * 1000;
  const fallbackCheckAt = new Date(fallbackCheckAtMs).toISOString();
  const fallbackWithinText = formatDispatchWithin(safeSchedulerTickSeconds);
  const fallbackCheckText = `约 ${fallbackWithinText}检查`;
  const fallbackCheckSummary = `${formatDispatchClock(fallbackCheckAtMs)} 前后`;

  if (status === 'scheduled') {
    return {
      status,
      attemptCount,
      plannedSubmitCount,
      concurrencyRetryCount,
      concurrencyRetryText,
      nextLabel: isDueNextRun ? '下次尝试' : '开始时间',
      nextAt: nextRunAt || '',
      nextText: nextRunAt ? '' : fallbackCheckText,
      reason: isDueNextRun ? '已到预定时间，等待调度补偿' : '等待预定开始时间',
      compactText: `尝试 ${attemptCount} 次 · 等待开始`,
    };
  }

  if (status === 'retry_wait') {
    if (isDueNextRun && hasFutureConcurrencyCooldown) {
      return {
        status,
        attemptCount,
        plannedSubmitCount,
        concurrencyRetryCount,
        concurrencyRetryText,
        nextLabel: '并发冷却',
        nextAt: concurrencyCooldownUntil,
        nextText: '',
        reason: '其他任务仍在并发冷却，冷却结束后继续调度',
        compactText: `尝试 ${attemptCount} 次 · 等待并发冷却`,
        blockedByConcurrencyCooldown: true,
      };
    }
    if (isDueNextRun) {
      return {
        status,
        attemptCount,
        plannedSubmitCount,
        concurrencyRetryCount,
        concurrencyRetryText,
        nextLabel: '等待调度',
        nextAt: '',
        nextText: '冷却已结束',
        reason: '重试冷却已结束，等待调度',
        compactText: `尝试 ${attemptCount} 次 · 等待调度`,
      };
    }
    return {
      status,
      attemptCount,
      plannedSubmitCount,
      concurrencyRetryCount,
      concurrencyRetryText,
      nextLabel: '下次重试',
      nextAt: nextRunAt || '',
      nextText: nextRunAt ? '' : fallbackCheckText,
      reason: isFutureNextRun ? '等待重试冷却结束' : '重试冷却已结束，等待调度',
      compactText: `尝试 ${attemptCount} 次 · 等待重试`,
    };
  }

  if (status === 'queued') {
    const routeLabel = task?.params?.model_version === 'seedance2.0fast' ? 'Fast' : '标准';
    const nextSummary = nextRunAt && isFutureNextRun
      ? `${formatDispatchClock(nextTime)} 前后`
      : fallbackCheckSummary;
    return {
      status,
      attemptCount,
      plannedSubmitCount,
      concurrencyRetryCount,
      concurrencyRetryText,
      nextLabel: nextRunAt ? '下次尝试' : '下次检查',
      nextAt: nextRunAt || fallbackCheckAt,
      nextText: isDueNextRun ? '即将尝试' : fallbackCheckText,
      nextSummary,
      reason: nextRunAt && isFutureNextRun ? `等待调度窗口，预计 ${nextSummary}走${routeLabel}` : `预计 ${nextSummary}调度，走${routeLabel}车道`,
      compactText: `尝试 ${attemptCount} 次 · ${nextRunAt && isFutureNextRun ? nextSummary : fallbackCheckText}`,
    };
  }

  return {
    status,
    attemptCount,
    plannedSubmitCount,
    concurrencyRetryCount,
    concurrencyRetryText,
    nextLabel: '下次尝试',
    nextAt: '',
    nextText: '-',
    reason: '',
    compactText: attemptCount ? `尝试 ${attemptCount} 次` : '',
  };
}

function formatDispatchWithin(seconds) {
  const safeSeconds = Math.max(1, Math.ceil(Number(seconds || 0)));
  if (safeSeconds < 60) return `${safeSeconds} 秒内`;
  const minutes = Math.ceil(safeSeconds / 60);
  if (minutes < 60) return `${minutes} 分钟内`;
  const hours = Math.floor(minutes / 60);
  const remainMinutes = minutes % 60;
  if (remainMinutes === 0) return `${hours} 小时内`;
  return `${hours} 小时 ${remainMinutes} 分钟内`;
}

function formatDispatchClock(ms) {
  const date = new Date(ms);
  if (Number.isNaN(date.getTime())) return '';
  return `${String(date.getHours()).padStart(2, '0')}:${String(date.getMinutes()).padStart(2, '0')}`;
}

function basename(value) {
  const clean = String(value || '').split('?')[0].split('#')[0];
  return clean.split(/[\\/]/).filter(Boolean).pop() || String(value || '结果视频');
}

/**
 * Derive task execution progress.
 * @returns {{ percent: number, stage: string }}
 */
export function deriveTaskProgress(task) {
  const status = task?.status || 'draft';
  const attemptCount = task?.attempt_count || 0;

  const map = {
    draft: { percent: 0, stage: '草稿' },
    queued: { percent: 8, stage: '等待调度' },
    scheduled: { percent: 8, stage: '等待预定时间' },
    paused: { percent: 8, stage: '已暂停' },
    submitting: { percent: 30, stage: '提交中' },
    submitted: { percent: 55, stage: '已提交，等待结果' },
    querying: { percent: 70, stage: '查询结果中' },
    retry_wait: { percent: 35, stage: `等待重试（第 ${attemptCount} 次）` },
    succeeded: { percent: 100, stage: '执行成功' },
    failed: { percent: 0, stage: '执行失败' },
  };

  return map[status] ?? { percent: 0, stage: status };
}

/** Collect unique model version strings from tasks. */
export function getModelOptions(tasks) {
  const models = new Set();
  for (const t of tasks) {
    if (t.params?.model_version) models.add(t.params.model_version);
  }
  return Array.from(models).sort();
}

/** Derive queue stat counts from task array. */
export function deriveQueueStats(tasks) {
  const stats = { waiting: 0, running: 0, retry: 0, done: 0, failed: 0, paused: 0 };
  for (const t of tasks) {
    const group = getStatusTabGroup(t.status);
    if (group in stats) stats[group]++;
  }
  return stats;
}
