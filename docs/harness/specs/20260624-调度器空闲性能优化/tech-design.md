# 技术方案细节 — 调度器空闲性能优化（方案 A：Condvar 唤醒）

> 本文是 `plan.md` 的实现级补充，提供精确改动点、数据结构、伪代码、触发点清单与回退方案。
> `plan.md` 保持需求事实，本文承接「怎么改」。执行仍遵守 `acceptance.md` 的测试先行约束。

## 1. 现状调用链（确认事实）

```
start_background_scheduler (lib.rs:5455)
  └─ thread::spawn(loop {
        process_queue_for_store_blocking(&store, "background")   // lib.rs:5251
          ├─ store.mutate(record_scheduler_tick "started")       // 全量读+写盘 + Debug 日志
          ├─ try_begin_process_queue() 原子 guard
          ├─ try_acquire_store_queue_lock()
          ├─ let data = store.snapshot()                          // 又一次全量读盘
          ├─ peek_due_task_cli(&data)                             // 纯内存，便宜
          │     ├─ querying/submitted 在途任务：按 is_backoff_due 决定是否重查
          │     └─ next_submit_task_id：待提交任务
          └─ Ok(None) 分支 → store.mutate(append no_due_task)     // 又一次全量读+写盘
        std::thread::sleep(30s)                                   // lib.rs:5483，死睡不可打断
     })
```

关键事实：
- `SCHEDULER_TICK_INTERVAL_SECS = 30`（lib.rs:42）。
- `store.mutate`（lib.rs:1382）与 `store.snapshot`（lib.rs:1366）每次都 `load_app_data_from_disk` 重新整份读盘；`persist`（lib.rs:1395）用 `to_string_pretty` 整份序列化 + 写临时文件 + rename。
- 调度不止"提交新任务"，还要对 `querying`/`submitted` 在途任务按退避周期重查（`peek_due_task_cli` lib.rs:4022-4046）。**因此"是否空闲"不能只看待提交队列，要看是否存在活跃任务。**
- `needs_keep_awake`（lib.rs:1786）已定义活跃任务集：`queued | scheduled | retry_wait | submitting | submitted | querying` 且 `!auto_query_stopped`。可直接复用为"有无待处理工作"的判定。

## 2. 等待时长策略：二元自适应（核心简化）

不做"精确到下一个退避到期时刻"的复杂计算（退避依赖 `consecutive_no_result_queries` + `last_query_at`，难测且易错）。改为二元：

```rust
const IDLE_WAIT_SECS: u64 = 60;   // 完全空闲上限，参考 settings.poll_interval_seconds 默认 60
const ACTIVE_WAIT_SECS: u64 = 30; // 有活跃任务，维持现有灵敏度（含在途重查节奏）

/// 纯函数，无副作用、无线程，便于测试先行
pub fn compute_wait_duration(tasks: &[ScheduledTask]) -> Duration {
    if has_active_tasks(tasks) {
        Duration::from_secs(ACTIVE_WAIT_SECS)
    } else {
        Duration::from_secs(IDLE_WAIT_SECS)
    }
}

/// 与 needs_keep_awake 同源的活跃判定（可复用 needs_keep_awake 本体，避免两份真值）
fn has_active_tasks(tasks: &[ScheduledTask]) -> bool { needs_keep_awake(tasks) }
```

理由：
- 有活跃任务 → 30s，在途 `querying/submitted` 重查节奏、退避逻辑全部**保持现状不变**，零回归面。
- 完全无活跃任务（app 开着、队列空）→ 60s，这正是用户实测高占用的主场景。
- `notify` 负责"空闲 → 新任务"的瞬时切换，所以拉长到 60s 不损灵敏度。

> 备注：未来若想在"有未来 `scheduled_at` 的 queued 任务"上进一步省电，可扩展为 `min(IDLE_WAIT, 距最近 scheduled_at)`。本期不做，留作后续。

## 3. 唤醒原语 SchedulerWaker

```rust
pub struct SchedulerWaker {
    pending: Mutex<bool>,   // true=有新工作待处理，防唤醒丢失
    cvar: Condvar,
}

impl SchedulerWaker {
    pub fn new() -> Self { Self { pending: Mutex::new(false), cvar: Condvar::new() } }

    /// 入队/改期路径调用：标记 pending 并唤醒调度线程
    pub fn notify(&self) {
        let mut p = self.pending.lock().unwrap();
        *p = true;
        self.cvar.notify_one();
    }

    /// 调度线程调用：最多等 wait，被 notify 可提前返回；返回前清 pending
    pub fn wait(&self, wait: Duration) {
        let mut p = self.pending.lock().unwrap();
        if !*p {
            let (g, _timeout) = self.cvar.wait_timeout(p, wait).unwrap();
            p = g;
        }
        *p = false; // 消费掉这次唤醒/超时
    }
}
```

- 随 app `manage`（在 lib.rs:5503-5504 同处 `.manage(SchedulerWaker::new())`）。
- `start_background_scheduler` 取 `app_handle.state::<SchedulerWaker>()`。
- **唤醒丢失防护**：`notify` 先在锁内置 `pending=true` 再 `notify_one`；`wait` 进入前先查 `pending`，为真则不睡直接返回。即使 notify 落在两次 wait 之间，下一次 wait 也会因 `pending==true` 立即返回。超时分支兜底，最坏多等一个 `wait`。

## 4. 调度循环改造（lib.rs:5455-5484）

