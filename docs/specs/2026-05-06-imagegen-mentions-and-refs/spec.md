# 生图模块复用 @ 提示词与素材参考图

- 任务键：`imagegen-mentions-and-refs`
- 创建日期：2026-05-06
- 状态：草案（已 stable，准备进入实现阶段）
- 任务跟踪：`docs/specs/2026-05-06-imagegen-mentions-and-refs/tasks.json`

## Problem

`ImageGenView`（"生图" Tab）当前用一个朴素 `<textarea>`，无法复用"新建任务"页已经成熟的 @ 提示词能力（mention 角色 / 角色图 / 临时图）；也不支持选择素材图作为生图参考；粘贴行为完全没有处理。结果：

- 用户必须把已经入库的素材路径手动填进 prompt，或者干脆放弃使用素材，导致生图结果与项目语境脱节。
- 后端 `generate_image_command` 只发 `images/generations`（纯文本到图）；无路径可走"图生图 / 编辑"。
- 与"新建任务"双轨实现，未来 prompt 体验改动需要两边同时维护。

## Goal

让 `ImageGenView` 的 prompt 编辑体验**与"新建任务"对齐**：

1. prompt 用 `PromptMentionEditor`（Tiptap + MaterialMentionPicker），支持 `@` 选择素材；
2. 支持"添加素材图"按钮（同 `addTempImage`），选中后自动作为参考图；
3. 支持直接在 prompt 内粘贴图片（不接受非图片粘贴行为）；
4. 后端在有参考图时切换到 `images/edits` 端点，把素材文件作为 multipart `image[]` 上传；
5. 整个生图请求（prompt + size + reference assets）一次成功并落入历史记录。

## DO

### 范围内

- **前端 `ImageGenView`**
  - 用 `PromptMentionEditor` 替换 `<textarea>`。
  - 复用 `buildMentionItems()`，但 picker 内仅展示 image / temp_image 两类（音频和单纯角色对生图无意义；保留 `role` 选项，因为角色挂载的图也会通过 role 间接进入）。
  - 新增 `imagegenForm` 状态：`{ prompt, size, temp_image_paths, temp_image_asset_ids, image_asset_ids }`，结构沿用 taskForm 中 image 部分的子集。
  - 复用 App 顶层 `addTempImage` / `pasteClipboardImage` / `pasteSystemClipboardImage`，向 `ImageGenView` 通过 props 透传。
  - 在 prompt 下方加"参考图缩略图"区域：展示当前 `image_asset_ids` 对应的素材图（最多 9 张），每张可单独删除（仅从生图本地表单移除，不删素材）。
  - 加"添加素材图"按钮（next to "生成" 按钮），点击触发 `addTempImage` 但写入到 `imagegenForm` 的 `temp_image_paths` / `temp_image_asset_ids` / `image_asset_ids`，**不污染 taskForm**。

- **`App` 层**
  - 暴露通用版 `addTempImage` / paste 函数：当前实现耦合了 `setTaskForm`，需重构成"返回 asset 数组"的纯 IO 函数（`importTempImageAssets(paths)` / `pasteClipboardImageAsset(file)` / `pasteSystemClipboardImageAsset()`），由 `CreateTaskView` 与 `ImageGenView` 各自决定写入到自己表单。
  - 把 `state` 与 `assetById` 也透传给 `ImageGenView`，用于 `buildMentionItems`。

- **后端 `generate_image_command`**
  - 入参增加 `reference_asset_ids: Vec<String>`（默认空）。
  - 当 `reference_asset_ids` 非空：
    - 查 `AppStore` 拿 asset → `stored_path`；
    - 改用 `POST {base_url}/images/edits`，multipart/form-data：
      - text 字段：`model`、`prompt`、`size`、`n=1`、`response_format=b64_json`；
      - file 字段：每个 asset 以 `image[]` 重复上传（参考 OpenAI gpt-image-1 多图编辑约定）。
  - 当 `reference_asset_ids` 为空：保持现有 `images/generations` 路径不变。
  - 复用现有 `describe_error` / `truncate` / b64 + url 兼容解析逻辑。

- **历史记录**
  - 历史 item 增加 `reference_asset_ids: string[]`、`reference_thumb_paths: string[]`（用 `convertFileSrc` 渲染缩略图）。
  - 渲染历史项时显示参考图小角标（如 "参考图 ×3"），点击可在预览面板看到原始参考图列表。

### 约束与假设

- OpenAI 官方 `gpt-image-1` 支持 `images/edits` 多图，3rd-party 兼容代理（如 geekai.co）行为待验证；规格不假设特定厂商，只保证调用符合 OpenAI 协议格式。
- 参考图最大数量与"新建任务"对齐为 **9 张**（仅前端约束；后端只透传，不强行截断）。
- 参考图类型仅支持 `image_asset_ids`；`audio_asset_ids` 不在范围。
- prompt 字符上限沿用 `PromptMentionEditor` 默认 1000，不为生图单独调。

### 允许的编辑

- 重写 `ImageGenView`（`src/main.jsx` 中函数）。
- 重构 App 顶层 `addTempImage` / paste 函数签名。
- 修改 `src-tauri/src/lib.rs` 中 `generate_image_command`，新增 multipart 分支。
- `src-tauri/Cargo.toml` 增加 `reqwest` 的 `multipart` feature。
- 新增工具文件 `src/imagegen-utils.js` 放置生图表单初值与 mention items 过滤函数。

## DON'T

### 范围外

