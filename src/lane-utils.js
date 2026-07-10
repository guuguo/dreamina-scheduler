/**
 * lane-utils.js
 * 车道状态前端推导工具。
 * — 后端 laneStatus 不可用时用于降级；
 * — 辅助计算任务级别的路由信息（每个任务走哪个车道）。
 */

const FAST_MODEL = 'seedance2.0fast';
const STANDARD_MODEL = 'seedance2.0';

/** 等待队列中的状态 */
const WAITING_STATUSES = ['draft', 'queued', 'scheduled', 'retry_wait'];
const LOCAL_LANE_STATUSES = ['queued', 'retry_wait'];

/**
 * 从一个模型版本字符串推导车道。
 * @param {string} modelVersion
 * @returns {'standard'|'fast'}
 */
export function modelToQueueKind(modelVersion) {
  return modelVersion === FAST_MODEL ? 'fast' : 'standard';
}

/**
 * 车道显示名称。
 * @param {'standard'|'fast'} kind
 * @returns {string}
 */
export function laneLabel(kind) {
  return kind === 'fast' ? 'Fast' : '标准';
}

/**
 * 车道模型版本。
 */
export function laneModelVersion(kind) {
  return kind === 'fast' ? FAST_MODEL : STANDARD_MODEL;
}

/**
 * 获取车道远端占用展示。
 * 并发冷却虽然不一定有本地 submit_id，但通常说明网页端或远端槽位仍在占用，
 * 不能展示为“空闲”。
 */
export function getLaneRemoteOccupancyDisplay(lane) {
  if (!lane) {
    return {
      value: '—',
      copy: '暂无车道数据',
      tone: 'idle',
    };
  }
  if (lane.enabled === false) {
    return {
      value: '已关闭',
      copy: '不会向这条车道提交新任务',
      tone: 'idle',
    };
  }
  if (lane.queuePosition != null) {
    return {
      value: `#${lane.queuePosition}${lane.queueLength != null ? ` / ${lane.queueLength}` : ''}`,
      copy: lane.currentTaskTitle || '远端队列中，等待生成推进。',
      tone: 'warn',
    };
  }
  if (lane.isActive) {
    return {
      value: '占用中',
      copy: lane.currentTaskTitle || '本地提交已占用远端槽位。',
      tone: 'warn',
    };
  }
  if (lane.isCoolingDown) {
    return {
      value: '远端占位中',
      copy: lane.cooldownReason || '远端并发位未释放，到点后自动重试。',
      tone: 'warn',
    };
  }
  return {
    value: '空闲',
    copy: '当前没有检测到远端占用',
    tone: 'idle',
  };
}

/**
 * 判断错误是否为并发限制。
 */
export function isConcurrencyLimitError(errorText) {
  if (!errorText) return false;
  return (
    errorText.includes('ExceedConcurrencyLimit') ||
    errorText.includes('ret=1310') ||
    errorText.includes('并发')
  );
}

/**
 * 判断任务是否在远端活跃（submitting / submitted / querying 且未停止自动查询）。
 */
export function isTaskRemotelyActive(task) {
  if (!task) return false;
  if (task.status === 'submitting') return true;
  if ((task.status === 'querying' || task.status === 'submitted') &&
      task.submit_id && task.submit_id.trim() &&
      !task.auto_query_stopped) return true;
  return false;
}

function isActiveExecutionRecord(record) {
  return ['querying', 'submitted'].includes(record?.status) &&
    record?.submit_id && String(record.submit_id).trim() &&
    !String(record.finished_at || '').trim();
}

function executionRecordQueueKind(record) {
  return modelToQueueKind(record?.input_snapshot?.params?.model_version || '');
}

function parseQueueInfoFromRecord(record) {
  const attempts = record?.query_records || record?.attempts || [];
  for (const attempt of [...attempts].reverse()) {
    try {
      const parsed = JSON.parse(attempt?.stdout || '');
      if (parsed?.queue_info) return parsed.queue_info;
    } catch {
      // ignore malformed query output
    }
  }
  return null;
}

