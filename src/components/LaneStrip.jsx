import React from 'react';
import { ChevronRight, Image, Power, RefreshCw, X } from 'lucide-react';
import {
  getSharedWaitingTasks,
  laneLabel,
  laneModelVersion,
  formatNextCheckAt,
  getLaneRemoteOccupancyDisplay,
} from '../lane-utils.js';
import { buildLanePerformance } from '../lane-performance-utils.js';
import { resolveMediaSrc } from '../media-src.js';

/**
 * LaneStrip — 双车道状态条（原型 lane-strip 区域）
 *
 * Props:
 *   laneStatuses  — Array<LaneStatus> (至少 [standard, fast])
 *   tasks         — 全量任务，用于推导共享待调度池与车道占用时间线
 *   nowMs         — 当前时间戳，用于倒计时格式化
 */
export default function LaneStrip({
  laneStatuses = [],
  tasks = [],
  generationStats = { today: 0, yesterday: 0, last7Days: 0, total: 0, days: [] },
  nowMs = Date.now(),
  onToggleLane,
  onProbeLane,
  onSelectTask,
  assetById,
  roles = [],
  taskPriorities = {},
  pendingLaneKind = '',
  pendingProbeLaneKind = '',
}) {
  const [sharedPoolOpen, setSharedPoolOpen] = React.useState(false);
  const [generationStatsOpen, setGenerationStatsOpen] = React.useState(false);
  const [performanceHour, setPerformanceHour] = React.useState(null);
  const standard = laneStatuses.find(s => s.queueKind === 'standard') || null;
  const fast = laneStatuses.find(s => s.queueKind === 'fast') || null;
  const enabledCount = laneStatuses.filter((lane) => lane.enabled !== false).length;
  const sharedTasks = React.useMemo(() => getSharedWaitingTasks(tasks), [tasks]);
  const sharedStats = React.useMemo(() => getSharedPoolStats(sharedTasks, nowMs), [sharedTasks, nowMs]);

  const selectTask = (taskId) => {
    setSharedPoolOpen(false);
    setGenerationStatsOpen(false);
    setPerformanceHour(null);
    onSelectTask?.(taskId);
  };

  return (
    <section className="lane-strip" aria-label="模型车道状态">
      <LaneCard lane={standard} kind="standard" tasks={tasks} nowMs={nowMs} sharedTaskCount={sharedStats.total}
        onToggleLane={onToggleLane} onProbeLane={onProbeLane}
        onSelectTask={selectTask} onSelectSpeedHour={(hour) => setPerformanceHour({ kind: 'standard', hour })}
        pending={pendingLaneKind === 'standard'} probePending={pendingProbeLaneKind === 'standard'} canDisable={enabledCount > 1} />
      <LaneCard lane={fast} kind="fast" tasks={tasks} nowMs={nowMs} sharedTaskCount={sharedStats.total}
        onToggleLane={onToggleLane} onProbeLane={onProbeLane}
        onSelectTask={selectTask} onSelectSpeedHour={(hour) => setPerformanceHour({ kind: 'fast', hour })}
        pending={pendingLaneKind === 'fast'} probePending={pendingProbeLaneKind === 'fast'} canDisable={enabledCount > 1} />
      <div className="lane-summary-row">
        <button type="button" className="shared-dispatch-pool" onClick={() => setSharedPoolOpen(true)}
          disabled={sharedStats.total === 0} title={sharedStats.total ? '查看共享待调度池' : '共享待调度池为空'}>
          <span className="shared-pool-icon">Q</span>
          <span className="shared-pool-title">
            <b>共享待调度池 {sharedStats.total} 个</b>
            <em>任一启用车道空闲后动态分配，标准优先</em>
          </span>
          <span className="shared-pool-metric"><em>待排</em><b>{sharedStats.waiting}</b></span>
          <span className="shared-pool-metric"><em>重试</em><b>{sharedStats.retry}</b></span>
          <span className="shared-pool-metric"><em>最老等待</em><b>{sharedStats.queueAge}</b></span>
          <ChevronRight size={16} />
        </button>
        <button type="button" className="lane-generation-summary" onClick={() => setGenerationStatsOpen(true)}
          title="查看视频生成统计">
          <span className="lane-generation-title">生成统计</span>
          <span className="lane-generation-metric"><em>昨日</em><b>{generationStats.yesterday}</b></span>
          <span className="lane-generation-metric"><em>今日</em><b>{generationStats.today}</b></span>
          <ChevronRight size={16} />
        </button>
      </div>
      {sharedPoolOpen ? (
        <SharedTasksModal
          tasks={sharedTasks}
          assetById={assetById}
          roles={roles}
          taskPriorities={taskPriorities}
          onSelectTask={selectTask}
          onClose={() => setSharedPoolOpen(false)}
        />
      ) : null}
      {generationStatsOpen ? (
        <GenerationStatsModal
          stats={generationStats}
          tasks={tasks}
          assetById={assetById}
          roles={roles}
          onSelectTask={selectTask}
          onClose={() => setGenerationStatsOpen(false)}
        />
      ) : null}
      {performanceHour ? (
        <PerformanceHourModal
          kind={performanceHour.kind}
          hour={performanceHour.hour}
          tasks={tasks}
          nowMs={nowMs}
          assetById={assetById}
          roles={roles}
          onSelectTask={selectTask}
          onClose={() => setPerformanceHour(null)}
        />
      ) : null}
    </section>
  );
}

