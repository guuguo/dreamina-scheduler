import fs from 'node:fs';
import os from 'node:os';
import path from 'node:path';

const titleQuery = process.argv[2];
const submitId = process.argv[3];
if (!titleQuery || !submitId) {
  console.error('用法: node scripts/inject-submit-id.mjs <任务标题片段> <submit_id>');
  process.exit(2);
}

const statePath = path.join(os.homedir(), '.dreamina-scheduler', 'state.json');
const original = fs.readFileSync(statePath, 'utf8');
const data = JSON.parse(original);

const task = (data.tasks || []).find((t) => (t.title || '').includes(titleQuery));
if (!task) {
  console.error(`找不到任务，标题包含 ${titleQuery}`);
  process.exit(2);
}

const now = new Date().toISOString();
const backup = `${statePath}.bak.${Date.now()}`;
fs.writeFileSync(backup, original);
console.log(`backup -> ${backup}`);

task.submit_id = submitId;
task.submitted_at = now;
task.status = 'querying';
task.last_error = '';
task.queue_info = null;
task.finished_at = '';
task.updated_at = now;
task.result_paths = [];
task.result_urls = [];
task.auto_query_stopped = false;
task.consecutive_no_result_queries = 0;
task.last_auto_query_at = null;
task.server_error_retry_count = 0;

const records = task.execution_records || [];
const targetIndex = records.length - 1;
const baseRecord = targetIndex >= 0 ? records[targetIndex] : null;
const updated = {
  ...(baseRecord || {}),
  id: baseRecord?.id || `exec_${Date.now().toString(16)}`,
  submit_id: submitId,
  status: 'querying',
  started_at: now,
  finished_at: '',
  command_preview: baseRecord?.command_preview || task.command_preview || [],
  input_snapshot: baseRecord?.input_snapshot || {
    prompt: task.prompt || '',
    image_asset_ids: task.image_asset_ids || [],
    audio_asset_ids: task.audio_asset_ids || [],
    role_ids: task.role_ids || [],
    manual_mention_ids: task.manual_mention_ids || [],
    auto_match_roles: task.auto_match_roles ?? true,
    params: task.params || {},
    temp_image_asset_ids: task.temp_image_asset_ids || [],
  },
  query_records: [],
  result_paths: [],
  result_urls: [],
  error_kind: '',
  error_detail: '',
};

if (targetIndex >= 0) {
  records[targetIndex] = updated;
} else {
  records.push(updated);
}
task.execution_records = records;

fs.writeFileSync(statePath, `${JSON.stringify(data, null, 2)}\n`);
console.log(`updated task: ${task.title} (${task.id})`);
console.log(`status: ${task.status}`);
console.log(`submit_id: ${task.submit_id}`);
console.log(`execution_records: ${task.execution_records.length}`);
