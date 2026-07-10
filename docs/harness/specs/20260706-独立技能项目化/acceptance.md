# 验收标准：即梦队列独立技能项目化

## 状态

- 状态：confirmed（P0 方向已确认；P1 细节待执行前确认）
- P0 是否已由用户确认：是
- 最近确认时间：2026-07-06

## 验收表

| 编号 | 对应来源 | 验收条件 | 重要性 | 证据要求 | 状态 |
| --- | --- | --- | --- | --- | --- |
| A1 | `plan.md#目标` | 存在可独立维护的项目结构，能从当前调度器 App 中解耦，包含 Python 包和 skill 草案 | P0 | 仓库/目录结构；README；`pyproject.toml`；`skills/.../SKILL.md` | pending |
| A2 | `plan.md#目标` | 项目具备 GitHub 发布条件，其他人 clone 后可按 README 安装和运行 | P0 | README quickstart；LICENSE；示例命令；安装 smoke test | pending |
| A3 | `plan.md#范围-做` | `dreaminaq --help` 可运行，命令名稳定，支持 agent/shell 调用 | P0 | 命令输出；安装方式记录 | pending |
| A4 | `plan.md#范围-做` | `dreaminaq doctor --json` 能输出环境诊断，包括即梦 CLI 是否存在、路径、版本、登录态、数据目录、keep-awake 能力 | P0 | JSON 示例和自动化测试 | pending |
| A5 | `plan.md#范围-做` | 已安装即梦 CLI 时能自动发现；未安装时能明确进入 `install-cli` 或 `doctor --fix` 流程 | P0 | 有 CLI/无 CLI 两组测试；日志或截图 | pending |
| A6 | `plan.md#范围-做` | 支持自动安装即梦 CLI 的 adapter；如果平台或官方渠道不能全自动，必须输出可恢复的阻塞状态和明确下一步 | P0 | macOS 路径实测；失败/人工步骤测试 | pending |
| A7 | `plan.md#范围-做` | 未登录时能检测并触发官方登录流程；登录完成后 worker 可以继续推进 | P0 | mock 或真实 CLI 冒烟；事件记录 | pending |
| A8 | `plan.md#范围-做` | `submit` 能把视频生成请求和素材写入本地队列，不依赖原始素材路径长期存在 | P0 | SQLite 记录；缓存文件；`submit --json` 输出 | pending |
| A9 | `plan.md#范围-做` | `worker --keep-awake` 能夜间长时间运行；队列未完成时阻止系统睡眠，完成后释放 | P0 | macOS `caffeinate` 或等价实现；日志；测试记录 | pending |
| A10 | `plan.md#范围-做` | worker 能调用官方即梦 CLI 提交/查询，不重写官方上传、提交、鉴权协议 | P0 | 代码审查；subprocess 调用路径；原始 stdout/stderr 保存 | pending |
| A11 | `plan.md#范围-做` | 支持 standard/Fast 策略入口：标准优先，长期排队或连续失败后可 Fast 兜底 | P0 | 策略测试；配置示例；事件记录 | pending |
| A12 | `plan.md#范围-做` | `status/history/result --json` 可查询任务、尝试、错误、submit_id、队列状态和结果路径 | P0 | JSON 示例；单元测试；人工 smoke | pending |
| A13 | `plan.md#不做` | 工具不保存密码、验证码、Cookie 明文，不绕过官方登录 | P0 | 代码审查；日志脱敏测试 | pending |
| A14 | `plan.md#不做` | 第一版不新增复杂 GUI 依赖，主入口保持 CLI + skill | P1 | 依赖清单；README 定位 | pending |
| A15 | `plan.md#推荐假设` | 支持 `pipx` 或 `uv tool` 从 GitHub 安装 | P1 | 安装命令实测 | pending |
| A16 | `plan.md#目标` | skill 文档能指导 agent 完成提交、启动 worker、查询状态、读取结果和处理登录阻塞 | P0 | `SKILL.md` 内容审查；示例 agent 调用 | pending |

## 不作为通过依据

- 只有需求文档，没有可安装的 CLI。
- 只有 shell wrapper，没有持久队列、缓存和查询记录。
- 只能在开发机运行，clone 到干净环境不能安装。
- 只提示用户自己安装即梦 CLI，但没有 doctor/install adapter。
- 登录失败只报错退出，没有可恢复的 blocking 状态。
- worker 可以循环，但没有防休眠证据。
- 只有人类可读日志，没有稳定 JSON。
- Python 代码直接重写或猜测即梦官方 API。

## 待确认验收

| 编号 | 问题 | 推荐答案 | 风险 | 状态 |
| --- | --- | --- | --- | --- |
| T1 | GitHub 仓库名 | `dreamina-queue` | 未确认会影响所有安装命令 | open |
| T2 | License | MIT | 未确认会影响开源可复用性 | open |
| T3 | 即梦 CLI 官方安装方式 | 执行前实测 | 自动安装验收依赖它 | open |
| T4 | 是否需要真实即梦账号端到端测试 | 最好做一次；CI 用 fake CLI | 无真实测试会有协议风险 | open |
| T5 | Windows/Linux 首版深度 | 先 doctor + adapter skeleton + 文档，macOS 实装 | 全平台实装会拉长首版 | open |

## 执行验收记录

> 执行完成后必须回填。本节记录最终证据，不记录运行过程态。证据较多时，创建同目录 `result.md` 并在此链接。

- 执行状态：partial-implemented
- 执行方式：独立目录实现 Python CLI + skill 首版
- 执行时间：2026-07-07
- 执行者/会话：Codex
- 关联提交/PR/变更摘要：未提交；独立项目目录 `/Volumes/Seamless SSD/dev/ai_video/dreamina-queue`
- 详细结果文件：`result.md`

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
| A13 | not-verified | — | 未执行 |
| A14 | not-verified | — | 未执行 |
| A15 | not-verified | — | 未执行 |
| A16 | not-verified | — | 未执行 |

### 验证命令与结果

| 命令/检查 | 结果 | 关键输出或证据位置 |
| --- | --- | --- |
| `dreaminaq --help` | not-run | — |
| `dreaminaq doctor --json` | not-run | — |
| `dreaminaq doctor --fix` | not-run | — |
| `dreaminaq login` | not-run | — |
| `dreaminaq submit ... --json` | not-run | — |
| `dreaminaq worker --keep-awake --interval 30 --json-lines` | not-run | — |
| `dreaminaq status --json` | not-run | — |
| `dreaminaq history --json` | not-run | — |
| `dreaminaq result TASK_ID --json` | not-run | — |
| GitHub clone/install smoke | not-run | — |

### 独立审查

- Spec review：未做
- Code review：未做
- 人工验收：未做

### 未验证项与风险

- 尚未创建独立 GitHub 仓库。
- 尚未确认即梦 CLI 官方安装命令。
- 尚未实现自动安装和登录检测。
- 尚未实现防休眠长跑 worker。
