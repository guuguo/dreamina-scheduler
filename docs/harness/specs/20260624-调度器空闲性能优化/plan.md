# 调度器空闲性能优化

## 状态

- 状态：confirmed（P0 已定：Q4 采用方案 A — Condvar 唤醒；执行采用测试先行）
- 版本归属：无明确版本
- 最近确认人：liuyun（用户）
- 最近确认时间：2026-06-24

## 目标

后台调度线程在「队列里没有任何到期/待处理任务」时，仍每 30 秒对整份 `state.json` 做多次全量磁盘读写与 JSON 序列化，并写入两条纯噪音 Debug 日志。用户在活动监视器中实测该进程 CPU/能耗偏高。

目标：在**没有任务时**显著降低调度线程的空闲开销（磁盘 I/O + 序列化 + 噪音日志），让活动监视器中的 CPU/能耗实测明显下降，同时**不损失有任务时的调度灵敏度**。

## 用户与场景

- 目标用户：运行 Dreamina 调度器桌面应用的用户（liuyun）。
- 主要场景：应用长时间挂着、队列空闲或任务都未到期时的常驻后台开销。
- 关键用户路径：应用启动 → 后台调度线程常驻 → 大部分时间空闲轮询 → 偶尔有任务到期才需要真正干活。

## 范围

### 做

1. **空闲短路**：每个 tick 先用**一次**内存快照判断是否存在「到期/待处理」任务；若没有，直接进入下一轮 sleep，跳过 `record_scheduler_tick("started")` 与 `no_due_task` 的落盘。
2. **砍噪音日志**：`scheduler_tick`（started/skipped_busy）与 `no_due_task` 这类每 30s 产生的 Debug 日志，在空闲时不再落盘（不写或仅内存计数）；保留错误日志与真正执行任务时的日志。
3. **单次快照复用**：一个 tick 内复用同一份 snapshot，避免 `snapshot()` 与多次 `mutate()` 各自重复 `load_app_data_from_disk` 整份读盘。
4. **自适应间隔 + 入队唤醒（方案 A，详见「方案 A 设计细节」）**：空闲时拉长等待间隔（默认 30s → 空闲上限 60s，参考 `settings.poll_interval_seconds`）；用 `Condvar` 把调度循环的 `sleep` 换成可被打断的 `wait_timeout`，新任务入队或到期时 `notify` 立即唤醒，恢复灵敏度。
5. **前端文案对齐**：`queue-view-utils.js` 的 `schedulerTickSeconds` 从固定 30 改为反映当前真实间隔，避免「30 秒内检查」文案在自适应后失真（若改动较大可作为 P1）。

### 不做

- 不改成完全事件驱动重构（移除轮询）。
- 不改 `state.json` 的存储格式 / 不拆分日志独立文件（全量序列化虽是根因之一，属更大改动，本期不做）。
- 不动任务级退避逻辑 `backoff_interval_secs` / `is_backoff_due`。
- 不改 MCP 工具 `dreamina_process_queue_once`（手动单步触发）的行为语义。

## 已确认事实

- 调度循环：`src-tauri/src/lib.rs:5456` 为 `thread::spawn(|| loop { process_queue_for_store_blocking(...); sleep(30s) })`，间隔常量 `SCHEDULER_TICK_INTERVAL_SECS = 30`（`lib.rs:42`）。有 30s sleep，故非 CPU 死循环空转。
- 空闲每 tick 开销（`process_queue_for_store_blocking`，`lib.rs:5251`）：
  - `record_scheduler_tick("started")` → `store.mutate`（`lib.rs:1382`）→ 读整份 `state.json` + 反序列化 → append Debug 日志 → `to_string_pretty` 整份序列化 → 写临时文件 → rename（`persist`，`lib.rs:1395`）。
  - `store.snapshot()`（`lib.rs:1366`）→ 又一次整份读盘 + 反序列化 + clone。
  - 无到期任务 → 再 `store.mutate` 写 `no_due_task` Debug 日志 → 再一次整份读+序列化+写盘。
  - 合计：空闲每 30s ≈ 3 次整份读+反序列化、2 次整份序列化+写盘、2 条噪音 Debug 日志。
