# 日志中心结构化日志与设计图还原

- 任务键：`log-center-parity`
- 创建日期：2026-05-08
- 状态：草案（方案已确认，准备进入实现规划）
- 任务跟踪：`docs/specs/2026-05-08-log-center-parity/tasks.json`
- 设计依据：用户提供的日志中心功能规划设计图

## Problem

当前日志模块只有后端 `AppData.logs: Vec<String>` 和前端 `LogsView` 的简单文本列表。它只能展示字符串和清空日志，无法支撑设计图中的日志中心能力：统计概览、级别/来源/任务筛选、日志详情、原始输出、关联事件、导出、自动刷新和快速定位。

同时，后端日志打印分散在多个命令中，直接 `data.logs.push(String)`，缺少统一字段、统一保留策略和统一上下文。任务执行、CLI 调用、调度、AI 模型测试、图片生成、素材/角色操作等事件无法被可靠分类，也无法从日志跳回任务或执行记录。

## Goal

以“结构化日志事件 + 旧日志兼容”为基础，尽量一比一还原设计图中的“日志中心”：

1. 后端把日志从纯字符串升级为结构化 `LogEntry`，并兼容读取旧字符串日志。
2. 所有新日志通过统一 `append_log` / `append_task_log` 辅助函数写入。
3. 前端日志中心按设计图重构为：顶部统计卡片、左侧分类、筛选栏、中间日志表格、右侧详情面板、关联事件和底部分页。
4. 日志支持搜索、按级别/来源/任务/时间过滤、自动刷新开关、导出、清空筛选、复制日志、定位任务、查看上下文。
5. 任务执行、查询、并发限制、CLI 安装登录、AI 模型测试、生图、素材/角色/系统设置等关键链路都能打印可追踪日志。

## DO

### 数据模型

- 新增结构化日志模型 `LogEntry`，建议字段：
  - `id`: 稳定唯一 ID，例如 `log_<uuid>`。
  - `timestamp`: RFC3339 时间。
  - `level`: `error | warn | info | success | debug`。
  - `source`: `CLI | Scheduler | Worker | RetryPolicy | System | AI | ImageGen | Asset | Role | Settings`。
  - `category`: `task | cli | error | system | retry | asset | role | imagegen | ai | settings`。
  - `event_type`: 机器可读事件类型，例如 `task.submit.failed`、`cli.install.completed`。
  - `message`: 一行摘要，用于表格摘要列。
  - `detail`: 人类可读详情，用于右侧日志详情。
  - `task_id`, `task_title`, `submit_id`, `execution_record_id`, `attempt_id`: 可选关联字段。
  - `module`: 线程/模块，例如 `cli.executor / concurrency_guard`。
  - `command_preview`: 可选命令预览。
  - `stdout`, `stderr`, `raw_output`: 已截断的原始输出/堆栈。
  - `error_kind`, `error_detail`: 错误分类和错误详情。
  - `duration_seconds`: 可选耗时。
  - `metadata`: `serde_json::Value`，存放扩展字段。
- 旧字符串日志迁移为 `LogEntry`：
  - `level = info`
  - `source = System`
  - `category = system`
  - `message = 原字符串前 120 字`
  - `detail = 原字符串全文`
  - `event_type = legacy.string_log`
- 保留现有 `settings.log_retention_count`，但对结构化日志按条数保留。
- 日志写入必须统一截断大文本，避免 `stdout/stderr/raw_output` 无限膨胀。

### 后端日志打印

- 新增统一辅助函数：
  - `append_log(data, LogEntryDraft)`：生成 ID、时间、默认字段、执行保留裁剪。
  - `append_task_log(data, task, patch)`：自动带上任务标题、task_id、submit_id、execution_record_id 等上下文。
  - `truncate_log_field(value)`：复用/替代现有 `truncate_log`，用于字段级截断。
- 替换直接 `data.logs.push(format!(...))` 的关键路径：
  - 素材导入、临时图片、角色媒体导入/移除、角色新增/更新/删除。
  - 草稿保存、任务创建、任务更新/删除/暂停/恢复/重新排布。
  - 队列无到期任务、预定到期补偿、队列执行完成。
  - 任务提交成功/失败、查询成功/失败/处理中、自动查询停止、并发限制重试。
  - CLI 安装、CLI 登录。
  - AI 模型测试成功/失败/响应解析失败。
  - 图片生成成功/失败。
  - 设置更新、日志清空。
