import { readFileSync } from 'node:fs';
import { strict as assert } from 'node:assert';
import { test } from 'node:test';

const source = readFileSync(new URL('./main.jsx', import.meta.url), 'utf8');
const tauriSource = readFileSync(new URL('../src-tauri/src/lib.rs', import.meta.url), 'utf8');

test('QueueView receives execution-record operation props used by execution history actions', () => {
  const queueViewMount = source.match(/<QueueView[\s\S]*?\/>/)?.[0] || '';
  const queueViewSignature = source.match(/function QueueView\(\{[\s\S]*?\}\) \{/)?.[0] || '';

  assert.match(queueViewMount, /pendingExecutionOps=\{pendingExecutionOps\}/);
  assert.match(queueViewMount, /queryExecutionRecord=\{queryExecutionRecord\}/);
  assert.match(queueViewSignature, /pendingExecutionOps = \{\}/);
  assert.match(queueViewSignature, /queryExecutionRecord/);
});

test('image generation history actions are registered tauri commands', () => {
  for (const command of [
    'download_imagegen_image_command',
    'retry_query_image_task_command',
    'regenerate_image_command',
  ]) {
    assert.match(source, new RegExp(`invoke\\('${command}'`));
    assert.match(tauriSource, new RegExp(`commands::${command}`));
  }
});

test('sidebar exposes disabled-aware latest output folder shortcut', () => {
  assert.match(source, /className="sidebar-output-button"/);
  assert.match(source, /disabled=\{!latestSuccessfulResultPath\}/);
  assert.match(source, /产出文件夹/);
  assert.match(source, /invoke\('open_result_dir_command', \{ path: latestSuccessfulResultPath \}\)/);
});

test('main app no longer includes README screenshot fixture mode', () => {
  assert.doesNotMatch(source, /readme-screenshot/);
  assert.doesNotMatch(source, /readmeScreenshot/);
});

test('queue center exposes direct batch queue action', () => {
  assert.match(source, /const openQueueMode = \(\) =>/);
  assert.match(source, /allowAlternatingFastQueue: canUseAlternatingFastQueue\(queueTasks\)/);
  assert.match(source, /scheduleModal\.mode === 'queue'/);
  assert.match(source, /invoke\('queue_tasks_with_model_strategy_command'/);
  assert.match(source, /排队模式/);
  assert.match(source, /交叉 Fast 模型/);
  assert.match(source, /确认排队/);
});
