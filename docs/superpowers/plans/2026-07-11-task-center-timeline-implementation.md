# 任务中心共享池与时间线详情 Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** 将未提交任务统一展示为共享待调度池，并把任务详情改成高密度、状态自适应的过程时间线操作台。

**Architecture:** 保持后端统一动态双车道调度不变，前端新增“未分配任务”和“实际执行车道”两个明确概念。`lane-utils.js` 提供可测试的纯推导，`LaneStrip.jsx` 负责共享池与真实车道展示，`main.jsx` 负责状态自适应详情编排，`styles.css` 完成紧凑布局。

**Tech Stack:** React 18、JavaScript ES Modules、Node Test Runner、Vite、Tauri 状态快照。

---

### Task 1: 锁定共享池与实际车道语义

**Files:**
- Modify: `src/lane-utils.test.mjs`
- Modify: `src/lane-utils.js`

- [ ] **Step 1: 写失败测试**

在 `src/lane-utils.test.mjs` 增加：

```js
test('queued and retry tasks stay in the shared dispatch pool', () => {
  const tasks = [
    task({ id: 'queued-standard', status: 'queued' }),
    task({ id: 'retry-fast', status: 'retry_wait', params: { model_version: 'seedance2.0fast' } }),
    task({ id: 'remote', status: 'querying', submit_id: 'sub-1' }),
  ];
  assert.deepEqual(getSharedWaitingTasks(tasks).map((item) => item.id), ['queued-standard', 'retry-fast']);
  assert.equal(getActualTaskQueueKind(tasks[0]), null);
});

test('active task uses the current execution snapshot as its actual lane', () => {
  const active = task({
    status: 'querying',
    submit_id: 'fast-submit',
    params: { model_version: 'seedance2.0' },
    execution_records: [record({ submit_id: 'fast-submit' })],
  });
  assert.equal(getActualTaskQueueKind(active), 'fast');
  assert.deepEqual(getTaskRouteInfo(active), { kind: 'fast', label: 'Fast', assigned: true });
});

test('queued next action describes dynamic lane assignment', () => {
  const next = deriveNextAction(task({ status: 'queued' }), Date.now(), { schedulerTickSeconds: 30 });
  assert.match(next.action, /等待任一车道/);
  assert.match(next.reason, /提交发生后确定/);
});
```

- [ ] **Step 2: 运行测试确认失败**

Run: `node --test src/lane-utils.test.mjs`

Expected: FAIL，提示 `getSharedWaitingTasks` 或 `getActualTaskQueueKind` 未导出，旧下一步文案仍预判标准/Fast。

- [ ] **Step 3: 实现纯推导**

在 `src/lane-utils.js` 增加并替换旧本地车道分组：

```js
const SHARED_WAITING_STATUSES = ['queued', 'retry_wait'];

export function getSharedWaitingTasks(tasks = []) {
  return tasks.filter((task) => SHARED_WAITING_STATUSES.includes(task?.status));
}

export function getActualTaskQueueKind(task) {
  if (!task || !['submitting', 'submitted', 'querying', 'succeeded', 'failed'].includes(task.status)) return null;
  const submitId = String(task.submit_id || '').trim();
  const records = task.execution_records || [];
  const current = submitId
    ? records.find((record) => String(record?.submit_id || '').trim() === submitId)
    : [...records].reverse().find((record) => record?.input_snapshot?.params?.model_version);
  if (current) return executionRecordQueueKind(current);
  return modelToQueueKind(task.params?.model_version || '');
}

export function getTaskRouteInfo(task) {
  const kind = getActualTaskQueueKind(task);
  if (!kind) return { kind: null, label: '共享池', assigned: false };
  return { kind, label: laneLabel(kind), assigned: true };
}
```

同时把 `queued/retry_wait` 的 `deriveNextAction` 改为“共享待调度池，任一车道空闲后动态分配”，不再根据保存模型预测车道。

- [ ] **Step 4: 运行单元测试**

Run: `node --test src/lane-utils.test.mjs`

Expected: PASS。

### Task 2: 车道卡改为真实占用并新增共享池入口

**Files:**
- Create: `src/lane-strip-contract.test.mjs`
- Modify: `src/components/LaneStrip.jsx`
- Modify: `src/styles.css`

- [ ] **Step 1: 写组件契约测试**

```js
import { strict as assert } from 'node:assert';
import { readFileSync } from 'node:fs';
import { test } from 'node:test';

const source = readFileSync(new URL('./components/LaneStrip.jsx', import.meta.url), 'utf8');

test('lane strip uses one shared dispatch pool', () => {
  assert.match(source, /共享待调度池/);
  assert.match(source, /getSharedWaitingTasks/);
  assert.doesNotMatch(source, /getLaneLocalTasks/);
  assert.doesNotMatch(source, />本地队列</);
});
```

- [ ] **Step 2: 运行测试确认失败**

Run: `node --test src/lane-strip-contract.test.mjs`

Expected: FAIL，旧组件仍按两条车道分别显示本地队列。

- [ ] **Step 3: 重构 `LaneStrip`**

