# 技术方案：CLI 兼容队列代理

> 配套 `plan.md`。本文件只写「怎么实现」，需求事实以 `plan.md` 为准。

## 0. 一句话方案

新增一个 Python CLI 包 `dreamina_queue`，命令名 `dreaminaq`。它对外尽量模拟即梦 CLI 的视频生成调用方式，对内把任务写入 SQLite 队列和内容寻址缓存，再由 `worker/once` 调用原 dreamina CLI 完成提交、查询、重试、Fast 兜底和结果落盘。

```text
agent / skill / shell
  -> dreaminaq submit/video ...        # 参数尽量像 dreamina cli
  -> ~/.dreaminaq/db.sqlite + blobs/
  -> dreaminaq worker / once
  -> subprocess: dreamina cli
  -> results/ + status/result --json
```

## 1. 包与目录结构

建议新增独立 Python 包目录：

```text
python/dreamina_queue/
  pyproject.toml
  src/dreamina_queue/
    __init__.py
    cli.py
    config.py
    db.py
    models.py
    assets.py
    request_hash.py
    dreamina_cli.py
    keep_awake.py
    scheduler.py
    worker.py
    result_parser.py
    status.py
    skill/
      SKILL.md
  tests/
```

默认数据目录：

```text
~/.dreaminaq/
  config.toml
  db.sqlite
  blobs/
    ab/abcdef...png
  results/
    task_id/
      result.mp4
      metadata.json
  tmp/
  logs/
```

## 2. CLI 命令设计

### 2.1 提交

推荐主命令：

```bash
dreaminaq submit \
  --prompt "..." \
  --image /path/a.png \
  --image /path/b.png \
  --audio /path/a.mp3 \
  --model_version seedance2.0 \
  --ratio 16:9 \
  --duration 15 \
  --idempotency-key scene-001 \
  --json
```

兼容别名：

```bash
dreaminaq video ...
dreaminaq queue ...
```

参数策略：

- 常用视频参数映射为结构化字段：prompt、image、audio、model_version、ratio、duration、resolution。
- 未识别但安全的参数可以记录到 `raw_args` 并透传给原 dreamina CLI。
- 队列专属参数不透传：`--idempotency-key`、`--reuse`、`--force`、`--start-at`、`--queue-model-policy`、`--json`。

### 2.2 推进

```bash
dreaminaq once --json
dreaminaq worker --interval 30 --keep-awake --json-lines
```

- `once` 推进一轮：优先查询到期 attempt，再提交到期 task。
- `worker` 循环调用 `once`；默认建议在存在未完成任务时启用 `--keep-awake`，避免夜间系统睡眠导致错过高效排队窗口。
- 同一 store 同时只允许一个推进者持有锁。

### 2.3 查询状态与结果

```bash
dreaminaq status --json
dreaminaq status --watch
dreaminaq task TASK_ID --json
dreaminaq result TASK_ID --json
dreaminaq cancel TASK_ID --json
```

输出必须机器可读，默认字段稳定：

```json
{
  "task_id": "...",
  "status": "queued",
  "next_run_at": "...",
  "attempts": [],
  "results": []
}
```

## 3. SQLite 数据模型

### 3.1 assets

```sql
create table assets (
  id text primary key,
  sha256 text not null,
  kind text not null,
  mime text,
  original_path text,
  cached_path text not null,
  size integer,
  created_at text not null,
  last_used_at text
);
create unique index idx_assets_sha256_kind on assets(sha256, kind);
```

### 3.2 tasks

```sql
create table tasks (
  id text primary key,
  idempotency_key text,
  request_hash text not null,
  prompt text not null,
  model_version text,
  ratio text,
  duration integer,
  resolution text,
  status text not null,
  priority integer not null default 1,
  created_at text not null,
  queued_at text,
  scheduled_at text,
  next_run_at text,
  raw_args_json text,
  error text,
  result_policy text not null default 'new'
);
create unique index idx_tasks_idempotency on tasks(idempotency_key) where idempotency_key is not null;
create index idx_tasks_due on tasks(status, next_run_at, queued_at);
create index idx_tasks_request_hash on tasks(request_hash);
```

### 3.3 task_assets

```sql
create table task_assets (
  task_id text not null,
  asset_id text not null,
  role text not null,
  position integer not null,
  primary key(task_id, asset_id, role, position)
);
```

### 3.4 attempts

```sql
create table attempts (
  id text primary key,
  task_id text not null,
  lane text not null,
  status text not null,
  submit_id text,
  started_at text,
  finished_at text,
  next_query_at text,
  queue_idx integer,
  queue_length integer,
  queue_status text,
  stdout text,
  stderr text,
  error text,
  raw_command_json text
);
create index idx_attempts_task on attempts(task_id, started_at);
create index idx_attempts_due_query on attempts(status, next_query_at);
```