function recordNextCheckAt(record, nowMs) {
  const attempts = record?.query_records || record?.attempts || [];
  const last = attempts[attempts.length - 1];
  const rawTime = last?.finished_at || last?.started_at || '';
  const lastMs = rawTime ? new Date(rawTime).getTime() : NaN;
  if (!Number.isFinite(lastMs)) return new Date(nowMs).toISOString();
  return new Date(lastMs + 60 * 1000).toISOString();
}

function taskNextCheckAt(task, nowMs) {
  const lastQuery = task?.last_auto_query_at;
  if (!lastQuery) return new Date(nowMs).toISOString();
  const lastMs = new Date(lastQuery).getTime();
  if (!Number.isFinite(lastMs)) return new Date(nowMs).toISOString();
  return new Date(lastMs + getQueryIntervalMs(task)).toISOString();
}

function activeLaneOccupant(tasks, kind, nowMs) {
  const activeTask = tasks.find(t => taskSubmitQueueKind(t) === kind && isTaskRemotelyActive(t));
  if (activeTask) {
    return {
      task: activeTask,
      submitId: activeTask.submit_id || '',
      queueInfo: activeTask.queue_info || null,
      nextCheckAt: taskNextCheckAt(activeTask, nowMs),
    };
  }

  for (const task of tasks) {
    if (task.auto_query_stopped) continue;
    const record = (task.execution_records || []).find(r => isActiveExecutionRecord(r) && executionRecordQueueKind(r) === kind);
    if (record) {
      return {
        task,
        submitId: record.submit_id || '',
        queueInfo: parseQueueInfoFromRecord(record),
        nextCheckAt: recordNextCheckAt(record, nowMs),
      };
    }
  }
  return null;
}

function latestRetryWaitExecutionRecord(task) {
  const records = task?.execution_records || [];
  const retryRecords = records.filter(record => record?.status === 'retry_wait');
  const candidates = retryRecords.length > 0 ? retryRecords : records;
  return candidates.reduce((latest, record) => {
    if (!latest) return record;
    const latestTime = new Date(latest.finished_at || latest.started_at || '').getTime();
    const recordTime = new Date(record.finished_at || record.started_at || '').getTime();
    if (!Number.isFinite(recordTime)) return latest;
    if (!Number.isFinite(latestTime) || recordTime >= latestTime) return record;
    return latest;
  }, null);
}

function isCurrentConcurrencyCooldown(task, kind, nowMs) {
  if (task?.status !== 'retry_wait') return false;
  if (taskSubmitQueueKind(task) !== kind) return false;
  if (!task.next_run_at) return false;
  const nextTime = new Date(task.next_run_at).getTime();
  if (!Number.isFinite(nextTime) || nextTime <= nowMs) return false;
  const record = latestRetryWaitExecutionRecord(task);
  return isConcurrencyLimitError(task.last_error) ||
    record?.error_kind === 'ConcurrencyLimit' ||
    isConcurrencyLimitError(record?.error_detail || '');
}

/**
 * 获取任务的提交队列类型（重试时以最近执行记录为准）。
 * @param {object} task
 * @returns {'standard'|'fast'}
 */
export function taskSubmitQueueKind(task) {
  if (!task) return 'standard';
  if (['submitting', 'submitted', 'querying'].includes(task.status)) {
    const submitId = String(task.submit_id || '').trim();
    const currentRecord = (task.execution_records || []).find(record =>
      submitId && String(record?.submit_id || '').trim() === submitId
    );
    if (currentRecord) return executionRecordQueueKind(currentRecord);
  }
  if (task.status === 'retry_wait') {
    const record = latestRetryWaitExecutionRecord(task);
    if (record) {
      const mv = record.input_snapshot?.params?.model_version || '';
      return modelToQueueKind(mv);
    }
  }
  return modelToQueueKind(task.params?.model_version || '');
}

/**
 * 车道卡片“本地队列”统计包含的任务，只展示真正等待调度或重试的任务。
 */
