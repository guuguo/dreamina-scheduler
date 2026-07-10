import React from 'react';
import { ChevronRight, Image, Power, RefreshCw, X } from 'lucide-react';
import {
  getLaneLocalTasks,
  laneLabel,
  laneModelVersion,
  formatNextCheckAt,
  getLaneRemoteOccupancyDisplay,
  taskSubmitQueueKind,
} from '../lane-utils.js';
import { resolveMediaSrc } from '../media-src.js';

const WAITING_STATUSES = ['queued'];
const ACTIVE_STATUSES = ['submitting', 'submitted', 'querying'];
const RETRY_STATUSES = ['retry_wait'];

/**
 * LaneStrip — 双车道状态条（原型 lane-strip 区域）
 *
 * Props:
 *   laneStatuses  — Array<LaneStatus> (至少 [standard, fast])
 *   tasks         — 全量任务，用于推导本地任务、车道年龄、占用时间线
 *   nowMs         — 当前时间戳，用于倒计时格式化
 */
export default function LaneStrip({
  laneStatuses = [],
  tasks = [],
  nowMs = Date.now(),
  queryIntervalSeconds = 30,
  onToggleLane,
  onProbeLane,
  onSelectTask,
  assetById,
  roles = [],
  pendingLaneKind = '',
  pendingProbeLaneKind = '',
}) {
  const [openLaneKind, setOpenLaneKind] = React.useState('');
  const standard = laneStatuses.find(s => s.queueKind === 'standard') || null;
  const fast = laneStatuses.find(s => s.queueKind === 'fast') || null;
  const enabledCount = laneStatuses.filter((lane) => lane.enabled !== false).length;
  const openLaneTasks = React.useMemo(
    () => openLaneKind ? getLaneLocalTasks(tasks, openLaneKind) : [],
    [openLaneKind, tasks]
  );

  const selectTask = (taskId) => {
    setOpenLaneKind('');
    onSelectTask?.(taskId);
  };

  return (
    <section className="lane-strip" aria-label="模型车道状态">
      <LaneCard lane={standard} kind="standard" tasks={tasks} nowMs={nowMs} queryIntervalSeconds={queryIntervalSeconds}
        onToggleLane={onToggleLane} onProbeLane={onProbeLane} onOpenTasks={() => setOpenLaneKind('standard')}
        pending={pendingLaneKind === 'standard'} probePending={pendingProbeLaneKind === 'standard'} canDisable={enabledCount > 1} />
      <LaneCard lane={fast} kind="fast" tasks={tasks} nowMs={nowMs} queryIntervalSeconds={queryIntervalSeconds}
        onToggleLane={onToggleLane} onProbeLane={onProbeLane} onOpenTasks={() => setOpenLaneKind('fast')}
        pending={pendingLaneKind === 'fast'} probePending={pendingProbeLaneKind === 'fast'} canDisable={enabledCount > 1} />
      {openLaneKind ? (
        <LaneTasksModal
          kind={openLaneKind}
          tasks={openLaneTasks}
          assetById={assetById}
          roles={roles}
          onSelectTask={selectTask}
          onClose={() => setOpenLaneKind('')}
        />
      ) : null}
    </section>
  );
}

