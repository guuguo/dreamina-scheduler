# 即梦队列独立技能项目化

## 状态

- 状态：confirmed（方向由用户确认；仓库名、License、发布渠道等细节待确认）
- 版本归属：无明确版本
- 最近确认人：liuyun（用户）
- 最近确认时间：2026-07-06

## 目标

把「即梦调度器」从本地桌面 App 形态，收敛为一个可独立维护、可开源发布到 GitHub、可被其他 agent/skill/脚本复用的 **即梦 CLI 队列增强项目**。

这个项目的核心不是重新做一个 GUI 调度器，而是在即梦 CLI 外面包一层稳定的缓存、排队、夜间保活、自动发现/安装、登录引导、记录查询和 agent 友好的 JSON 输出。

目标效果：

```text
agent / skill / shell
  -> dreaminaq ...                 # 调用方式尽量接近即梦 CLI
  -> 自动发现或安装即梦 CLI
  -> 未登录时触发官方登录流程并等待恢复
  -> 本地缓存素材和请求
  -> 夜间长时间排队、查询、重试、Fast 兜底
  -> status/history/result 查询记录
```

## 用户与场景

- 目标用户：使用即梦 CLI、Codex/Claude/其他 agent、MCP 或脚本批量生成视频的人。
- 首要场景：agent 生产提示词和素材后，直接提交大量视频任务，不再靠人工在网页一个个发。
- 关键价值：
  - 尽量把标准模型队列排满。
  - 标准模型长期不可用时，用 Fast 模型先把结果出出来。
  - 夜间 2 点到 6 点等高效窗口不能因为电脑睡眠或没有 worker 而错过。
  - 队列、失败、重试、产物、历史记录都能被 agent 查询。
  - 其他人克隆 GitHub 项目后能安装使用，而不是依赖当前 Tauri App。

## 范围

### 做

- 建立独立项目，建议仓库名 `dreamina-queue`，Python 包名 `dreamina_queue`，命令名 `dreaminaq`。
- 仓库可直接发布到 GitHub，包含 README、安装说明、License、示例、测试和 CI。
- 提供一个 Codex/agent skill，指导 agent 使用 `dreaminaq`，而不是直接调用原始即梦 CLI。
- `dreaminaq` 的常用视频生成参数尽量与即梦 CLI 一致；队列层只新增必要参数，例如 `--start-at`、`--idempotency-key`、`--queue-model-policy`、`--reuse exact`。
- 自动发现即梦 CLI：从配置、PATH、常见安装位置和平台约定中查找。
- 如果发现未安装即梦 CLI，提供 `dreaminaq doctor --fix` 或 `dreaminaq install-cli` 自动安装；如果上游安装步骤必须人工确认，则给出可继续恢复的阻塞提示。
- 自动检查登录态：未登录时触发官方 CLI 登录流程或打开官方登录引导；登录完成后队列能继续推进。
- 使用 SQLite + 文件缓存保存素材、任务、尝试、事件、结果和环境检查记录。
- 实现持久队列、幂等提交、失败重试、并发探测、standard/Fast 双车道策略和历史查询。
- 第一版必须支持夜间长时间运行和防休眠，macOS 必须可用。
- 提供机器可读 JSON 输出：`submit/status/history/result/doctor` 都要能被 agent 稳定解析。
- 保留轻量人类可读输出，但不把 GUI 作为主入口。

### 不做

- 不重新实现即梦官方 API、上传协议、鉴权协议或绕过官方登录。
- 不保存用户密码、验证码、Cookie 明文或任何不该由工具接管的凭据。
- 不把现有 Tauri App 作为第一版主入口继续扩展。
- 不把复杂任务中心、角色库、生图、完整日志中心迁移进新项目第一版。
- 不默认复用相同参数的历史结果；只有显式 `--reuse exact` 才直接返回已有成功结果。
- 不承诺绕过即梦服务端并发限制；只能降低空转，按策略排队和探测。

## 已确认事实

- 用户希望该能力作为单独技能项目维护，而不是继续依附当前桌面 App。
- 用户目标是提交到 GitHub，供其他人安装和使用。
- 用户明确要求：可以自动发现即梦 CLI；如果没安装，自动安装；自动提醒或触发登录；自动排队；可以查记录。
- 用户此前已经确认：第一版需要晚上长时间不休眠调度能跑起来，否则没有作用。
- 当前项目已有队列调度经验：standard/Fast 车道、排队、查询、重试、Fast 兜底、执行记录、MCP 入队。
- 当前主要使用方式正在从手动创建任务转向 MCP/agent 调用提交任务。
- 新项目边界应是「即梦 CLI 增强代理」，不是「即梦官方 API 重写」。

