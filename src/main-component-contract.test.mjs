import { readFileSync } from 'node:fs';
import { strict as assert } from 'node:assert';
import { test } from 'node:test';

const source = readFileSync(new URL('./main.jsx', import.meta.url), 'utf8');

test('QueueView receives execution-record operation props used by execution history actions', () => {
  const queueViewMount = source.match(/<QueueView[\s\S]*?\/>/)?.[0] || '';
  const queueViewSignature = source.match(/function QueueView\(\{[\s\S]*?\}\) \{/)?.[0] || '';

  assert.match(queueViewMount, /pendingExecutionOps=\{pendingExecutionOps\}/);
  assert.match(queueViewMount, /queryExecutionRecord=\{queryExecutionRecord\}/);
  assert.match(queueViewSignature, /pendingExecutionOps = \{\}/);
  assert.match(queueViewSignature, /queryExecutionRecord/);
});
