export function resolveScheduleAt(options = {}, now = new Date()) {
  const mode = options.mode || 'immediate';
  if (mode === 'immediate') return null;

  if (mode === 'relative') {
    const hours = Math.max(1, Number(options.hours || 1));
    return new Date(now.getTime() + hours * 60 * 60 * 1000).toISOString();
  }

  if (mode === 'dayTime') {
    const date = new Date(now);
    const [hour, minute] = parseTimeValue(options.time || '02:00');
    date.setHours(hour, minute, 0, 0);
    if (options.day === 'tomorrow' || (options.day === 'auto' && date.getTime() <= now.getTime())) {
      date.setDate(date.getDate() + 1);
    }
    return date.toISOString();
  }

  if (mode === 'custom') {
    if (!options.customValue) return '';
    return new Date(options.customValue).toISOString();
  }

  return null;
}

export function buildBatchSchedulePlan(taskIds = [], { startAt, intervalMinutes = 0 } = {}) {
  const startMs = new Date(startAt).getTime();
  const intervalMs = Math.max(0, Number(intervalMinutes || 0)) * 60 * 1000;
  return taskIds.map((taskId, index) => ({
    taskId,
    scheduledAt: new Date(startMs + index * intervalMs).toISOString(),
  }));
}

export function buildBatchQueuePlan(taskIds = [], { startAt = null, intervalMinutes = 0, now = new Date() } = {}) {
  const intervalMs = Math.max(0, Number(intervalMinutes || 0)) * 60 * 1000;
  if (!startAt && intervalMs === 0) {
    return taskIds.map((taskId) => ({ taskId, scheduledAt: null }));
  }
  const startMs = startAt ? new Date(startAt).getTime() : now.getTime();
  return taskIds.map((taskId, index) => ({
    taskId,
    scheduledAt: !startAt && index === 0
      ? null
      : new Date(startMs + index * intervalMs).toISOString(),
  }));
}

export function canScheduleTask(task) {
  if (task?.status === 'submitted' && task?.auto_query_stopped) return true;
  return [
    'draft',
    'queued',
    'scheduled',
    'paused',
    'retry_wait',
    'failed',
    'succeeded',
  ].includes(task?.status);
}

export function canUseAlternatingFastQueue(tasks = []) {
  return tasks.length > 0
    && tasks.every((task) => task?.params?.model_version === 'seedance2.0');
}

export function formatSchedulePlanSummary(plan = []) {
  if (!plan.length) return '未选择任务';
  const scheduledItems = plan.filter((item) => item.scheduledAt);
  if (!scheduledItems.length) {
    return `${plan.length} 个任务 · 立即连续排队`;
  }
  const first = new Date(scheduledItems[0].scheduledAt);
  const last = new Date(scheduledItems[scheduledItems.length - 1].scheduledAt);
  if (!plan[0].scheduledAt) {
    return `${plan.length} 个任务 · 立即开始，排布至 ${last.toLocaleString()}`;
  }
  if (first.getTime() === last.getTime()) {
    return `${plan.length} 个任务 · 从 ${first.toLocaleString()} 连续排队`;
  }
  return `${plan.length} 个任务 · ${first.toLocaleString()} 至 ${last.toLocaleString()}`;
}

export function resolvePrepareGenerateOperation({ scheduledAt } = {}) {
  return scheduledAt
    ? { type: 'schedule', scheduledAt }
    : { type: 'submit', scheduledAt: null };
}

function parseTimeValue(value) {
  const [hour, minute] = String(value).split(':').map((part) => Number(part));
  return [
    Number.isFinite(hour) ? Math.min(23, Math.max(0, hour)) : 2,
    Number.isFinite(minute) ? Math.min(59, Math.max(0, minute)) : 0,
  ];
}