## 推荐假设

- 仓库名使用 `dreamina-queue`，命令名使用 `dreaminaq`。理由：仓库名清楚，命令名短，适合 agent 调用。
- GitHub 首发，PyPI 后置；第一版安装方式支持 `pipx install git+https://github.com/<owner>/dreamina-queue.git`。
- Python 版本使用 3.11+，减少跨平台打包负担。
- 数据目录使用 `~/.dreaminaq/`，避免和现有 `~/.dreamina-scheduler/` 混淆。
- macOS 是第一优先平台：自动安装、登录触发、防休眠必须先跑通；Windows/Linux 先提供适配接口和清晰文档。
- 自动安装即梦 CLI 采用 adapter：能自动装就自动装，不能自动装时输出明确命令和恢复点。
- skill 只负责告诉 agent 怎么调用，不承担后台 worker 生命周期。

## 待确认问题

| 优先级 | 问题 | 推荐答案 | 不确认的影响 | 状态 |
| --- | --- | --- | --- | --- |
| P1 | GitHub 仓库名 | `dreamina-queue` | 影响 README、包名、安装命令 | open |
| P1 | License | MIT | 影响他人复用和开源发布 | open |
| P1 | 是否首发 PyPI | 否，先 GitHub 安装，稳定后发 PyPI | PyPI 会增加发布流程和包名占用 | open |
| P1 | 即梦 CLI 的官方安装来源和命令 | 在实现前实测并固化到 installer adapter | 自动安装能力无法验收 | open |
| P1 | 是否内置 MCP Server | 第一版不内置，只提供 CLI + skill；MCP 后置 | MCP 会增加协议维护成本 | open |
| P1 | 是否迁移现有 `.dreamina-scheduler` 历史数据 | 不迁移，后置导入命令 | 强迁移会拖慢独立项目首版 | open |
| P2 | 是否需要轻量 Web/TUI 状态页 | 后置；第一版 `status --watch` 足够 | UI 会把项目重新拉回 App 形态 | open |

## 现有系统事实

- 当前仓库是 `dreamina-scheduler`，包含 Tauri App、Rust 后端、MCP 工具和前端任务中心。
- `docs/harness/specs/20260706-cli兼容队列代理/` 已记录 Python CLI 兼容队列代理方向。
- 当前独立项目化是在该方向上的产品定位升级：从“当前项目里的 Python 代理”升级为“可 GitHub 发布的独立技能/CLI 项目”。
- 本需求文档只定义独立项目规格，不直接迁移代码、不删除现有 App。

## 约束与风险

- 自动安装依赖即梦 CLI 官方发布方式；如果官方安装渠道不稳定，必须做检测、回退和清晰提示。
- 登录不能完全无人值守，因为账号授权可能需要浏览器、验证码或扫码；工具只能触发、检测、等待和恢复。
- 即梦 CLI 的 stdout/stderr 格式可能变化，解析器必须容错，并保存原始输出。
- 夜间保活是 P0，不能只靠用户手动防睡眠；至少 macOS 要用 `caffeinate` 或等价机制。
- 公开发布后需要避免泄露本机路径、账号信息、token、Cookie、素材隐私。
- 如果 CLI 参数追随官方变化，第一版应支持未知参数透传，降低维护成本。

## 稳定规范引用

- 暂无 `_stable/` 强相关项。

## 持久子需求判断

- 是否需要 `sub-*.md`：否。
- 判断理由：当前是独立项目的总需求和技术方案，执行时可以按模块拆任务，但这些拆分不是长期事实。

## 验收标准引用

- `acceptance.md`
- 技术方案：`tech-design.md`

## 执行交接

- 建议读取顺序：`plan.md` → `tech-design.md` → `acceptance.md` → `grill.md`。
- 执行前必须确认仓库名、License 和即梦 CLI 安装来源。
- 执行层不得把本规格解释为继续扩展现有 Tauri App；第一交付应是可独立安装的 CLI + skill 项目。
- 执行完成声明必须回填 `acceptance.md#执行验收记录`，或创建并链接同目录 `result.md`。

## 变更记录

| 日期 | 变更 | 来源 |
| --- | --- | --- |
| 2026-07-06 | 初稿，确认独立 GitHub 技能/CLI 项目方向 | 用户补充：作为单独技能项目维护，可自动发现/安装 CLI、提醒登录、排队、查记录 |
