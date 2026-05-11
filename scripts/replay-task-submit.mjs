import fs from 'node:fs';
import os from 'node:os';
import path from 'node:path';
import { spawnSync } from 'node:child_process';

const titleQuery = process.argv[2];
const mode = process.argv[3] || 'full'; // 'full' | 'short'
if (!titleQuery) {
  console.error('用法: node scripts/replay-task-submit.mjs <任务标题片段> [full|short]');
  process.exit(2);
}

const statePath = path.join(os.homedir(), '.dreamina-scheduler', 'state.json');
const data = JSON.parse(fs.readFileSync(statePath, 'utf8'));
const task = (data.tasks || []).find((t) => (t.title || '').includes(titleQuery));
if (!task) {
  console.error(`找不到任务，标题包含 ${titleQuery}`);
  process.exit(2);
}

const args = (task.command_preview || []).slice();
if (!args.length) {
  console.error('任务没有 command_preview');
  process.exit(2);
}

if (mode === 'short') {
  for (let i = 0; i < args.length; i += 1) {
    if (args[i].startsWith('--prompt=')) {
      args[i] = '--prompt=测试一段 15 秒卡通短片，妞妞在客厅里玩积木。';
      break;
    }
  }
}

const promptArg = args.find((a) => a.startsWith('--prompt=')) || '';
console.log(`task: ${task.title}`);
console.log(`mode: ${mode}`);
console.log(`prompt length (chars): ${promptArg.length - '--prompt='.length}`);
console.log(`args count: ${args.length}`);

const start = Date.now();
const result = spawnSync('dreamina', args, { encoding: 'utf8' });
const ms = Date.now() - start;
console.log(`exit code: ${result.status}`);
console.log(`signal: ${result.signal || ''}`);
console.log(`elapsed_ms: ${ms}`);
console.log(`stdout (${(result.stdout || '').length} bytes):`);
console.log(result.stdout || '<empty>');
console.log(`stderr (${(result.stderr || '').length} bytes):`);
console.log(result.stderr || '<empty>');
