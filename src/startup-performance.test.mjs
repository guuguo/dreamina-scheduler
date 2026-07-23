import { readFileSync } from 'node:fs';
import { strict as assert } from 'node:assert';
import { test } from 'node:test';

const mainSource = readFileSync(new URL('./main.jsx', import.meta.url), 'utf8');
const indexSource = readFileSync(new URL('../index.html', import.meta.url), 'utf8');
const tauriConfig = JSON.parse(readFileSync(new URL('../src-tauri/tauri.conf.json', import.meta.url), 'utf8'));

test('webview is throttled without full suspension and uses the app shell color', () => {
  const mainWindow = tauriConfig.app.windows[0];

  assert.equal(mainWindow.backgroundThrottling, 'throttle');
  assert.equal(mainWindow.backgroundColor, '#F4F7FC');
});

test('html paints a startup shell before the JavaScript bundle is ready', () => {
  assert.match(indexSource, /<div id="boot-screen" class="boot-shell"/);
  assert.match(indexSource, /\.boot-shell/);
  assert.match(indexSource, /正在载入任务/);
  assert.match(mainSource, /document\.getElementById\('boot-screen'\)\?\.remove\(\)/);
});

test('foreground recovery coalesces focus events and refreshes only changed state', () => {
  assert.match(mainSource, /const resumeRefreshInFlightRef = useRef\(null\)/);
  assert.match(mainSource, /async function refreshStateIfChanged/);
  assert.match(mainSource, /if \(sig === lastStateSignatureRef\.current\) return false/);
  assert.match(mainSource, /function scheduleVisibleStateRefresh\(\)/);
  assert.match(mainSource, /if \(document\.hidden\) return/);
  assert.match(mainSource, /window\.requestAnimationFrame/);
  assert.doesNotMatch(mainSource, /const handleFocus = \(\) => \{[\s\S]*?refreshStateRef\.current\?\.\(\)/);
});

test('task prompt editor is loaded only when the create view needs it', () => {
  assert.doesNotMatch(mainSource, /import PromptMentionEditor from/);
  assert.match(mainSource, /React\.lazy\(\(\) => import\('\.\/components\/PromptMentionEditor\.jsx'\)\)/);
  assert.match(mainSource, /<React\.Suspense/);
});
