# 技术方案：即梦队列独立技能项目化

> 配套 `plan.md`。本文件只写实现建议，需求事实以 `plan.md` 为准。

## 0. 一句话方案

创建独立 GitHub 项目 `dreamina-queue`：它包含 Python CLI `dreaminaq`、本地 SQLite 队列、即梦 CLI 发现/安装/登录适配器、夜间 keep-awake worker、记录查询命令，以及一个给 agent 使用的 skill。

```text
Codex / Claude / shell / MCP caller
  -> skill: 使用 dreaminaq
  -> dreaminaq doctor/login/submit/worker/status/history/result
  -> ~/.dreaminaq/db.sqlite + blobs/ + results/
  -> official Dreamina CLI subprocess
```

## 1. 推荐仓库结构

```text
dreamina-queue/
  README.md
  LICENSE
  CHANGELOG.md
  pyproject.toml
  src/dreamina_queue/
    __init__.py
    cli.py
    config.py
    paths.py
    db.py
    schema.sql
    assets.py
    request_hash.py
    doctor.py
    installer.py
    auth.py
    dreamina_cli.py
    scheduler.py
    worker.py
    keep_awake.py
    parsers.py
    history.py
    output.py
  skills/dreamina-queue/
    SKILL.md
  examples/
    submit_one.sh
    submit_batch.py
    worker-night.sh
  tests/
    fake_dreamina_cli.py
    test_doctor.py
    test_installer.py
    test_auth.py
    test_queue.py
    test_worker.py
    test_history.py
  .github/workflows/ci.yml
```

## 2. 安装与启动路径

### 2.1 用户安装

第一版推荐支持：

```bash
pipx install git+https://github.com/<owner>/dreamina-queue.git
dreaminaq doctor --fix
```

也支持开发者安装：

```bash
git clone https://github.com/<owner>/dreamina-queue.git
cd dreamina-queue
uv sync
uv run dreaminaq doctor --fix
```

### 2.2 agent 使用

skill 中推荐 agent 固定流程：

```bash
dreaminaq doctor --json
dreaminaq submit ... --json
dreaminaq worker --interval 30 --keep-awake --json-lines
dreaminaq status --json
dreaminaq result TASK_ID --json
```

如果 `doctor` 返回 `blocking_reason=cli_missing` 或 `auth_required`，agent 不应盲目重试提交，而应触发安装或登录流程。

## 3. CLI 命令设计

### 3.1 环境诊断

```bash
dreaminaq doctor --json
dreaminaq doctor --fix
```

输出示例：

```json
{
  "ok": false,
  "dreamina_cli": {
    "found": false,
    "path": null,
    "version": null,
    "install_supported": true
  },
  "auth": {
    "logged_in": false,
    "method": "official_cli",
    "blocking_reason": "cli_missing"
  },
  "keep_awake": {
    "supported": true,
    "provider": "macos_caffeinate"
  },
  "next_action": "run: dreaminaq install-cli"
}
```

### 3.2 安装即梦 CLI

```bash
dreaminaq install-cli --json
dreaminaq install-cli --yes --json
```

实现策略：

- `installer.py` 只使用明确的官方安装源。
- macOS 先实装，Windows/Linux 先做 adapter skeleton。
- 安装前输出将执行的命令；非 `--yes` 时要求用户确认。
- 安装后重新运行 discovery。
- 如果官方安装需要浏览器或登录，返回 blocking JSON，不把半完成状态伪装成成功。

### 3.3 登录

```bash
dreaminaq login
dreaminaq login --json
```

实现策略：

- `auth.py` 通过官方 CLI 的用户信息命令或一次 harmless check 判断登录态。
- 未登录时调用官方 CLI 登录命令或打开官方登录入口。
- 登录过程中队列状态标记为 `blocked_auth_required`。
- 登录完成后 `doctor` 变为 ok，worker 自动恢复。
- 不采集、不保存密码、验证码、Cookie 明文。