- 日志等级建议：
  - `error`: 执行失败、查询失败、AI/生图失败、解析失败、无法找到任务/素材。
  - `warn`: 并发限制、重试、自动查询停止、队列补偿、配置缺失但可恢复。
  - `info`: 常规任务状态、导入、设置、CLI 普通输出。
  - `success`: 任务成功、生图成功、CLI 安装/登录成功。
  - `debug`: 暂不在 UI 默认展示，可预留。

### 前端日志中心 UI

- 重写 `LogsView`，视觉结构尽量对齐设计图：
  - 页面标题：`日志中心`。
  - 副标题：`查看任务执行日志、CLI 输出、错误信息与系统事件`。
  - 顶部统计卡片：
    - 今日日志
    - 错误
    - 警告
    - 信息
    - 日志保留（显示 `N 天` 或当前保留条数策略；若没有天数策略，则显示 `保留 X 条`）
  - 筛选工具栏：
    - 搜索框：搜索日志内容、`submit_id`、任务名。
    - 级别：全部 / ERROR / WARN / INFO / SUCCESS。
    - 来源：全部 / CLI / Scheduler / Worker / RetryPolicy / System / AI / ImageGen / Asset / Role / Settings。
    - 任务：全部 / 当前有日志任务。
    - 时间：最近 1 小时 / 最近 24 小时 / 最近 7 天 / 全部。
    - 自动刷新开关。
    - 导出日志按钮。
    - 清空筛选按钮。
  - 左侧分类：
    - 全部日志
    - 任务日志
    - CLI 日志
    - 错误日志
    - 系统事件
    - 重试记录
  - 左侧快速定位：
    - 今日错误
    - 最近 1 小时
    - 我关注的任务（可先定义为当前选中/最近任务，若无则显示 0）
    - 高频错误 Top10
    - 添加过滤器按钮（首版可禁用或作为后续入口）
  - 中间日志表格：
    - 时间
    - 级别
    - 来源
    - 任务 / submit_id
    - 摘要
    - 支持选中行，选中后右侧详情同步更新。
  - 右侧日志详情：
    - 级别 Badge。
    - 时间、来源、任务名、`submit_id`、线程/模块。
    - 日志消息/详情。
    - 原始输出/堆栈代码块，支持复制。
    - 关联事件列表（同 `task_id` 或同 `submit_id` 的前后事件，最多 3 条）。
    - 操作按钮：复制日志、定位任务、查看上下文。
  - 底部分页：总数、页码、每页条数。
- 前端派生工具建议新增 `src/log-view-utils.js`：
  - 规范化旧/新日志。
  - 统计卡片计算。
  - 分类计数。
  - 筛选、搜索、排序、分页。
  - 详情面板关联事件派生。
  - 导出文本/JSON 内容生成。

### 设计图对齐细节（UI 验收基准）

- 页面整体：
  - 继续使用现有 App shell（左侧主导航 + 顶部窗口栏），不重做壳层。
  - 日志页内容区采用 `height: calc(100vh - 56px)`，内部滚动收敛在表格/详情区域，避免整页滚动破坏设计图的固定工作台感。
  - 内容区 padding 对齐队列中心风格：左右约 `24px`、顶部约 `18px`、区块间距 `14-16px`。
  - 背景使用 `var(--page)`，卡片使用 `var(--panel)`，边框使用 `var(--border)`，不新增不一致的灰阶体系。
- 标题区：
  - 主标题为 `日志中心`，副标题为 `查看任务执行日志、CLI 输出、错误信息与系统事件`。
  - 标题区高度紧凑，主标题视觉层级应接近角色库/队列中心标题，不要使用过大的营销页标题。
  - 顶部操作按钮与标题在同一视觉行或紧邻下一行，避免挤压统计卡片。
- 统计卡片：
  - 一行 5 张卡片：今日日志、错误、警告、信息、日志保留。
  - 设计尺寸：高度约 `74-82px`，圆角 `12px`，卡片间距 `12px`。
  - 每张卡片包含左侧图标容器、标题、主数值、变化/说明小字；图标颜色与指标语义一致。
  - 错误/警告/信息/成功色使用现有 `StatusPill` 语义色扩展，不直接散落硬编码颜色。
  - 当窗口宽度不足时允许卡片横向压缩，但不换成单列移动端布局；当前 app 最小宽度是桌面工作台。
