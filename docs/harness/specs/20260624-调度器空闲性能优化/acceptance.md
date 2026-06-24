# 验收标准

## 状态

- 状态：confirmed
- P0 是否已由用户确认：是（Q4 = 方案 A Condvar 唤醒；执行测试先行）
- 最近确认时间：2026-06-24

## 验收表

| 编号 | 对应来源 | 验收条件 | 重要性 | 证据要求 | 状态 |
| --- | --- | --- | --- | --- | --- |
| A0 | `plan.md#执行交接` | 测试先行：每个改动单元有「先红后绿」证据（`compute_wait_duration`、`SchedulerWaker` 唤醒、空闲不落盘各自先有失败测试再实现） | P0 | 提交历史/测试文件显示测试先于实现；或执行记录中先跑红的输出 | pending |
| A1 | `plan.md#目标` | 空闲（无到期/待处理任务）状态下，单个 tick **不再**对整份 `state.json` 产生写盘（无 `state.json.tmp` rename）与噪音 Debug 日志落盘 | P0 | 行为测试断言空队列下无 `tick`/`no_due_task` 日志、无 persist；+ 运行时观测 `state.json` mtime 多个周期不变 | pending |
| A2 | `plan.md#目标` | 活动监视器实测：空闲常驻时该进程 CPU/能耗较优化前明显下降 | P0 | 优化前后活动监视器截图或数值对比（同等空闲条件） | pending |
| A3 | `plan.md#范围` 做1 | 一个 tick 内只读一次磁盘快照，不再 `snapshot()`+多次 `mutate()` 重复 `load_app_data_from_disk` | P1 | 代码走查 | pending |
| A4 | `plan.md#方案A` | `compute_wait_duration` 纯函数单测覆盖 4 分支：无任务=60s、有未来到期=距到期、有立即到期=short/0、距到期>60s 被截断为 60s | P0 | `cargo test` 输出 | pending |
| A5 | `plan.md#方案A` | `SchedulerWaker` 唤醒生效：wait 线程被 `notify` 后**远小于超时**即返回；且「先 notify 再 wait」不丢失（pending 标志） | P0 | 集成测试输出（断言返回耗时 ≪ 超时） | pending |
| A6 | `plan.md#范围` 做4 | 端到端灵敏度不回归：桌面 app 内空闲拉长间隔后入队任务，拾起延迟 ≤ 约定阈值 | P0 | 手动测试：观测入队到拾起耗时 | pending |
| A7 | `plan.md#范围` 做5 | 前端「下次检查」文案不与真实间隔矛盾 | P1 | 界面截图或 `queue-view-utils` 单测 | pending |
| A8 | 全局 | 既有测试全绿（Rust `cargo test` + JS `node --test`），含同步更新后的 `queue-view-utils.test.mjs` | P0 | 测试命令输出 | pending |

## 不作为通过依据

- 仅「代码改了」不算通过；A2 需要实测对比，A6 需要实际灵敏度验证。
- 没有「先红」证据的实现不满足 A0（测试先行）。
- 执行层自称完成、未跑测试、未提供活动监视器对比，均不算通过。

## 待确认验收

| 编号 | 问题 | 推荐答案 | 风险 | 状态 |
| --- | --- | --- | --- | --- |
| AC-Q1 | A6 灵敏度阈值定多少？ | 桌面 app 内入队到拾起 ≤ 35s（接近原 30s 体验） | 阈值过松等于放弃灵敏度 | open（推荐 35s） |
| AC-Q2 | A2 是否需要量化下降幅度，还是「肉眼明显下降」即可？ | 肉眼明显下降 + 截图即可 | 不量化则验收偏主观 | open（推荐肉眼+截图） |

## 执行验收记录

> 执行完成后必须回填。证据较多时创建同目录 `result.md` 并在此链接。

