import { strict as assert } from 'node:assert';
import { readFileSync } from 'node:fs';
import { test } from 'node:test';

const source = readFileSync(new URL('./components/LaneStrip.jsx', import.meta.url), 'utf8');
const styles = readFileSync(new URL('./styles.css', import.meta.url), 'utf8');

test('lane strip uses one shared dispatch pool instead of model-based local queues', () => {
  assert.match(source, /共享待调度池/);
  assert.match(source, /getSharedWaitingTasks/);
  assert.doesNotMatch(source, /getLaneLocalTasks/);
  assert.doesNotMatch(source, />本地队列</);
});

test('shared pool modal describes dynamic lane assignment', () => {
  assert.match(source, /任一启用车道空闲后动态分配/);
  assert.match(source, /仅展示排队中和等待重试的任务/);
});

test('generation stats supports time and model filters with completed task navigation', () => {
  assert.match(source, /近 7 天/);
  assert.match(source, /data-model=\{key\}/);
  assert.match(source, /rangeKey/);
  assert.match(source, /modelKind/);
  assert.match(source, /onSelectTask\(record\.taskId\)/);
  assert.match(source, /标准模型/);
  assert.match(source, /Fast 模型/);
  assert.match(source, /selectChartDay\(day\)/);
  assert.match(source, /aria-pressed=\{highlightedDayKey === day\.key\}/);
  assert.match(source, /selectedDay\.records/);
});

test('generation stats keeps its overview height and five-column rows inside long result lists', () => {
  assert.match(styles, /\.generation-stats-stage\s*\{[\s\S]*?flex:\s*0 0 210px/);
  assert.match(styles, /\.lane-task-row\.generation-record-row\s*\{[\s\S]*?grid-template-columns:\s*46px minmax\(0, 1fr\) 82px 48px 18px/);
  assert.match(styles, /\.generation-record-list\s*\{[\s\S]*?flex:\s*1 1 auto/);
  assert.match(styles, /\.generation-sparkline button\.active > b\s*\{/);
});

test('lane performance shows real 24-hour occupancy and clickable seven-day speed hours', () => {
  assert.doesNotMatch(source, /占用时间线（最近 60 分钟）/);
  assert.match(source, /近 24 小时任务占用/);
  assert.match(source, /近 7 天速度 · 按开始小时/);
  assert.match(source, /buildLanePerformance/);
  assert.match(source, /onSelectSpeedHour/);
  assert.match(source, /PerformanceHourModal/);
  assert.match(source, /同车道、同视频秒数的中位耗时为基线/);
  assert.match(styles, /\.lane-speed-grid\s*\{[\s\S]*?grid-template-columns:\s*repeat\(24/);
  assert.match(styles, /\.lane-occupancy-segment\s*\{[\s\S]*?position:\s*absolute/);
  assert.doesNotMatch(styles, /occupancy-pulse/);
  assert.match(styles, /\.lane-task-row\.performance-record-row\s*\{/);
});