- 不动 `CreateTaskView` 与 taskForm 的现有逻辑（仅适配 App 顶层重构后的函数签名）。
- 不动 `PromptMentionEditor` / `MaterialMentionPicker` 内部实现（只做调用方差异化）。
- 不实现"生成中"状态跨进程恢复（依旧是前端 React state）。
- 不引入新的图片来源（不接 URL 远程下载、不接拖拽到 dropzone）。
- 不重做生图历史 schema（仍是 localStorage `imagegen_history_v1`，仅向后兼容地新增可选字段）。
- 不在生图 prompt 内出现"角色"概念的额外副作用（不会写回 `taskForm.role_ids`，互不干扰）。

### 禁止的改动

- 不要在 `commands` 模块外暴露 reqwest 的 multipart 类型。
- 不要把素材文件读到内存后写到 base64 prompt 字符串里（违背 OpenAI 协议且体积爆炸）。
- 不要持久化 prompt 草稿（重启清空，与"新建任务"行为对齐——它本身也是前端表单）。

### 升级决策点（需用户确认）

- 若 geekai.co 代理 **不支持** `images/edits` 协议，是否退化为：把参考图编码进 prompt（不推荐）/ 直接报错提示用户切换 base URL？
- 是否需要给历史记录 schema 升版（`imagegen_history_v2`）？当前选择"宽松向后兼容"。

## VERIFY

### 静态

1. `cargo check` 无新增 error / warning。
2. `npm run build` 通过，无 TypeScript / JSX 报错。
3. `src/main.jsx` 中 `ImageGenView` 不再出现 `<textarea`（除非作为 PromptMentionEditor 内部实现）。
4. `src-tauri/src/lib.rs` `generate_image_command` 函数签名包含 `reference_asset_ids: Vec<String>`。
5. `src-tauri/Cargo.toml` `reqwest` features 包含 `multipart`。

### 集中测试（手动）

1. **纯 prompt 生图**（无参考图）：调用走 `images/generations`，结果落入历史，历史项不显示参考图标记。
2. **添加素材图按钮**：点击 → 文件选择对话框 → 选 1 张 PNG → prompt 下方出现该图缩略图 → 生成成功。
3. **粘贴图片**：在 prompt 编辑器内 `Cmd+V` 粘贴系统剪贴板图片 → 自动插入 `@分镜图N` mention 节点 → 同时进入参考图列表。
4. **粘贴非图片**：粘贴纯文本 / URL → 走默认行为（普通文本插入），不弹错误。
5. **@ 选素材图**：输入 `@` → 弹出 picker → 选一张已入库的角色图 → mention 节点出现 → 参考图列表自动加上该图。
6. **多图组合**：素材图 + 临时图 + 角色图共 3 张混用 → 后端收到 multipart 3 个 `image[]` 字段（用 dev tools / 后端日志验证）→ 生成成功或厂商返回明确错误。
7. **删除参考图**：点击参考图缩略图 X → 仅本表单移除 → 素材库与角色绑定不变。
8. **设置未配置**：清空 image_model_config.api_key → 生成按钮置灰 + 提示。

### 自动化测试

- 不强制新增单测；若 `prompt-editor-utils.js` / `mention-utils.js` 既有测试覆盖被本次改动影响，需同步更新。

### Open Questions / Verification Needs

- **Q1 ❓ geekai.co 协议兼容性**：用户的 base URL 是否支持 `images/edits` multipart？需要用真实账号一次性手测。决策点：若不支持则升级讨论（见 DON'T 升级点）。
- **Q2 ❓ 参考图大小**：OpenAI 协议要求每张图 ≤ 25MB；目前素材库内部图未做限制。规格暂不加前置校验，由后端透传错误。
- **Q3 ❓ 角色 @ 的语义**：在生图 prompt 里 @ 一个"角色"（无图） → 当前规格选择"仅作为文本 token，不充当参考图"。是否合理？

## Governance Card

### DO

- 允许的编辑路径：
  - `src/main.jsx`（`ImageGenView`、App 顶层 `addTempImage` / paste 函数）
  - `src/imagegen-utils.js`（新增）
  - `src-tauri/src/lib.rs`（`generate_image_command` 重写）
  - `src-tauri/Cargo.toml`（`reqwest` features）
  - `src/styles.css`（参考图缩略图、生图 prompt 区域微调）
- 必须同步的文档：本 spec 的 `## Decision Log` 段落（实现期间发现的协议偏差、行为决策）。

### DON'T

- 禁止编辑：`PromptMentionEditor.jsx`、`MaterialMentionPicker.jsx`、`mention-utils.js`、`prompt-editor-utils.js`（除非发现 bug 必须修，且需在 Decision Log 记录）。
- 禁止编辑：CreateTaskView 内部业务逻辑（仅可适配 App 顶层函数签名变化）。
- 升级决策点：协议不兼容时停下来，不私自降级方案。

### VERIFY

- 静态：见 `## VERIFY → 静态` 全部 5 项。
- 焦点测试：见 `## VERIFY → 集中测试（手动）` 1 / 2 / 3 / 5 / 6 必过。
- 手动：剩余 4 / 7 / 8 项由用户在交付时验收。

## Decision Log

### 2026-05-06 — 初稿

- 决定：用 `PromptMentionEditor` 复用而非另起；参考图通过 `image_asset_ids` 串到后端。
- 原因：避免双轨；asset id 已经是稳定 IPC 句柄。
- 影响：App 顶层需要重构 paste / addTempImage 为返回 asset 的纯 IO 函数。
