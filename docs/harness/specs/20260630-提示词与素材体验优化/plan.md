# 提示词与素材体验优化

## 状态

- 状态：confirmed（P0 全部已确认；本轮交付文档+原型，代码执行另起一轮）
- 版本归属：无明确版本
- 最近确认人：liuyun（用户）
- 最近确认时间：2026-06-30

## 目标

「软件 → 创建任务」的提示词编辑与素材引用环节体验不佳，集中在 5 点：

1. 提示词框能粘贴图片，但**不能粘贴音频**。
2. 素材管理器弹窗**没有把"最近使用"排在前面**。
3. @引用的素材**再次编辑就不高亮**了。
4. 高亮素材**点击无法预览**。
5. 编辑器里的素材**展示顺序、和右侧图片预览的顺序很难对应感知**，**名字也对不上**。

目标：让 @引用稳定不掉、点得动能预览；编辑器与右侧预览在顺序、名字上一致可感知；素材管理器把真正"最近使用"的素材置顶；提示词框支持粘贴音频。成功后用户在创建任务时，"我引用了哪些素材、它们排第几、各自是什么"一眼可感知且操作稳定。

## 用户与场景

- 目标用户：用 Dreamina 调度器桌面应用创建视频任务的用户（liuyun）。
- 主要场景：在「创建任务」里写提示词、用 @ 引用角色/图片/音频素材、粘贴参考图、再回看修改任务。
- 关键用户路径：打开创建任务 → 写提示词并 @引用素材 / 粘贴图片或音频 → 在素材管理器里挑素材 → 右侧确认绑定的素材与顺序 → 保存/再次编辑该任务。

## 范围

> 优先级（执行方自排，用户已授权）：**C 引用稳定+可预览 > D 顺序/名字对齐 > B 最近使用置顶 > A 粘贴音频**。

### 做

1. **[C] 提示词改为富文本结构持久化（病根修复）**：任务新增可选字段 `prompt_doc`（TipTap JSON）作为前端 source of truth；@mention 的 id/type/assetId 随结构持久化，不再每次按 label 字符串猜重建。再次编辑/切回任务时高亮稳定保留。
2. **[C] @高亮素材可预览**：mention 节点点击 → 图片走 `openImagePreview`，音频走试听，复用既有预览能力；保证 `data-id`/`assetId` 在重建后不丢，使点击有目标。
3. **[D] 顺序对齐**：以富文本 doc 中 mention 出现顺序为唯一真相，右侧预览按同一顺序渲染；编辑器删除某个 @mention 时同步移除对应 asset id，杜绝错位。
4. **[D] 名字对齐**：统一显示口径——主标题用"显示名"(label 口径，带角色前缀)，`asset.name` 作为副标题/hover；编辑器与右侧两处显示同一主标题。
5. **[B] 素材管理器"最近使用"置顶**：新增 `last_used_at` 打点（素材被任务引用/入队时更新）；弹窗默认按 `last_used_at` 倒序，并在「全部」视图顶部固定"最近使用"分组；保留现有分类 tab。
6. **[A] 提示词框支持粘贴/拖拽音频**：`handlePaste`/`drop` 识别 `audio/*`，落为临时音频素材并绑定到 `audio_asset_ids`，在右侧"音频"区显示且可试听，可被 @mention 引用。
7. **[文档]** 产出 `tech-design.md`（技术方案）与 `prototype.html`（可视化原型）。

### 不做

- 不改 MCP 工具 `dreamina_queue_video` / `dreamina_queue_videos` 的入参与语义（富文本只在前端内部，向下游恒等导出 `prompt` 纯文本 + `image_paths`/`audio_paths`）。
- 不支持粘贴视频（下游即梦不接受 video 作为输入素材）。
- 本轮不落代码、不做执行层任务拆分（gg-harness 不负责执行）。
- 不重构素材库底层存储模型，仅增量加字段（`prompt_doc`、`last_used_at`）。
- 不改调度器/队列执行逻辑（与 `20260624-调度器空闲性能优化` 不交叉）。

## 已确认事实

- 提示词改为「富文本 doc(JSON) 为真相 + 派生纯文本 prompt + 素材引用列表」双表示。
- MCP 零改动；富文本仅前端 UI 内部表示。
- 本轮只产出文档与原型；输入媒体扩展仅限音频。

## 推荐假设

- `last_used_at` 在"任务保存/入队引用到该素材"时打点；展示按它倒序，缺失则回落 `created_at`。
- 名字统一以 label 口径为主标题、`asset.name` 为副标题。
- 音频粘贴落为 `temp`/临时音频素材，生命周期类比现有 `temp_image`。
- `prompt_doc` 为可选字段；缺失时 fallback 现有 `parseInlineContent`，不退化于当前行为。
- 原型沿用现有应用配色（角色绿 `#e8fbf3`/`#087d58`、音频蓝 `#eaf1ff`/`#1559d6`、临时图黄 `#fff4de`/`#d47a00`）。