export function getLaneLocalTasks(tasks, kind) {
  return (tasks || []).filter((task) => (
    LOCAL_LANE_STATUSES.includes(task?.status) && taskSubmitQueueKind(task) === kind
  ));
}

function deriveLaneStatus(tasks, kind, nowMs = Date.now()) {
  const active = activeLaneOccupant(tasks, kind, nowMs);
  const coolingTasks = tasks.filter(t => isCurrentConcurrencyCooldown(t, kind, nowMs));

  const waitingCount = tasks.filter(t => {
    return WAITING_STATUSES.includes(t.status) && taskSubmitQueueKind(t) === kind;
  }).length;

  let nextCheckAt = '';
  if (active) {
    nextCheckAt = active.nextCheckAt;
  } else if (coolingTasks.length > 0) {
    nextCheckAt = coolingTasks.map(t => t.next_run_at).sort()[0] || '';
  } else {
    const dueTasks = tasks.filter(t => {
      return ['queued', 'retry_wait', 'scheduled'].includes(t.status) &&
        taskSubmitQueueKind(t) === kind &&
        t.next_run_at;
    });
    nextCheckAt = dueTasks.map(t => t.next_run_at).sort()[0] || '';
  }

  const qi = active?.queueInfo || null;
  return {
    queueKind: kind,
    modelVersion: laneModelVersion(kind),
    enabled: true,
    isActive: !!active,
    isCoolingDown: coolingTasks.length > 0,
    cooldownReason: coolingTasks.length > 0
      ? `并发限制，${coolingTasks.length} 个任务等待重试`
      : '',
    currentTaskId: active?.task?.id || '',
    currentTaskTitle: active?.task?.title || '',
    submitId: active?.submitId || '',
    queuePosition: qi?.queue_idx ?? null,
    queueLength: qi?.queue_length ?? null,
    nextCheckAt,
    waitingTaskCount: waitingCount,
  };
}

/**
 * 从 tasks 数组推导标准车道状态。
 * @returns {{ isActive, isCoolingDown, cooldownReason, currentTaskId, currentTaskTitle, submitId, queuePosition, queueLength, nextCheckAt, waitingTaskCount }}
 */
export function deriveStandardLaneStatus(tasks, nowMs = Date.now()) {
  return deriveLaneStatus(tasks, 'standard', nowMs);
}

/**
 * 从 tasks 数组推导 Fast 车道状态。
 */
export function deriveFastLaneStatus(tasks, nowMs = Date.now()) {
  return deriveLaneStatus(tasks, 'fast', nowMs);
}

/**
 * 获取查询间隔（毫秒），与 Rust query_interval_secs 保持一致。
 */
function getQueryIntervalMs(task) {
  const qi = task?.queue_info;
  if (!qi) return 60 * 1000;
  const isGenerating = (qi.queue_status || '').toLowerCase() === 'generating';
  if (isGenerating) return 60 * 1000;
  if (qi.queue_idx == null) return 180 * 1000;
  if (qi.queue_idx <= 100) return 180 * 1000;
  if (qi.queue_idx <= 1000) return 600 * 1000;
  return 1200 * 1000;
}

/**
 * 合并后端 laneStatus 和前端推导结果（后端优先）。
 * @param {Array} backendStatus - state.laneStatus（可能为空）
 * @param {Array} tasks
 * @returns {Array} 两个车道的状态数组 [standard, fast]
 */
export function getLaneStatuses(backendStatus, tasks) {
  if (backendStatus && backendStatus.length >= 2) {
    // 后端返回的数据优先使用
    return backendStatus;
  }
  // 降级：前端推导
  const nowMs = Date.now();
  return [
    deriveStandardLaneStatus(tasks, nowMs),
    deriveFastLaneStatus(tasks, nowMs),
  ];
}

export function resolveNextEnabledLane(laneStatuses = []) {
  const available = laneStatuses.filter((lane) =>
    lane?.enabled !== false && !lane?.isActive && !lane?.isCoolingDown
  );
  return available.find((lane) => lane.queueKind === 'standard')
    || available.find((lane) => lane.queueKind === 'fast')
    || null;
}