- 筛选工具栏：
  - 高度约等于现有 `--control-h`（36px），所有输入、select、按钮垂直居中。
  - 搜索框占主要宽度，左侧带 `Search` 图标，placeholder 对齐设计图：`搜索日志内容、submit_id、任务名...`。
  - select 过滤器宽度紧凑：级别/来源/任务/时间各约 `104-132px`，不占满整行。
  - 自动刷新用现有 `ToggleSwitch` 的紧凑变体或抽取 `UiSwitch`；视觉上保持紫色开启态。
  - 导出/清空按钮复用 `outline-button`/`icon-ghost` 体系，图标大小 `13-14px`。
- 主体三栏：
  - 左侧分类栏宽度约 `130-150px`，中间表格 `minmax(0, 1fr)`，右侧详情 `300-340px`。
  - 三栏之间间距约 `14px`，高度一致，右侧详情固定在当前日志页内部，不随页面外层滚动。
  - 左侧分类项高度约 `34-38px`，激活态使用淡紫背景和主色文字，计数右对齐。
  - 快速定位区域与分类之间留 `16-18px` 分隔，可使用小标题 `快速定位`。
- 日志表格：
  - 表头 sticky 在表格容器顶部，背景 `#f8f9fd` 或现有浅灰，字体 `11px/800`。
  - 行高约 `44-48px`，时间列窄、级别列用 badge、来源列短文本、任务列两行（任务名 + submit_id）、摘要列自动截断。
  - 选中行要有左侧 2px 主色边或淡紫描边，整体背景接近设计图的浅紫选中态。
  - hover 态轻微变色，不改变布局高度。
- 右侧详情：
  - 详情卡片顶部展示 level badge，右侧可显示复制按钮或状态。
  - 字段区使用 label/value 两列：label `11px` muted，value `12px` 深色，行距 `8-10px`。
  - `submit_id` 旁边有复制小按钮，复用 `CopyableInput` 的复制逻辑但不一定使用 input 外观。
  - 日志详情和原始输出分块展示，原始输出使用等宽字体、深/浅代码块均可，但必须支持横向滚动和截断提示。
  - 关联事件以小列表展示：时间、level badge、来源、摘要，最多 3 条。
  - 底部操作按钮固定在详情卡片底部附近：复制日志、定位任务、查看上下文；主按钮使用 `gradient-button`。
- 分页：
  - 底部左侧显示 `共 N 条日志`，中间页码，右侧每页条数 select。
  - 样式优先复用队列中心分页按钮和 `pagination-*` 类，避免新建另一套分页视觉。
- 空状态和错误态：
  - 无日志时中间表格区域显示图标 + `暂无日志`，右侧详情显示 `选择日志查看详情`。
  - 筛选无结果时显示 `无匹配日志`，同时保留筛选栏和左侧分类。
  - 字段缺失显示 `-`，不得显示 `undefined` / `null`。

### UI 基建与组件复用/升级

- 必须优先复用现有 UI 基建：
  - `StatusPill`：用于日志 level badge，并扩展或映射 `error/warn/info/success/debug` 到现有 `bad/warn/info/ok/neutral`。
  - `ToggleSwitch`：用于自动刷新；如尺寸不合适，允许增加 `size=\"compact\"` 或新增 class，而不是复制一份开关实现。
  - `CopyableInput`：复用其复制反馈逻辑；如详情字段不适合 input 外观，可抽取 `CopyButton` 或 `InlineCopy`。
  - `PanelHeading` / `Metric`：可作为参考，但若只存在于 `main.jsx` 内且不适合复用，应抽到 `src/components/ui/` 后再用于日志中心。
  - 现有 `.queue-table` / `.pagination-*` / `.button-cluster` / `.outline-button` / `.gradient-button` 视觉规则应作为基线。
- 建议新增或升级的通用组件：
  - `UiStatCard`：统一统计卡片结构（图标、标题、数值、sub、tone），可替代/兼容现有 `Metric`。
  - `UiSearchBox`：带图标搜索框，供角色库、队列中心、日志中心未来统一。
  - `UiFilterSelect`：统一筛选 select 宽度、label、compact 尺寸。
  - `UiDataTable` 或轻量 `LogTable`：首版可只做日志专用，但表头/行/选中态应沉淀为可复用 class。
  - `UiDetailPanel`：右侧详情面板壳层，统一 header/body/footer、固定底部操作区。
  - `InlineCopyButton`：用于 `submit_id`、原始输出、日志详情复制。
  - `UiPager`：复用现有 pagination 样式，避免每个页面手写分页。
