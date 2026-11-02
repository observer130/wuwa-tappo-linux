import test from 'node:test';
import assert from 'node:assert/strict';
import { readdir, readFile } from 'node:fs/promises';

const root = new URL('../', import.meta.url);
const contract = JSON.parse(await readFile(new URL('tests/contracts.json', root), 'utf8'));

test('every official runtime command has a Linux handler', async () => {
  const modules = ['lib', 'windows', 'storage', 'input', 'system', 'socket', 'shortcuts'];
  const implementation = (await Promise.all(
    modules.map(name => readFile(new URL(`src-tauri/src/${name}.rs`, root), 'utf8')),
  )).join('\n');
  const missing = contract.commands
    .filter(item => item.kind === 'application' && item.scopes.includes('application-runtime'))
    .filter(item => !implementation.includes(`"${item.name}"`) && !implementation.includes(`fn ${item.name}(`))
    .map(item => item.name);
  assert.deepEqual(missing, []);
});

test('taskbar sync and maintenance exit are handled without a native menu', async () => {
  const windows = await readFile(new URL('src-tauri/src/windows.rs', root), 'utf8');
  for (const command of ['set_main_skip_taskbar', 'ensure_main_taskbar_icon', 'exit_maintenance_app', 'report_webview_metrics']) {
    assert.ok(windows.includes(`"${command}"`), `${command} is not registered`);
  }
  assert.match(windows, /set_skip_taskbar\(skip\)/);
  assert.doesNotMatch(windows, /\[cfg\(target_os = "macos"\)\]/);
});

test('injected runtime scripts are Linux builds with platform-neutral file names', async () => {
  const windows = await readFile(new URL('src-tauri/src/windows.rs', root), 'utf8');
  assert.match(windows, /scripts\/websocket\.js/);
  assert.match(windows, /scripts\/platform\.js/);
  const platform = await readFile(new URL('scripts/platform.js', root), 'utf8');
  assert.match(platform, /wuwa-input-status/);
  assert.doesNotMatch(platform, /mac-input-status/);
  const sources = await readdir(new URL('src-tauri/src/', root));
  assert.deepEqual(sources.filter(name => name.endsWith('.mac.rs')), []);
});