## 待确认问题

| 优先级 | 问题 | 推荐答案 | 不确认的影响 | 状态 |
| --- | --- | --- | --- | --- |
| P1 | `last_used_at` 打点时机：保存任务时 vs 真正入队执行时 | 保存任务时即打点（更贴合"我最近在用"） | 选执行时则草稿引用不浮顶 | open（执行轮再定，含推荐答案） |
| P1 | 老任务迁移：是否一次性回填 `prompt_doc` | 否，惰性迁移——打开老任务时用 fallback 重建并在保存时写回 | 一次性回填风险大、收益低 | open（推荐惰性） |
| P2 | 名字副标题在右侧是常显还是 hover | 空间足常显，紧凑则 hover | 仅影响信息密度 | 记录假设 |

## 现有系统事实

- 技术栈：React 18 + Tauri 2 + TipTap 3.22.5（`@tiptap/extension-mention` + suggestion）；原生 CSS。
- 编辑器：`src/components/PromptMentionEditor.jsx`。`handlePaste`（`:217-263`）只识别 `item.type.startsWith('image/')`，**无音频分支**。
- 高亮病根：编辑器只存纯文本 prompt；重开时 `prompt-editor-utils.js:promptTextToTiptapDoc` → `parseInlineContent`（`:162-194`）用正则 `@([^\s@]+)` 按 **label 字符串**反查 `mentionItems`，匹配失败即降级为纯文本 → 掉高亮、丢 `data-id`、点不动。
- 素材管理器：`src/components/MaterialMentionPicker.jsx` + `material-mention-picker-utils.js`。已有 `recent` 分类，但 `isRecent` 仅基于 `created_at` 的 3 天窗口（`mention-utils.js:60-87`），非真正"使用过"，且为并列 tab 而非置顶。
- 名字两源：编辑器 mention 显示 `node.attrs.label`；右侧 Thumb 显示 `asset.name`（`main.jsx:~1487`）。顺序两源：右侧按 `image_asset_ids` 数组渲染（`getTaskHitResources` `queue-view-utils.js:160-183`），编辑器按文本中 @ 出现顺序。
- 引用同步：`extractMentionRefsFromTiptapDoc`/`applyMentionRefsToTaskForm`（`prompt-editor-utils.js`）已能从 doc 收集 refs，具备改为"以 doc 为真相"的基础。
- MCP 入参：`dreamina_queue_video` required 仅 `prompt`(string)，素材为 `image_paths`/`audio_paths`(本地路径数组)，无富文本概念。

## 约束与风险

- 兼容性：新增字段必须可选，老任务与 MCP 任务无 `prompt_doc` 时 fallback 不退化。
- 数据一致性：doc(真相) 与派生的 `prompt`/`*_asset_ids` 必须单向同步，避免两份各写。
- 体验回归：富文本不得破坏现有粘贴图片、@suggestion、文本长度限制等已工作能力。
- 存储体积：`prompt_doc` 增加 `state.json` 体积；与 `20260624` 的"压实 state.json"目标存在张力，需控制（只存必要 attrs）。
- 预览路径：点击预览依赖 `stored_path`/`storedPath` 有效，需在重建后保留 assetId 以便回查 asset。

## 稳定规范引用

- 暂无 `_stable/` 强相关项。本需求触及 UI，但项目尚无 `DESIGN.md`；交付时建议补建（见执行交接）。

## 持久子需求判断

- 是否需要 `sub-*.md`：否。四个优化点同属一个用户路径、共享富文本真相这一底层改动，无独立版本/owner/外部契约，运行时拆分即可。

## 验收标准引用

- `acceptance.md`
- 技术方案：`tech-design.md`

## 执行交接

- 执行层由用户运行时指定；本需求文档不绑定具体执行技能。
- 读取顺序：`plan.md` → `tech-design.md` → `acceptance.md` → `grill.md`。
- 执行前必须读 `acceptance.md`；完成声明必须回填 `acceptance.md#执行验收记录` 或同目录 `result.md`。
- 执行层不得擅自改写需求事实；发现事实变化先记录并等用户确认。
- 建议（非本轮强制）：本项目触及 UI 但无 `DESIGN.md`/`AGENTS.md`，可在执行轮按 gg-harness 0.4.0 补建 root `DESIGN.md` 并索引。

## 变更记录

| 日期 | 变更 | 来源 |
| --- | --- | --- |
| 2026-06-30 | 初稿，P0 全部确认，状态 confirmed | brainstorming + 代码探索 + 用户确认 |