/**
 * 格式化下次检查时间的显示文本。
 * @param {string} isoTime - ISO 8601 时间字符串
 * @param {number} nowMs
 * @returns {string}
 */
export function formatNextCheckAt(isoTime, nowMs = Date.now()) {
  if (!isoTime) return '—';
  const time = new Date(isoTime).getTime();
  if (!Number.isFinite(time)) return '—';
  const diffMs = time - nowMs;
  if (diffMs <= 0) return '应立即检查';
  const diffSec = Math.ceil(diffMs / 1000);
  if (diffSec < 60) return `${diffSec} 秒`;
  const diffMin = Math.ceil(diffSec / 60);
  if (diffMin < 60) return `${diffMin} 分钟`;
  const diffHour = Math.floor(diffMin / 60);
  const remainMin = diffMin % 60;
  if (remainMin === 0) return `${diffHour} 小时`;
  return `${diffHour} 小时 ${remainMin} 分`;
}

/**
 * 获取任务的路由目标车道标签。
 * @param {object} task
 * @returns {{ kind: 'standard'|'fast', label: string }}
 */
export function getTaskRouteInfo(task) {
  if (!task) return { kind: 'standard', label: '标准' };
  const kind = taskSubmitQueueKind(task);
  return { kind, label: laneLabel(kind) };
}

/**
 * 格式化任务路由下次时间显示。
 * @param {object} task
 * @param {number} nowMs
 * @returns {string}
 */
export function formatTaskNextTime(task, nowMs = Date.now()) {
  if (!task) return '';
  const status = task.status;
  if (status === 'succeeded' || status === 'failed') return '';

  const nextRunAt = task.next_run_at;
  if (!nextRunAt) return '';

  const nextMs = new Date(nextRunAt).getTime();
  if (!Number.isFinite(nextMs)) return '';

  const diffMs = nextMs - nowMs;
  if (diffMs <= 0) return '应立即尝试';

  return formatNextCheckAt(nextRunAt, nowMs);
}

/**
 * 推导任务详情面板中「下一步」的描述。
 * @param {object} task
 * @param {number} nowMs
 * @returns {{ action: string, reason: string }}
 */