function GenerationStatsModal({ stats, tasks, assetById, roles, onSelectTask, onClose }) {
  const [rangeKey, setRangeKey] = React.useState('last7Days');
  const [modelKind, setModelKind] = React.useState('all');
  const [selectedDayKey, setSelectedDayKey] = React.useState('');
  const days = stats.days || [];
  const selectedDay = days.find((day) => day.key === selectedDayKey) || null;
  const range = selectedDay
    ? {
        total: selectedDay.count,
        standard: selectedDay.standard,
        fast: selectedDay.fast,
        records: selectedDay.records,
      }
    : stats.ranges?.[rangeKey] || { total: 0, standard: 0, fast: 0, records: [] };
  const filteredRecords = (range.records || []).filter(
    (record) => modelKind === 'all' || record.modelKind === modelKind
  );
  const taskById = React.useMemo(
    () => new Map((tasks || []).map((task) => [task.id, task])),
    [tasks]
  );
  const maxDayCount = Math.max(...days.map((day) => day.count), 1);
  const standardPercent = range.total ? Math.round((range.standard / range.total) * 100) : 0;
  const fastPercent = range.total ? 100 - standardPercent : 0;
  const highlightedDayKey = selectedDayKey
    || (rangeKey === 'today' ? days[0]?.key : rangeKey === 'yesterday' ? days[1]?.key : '');
  const rangeMeta = selectedDay
    ? {
        label: selectedDay.label === '今天'
          ? '今日产出'
          : selectedDay.label === '昨天'
            ? '昨日产出'
            : `${selectedDay.label}产出`,
        note: '已选择单日',
      }
    : {
        today: { label: '今日产出', note: `截至 ${formatClock(new Date().toISOString())}` },
        yesterday: { label: '昨日产出', note: '按完成时间统计' },
        last7Days: { label: '近 7 天产出', note: '点击柱子查看单日' },
      }[rangeKey];

  const selectChartDay = (day) => {
    setSelectedDayKey(day.key);
    if (day.key === days[0]?.key) setRangeKey('today');
    else if (day.key === days[1]?.key) setRangeKey('yesterday');
    else setRangeKey('day');
  };

  const selectPresetRange = (key) => {
    setSelectedDayKey('');
    setRangeKey(key);
  };

  React.useEffect(() => {
    const closeOnEscape = (event) => {
      if (event.key === 'Escape') onClose();
    };
    window.addEventListener('keydown', closeOnEscape);
    return () => window.removeEventListener('keydown', closeOnEscape);
  }, [onClose]);

  return (
    <div className="modal-backdrop lane-tasks-modal-backdrop" role="presentation" onMouseDown={onClose}>
      <section className="lane-tasks-dialog generation-stats-dialog" role="dialog" aria-modal="true"
        aria-labelledby="generation-stats-title" onMouseDown={(event) => event.stopPropagation()}>
        <header className="lane-tasks-dialog-head">
          <div>
            <span>VIDEO OUTPUT</span>
            <h2 id="generation-stats-title">生成统计</h2>
          </div>
          <strong>累计 {stats.total} 个</strong>
          <button type="button" className="icon-ghost" onClick={onClose} title="关闭"><X size={17} /></button>
        </header>

        <div className="generation-stats-stage">
          <article className="generation-trend-panel">
            <span>{rangeMeta.label}</span>
            <div className="generation-trend-total">
              <b>{range.total}</b>
              <em>个视频</em>
              <strong>{rangeMeta.note}</strong>
            </div>
            <div className={`generation-sparkline${highlightedDayKey ? ' has-selection' : ''}`}
              aria-label="最近七天逐日产出，点击柱子查看当天记录">
              {[...days].reverse().map((day) => (
                <button type="button" key={day.key}
                  className={highlightedDayKey === day.key ? 'active' : ''}
                  aria-label={`${day.label}，生成 ${day.count} 个视频`}
                  aria-pressed={highlightedDayKey === day.key}
                  title={`${day.label} ${day.count} 个`}
                  onClick={() => selectChartDay(day)}>
                  <b style={{ height: `${day.count ? Math.max(12, (day.count / maxDayCount) * 100) : 5}%` }} />
                </button>
              ))}
            </div>
          </article>

          <article className="generation-model-panel">
            <h3>模型贡献</h3>
            <div className="generation-model-row">
              <div><b>标准模型</b><span>{range.standard} · {standardPercent}%</span></div>
              <i><b style={{ width: `${standardPercent}%` }} /></i>
            </div>
            <div className="generation-model-row fast">
              <div><b>Fast 模型</b><span>{range.fast} · {fastPercent}%</span></div>
              <i><b style={{ width: `${fastPercent}%` }} /></i>
            </div>
          </article>

          <article className="generation-snapshot-panel">
            <h3>产出快照</h3>
            <div><span>今日生成</span><b>{stats.today}</b></div>
            <div><span>昨日生成</span><b>{stats.yesterday}</b></div>
            <div><span>累计生成</span><b>{stats.total}</b></div>
          </article>
        </div>

        <div className="generation-stats-controls">
          <div className="generation-model-tabs" role="tablist" aria-label="模型筛选">
            {[
              ['all', `全部 ${range.total}`],
              ['standard', `标准 ${range.standard}`],
              ['fast', `Fast ${range.fast}`],
            ].map(([key, label]) => (
              <button type="button" key={key} data-model={key}
                className={modelKind === key ? 'active' : ''}
                onClick={() => setModelKind(key)}>
                {label}
              </button>
            ))}
          </div>
          <div className="generation-range-tabs" role="tablist" aria-label="时间范围">
            {[
              ['today', '今天'],
              ['yesterday', '昨天'],
              ['last7Days', '近 7 天'],
            ].map(([key, label]) => (
              <button type="button" key={key}
                className={!selectedDayKey && rangeKey === key ? 'active' : (
                  selectedDayKey && rangeKey === key && ['today', 'yesterday'].includes(key) ? 'active' : ''
                )}
                onClick={() => selectPresetRange(key)}>
                {label}
              </button>
            ))}
          </div>
        </div>

        <div className="lane-tasks-list generation-record-list">
          {filteredRecords.length ? filteredRecords.map((record) => {
            const task = taskById.get(record.taskId);
            const thumbPath = findTaskThumbPath(task || {}, assetById, roles);
            return (
              <button type="button" className="lane-task-row generation-record-row"
                key={`${record.taskId}:${record.id}`}
                onClick={() => onSelectTask(record.taskId)}>
                <span className="lane-task-thumb">
                  {thumbPath ? <img src={resolveMediaSrc(thumbPath)} alt="" /> : <Image size={17} />}
                </span>
                <span className="lane-task-copy">
                  <b>{record.taskTitle}</b>
                  <em>{record.modelVersion} · {formatGenerationRecordTime(record.finishedAt)}</em>
                </span>
                <span className="generation-record-count">{record.count} 个视频</span>
                <span className={`generation-record-model ${record.modelKind}`}>{record.modelKind === 'fast' ? 'Fast' : '标准'}</span>
                <ChevronRight className="lane-task-open-icon" size={16} />
              </button>
            );
          }) : (
            <div className="generation-record-empty">当前筛选下暂无完成记录</div>
          )}
        </div>
        <footer>点击完成记录可回到对应任务详情；本地文件与远程链接不会重复计数。</footer>
      </section>
    </div>
  );
}