### 3.5 results

```sql
create table results (
  id text primary key,
  task_id text not null,
  attempt_id text,
  path text,
  url text,
  sha256 text,
  created_at text not null
);
create index idx_results_task on results(task_id, created_at);
```

### 3.6 events

```sql
create table events (
  id text primary key,
  task_id text,
  attempt_id text,
  level text not null,
  type text not null,
  message text not null,
  detail text,
  created_at text not null
);
```

## 4. 缓存策略

### 4.1 素材缓存

- 对每个输入图片/音频读取内容 sha256。
- 复制到 `blobs/<sha256前2位>/<sha256>.<ext>`。
- 同 hash + kind 已存在时复用 cached_path，只更新 `last_used_at`。
- 任务只引用 asset id，不依赖原始路径。

### 4.2 任务幂等

`request_hash` 由以下字段计算：

```text
prompt + model_version + ratio + duration + resolution + ordered asset sha256 list
```

行为：

- 有 `--idempotency-key` 且已存在：返回已有任务，不新建。
- 无 `--idempotency-key`：即使 request_hash 相同，也默认新建任务，允许多候选。
- `--reuse exact`：如果 request_hash 已有成功结果，直接返回缓存结果。
- `--force`：忽略 idempotency/reuse，强制新任务。

### 4.3 结果缓存

- 成功后将结果下载/复制到 `results/<task_id>/`。
- 保存 `metadata.json`，包括 prompt、模型、asset hashes、attempt id、submit id、原始 URL/路径。
- `dreaminaq result TASK_ID --json` 只读本地结果记录。

## 5. 调度状态机

任务状态：

```text
draft/queued/scheduled/submitting/submitted/querying/retry_wait/succeeded/failed/cancelled
```

attempt 状态：

```text
submitting/submitted/querying/retry_wait/succeeded/failed
```

`once` 推荐顺序：

1. 获取 SQLite 推进锁。
2. 查找 due query attempt：`status in submitted/querying` 且 `next_query_at <= now`。
3. 若存在，调用原 dreamina CLI 查询。
4. 否则查找 due retry/task：`queued/scheduled/retry_wait` 且到期。
5. 判断 standard/Fast lane 是否可提交。
6. 调用原 dreamina CLI submit。
7. 解析 submit_id / queue_info / result。
8. 写 attempt、events、task status、next_run_at。
9. 释放锁。

排序原则：

- 同优先级下按 `queued_at` / `created_at` 升序，先进入队列先提交。
- Fast 与 standard 逻辑独立，但由同一个 worker 串行持锁推进，避免本地并发写坏状态。
- 如果标准模型排队超过 2 小时仍未开始，且无活跃 Fast attempt，则允许创建 Fast 兜底 attempt。

## 6. 原 dreamina CLI 适配层

`dreamina_cli.py` 负责：

- 查找 CLI 路径。
- 构造命令。
- subprocess 执行。
- 设置超时。
- 捕获 stdout/stderr/exit code。
- 解析输出。
- 保存原始命令和原始输出。

解析策略：

- 优先解析 JSON 输出。
- 若非 JSON，则用保守正则提取 submit_id、queue_idx、queue_length、result path/url。
- 解析失败不丢原始输出，attempt 标记为 failed 或 retry_wait，并写 events。

注意：

- Python 不实现上传/鉴权/接口请求。
- 所有真实提交和查询都委托给原 dreamina CLI。

## 7. 并发与锁

推荐使用 SQLite 事务锁：

```sql
create table locks (
  name text primary key,
  owner text not null,
  expires_at text not null
);
```

获取锁流程：

- `begin immediate`
- 如果锁不存在或已过期，写入当前 owner。
- 如果锁未过期，退出本轮并返回 `locked`。
- 每轮推进结束删除锁或刷新 expires。

也可用 `portalocker` 文件锁作为补充，但 SQLite 事务应是主机制。

## 8. 长时间运行与防休眠

夜间可持续调度是第一版 P0 能力。worker 不能只“能循环”，还要在存在未完成任务时阻止系统自动睡眠，或者明确持有等价保活机制。

### 8.1 行为规则

- `dreaminaq worker` 支持 `--keep-awake / --no-keep-awake`。
- 默认策略建议为 `auto`：队列存在未完成任务时启用防休眠；队列清空后释放防休眠。
- worker 每轮写 heartbeat event，包含 `keep_awake=active/inactive/unsupported`。
- worker 收到 SIGINT/SIGTERM 时必须释放防休眠句柄并正常退出。
- 如果平台不支持防休眠，worker 必须在启动日志和 `status --json` 中明确标记 `keep_awake_supported=false`，不能静默假装已启用。

### 8.2 平台适配

`keep_awake.py` 提供统一接口：

