export function resolveScheduleAt(options = {}, now = new Date()) {
  const mode = options.mode || 'immediate';
  if (mode === 'immediate') return null;

  if (mode === 'relative') {
    const hours = Math.max(1, Number(options.hours || 1));
    return new Date(now.getTime() + hours * 60 * 60 * 1000).toISOString();
  }

  if (mode === 'dayTime') {
    const date = new Date(now);
    if (options.day === 'tomorrow') date.setDate(date.getDate() + 1);
    const [hour, minute] = parseTimeValue(options.time || '02:00');
    date.setHours(hour, minute, 0, 0);
    return date.toISOString();
  }

  if (mode === 'custom') {
    if (!options.customValue) return '';
    return new Date(options.customValue).toISOString();
  }

  return null;
}

export function buildBatchSchedulePlan(taskIds = [], { startAt, intervalMinutes = 30 } = {}) {
  const startMs = new Date(startAt).getTime();
  const intervalMs = Math.max(1, Number(intervalMinutes || 30)) * 60 * 1000;
  return taskIds.map((taskId, index) => ({
    taskId,
    scheduledAt: new Date(startMs + index * intervalMs).toISOString(),
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

export function formatSchedulePlanSummary(plan = []) {
  if (!plan.length) return '未选择任务';
  const first = new Date(plan[0].scheduledAt);
  const last = new Date(plan[plan.length - 1].scheduledAt);
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
