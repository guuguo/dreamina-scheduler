/**
 * log-view-utils.js — 日志中心前端派生工具
 * 纯函数模块，不依赖 React，可被 main.jsx 和测试直接引用。
 */

// ── 常量 ──────────────────────────────────────────────

export const LOG_LEVELS = ['error', 'warn', 'info', 'success', 'debug'];
export const LOG_SOURCES = ['CLI', 'Scheduler', 'Worker', 'RetryPolicy', 'System', 'AI', 'ImageGen', 'Asset', 'Role', 'Settings'];
export const LOG_CATEGORIES = ['task', 'cli', 'error', 'system', 'retry', 'asset', 'role', 'imagegen', 'ai', 'settings'];

export const LEVEL_VARIANT_MAP = {
  error: 'bad',
  warn: 'warn',
  info: 'info',
  success: 'ok',
  debug: 'neutral',
};

export const LEVEL_ICON_MAP = {
  error: 'AlertCircle',
  warn: 'AlertTriangle',
  info: 'Info',
  success: 'CheckCircle',
  debug: 'Bug',
};

export const SOURCE_LABEL_MAP = {
  CLI: 'CLI',
  Scheduler: '调度器',
  Worker: '执行器',
  RetryPolicy: '重试策略',
  System: '系统',
  AI: 'AI',
  ImageGen: '生图',
  Asset: '素材',
  Role: '角色',
  Settings: '设置',
};

export const CATEGORY_LABEL_MAP = {
  task: '任务',
  cli: 'CLI',
  error: '错误',
  system: '系统',
  retry: '重试',
  asset: '素材',
  role: '角色',
  imagegen: '生图',
  ai: 'AI',
  settings: '设置',
};

export const SIDEBAR_CATEGORIES = [
  { key: 'all', label: '全部日志', icon: 'List' },
  { key: 'task', label: '任务日志', icon: 'Video' },
  { key: 'cli', label: 'CLI 日志', icon: 'Terminal' },
  { key: 'error', label: '错误日志', icon: 'AlertCircle' },
  { key: 'system', label: '系统事件', icon: 'Monitor' },
  { key: 'retry', label: '重试记录', icon: 'RotateCcw' },
];

export const QUICK_LOCATIONS = [
  { key: 'today_errors', label: '今日错误' },
  { key: 'last_1h', label: '最近 1 小时' },
];

export const TIME_RANGE_OPTIONS = [
  { key: 'all', label: '全部' },
  { key: '1h', label: '最近 1 小时' },
  { key: '24h', label: '最近 24 小时' },
  { key: '7d', label: '最近 7 天' },
];

export const DEFAULT_PAGE_SIZE = 20;

// ── 规范化 ────────────────────────────────────────────

/**
 * 将后端返回的日志条目（可能是旧字符串或新结构化对象）规范化为统一格式。
 * 旧字符串 → { id, timestamp, level:'info', source:'System', category:'system',
 *              eventType:'legacy.string_log', message, detail, legacyString: true }
 */
export function normalizeLogEntry(raw) {
  if (typeof raw === 'string') {
    return {
      id: `legacy-${raw.length}-${raw.slice(0, 8).replace(/\s/g, '_')}`,
      timestamp: '',
      level: 'info',
      source: 'System',
      category: 'system',
      eventType: 'legacy.string_log',
      message: raw.length > 120 ? raw.slice(0, 120) + '…' : raw,
      detail: raw,
      taskId: null,
      taskTitle: null,
      submitId: null,
      executionRecordId: null,
      errorDetail: null,
      rawOutput: null,
      stdout: null,
      stderr: null,
      module: null,
      legacyString: true,
    };
  }
  // 新结构化日志，确保所有字段有默认值
  return {
    id: raw.id || '',
    timestamp: raw.timestamp || '',
    level: raw.level || 'info',
    source: raw.source || 'System',
    category: raw.category || 'system',
    eventType: raw.eventType || raw.event_type || '',
    message: raw.message || '',
    detail: raw.detail || '',
    taskId: raw.taskId || raw.task_id || null,
    taskTitle: raw.taskTitle || raw.task_title || null,
    submitId: raw.submitId || raw.submit_id || null,
    executionRecordId: raw.executionRecordId || raw.execution_record_id || null,
    errorDetail: raw.errorDetail || raw.error_detail || null,
    rawOutput: raw.rawOutput || raw.raw_output || null,
    stdout: raw.stdout || null,
    stderr: raw.stderr || null,
    module: raw.module || null,
    legacyString: false,
  };
}