- 组件升级原则：
  - 若一个 UI 结构只在日志页使用且短期无复用价值，可以先以 `Log*` 局部组件实现。
  - 若结构已在 2 个以上页面出现（统计卡片、搜索框、分页、详情面板、复制按钮），应抽到 `src/components/ui/`。
  - 抽组件不能引入新依赖，不能改变现有页面行为；旧页面复用升级必须有回归验证。
  - 新增组件样式应放在 `styles.css` 的组件库区域或明确的 Log Center 区域，命名使用 `ui-*` 或 `log-*`，避免与旧页面 class 冲突。
- 日志中心首版建议组件边界：
  - `src/components/ui/StatCard.jsx`
  - `src/components/ui/SearchBox.jsx`
  - `src/components/ui/FilterSelect.jsx`
  - `src/components/ui/InlineCopyButton.jsx`
  - `src/components/ui/Pager.jsx`
  - 日志页局部组件可保留在 `main.jsx` 内：`LogLevelBadge`、`LogCategorySidebar`、`LogTable`、`LogDetailPanel`；若函数过长，再拆到 `src/components/logs/`。
- UI 验证要求：
  - 实现完成后需要打开设计图逐区对照：标题区、统计卡片、筛选栏、左侧分类、表格、详情、分页。
  - 如果某个设计图元素因现有系统约束无法一比一实现，必须在 Decision Log 写明偏差和原因。

### 交互行为

- 默认选中最新一条 `error` 日志；若没有 error，则选中最新一条日志。
- 搜索和筛选变化后，如果当前选中日志不在结果中，自动选中结果第一条。
- 自动刷新开关首版只控制前端是否沿用现有状态刷新结果；不要求新增后端实时推送。
- 导出日志首版使用浏览器下载 `.json` 或 `.txt`，不要求后端文件保存。
- 清空日志仍调用现有 `clear_logs_command`，但清空动作本身可以不再写日志，避免清空后立刻出现新日志；若要记录清空事件，需用户确认。
- 定位任务：若 `task_id` 存在，切换到任务中心并选中任务；如果任务不存在，展示反馈。
- 查看上下文：首版可等同于按 `task_id/submit_id` 临时筛选或滚动到关联事件。

### 允许编辑区域

- `src-tauri/src/lib.rs`
- `src/main.jsx`
- `src/styles.css`
- 新增 `src/log-view-utils.js`
- 新增/更新 `src/*.test.mjs` 中日志相关测试
- 新增/升级 `src/components/ui/*` 中必要通用组件
- 必要时新增 `src/components/logs/*` 局部日志组件
- 当前规格目录：`docs/specs/2026-05-08-log-center-parity/`
- 必要时更新 `docs/更新文档/v0.2.0.md`

## DON'T

### 范围外

- 不引入外部日志服务、数据库、SQLite、文件滚动日志或全文索引引擎。
- 不实现实时流式日志推送；首版继续依赖 App 状态刷新。
- 不做多用户审计、安全审计或不可篡改日志。
- 不做日志保留“按天数”真实删除策略，除非用户另行确认；当前继续使用条数保留。
- 不重构任务中心和任务详情的核心业务，只允许为了“定位任务”接入必要 props/state。
- 不更改现有任务执行语义；日志增强不能影响任务状态机。
- 不把完整大 stdout/stderr 无限制持久化。

### 禁止改动

- 不删除旧日志数据兼容能力。
- 不改变 `settings.log_retention_count` 的语义。
- 不把日志 UI 状态持久化到后端，除非后续明确要求。
- 不为了 UI 展示把后端错误吞掉或降级为成功。

### 升级决策点

- 如果结构化日志迁移需要破坏现有 `data.json` 格式，必须停止并询问。
- 如果“导出日志”需要系统文件保存对话框，而不是浏览器下载，需用户确认。
- 如果“日志保留 30 天”必须真实按天数实现，需要单独扩展设置项并确认 UI 文案。
- 如果“添加过滤器”要做成可保存的高级过滤器，需要另开规格。

## VERIFY

### 静态与自动化

