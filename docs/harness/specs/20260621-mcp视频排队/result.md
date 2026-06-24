# MCP 视频排队结果记录

## 结果

- 新增 `dreamina-mcp` stdio MCP 二进制，提供 `dreamina_queue_video`、`dreamina_queue_videos`、`dreamina_process_queue_once`、`dreamina_get_queue_snapshot` 四个工具。
- MCP 入队接受本地图片/音频路径，导入到 `role-media/mcp/` 后创建现有调度任务，不要求使用 App 内素材。
- 简化视频参数：`portrait -> 9:16`、`landscape -> 16:9`、`fast -> seedance2.0fast`、`standard -> seedance2.0`，默认 `portrait / fast / 15s / 720p`。
- 批量入队支持共享 `start_at`，到点后复用现有队列逻辑连续推进。
- 队列推进抽出公共同步入口，并用 `queue.lock` 做轻量跨进程互斥，避免 App 后台和 MCP 同时推进同一轮队列。

## 验证

- `cargo test --manifest-path src-tauri/Cargo.toml`
- `cargo build --manifest-path src-tauri/Cargo.toml --bin dreamina-mcp`
- `cargo build --manifest-path src-tauri/Cargo.toml --release --bin dreamina-mcp`
- `cargo fmt --manifest-path src-tauri/Cargo.toml --check`
- `npm test`
- `npm run build`
- `npm run tauri:build`
- MCP stdio 冒烟：`initialize`、`tools/list`、临时 `DREAMINA_SCHEDULER_HOME` 下调用 `dreamina_queue_video` 均通过。

## 产物

- App：`src-tauri/target/release/bundle/macos/即梦调度器.app`
- DMG：`src-tauri/target/release/bundle/dmg/即梦调度器_0.2.3_aarch64.dmg`
- MCP：`src-tauri/target/release/dreamina-mcp`

## 未做

- 没有重写上传、重试、公平调度策略。
- 没有支持 App 内素材 ID 作为 MCP 输入。
- 没有引入额外 MCP SDK 依赖。