```python
class KeepAwakeHandle:
    def start(self) -> None: ...
    def stop(self) -> None: ...
    def status(self) -> dict: ...
```

首版推荐：

- macOS：使用 `/usr/bin/caffeinate` 子进程，参数类似 `caffeinate -dimsu`；worker 生命周期内持有，队列清空或退出时 terminate。
- Windows：预留 `SetThreadExecutionState` 适配器；如果首版不实现，必须在文档里标注 unsupported。
- Linux：预留 `systemd-inhibit` / desktop portal 适配器；如果首版不实现，必须在文档里标注 unsupported。

### 8.3 状态展示

`dreaminaq status --json` 增加：

```json
{
  "worker": {
    "heartbeat_at": "...",
    "keep_awake": "active",
    "keep_awake_backend": "caffeinate",
    "keep_awake_supported": true
  }
}
```

### 8.4 验证方式

- 单元测试 mock `KeepAwakeHandle`：队列有任务时 start，队列空时 stop，异常退出时 stop。
- macOS 冒烟：启动 `dreaminaq worker --keep-awake` 后能看到 `caffeinate` 子进程，退出后子进程消失。
- 长跑模拟：fake dreamina CLI + worker 至少运行多轮，期间 heartbeat 持续更新且不重复提交。

## 9. 配置

`~/.dreaminaq/config.toml`：

```toml
dreamina_cli = "dreamina"
default_model_version = "seedance2.0"
fast_model_version = "seedance2.0fast"
worker_interval_seconds = 30
keep_awake = "auto" # auto / always / never
standard_fast_fallback_hours = 2
max_retry_attempts = 8
retry_delay_seconds = 30
result_dir = "~/.dreaminaq/results"
```

命令行参数优先级高于环境变量，环境变量高于 config。

## 10. Skill 包装方式

新增或后续迁移一个 skill，例如：

```text
dreaminaq/
  SKILL.md
```

Skill 内容只写：

- 什么时候用 `dreaminaq`。
- 如何提交单个/批量视频。
- 如何传 idempotency key。
- 如何启动 worker 或调用 once。
- 夜间批量任务必须启动 `dreaminaq worker --keep-awake`，并用 `status --json` 确认 keep_awake 状态。
- 如何读取 status/result JSON。
- 不要直接调用原 dreamina CLI，除非用户明确绕过队列。

Skill 不保存进度、不轮询、不维护运行状态。

## 11. 轻量展示

第一版推荐只做：

- `dreaminaq status --json`
- `dreaminaq status --watch`
- `dreaminaq events --tail`

后续如确实需要，再做：

- Textual/Rich TUI。
- 本地只读 Web 状态页。
- 现有 Tauri App 读取 `~/.dreaminaq/db.sqlite` 作为轻量 monitor。

## 12. 与现有项目的关系

短期：

- Python 代理作为新目录/新包并行存在。
- 不要求改动现有 Tauri App 主流程。
- 可以复用现有 Rust 实现中的策略经验和测试案例。

中期：

- MCP 可以变薄：直接调用 `dreaminaq` 或复用同一 Python core。
- App 可以降级为只读监控/少量人工动作。

长期：

- 若 Python 路线验证稳定，现有 App 的复杂任务中心、角色库、生图、完整日志中心可以退役或迁出。

## 13. 测试策略

### 单元测试

- request_hash 稳定性。
- asset sha256 去重。
- idempotency key 重复提交。
- `--reuse exact` 命中与未命中。
- 状态机迁移。
- Fast 兜底策略。
- keep_awake 生命周期。
- CLI 输出解析。

### 集成测试

- 使用 fake dreamina CLI 模拟 submit/query/result。
- worker 并发启动两个进程，只有一个能推进。
- worker `--keep-awake` 启动后，mock 或平台后端被正确持有和释放。
- SQLite 数据损坏/锁过期恢复。

### 冒烟测试

- 真实 dreamina CLI 单任务提交。
- status/result JSON 可被 agent 解析。
- worker 跑 2-3 轮后无重复提交。
- macOS 上 `worker --keep-awake` 能持有 `caffeinate`，退出后释放。

## 14. 风险与降级

- 如果官方 dreamina CLI 输出不可稳定解析，先要求 CLI 以 JSON 模式运行；没有 JSON 时只保留原始输出并标记需要人工处理。
- 如果跨平台 worker 服务安装复杂，先提供普通 `worker` 命令和文档。
- 如果非 macOS 平台防休眠首版来不及完整实现，必须清晰标记 unsupported，并保留适配接口；macOS 不可降级，因为用户当前夜间调度窗口依赖它。
- 如果结果下载路径不稳定，先记录 URL/路径，下载动作作为独立 `result --fetch`。
- 如果旧 App 数据兼容成本高，明确第一版不兼容旧数据，只支持新队列。