function LaneCard({
  lane,
  kind,
  tasks,
  nowMs,
  sharedTaskCount,
  onToggleLane,
  onProbeLane,
  onSelectTask,
  onSelectSpeedHour,
  pending,
  probePending,
  canDisable,
}) {
  const performance = React.useMemo(
    () => buildLanePerformance(tasks, kind, nowMs),
    [tasks, kind, nowMs]
  );
  if (!lane) {
    return (
      <article className={`lane-card ${kind === 'fast' ? 'fast' : ''}`}>
        <div className="lane-card-empty">暂无车道数据</div>
      </article>
    );
  }

  const isFast = kind === 'fast';
  const enabled = lane.enabled !== false;
  const iconLetter = isFast ? 'F' : 'S';
  const title = `${laneLabel(kind)}车道`;
  const modelVer = laneModelVersion(kind);
  const pressure = getLanePressure(lane);
  const timeValue = enabled
    ? formatLaneTime(lane.nextCheckAt, nowMs, lane.isActive ? '应立即查询' : sharedTaskCount > 0 ? '应立即提交' : '应立即探测')
    : '—';
  const nextAt = formatLaneNextClock(lane.nextCheckAt, nowMs);
  const remote = getLaneRemoteOccupancyDisplay(lane);
  const nextStep = getLaneNextStep(lane, sharedTaskCount);

  return (
    <article className={`lane-card ${isFast ? 'fast' : ''}${enabled ? '' : ' disabled'}`}>
      <div className="lane-head">
        <div className="lane-title">
          <div className="lane-icon">{iconLetter}</div>
          <div>
            <b>{title} <span className="lane-tag">{isFast ? '并行' : '优先'}</span></b>
            <span className="lane-model-name">{modelVer}</span>
          </div>
        </div>
        <div className="lane-usage">
          <span>压力 <b>{pressure}%</b></span>
          <div className="lane-bar" style={{ '--v': `${pressure}%` }}><i /></div>
          <button type="button" className={`lane-toggle${enabled ? ' on' : ''}`}
            onClick={() => onToggleLane?.(kind, !enabled)}
            disabled={pending || (enabled && !canDisable)}
            title={enabled ? (canDisable ? `关闭${title}` : '至少保留一条车道') : `启用${title}`}>
            <Power size={12} /> {pending ? '切换中' : enabled ? '开启' : '关闭'}
          </button>
        </div>
      </div>

      <div className="lane-bento">
        <div className="lane-tile span-5">
          <span className="lane-tile-label">实际远端/冷却</span>
          <b className={remote.tone}>{remote.value}</b>
          <em>{remote.copy}</em>
        </div>
        <div className="lane-tile lane-probe-tile span-3">
          <span className="lane-tile-label">冷却探测</span>
          <button type="button" className={`lane-probe-button${probePending ? ' spinning' : ''}`}
            onClick={() => onProbeLane?.(lane)} disabled={!enabled || probePending}
            title={`立即探测${title}`} aria-label={`立即探测${title}`}>
            <RefreshCw size={13} />
          </button>
          <b>{timeValue}</b>
          <em>下次 {nextAt}<br />自适应轮询</em>
        </div>
        <div className="lane-tile span-4">
          <span className="lane-tile-label">下一步</span>
          <b className={nextStep.tone}>{nextStep.title}</b>
          <em>{nextStep.copy}</em>
        </div>
      </div>

      <div className="lane-performance">
        <div className="lane-performance-head">
          <span>近 24 小时任务占用</span>
          <span>{formatDayRange(nowMs)}</span>
        </div>
        <div className="lane-occupancy-track" aria-label={`${title}近 24 小时任务占用`}>
          {[0, 1, 2, 3, 4].map((index) => <i key={index} style={{ left: `${index * 25}%` }} />)}
          {performance.occupancy.map((record) => (
            <button
              type="button"
              key={record.id}
              className={`lane-occupancy-segment status-${record.active ? 'active' : record.status || 'finished'}`}
              style={{ left: `${record.leftPercent}%`, width: `${record.widthPercent}%` }}
              title={`${record.taskTitle} · ${record.active ? `已运行 ${formatElapsed(record.elapsedMs)}` : `耗时 ${formatElapsed(record.elapsedMs)}`}`}
              aria-label={`${record.taskTitle}，${record.active ? '正在生成' : '已结束'}，${formatElapsed(record.elapsedMs)}`}
              onClick={() => onSelectTask?.(record.taskId)}
            />
          ))}
        </div>
        <div className="lane-occupancy-axis">
          {formatRollingHourLabels(nowMs).map((label) => <span key={label}>{label}</span>)}
        </div>

        <div className="lane-speed-head">
          <span>近 7 天速度 · 按开始小时</span>
          <span className="lane-speed-legend"><i className="faster" />快 <i className="steady" />正常 <i className="slower" />慢</span>
        </div>
        <div className="lane-speed-grid" aria-label={`${title}近 7 天分时速度`}>
          {performance.hours.map((hour) => (
            <button
              type="button"
              key={hour.hour}
              className={hour.tone}
              disabled={!hour.count}
              title={formatSpeedHourTitle(hour)}
              aria-label={formatSpeedHourTitle(hour)}
              onClick={() => onSelectSpeedHour?.(hour.hour)}
            >
              {hour.count || ''}
            </button>
          ))}
        </div>
        <div className="lane-speed-axis">
          {[0, 3, 6, 9, 12, 15, 18, 21, 23].map((hour) => (
            <span key={hour} style={{ gridColumn: hour + 1 }}>{String(hour).padStart(2, '0')}</span>
          ))}
        </div>
      </div>
    </article>
  );
}

