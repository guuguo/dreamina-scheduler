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

test('image generation backend commands remain registered without page UI actions', () => {
  for (const command of [
    'download_imagegen_image_command',
    'retry_query_image_task_command',
    'regenerate_image_command',
  ]) {
    assert.doesNotMatch(source, new RegExp(`invoke\\('${command}'`));
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

test('queue center uses one unified batch queue action', () => {
  assert.doesNotMatch(source, /const openQueueMode = \(\) =>/);
  assert.doesNotMatch(source, /scheduleModal\.mode === 'queue'/);
  assert.doesNotMatch(source, /> 排队模式/);
  assert.match(source, /const openBatchSchedule = \(\) =>/);
  assert.doesNotMatch(source, /allowAlternatingFastQueue/);
  assert.match(source, /invoke\('queue_tasks_with_batch_schedule_command'/);
  assert.match(source, /批量排队/);
  assert.doesNotMatch(source, /交叉 Fast 模型/);
  assert.doesNotMatch(source, /改用 Fast/);
  assert.match(source, /确认排队/);
  assert.match(source, /const \[scheduleMode, setScheduleMode\] = useState\('immediate'\);/);
  assert.match(source, /const \[intervalMinutes, setIntervalMinutes\] = useState\(0\);/);
  assert.match(source, /alternateFastModel: false/);
});

test('queue center keeps actions in the right detail column and exposes lane switches', () => {
  assert.match(source, /className="qc-detail-column"/);
  assert.match(source, /onToggleLane=\{handleToggleLane\}/);
  assert.match(source, /invoke\('set_lane_enabled_command'/);
  assert.match(source, /> 排队</);
});

test('queue center exposes two-level queue priority controls', () => {
  assert.match(source, /invoke\('set_task_queue_priority_command'/);
  assert.match(source, /className="qc-priority-picker"/);
  assert.match(source, /★★ 下一位/);
  assert.match(source, /★ 第二优先/);
  assert.match(source, /taskPriorities/);
});

test('recent history exposes one unified show-all records control', () => {
  assert.match(source, /查看全部记录/);
  assert.match(source, /setRecordsModal\(\{/);
  assert.match(source, /<TaskRecordsModal/);
  assert.match(source, /function TaskRecordsModal/);
  assert.doesNotMatch(source, /全部查询/);
});

test('queue detail primary actions always expose task deletion', () => {
  const detailActions = source.match(/<div className="qc-selected-actions qc-selected-actions-stack">[\s\S]*?<\/div>/)?.[0] || '';
  assert.doesNotMatch(detailActions, /canDeleteTask\(selectedTask\)/);
  assert.match(detailActions, /handleDeleteTask\(selectedTask\)/);
  assert.match(detailActions, /删除任务/);
  assert.doesNotMatch(source, /className="qc-detail-actions"/);
});

test('main navigation icons are imported before use', () => {
  const lucideImport = source.match(/import \{[\s\S]*?\} from 'lucide-react';/)?.[0] || '';
  assert.match(source, /icon: ListChecks/);
  assert.match(lucideImport, /\bListChecks\b/);
});

test('unused logs and image generation pages are removed from the main UI', () => {
  assert.doesNotMatch(source, /id: 'logs'/);
  assert.doesNotMatch(source, /id: 'imagegen'/);
  assert.doesNotMatch(source, /<LogsView/);
  assert.doesNotMatch(source, /function LogsView/);
  assert.doesNotMatch(source, /<ImageGenView/);
  assert.doesNotMatch(source, /function ImageGenView/);
});

test('queue center shows all tasks without rarely used toolbar actions', () => {
  assert.doesNotMatch(source, /STATUS_TABS/);
  assert.doesNotMatch(source, /statusTab/);
  assert.doesNotMatch(source, /setStatusTab/);
  assert.doesNotMatch(source, /清空已完成/);
  assert.doesNotMatch(source, /执行策略/);
  assert.doesNotMatch(source, /运行一次/);
});

test('role library is grid-only and exposes series plus disabled editing', () => {
  assert.doesNotMatch(source, /roleSearchQuery/);
  assert.doesNotMatch(source, /roleActiveTab/);
  assert.doesNotMatch(source, /roleViewMode/);
  assert.doesNotMatch(source, /role-page-filter/);
  assert.doesNotMatch(source, /role-view-toggle/);
  assert.match(source, /roleForm\.series/);
  assert.match(source, /roleForm\.disabled/);
  assert.match(source, /停用角色/);
});