function LaneCard({ lane, kind, tasks, nowMs, queryIntervalSeconds, onToggleLane, onProbeLane, onOpenTasks, pending, probePending, canDisable }) {
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
  const stats = getLaneTaskStats(tasks, kind, nowMs);
  const pressure = getLanePressure(lane, stats);
  const timeValue = enabled
    ? formatLaneTime(lane.nextCheckAt, nowMs, lane.isActive ? '应立即查询' : lane.waitingTaskCount > 0 ? '应立即提交' : '应立即探测')
    : '—';
  const nextAt = formatLaneNextClock(lane.nextCheckAt, nowMs, queryIntervalSeconds);
  const remote = getLaneRemoteOccupancyDisplay(lane);
  const nextStep = getLaneNextStep(lane);
  const ticks = getLaneTicks(tasks, kind, nowMs);

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
        <button type="button" className="lane-tile lane-task-trigger span-3" onClick={onOpenTasks}
          disabled={stats.localTotal === 0} title={stats.localTotal ? `查看${title}的本地队列` : `${title}暂无待排任务`}>
          <span className="lane-tile-label">本地队列</span>
          <b>{stats.localTotal} 个 <ChevronRight size={14} /></b>
          <em>排队 {stats.waiting} · 重试 {stats.retry}</em>
          <small>队列年龄 {stats.queueAge}</small>
        </button>
        <div className="lane-tile span-4">
          <span className="lane-tile-label">远端/冷却状态</span>
          <b className={remote.tone}>{remote.value}</b>
          <em>{remote.copy}</em>
        </div>
        <div className="lane-tile lane-probe-tile span-2">
          <span className="lane-tile-label">冷却探测</span>
          <button type="button" className={`lane-probe-button${probePending ? ' spinning' : ''}`}
            onClick={() => onProbeLane?.(lane)} disabled={!enabled || probePending}
            title={`立即探测${title}`} aria-label={`立即探测${title}`}>
            <RefreshCw size={13} />
          </button>
          <b>{timeValue}</b>
          <em>下次 {nextAt}<br />间隔 {queryIntervalSeconds} 秒</em>
        </div>
        <div className="lane-tile span-3">
          <span className="lane-tile-label">下一步</span>
          <b className={nextStep.tone}>{nextStep.title}</b>
          <em>{nextStep.copy}</em>
        </div>
      </div>

      <div className="lane-timeline">
        <div><span>占用时间线（最近 60 分钟）</span><span>{formatHourRange(nowMs)}</span></div>
        <p>
          {ticks.map((on, index) => <i key={index} className={on ? 'on' : ''} />)}
        </p>
      </div>
    </article>
  );
}

function getLaneTaskStats(tasks, kind, nowMs) {
  const laneTasks = getLaneLocalTasks(tasks, kind);
  const waitingTasks = laneTasks.filter(task => WAITING_STATUSES.includes(task.status));
  const retryTasks = laneTasks.filter(task => RETRY_STATUSES.includes(task.status));
  const ageCandidates = [...waitingTasks, ...retryTasks]
    .map(task => task.queued_at || task.scheduled_at || task.next_run_at || task.created_at || task.updated_at)
    .map(value => value ? new Date(value).getTime() : NaN)
    .filter(Number.isFinite);
  const oldest = ageCandidates.length ? Math.min(...ageCandidates) : NaN;
  return {
    waiting: waitingTasks.length,
    retry: retryTasks.length,
    localTotal: laneTasks.length,
    queueAge: Number.isFinite(oldest) ? formatDuration(nowMs - oldest) : '—',
  };
}

