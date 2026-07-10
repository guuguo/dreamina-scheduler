# 统一动态双车道调度 Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** 将标准/Fast 两套手动入口合并为一个动态队列，并完成车道开关、左右工作区布局和关键记录精简。

**Architecture:** 后端新增车道启用设置，并由一个统一选择函数按“已启用、空闲、标准优先”决定每次真实提交；并发冷却只约束发生限制的车道。前端只负责根据后端车道状态展示动态下一步，所有排队入口都不再写死模型。

**Tech Stack:** Rust、Tauri 2、React 18、Node test、Cargo test、CSS Grid/Flexbox。

---

## 文件结构

- `src-tauri/src/lib.rs`：设置迁移、车道开关命令、动态车道选择与后端单元测试。
- `src/lane-utils.js`：前端动态下一车道和关键记录推导。
- `src/lane-utils.test.mjs`：动态下一步与关键记录单元测试。
- `src/components/LaneStrip.jsx`：车道启用开关和关闭态。
- `src/main.jsx`：统一排队入口、右侧操作/详情纵向容器、关键记录展示。
- `src/styles.css`：左右工作区和车道关闭态样式。
- `src/main-component-contract.test.mjs`：移除旧入口及布局契约。

### Task 1: 后端车道开关设置

**Files:**
- Modify: `src-tauri/src/lib.rs:1367-1395`
- Modify: `src-tauri/src/lib.rs:2140-2210`
- Test: `src-tauri/src/lib.rs` 内联测试模块

- [ ] **Step 1: 写设置默认值和禁止全关的失败测试**

```rust
#[test]
fn lane_settings_default_to_both_enabled() {
    let settings = SchedulerSettings::default();
    assert!(settings.standard_lane_enabled);
    assert!(settings.fast_lane_enabled);
}

#[test]
fn disabling_the_last_lane_is_rejected() {
    let mut data = AppData::default();
    data.settings.fast_lane_enabled = false;
    let error = set_lane_enabled(&mut data, ModelQueueKind::Standard, false).unwrap_err();
    assert!(error.to_string().contains("至少保留一条车道"));
}
```

- [ ] **Step 2: 运行测试确认失败**

Run: `cargo test lane_settings_default_to_both_enabled disabling_the_last_lane_is_rejected`

Expected: FAIL，缺少设置字段和 `set_lane_enabled`。

- [ ] **Step 3: 增加设置字段、迁移和命令**

```rust
#[serde(default = "default_true")]
pub standard_lane_enabled: bool,
#[serde(default = "default_true")]
pub fast_lane_enabled: bool,

fn set_lane_enabled(
    data: &mut AppData,
    kind: ModelQueueKind,
    enabled: bool,
) -> Result<(), SchedulerError> {
    let (standard, fast) = match kind {
        ModelQueueKind::Standard => (enabled, data.settings.fast_lane_enabled),
        ModelQueueKind::Fast => (data.settings.standard_lane_enabled, enabled),
    };
    if !standard && !fast {
        return Err(SchedulerError::Io("至少保留一条车道".to_string()));
    }
    data.settings.standard_lane_enabled = standard;
    data.settings.fast_lane_enabled = fast;
    Ok(())
}
```

注册 `set_lane_enabled_command(queue_kind, enabled)`，成功后唤醒调度器。
同时在 `LaneStatus` 增加 `enabled: bool`，`compute_lane_status` 从设置写入该字段，前端不再猜测车道是否开启。

- [ ] **Step 4: 运行相关测试**

Run: `cargo test lane_settings_ disabling_the_last_lane`

Expected: PASS。

### Task 2: 动态标准优先调度

**Files:**
- Modify: `src-tauri/src/lib.rs:4780-4905`
- Test: `src-tauri/src/lib.rs` 内联测试模块

- [ ] **Step 1: 写动态切换失败测试**