// ── 统计 ──────────────────────────────────────────────

/**
 * 从规范化日志列表派生统计卡片数据。
 * @param {Array} logs - 规范化后的日志
 * @param {number} retentionCount - settings.log_retention_count
 * @returns {{ today: number, errors: number, warnings: number, infos: number, retention: string }}
 */
export function deriveLogStats(logs, retentionCount) {
  const now = new Date();
  const todayStart = new Date(now.getFullYear(), now.getMonth(), now.getDate()).toISOString();
  let today = 0, errors = 0, warnings = 0, infos = 0;

  for (const log of logs) {
    if (log.timestamp && log.timestamp >= todayStart) today++;
    if (log.level === 'error') errors++;
    if (log.level === 'warn') warnings++;
    if (log.level === 'info' || log.level === 'success') infos++;
    // debug 级别不计入统计卡片
  }

  return {
    today,
    errors,
    warnings,
    infos,
    retention: retentionCount > 0 ? `保留 ${retentionCount} 条` : '无限制',
  };
}

/**
 * 按左侧分类计算各分类的日志数量。
 */
export function deriveCategoryCounts(logs) {
  const counts = { all: logs.length, task: 0, cli: 0, error: 0, system: 0, retry: 0 };
  for (const log of logs) {
    if (log.category === 'task' || log.taskId) counts.task++;
    if (log.source === 'CLI' || log.category === 'cli') counts.cli++;
    if (log.level === 'error') counts.error++;
    if (log.category === 'system' || log.source === 'System') counts.system++;
    if (log.category === 'retry' || log.source === 'RetryPolicy') counts.retry++;
  }
  return counts;
}

// ── 筛选 ──────────────────────────────────────────────

/**
 * 组合筛选日志。
 * @param {Array} logs - 规范化后的日志
 * @param {Object} filters
 * @param {string} filters.search - 搜索关键词（匹配 message/detail/submitId/taskTitle）
 * @param {string} filters.level - 日志级别筛选
 * @param {string} filters.source - 来源筛选
 * @param {string} filters.taskId - 任务 ID 筛选
 * @param {string} filters.timeRange - 时间范围 'all'|'1h'|'24h'|'7d'
 * @param {string} filters.category - 左侧分类 'all'|'task'|'cli'|'error'|'system'|'retry'
 * @returns {Array}
 */
export function filterLogs(logs, filters = {}) {
  const { search = '', level = '', source = '', taskId = '', timeRange = 'all', category = 'all' } = filters;
  const now = Date.now();
  const searchLower = search.toLowerCase();

  return logs.filter((log) => {
    // 默认隐藏 debug 级别，除非用户显式选择
    if (log.level === 'debug' && level !== 'debug') return false;
    // 搜索
    if (searchLower) {
      const haystack = [log.message, log.detail, log.submitId, log.taskTitle, log.taskId]
        .filter(Boolean).join(' ').toLowerCase();
      if (!haystack.includes(searchLower)) return false;
    }
    // 级别
    if (level && log.level !== level) return false;
    // 来源
    if (source && log.source !== source) return false;
    // 任务
    if (taskId && log.taskId !== taskId) return false;
    // 时间范围
    if (timeRange !== 'all' && log.timestamp) {
      const ts = new Date(log.timestamp).getTime();
      const cutoff = timeRange === '1h' ? now - 3600000
        : timeRange === '24h' ? now - 86400000
        : timeRange === '7d' ? now - 604800000
        : 0;
      if (ts < cutoff) return false;
    }
    // 左侧分类
    if (category === 'task' && log.category !== 'task' && !log.taskId) return false;
    if (category === 'cli' && log.source !== 'CLI' && log.category !== 'cli') return false;
    if (category === 'error' && log.level !== 'error') return false;
    if (category === 'system' && log.category !== 'system' && log.source !== 'System') return false;
    if (category === 'retry' && log.category !== 'retry' && log.source !== 'RetryPolicy') return false;

    return true;
  });
}

