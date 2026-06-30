# 技术方案：提示词与素材体验优化

> 配套 `plan.md`。本文件只写"怎么实现"，需求事实以 `plan.md` 为准。

## 0. 一句话方案

把提示词从「只存纯文本」升级为「**富文本 doc(JSON) 为唯一真相**」，前端所有展示（高亮、顺序、名字、预览）都从这份 doc 派生；对**所有下游**（MCP、调度执行、`state.json` 兼容）则从同一份 doc **恒等导出** `prompt` 纯文本 + `image_paths`/`audio_paths`。MCP 因此一行不改。

## 1. 核心数据模型：单一真相 + 派生

### 1.1 现状（病根）

```
存储: task.prompt (纯文本)  ← 唯一持久化
重开: prompt --正则@label反查mentionItems--> 重建 mention 节点
      匹配失败 → 降级纯文本 → 掉高亮 / 丢 data-id / 点不动
```
纯文本是有损表示，丢掉了 id/type/assetId，每次靠"按名字猜"恢复，脆弱。

### 1.2 目标模型

```
                    ┌─────────────────────────┐
   编辑器读写 ◄────► │  task.prompt_doc (真相)  │  TipTap JSON, 含 mention.attrs:
                    │  source of truth         │  {id, label, type, roleId, assetId}
                    └───────────┬─────────────┘
                                │ 单向派生 (doc → 下游)
              ┌─────────────────┼──────────────────┐
              ▼                 ▼                  ▼
       task.prompt        image_asset_ids /    顺序 / 名字 / 高亮
       (纯文本, 给MCP)     audio_asset_ids      (右侧预览 & 编辑器, 同源)
              │                 │
              ▼                 ▼
         MCP prompt        resolve→ image_paths / audio_paths (给 MCP)
```

- **写**：只写 `prompt_doc`。每次编辑器变更，用现有 `extractMentionRefsFromTiptapDoc(doc)` 派生 refs，用 doc 的 textContent 派生 `prompt`。`prompt` 与 `*_asset_ids` 都是**派生缓存**，永不被单独编辑。
- **读（前端）**：编辑器直接 `setContent(prompt_doc)`，不再走 `parseInlineContent` 猜测路径。
- **读（下游/MCP）**：永远只用 `prompt` + 解析后的 paths，与今天完全一致。

### 1.3 字段与兼容

| 字段 | 位置 | 说明 |
| --- | --- | --- |
| `prompt_doc` | task（新增，可选） | TipTap JSON。前端真相。 |
| `prompt` | task（保留） | 纯文本，doc 派生。MCP/展示/搜索用。 |
| `image_asset_ids` / `audio_asset_ids` / `temp_image_asset_ids` | task（保留） | doc 派生，**顺序 = doc 中 mention 出现顺序**。 |

**兼容矩阵：**

| 任务来源 | 有 `prompt_doc`? | 行为 |
| --- | --- | --- |
| 新建/编辑后保存 | 有 | 直接 setContent，稳定高亮 |
| 老任务（仅纯文本） | 无 | fallback `parseInlineContent` 重建（= 今天行为，不退化）；用户保存时写回 `prompt_doc`（惰性迁移） |
| MCP 入队任务 | 无 | 本就无 @mention，纯文本 + 右侧绑定缩略图；无掉高亮问题 |

→ **MCP 习惯 100% 保留**：MCP 既不产出也不消费 `prompt_doc`。

## 2. [C] @引用稳定 + 可预览

### 2.1 稳定高亮

根因消除：不再 `promptTextToTiptapDoc(prompt, mentionItems)` 按 label 重建，而是 `editor.setContent(task.prompt_doc)`。mention 的 `id/type/assetId` 是结构的一部分，编辑、切任务、保存往返都不丢。

`PromptMentionEditor.jsx` 初始化（`:279-292`）改为：
```
if (value.prompt_doc) editor.commands.setContent(value.prompt_doc)
else editor.commands.setContent(promptTextToTiptapDoc(value.prompt, mentionItems)) // fallback
```
`onUpdate` 回调同时上报 `{ doc: editor.getJSON(), plainText, refs }`，由 `handleEditorUpdate` 写 `prompt_doc` + 派生字段。

### 2.2 点击可预览

mention 节点已渲染 `data-id`/`data-asset-id`（`renderHTML`）。加点击处理（TipTap NodeView 或编辑器容器事件委托）：
```
onMentionClick(attrs):
  asset = assetById.get(attrs.assetId)
  if type ∈ {image, temp_image}: openImagePreview(asset.stored_path, displayName(asset))
  if type == audio:             toggleAudio(asset)   // 复用 MaterialMentionPicker 的试听
  if type == role:              定位/高亮该角色（或无操作）
```
关键前置：2.1 保证了 `assetId` 不丢，点击才有目标。预览复用既有 `openImagePreview` 与音频 `toggleAudio`，不新造预览器。