function PerformanceHourModal({ kind, hour, tasks, nowMs, assetById, roles, onSelectTask, onClose }) {
  const performance = React.useMemo(
    () => buildLanePerformance(tasks, kind, nowMs),
    [tasks, kind, nowMs]
  );
  const selectedHour = performance.hours[hour] || { records: [], durationGroups: [], count: 0, speedRatio: NaN };
  const hourLabel = `${String(hour).padStart(2, '0')}:00–${String((hour + 1) % 24).padStart(2, '0')}:00`;

  React.useEffect(() => {
    const closeOnEscape = (event) => {
      if (event.key === 'Escape') onClose();
    };
    window.addEventListener('keydown', closeOnEscape);
    return () => window.removeEventListener('keydown', closeOnEscape);
  }, [onClose]);

  return (
    <div className="modal-backdrop lane-tasks-modal-backdrop" role="presentation" onMouseDown={onClose}>
      <section className="lane-tasks-dialog performance-hour-dialog" role="dialog" aria-modal="true"
        aria-labelledby="performance-hour-title" onMouseDown={(event) => event.stopPropagation()}>
        <header className="lane-tasks-dialog-head">
          <div>
            <span>LANE PERFORMANCE</span>
            <h2 id="performance-hour-title">{laneLabel(kind)}车道 · {hourLabel}</h2>
          </div>
          <strong>近 7 天 {selectedHour.count} 个</strong>
          <button type="button" className="icon-ghost" onClick={onClose} title="关闭"><X size={17} /></button>
        </header>

        <div className="performance-hour-summary">
          <div><span>相对速度</span><b className={selectedHour.tone}>{formatSpeedRatio(selectedHour.speedRatio)}</b></div>
          {selectedHour.durationGroups.map((group) => (
            <div key={group.duration}>
              <span>{group.duration} 秒视频 · {group.count} 个</span>
              <b>中位 {formatElapsed(group.medianMs)} · P90 {formatElapsed(group.p90Ms)}</b>
            </div>
          ))}
        </div>

        <div className="lane-tasks-list performance-record-list">
          {selectedHour.records.length ? selectedHour.records.map((record) => {
            const task = (tasks || []).find((item) => item.id === record.taskId);
            const thumbPath = findTaskThumbPath(task || {}, assetById, roles);
            return (
              <button type="button" className="lane-task-row performance-record-row"
                key={`${record.taskId}:${record.id}`} onClick={() => onSelectTask(record.taskId)}>
                <span className="lane-task-thumb">
                  {thumbPath ? <img src={resolveMediaSrc(thumbPath)} alt="" /> : <Image size={17} />}
                </span>
                <span className="lane-task-copy">
                  <b>{record.taskTitle}</b>
                  <em>{formatPerformanceRecordTime(record.startedAt)} 开始 · {record.videoDuration} 秒视频</em>
                </span>
                <span className="performance-record-duration">{formatElapsed(record.elapsedMs)}</span>
                <span className={`performance-record-speed ${speedTone(record.speedRatio)}`}>
                  {formatSpeedRatio(record.speedRatio)}
                </span>
                <ChevronRight className="lane-task-open-icon" size={16} />
              </button>
            );
          }) : (
            <div className="generation-record-empty">这个开始时段暂无完成记录</div>
          )}
        </div>
        <footer>速度以最近 7 天同车道、同视频秒数的中位耗时为基线；点击任务可查看完整详情。</footer>
      </section>
    </div>
  );
}