- 复合恶化：噪音日志每天约 5760 条，受 `log_retention_count` 顶在上限，使 `state.json` 长期撑满，后续每次全量读写更慢。
- 无唤醒机制：Rust 端无 `Condvar/notify/channel/mpsc`，调度纯靠 sleep 轮询（grep 确认）。因此自适应拉长间隔会延迟新任务拾起，**必须配套入队唤醒**。
- 前端耦合：`schedulerTickSeconds = 30` 是 `src/queue-view-utils.js:188` 默认参数，渲染「30 秒内检查」文案（`:204`）。

## 推荐假设

- 空闲上限间隔取 60s（参考 `poll_interval_seconds` 默认值）；如果只做范围 1~3（最小改动）就已消除绝大部分空闲 I/O，自适应间隔属增量收益。
- 砍掉 tick/no_due_task Debug 日志不影响用户排障（用户已确认「可以删/降级」）。

## 待确认问题

| 优先级 | 问题 | 推荐答案 | 不确认的影响 | 状态 |
| --- | --- | --- | --- | --- |
| P0 | 自适应间隔是否必须配套「入队/到期唤醒信号」？ | 是，加 `Condvar` 在入队与到期时唤醒，避免新任务等满长间隔 | 不加则新任务最坏等满空闲上限才被拾起，损失灵敏度，违背目标 | resolved：采用方案 A（Condvar） |
| P1 | 空闲上限间隔取值？ | 60s（参考 `poll_interval_seconds`） | 取太大→唤醒依赖更强；取太小→省的有限 | resolved：60s |
| P1 | 前端「30 秒内检查」文案是否本期一并对齐真实间隔？ | 是，把当前间隔传入 `schedulerTickSeconds` | 不改则自适应后文案对用户失真（非功能性） | 本期做（见范围做5） |

## 现有系统事实

- `peek_due_task_cli` 为纯内存判断，便宜；瓶颈在其前后的 `mutate`/`snapshot` 全量读写。
- `mutate` 与 `snapshot` 每次都会 `load_app_data_from_disk` 重新读盘后再操作（`lib.rs:1367-1389`），这是「单次快照复用」的优化点。
- MCP 侧 `dreamina_process_queue_once` 复用同一调度核心，手动单步语义需保持不变。

## 方案 A 设计细节（Condvar 唤醒）

> 实现级精确改动点、伪代码、notify 触发点清单、回退方案见同目录 **`tech-design.md`**。本节给设计要点。

> 重要设计取舍（详见 `tech-design.md` 第 2 节）：等待间隔做**二元自适应**——有任何活跃任务（复用 `needs_keep_awake` 判定）保持 30s，**完全空闲才拉长到 60s**。不做"精确到下一个退避到期时刻"的计算，从而对 `querying/submitted` 在途重查逻辑零回归。

### 新增共享唤醒原语

```rust
struct SchedulerWaker {
    // pending=true 表示「有新工作待处理」，用于防止唤醒丢失
    pending: Mutex<bool>,
    cvar: Condvar,
}
```

- 随 app 一起 `manage`，供调度线程与各入队命令共享。
- MCP 二进制（`src-tauri/src/bin/dreamina-mcp.rs`）走独立进程、`dreamina_process_queue_once` 为手动单步触发，不共享同一进程内的 `Condvar`；本期唤醒仅作用于桌面 app 内的后台调度线程，MCP 单步语义不变。

### 调度循环改造（`src-tauri/src/lib.rs:5456`）

- 把 `std::thread::sleep(30s)` 换成 `cvar.wait_timeout(guard, wait)`。
- 等待时长 `wait = compute_wait_duration(now, tasks, idle_cap=60s, short=30s)`：
  - 无待处理任务 → `idle_cap`（60s）。
  - 有任务但都未到期 → `min(idle_cap, 距最近 next_run_at 的时间)`，到点自然醒，无需额外定时器。
  - 有立即到期/待处理任务 → `short`（≤30s，循环会先处理再等待）。
- 防唤醒丢失标准写法：`notify` 前在锁内置 `pending=true`；调度线程醒来后 `while !pending` 重检，处理完置回 `false`。超时分支兜底，即使漏掉一次 notify，最坏也只多等一个 `wait`。

