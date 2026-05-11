import test from 'node:test';
import assert from 'node:assert/strict';
import { readFileSync } from 'node:fs';
import { dirname, join } from 'node:path';
import { fileURLToPath } from 'node:url';

const srcDir = dirname(fileURLToPath(import.meta.url));

function readSource(relativePath) {
  return readFileSync(join(srcDir, relativePath), 'utf8');
}

test('settings page no longer renders the future roadmap sidebar', () => {
  const mainSource = readSource('main.jsx');
  const stylesSource = readSource('styles.css');

  assert.equal(mainSource.includes('未来功能路线图'), false);
  assert.equal(mainSource.includes('settings-roadmap'), false);
  assert.equal(mainSource.includes('roadmapItems'), false);
  assert.equal(stylesSource.includes('settings-roadmap'), false);
  assert.equal(stylesSource.includes('roadmap-card'), false);
});

test('settings page renders an explicit save settings submit button', () => {
  const mainSource = readSource('main.jsx');
  const settingsViewSource = mainSource.match(/function SettingsView\([\s\S]*?\n}\n\nfunction Metric/)?.[0] || '';

  assert.match(settingsViewSource, /type="submit"[\s\S]*保存设置/);
});
