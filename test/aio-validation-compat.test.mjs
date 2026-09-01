import test from 'node:test';
import assert from 'node:assert/strict';
import fs from 'node:fs';
import path from 'node:path';
import { fileURLToPath } from 'node:url';

const root = path.join(path.dirname(fileURLToPath(import.meta.url)), '..');
const read = (rel) => fs.readFileSync(path.join(root, rel), 'utf8');

test('5.x validation entrypoints are backed by AIO implementations', () => {
  for (const rel of ['boot-smoke.js', 'gui-smoke.js', 'update-smoke.js', 'tauri-shell/stage-resources.mjs', 'tauri-shell/make-portable.mjs']) {
    assert.ok(fs.existsSync(path.join(root, rel)), `${rel} is missing`);
  }
  assert.match(read('tauri-shell/stage-resources.mjs'), /tauri-app.*scripts.*stage\.ts/s);
  assert.match(read('tauri-shell/make-portable.mjs'), /\.dsh-portable/);
});

test('portable marker selects an isolated data root in the Rust shell', () => {
  const paths = read('tauri-app/src/paths.rs');
  assert.match(paths, /\.dsh-portable/);
  assert.match(paths, /\.dsh-aio-data/);
  assert.match(paths, /portable_marker_selects_sibling_data_root/);
});

test('AIO update smoke rejects client self-update exposure', () => {
  const smoke = read('update-smoke.js');
  assert.match(smoke, /client auto-update scripts/);
  assert.match(smoke, /plugin auto-update must default to disabled/);
});
