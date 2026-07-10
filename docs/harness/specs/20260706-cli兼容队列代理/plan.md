# CLI 兼容队列代理

## 状态

- 状态：confirmed（P0 方向已由用户多轮确认；P1 细节留待执行轮确认）
- 版本归属：无明确版本
- 最近确认人：liuyun（用户）
- 最近确认时间：2026-07-06

## 目标

把当前「即梦调度器 App + MCP 入队 + 本地队列」的能力，收敛为一个面向 agents 和脚本的 **CLI 兼容队列代理层**。

成功后，agent、skill、shell 或 Python 脚本可以像调用即梦 CLI 一样调用新的队列代理；代理在本地负责缓存、入队、并发探测、轮询、失败重试、Fast 兜底和结果落盘。现有 App 不再作为主入口，只作为可选的轻量状态查看/人工干预工具，甚至可以后续退役。

## 用户与场景

- 目标用户：使用 agent/skill/MCP/脚本批量生成即梦视频的用户。
- 主要场景：短视频生产流水线中，agent 生成提示词和素材后，批量提交视频生成任务，并在本地队列中自动等待、重试、查询和收集结果。
- 关键用户路径：
  - agent 以近似即梦 CLI 的参数调用 `dreaminaq`。
  - `dreaminaq` 将任务和素材写入本地缓存/队列。
  - `dreaminaq worker` 或周期性 `dreaminaq once` 推进任务。
  - agent 通过 `dreaminaq status/result --json` 获取状态和产物路径。
  - 需要人工排查时，查看轻量状态页/状态命令，而不是打开复杂任务中心。

## 范围

### 做

- 实现一个 Python 包和命令行工具，暂定 CLI 名称为 `dreaminaq`。
- `dreaminaq` 的视频提交参数尽量兼容原即梦 CLI；新增参数只用于队列语义，例如 `--idempotency-key`、`--reuse exact`、`--force`、`--start-at`、`--queue-model-policy`。
- Python 只作为「队列代理层」：接收参数、缓存素材、记录任务、调度 worker、调用原 dreamina CLI、解析 stdout/stderr、记录 submit_id/queue_info/result。
- 使用 SQLite 作为本地状态库，统一保存 assets、tasks、attempts、results 和事件。
- 实现内容寻址素材缓存：图片/音频按 sha256 入库，避免重复复制和原路径失效。
- 实现任务幂等缓存：`idempotency_key` 相同时返回已有任务；`request_hash` 用于显式复用成功结果。
- 实现 worker/once 推进机制：同一时间只允许一个 worker 推进任务；支持跨平台运行。
- 第一版 worker 必须支持夜间长时间调度：当队列存在未完成任务时，能阻止电脑自动睡眠或提供等价保活机制，避免错过 2 点到 6 点等高效生成窗口。
- 保留 standard/Fast 双模型策略：标准优先，长时间排队或失败时允许 Fast 兜底。
- 输出默认机器可读 JSON，方便 agent 和 skill 消费。
- 产出配套 skill 使用说明，指导 agent 不直接调用即梦 CLI，而是调用 `dreaminaq`。
- 明确现有 Tauri App 的新定位：非主入口；后续只保留轻量状态查看、失败摘要、产物入口和少量手动干预。

### 不做

- 不用 Python 重写即梦官方 API、上传协议、鉴权协议或服务端请求细节。
- 不让 skill 承担后台调度生命周期；skill 只记录调用规范和策略。
- 不继续把复杂任务创建、角色库、生图、完整日志中心作为主体验投入。
- 不要求本轮直接删除现有 Tauri/Rust 实现；Python 代理先以新能力验证，后续再决定迁移或退役。
- 不默认复用同参数历史结果；除非调用方显式传 `--reuse exact`。
- 不把轻量状态展示做成完整桌面工作台。

## 已确认事实

- 当前项目已有 `dreamina-mcp`，提供 `dreamina_queue_video`、`dreamina_queue_videos`、`dreamina_process_queue_once`、`dreamina_get_queue_snapshot`。
- 当前队列核心已经包含本地 store、queue lock、任务状态、执行记录、standard/Fast 车道、轮询、失败重试和 Fast 兜底等经验。
- 用户当前主用法正在从手动创建任务转向 agent/MCP 提交任务。
- 用户倾向于更轻的 agents 时代形态：CLI/skill/脚本主导，App 降级或退役。
- 用户提出新的方向：调用方式尽量与即梦 CLI 一样，只多一层缓存和排队。
- 用户认可 Python 模式可能更适合跨平台复用和 skill 调用，但边界是「队列代理」，不是重写即梦 API。
- 用户明确补充：第一版必须支持晚上长时间不休眠调度能跑起来，否则没有实际作用。