- 执行状态：partially-passed（自动化 + A2 实测对照通过；仅 A6 端到端入队灵敏度待用户在 app 内点「新建任务」确认）
- 执行方式：普通 agent（Claude Code）+ 测试先行 TDD
- 执行时间：2026-06-24
- 执行者/会话：本会话
- 关联提交/PR/变更摘要：尚未提交。改 `src-tauri/src/lib.rs`：新增 `compute_wait_duration`/`has_active_tasks`/`SchedulerWaker`/`should_process_now`；改写 `start_background_scheduler`（空闲短路 + 单次快照 + `waker.wait`）；`.manage(SchedulerWaker::new())`；`create_task_command`/`update_task_command` 加 `notify` 与 `waker` 参数。新增 9 个单/集成测试。前端无改动。
- 详细结果文件：无

### 验收项结果

| 编号 | 状态 | 实际证据 | 说明 |
| --- | --- | --- | --- |
| A0 | passed | 每单元先写测试跑红（如 `cannot find function compute_wait_duration`）再实现转绿 | 测试先行，三单元各自 RED→GREEN |
| A1 | passed | `should_process_now` 3 单测 + 循环仅在 `should_process_now` 为真时调用重函数 | 空闲分支不进 `process_queue_for_store_blocking`，自然不写 started/no_due_task 噪音日志、不 persist。运行时 mtime 观测留作 A2 同时进行 |
| A2 | passed | 同一 37.8MB state.json、同机对照实测（65s 空闲窗口）：**旧版反复写 37MB（mtime 变化）+ 2.22 CPU 秒**；**新版零写盘（mtime 不变）+ 1.44 CPU 秒**。写盘完全消除，CPU ↓约 35% | 决定性收益是消除空闲期 37MB 全量序列化+写盘（能耗/SSD 损耗大头）。剩余 CPU 主要来自前端 30s `refreshState` 读大文件 + state.json 异常偏大（见未验证项），属后续优化 |
| A3 | passed | 循环改为 `let snapshot = store.snapshot()` 一次，供短路与等待时长共用 | 代码走查 |
| A4 | passed | `cargo test --lib`：`wait_duration_*` 4 测通过（空=60s、纯 inactive=60s、queued=30s、querying=30s） | |
| A5 | passed | `cargo test --lib`：`waker_notify_wakes_waiter_well_before_timeout`、`waker_notify_before_wait_is_not_lost` 通过 | notify 后 <2s 唤醒（超时设 10s）；pending 先置不丢失 |
| A6 | **not-verified** | — | **需用户跑桌面 app：空闲下入队任务，观测拾起延迟 ≤ 35s** |
| A7 | passed | 验证「30 秒内检查」仅对 scheduled/retry_wait/queued 等活跃状态渲染，而活跃任务下调度器恒为 30s 模式，故文案仍准确，无需改 | 270 JS 测试全绿 |
| A8 | partially-passed | Rust lib 50 passed/0 failed；JS 270 passed/0 failed；集成 core_test 2 项失败 | **2 项失败为预存 WIP（mention/素材排序重构）所致，与本需求无关**：失败测试调用 `resolve_task_inputs`，`git diff` 显示该函数改动来自会话前已 `M` 的 lib.rs，非本次调度改动 |

### 验证命令与结果

| 命令/检查 | 结果 | 关键输出或证据位置 |
| --- | --- | --- |
| `cargo test --lib`（src-tauri） | pass | 50 passed; 0 failed（含 9 个新增调度测试） |
| `cargo test`（含集成） | 2 fail | core_test 2 项预存失败（asset 排序，非本需求；见 A8） |
| `node --test`（全部前端单测） | pass | 270 passed; 0 failed |
| 活动监视器空闲对比 | done | 65s 空闲：旧版 2.22 CPU 秒+反复写 37MB；新版 1.44 CPU 秒+零写盘 |
| 端到端入队灵敏度 | not-run | 待用户在 app 内新建任务，观测拾起延迟（A6） |

### 独立审查

- Spec review：未做
- Code review：未做（建议提交前跑 `/code-review`）
- 人工验收：未做（A2/A6 待用户桌面实测）

### 未验证项与风险

- **A2 已实测对照通过**（见上）。**A6（P0）仍需用户在 app 内点「新建任务」**，观测是否秒级被拾起（验 Condvar 唤醒端到端生效）。
- **新发现：`state.json` 异常偏大（约 34–37.8MB / 121 任务，其中 `execution_records` 11MB + `attempts` 6.3MB ≈ 96%）**。已落实下方两项后续优化。`execution_records/attempts` 无界增长属第三项后续（数据治理，需用户决定是否裁剪历史），本轮未做。