```rust
#[test]
fn fast_concurrency_retry_switches_immediately_to_idle_standard_lane() {
    let now = Utc::now();
    let mut task = make_queued_task_for_submit("fast-cooling");
    task.status = "retry_wait".into();
    task.next_run_at = Some((now + Duration::minutes(3)).to_rfc3339());
    task.last_error = "ExceedConcurrencyLimit".into();
    task.execution_records.push(make_retry_record(ModelQueueKind::Fast, now));
    let data = AppData { tasks: vec![task], ..AppData::default() };

    let selection = next_submit_task_id_for_available_queues(&data, now, &HashSet::new()).unwrap();
    assert_eq!(selection.target_queue_kind, ModelQueueKind::Standard);
}

fn make_retry_record(kind: ModelQueueKind, now: DateTime<Utc>) -> TaskExecutionRecord {
    let model_version = match kind {
        ModelQueueKind::Standard => "seedance2.0",
        ModelQueueKind::Fast => "seedance2.0fast",
    };
    TaskExecutionRecord {
        id: "retry-record".into(),
        submit_id: String::new(),
        status: "retry_wait".into(),
        started_at: now.to_rfc3339(),
        finished_at: now.to_rfc3339(),
        input_snapshot: TaskExecutionInputSnapshot {
            params: VideoParams { model_version: model_version.into(), ..VideoParams::default() },
            ..TaskExecutionInputSnapshot::default()
        },
        command_preview: vec![],
        query_records: vec![],
        result_paths: vec![],
        result_urls: vec![],
        error_kind: "ConcurrencyLimit".into(),
        error_detail: "ExceedConcurrencyLimit".into(),
    }
}

#[test]
fn both_idle_lanes_choose_standard_first() {
    let now = Utc::now();
    let data = AppData { tasks: vec![make_queued_task_for_submit("queued")], ..AppData::default() };
    let selection = next_submit_task_id_for_available_queues(&data, now, &HashSet::new()).unwrap();
    assert_eq!(selection.target_queue_kind, ModelQueueKind::Standard);
}
```

- [ ] **Step 2: 运行测试确认旧逻辑失败**

Run: `cargo test fast_concurrency_retry_switches_immediately both_idle_lanes_choose_standard_first`

Expected: Fast 冷却任务不会提前选择标准。

- [ ] **Step 3: 实现目标车道候选判断**

```rust
fn task_is_due_for_target_lane(
    task: &ScheduledTask,
    now: DateTime<Utc>,
    target: ModelQueueKind,
) -> bool {
    if task.status != "retry_wait" {
        return is_due_for_submit(task, now);
    }
    let source = submit_queue_kind_for_task(task);
    if source != target && is_concurrency_limit(&task.last_error) {
        return true;
    }
    is_due(task.next_run_at.as_deref(), now)
}
```

选择顺序固定为标准、Fast；过滤已关闭、活跃或正在冷却的车道。删除仅允许“标准阻塞后 Fast 接管”的单向特例，改为对称动态选择。

- [ ] **Step 4: 运行后端全量测试**

Run: `cargo test --manifest-path src-tauri/Cargo.toml`

Expected: 全部 PASS。

### Task 3: 前端动态下一步、车道开关和关键记录

**Files:**
- Modify: `src/lane-utils.js:320-530`
- Modify: `src/lane-utils.test.mjs`
- Modify: `src/components/LaneStrip.jsx`

- [ ] **Step 1: 写前端失败测试**

```javascript
test('retry on cooling fast lane previews idle standard immediately', () => {
  const next = deriveNextAction(fastRetryTask, nowMs, {
    laneStatuses: [idleStandard, coolingFast],
    schedulerTickSeconds: 30,
  });
  assert.equal(next.action, '约 30 秒内 走 标准');
});

test('selectKeyTimelineRecords keeps at most five merged records', () => {
  const records = selectKeyTimelineRecords(events, queries, 5);
  assert.equal(records.length, 5);
  assert.ok(records.some(record => record.kind === 'submit'));
});
```

- [ ] **Step 2: 运行测试确认失败**

Run: `node --test --test-name-pattern='cooling fast lane|KeyTimeline' src/lane-utils.test.mjs`

Expected: 缺少动态预览参数和关键记录函数。

- [ ] **Step 3: 实现纯函数**

```javascript
export function resolveNextEnabledLane(laneStatuses = []) {
  const available = laneStatuses.filter(lane => lane.enabled !== false && !lane.isActive && !lane.isCoolingDown);
  return available.find(lane => lane.queueKind === 'standard') || available.find(lane => lane.queueKind === 'fast') || null;
}

export function selectKeyTimelineRecords(events = [], queries = [], limit = 5) {
  const merged = [
    ...events.map(event => ({ ...event, kind: 'event', sortAt: event.at || event.timestamp || '' })),
    ...queries.map(query => ({ ...query, kind: 'query', sortAt: query.finished_at || query.started_at || '' })),
  ].sort((left, right) => new Date(right.sortAt) - new Date(left.sortAt));
  const compact = [];
  for (const record of merged) {
    const isProbe = record.kind === 'query' || /探测|查询/.test(record.title || '');
    const previous = compact[compact.length - 1];
    if (isProbe && previous?.isProbe && previous.status === record.status) continue;
    compact.push({ ...record, isProbe });
  }
  return compact.slice(0, limit);
}
```