// ── 分页 ──────────────────────────────────────────────

/**
 * 对筛选后的日志进行分页。
 * @param {Array} logs - 已筛选日志
 * @param {number} page - 从 1 开始
 * @param {number} pageSize
 * @returns {{ items: Array, page: number, pageSize: number, total: number, totalPages: number }}
 */
export function paginateLogs(logs, page = 1, pageSize = DEFAULT_PAGE_SIZE) {
  const total = logs.length;
  const totalPages = Math.max(1, Math.ceil(total / pageSize));
  const safePage = Math.min(Math.max(1, page), totalPages);
  const start = (safePage - 1) * pageSize;
  return {
    items: logs.slice(start, start + pageSize),
    page: safePage,
    pageSize,
    total,
    totalPages,
  };
}

// ── 关联事件 ──────────────────────────────────────────

/**
 * 查找与指定日志条目关联的其他日志（同 taskId 或同 submitId）。
 * @param {Array} logs - 全部规范化日志
 * @param {Object} target - 目标日志条目
 * @param {number} maxCount - 最多返回条数
 * @returns {Array}
 */
export function deriveLogAssociations(logs, target, maxCount = 3) {
  if (!target) return [];
  const { taskId, submitId, id } = target;
  if (!taskId && !submitId) return [];

  return logs
    .filter((log) => log.id !== id && (
      (taskId && log.taskId === taskId) || (submitId && log.submitId === submitId)
    ))
    .slice(0, maxCount);
}

// ── 导出 ──────────────────────────────────────────────

/**
 * 构建日志导出内容。
 * @param {Array} logs - 要导出的规范化日志
 * @param {'json'|'text'} format
 * @returns {string}
 */
export function buildLogExportPayload(logs, format = 'json') {
  if (format === 'text') {
    return logs.map((log) => {
      const parts = [`[${log.timestamp}]`, `[${log.level}]`, `[${log.source}]`, log.message];
      if (log.taskTitle) parts.push(`任务: ${log.taskTitle}`);
      if (log.submitId) parts.push(`submit_id: ${log.submitId}`);
      if (log.detail) parts.push(log.detail);
      return parts.join(' ');
    }).join('\n');
  }
  return JSON.stringify(logs, null, 2);
}

// ── 默认选中 ──────────────────────────────────────────

/**
 * 决定默认选中的日志 ID：优先最新 error，否则最新一条。
 */
export function findDefaultSelectedLogId(logs) {
  if (!logs.length) return null;
  const latestError = [...logs].reverse().find((l) => l.level === 'error');
  if (latestError) return latestError.id;
  return logs[logs.length - 1].id;
}

// ── 快速定位 ──────────────────────────────────────────

/**
 * 根据快速定位 key 返回对应的筛选条件。
 */
export function getQuickLocationFilters(key) {
  const now = new Date();
  const todayStart = new Date(now.getFullYear(), now.getMonth(), now.getDate()).toISOString();
  switch (key) {
    case 'today_errors':
      return { level: 'error', timeRange: '24h' };
    case 'last_1h':
      return { timeRange: '1h' };
    default:
      return {};
  }
}

// ── 任务选项 ──────────────────────────────────────────

/**
 * 从日志列表中提取有日志的任务选项（去重）。
 * @returns {Array<{id: string, title: string}>}
 */
export function deriveTaskOptions(logs) {
  const seen = new Map();
  for (const log of logs) {
    if (log.taskId && log.taskTitle && !seen.has(log.taskId)) {
      seen.set(log.taskId, log.taskTitle);
    }
  }
  return Array.from(seen.entries()).map(([id, title]) => ({ id, title }));
}
