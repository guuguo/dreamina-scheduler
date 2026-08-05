import { readFileSync } from 'node:fs';
import { strict as assert } from 'node:assert';
import { test } from 'node:test';

const mainSource = readFileSync(new URL('./main.jsx', import.meta.url), 'utf8');
const indexSource = readFileSync(new URL('../index.html', import.meta.url), 'utf8');
const stylesSource = readFileSync(new URL('./styles.css', import.meta.url), 'utf8');
const tauriConfig = JSON.parse(readFileSync(new URL('../src-tauri/tauri.conf.json', import.meta.url), 'utf8'));
const tauriSource = readFileSync(new URL('../src-tauri/src/lib.rs', import.meta.url), 'utf8');

test('webview is throttled without full suspension and uses the app shell color', () => {
  const mainWindow = tauriConfig.app.windows[0];

  assert.equal(mainWindow.backgroundThrottling, 'throttle');
  assert.equal(mainWindow.backgroundColor, '#F4F7FC');
  const windowBarStyles = stylesSource.match(/\.window-bar\s*\{[^}]*\}/)?.[0] || '';
  assert.doesNotMatch(windowBarStyles, /backdrop-filter/);
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

test('large state reads and task mutations use Tauri async command dispatch', () => {
  for (const command of [
    'get_app_state',
    'pause_task_command',
    'pause_tasks_command',
    'resume_task_command',
    'reschedule_task_command',
    'delete_task_command',
    'delete_tasks_command',
    'record_lifecycle_event_command',
    'save_task_draft_command',
    'create_task_command',
    'update_task_command',
    'create_role_command',
    'update_settings_command',
    'process_queue_command',
  ]) {
    assert.match(tauriSource, new RegExp(`#\\[tauri::command\\(async\\)\\]\\n    pub fn ${command}`));
  }
});

test('application exit releases its macOS keep-awake assertion', () => {
  assert.match(
    tauriSource,
    /RunEvent::ExitRequested[\s\S]*?state::<keep_awake::KeepAwakeGuard>\(\)[\s\S]*?\.release\(\)/,
  );
});

test('task prompt editor is loaded only when the create view needs it', () => {
  assert.doesNotMatch(mainSource, /import PromptMentionEditor from/);
  assert.match(mainSource, /React\.lazy\(\(\) => import\('\.\/components\/PromptMentionEditor\.jsx'\)\)/);
  assert.match(mainSource, /<React\.Suspense/);
});