1. `npm test` 通过。
2. `npm run build` 通过。
3. `cargo test` 或至少 `cargo check` 通过。
4. 新增 `log-view-utils` 单测覆盖：
   - 旧字符串日志规范化。
   - level/source/category/time/search 组合筛选。
   - 统计卡片计数。
   - 关联事件派生。
   - 分页。
5. 后端结构化日志单测或 Rust 单元测试覆盖：
   - 旧 `Vec<String>` 能加载为结构化日志。
   - retention 裁剪仍按条数生效。
   - `append_task_log` 自动填充任务上下文。
6. 代码搜索确认关键直接写入已收敛：除迁移/测试外，不再新增裸 `data.logs.push(format!(...))` 字符串写法。
7. UI 组件基建检查：
   - 日志 level badge 复用 `StatusPill` 或其扩展，不新建重复 badge 体系。
   - 自动刷新复用 `ToggleSwitch` 或其 compact 扩展。
   - 分页复用/升级现有 pagination 样式。
   - 复制动作复用/抽取统一复制按钮逻辑。
8. CSS 命名检查：新增样式使用 `log-*` 或 `ui-*` 前缀，不污染已有页面 class。

### 手动验收

1. 打开日志页，整体布局与设计图一致：顶部统计、左侧分类、中间表格、右侧详情四区齐全。
2. 触发任务创建、队列执行、失败查询、CLI 检测/登录或 AI 模型测试后，日志能按正确 level/source/category 出现。
3. 搜索任务名或 submit_id 能命中对应日志。
4. 级别筛选 `ERROR/WARN/INFO/SUCCESS` 正常。
5. 来源筛选 `CLI/Scheduler/Worker/System/AI/ImageGen` 正常。
6. 点击日志行，右侧详情显示时间、来源、任务、submit_id、模块、详情和原始输出。
7. 关联事件能显示同任务或同 submit_id 的前后事件。
8. 复制日志能把详情复制到剪贴板。
9. 定位任务能跳到任务中心并选中对应任务；无任务关联时按钮禁用或提示。
10. 导出日志能生成包含当前筛选结果的文件内容。
11. 清空日志后页面显示空状态，不出现异常。
12. 使用设计图逐区对照 UI：统计卡片高度/间距、筛选栏控件高度、三栏宽度、表格行高、详情字段密度、分页位置均与设计图接近。
13. 与现有 app 视觉系统对照：字体大小、按钮样式、卡片圆角、边框颜色、主色激活态不突兀。

## Governance Card

### DO

- allowed edits:
  - `src-tauri/src/lib.rs`
  - `src/main.jsx`
  - `src/styles.css`
  - `src/components/ui/*`
  - `src/components/logs/*`（如拆出日志局部组件）
  - `src/log-view-utils.js`
  - `src/*log*.test.mjs`
  - `docs/specs/2026-05-08-log-center-parity/*`
  - `docs/更新文档/v0.2.0.md`（若实现纳入当前版本）
- required doc sync:
  - 实现中如果字段、筛选项、日志等级或交互与本规格不同，必须更新本 spec 的 Decision Log。
  - 实现中如果 UI 对齐细节或组件抽取策略与本规格不同，必须更新 Decision Log。
  - 任务状态只更新 `tasks.json`，不要散落在聊天记录中。

### DON'T

- forbidden edits:
  - 不改任务状态机的业务含义。
  - 不改素材、角色、生图、AI 模型的核心行为，只加日志。
  - 不引入新依赖，除非用户确认。
- ask or stop when:
  - 需要破坏旧日志数据格式。
  - 需要真实“按天保留”而非“按条数保留”。
  - 需要后端文件导出或系统保存对话框。

### VERIFY

- static or type checks:
  - `npm test`
  - `npm run build`
  - `cargo check` 或 `cargo test`
- focused tests:
  - `node --test src/log-view-utils.test.mjs`
  - Rust 日志迁移/retention/append helper 测试
- manual checks:
  - 按 `VERIFY → 手动验收` 1-11 逐项确认。

## Decision Log

### 2026-05-08 — 方案确认

- 决定：采用“结构化日志事件 + 前端日志中心还原设计图 + 旧日志兼容”。
- 原因：仅重做 UI 无法支撑筛选/详情/关联事件；完整事件溯源系统当前过重。
- 影响：需要先改后端日志 schema 和统一写入 helper，再重构前端日志中心。
