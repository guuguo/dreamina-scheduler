# Dreamina Scheduler

独立 Tauri 桌面 App，用于提前整理角色库、角色图片、角色音频和提示词，并以 `dreamina multimodal2video` 创建即梦多模态视频任务。

## MVP 范围

- 只支持 `multimodal2video`。
- 不提供独立素材库；图片和音频都挂在角色下。
- 角色支持参考图和音色音频；视频素材暂不支持。
- 默认模型为 `seedance2.0`，可切换 `seedance2.0fast`。
- 宽高比只能从 `1:1 / 3:4 / 16:9 / 4:3 / 9:16 / 21:9` 中选择。
- 支持角色库、手动选择角色、提示词 `@角色` 引用、确定性自动匹配角色。
- 角色库资源统一复制到 App 内缓存目录 `role-media/`；支持文件选择器和拖拽导入角色图片/音频。
- 提交时只使用 `--image` 和 `--audio`，不会传 `--video`。
- 并发受限错误会识别 `ExceedConcurrencyLimit / ret=1310 / ConcurrencyLimit / 并发上限 / 并发限制`。
- 支持“运行一次”和“启动自动队列”，队列并发固定为 1。
- 支持提交后按 `submit_id` 查询结果，并在任务详情展示 attempt、错误摘要、结果路径或 URL。
- 支持在设置页配置并发限制的静默重试/静默失败策略、重试间隔和最大重试次数。
- 支持在设置页触发 CLI 安装和 CLI 登录；macOS 默认参考 `dreamina-cli-skill` 使用官方安装脚本，Windows 需要填入确认过的 PowerShell 安装命令。

## 本地启动

```bash
npm run dreamina-scheduler:dev
```

## 构建

```bash
npm run dreamina-scheduler:build
```

macOS 构建产物在：

```text
apps/dreamina-scheduler/src-tauri/target/release/bundle/
```

## 当前限制

- 自动队列只在 App 运行期间生效；App 退出、电脑休眠或关机后不会后台执行。
- 查询结果已支持基础路径/URL 解析；下载目录、结果打开和更完整的查询超时策略还需要继续补。
- Windows CLI 一键安装命令尚未确认官方默认源，未配置时会阻断执行，避免误装未知包。
- CLI 安装和登录当前等待命令结束后返回摘要，尚未做实时日志流或二维码专门展示。
- 角色创建表单仍保留路径输入作为兜底；常规使用建议通过“选择”按钮或拖拽导入。
