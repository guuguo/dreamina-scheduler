import { strict as assert } from 'node:assert';
import { test } from 'node:test';
import { buildLanePerformance } from './lane-performance-utils.js';

const nowMs = new Date('2026-07-24T12:00:00+08:00').getTime();

function task(id, records, params = { model_version: 'seedance2.0', duration: 15 }) {
  return {
    id,
    title: `任务 ${id}`,
    status: 'succeeded',
    params,
    execution_records: records,
  };
}

function record(id, startedAt, finishedAt, duration = 15, modelVersion = 'seedance2.0') {
  return {
    id,
    status: 'succeeded',
    started_at: startedAt,
    finished_at: finishedAt,
    input_snapshot: { params: { duration, model_version: modelVersion } },
    result_urls: [`https://example.com/${id}.mp4`],
  };
}

test('buildLanePerformance renders a six-hour task as a continuous 24-hour occupancy segment', () => {
  const performance = buildLanePerformance([
    task('long', [record('long-record', '2026-07-24T02:00:00+08:00', '2026-07-24T08:00:00+08:00')]),
  ], 'standard', nowMs);

  assert.equal(performance.occupancy.length, 1);
  assert.equal(Math.round(performance.occupancy[0].widthPercent), 25);
  assert.equal(performance.occupancy[0].elapsedMs, 6 * 60 * 60 * 1000);
});

test('speed heatmap compares records against the same video-duration baseline', () => {
  const performance = buildLanePerformance([
    task('15-fast', [record('a', '2026-07-22T02:00:00+08:00', '2026-07-22T04:00:00+08:00', 15)]),
    task('15-slow', [record('b', '2026-07-23T05:00:00+08:00', '2026-07-23T11:00:00+08:00', 15)]),
    task('5-normal', [record('c', '2026-07-22T08:00:00+08:00', '2026-07-22T09:00:00+08:00', 5)]),
  ], 'standard', nowMs);

  assert.equal(performance.hours[2].tone, 'faster');
  assert.equal(performance.hours[5].tone, 'slower');
  assert.equal(performance.hours[8].tone, 'steady');
  assert.equal(performance.hours[5].records[0].videoDuration, 15);
});

test('standard and fast lanes keep independent performance records', () => {
  const performance = buildLanePerformance([
    task('standard', [record('standard-record', '2026-07-23T03:00:00+08:00', '2026-07-23T05:00:00+08:00')]),
    task('fast', [record('fast-record', '2026-07-23T03:00:00+08:00', '2026-07-23T04:00:00+08:00', 15, 'seedance2.0fast')],
      { model_version: 'seedance2.0fast', duration: 15 }),
  ], 'fast', nowMs);

  assert.equal(performance.hours[3].count, 1);
  assert.equal(performance.hours[3].records[0].taskId, 'fast');
});
