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
