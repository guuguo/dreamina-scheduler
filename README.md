<h1 align="center">Dreamina Scheduler</h1>

<p align="center">
  <strong>桌面化任务编排 · 素材复用 · 队列自动查询</strong><br>
  为 Dreamina CLI 熟手打造的 Tauri 桌面 App，<br>
  把 <code>multimodal2video</code> 的角色管理、素材引用和任务队列收进一个窗口。
</p>

<p align="center">
  <a href="#安装与启动">快速开始</a> · <a href="#典型工作流">使用方案</a> · <a href="#当前限制">已知限制</a> · <a href="#roadmap">Roadmap</a>
</p>

---

## 为什么不用纯 CLI

Dreamina CLI 已经能提交视频任务，但反复敲命令做这些事很累：

- **角色素材散落各处** — 每次提交都要重新找图、找音频、拼路径
- **并发限制靠手动重试** — 碰到 `ConcurrencyLimit` 只能自己等、自己重跑
- **任务结果靠人肉追踪** — `submit_id` 一多就容易漏查

Dreamina Scheduler 把这些收进桌面：角色库一次建好、素材拖拽导入、Prompt 里 `@角色` 直接引用、队列自动重试和轮询结果。

## 核心特性

- 🎭 **角色库** — 统一管理角色参考图和音色音频，拖拽或选择器导入
- 📝 **Prompt 引用** — 编辑器内 `@角色` / `@图片N` 自动补全，素材路径随角色绑定
- 🖼️ **生图工作流** — 调用 AI 图片模型生成角色参考图或分镜，结果直接挂回角色/任务
- ⏱️ **任务队列** — 运行一次或启动自动队列，并发冲突自动静默重试
- 🔍 **结果追踪** — 按 `submit_id` 自动查询，展示 attempt、错误摘要和结果路径
- ⚙️ **CLI 协同** — 设置页触发 CLI 安装和登录，重试策略可配置

## 适用人群

- 长期使用 Dreamina CLI 的创作者，希望减少重复操作
- 需要管理多个角色和大量素材的视频制作者
- 想用队列批量提交并自动追踪结果的效率型用户

## 安装与启动

### 前置要求

- Node.js ≥ 18
- Rust 工具链（Tauri 构建）
- Dreamina CLI 已安装并登录（App 内也可触发安装）

### 开发模式

```bash
npm install
npm run tauri:dev
```

### 构建

```bash
npm run tauri:build
```

macOS 构建产物在 `src-tauri/target/release/bundle/`。

> ⚠️ 当前 npm scripts 使用 `tauri:dev` / `tauri:build`，而非旧版 `dreamina-scheduler:dev` / `dreamina-scheduler:build`。请以 `package.json` 中的 scripts 为准。

## 典型工作流

### 1. 首次配置 CLI

进入 **设置页** → 点击"安装 CLI"（macOS 使用官方脚本自动安装）→ 点击"登录 CLI"完成授权。Windows 需手动填入已确认���的 PowerShell 安装命令。

### 2. 建角色与导入素材

进入 **角色库** → 新建角色 → 通过选择器或拖拽导入参考图和音色音频。素材自动复制到 App 内缓存目录，后续引用无需再找路径。

### 3. 写 Prompt 并引用素材

进入 **任务中心** → 新建任务 → 在 Prompt 编辑器中输入 `@` 触发角色/图片补全 → 选中的角色会自动带上图片和音频参数。也可粘贴临时图片，Prompt 中显示为 `@图片N`。

### 4. 提交与队列管理

选择模型（`seedance2.0` 或 `seedance2.0fast`）、宽高比、时长 → 点击"运行一次"即时提交，或"启动自动队列"批量提交。并发冲突会被识别并静默重试（策略在设置页配置）。

### 5. 查看结果与失败排查

任务中心实时展示队列状态。任务详情页可查看 attempt 次数、错误摘要和结果路径/URL。日志中心记录完整的命令执行和响应信息。

## 与 Dreamina CLI 的关系

| 维度 | CLI | Scheduler |
|------|-----|-----------|
| 素材管理 | 手动拼路径 | 角色库 + 拖拽导入 + `@` 引用 |
| 并发冲突 | 手动等待重跑 | 自动识别 + 静默重试 |
| 结果追踪 | 手动 `submit_id` 查询 | 自动轮询 + 详情展示 |
| 批量提交 | 循环脚本 | 队列模式 |
| 适用场景 | 单次快速调用 | 日常批量生产 |

Scheduler 不替代 CLI，而是在 CLI 之上提供桌面化的编排和自动化层。CLI 仍然是底层执行引擎。

## 当前限制

- 队列仅在 App 运行期间生效，App 退出或电脑休眠后不会后台执行
- 仅支持 `multimodal2video` 任务类型
- 视频素材暂不支持，提交只使用 `--image` 和 `--audio`
- Windows CLI 一键安装命令尚未确认官方默认源
- CLI 安装/登录返回摘要，暂无实时日志流或二维码展示
- 角色创建表单保留路径输入作为兜底，常规建议用选择器或拖拽

## Roadmap

- [ ] 下载目录配置与结果一键打开
- [ ] 更完整的查询超时策略
- [ ] 视频素材支持
- [ ] 更多任务类型（图片生成等）
- [ ] CLI 实时日志流与二维码展示
- [ ] Windows CLI 一键安装

## 贡献

欢迎提 Issue 和 PR。提交前请确认：

- Node.js 测试通过：`npm test`
- Tauri 构建通过：`npm run tauri:build`
- 改动不影响现有角色/任务数据

## License

MIT