export function deriveNextAction(task, nowMs = Date.now(), { schedulerTickSeconds = 30, laneStatuses = [] } = {}) {
  if (!task) return { action: '', reason: '' };
  const kind = taskSubmitQueueKind(task);
  const lane = laneLabel(kind);
  const availableLane = resolveNextEnabledLane(laneStatuses);
  const targetKind = availableLane?.queueKind || kind;
  const targetLane = laneLabel(targetKind);
  const status = task.status;

  if (status === 'draft') {
    return { action: '等待提交', reason: '任务尚未进入队列，需手动排队或等待调度。' };
  }
  if (status === 'queued') {
    const nextTime = availableLane
      ? formatSchedulerTickEta(nowMs, schedulerTickSeconds)
      : task.next_run_at
      ? formatTaskDueActionTime(task.next_run_at, nowMs, '应立即提交')
      : formatSchedulerTickEta(nowMs, schedulerTickSeconds);
    const nextClock = !availableLane && task.next_run_at
      ? formatTaskClock(task.next_run_at)
      : formatClockFromMs(nowMs + Math.max(1, Number(schedulerTickSeconds || 30)) * 1000);
    return {
      action: `${nextTime} 走 ${targetLane}`,
      reason: `任务在队列中等待，预计 ${nextClock} 前后提交到${targetLane}车道。`,
    };
  }
  if (status === 'scheduled') {
    const time = task.scheduled_at ? formatTaskDueActionTime(task.scheduled_at, nowMs, '应立即入队') : '等待预定时间';
    return { action: `${time} 进入队列`, reason: `等待预定时间到达后自动进入${lane}车道队列。` };
  }
  if (status === 'submitting') {
    return { action: `提交到${lane}车道中`, reason: '正在调用远端 API 提交生成任务。' };
  }
  if (status === 'submitted' || status === 'querying') {
    const qi = task.queue_info;
    if (qi?.queue_idx != null && qi?.queue_length != null) {
      return {
        action: `远端排队 #${qi.queue_idx} / ${qi.queue_length}`,
        reason: `任务已提交到${lane}车道远端队列，等待生成完成。`,
      };
    }
    return {
      action: `远端处理中`,
      reason: `任务已提交到${lane}车道远端，但暂未返回排队名次。`,
    };
  }
  if (status === 'retry_wait') {
    const nextTime = task.next_run_at ? formatTaskDueActionTime(task.next_run_at, nowMs, '应立即重试') : '等待重试';
    const isReviewRetry = String(task.last_error || '').toLowerCase().includes('pre-tns') ||
      (task.execution_records || []).some((record) => (
        record.status === 'retry_wait' && record.error_kind === 'GenerationPrecheck'
      ));
    if (isReviewRetry) {
      return {
        action: `${nextTime} · 队尾`,
        reason: '生成审核未通过，已移到队伍末尾；前面的任务完成后再重试。',
      };
    }
    const isConcurrency = isConcurrencyLimitError(task.last_error) ||
      (task.execution_records || []).some(r => r.error_kind === 'ConcurrencyLimit');
    if (isConcurrency) {
      if (availableLane && targetKind !== kind) {
        return {
          action: `${formatSchedulerTickEta(nowMs, schedulerTickSeconds)} 走 ${targetLane}`,
          reason: `${targetLane}车道空闲，将在下次调度检查时切换提交。`,
        };
      }
      return {
        action: `${nextTime} 走 ${lane}（冷却中）`,
        reason: `${lane}车道触发并发限制，冷却结束后自动重试。`,
      };
    }
    return {
      action: `${nextTime} 走 ${lane}`,
      reason: `上次提交遇到临时错误，等待重试冷却结束后走${lane}车道。`,
    };
  }
  if (status === 'succeeded') {
    return { action: '已完成', reason: `任务已在${lane}车道执行成功。` };
  }
  if (status === 'failed') {
    return { action: '已失败', reason: `任务在${lane}车道执行失败，可手动重试。` };
  }

  return { action: '', reason: '' };
}

function formatTaskDueActionTime(isoTime, nowMs, dueText) {
  const time = new Date(isoTime).getTime();
  if (!Number.isFinite(time)) return '等待调度';
  if (time <= nowMs) return dueText;
  return formatNextCheckAt(isoTime, nowMs);
}

function formatSchedulerTickEta(nowMs, schedulerTickSeconds) {
  const seconds = Math.max(1, Math.ceil(Number(schedulerTickSeconds || 30)));
  if (seconds < 60) return `约 ${seconds} 秒内`;
  const minutes = Math.ceil(seconds / 60);
  if (minutes < 60) return `约 ${minutes} 分钟内`;
  const hours = Math.floor(minutes / 60);
  const remainMinutes = minutes % 60;
  if (remainMinutes === 0) return `约 ${hours} 小时内`;
  return `约 ${hours} 小时 ${remainMinutes} 分钟内`;
}

function formatTaskClock(isoTime) {
  const time = new Date(isoTime).getTime();
  if (!Number.isFinite(time)) return '稍后';
  return formatClockFromMs(time);
}

function formatClockFromMs(ms) {
  const date = new Date(ms);
  if (Number.isNaN(date.getTime())) return '稍后';
  return `${String(date.getHours()).padStart(2, '0')}:${String(date.getMinutes()).padStart(2, '0')}`;
}

/**
 * 从执行记录和任务状态推导时间线事件。
 * @param {object} task
 * @returns {Array<{ time: string, title: string, detail: string }>}
 */