### 3.4 提交

```bash
dreaminaq submit \
  --prompt "..." \
  --image /path/a.png \
  --model_version seedance2.0 \
  --ratio 16:9 \
  --duration 15 \
  --idempotency-key scene-001 \
  --queue-model-policy standard-then-fast \
  --json
```

兼容策略：

- 常用即梦 CLI 参数结构化保存。
- 未识别参数保存在 `raw_args`，必要时透传给官方 CLI。
- 队列层参数不透传。
- 输入素材复制到内容寻址缓存后再入队。

### 3.5 worker

```bash
dreaminaq once --json
dreaminaq worker --interval 30 --keep-awake --json-lines
```

行为：

- `once` 推进一轮到期任务和到期查询。
- `worker` 长循环，默认间隔 30 秒。
- 同一数据目录只允许一个 worker 持有推进锁。
- 有未完成任务且开启 `--keep-awake` 时，启动 keep-awake provider。
- 队列清空或进程退出时释放 keep-awake。

### 3.6 查询

```bash
dreaminaq status --json
dreaminaq status --watch
dreaminaq history --limit 50 --json
dreaminaq task TASK_ID --json
dreaminaq result TASK_ID --json
dreaminaq cancel TASK_ID --json
```

第一版查询重点：

- 当前队列数量。
- 当前 worker 是否运行。
- 每个任务状态。
- submit_id。
- 最近尝试和错误。
- next_run_at。
- 结果路径。

## 4. 自动发现即梦 CLI

`doctor.py` 检测顺序：

1. `DREAMINA_CLI_PATH` 环境变量。
2. `~/.dreaminaq/config.toml` 中的 `dreamina_cli_path`。
3. `PATH` 中的候选命令。
4. macOS 常见安装路径。
5. Windows/Linux 常见安装路径。

候选命令名在实现前实测确认，设计上支持配置：

```toml
[dreamina_cli]
path = "/usr/local/bin/dreamina"
extra_args = []
```

发现后必须验证：

- 可执行。
- 版本可读取或至少能运行 help。
- 能执行登录态检查命令。

## 5. 自动安装 adapter

建议接口：

```python
class InstallAdapter:
    platform: str
    def supported(self) -> bool: ...
    def plan(self) -> InstallPlan: ...
    def run(self, yes: bool) -> InstallResult: ...
```

`InstallResult` 需要区分：

- `installed`
- `already_installed`
- `blocked_manual_step`
- `unsupported_platform`
- `failed`

这样 agent 可以根据 JSON 决策，而不是从 stderr 猜。

## 6. 登录 adapter

建议接口：

```python
class AuthAdapter:
    def check(self) -> AuthState: ...
    def login(self) -> AuthResult: ...
```

`AuthState` 字段：

- `logged_in`
- `user_id`
- `display_name`
- `expires_at`
- `blocking_reason`
- `raw_stdout`
- `raw_stderr`

所有原始输出进入本地事件库前必须脱敏。

## 7. 数据目录与数据库

默认：

```text
~/.dreaminaq/
  config.toml
  db.sqlite
  blobs/
  results/
  logs/
  tmp/
```

核心表：

- `assets`：素材缓存。
- `tasks`：任务。
- `task_assets`：任务素材关系。
- `attempts`：提交/查询尝试。
- `results`：结果。
- `events`：事件。
- `environment_checks`：doctor、安装、登录、keep-awake 检查记录。

`environment_checks` 示例：

```sql
create table environment_checks (
  id text primary key,
  kind text not null,
  status text not null,
  detail_json text,
  created_at text not null
);
```

## 8. 队列策略

### 8.1 排序

同一优先级下按以下顺序：

1. `scheduled_at` 更早。
2. `queued_at` 更早。
3. `created_at` 更早。
4. `id` 字典序兜底。

这避免“后提交反而先执行”。

### 8.2 双车道

推荐策略名：