- 用 `getSharedWaitingTasks(tasks)` 计算共享池。
- 两张 `LaneCard` 删除“本地队列”按钮，只保留远端/冷却、探测、下一步和占用时间线。
- 在两张卡下面增加全宽 `.shared-dispatch-pool`，展示总数、排队、重试和最老等待时间。
- 共享池点击后复用现有弹窗，标题改为“共享待调度池”，列表不显示模型车道，只显示任务状态、优先级和等待/重试时间。
- `getLaneNextStep` 接收共享池数量；空闲车道有共享任务时显示“可接收共享池任务”。

- [ ] **Step 4: 增加紧凑样式**

在 `src/styles.css` 增加 `.shared-dispatch-pool`、`.shared-pool-metric`，让共享池横跨两列；车道内容使用 `5/3/4` 网格分配。窄窗口下共享池指标换行但入口不隐藏。

- [ ] **Step 5: 运行相关测试**

Run: `node --test src/lane-utils.test.mjs src/lane-strip-contract.test.mjs`

Expected: PASS。

### Task 3: 实现状态自适应紧凑详情

**Files:**
- Modify: `src/queue-view-utils.test.mjs`
- Modify: `src/queue-view-utils.js`
- Modify: `src/main-component-contract.test.mjs`
- Modify: `src/main.jsx`
- Modify: `src/styles.css`

- [ ] **Step 1: 写详情推导失败测试**

```js
test('detail metrics omit empty placeholders and adapt to queued state', () => {
  const metrics = deriveTaskDetailMetrics(makeTask({
    status: 'queued',
    queued_at: '2026-07-11T03:10:00Z',
  }), { nowMs: new Date('2026-07-11T07:00:00Z').getTime(), sharedPoolCount: 10 });
  assert.deepEqual(metrics.map((item) => item.label), ['共享池', '已等待', '重试', '下次检查']);
  assert.equal(metrics[0].value, '10 个');
  assert.ok(metrics.every((item) => item.value !== '—'));
});

test('succeeded details put results before timeline', () => {
  assert.deepEqual(getTaskDetailSectionOrder('succeeded'), ['results', 'timeline', 'resources']);
  assert.deepEqual(getTaskDetailSectionOrder('queued'), ['timeline', 'resources', 'results']);
});
```

- [ ] **Step 2: 运行测试确认失败**

Run: `node --test src/queue-view-utils.test.mjs`

Expected: FAIL，两个新推导函数尚不存在。

- [ ] **Step 3: 实现详情推导函数**

在 `src/queue-view-utils.js` 新增 `deriveTaskDetailMetrics(task, context)` 和 `getTaskDetailSectionOrder(status)`。指标只返回有意义的值，排队/重试、远端、成功、失败各自最多四项。

- [ ] **Step 4: 更新详情组件契约测试**

要求 `src/main.jsx` 包含 `qc-compact-console`、`过程时间线`、三个快捷优先级和始终可见的 `删除任务`；不再包含 `执行概览`、`qc-summary-grid-side` 或 `排队参数`。

- [ ] **Step 5: 替换详情 JSX**

- 把右侧 sticky side panel 合并回单列详情。
- 任务头部下方渲染两层 `.qc-compact-console`：状态和四项指标一层，优先级与动作一层。
- 过程时间线第一项固定渲染 `selectedNextAction`，下面默认展示最多 4 条关键记录。
- 根据 `getTaskDetailSectionOrder` 决定结果、时间线、资源折叠入口的顺序。
- 命中资源、执行历史、命令与参数使用紧凑 `<details>`；原有完整记录弹窗、结果预览、命令预览能力继续复用。
- 删除按钮始终渲染，继续调用现有 `handleDeleteTask` 二次确认。

- [ ] **Step 6: 更新样式**

删除不再使用的 side-panel、summary-card 和 strategy-box 样式，新增紧凑操作台、状态指标、快捷优先级、时间线和折叠入口样式。宽屏两层总高度控制在约 120px；窄屏允许三行换行，不遮挡按钮。

- [ ] **Step 7: 运行前端测试**

Run: `npm test`

Expected: 全部 PASS。

### Task 4: 生产构建与验收

**Files:**
- Verify: `src/components/LaneStrip.jsx`
- Verify: `src/main.jsx`
- Verify: `src/styles.css`

- [ ] **Step 1: 运行生产构建**

Run: `npm run build`

Expected: Vite 构建成功，无语法、导入或 CSS 错误。

- [ ] **Step 2: 启动开发服务器**

Run: `npm run dev`

Expected: 输出可访问的本地 URL；页面可加载。

- [ ] **Step 3: 浏览器验收**

检查桌面宽屏和窄屏：共享池只出现一次；两车道不再显示 10/0 的预分配；优先级三项可见；删除可见；排队状态时间线优先；成功任务结果优先；文本和按钮无重叠。

- [ ] **Step 4: 检查改动范围**

Run: `git diff --check && git status --short`

Expected: 无空白错误；不包含 Tauri bundle 或 `/Applications` 覆盖产物。

- [ ] **Step 5: 提交前端实现**

```bash
git add src/lane-utils.js src/lane-utils.test.mjs src/lane-strip-contract.test.mjs src/components/LaneStrip.jsx src/queue-view-utils.js src/queue-view-utils.test.mjs src/main-component-contract.test.mjs src/main.jsx src/styles.css docs/superpowers/plans/2026-07-11-task-center-timeline-implementation.md
git commit -m "feat: 重构任务中心共享池与时间线详情"
```