function getSharedPoolStats(tasks, nowMs) {
  const waiting = tasks.filter((task) => task.status === 'queued').length;
  const retry = tasks.filter((task) => task.status === 'retry_wait').length;
  const ageCandidates = tasks
    .map(task => task.queued_at || task.created_at || task.updated_at)
    .map(value => value ? new Date(value).getTime() : NaN)
    .filter(Number.isFinite);
  const oldest = ageCandidates.length ? Math.min(...ageCandidates) : NaN;
  return {
    waiting,
    retry,
    total: tasks.length,
    queueAge: Number.isFinite(oldest) ? formatDuration(nowMs - oldest) : '—',
  };
}

function SharedTasksModal({ tasks, assetById, roles, taskPriorities, onSelectTask, onClose }) {
  React.useEffect(() => {
    const closeOnEscape = (event) => {
      if (event.key === 'Escape') onClose();
    };
    window.addEventListener('keydown', closeOnEscape);
    return () => window.removeEventListener('keydown', closeOnEscape);
  }, [onClose]);

  return (
    <div className="modal-backdrop lane-tasks-modal-backdrop" role="presentation" onMouseDown={onClose}>
      <section className="lane-tasks-dialog" role="dialog" aria-modal="true"
        aria-labelledby="lane-tasks-dialog-title" onMouseDown={(event) => event.stopPropagation()}>
        <header className="lane-tasks-dialog-head">
          <div>
            <span>动态双车道</span>
            <h2 id="lane-tasks-dialog-title">共享待调度池</h2>
          </div>
          <strong>{tasks.length} 个</strong>
          <button type="button" className="icon-ghost" onClick={onClose} title="关闭"><X size={17} /></button>
        </header>
        <div className="lane-tasks-list">
          {tasks.map((task) => {
            const thumbPath = findTaskThumbPath(task, assetById, roles);
            const priority = Math.max(0, Math.min(2, Number(taskPriorities?.[task.id] || 0)));
            return (
              <button type="button" className="lane-task-row" key={task.id} onClick={() => onSelectTask(task.id)}>
                <span className="lane-task-thumb">
                  {thumbPath ? <img src={resolveMediaSrc(thumbPath)} alt="" /> : <Image size={17} />}
                </span>
                <span className="lane-task-copy">
                  <b>{task.title || '未命名任务'}</b>
                  <em>{priority ? `${'★'.repeat(priority)} 优先` : '随机顺序'} · {task.params?.ratio || '比例未设置'}</em>
                </span>
                <span className={`lane-task-state status-${task.status || 'draft'}`}>
                  <b>{laneTaskStatusLabel(task.status)}</b>
                  <em>{laneTaskTimeLabel(task)}</em>
                </span>
                <ChevronRight className="lane-task-open-icon" size={16} />
              </button>
            );
          })}
        </div>
        <footer>仅展示排队中和等待重试的任务；点击可查看完整详情</footer>
      </section>
    </div>
  );
}