## 后续优化执行记录（同一需求延伸）

用户确认「继续」后追加两项，均测试先行：

### 优化 A：`persist` 改紧凑序列化
- 改动：`AppStore::persist` 由 `to_string_pretty` → `to_string`（`src-tauri/src/lib.rs`）。pretty 缩进让磁盘体积近乎翻倍；compact 后磁盘约减半（37.8MB→约 18MB，下次实际写入时生效），每次读/写/解析 I/O 同步减半。
- 测试：`persist_writes_compact_json_not_pretty`（先红后绿）+ 既有持久化回读测试不回归。

### 优化 B：前端空闲跳过整份刷新
- 改动：新增廉价签名 `AppStore::state_signature`（仅 stat 文件 `体积:mtime`，不读内容）+ 命令 `get_state_signature`；`src/main.jsx` 的 30s 轮询先比签名，未变则跳过 `get_app_state`，空闲时不再读+解析整份大状态。可捕获本进程与 MCP 等外部写入。
- 测试：`state_signature_changes_after_mutate_and_is_stable_when_idle`（先红后绿）；270 JS 测试全绿。

### 后续优化实测（同机，65s 空闲窗口，state ≈ 34MB）
| 版本 | 空闲写盘 | 空闲 65s CPU | vs 旧版 |
| --- | --- | --- | --- |
| 旧版（无优化） | 反复写 34MB | 2.22 CPU 秒 | 基线 |
| v1（调度空闲短路+Condvar） | 零写盘 | 1.44 CPU 秒 | −35% |
| v2（+compact+前端签名门控） | 零写盘 | 1.20 CPU 秒 | **−46%** |

### 优化 C：历史裁剪 + 一次性压实（数据治理，用户确认 30/50）
- 根因：`execution_records[].query_records`（自动轮询历史）无界增长，单条执行记录攒到 698 条；`attempts` 同理。
- 改动：`cap_execution_history`（query_records 留最近 30 / attempts 留最近 50），接入 `normalize_loaded_app_data`（所有读/写都规整）；`AppStore::load` 新增 `compact_on_disk_if_oversized`，启动时若磁盘文件明显大于规整后数据则重写一次，使旧大文件立即瘦身。只裁过程态历史，**结果/状态/成败完整保留**。
- 测试（先红后绿）：`cap_execution_history_keeps_most_recent_and_reports_removed`、`cap_execution_history_noop_under_cap_returns_zero`、`load_compacts_and_trims_oversized_existing_file`。
- **真实文件实测（字节）**：`state.json` 35.5MB → **17.9MB（−50%）**，query_records 上限 698→30、attempts 350→50 已核验。每次读/解析/写按比例减半。
- 数据安全：裁剪前已备份 `state.json.pretrim-backup-20260624-230651`（35.5MB），可回滚。

### 诚实修正
- 优化 A（compact 序列化）此前被我描述为"砍半 I/O"。**对这份真实数据是错的**：原文件本就是紧凑格式（0 换行），数据为值密集型，pretty 空白占比可忽略，故 compact 实测**几乎没省**（未裁剪 compact 33.9MB ≈ 原 34MB）。compact 改动无害、原理正确、可防 pretty 膨胀，但本轮真正的体积功臣是**优化 C 的裁剪**，不是 compact。

### 仍可继续（未做）
1. **调度循环 snapshot 也按签名跳过**：空闲且签名未变时连 scheduler 的 60s 整份读盘也省掉。收益在测量噪声量级，且改调度热路径有风险，未擅自做。
2. **大字段拆分独立文件 / 懒加载**：根治"读写整份大文件"，改动面较大。当前裁剪已把文件压到 17.9MB，紧迫性下降。
- core_test 2 项预存失败与本需求无关，但建议作者单独处理该 WIP，避免掩盖真实回归。
- AC-Q1 阈值按 35s、AC-Q2 按"肉眼+截图"推荐值，待用户实测时确认。
- 变更尚未提交；按项目约定 commit/push 需用户指示。
