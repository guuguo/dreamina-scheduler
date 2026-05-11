import fs from 'node:fs';
import os from 'node:os';
import path from 'node:path';

const targets = process.argv.slice(2);
const statePath = path.join(os.homedir(), '.dreamina-scheduler', 'state.json');
const data = JSON.parse(fs.readFileSync(statePath, 'utf8'));

const tasks = (data.tasks || []).filter((task) => {
  const title = task.title || '';
  if (targets.length > 0) return targets.some((target) => title.includes(target));
  return task.status === 'failed';
});

console.log(`state: ${statePath}`);
console.log(`matched tasks: ${tasks.length}`);

for (const task of tasks) {
  console.log('\n=== TASK ===');
  console.log(`title: ${task.title || ''}`);
  console.log(`id: ${task.id || ''}`);
  console.log(`status: ${task.status || ''}`);
  console.log(`updated_at: ${task.updated_at || ''}`);
  console.log(`submit_id: ${task.submit_id || ''}`);
  console.log(`attempt_count: ${task.attempt_count ?? ''}`);
  console.log(`concurrency_retry_count: ${task.concurrency_retry_count ?? ''}`);
  console.log(`server_error_retry_count: ${task.server_error_retry_count ?? ''}`);
  console.log(`last_error: ${task.last_error || ''}`);

  const records = task.execution_records || [];
  console.log(`execution_records: ${records.length}`);
  for (const record of records) {
    console.log('\n  --- RECORD ---');
    console.log(`  id: ${record.id || ''}`);
    console.log(`  status: ${record.status || ''}`);
    console.log(`  submit_id: ${record.submit_id || ''}`);
    console.log(`  started_at: ${record.started_at || ''}`);
    console.log(`  finished_at: ${record.finished_at || ''}`);
    console.log(`  error_kind: ${record.error_kind || ''}`);
    console.log(`  error_detail: ${record.error_detail || ''}`);

    const queryRecords = record.query_records || [];
    console.log(`  query_records: ${queryRecords.length}`);
    for (const query of queryRecords) {
      console.log('    --- QUERY ---');
      console.log(`    status: ${query.status || ''}`);
      console.log(`    started_at: ${query.started_at || ''}`);
      console.log(`    finished_at: ${query.finished_at || ''}`);
      console.log(`    error_kind: ${query.error_kind || ''}`);
      console.log(`    error_detail: ${query.error_detail || ''}`);
      if (query.stdout) console.log(`    stdout: ${query.stdout.slice(0, 1000)}`);
      if (query.stderr) console.log(`    stderr: ${query.stderr.slice(0, 1000)}`);
    }
  }

  const attempts = task.attempts || [];
  console.log(`attempts: ${attempts.length}`);
  for (const attempt of attempts.slice(-5)) {
    console.log('\n  --- ATTEMPT ---');
    console.log(`  command_preview: ${(attempt.command_preview || []).join(' ')}`);
    console.log(`  status: ${attempt.status || ''}`);
    console.log(`  started_at: ${attempt.started_at || ''}`);
    console.log(`  finished_at: ${attempt.finished_at || ''}`);
    console.log(`  error_kind: ${attempt.error_kind || ''}`);
    console.log(`  error_detail: ${attempt.error_detail || ''}`);
    if (attempt.stdout) console.log(`  stdout: ${attempt.stdout.slice(0, 1000)}`);
    if (attempt.stderr) console.log(`  stderr: ${attempt.stderr.slice(0, 1000)}`);
  }
}

console.log('\n=== RELATED LOGS ===');
const taskIds = new Set(tasks.map((task) => task.id).filter(Boolean));
const taskTitles = new Set(tasks.map((task) => task.title).filter(Boolean));
const submitIds = new Set(tasks.map((task) => task.submit_id).filter(Boolean));
const relatedLogs = (data.logs || []).filter((log) => {
  const fields = [
    log.task_id,
    log.task_title,
    log.submit_id,
    log.message,
    log.detail,
    log.error_detail,
    log.raw_output,
    log.stdout,
    log.stderr,
  ].map((value) => String(value || ''));
  return fields.some((text) => {
    for (const value of taskIds) if (text.includes(value)) return true;
    for (const value of taskTitles) if (text.includes(value)) return true;
    for (const value of submitIds) if (text.includes(value)) return true;
    return false;
  });
}).slice(-50);

for (const log of relatedLogs) {
  console.log('\n--- LOG ---');
  console.log(`timestamp: ${log.timestamp || log.created_at || ''}`);
  console.log(`level: ${log.level || ''}`);
  console.log(`source: ${log.source || ''}`);
  console.log(`category: ${log.category || ''}`);
  console.log(`event_type: ${log.event_type || ''}`);
  console.log(`task_title: ${log.task_title || ''}`);
  console.log(`message: ${log.message || ''}`);
  if (log.error_detail) console.log(`error_detail: ${String(log.error_detail).slice(0, 1000)}`);
  if (log.detail) console.log(`detail: ${String(log.detail).slice(0, 1000)}`);
  if (log.raw_output) console.log(`raw_output: ${String(log.raw_output).slice(0, 1000)}`);
  if (log.stdout) console.log(`stdout: ${String(log.stdout).slice(0, 1000)}`);
  if (log.stderr) console.log(`stderr: ${String(log.stderr).slice(0, 1000)}`);
}