function findTaskThumbPath(task, assetById, roles) {
  for (const id of task.image_asset_ids || []) {
    const asset = assetById?.get?.(id);
    if (asset?.stored_path) return asset.stored_path;
  }
  const role = (roles || []).find((item) => item.id === (task.role_ids || [])[0]);
  if (role?.asset_ids?.length) {
    const asset = assetById?.get?.(role.asset_ids[0]);
    if (asset?.stored_path) return asset.stored_path;
  }
  return '';
}

function laneTaskStatusLabel(status) {
  return {
    draft: '草稿',
    queued: '排队中',
    scheduled: '已定时',
    retry_wait: '等待重试',
    submitting: '提交中',
    submitted: '已提交',
    querying: '远端处理中',
  }[status] || status || '未知';
}

function laneTaskTimeLabel(task) {
  const value = task.status === 'retry_wait'
    ? task.next_run_at
    : task.status === 'scheduled'
      ? task.scheduled_at
      : task.submitted_at || task.queued_at || task.updated_at || task.created_at;
  if (!value) return '—';
  const time = new Date(value);
  if (!Number.isFinite(time.getTime())) return '—';
  return `${String(time.getMonth() + 1).padStart(2, '0')}-${String(time.getDate()).padStart(2, '0')} ${String(time.getHours()).padStart(2, '0')}:${String(time.getMinutes()).padStart(2, '0')}`;
}