## 推荐假设

- Python 包名使用 `dreamina_queue`，可执行命令使用短名 `dreaminaq`。
- 本地数据目录使用 `~/.dreaminaq/`，避免与当前 `~/.dreamina-scheduler/` 混淆；可提供迁移/导入命令。
- 存储使用 SQLite，素材与结果文件放在同目录下的 `blobs/`、`results/`。
- worker 初版可用普通长跑进程实现，但必须内置保活/防休眠能力；launchd/systemd/Windows Task Scheduler 安装器可以后置。
- 原 dreamina CLI 路径通过配置或环境变量发现，不在 Python 中内置平台特殊路径。
- 轻量展示初版优先做 CLI `status --json` 和 `status --watch`，Web/TUI 状态页可后置。

## 待确认问题

| 优先级 | 问题 | 推荐答案 | 不确认的影响 | 状态 |
| --- | --- | --- | --- | --- |
| P1 | 第一版是否必须提供 Web/TUI 状态页 | 否，先做 `status --json` 和 `status --watch`，Web/TUI 后置 | 若立即做 UI，会拖慢核心 CLI 验证 | open |
| P1 | Python 代理是否需要兼容现有 `.dreamina-scheduler` 数据 | 第一版不直接读写旧 store，只支持导入/并行使用 | 强兼容会增加状态映射成本 | open |
| P1 | `dreaminaq` 参数兼容范围是完全兼容还是常用子集 | 常用视频生成参数先兼容，未知参数透传给 dreamina CLI | 完全兼容需要反向维护官方 CLI 变更 | open |
| P1 | worker 是 Python 长进程还是同时提供系统服务安装 | 第一版长进程 + 文档，服务安装后置 | 服务安装跨平台细节多 | open |
| P1 | 防休眠是否必须覆盖全部平台 | macOS 必须可用；Windows/Linux 至少提供平台适配接口和清晰能力说明 | 全平台一次性做好会增加首版复杂度 | open |
| P2 | 结果复用策略是否需要全局默认开启 | 否，默认不复用，显式 `--reuse exact` 才复用 | 默认复用可能错过用户想要多候选的需求 | recorded |

## 现有系统事实

- `src-tauri/src/bin/dreamina-mcp.rs` 已有 MCP 工具定义和 stdio 调用入口。
- `src-tauri/src/lib.rs` 已有 `queue_mcp_video_task`、`queue_mcp_video_tasks`、`process_queue_for_store_blocking`。
- `docs/harness/specs/20260621-mcp视频排队/result.md` 记录了 MCP 入队、批量排队、共享 `start_at`、queue.lock 的实现事实。
- 当前 App 已多次转向队列监控和双车道状态展示，但用户开始质疑完整可视化监控台是否过重。
- 项目当前 harness 版本为 0.3.0；本需求不做 harness 升级、不改 AGENTS。

## 约束与风险

- 原 dreamina CLI stdout/stderr 格式可能变化，Python 解析必须容错，并保留原始输出。
- 多进程 worker 并发需要 SQLite 事务或文件锁保证单推进。
- 不能让 Python 代理与现有 App/Rust worker 同时推进同一批旧数据，除非共享锁和状态模型已经明确。
- 跨平台路径、文件锁、子进程信号处理和长进程守护方式需要分层设计。
- 夜间防休眠是核心价值路径；如果保活失败，worker 虽然存在但会错过高效排队窗口。
- 默认结果复用会伤害视频多候选生成语义，必须显式启用。
- 如果未来官方即梦 CLI 参数变化，兼容代理需要可透传未知参数，降低维护压力。

## 稳定规范引用

- 暂无 `_stable/` 强相关项。

## 持久子需求判断

- 是否需要 `sub-*.md`：否。
- 判断理由：本需求是一个产品形态和技术架构收敛，不需要为执行拆分创建持久子需求；执行层可以按模块拆任务，但这些拆分不是长期事实。

## 验收标准引用

- `acceptance.md`
- 技术方案：`tech-design.md`

## 执行交接

- 执行层由用户运行时指定；本需求文档不绑定 Superpowers、普通 agent 或其他执行技能。
- 建议读取顺序：`plan.md` → `tech-design.md` → `acceptance.md` → `grill.md`。
- 执行前必须读取 `acceptance.md`。
- 执行完成声明必须回填 `acceptance.md#执行验收记录`，或创建并链接同目录 `result.md`。
- 执行层不得擅自改写本文件的需求事实；如发现事实变化，先记录建议并等待用户确认。

## 变更记录

| 日期 | 变更 | 来源 |
| --- | --- | --- |
| 2026-07-06 | 初稿，确认 Python CLI 兼容队列代理方向 | 用户讨论 + 现有 MCP/队列实现审阅 |