## 3. [D] 顺序对齐 + 名字对齐

### 3.1 顺序：doc 为唯一真相

右侧预览不再独立按 `image_asset_ids` 数组顺序，而是按**派生时保持的 doc 顺序**渲染。因为 `extractMentionRefsFromTiptapDoc` 是按 doc 深度遍历收集的，天然就是出现顺序——只要右侧 `getTaskHitResources` 改为消费这份有序 refs（而非可能被其它路径改动的数组），编辑器删 mention 时 refs 自动少一项，右侧同步消失，错位消除。

右侧分组（角色图/临时图/音频）内部各自保持 doc 顺序；分组之间的相对次序固定（图在前、音频在后），并在每个缩略图上标"在提示词中第 N 处引用"的角标，让"展示逻辑"可感知。

### 3.2 名字：统一显示名

定义单一 `displayName(asset|item)`：
```
displayName = item.label           // 主标题, 带角色前缀的友好名, 编辑器与右侧一致
subName     = asset.name           // 副标题/hover, 库原始名
```
编辑器 mention 文本、右侧 Thumb 主标题都用 `displayName`；右侧补一行/hover 显示 `subName`。两侧主标题同源即对齐。

## 4. [B] 素材管理器"最近使用"置顶

### 4.1 真正的 last_used

新增素材级 `last_used_at`。打点时机（推荐）：**任务保存时**，对该任务引用到的每个 asset 写 `last_used_at = now`。
```
onSaveTask(task):
  for assetId in (image_asset_ids ∪ audio_asset_ids ∪ temp_image_asset_ids):
      asset.last_used_at = now
```

### 4.2 排序与置顶

`material-mention-picker-utils.js`：
- 默认排序 key：`last_used_at`（缺失回落 `created_at`）倒序。
- 「全部」视图顶部固定一个 **"最近使用"分组**（取 last_used_at 最近的 N 个，如 8），其下再按现有分类。
- 保留现有 `recent` tab，但其判定从"created 3天内"改为"有 last_used_at 即算用过"。

## 5. [A] 粘贴 / 拖拽音频

`PromptMentionEditor.jsx` `handlePaste`（`:217`）+ 新增 `handleDrop`：
```
audioItem = items.find(i => i.kind==='file' && i.type.startsWith('audio/'))
if audioItem && onPasteAudio:
    asset = await onPasteAudio(file)        // 落临时音频素材, 类比 onPasteImage
    insertTempAudioMention(asset)           // 插入 audio 类型 mention
    // 派生 → audio_asset_ids, 右侧"音频"区显示 + 可试听
```
- 复用图片粘贴的整条链路（`onPasteImage` → asset → mention → 派生），只是 mime 分支与 `type:'audio'`。
- 下游导出：音频 asset → `stored_path` → MCP `audio_paths`，与现有音频素材完全同路。
- 边界：非 image/audio 的粘贴维持原文本/忽略行为，不报错。

## 6. 与既有目标的张力处理

- **state.json 体积**（与 `20260624` 压实目标冲突）：`prompt_doc` 只存渲染必需的 mention attrs（id/label/type/roleId/assetId）+ 段落文本，不存样式/历史；纯文本 `prompt` 仍在，搜索/列表不读 doc。体积增量可控。
- **单向同步纪律**：`prompt`/`*_asset_ids` 标注为派生，任何写入路径只能源自 doc 派生函数，禁止旁路直接 set，避免双写不一致。

## 7. 改动面清单（供执行轮，本轮不落码）

| 模块 | 文件 | 改动 |
| --- | --- | --- |
| 编辑器 | `PromptMentionEditor.jsx` | setContent(doc) 优先；mention 点击预览；音频粘贴/拖拽分支；onUpdate 上报 doc |
| 派生 | `prompt-editor-utils.js` | 以 doc 为真相导出 prompt+有序 refs；fallback 保留 |
| 右侧预览 | `main.jsx` / `queue-view-utils.js` | 消费有序 refs；displayName/subName；引用序号角标 |
| 素材管理器 | `MaterialMentionPicker.jsx` / `material-mention-picker-utils.js` / `mention-utils.js` | last_used_at 排序 + "最近使用"置顶分组 |
| 存储 | task 模型（前端 + 必要时 Rust 持久层） | 新增可选 `prompt_doc`、asset `last_used_at` |
| MCP | — | **不改** |

## 8. 验证要点

见 `acceptance.md`。关键回归：富文本往返不掉高亮、MCP 入队任务表现不变、老任务 fallback 不退化、state.json 体积可接受。