function formatGenerationRecordTime(value, now = new Date()) {
  const time = new Date(value);
  if (!Number.isFinite(time.getTime())) return '完成时间未知';
  const today = new Date(now);
  today.setHours(0, 0, 0, 0);
  const yesterday = new Date(today);
  yesterday.setDate(yesterday.getDate() - 1);
  const clock = `${String(time.getHours()).padStart(2, '0')}:${String(time.getMinutes()).padStart(2, '0')}`;
  if (time >= today) return `今天 ${clock}`;
  if (time >= yesterday) return `昨天 ${clock}`;
  return `${time.getMonth() + 1}月${time.getDate()}日 ${clock}`;
}

function getLanePressure(lane) {
  if (lane.enabled === false) return 0;
  const remote = lane.isActive ? 58 : 0;
  const cooling = lane.isCoolingDown ? 24 : 0;
  return Math.max(0, Math.min(96, remote + cooling));
}

function getLaneNextStep(lane, sharedTaskCount) {
  if (lane.enabled === false) {
    return { tone: 'idle', title: '已关闭', copy: '不会接收新的排队和重试任务。' };
  }
  if (lane.isActive) {
    return { tone: 'warn', title: '查询远端中', copy: '到点后查询当前 submit 结果，保留远端任务。' };
  }
  if (lane.isCoolingDown) {
    return { tone: 'warn', title: '等待冷却', copy: lane.cooldownReason || '并发限制未解除，冷却结束后继续探测。' };
  }
  if (sharedTaskCount > 0) {
    return { tone: 'ok', title: '可接收', copy: '将从共享池按优先级取下一个任务。' };
  }
  return { tone: 'idle', title: '空闲', copy: '暂无本地待提交任务，继续按间隔探测。' };
}

