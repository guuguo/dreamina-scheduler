# 验收标准

## 状态

- 状态：confirmed
- P0 是否已由用户确认：是
- 最近确认时间：2026-08-04

## 验收表

| 编号 | 对应来源 | 验收条件 | 重要性 | 证据要求 | 状态 |
| --- | --- | --- | --- | --- | --- |
| A1 | `plan.md#目标` | README 首屏在标题、短说明和首个能力区明确传达“不间断排队” | P0 | README 人工审阅 | passed |
| A2 | `plan.md#范围` | README 嵌入至少 2 张当前正式界面的重点功能截图 | P0 | 图片文件与 Markdown 图片引用 | passed |
| A3 | `plan.md#不变项` | 截图不含真实任务、提示词、账号、密钥或个人路径 | P0 | 截图人工检查 | passed |
| A4 | `plan.md#范围` | README 准确说明自动重试、自动查询、防休眠与 App 退出后的边界 | P0 | README 与代码/设置项对照 | passed |
| A5 | `plan.md#本期最小闭环` | README 图片路径、内部锚点、安装/测试/构建命令可解析 | P1 | Markdown 链接检查与命令检查 | passed |
| A6 | `plan.md#不做` | 未修改产品功能代码，未覆盖工作区原有未提交改动 | P0 | `git diff -- README.md docs/...` 与 `git status` | passed |
| A7 | `plan.md#复杂度预算` | 未新增运行时机制或第二事实源 | P0 | 变更文件清单 | passed |
| A8 | `plan.md#本期最小闭环` | README 提供可执行的 MCP / Agent 接入说明，并准确区分 App 自动调度与纯 MCP 单步推进 | P0 | MCP 构建、`tools/list` 冒烟与 README 人工审阅 | passed |

## 不作为通过依据

- 只改了标题，但没有重构信息层级。
- 使用原型图冒充当前正式界面。
- 截图来自真实用户数据，或经过裁切仍可识别敏感信息。
- 宣传文案承诺 App 退出、关机或未支持平台仍可持续执行。

## 待确认验收

无。

## 执行验收记录

- 执行状态：passed
- 执行方式：Codex + `gg-harness`；截图阶段使用隔离演示实例
- 执行时间：2026-08-04
- 执行者/会话：Codex 当前任务
- 关联提交/PR/变更摘要：`c7347fd` 完成首轮 README 与截图；本轮补充 MCP / Agent 接入说明与验收证据
- 详细结果文件：无

### 验收项结果

| 编号 | 状态 | 实际证据 | 说明 |
| --- | --- | --- | --- |
| A1 | passed | `README.md` 首屏主张为“让即梦视频任务持续排队，空出并发就自动补上” | 首屏与工作原理均围绕不间断排队 |
| A2 | passed | `docs/images/queue-center.png`、`docs/images/queue-settings.png` | 两张图片均被 README 相对路径引用 |
| A3 | passed | 隔离目录 `/tmp/dreamina-readme-demo` + 截图人工查看 | 只含虚构任务；已裁去顶部 CLI 账户状态区域；设置图无真实密钥 |
| A4 | passed | README `不间断排队是怎么工作的`、`运行边界`；Rust 定向测试 | 自动重试、查询间隔、双车道状态和 macOS 防睡眠测试通过 |
| A5 | passed | 图片存在性、编码、标题与 `git diff --check` 检查；`npm test`、`npm run build` | 命令与相对图片路径可用 |
| A6 | passed | `git status --short` 与变更文件核对 | 仅 README、图片和本需求文档属于本轮；原有源码改动未触碰 |
| A7 | passed | 本轮变更文件清单 | 无运行时代码、表、状态、线程或调度机制变更 |
| A8 | passed | Release 构建成功；stdio `initialize + tools/list` 返回 `dreamina-scheduler 0.2.5` 与 6 个预期工具 | README 同时写明 App 常驻调度、纯 MCP 单步推进和共享数据目录 |

### 验证命令与结果

| 命令/检查 | 结果 | 关键输出或证据位置 |
| --- | --- | --- |
| `npm test` | pass | 300 tests passed，0 failed |
| `npm run build` | pass | Vite production build 完成，1900 modules transformed |
| `cargo test query_interval_` | pass | 5 passed，覆盖按远端状态/排位自适应查询 |
| `cargo test lane_status_` | pass | 4 passed，覆盖双车道、Fast 执行记录与并发冷却 |
| `cargo test macos_keep_awake_prevents_system_sleep_without_forcing_display_awake` | pass | 1 passed |
| `cargo test submit_with_submit_id_and_exceed_concurrency_limit_enters_retry_wait` | pass | 1 passed |
| `git diff --check` | pass | 无空白错误 |
| README 图片与编码检查 | pass | 两张 PNG 存在且非空；README 无 Unicode replacement character |
| `cargo build --release --manifest-path src-tauri/Cargo.toml --bin dreamina-mcp` | pass | Release MCP 二进制构建成功 |
| MCP stdio `initialize + tools/list` | pass | Server `dreamina-scheduler 0.2.5`；返回 6 个 README 所列工具 |
| `cargo test --manifest-path src-tauri/Cargo.toml --bin dreamina-mcp` | pass | 4 passed，覆盖标准、嵌套、别名包装与扁平参数 |

### 独立审查

- Spec review：通过；按 `plan.md` 范围与事实逐项回看
- Code review：不适用；不改产品代码
- 人工验收：执行者已检查截图隐私、清晰度与文案；最终视觉偏好待用户确认

### 未验证项与风险

- Harness 项目版本仍为 0.3.0，本轮按范围未执行 0.6.0 升级。
- 仓库当前没有独立 `LICENSE` 文件；README 沿用项目原有的 MIT 声明，未在本轮扩大范围补建许可证文件。