`deriveNextAction` 接收 `laneStatuses`，并发重试优先显示空闲已启用车道；没有空闲车道时才显示冷却时间。

- [ ] **Step 4: LaneStrip 增加开关**

```jsx
<button
  type="button"
  role="switch"
  aria-checked={enabled}
  className={`lane-toggle${enabled ? ' on' : ''}`}
  onClick={() => onToggleLane(kind, !enabled)}
>
  {enabled ? '已启用' : '已关闭'}
</button>
```

关闭态不参与压力和下一步计算，文案固定为“已关闭 / 不接收新任务”。

- [ ] **Step 5: 运行 lane-utils 全量测试**

Run: `node --test src/lane-utils.test.mjs`

Expected: 全部 PASS。

### Task 4: 统一排队入口与左右详情布局

**Files:**
- Modify: `src/main.jsx:1900-2520`
- Modify: `src/main.jsx:2621-2800`
- Modify: `src/styles.css:5125-6785`
- Modify: `src/main-component-contract.test.mjs`

- [ ] **Step 1: 写 UI 契约失败测试**

```javascript
test('queue center uses one automatic lane entry', () => {
  assert.doesNotMatch(source, /改用 Fast|交叉 Fast 模型|allowAlternatingFastQueue/);
  assert.match(source, />\s*排队\s*</);
  assert.match(source, /className="qc-detail-column"/);
});

test('task detail defaults to five key records', () => {
  assert.match(source, /selectKeyTimelineRecords\(allEvents, allQueryAttempts, 5\)/);
});
```

- [ ] **Step 2: 运行契约测试确认失败**

Run: `node --test src/main-component-contract.test.mjs`

Expected: 旧 Fast 按钮、交叉 Fast 控件和 12+12 条默认记录仍存在。

- [ ] **Step 3: 合并入口**

删除 `handleSwitchTaskToFast`、`allowAlternatingFastQueue` 和弹窗复选框。单任务主按钮改为“排队”，仍打开排期弹窗；批量调用传 `alternateFastModel: false`，后端依据实时车道自动选择。

```jsx
<button className="qc-btn qc-btn-primary" onClick={() => openPrepareGenerate(selectedTask)}>
  <ListPlus size={13} /> 排队
</button>
```

- [ ] **Step 4: 重组左右布局**

```jsx
<div className="qc-body-dual">
  <div className="qc-task-list">...</div>
  <div className="qc-detail-column">
    <div className="qc-toolbar">...</div>
    <div className="qc-selected">...</div>
  </div>
</div>
```

```css
.qc-body-dual { display:grid; grid-template-columns:minmax(390px, 430px) minmax(0, 1fr); gap:12px; min-height:0; }
.qc-detail-column { display:grid; grid-template-rows:auto minmax(0, 1fr); gap:8px; min-height:0; }
.qc-task-list, .qc-selected { min-height:0; }
```

- [ ] **Step 5: 默认仅渲染 5 条关键记录**

使用 `selectKeyTimelineRecords` 替换 `allEvents.slice(0, 12)` 和 `recentQueryAttempts.slice(0, 12)`；完整记录弹窗仍传全部数组。

- [ ] **Step 6: 运行前端全量测试和构建**

Run: `npm test && npm run build`

Expected: 测试全部 PASS，Vite 构建成功。

### Task 5: 真实状态验证与覆盖安装

**Files:**
- Verify: `~/.dreamina-scheduler/state.json`
- Build: `src-tauri/target/release/bundle/macos/即梦调度器.app`

- [ ] **Step 1: 用真实状态运行动态路由检查**

验证当前 Fast 冷却、标准空闲任务被预览为标准，并且关闭车道后的候选只来自另一条。

- [ ] **Step 2: 完整构建 App**

Run: `npm run tauri:build -- --bundles app`

Expected: 生成 `即梦调度器.app`，退出码 0。

- [ ] **Step 3: 覆盖安装并签名**

退出旧 App，覆盖 `/Applications/即梦调度器.app`，执行本机临时签名并重新打开。

- [ ] **Step 4: 最终验证**

确认 App 进程运行、签名有效、任务数与资源数未变化；任务中心显示标准优先的动态下一步、左右布局和最多 5 条关键记录。