### 入队/唤醒触发点

每个把任务写进队列、或让任务变为「更早可执行」的路径，在提交后调用 `waker.notify()`（置 `pending=true` 再 `notify_one`）：

- 桌面 app 的入队 Tauri command（`queue_video` 系列、批量排程）。
- 任务被手动改期 / 从失败重试 / `next_run_at` 提前的命令路径。
- 说明：MCP `dreamina_queue_video` / `dreamina_queue_videos` 在独立进程内，不直接 notify 桌面进程的调度线程；其入队写入 `state.json` 后，由桌面调度线程下一次（最长 60s）轮询拾起，或在桌面 app 内的刷新动作触发——此差异需在验收中明确，不算回归。

### 可测试性拆分（为测试先行服务）

`Condvar`/线程行为难做确定性单测，故把逻辑切成可测单元：

1. **纯函数 `compute_wait_duration(now, tasks, idle_cap, short) -> Duration`**：无副作用、无线程，覆盖「无任务=idle_cap」「有未来到期=距到期」「有立即到期=short/0」「距到期 > idle_cap 时被 idle_cap 截断」等分支。这是测试先行的主战场。
2. **`SchedulerWaker` 唤醒行为集成测试**：起一个线程 `wait_timeout(很长)`，主线程 `notify`，断言**远小于超时**即返回（证明「睡着可被叫醒」）；以及「先 notify 再 wait 不丢失」（pending 标志生效）。
3. **空闲短路 / 不写噪音日志**：通过对 `process_queue_for_store_blocking` 在空队列下的行为断言——不产生 `tick`/`no_due_task` 日志、不触发 `persist`（可用 `state.json` mtime 或注入计数验证）。

## 约束与风险

- 自适应间隔无唤醒 = 灵敏度回归（P0 设计点，见上）。
- 砍日志/改间隔可能影响已有测试：`queue-view-utils.test.mjs` 断言 `schedulerTickSeconds: 30` 与「30 秒内检查」「下次检查」文案；改动需同步更新测试。
- `record_scheduler_tick` 也被 MCP/其他 origin 复用，改其落盘策略需确认不影响非 background origin 的排障预期。

## 稳定规范引用

- 无（本项目暂无 `docs/harness/specs/_stable/`）。

## 持久子需求判断

- 是否需要 `sub-*.md`：否
- 判断理由：单一性能优化，无独立版本/owner/外部契约，运行时一次性完成即可。

## 验收标准引用

- `acceptance.md`

## 执行交接

- 执行层由用户运行时指定；本需求文档不绑定 Superpowers、普通 agent 或其他执行技能。
- **测试先行（强约束）**：每个改动单元先写**会失败**的测试，跑红，再写实现转绿。顺序建议：
  1. 先写 `compute_wait_duration` 的失败单测（覆盖 4 个分支）→ 实现该纯函数。
  2. 再写 `SchedulerWaker` 唤醒/不丢失的集成测试 → 实现唤醒原语。
  3. 再写「空闲不写噪音日志 / 不 persist」的行为测试 → 改 `process_queue_for_store_blocking` 与调度循环。
  4. 最后更新受影响的既有测试（`queue-view-utils.test.mjs` 的 `schedulerTickSeconds`/文案断言）。
- 执行前必须读取 `acceptance.md`。
- 执行完成声明必须回填 `acceptance.md#执行验收记录`，或创建并链接同目录 `result.md`。
- 执行层不得擅自改写本文件的需求事实；如发现事实变化，先记录建议并等待用户确认。

## 变更记录

| 日期 | 变更 | 来源 |
| --- | --- | --- |
| 2026-06-24 | 创建需求事实文档（draft）；确认根因为空闲 tick 全量读写 + 噪音日志 | liuyun + 代码勘查 |
| 2026-06-24 | Q4 定为方案 A（Condvar 唤醒）；补「方案 A 设计细节」与可测试性拆分；定测试先行执行约束；状态 → confirmed | liuyun |
| 2026-06-24 | 新增 `tech-design.md` 实现级技术方案；确认二元自适应（活跃 30s / 空闲 60s）以对在途重查零回归 | liuyun + 代码勘查 |
