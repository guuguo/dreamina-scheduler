# 执行结果：即梦队列独立技能项目化首版

## 状态

- 执行状态：partial-implemented
- 执行时间：2026-07-07
- 独立项目目录：`/Volumes/Seamless SSD/dev/ai_video/dreamina-queue`

## 已完成

- 创建独立 Python 项目 `dreamina-queue`。
- 新增 CLI 入口 `dreaminaq`。
- 新增核心命令：
  - `doctor`
  - `install-cli`
  - `login`
  - `submit`
  - `once`
  - `worker`
  - `status`
  - `history`
  - `task`
  - `result`
  - `list`
- 实现本地 SQLite store。
- 实现素材 SHA-256 内容寻址缓存。
- 实现 `idempotency-key` 幂等入队。
- 实现即梦 CLI 自动发现：
  - `DREAMINA_CLI_PATH`
  - `~/.dreaminaq/config.toml`
  - `PATH`
  - 常见路径 `~/.local/bin/dreamina`
- 实现 `doctor --json` 环境诊断。
- 实现登录态检测：通过官方 `dreamina user_credit`。
- 实现登录触发：`dreaminaq login` 调用官方 OAuth Device Flow。
- 实现官方安装脚本 adapter：默认脚本 `https://jimeng.jianying.com/cli`，`--yes` 后执行。
- 实现 macOS keep-awake 能力检测和 `worker --keep-awake` 的 `caffeinate` provider。
- 实现 macOS LaunchAgent 服务命令：
  - `dreaminaq service install`
  - `dreaminaq service status`
  - `dreaminaq service uninstall`
- 增加 worker stale lock 恢复：异常退出后遗留的旧 PID lock 会被下一轮清理。
- 增加付费提交保险丝：
  - 单个 task 默认最多真实提交 1 次。
  - 同一个 `request_hash` 默认 24 小时内最多真实提交 1 次。
  - 已存在 `submit_id` 时，worker 会切换到查询模式，不再重新提交。
- 实现 standard/Fast 策略入口：
  - `standard-then-fast`
  - 超过 `fast_after_seconds` 后将 `seedance2.0` 切为 `seedance2.0fast`
- 新增 agent skill：`skills/dreamina-queue/SKILL.md`。
- 新增 README、CHANGELOG、LICENSE、examples、GitHub Actions CI。
- 新增 fake Dreamina CLI 测试，不消耗真实生成额度。

## 验证结果

| 检查 | 结果 | 证据 |
| --- | --- | --- |
| Python 语法检查 | pass | `python3 -m compileall src` |
| 单元测试 | pass | `PYTHONPATH=src python3 -m unittest discover -s tests -v`，12 tests OK |
| CLI help | pass | `PYTHONPATH=src python3 -m dreamina_queue.cli --help` |
| service CLI help | pass | `PYTHONPATH=src python3 -m dreamina_queue.cli service --help` |
| service plist 冒烟 | pass | 临时 `HOME` 下 `service install --no-load --json` 生成 LaunchAgent plist |
| 真实 Dreamina CLI discovery | pass | `doctor --json --no-auth-check` 找到 `/Users/guodeqing/.local/bin/dreamina`，版本 `2a20fff-dirty` |
| fake CLI 队列闭环 | pass | `submit -> once -> status`，任务进入 `success` |
| fake CLI 重复提交保险丝 | pass | 两个相同 request 只产生 1 次真实 submit，第二个进入 `blocked`，`safety.total_submit_attempts = 1` |
| venv editable install | blocked | pip 下载 build dependency `setuptools` 时网络超时；非代码错误 |

## 重要插曲与修复

执行过程中，安装测试最初没有隔离 `HOME`，导致 fake CLI 被复制到真实 `/Users/guodeqing/.local/bin/dreamina`。

已经完成修复：

- 测试环境现在设置临时 `HOME`。
- 新增 `DREAMINAQ_NO_DEFAULT_CLI_DISCOVERY=1` 用于隔离 discovery 测试。
- 已通过官方脚本 `https://jimeng.jianying.com/cli` 恢复真实 Dreamina CLI。
- 恢复后验证：
  - 文件类型：Mach-O 64-bit executable arm64
  - 版本：`2a20fff-dirty`

## 未完成 / 后续

- 尚未真实提交 Dreamina 付费生成任务。
- 尚未做长时间夜间 worker 实跑。
- 尚未在真实 `~/Library/LaunchAgents` 执行 `service install` 加载服务；本轮只做临时 HOME 的 `--no-load` 冒烟。
- 尚未推送 GitHub 仓库。
- venv/pip 安装 smoke 受外网依赖影响，需在网络稳定时重跑：

```bash
python3 -m venv .venv
.venv/bin/python -m pip install -e .
.venv/bin/dreaminaq --help
```

如果本地 venv 没有 `setuptools` 且网络不稳定，可以临时用：

```bash
PYTHONPATH=src python3 -m dreamina_queue.cli --help
```