```rust
fn start_background_scheduler(app_handle: tauri::AppHandle) {
    std::thread::spawn(move || loop {
        let store = app_handle.state::<AppStore>();
        let waker = app_handle.state::<SchedulerWaker>();

        // —— 单次快照，供"空闲短路 + 等待时长"共用（A3：复用一次读盘）——
        let snap = store.snapshot();
        let idle = !has_active_tasks(&snap.tasks)
            && matches!(peek_due_task_cli(&snap), Ok(None));

        if !idle {
            if let Err(error) = process_queue_for_store_blocking(&store, "background") {
                // 仅错误仍落盘（保留排障）
                log_tick_error(&store, error);
            }
        }
        // idle 分支：不调用重函数、不写 started/no_due_task 噪音日志、不 persist

        let wait = compute_wait_duration(&snap.tasks);
        waker.wait(wait); // 可被入队 notify 打断
    });
}
```

要点：
- **空闲短路**：idle 时根本不进入 `process_queue_for_store_blocking`，于是 `record_scheduler_tick("started")` 与 `no_due_task` 两次全量写盘 + 两条 Debug 日志**自然消失**（满足 A1）。
- `process_queue_for_store_blocking` 本体**不改签名/不改 MCP 与手动 `process_queue_command` 的行为**——它们仍照常记 tick 日志（语义不变，满足边界约束）。
- 错误路径仍落盘，保留排障能力。

## 5. notify 触发点清单（精确位置）

每个让任务变为"可被更早执行"的写路径，提交后调用 `waker.notify()`：

| 触发点 | 位置 | 说明 |
| --- | --- | --- |
| 创建任务 | `create_task_command`（lib.rs:6005） | 新 queued/scheduled 任务入队 |
| 批量排程 | 批量排程命令（`buildBatchSchedulePlan` 对应的后端入队） | 多任务入队后 notify 一次即可 |
| 手动改期/立即执行 | 设置 `next_run_at`/清空 `scheduled_at` 的命令路径（如 lib.rs:3020 附近重置逻辑对应命令） | 任务提前到期 |
| 失败重试转 retry_wait→可执行 | 状态机将任务转回活跃且 `next_run_at` 到期的路径 | 让重试更快被拾起（可选，超时兜底已覆盖） |

注入方式：这些命令多为 `#[tauri::command]`，已可拿 `State<'_, AppStore>`；新增 `waker: State<'_, SchedulerWaker>` 参数，`store.mutate` 成功后 `waker.notify()`。

**边界（写入 plan，已确认非回归）**：MCP `dreamina_queue_video`/`dreamina_queue_videos` 在 `dreamina-mcp.rs` 独立进程，**不共享桌面进程的 SchedulerWaker**，无法直接 notify。经 MCP 入队的任务由桌面调度线程下一次轮询（最长 60s）拾起。本期不打通跨进程唤醒。

## 6. 前端文案对齐（A7）

`src/queue-view-utils.js:188` 的 `schedulerTickSeconds = 30` 默认值，在自适应后对"完全空闲"场景应反映 60s。

- 最小改动：`deriveTaskDispatchInfo` 调用方（`main.jsx:1751/1930`）按当前是否有活跃任务传入 30 或 60；或将文案从精确秒数改为"约 N 秒内检查"。
- 同步更新 `queue-view-utils.test.mjs` 中对 `schedulerTickSeconds: 30` 与「30 秒内检查」的断言。

## 7. 改动文件清单

| 文件 | 改动 |
| --- | --- |
| `src-tauri/src/lib.rs` | 新增 `SchedulerWaker`、`compute_wait_duration`、`has_active_tasks`；改 `start_background_scheduler`；`.manage(SchedulerWaker)`；各入队命令加 `notify` |
| `src-tauri/src/bin/dreamina-mcp.rs` | 不改唤醒（独立进程）；仅确认不破坏 |
| `src/queue-view-utils.js` | `schedulerTickSeconds` 取值对齐真实间隔/改文案 |
| `src/main.jsx` | 调用 `deriveTaskDispatchInfo` 时传入真实间隔（如需要） |

## 8. 测试清单（测试先行，对应 acceptance A0/A4/A5/A1/A8）

| 测试 | 文件 | 覆盖 |
| --- | --- | --- |
| `compute_wait_duration`：空任务=60s | `src-tauri` 单测 | A4 分支1 |
| `compute_wait_duration`：有 querying/queued=30s | `src-tauri` 单测 | A4 分支2/3 |
| `SchedulerWaker`：wait 被 notify 后 ≪ 超时返回 | `src-tauri` 集成测试 | A5 |
| `SchedulerWaker`：先 notify 再 wait 不丢失（立即返回） | `src-tauri` 集成测试 | A5 |
| 空闲短路：空队列下不产生 tick/no_due_task 日志、不 persist | `src-tauri` 测试（计数/mtime 注入） | A1 |
| `deriveTaskDispatchInfo` 文案随间隔变化 | `queue-view-utils.test.mjs` | A7 |
| 既有用例同步更新后全绿 | `cargo test` / `node --test` | A8 |

每项均**先写失败测试跑红，再实现转绿**。

## 9. 回退方案

- 改动集中在调度循环 + 唤醒原语 + 入队 notify，均为加法。
- 若唤醒出现异常（如某入队点漏 notify），`wait_timeout` 的 60s 超时兜底仍保证任务最终被拾起，不会永久卡死。
- 紧急回退：把 `start_background_scheduler` 内 `waker.wait(wait)` 换回 `thread::sleep(30s)`、idle 短路条件恒为 false，即恢复原行为，其余新增代码不影响。