export function deriveTimelineEvents(task) {
  if (!task) return [];
  const events = [];
  const now = new Date();

  // 从执行记录提取事件
  const records = task.execution_records || [];
  for (const rec of records) {
    const modelLabel = rec.input_snapshot?.params?.model_version === FAST_MODEL ? 'Fast' : '标准';
    pushTimelineEvent(events, rec.started_at, now, {
      title: `${modelLabel} 开始`,
      detail: `开始提交到${modelLabel}车道。`,
    });

    if (rec.submit_id) {
      pushTimelineEvent(events, rec.started_at, now, {
        title: `${modelLabel} 提交成功`,
        detail: `拿到远端 submit #${rec.submit_id.slice(0, 8)}，开始进入远端队列。`,
      });
    }

    if (rec.status === 'retry_wait' || rec.error_kind === 'ConcurrencyLimit') {
      pushTimelineEvent(events, rec.finished_at || rec.started_at, now, {
        title: `${modelLabel} 探测`,
        detail: rec.error_detail || '远端返回并发限制，车道进入冷却。',
      });
    } else if (rec.status === 'failed') {
      pushTimelineEvent(events, rec.finished_at || rec.started_at, now, {
        title: `${modelLabel} 失败`,
        detail: rec.error_detail || '执行失败。',
      });
    } else if (rec.status === 'succeeded') {
      pushTimelineEvent(events, rec.finished_at || rec.started_at, now, {
        title: `${modelLabel} 完成`,
        detail: '任务执行成功。',
      });
    } else if (rec.status === 'querying' || rec.status === 'submitted') {
      pushTimelineEvent(events, rec.finished_at || rec.started_at, now, {
        title: `${modelLabel} 查询`,
        detail: rec.submit_id
          ? `远端 submit #${rec.submit_id.slice(0, 8)}，等待结果。`
          : '等待远端返回结果。',
      });
    }
  }

  // 添加排队开始事件
  if (task.queued_at) {
    pushTimelineEvent(events, task.queued_at, now, {
      title: '进入队列',
      detail: '按排队开始时间排序，先进入的优先。',
    });
  }

  return events
    .sort((a, b) => b.sortMs - a.sortMs)
    .map(({ sortMs, ...event }) => event);
}

function pushTimelineEvent(events, isoTime, now, event) {
  if (!isoTime) return;
  const sortMs = new Date(isoTime).getTime();
  if (!Number.isFinite(sortMs)) return;
  events.push({
    at: isoTime,
    time: formatTimelineTime(isoTime, now),
    sortMs,
    ...event,
  });
}

export function selectKeyTimelineRecords(events = [], queryAttempts = [], limit = 5) {
  const records = [
    ...events.map((event, index) => ({
      kind: 'event',
      id: `event-${event.at || event.time || index}-${index}`,
      at: event.at || '',
      event,
    })),
    ...queryAttempts.map((attempt, index) => ({
      kind: 'query',
      id: `query-${attempt.id || index}`,
      at: attempt.finished_at || attempt.started_at || '',
      attempt,
    })),
  ].sort((left, right) => new Date(right.at || 0).getTime() - new Date(left.at || 0).getTime());

  const selected = [];
  for (const record of records) {
    const isProbe = record.kind === 'query'
      || /探测|查询/.test(record.event?.title || '');
    const previous = selected[selected.length - 1];
    const previousIsProbe = previous && (previous.kind === 'query'
      || /探测|查询/.test(previous.event?.title || ''));
    if (isProbe && previousIsProbe) continue;
    selected.push(record);
    if (selected.length >= limit) break;
  }
  return selected;
}

function formatTimelineTime(isoTime, now) {
  if (!isoTime) return '';
  const d = new Date(isoTime);
  if (!Number.isFinite(d.getTime())) return '';
  const isToday = d.toDateString() === now.toDateString();
  const hours = String(d.getHours()).padStart(2, '0');
  const minutes = String(d.getMinutes()).padStart(2, '0');
  if (isToday) return `${hours}:${minutes}`;
  const month = String(d.getMonth() + 1).padStart(2, '0');
  const day = String(d.getDate()).padStart(2, '0');
  return `${month}-${day} ${hours}:${minutes}`;
}
