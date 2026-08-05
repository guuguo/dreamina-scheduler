const HOUR_MS = 60 * 60 * 1000;
const DAY_MS = 24 * HOUR_MS;
const WEEK_MS = 7 * DAY_MS;
const ACTIVE_STATUSES = new Set(['submitting', 'submitted', 'querying', 'retry_wait']);

function toTime(value) {
  const time = value ? new Date(value).getTime() : NaN;
  return Number.isFinite(time) ? time : NaN;
}

function median(values) {
  if (!values.length) return NaN;
  const sorted = [...values].sort((a, b) => a - b);
  const middle = Math.floor(sorted.length / 2);
  return sorted.length % 2
    ? sorted[middle]
    : (sorted[middle - 1] + sorted[middle]) / 2;
}

function percentile(values, ratio) {
  if (!values.length) return NaN;
  const sorted = [...values].sort((a, b) => a - b);
  return sorted[Math.max(0, Math.ceil(sorted.length * ratio) - 1)];
}

function modelKindFor(record, task) {
  const version = record?.input_snapshot?.params?.model_version
    || task?.params?.model_version
    || '';
  return version === 'seedance2.0fast' ? 'fast' : 'standard';
}

function videoDurationFor(record, task) {
  const value = Number(record?.input_snapshot?.params?.duration ?? task?.params?.duration ?? 15);
  return Number.isFinite(value) && value > 0 ? value : 15;
}

function hasSuccessfulResult(record) {
  return record?.status === 'succeeded'
    || (record?.result_paths || []).length > 0
    || (record?.result_urls || []).length > 0;
}

function flattenLaneRecords(tasks, kind, nowMs) {
  const records = [];

  for (const task of tasks || []) {
    const executionRecords = task.execution_records?.length
      ? task.execution_records
      : (task.submitted_at ? [{
          id: `legacy:${task.id}`,
          status: task.status,
          started_at: task.submitted_at,
          finished_at: task.finished_at,
          input_snapshot: { params: task.params || {} },
          result_paths: task.result_paths || [],
          result_urls: task.result_urls || [],
        }] : []);

    for (const record of executionRecords) {
      if (modelKindFor(record, task) !== kind) continue;
      const startMs = toTime(record.started_at);
      if (!Number.isFinite(startMs)) continue;
      const finishedMs = toTime(record.finished_at);
      const active = !Number.isFinite(finishedMs)
        && (ACTIVE_STATUSES.has(record.status) || ACTIVE_STATUSES.has(task.status));
      const endMs = Number.isFinite(finishedMs) ? finishedMs : active ? nowMs : startMs;
      if (endMs < startMs) continue;

      records.push({
        id: record.id || `${task.id}:${record.started_at}`,
        taskId: task.id,
        taskTitle: task.title || '未命名任务',
        status: record.status || task.status || '',
        modelKind: kind,
        videoDuration: videoDurationFor(record, task),
        startedAt: record.started_at,
        finishedAt: record.finished_at || '',
        startMs,
        endMs,
        elapsedMs: endMs - startMs,
        active,
        succeeded: Number.isFinite(finishedMs) && hasSuccessfulResult(record),
      });
    }
  }

  return records.sort((left, right) => right.startMs - left.startMs);
}

export function buildLanePerformance(tasks, kind, nowMs = Date.now()) {
  const records = flattenLaneRecords(tasks, kind, nowMs);
  const occupancyStartMs = nowMs - DAY_MS;
  const occupancy = records
    .filter((record) => record.startMs < nowMs && record.endMs > occupancyStartMs)
    .map((record) => {
      const visibleStart = Math.max(record.startMs, occupancyStartMs);
      const visibleEnd = Math.min(Math.max(record.endMs, visibleStart + 60_000), nowMs);
      return {
        ...record,
        clippedBefore: record.startMs < occupancyStartMs,
        leftPercent: ((visibleStart - occupancyStartMs) / DAY_MS) * 100,
        widthPercent: Math.max(0.25, ((visibleEnd - visibleStart) / DAY_MS) * 100),
      };
    });

  const completed = records.filter(
    (record) => record.succeeded && record.startMs >= nowMs - WEEK_MS && record.startMs <= nowMs
  );
  const durationBaselines = new Map();
  for (const duration of new Set(completed.map((record) => record.videoDuration))) {
    durationBaselines.set(
      duration,
      median(completed.filter((record) => record.videoDuration === duration).map((record) => record.elapsedMs))
    );
  }

  const normalized = completed.map((record) => {
    const baselineMs = durationBaselines.get(record.videoDuration);
    return {
      ...record,
      baselineMs,
      speedRatio: Number.isFinite(baselineMs) && baselineMs > 0 ? record.elapsedMs / baselineMs : 1,
    };
  });

  const hours = Array.from({ length: 24 }, (_, hour) => {
    const hourRecords = normalized.filter((record) => new Date(record.startMs).getHours() === hour);
    const speedRatio = median(hourRecords.map((record) => record.speedRatio));
    const durationGroups = [...new Set(hourRecords.map((record) => record.videoDuration))]
      .sort((a, b) => a - b)
      .map((duration) => {
        const group = hourRecords.filter((record) => record.videoDuration === duration);
        return {
          duration,
          count: group.length,
          medianMs: median(group.map((record) => record.elapsedMs)),
          p90Ms: percentile(group.map((record) => record.elapsedMs), 0.9),
        };
      });
    return {
      hour,
      count: hourRecords.length,
      speedRatio,
      tone: !hourRecords.length
        ? 'empty'
        : speedRatio < 0.85
          ? 'faster'
          : speedRatio > 1.15
            ? 'slower'
            : 'steady',
      records: hourRecords.sort((left, right) => right.startMs - left.startMs),
      durationGroups,
    };
  });

  return { occupancy, hours };
}
