# 验收标准

## 状态

- 状态：confirmed（P0 已由用户方向性确认；P1 细节可在执行轮调整）
- P0 是否已由用户确认：是
- 最近确认时间：2026-07-06

## 验收表

| 编号 | 对应来源 | 验收条件 | 重要性 | 证据要求 | 状态 |
| --- | --- | --- | --- | --- | --- |
| A1 | `plan.md#目标` | 产物中存在 Python CLI 队列代理，命令名暂定 `dreaminaq`，能被 shell/agent 调用 | P0 | `dreaminaq --help`、包结构、安装/运行说明 | pending |
| A2 | `plan.md#范围-做` | `dreaminaq` 支持与即梦 CLI 接近的视频提交参数，并能把任务写入本地队列 | P0 | 示例命令、入队后的 `tasks` 记录、`status --json` 输出 | pending |
| A3 | `plan.md#范围-做` | Python 代理通过 subprocess 调用原 dreamina CLI，不自行实现即梦 API、上传或鉴权 | P0 | 代码审查；调用路径和原始 stdout/stderr 记录 | pending |
| A4 | `plan.md#范围-做` | 本地缓存包含 assets、tasks、attempts、results 四类核心数据，且存储在 SQLite + 文件目录中 | P0 | SQLite schema、数据目录结构、入队/完成样例 | pending |
| A5 | `plan.md#范围-做` | 同一 `idempotency_key` 重复提交不会创建重复任务，而是返回已有 task_id | P0 | 自动化测试或手工命令记录 | pending |
| A6 | `plan.md#范围-做` | worker/once 能推进任务，并保证同一时刻只有一个推进者修改队列 | P0 | 并发测试、SQLite 事务/锁实现证据 | pending |
| A7 | `plan.md#范围-做` | 轮询能记录 submit_id、queue_info、失败错误和结果路径；`result --json` 可返回产物 | P0 | mock CLI 或真实 CLI 冒烟日志、结果 JSON | pending |
| A8 | `plan.md#范围-做` | worker 在存在未完成任务时具备防休眠/保活能力，能支撑夜间长时间调度，不因系统自动睡眠错过排队窗口 | P0 | macOS `caffeinate` 或等价平台适配证据；worker 日志显示 keep-awake 生命周期；人工长跑或模拟测试记录 | pending |
| A9 | `plan.md#范围-做` | 支持 standard/Fast 策略，包含标准优先和超过 2 小时/失败后的 Fast 兜底策略入口 | P1 | 单元测试或策略函数测试 | pending |
| A10 | `plan.md#不做` | 默认不复用同参数历史结果；只有 `--reuse exact` 命中成功结果时才直接返回缓存 | P1 | 测试：无 reuse 新建任务，有 reuse 返回结果 | pending |
| A11 | `plan.md#不做` | 不要求现有 Tauri App 继续作为主入口；如保留 UI，只能作为轻量状态/干预层 | P1 | 文档和产品入口说明；无新增复杂任务中心依赖 | pending |
| A12 | `tech-design.md` | 配套 skill 文档或草案能指导 agent 使用 `dreaminaq`，包括提交、worker、status、result | P1 | skill 文件/草案或 README 片段 | pending |

## 不作为通过依据

- 只看到 Python 文件存在，但没有可运行 CLI。
- 只封装 MCP schema，而没有 CLI 兼容调用路径。
- Python 直接重写即梦接口、上传或鉴权。
- worker 只能单次跑通，但没有锁/事务防并发。
- worker 能循环运行，但没有防休眠/保活证据。
- 只有文本日志，没有机器可读 JSON 状态。
- 执行层自称完成但没有回填本文件或同目录 `result.md`。

## 待确认验收

| 编号 | 问题 | 推荐答案 | 风险 | 状态 |
| --- | --- | --- | --- | --- |
| T1 | 是否要求第一版有 Web/TUI 状态页 | 否，CLI `status --watch` 足够 | 做 UI 会拉长周期 | open |
| T2 | 是否要求读取旧 App 数据 | 否，第一版新目录并行 | 旧数据兼容会拖慢主线 | open |
| T3 | 是否要求真实即梦 CLI 端到端测试 | 最好有，但若账号/网络不稳定，可用 mock + 一次真实冒烟 | 无真实冒烟时协议风险更高 | open |
| T4 | 防休眠首版覆盖平台 | macOS 必须可用；Windows/Linux 可先给适配接口和文档 | 全平台一次性实现会拉长首版 | open |

## 执行验收记录

> 执行完成后必须回填。本节记录最终证据，不记录运行过程态。证据较多时，创建同目录 `result.md` 并在此链接。

- 执行状态：not-started
- 执行方式：未开始
- 执行时间：—
- 执行者/会话：—
- 关联提交/PR/变更摘要：—
- 详细结果文件：无

### 验收项结果

| 编号 | 状态 | 实际证据 | 说明 |
| --- | --- | --- | --- |
| A1 | not-verified | — | 未执行 |
| A2 | not-verified | — | 未执行 |
| A3 | not-verified | — | 未执行 |
| A4 | not-verified | — | 未执行 |
| A5 | not-verified | — | 未执行 |
| A6 | not-verified | — | 未执行 |
| A7 | not-verified | — | 未执行 |
| A8 | not-verified | — | 未执行 |
| A9 | not-verified | — | 未执行 |
| A10 | not-verified | — | 未执行 |
| A11 | not-verified | — | 未执行 |
| A12 | not-verified | — | 未执行 |

### 验证命令与结果

| 命令/检查 | 结果 | 关键输出或证据位置 |
| --- | --- | --- |
| `dreaminaq --help` | not-run | — |
| `dreaminaq submit ... --json` | not-run | — |
| `dreaminaq once --json` | not-run | — |
| `dreaminaq worker --keep-awake --interval 30 --json-lines` | not-run | — |
| `dreaminaq status --json` | not-run | — |
| `dreaminaq result TASK_ID --json` | not-run | — |
| Python 单元测试 | not-run | — |

### 独立审查

- Spec review：未做
- Code review：未做
- 人工验收：未做

### 未验证项与风险

- 尚未实现代码。
- 尚未确认是否需要首版 Web/TUI 状态页。
- 尚未确认是否需要兼容旧 `.dreamina-scheduler` 数据。
- 尚未实现防休眠/保活能力。