function LaneTasksModal({ kind, tasks, assetById, roles, onSelectTask, onClose }) {
  React.useEffect(() => {
    const closeOnEscape = (event) => {
      if (event.key === 'Escape') onClose();
    };
    window.addEventListener('keydown', closeOnEscape);
    return () => window.removeEventListener('keydown', closeOnEscape);
  }, [onClose]);

  const title = `${laneLabel(kind)}车道待排任务`;
  return (
    <div className="modal-backdrop lane-tasks-modal-backdrop" role="presentation" onMouseDown={onClose}>
      <section className={`lane-tasks-dialog${kind === 'fast' ? ' fast' : ''}`} role="dialog" aria-modal="true"
        aria-labelledby="lane-tasks-dialog-title" onMouseDown={(event) => event.stopPropagation()}>
        <header className="lane-tasks-dialog-head">
          <div>
            <span>{laneModelVersion(kind)}</span>
            <h2 id="lane-tasks-dialog-title">{title}</h2>
          </div>
          <strong>{tasks.length} 个</strong>
          <button type="button" className="icon-ghost" onClick={onClose} title="关闭"><X size={17} /></button>
        </header>
        <div className="lane-tasks-list">
          {tasks.map((task) => {
            const thumbPath = findTaskThumbPath(task, assetById, roles);
            const queueInfo = task.queue_info;
            return (
              <button type="button" className="lane-task-row" key={task.id} onClick={() => onSelectTask(task.id)}>
                <span className="lane-task-thumb">
                  {thumbPath ? <img src={resolveMediaSrc(thumbPath)} alt="" /> : <Image size={17} />}
                </span>
                <span className="lane-task-copy">
                  <b>{task.title || '未命名任务'}</b>
                  <em>{task.params?.model_version || laneModelVersion(kind)} · {task.params?.ratio || '比例未设置'}</em>
                </span>
                <span className={`lane-task-state status-${task.status || 'draft'}`}>
                  <b>{laneTaskStatusLabel(task.status)}</b>
                  <em>{queueInfo?.queue_idx != null
                    ? `#${queueInfo.queue_idx}${queueInfo.queue_length != null ? ` / ${queueInfo.queue_length}` : ''}`
                    : laneTaskTimeLabel(task)}</em>
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

function getLanePressure(lane, stats) {
  if (lane.enabled === false) return 0;
  const remote = lane.isActive ? 38 : 0;
  const cooling = lane.isCoolingDown ? 18 : 0;
  const backlog = Math.min(42, (stats.waiting + stats.retry) * 7);
  return Math.max(0, Math.min(96, remote + cooling + backlog));
}

function getLaneNextStep(lane) {
  if (lane.enabled === false) {
    return { tone: 'idle', title: '已关闭', copy: '不会接收新的排队和重试任务。' };
  }
  if (lane.isActive) {
    return { tone: 'warn', title: '查询远端中', copy: '到点后查询当前 submit 结果，保留远端任务。' };
  }
  if (lane.isCoolingDown) {
    return { tone: 'warn', title: '等待冷却', copy: lane.cooldownReason || '并发限制未解除，冷却结束后继续探测。' };
  }
  if (lane.waitingTaskCount > 0) {
    return { tone: 'ok', title: '可提交', copy: '将尝试提交队首任务，先进入队列的优先。' };
  }
  return { tone: 'idle', title: '空闲', copy: '暂无本地待提交任务，继续按间隔探测。' };
}

function getLaneTicks(tasks, kind, nowMs) {
  const bucketCount = 54;
  const bucketMs = 60 * 60 * 1000 / bucketCount;
  const ticks = new Array(bucketCount).fill(false);
  const startMs = nowMs - 60 * 60 * 1000;

  const mark = (value) => {
    if (!value) return;
    const ms = new Date(value).getTime();
    if (!Number.isFinite(ms) || ms < startMs || ms > nowMs) return;
    const index = Math.min(bucketCount - 1, Math.max(0, Math.floor((ms - startMs) / bucketMs)));
    ticks[index] = true;
  };

  for (const task of tasks) {
    if (taskSubmitQueueKind(task) === kind && ACTIVE_STATUSES.includes(task.status)) {
      mark(task.submitted_at || task.updated_at);
    }
    for (const record of task.execution_records || []) {
      const recordKind = record?.input_snapshot?.params?.model_version === 'seedance2.0fast' ? 'fast' : 'standard';
      if (recordKind !== kind) continue;
      mark(record.started_at);
      mark(record.finished_at);
      for (const query of record.query_records || record.attempts || []) {
        mark(query.started_at);
        mark(query.finished_at);
      }
    }
  }
  return ticks;
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

function formatLaneNextClock(isoTime, nowMs, intervalSeconds) {
  const time = isoTime ? new Date(isoTime).getTime() : NaN;
  if (!Number.isFinite(time) || time <= nowMs) {
    return formatClock(new Date(nowMs + Math.max(1, Number(intervalSeconds || 30)) * 1000).toISOString());
  }
  return formatClock(isoTime);
}

function formatHourRange(nowMs) {
  const start = new Date(nowMs - 60 * 60 * 1000);
  const end = new Date(nowMs);
  return `${String(start.getHours()).padStart(2, '0')}:${String(start.getMinutes()).padStart(2, '0')} 至 ${String(end.getHours()).padStart(2, '0')}:${String(end.getMinutes()).padStart(2, '0')}`;
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