function formatLaneTime(isoTime, nowMs, immediateText) {
  if (!isoTime) return '—';
  const time = new Date(isoTime).getTime();
  if (!Number.isFinite(time)) return '—';
  if (time <= nowMs) return immediateText;
  return formatNextCheckAt(isoTime, nowMs);
}

function formatClock(isoTime) {
  if (!isoTime) return '—';
  const time = new Date(isoTime);
  if (!Number.isFinite(time.getTime())) return '—';
  return `${String(time.getHours()).padStart(2, '0')}:${String(time.getMinutes()).padStart(2, '0')}:${String(time.getSeconds()).padStart(2, '0')}`;
}

function formatLaneNextClock(isoTime, nowMs) {
  const time = isoTime ? new Date(isoTime).getTime() : NaN;
  if (!Number.isFinite(time)) return '—';
  if (time <= nowMs) return '现在';
  return formatClock(isoTime);
}

function formatDayRange(nowMs) {
  const start = new Date(nowMs - 24 * 60 * 60 * 1000);
  const end = new Date(nowMs);
  return `${formatMonthDayClock(start)} 至 ${formatMonthDayClock(end)}`;
}

function formatMonthDayClock(time) {
  return `${time.getMonth() + 1}/${time.getDate()} ${String(time.getHours()).padStart(2, '0')}:${String(time.getMinutes()).padStart(2, '0')}`;
}

function formatRollingHourLabels(nowMs) {
  return [24, 18, 12, 6, 0].map((hoursAgo) => {
    if (hoursAgo === 0) return '现在';
    const time = new Date(nowMs - hoursAgo * 60 * 60 * 1000);
    return `${String(time.getHours()).padStart(2, '0')}:00`;
  });
}

function formatElapsed(ms) {
  if (!Number.isFinite(ms) || ms < 0) return '—';
  const minutes = Math.max(1, Math.round(ms / 60_000));
  if (minutes < 60) return `${minutes}分`;
  const hours = Math.floor(minutes / 60);
  const remain = minutes % 60;
  return remain ? `${hours}时${remain}分` : `${hours}时`;
}

function speedTone(ratio) {
  if (!Number.isFinite(ratio)) return 'empty';
  if (ratio < 0.85) return 'faster';
  if (ratio > 1.15) return 'slower';
  return 'steady';
}

function formatSpeedRatio(ratio) {
  if (!Number.isFinite(ratio)) return '样本不足';
  const percent = Math.round(Math.abs(ratio - 1) * 100);
  if (percent < 5) return '接近基线';
  return ratio < 1 ? `快 ${percent}%` : `慢 ${percent}%`;
}

function formatSpeedHourTitle(hour) {
  const label = `${String(hour.hour).padStart(2, '0')}:00–${String((hour.hour + 1) % 24).padStart(2, '0')}:00`;
  if (!hour.count) return `${label}，近 7 天暂无完成样本`;
  const groups = hour.durationGroups
    .map((group) => `${group.duration}秒：中位${formatElapsed(group.medianMs)}，${group.count}个`)
    .join('；');
  return `${label}，${formatSpeedRatio(hour.speedRatio)}，${groups}`;
}

function formatPerformanceRecordTime(value) {
  const time = new Date(value);
  if (!Number.isFinite(time.getTime())) return '时间未知';
  return `${time.getMonth() + 1}月${time.getDate()}日 ${String(time.getHours()).padStart(2, '0')}:${String(time.getMinutes()).padStart(2, '0')}`;
}

function formatDuration(ms) {
  if (!Number.isFinite(ms) || ms < 0) return '—';
  const minutes = Math.floor(ms / 60000);
  if (minutes < 1) return '刚刚';
  if (minutes < 60) return `${minutes}分`;
  const hours = Math.floor(minutes / 60);
  const remain = minutes % 60;
  if (hours < 24) return remain ? `${hours}时${remain}分` : `${hours}时`;
  const days = Math.floor(hours / 24);
  return `${days}天${hours % 24}时`;
}