- `standard-only`
- `fast-only`
- `standard-then-fast`
- `parallel-standard-fast`

默认：

- 标准模型优先。
- 标准排队超过阈值或连续失败，允许 Fast 兜底。
- Fast 成功后，标准任务如果一直未开始，可按配置取消、继续等待或保留记录。

### 8.3 并发探测

- 不要多个任务同时疯狂探测。
- 每个车道有独立冷却时间。
- 远端并发占用时记录 `remote_busy`，下一次只探测车道，不批量重试所有任务。

## 9. 夜间保活

第一版 macOS 必须实装：

```text
worker --keep-awake
  -> 队列有未完成任务
  -> 启动 caffeinate -dimsu -w <worker_pid> 或等价方式
  -> 周期性 heartbeat 写入事件
  -> 队列完成/退出时释放
```

状态输出需要包含：

```json
{
  "keep_awake": {
    "active": true,
    "provider": "macos_caffeinate",
    "started_at": "2026-07-06T02:00:00+08:00",
    "pid": 12345
  }
}
```

Windows/Linux：

- 第一版可以先返回 `supported=false` 或 `adapter_pending`。
- 文档必须清楚说明 macOS 已支持，其他平台支持程度。

## 10. skill 设计

`skills/dreamina-queue/SKILL.md` 应包含：

- 什么时候使用：需要即梦视频批量生成、排队、结果查询。
- 先跑 `dreaminaq doctor --json`。
- 缺 CLI 时跑 `dreaminaq install-cli` 或提示用户确认。
- 未登录时跑 `dreaminaq login`，等待用户完成官方登录。
- 提交任务必须加 `--json`。
- 长任务建议启动 `worker --keep-awake`。
- 查询结果使用 `status/history/result --json`。
- 不直接调用官方即梦 CLI，除非 `dreaminaq doctor` 明确要求人工排查。

## 11. GitHub 发布要求

README 最少包含：

- 项目定位：即梦 CLI 的本地队列增强层。
- 安装命令。
- 快速开始。
- 夜间 worker 示例。
- agent/skill 使用方式。
- 数据目录说明。
- 安全边界：不保存密码，不绕过官方登录。
- 常见问题：CLI 未安装、未登录、远端并发占用、电脑睡眠。

CI 最少包含：

- Python lint/type optional。
- 单元测试。
- fake Dreamina CLI 集成测试。
- macOS keep-awake provider 的可用性测试或 mock 测试。

## 12. 迁移策略

第一版不迁移当前 App 数据。

后续可以提供：

```bash
dreaminaq import-scheduler ~/.dreamina-scheduler
```

但这不是 P0。现在优先保证独立项目能被其他人安装和跑通。

## 13. 实施顺序

推荐分阶段：

1. 新建独立 repo scaffold：README、pyproject、CLI skeleton、skill skeleton。
2. 实现 `doctor`：发现 CLI、检查登录、检查 keep-awake。
3. 实现 `install-cli` adapter：macOS 优先。
4. 实现 `login` adapter。
5. 实现 SQLite schema、素材缓存、`submit/status/history`。
6. 实现 `worker/once`、锁、官方 CLI subprocess、事件记录。
7. 实现 standard/Fast 策略和远端并发探测冷却。
8. 实现 `result` 和结果落盘。
9. 补齐测试、README、GitHub Actions。
10. 真实账号 smoke test，回填 `acceptance.md` 或 `result.md`。

## 14. 主要风险与防线

- 官方 CLI 安装方式变化：installer adapter 必须可配置，不把命令写死到不可维护。
- 登录态检测不稳定：保存原始脱敏输出，允许用户手动指定 `--assume-authenticated` 作为临时排查开关。
- 长跑进程异常退出：worker heartbeat 写事件，下一次启动能恢复未完成任务。
- 数据损坏：SQLite 事务、schema migration、定期 backup。
- 公开日志泄密：默认脱敏 token、Cookie、本机绝对路径可按配置隐藏。
